//! The suite.
//!
//! The centrepiece is [`Machine`], a model of the robot that applies actions and can be observed.
//! It exists so the two claims §33.3 makes about the reconciler can be *proved* rather than
//! asserted in prose:
//!
//! * **it converges from any starting state** — run the loop until the plan is empty, from starting
//!   states including ones nobody would arrange on purpose;
//! * **it is idempotent** — a converged machine produces an empty plan, and applying an empty plan
//!   changes nothing.
//!
//! Convergence is bounded rather than immediate, and deliberately so: removing a running package
//! takes two passes, because stopping it and removing it are different actions at different stages
//! and the second is only correct once the first has been observed. A reconciler that did both in
//! one pass would be acting on what it *intends* to be true rather than on what it observed, which
//! is the exact discipline §33.3 forbids.

use skynet_json::Json;

use crate::actual::{Actual, InstalledPackage, OsSlots, UnitState, UnitStatus};
use crate::desired::{
    DesiredPackage, DesiredState, PackageKind, ProfileLimits, ProfileProximity, RunState,
};
use crate::health::{
    readiness, Health, PackageHealth, Production, Readiness, DEFAULT_TOLERANCE_PERIODS,
};
use crate::lease::{LeaseDenial, LeaseTable, PreemptionOutcome};
use crate::reconcile::{plan, ActionKind, Gate, SafeState};
use crate::supervise::{CrashPolicy, RestartDecision, Supervision, SupervisionState};

// ── A model of the machine ───────────────────────────────────────────────────

/// Enough of a robot to apply a plan to and then observe.
///
/// Not a mock of an interface — a model of the *state*, because that is what the reconciler is a
/// function of. It applies actions the way the real system would, including the parts that make
/// convergence take more than one pass.
#[derive(Debug, Clone, Default)]
struct Machine {
    actual: Actual,
    safe: bool,
    applied: Vec<ActionKind>,
}

impl Machine {
    fn new(safe: bool) -> Self {
        Self {
            actual: Actual::default(),
            safe,
            applied: Vec::new(),
        }
    }

    fn safe_state(&self) -> SafeState {
        if self.safe {
            SafeState::Yes
        } else {
            SafeState::No
        }
    }

    /// One reconcile pass: plan, apply what is not gated, observe.
    ///
    /// The number returned is how many actions were applied, so a caller can tell a pass that did
    /// something from one that could only defer.
    fn pass(&mut self, desired: &DesiredState) -> usize {
        let plan = plan(desired, &self.actual);
        let ready: Vec<_> = plan
            .actions
            .iter()
            .filter(|action| action.gate.is_open(self.safe_state(), &self.actual))
            .cloned()
            .collect();
        for action in &ready {
            self.apply(&action.kind);
        }
        ready.len()
    }

    /// Run to convergence, or give up and say how far it got.
    fn converge(&mut self, desired: &DesiredState) -> usize {
        for round in 1..=8 {
            let applied = self.pass(desired);
            if applied == 0 {
                return round;
            }
        }
        panic!(
            "did not converge in 8 passes; still to do: {:?}",
            plan(desired, &self.actual).actions
        );
    }

    fn apply(&mut self, action: &ActionKind) {
        self.applied.push(action.clone());
        match action {
            ActionKind::StageOsImage { version } => {
                self.actual.os.staged = Some(version.clone());
            }
            ActionKind::ApplyOsImage { version } => {
                self.actual.os.active = version.clone();
                self.actual.os.staged = None;
            }
            ActionKind::FetchPackage { id, digest } => {
                self.actual.installed.retain(|entry| &entry.id != id);
                self.actual.installed.push(InstalledPackage {
                    id: id.clone(),
                    digest: digest.clone(),
                    activated: false,
                });
            }
            ActionKind::ActivatePackage { id, digest } => {
                for entry in &mut self.actual.installed {
                    if &entry.id == id {
                        entry.digest = digest.clone();
                        entry.activated = true;
                    }
                }
            }
            ActionKind::RemovePackage { id } => {
                self.actual.installed.retain(|entry| &entry.id != id);
                self.actual.units.retain(|unit| &unit.id != id);
            }
            ActionKind::ApplyProfile { digest, .. } => {
                self.actual.profile_digest = Some(digest.clone());
            }
            ActionKind::GrantCapability { id, token } => {
                self.actual.grants.push((id.clone(), token.clone()));
                self.actual.grants.sort();
            }
            ActionKind::RevokeCapability { id, token } => {
                self.actual
                    .grants
                    .retain(|(package, held)| !(package == id && held == token));
            }
            ActionKind::StartPackage { id } => {
                self.actual.units.retain(|unit| &unit.id != id);
                self.actual.units.push(UnitStatus {
                    id: id.clone(),
                    state: UnitState::Running,
                    restarts: 0,
                    last_exit: None,
                });
            }
            ActionKind::StopPackage { id } => {
                for unit in &mut self.actual.units {
                    if &unit.id == id {
                        unit.state = UnitState::Stopped;
                    }
                }
                // A stopped package holds no leases. The real supervisor does this too, and for the
                // same reason: a lease leaked by a stopped policy refuses the arm to its
                // replacement with no reason on any screen.
                self.actual.leases.retain(|(holder, _)| holder != id);
            }
            ActionKind::ReleaseLease { id, token } => {
                self.actual
                    .leases
                    .retain(|(holder, held)| !(holder == id && held == token));
            }
        }
    }
}

// ── Fixtures ─────────────────────────────────────────────────────────────────

fn package(id: &str, digest: &str, kind: PackageKind, state: RunState) -> DesiredPackage {
    DesiredPackage {
        id: id.to_string(),
        version: "1.0.0".to_string(),
        digest: digest.to_string(),
        kind,
        state,
        grants: Vec::new(),
        signature: None,
    }
}

fn desired(packages: Vec<DesiredPackage>) -> DesiredState {
    DesiredState {
        generation: 41,
        os_channel: "stable".to_string(),
        os_version: "1.0.0".to_string(),
        packages,
        profile: None,
    }
}

// ── The level-triggered claim ────────────────────────────────────────────────

#[test]
fn an_empty_robot_converges_on_the_whole_desired_state() {
    let want = desired(vec![
        package(
            "vendor/arm",
            "sha256:d1",
            PackageKind::Driver,
            RunState::Running,
        ),
        package(
            "openrobot/grasp",
            "sha256:p1",
            PackageKind::Policy,
            RunState::Running,
        ),
    ]);
    let mut machine = Machine::new(true);
    machine.converge(&want);

    assert_eq!(machine.actual.os.active, "1.0.0");
    assert!(machine.actual.is_running("vendor/arm"));
    assert!(machine.actual.is_running("openrobot/grasp"));
    assert!(plan(&want, &machine.actual).is_empty());
}

#[test]
fn a_converged_robot_plans_nothing_and_re_planning_changes_nothing() {
    // The idempotence claim in §33.3, stated as narrowly as it is true: the plan for a converged
    // machine is empty, and running the loop again does not make it non-empty.
    let want = desired(vec![package(
        "openrobot/grasp",
        "sha256:p1",
        PackageKind::Policy,
        RunState::Running,
    )]);
    let mut machine = Machine::new(true);
    machine.converge(&want);

    let before = machine.actual.clone();
    assert_eq!(machine.pass(&want), 0);
    assert_eq!(
        machine.actual, before,
        "a pass over a converged machine changed it"
    );
}

#[test]
fn it_converges_from_a_state_nobody_arranged() {
    // Half-installed, half-running, an image staged for a version nobody wants, a package the
    // desired state has never heard of, and a grant for a package that is not installed. This is
    // the case an edge-triggered supervisor cannot handle at all, because no edge will ever be
    // re-delivered for any of it.
    let want = desired(vec![
        package(
            "openrobot/grasp",
            "sha256:p2",
            PackageKind::Policy,
            RunState::Running,
        ),
        package(
            "openrobot/idle",
            "sha256:i1",
            PackageKind::Policy,
            RunState::Stopped,
        ),
    ]);

    let mut machine = Machine::new(true);
    machine.actual = Actual {
        observed_at_ms: 5_000,
        generation: 12,
        os: OsSlots {
            active: "0.9.0".to_string(),
            staged: Some("0.9.1".to_string()),
        },
        installed: vec![
            InstalledPackage {
                id: "openrobot/grasp".to_string(),
                digest: "sha256:p1".to_string(),
                activated: true,
            },
            InstalledPackage {
                id: "someone/forgotten".to_string(),
                digest: "sha256:f1".to_string(),
                activated: true,
            },
        ],
        units: vec![UnitStatus {
            id: "someone/forgotten".to_string(),
            state: UnitState::Running,
            restarts: 3,
            last_exit: Some(1),
        }],
        profile_digest: None,
        profile_limits: None,
        grants: vec![("ghost/gone".to_string(), "manipulation.grasp".to_string())],
        leases: vec![("ghost/gone".to_string(), "manipulation.grasp".to_string())],
    };

    machine.converge(&want);

    assert_eq!(machine.actual.os.active, "1.0.0");
    assert_eq!(machine.actual.os.staged, None);
    assert_eq!(
        machine
            .actual
            .installed("openrobot/grasp")
            .map(|p| p.digest.as_str()),
        Some("sha256:p2"),
        "the upgrade landed"
    );
    assert!(machine.actual.installed("someone/forgotten").is_none());
    assert!(
        machine.actual.grants.is_empty(),
        "a grant for a package nobody wants is revoked"
    );
    assert!(machine.actual.leases.is_empty(), "and its lease released");
    assert!(machine.actual.is_running("openrobot/grasp"));
    assert!(!machine.actual.is_running("openrobot/idle"));
}

#[test]
fn removing_a_running_package_takes_two_passes_and_that_is_correct() {
    // Stated as a test rather than left as a surprise. The removal is only emitted once the stop
    // has been *observed*, because a reconciler that removed a package it had merely decided to
    // stop would be acting on its own intentions — the discipline §33.3 forbids.
    let want = desired(Vec::new());
    let mut machine = Machine::new(true);
    machine.actual.os.active = "1.0.0".to_string();
    machine.actual.installed.push(InstalledPackage {
        id: "openrobot/grasp".to_string(),
        digest: "sha256:p1".to_string(),
        activated: true,
    });
    machine.actual.units.push(UnitStatus {
        id: "openrobot/grasp".to_string(),
        state: UnitState::Running,
        restarts: 0,
        last_exit: None,
    });

    assert_eq!(machine.pass(&want), 1, "the first pass only stops it");
    assert!(machine.actual.installed("openrobot/grasp").is_some());
    assert_eq!(machine.pass(&want), 1, "the second removes it");
    assert!(machine.actual.installed("openrobot/grasp").is_none());
    assert_eq!(machine.pass(&want), 0);
}

// ── The safe-state barrier ───────────────────────────────────────────────────

#[test]
fn staging_is_free_and_applying_waits() {
    let want = desired(Vec::new());
    let actual = Actual {
        os: OsSlots {
            active: "0.9.0".to_string(),
            staged: None,
        },
        ..Actual::default()
    };

    let staged = plan(&want, &actual);
    assert!(matches!(
        staged.actions[0].kind,
        ActionKind::StageOsImage { .. }
    ));
    assert_eq!(
        staged.actions[0].gate,
        Gate::Open,
        "fetching an image touches nothing"
    );

    let with_staged = Actual {
        os: OsSlots {
            active: "0.9.0".to_string(),
            staged: Some("1.0.0".to_string()),
        },
        ..actual
    };
    let applying = plan(&want, &with_staged);
    assert_eq!(applying.actions[0].gate, Gate::SafeState);
    assert_eq!(applying.ready(SafeState::No, &with_staged).count(), 0);
    assert_eq!(applying.deferred(SafeState::No, &with_staged).count(), 1);
    assert!(applying
        .deferred(SafeState::No, &with_staged)
        .next()
        .unwrap()
        .1
        .contains("safe state"));
}

#[test]
fn stopping_never_waits_and_starting_always_does() {
    let mut want = desired(vec![package(
        "openrobot/grasp",
        "sha256:p1",
        PackageKind::Policy,
        RunState::Running,
    )]);
    let actual = Actual {
        os: OsSlots {
            active: "1.0.0".to_string(),
            staged: None,
        },
        installed: vec![InstalledPackage {
            id: "openrobot/grasp".to_string(),
            digest: "sha256:p1".to_string(),
            activated: true,
        }],
        ..Actual::default()
    };

    let starting = plan(&want, &actual);
    let start = starting
        .actions
        .iter()
        .find(|action| matches!(action.kind, ActionKind::StartPackage { .. }))
        .expect("a start was planned");
    assert_eq!(start.gate, Gate::SafeState);

    want.packages[0].state = RunState::Stopped;
    let running = Actual {
        units: vec![UnitStatus {
            id: "openrobot/grasp".to_string(),
            state: UnitState::Running,
            restarts: 0,
            last_exit: None,
        }],
        ..actual
    };
    let stopping = plan(&want, &running);
    let stop = stopping
        .actions
        .iter()
        .find(|action| matches!(action.kind, ActionKind::StopPackage { .. }))
        .expect("a stop was planned");
    assert_eq!(
        stop.gate,
        Gate::Open,
        "making a robot less capable never waits"
    );
    assert_eq!(stopping.ready(SafeState::No, &running).count(), 1);
}

#[test]
fn a_revocation_is_immediate_and_a_grant_waits_for_the_package_to_stop() {
    let mut want = desired(vec![package(
        "openrobot/grasp",
        "sha256:p1",
        PackageKind::Policy,
        RunState::Running,
    )]);
    want.packages[0].grants = vec!["manipulation.grasp".to_string()];

    let actual = Actual {
        os: OsSlots {
            active: "1.0.0".to_string(),
            staged: None,
        },
        installed: vec![InstalledPackage {
            id: "openrobot/grasp".to_string(),
            digest: "sha256:p1".to_string(),
            activated: true,
        }],
        units: vec![UnitStatus {
            id: "openrobot/grasp".to_string(),
            state: UnitState::Running,
            restarts: 0,
            last_exit: None,
        }],
        grants: vec![(
            "openrobot/grasp".to_string(),
            "sensing.camera.rgb".to_string(),
        )],
        ..Actual::default()
    };

    let plan = plan(&want, &actual);
    let revoke = plan
        .actions
        .iter()
        .find(|action| matches!(action.kind, ActionKind::RevokeCapability { .. }))
        .expect("the unwanted grant is revoked");
    assert_eq!(
        revoke.gate,
        Gate::Open,
        "taking a capability away never waits"
    );

    let grant = plan
        .actions
        .iter()
        .find(|action| matches!(action.kind, ActionKind::GrantCapability { .. }))
        .expect("the wanted grant is planned");
    assert_eq!(
        grant.gate,
        Gate::PackageStopped("openrobot/grasp".to_string()),
        "a running process must not gain a capability its admission did not cover"
    );
    // And the revocation is applicable even at an unsafe state, which is the case that matters: an
    // operator revoking a grant from a policy that is misbehaving is not told to wait.
    assert!(plan
        .ready(SafeState::No, &actual)
        .any(|action| matches!(action.kind, ActionKind::RevokeCapability { .. })));
}

#[test]
fn narrowing_a_profile_is_immediate_and_widening_one_is_not() {
    use crate::desired::DesiredProfile;

    let tight = ProfileLimits {
        joint_velocity_max_rad_s: Some(1.0),
        joint_torque_max_nm: Some(40.0),
        ..ProfileLimits::default()
    };
    let loose = ProfileLimits {
        joint_velocity_max_rad_s: Some(3.0),
        joint_torque_max_nm: Some(40.0),
        ..ProfileLimits::default()
    };

    let in_force = Actual {
        os: OsSlots {
            active: "1.0.0".to_string(),
            staged: None,
        },
        profile_digest: Some("sha256:old".to_string()),
        profile_limits: Some(loose.clone()),
        ..Actual::default()
    };

    let mut want = desired(Vec::new());
    want.profile = Some(DesiredProfile {
        name: "site-a".to_string(),
        digest: "sha256:new".to_string(),
        signature: None,
        verified: true,
        limits: tight.clone(),
        cell: None,
    });
    let narrowing = plan(&want, &in_force);
    assert_eq!(narrowing.actions[0].gate, Gate::Open);

    let widened = Actual {
        profile_limits: Some(tight),
        ..in_force
    };
    want.profile.as_mut().unwrap().limits = loose;
    let widening = plan(&want, &widened);
    assert_eq!(widening.actions[0].gate, Gate::SafeState);
}

#[test]
fn an_unsigned_profile_produces_no_action_at_all() {
    use crate::desired::DesiredProfile;

    // §38.1: an unsigned profile is not applied. The plan says what will happen, so an entry
    // nobody intends to apply would make it a worse statement, not a more complete one.
    let mut want = desired(Vec::new());
    want.profile = Some(DesiredProfile {
        name: "tampered".to_string(),
        digest: "sha256:new".to_string(),
        signature: None,
        verified: false,
        limits: ProfileLimits::default(),
        cell: None,
    });
    let actual = Actual {
        os: OsSlots {
            active: "1.0.0".to_string(),
            staged: None,
        },
        ..Actual::default()
    };
    assert!(plan(&want, &actual).is_empty());
}

/// A running policy being upgraded, which is the ordinary case and was the one that did not work.
///
/// The activation waits for the package to stop, and until this the plan emitted nothing that would
/// stop it: the desired state wants it *running*, so stage 5 had no reason to. The result was an
/// upgrade that deferred for ever while reporting itself as merely waiting. Found by upgrading a
/// policy on a machine with real systemd, where a package that is genuinely running is the normal
/// state rather than the interesting one.
#[test]
fn a_running_policy_can_be_upgraded() {
    let mut machine = Machine::new(true);
    let first = desired(vec![package(
        "openrobot/grasp",
        "aaa",
        PackageKind::Policy,
        RunState::Running,
    )]);
    machine.converge(&first);
    assert!(machine.actual.is_running("openrobot/grasp"));

    let second = desired(vec![package(
        "openrobot/grasp",
        "bbb",
        PackageKind::Policy,
        RunState::Running,
    )]);
    machine.applied.clear();
    let rounds = machine.converge(&second);

    assert!(
        machine.applied.contains(&ActionKind::StopPackage {
            id: "openrobot/grasp".to_string()
        }),
        "the swap needs the package stopped, so the plan has to ask for it: {:?}",
        machine.applied
    );
    assert_eq!(
        machine
            .actual
            .installed("openrobot/grasp")
            .map(|e| e.digest.as_str()),
        Some("bbb")
    );
    assert!(
        machine.actual.is_running("openrobot/grasp"),
        "and it comes back up"
    );
    assert!(rounds <= 5, "converged in {rounds} passes");
    assert!(plan(&second, &machine.actual).is_empty());
}

/// The same shape without the upgrade: a running package the desired state is happy with is not
/// stopped, and a supervisor that restarted one on every pass would be a supervisor that never let
/// a robot work.
#[test]
fn a_running_policy_at_the_right_digest_is_left_alone() {
    let mut machine = Machine::new(true);
    let state = desired(vec![package(
        "openrobot/grasp",
        "aaa",
        PackageKind::Policy,
        RunState::Running,
    )]);
    machine.converge(&state);
    machine.applied.clear();
    assert_eq!(machine.pass(&state), 0, "{:?}", machine.applied);
    assert!(machine.applied.is_empty());
}

#[test]
fn a_driver_upgrade_always_waits_for_a_safe_state() {
    let want = desired(vec![package(
        "vendor/arm",
        "sha256:d2",
        PackageKind::Driver,
        RunState::Running,
    )]);
    let actual = Actual {
        os: OsSlots {
            active: "1.0.0".to_string(),
            staged: None,
        },
        installed: vec![InstalledPackage {
            id: "vendor/arm".to_string(),
            digest: "sha256:d1".to_string(),
            activated: true,
        }],
        ..Actual::default()
    };
    let plan = plan(&want, &actual);

    let fetch = plan
        .actions
        .iter()
        .find(|action| matches!(action.kind, ActionKind::FetchPackage { .. }))
        .unwrap();
    assert_eq!(fetch.gate, Gate::Open, "fetching a driver is still free");

    let activate = plan
        .actions
        .iter()
        .find(|action| matches!(action.kind, ActionKind::ActivatePackage { .. }))
        .unwrap();
    assert_eq!(activate.gate, Gate::SafeState);
    assert_eq!(
        activate.stage, 2,
        "drivers are stage 2, before everything else"
    );
}

#[test]
fn the_plan_is_ordered_by_stage() {
    let mut want = desired(vec![
        package(
            "openrobot/grasp",
            "sha256:p1",
            PackageKind::Policy,
            RunState::Running,
        ),
        package(
            "vendor/arm",
            "sha256:d1",
            PackageKind::Driver,
            RunState::Running,
        ),
    ]);
    want.packages[0].grants = vec!["manipulation.grasp".to_string()];
    want.os_version = "2.0.0".to_string();

    let plan = plan(&want, &Actual::default());
    let stages: Vec<u8> = plan.actions.iter().map(|action| action.stage).collect();
    let mut sorted = stages.clone();
    sorted.sort_unstable();
    assert_eq!(stages, sorted, "the plan came out of order: {stages:?}");
    assert_eq!(stages.first(), Some(&1), "the image is first");
}

#[test]
fn a_failed_unit_is_left_alone() {
    // §33.5: after the threshold the package is marked failed and left alone. A reconciler that
    // restarted it would be the infinite restart loop the threshold exists to stop, one iteration
    // slower.
    let want = desired(vec![package(
        "openrobot/grasp",
        "sha256:p1",
        PackageKind::Policy,
        RunState::Running,
    )]);
    let actual = Actual {
        os: OsSlots {
            active: "1.0.0".to_string(),
            staged: None,
        },
        installed: vec![InstalledPackage {
            id: "openrobot/grasp".to_string(),
            digest: "sha256:p1".to_string(),
            activated: true,
        }],
        units: vec![UnitStatus {
            id: "openrobot/grasp".to_string(),
            state: UnitState::Failed,
            restarts: 5,
            last_exit: Some(139),
        }],
        ..Actual::default()
    };
    assert!(plan(&want, &actual).is_empty());
}

#[test]
fn a_unit_in_flight_is_not_acted_on_twice() {
    let want = desired(vec![package(
        "openrobot/grasp",
        "sha256:p1",
        PackageKind::Policy,
        RunState::Running,
    )]);
    let actual = Actual {
        os: OsSlots {
            active: "1.0.0".to_string(),
            staged: None,
        },
        installed: vec![InstalledPackage {
            id: "openrobot/grasp".to_string(),
            digest: "sha256:p1".to_string(),
            activated: true,
        }],
        units: vec![UnitStatus {
            id: "openrobot/grasp".to_string(),
            state: UnitState::Transitioning,
            restarts: 0,
            last_exit: None,
        }],
        ..Actual::default()
    };
    assert!(plan(&want, &actual).is_empty());
}

// ── Desired-state decoding ───────────────────────────────────────────────────

#[test]
fn a_desired_state_decodes() {
    let json = Json::parse(
        r#"{"generation":41,"os":{"channel":"stable","version":"1.4.0"},
            "packages":[{"id":"openrobot/grasp","version":"1.4.0","digest":"sha256:p1",
                         "kind":"policy","state":"running",
                         "grants":["manipulation.grasp","sensing.camera.rgb"]}],
            "profile":{"name":"site-a","digest":"sha256:pr","verified":true,
                       "limits":{"joint_velocity_max_rad_s":1.5}}}"#,
    )
    .unwrap();
    let desired = DesiredState::from_json(&json).unwrap();
    assert_eq!(desired.generation, 41);
    assert_eq!(desired.os_version, "1.4.0");
    assert_eq!(desired.packages.len(), 1);
    assert_eq!(desired.packages[0].kind, PackageKind::Policy);
    assert_eq!(desired.packages[0].state, RunState::Running);
    assert_eq!(
        desired.packages[0].grants,
        vec![
            "manipulation.grasp".to_string(),
            "sensing.camera.rgb".to_string()
        ]
    );
    let profile = desired.profile.unwrap();
    // §38.1, and the hole this closes. The document above says `"verified": true`, and the decoder
    // refuses to believe it: the desired state is a file on the robot's own disk, so anybody who
    // could edit the limits could edit that boolean in the same pass, and the kernel's ceilings were
    // on the other side of it. Only a signature checked on this robot may set it, and that happens
    // in `skynetd-io` where the trust store is.
    assert!(
        !profile.verified,
        "a document cannot assert its own verification"
    );
    assert_eq!(profile.limits.joint_velocity_max_rad_s, Some(1.5));
}

#[test]
fn a_profile_signature_is_decoded_but_never_believed() {
    let json = Json::parse(
        r#"{"generation":1,
            "profile":{"name":"site-a","digest":"sha256:pr","verified":true,
                       "signature":{"key":"aa","value":"bb"},
                       "limits":{"joint_velocity_max_rad_s":1.5}}}"#,
    )
    .unwrap();
    let profile = DesiredState::from_json(&json).unwrap().profile.unwrap();
    // The signature travels; the verdict does not.
    assert_eq!(
        profile.signature,
        Some(crate::desired::DesiredSignature {
            key: "aa".to_string(),
            value: "bb".to_string(),
        })
    );
    assert!(!profile.verified);
}

#[test]
fn an_unpinned_package_is_a_decode_error_naming_the_field() {
    // §37.4. The robot does no version solving, so a package with no digest is not a package with a
    // digest to be worked out later — it is a document the robot cannot act on.
    let json = Json::parse(
        r#"{"generation":1,"packages":[{"id":"a/b","kind":"policy","version":"1.0.0"}]}"#,
    )
    .unwrap();
    let error = DesiredState::from_json(&json).unwrap_err();
    assert_eq!(error.field, "packages[0].digest");
    assert!(error.detail.contains("37.4"));
}

#[test]
fn an_unknown_kind_is_refused_rather_than_defaulted() {
    let json = Json::parse(
        r#"{"generation":1,"packages":[{"id":"a/b","kind":"widget","digest":"sha256:x"}]}"#,
    )
    .unwrap();
    let error = DesiredState::from_json(&json).unwrap_err();
    assert_eq!(error.field, "packages[0].kind");
    assert!(error.detail.contains("restart policy"));
}

#[test]
fn one_id_twice_is_a_document_that_says_two_things() {
    let json = Json::parse(
        r#"{"generation":1,"packages":[
              {"id":"a/b","kind":"policy","digest":"sha256:1"},
              {"id":"a/b","kind":"policy","digest":"sha256:2"}]}"#,
    )
    .unwrap();
    assert_eq!(
        DesiredState::from_json(&json).unwrap_err().field,
        "packages[1].id"
    );
}

#[test]
fn a_missing_generation_is_an_error_rather_than_a_zero() {
    let json = Json::parse(r#"{"packages":[]}"#).unwrap();
    assert_eq!(
        DesiredState::from_json(&json).unwrap_err().field,
        "generation"
    );
}

#[test]
fn dropping_a_ceiling_is_a_widening() {
    // The case worth a test of its own: a profile edit that deletes a field removes a limit, and
    // treating that as the harmless direction would apply it under a running policy.
    let with = ProfileLimits {
        joint_torque_max_nm: Some(40.0),
        ..ProfileLimits::default()
    };
    let without = ProfileLimits::default();
    assert!(
        !without.narrows(&with),
        "dropping a ceiling is not narrowing"
    );
    assert!(with.narrows(&without), "adding one is");
}

/// A margin is a **floor**, and classifying it as a ceiling inverts §33.4's asymmetry.
///
/// The trap in adding this field. `narrower` says smaller-is-stricter, which is right for every
/// ceiling in this struct and wrong for a separation: halving a collision margin lets two bodies get
/// twice as close. Had it gone through `narrower`, that edit would have been classified as a
/// *narrowing* and applied under a running policy — the exact failure the classifier exists to
/// prevent, arriving through the classifier.
#[test]
fn shrinking_a_collision_margin_is_a_widening() {
    let far = ProfileLimits {
        self_collision_margin_m: Some(0.05),
        ..ProfileLimits::default()
    };
    let near = ProfileLimits {
        self_collision_margin_m: Some(0.01),
        ..ProfileLimits::default()
    };
    assert!(
        !near.narrows(&far),
        "letting bodies come twice as close is not a narrowing"
    );
    assert!(far.narrows(&near), "keeping them further apart is");
    // And dropping it is a widening for the same reason dropping a ceiling is.
    assert!(!ProfileLimits::default().narrows(&far));
    assert!(far.narrows(&ProfileLimits::default()));
}

/// Two of the three separation thresholds are floors and the third is a ceiling.
#[test]
fn a_separation_distance_is_a_floor_and_the_slow_factor_is_a_ceiling() {
    let profile = |stop: f64, factor: f64| ProfileLimits {
        proximity: Some(ProfileProximity {
            stop_distance_m: Some(stop),
            slow_distance_m: Some(2.0),
            slow_factor: Some(factor),
        }),
        ..ProfileLimits::default()
    };

    // Stopping further away is stricter.
    assert!(profile(1.5, 0.3).narrows(&profile(1.0, 0.3)));
    assert!(!profile(0.5, 0.3).narrows(&profile(1.0, 0.3)));

    // A smaller multiplier slows the robot more, so it is stricter.
    assert!(profile(1.0, 0.1).narrows(&profile(1.0, 0.3)));
    assert!(!profile(1.0, 0.6).narrows(&profile(1.0, 0.3)));

    // Dropping the whole block removes a limit.
    assert!(!ProfileLimits::default().narrows(&profile(1.0, 0.3)));
    assert!(profile(1.0, 0.3).narrows(&ProfileLimits::default()));
}

/// §11 carries them, which is the delivery path §7.6 recorded as missing.
#[test]
fn the_profile_carries_a_margin_and_a_separation_block() {
    let json = Json::parse(
        r#"{"generation":4,"packages":[],"profile":{"name":"cell-a","digest":"sha256:aa",
           "limits":{"joint_torque_max_nm":40,"self_collision_margin_m":0.05,
           "proximity":{"stop_distance_m":1.2,"slow_distance_m":2.5,"slow_factor":0.25}}}}"#,
    )
    .unwrap();
    let desired = DesiredState::from_json(&json).unwrap();
    let profile = desired.profile.expect("a profile");
    assert_eq!(profile.limits.self_collision_margin_m, Some(0.05));
    let prox = profile.limits.proximity.expect("a proximity block");
    assert_eq!(prox.stop_distance_m, Some(1.2));
    assert_eq!(prox.slow_distance_m, Some(2.5));
    assert_eq!(prox.slow_factor, Some(0.25));
}

/// A partial separation block stays partial rather than acquiring a default.
///
/// The promotion to `skynet_kernel::Proximity` happens in `skynetd-io` and needs all three. Here the
/// only thing that matters is that the decoder does not invent the missing one: a profile declaring
/// two of three is a document an operator has not finished, and §7.2 has the guard report the input
/// it lacks rather than run on a number nobody chose.
#[test]
fn a_partial_separation_block_is_carried_as_partial() {
    let json = Json::parse(
        r#"{"generation":4,"packages":[],"profile":{"name":"cell-a","digest":"sha256:aa",
           "limits":{"proximity":{"stop_distance_m":1.2}}}}"#,
    )
    .unwrap();
    let profile = DesiredState::from_json(&json).unwrap().profile.unwrap();
    let prox = profile.limits.proximity.expect("a proximity block");
    assert_eq!(prox.stop_distance_m, Some(1.2));
    assert_eq!(prox.slow_distance_m, None);
    assert_eq!(prox.slow_factor, None);
}

// ── Supervision ──────────────────────────────────────────────────────────────

#[test]
fn a_policy_backs_off_exponentially_and_then_gives_up() {
    let mut supervision = Supervision::new(
        "openrobot/grasp",
        PackageKind::Policy,
        CrashPolicy::default(),
    );

    assert_eq!(
        supervision.on_exit(0, 0.0),
        RestartDecision::RestartAt(1_000)
    );
    assert_eq!(
        supervision.on_exit(1_100, 0.0),
        RestartDecision::RestartAt(3_100)
    );
    assert_eq!(
        supervision.on_exit(3_200, 0.0),
        RestartDecision::RestartAt(7_200)
    );
    assert_eq!(
        supervision.on_exit(7_300, 0.0),
        RestartDecision::RestartAt(15_300)
    );
    // Five failures in ten minutes.
    assert_eq!(supervision.on_exit(15_400, 0.0), RestartDecision::Failed);
    assert!(supervision.is_failed());
    assert_eq!(supervision.next_start(), None);
}

#[test]
fn the_window_forgets_by_time_rather_than_by_success() {
    // A policy that crashes, runs for ninety seconds and crashes again is crash-looping. Clearing
    // the history on a successful start would restart it for ever.
    let mut supervision = Supervision::new("a/b", PackageKind::Policy, CrashPolicy::default());
    for at in [0, 90_000, 180_000, 270_000] {
        supervision.on_exit(at, 0.0);
        supervision.on_start();
    }
    assert!(
        !supervision.is_failed(),
        "four in the window is under the threshold"
    );
    assert_eq!(supervision.on_exit(360_000, 0.0), RestartDecision::Failed);

    // Spread beyond the window, the same five crashes are not a loop.
    let mut spread = Supervision::new("a/b", PackageKind::Policy, CrashPolicy::default());
    for round in 0..5 {
        let decision = spread.on_exit(round * 700_000, 0.0);
        assert!(
            matches!(decision, RestartDecision::RestartAt(_)),
            "round {round}"
        );
        spread.on_start();
    }
}

#[test]
fn a_driver_is_never_restarted_automatically() {
    let mut supervision =
        Supervision::new("vendor/arm", PackageKind::Driver, CrashPolicy::default());
    assert_eq!(supervision.on_exit(0, 0.0), RestartDecision::AwaitOperator);
    assert!(supervision.is_failed());
    // And an operator's deliberate act clears it, history and all.
    supervision.reset();
    assert_eq!(supervision.state, SupervisionState::Up);
    assert!(supervision.failures.is_empty());
}

#[test]
fn jitter_only_ever_reduces_the_delay() {
    // Backoff that jitters upward can exceed its own cap, and a cap that is sometimes exceeded is
    // not a cap.
    let supervision = Supervision::new("a/b", PackageKind::Policy, CrashPolicy::default());
    for failures in 1..12 {
        let plain = supervision.backoff_ms(failures, 0.0);
        for jitter in [0.0, 0.3, 0.99, 1.0, 4.0, -1.0] {
            let jittered = supervision.backoff_ms(failures, jitter);
            assert!(jittered <= plain, "{failures} failures, jitter {jitter}");
            assert!(jittered <= CrashPolicy::default().cap_ms);
        }
    }
}

#[test]
fn the_backoff_is_capped_rather_than_overflowing() {
    let supervision = Supervision::new("a/b", PackageKind::Policy, CrashPolicy::default());
    assert_eq!(
        supervision.backoff_ms(200, 0.0),
        CrashPolicy::default().cap_ms
    );
}

// ── Health ───────────────────────────────────────────────────────────────────

#[test]
fn alive_and_producing_nothing_is_reported_as_exactly_that() {
    let production = Production {
        last_sequence: Some(4_212),
        seen_at_ms: 1_000,
        declared_hz: Some(50.0),
    };
    assert_eq!(
        readiness(true, production, 1_020, DEFAULT_TOLERANCE_PERIODS),
        Readiness::Ready
    );
    assert_eq!(
        readiness(true, production, 1_400, DEFAULT_TOLERANCE_PERIODS),
        Readiness::Stalled,
        "twenty periods late is stalled"
    );
    assert_eq!(
        readiness(false, production, 9_999, DEFAULT_TOLERANCE_PERIODS),
        Readiness::NotRunning
    );
}

#[test]
fn a_policy_in_its_first_cycle_is_not_a_stall() {
    let fresh = Production {
        last_sequence: None,
        seen_at_ms: 0,
        declared_hz: Some(50.0),
    };
    assert_eq!(
        readiness(true, fresh, 100_000, DEFAULT_TOLERANCE_PERIODS),
        Readiness::Unknown
    );
}

#[test]
fn a_package_that_declared_no_rate_cannot_be_judged() {
    let undeclared = Production {
        last_sequence: Some(1),
        seen_at_ms: 0,
        declared_hz: None,
    };
    assert_eq!(
        readiness(true, undeclared, 100_000, DEFAULT_TOLERANCE_PERIODS),
        Readiness::Unknown,
        "assuming ready would be inventing a measurement"
    );
}

#[test]
fn an_unmeasured_health_field_is_not_a_fault() {
    let health = Health {
        packages: vec![PackageHealth {
            id: "a/b".to_string(),
            live: true,
            readiness: Readiness::Ready,
            last_sequence: Some(9),
            age_ms: 12,
        }],
        generation_lag: None,
        storage_headroom: None,
        guard_trip_rate: None,
        battery: None,
        temperature_c: None,
    };
    assert!(!health.is_degraded(), "a gap is not a fault");

    let stalled = Health {
        packages: vec![PackageHealth {
            id: "a/b".to_string(),
            live: true,
            readiness: Readiness::Stalled,
            last_sequence: Some(9),
            age_ms: 4_000,
        }],
        ..health
    };
    assert!(stalled.is_degraded());
    assert_eq!(stalled.stalled().count(), 1);
}

// ── Leases ───────────────────────────────────────────────────────────────────

#[test]
fn two_packages_cannot_drive_one_elbow() {
    let mut table = LeaseTable::new();
    assert_eq!(
        table.acquire("a/one", "manipulation.grasp", true, false),
        PreemptionOutcome::Acquired
    );
    assert_eq!(
        table.acquire("b/two", "manipulation.grasp", true, false),
        PreemptionOutcome::Denied(LeaseDenial::Held {
            holder: "a/one".to_string()
        })
    );
    assert_eq!(
        table.acquire("a/one", "manipulation.grasp", true, false),
        PreemptionOutcome::AlreadyHeld,
        "re-acquiring is idempotent, so a reconciler re-running is harmless"
    );
}

#[test]
fn a_grant_is_checked_before_the_holder() {
    // A package with no permission is told it has no permission rather than that somebody else is
    // using the arm — which would be true and would send them looking in the wrong place.
    let mut table = LeaseTable::new();
    table.acquire("a/one", "manipulation.grasp", true, false);
    assert_eq!(
        table.acquire("b/two", "manipulation.grasp", false, false),
        PreemptionOutcome::Denied(LeaseDenial::NotGranted)
    );
}

#[test]
fn two_policies_may_read_one_camera() {
    let mut table = LeaseTable::new();
    assert_eq!(
        table.acquire("a/one", "sensing.camera.rgb", true, false),
        PreemptionOutcome::Acquired
    );
    assert_eq!(
        table.acquire("b/two", "sensing.camera.rgb", true, false),
        PreemptionOutcome::Acquired,
        "sensing is shared freely"
    );
}

#[test]
fn exactly_one_thing_pre_empts() {
    let mut table = LeaseTable::new();
    table.acquire("a/policy", "manipulation.grasp", true, false);
    assert_eq!(
        table.acquire("b/reflex", "manipulation.grasp", true, true),
        PreemptionOutcome::Preempted {
            displaced: "a/policy".to_string()
        }
    );
    assert_eq!(table.holder("manipulation.grasp"), Some("b/reflex"));
}

#[test]
fn a_crashed_package_does_not_keep_holding_the_arm() {
    let mut table = LeaseTable::new();
    table.acquire("a/one", "manipulation.grasp", true, false);
    table.acquire("a/one", "locomotion.walk", true, false);
    let released = table.release_all("a/one");
    assert_eq!(released.len(), 2);
    assert_eq!(table.holder("manipulation.grasp"), None);
    assert_eq!(
        table.acquire("b/two", "manipulation.grasp", true, false),
        PreemptionOutcome::Acquired,
        "the replacement can take it"
    );
}

#[test]
fn releasing_a_lease_nobody_holds_is_not_an_error() {
    let mut table = LeaseTable::new();
    table.release("nobody", "manipulation.grasp");
    assert!(table.leases().is_empty());
}

#[test]
fn an_unrecognised_token_is_exclusive_rather_than_shared() {
    // Erring this way costs a refusal. Erring the other way is two policies driving one elbow.
    let mut table = LeaseTable::new();
    table.acquire("a/one", "something.nobody.modelled", true, false);
    assert!(matches!(
        table.acquire("b/two", "something.nobody.modelled", true, false),
        PreemptionOutcome::Denied(LeaseDenial::Held { .. })
    ));
}
// ── §54.4's `acquiring` stage, over a whole skill's tokens ───────────────────

mod acquiring_a_whole_skill {
    use crate::lease::LeaseTable;
    use skynet_contracts::skill::{advance, LeaseState, Progress, SkillPhase};

    fn tokens(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn everything(_token: &str) -> bool {
        true
    }

    #[test]
    fn a_skill_that_gets_every_token_is_held() {
        let mut table = LeaseTable::new();
        let result = table.acquire_all(
            "acme/wipe",
            &tokens(&["manipulation.grasp", "locomotion.walk"]),
            everything,
            false,
        );
        assert_eq!(result.state, LeaseState::Held);
        assert_eq!(result.taken.len(), 2);
        assert_eq!(table.holder("manipulation.grasp"), Some("acme/wipe"));
    }

    /// The property that only becomes visible from §54.4: **acquisition is all or nothing.**
    ///
    /// A skill that got two of its three tokens holds two joints it cannot use and refuses them to
    /// everything else, and the symptom is a second package that will not start with no reason on
    /// any screen — the leaked-lease failure arriving through the front door.
    #[test]
    fn a_denial_rolls_back_everything_this_call_took() {
        let mut table = LeaseTable::new();
        table.acquire("other/policy", "locomotion.walk", true, false);

        let result = table.acquire_all(
            "acme/wipe",
            &tokens(&["manipulation.grasp", "locomotion.walk", "manipulation.lift"]),
            everything,
            false,
        );

        assert_eq!(result.state, LeaseState::Denied);
        assert_eq!(result.token.as_deref(), Some("locomotion.walk"));
        assert_eq!(result.holder.as_deref(), Some("other/policy"));
        assert!(result.taken.is_empty());

        // The one it did take before the refusal is back, and the one it never reached was never
        // taken. Anything else is an arm this skill is holding and cannot use.
        assert_eq!(table.holder("manipulation.grasp"), None);
        assert_eq!(table.holder("manipulation.lift"), None);
        // And the holder that refused it still holds it.
        assert_eq!(table.holder("locomotion.walk"), Some("other/policy"));
    }

    /// It rolls back what **this call** took, and nothing else. A token the requester already held
    /// is not this acquisition's to give away — and a level-triggered loop calling `acquire_all`
    /// every pass would otherwise drop a lease it was in the middle of using.
    #[test]
    fn a_rollback_does_not_release_what_the_requester_already_held() {
        let mut table = LeaseTable::new();
        table.acquire("acme/wipe", "manipulation.grasp", true, false);
        table.acquire("other/policy", "locomotion.walk", true, false);

        let result = table.acquire_all(
            "acme/wipe",
            &tokens(&["manipulation.grasp", "locomotion.walk"]),
            everything,
            false,
        );

        assert_eq!(result.state, LeaseState::Denied);
        assert_eq!(
            table.holder("manipulation.grasp"),
            Some("acme/wipe"),
            "a lease held before the call survives the call"
        );
    }

    /// §25.2 checks the grant before the holder, "so a package with no permission is told it has no
    /// permission rather than that somebody else is using the arm". That distinction has to survive
    /// the projection, and `holder: None` is how it does.
    #[test]
    fn an_ungranted_token_names_no_holder() {
        let mut table = LeaseTable::new();
        let result = table.acquire_all(
            "acme/wipe",
            &tokens(&["manipulation.grasp"]),
            |_| false,
            false,
        );
        assert_eq!(result.state, LeaseState::Denied);
        assert_eq!(result.token.as_deref(), Some("manipulation.grasp"));
        assert_eq!(
            result.holder, None,
            "nobody has the arm; this package may not ask for it"
        );
    }

    /// A skill declaring no capabilities acquires nothing and is held, which is the honest answer:
    /// there was nothing to arbitrate. Refusing it would stop every skill that only reads.
    #[test]
    fn a_skill_that_needs_nothing_is_held() {
        let mut table = LeaseTable::new();
        let result = table.acquire_all("acme/look", &[], everything, false);
        assert_eq!(result.state, LeaseState::Held);
        assert!(result.taken.is_empty());
    }

    /// §25.2's one pre-emption, reported so it can become a black box event.
    #[test]
    fn a_safety_behaviour_displaces_and_the_displacement_is_reported() {
        let mut table = LeaseTable::new();
        table.acquire("acme/wipe", "manipulation.grasp", true, false);

        let result = table.acquire_all(
            "vendor/fall-arrest",
            &tokens(&["manipulation.grasp"]),
            everything,
            true,
        );
        assert_eq!(result.state, LeaseState::Held);
        assert_eq!(
            result.displaced,
            vec![("manipulation.grasp".to_string(), "acme/wipe".to_string())]
        );
    }

    /// The projection the contract's documentation promised lived at a call site. This is the join
    /// between §25.2's table and §54.4's `acquiring` stage, and it is the only place the two meet.
    #[test]
    fn the_acquisition_drives_the_lifecycle() {
        let mut table = LeaseTable::new();
        table.acquire("other/policy", "manipulation.grasp", true, false);

        let mut progress = Progress {
            invoked: true,
            entry_permitted: Some(true),
            ..Progress::default()
        };
        table
            .acquire_all(
                "acme/wipe",
                &tokens(&["manipulation.grasp"]),
                everything,
                false,
            )
            .into_progress(&mut progress);

        let transition = advance(SkillPhase::Acquiring, &progress);
        assert_eq!(transition.to, SkillPhase::Terminal);
        assert!(transition.detail.contains("manipulation.grasp"));
        assert!(transition.detail.contains("other/policy"));
        assert!(transition.release_leases);
    }
}
