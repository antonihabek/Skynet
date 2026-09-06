//! The level-triggered reconciler (§33.3, §33.4).
//!
//! > That distinction is the most important sentence in this section. An edge-triggered supervisor
//! > acts on transitions — "a new package appeared, install it". Miss one edge, because the process
//! > was restarting or the disk was full or the network dropped mid-message, and the system stays
//! > wrong forever, because nothing will ever re-deliver that edge. A level-triggered supervisor
//! > asks "what is true, what should be true, what is the difference" and converges from *any*
//! > starting state, including states nobody anticipated.
//!
//! [`plan`] is that question, as a function. It takes what the Hub wants and what the machine is,
//! and returns the difference as ordered actions. It reads no clock, opens no socket and applies
//! nothing, which is what makes the two properties §33.3 claims into properties a test can prove:
//!
//! * **it converges from any starting state** — including ones with a half-installed package, a
//!   failed unit, a package the desired state has never heard of, and an image staged for a version
//!   nobody wants any more;
//! * **it is idempotent** — apply the plan, observe again, and the next plan is empty. The suite
//!   asserts exactly that against a model of the machine, for every case it has.
//!
//! # The ordering, and the barrier
//!
//! Actions are not independent, so the plan is ordered rather than a set (§33.4):
//!
//! | # | Stage | Note |
//! |---|---|---|
//! | 1 | OS image | Staging is always safe; applying reboots |
//! | 2 | RHAL drivers | Changing the driver changes the body |
//! | 3 | Package installs and upgrades | Fetching and verifying is free; activating is not |
//! | 4 | Profiles and grants | A narrowing takes effect immediately; a widening waits |
//! | 5 | Policy start and stop | Stopping is always allowed |
//! | 6 | Leases | Follows from 5 |
//!
//! Two rules govern the barrier, and the second is the one that appears throughout this design:
//!
//! * **Anything that changes what the robot is capable of waits for a safe state.** Swapping a
//!   driver under a running policy would change the meaning of every command in flight.
//! * **Anything that reduces what the robot may do applies immediately.** Revoking a grant,
//!   narrowing a limit, and stopping a policy never wait, because making a robot less capable is
//!   never the dangerous direction.
//!
//! A deferred action stays in the plan, carrying its [`Gate`], so an operator sees "waiting for a
//! safe state" rather than an update that silently never happened.

use skynet_contracts::opstate::OperatingState;

use crate::actual::{Actual, UnitState};
use crate::desired::{DesiredState, PackageKind, RunState};

/// Whether the robot is somewhere an action that changes its capabilities may be applied.
///
/// A newtype rather than a bare `bool` because the call site reads `SafeState::No` instead of
/// `false`, and "false" at a call site that also takes three other booleans is how a barrier gets
/// inverted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeState {
    /// At a declared safe state: nothing is held, nothing is mid-task.
    Yes,
    /// Not. Actions that change what the robot can do are deferred and reported as deferred.
    No,
}

impl SafeState {
    /// Whether the barrier is open.
    pub fn is_safe(self) -> bool {
        self == Self::Yes
    }

    /// The barrier, read off §27's operating state.
    ///
    /// This is what the newtype was waiting for. §33.4's barrier rests on "nothing is held, nothing
    /// is mid-task", which is a property of the state the robot is in — and until §27's machine was
    /// declared there was nowhere to read that from, so every caller asserted it. A caller asserting
    /// a safe state while a policy is commanding is precisely the barrier inversion the comment
    /// above this type worries about, and it is no longer expressible.
    ///
    /// [`OperatingState::Stopping`] is the entry that makes the derivation worth having: a robot
    /// mid-ramp is the state most likely to be declared safe by somebody watching it slow down.
    pub fn from_state(state: OperatingState) -> Self {
        if state.is_safe_state() {
            Self::Yes
        } else {
            Self::No
        }
    }

    /// The barrier, narrowed by a second opinion.
    ///
    /// §33.4's asymmetry, applied to the two sources of this fact: "anything that reduces what the
    /// robot may do applies immediately". Either source may **close** the barrier and neither may
    /// open it alone, so a declaration somebody left set to `yes` cannot outvote a machine that can
    /// see a policy running, and a machine on an edition that cannot observe the body cannot outvote
    /// an operator who has not declared anything.
    pub fn and(self, other: Self) -> Self {
        if self.is_safe() && other.is_safe() {
            Self::Yes
        } else {
            Self::No
        }
    }
}

/// What has to be true before an action may be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Apply now. Everything that reduces what the robot may do is here.
    Open,
    /// §33.4's barrier: this changes what the robot is capable of.
    SafeState,
    /// A widening of what one package may do, which takes effect when that package next starts.
    ///
    /// Distinct from [`Gate::SafeState`] because it is narrower: the rest of the robot may carry on.
    /// A grant widened under a running policy would give a *running* process a capability its
    /// admission decision did not cover.
    PackageStopped(String),
}

impl Gate {
    /// Whether this action may be applied given the robot's state.
    pub fn is_open(&self, safe: SafeState, actual: &Actual) -> bool {
        match self {
            Self::Open => true,
            Self::SafeState => safe.is_safe(),
            Self::PackageStopped(id) => !actual.is_running(id),
        }
    }

    /// Why it is closed, in a sentence an operator can act on.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Open => "",
            Self::SafeState => {
                "waiting for a safe state: this changes what the robot is capable of, and applying \
                 it under a running policy would change the meaning of every command in flight"
            }
            Self::PackageStopped(_) => {
                "waiting for the package to stop: this widens what it may do, and a running process \
                 must not gain a capability its admission decision did not cover"
            }
        }
    }
}

/// One thing to do.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionKind {
    /// Fetch and verify an OS image into the inactive slot. Always safe (§33.4).
    StageOsImage {
        /// Version to stage.
        version: String,
    },
    /// Mark the staged slot active. Reboots.
    ApplyOsImage {
        /// Version being applied.
        version: String,
    },
    /// Fetch, verify, admit and stage a package. Free under a running policy.
    FetchPackage {
        /// Package id.
        id: String,
        /// Content address.
        digest: String,
    },
    /// Swap the activation symlink (§37.1).
    ActivatePackage {
        /// Package id.
        id: String,
        /// Content address being activated.
        digest: String,
    },
    /// Drop a package the desired state no longer names.
    RemovePackage {
        /// Package id.
        id: String,
    },
    /// Apply an operator profile.
    ApplyProfile {
        /// Profile name, for the audit log.
        name: String,
        /// Content address.
        digest: String,
    },
    /// Grant a capability token to a package.
    GrantCapability {
        /// Package id.
        id: String,
        /// Token.
        token: String,
    },
    /// Take a capability token away.
    RevokeCapability {
        /// Package id.
        id: String,
        /// Token.
        token: String,
    },
    /// Start a package.
    StartPackage {
        /// Package id.
        id: String,
    },
    /// Stop a package.
    StopPackage {
        /// Package id.
        id: String,
    },
    /// Release a lease held by a package that is no longer running (§25.2).
    ReleaseLease {
        /// Package id.
        id: String,
        /// Token.
        token: String,
    },
}

/// One action, with the stage it belongs to and what gates it.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    /// 1–6, per §33.4's table. Carried explicitly so a report can group by it.
    pub stage: u8,
    /// What to do.
    pub kind: ActionKind,
    /// What has to be true first.
    pub gate: Gate,
}

impl Action {
    fn new(stage: u8, kind: ActionKind, gate: Gate) -> Self {
        Self { stage, kind, gate }
    }

    /// The subject this action is about, for reporting.
    pub fn subject(&self) -> &str {
        match &self.kind {
            ActionKind::StageOsImage { version } | ActionKind::ApplyOsImage { version } => version,
            ActionKind::FetchPackage { id, .. }
            | ActionKind::ActivatePackage { id, .. }
            | ActionKind::RemovePackage { id }
            | ActionKind::GrantCapability { id, .. }
            | ActionKind::RevokeCapability { id, .. }
            | ActionKind::StartPackage { id }
            | ActionKind::StopPackage { id }
            | ActionKind::ReleaseLease { id, .. } => id,
            ActionKind::ApplyProfile { name, .. } => name,
        }
    }
}

/// An ordered plan, and what could not be applied yet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Plan {
    /// Every action, in the order §33.4 requires.
    pub actions: Vec<Action>,
}

impl Plan {
    /// Whether there is nothing to do. The state a converged robot sits in.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Actions whose gate is open, in order — what this pass will actually apply.
    pub fn ready<'a>(
        &'a self,
        safe: SafeState,
        actual: &'a Actual,
    ) -> impl Iterator<Item = &'a Action> {
        self.actions
            .iter()
            .filter(move |action| action.gate.is_open(safe, actual))
    }

    /// Actions that are waiting, with the reason.
    ///
    /// Reported rather than dropped: §33.4 requires an operator to see "waiting for a safe state"
    /// rather than an update that silently never happened.
    pub fn deferred<'a>(
        &'a self,
        safe: SafeState,
        actual: &'a Actual,
    ) -> impl Iterator<Item = (&'a Action, &'static str)> {
        self.actions.iter().filter_map(move |action| {
            if action.gate.is_open(safe, actual) {
                None
            } else {
                Some((action, action.gate.reason()))
            }
        })
    }
}

/// Compare desired against observed and emit the difference, ordered.
///
/// Pure, and it answers the level-triggered question rather than reacting to an event: nothing here
/// consults what happened last time, so a robot that was interrupted mid-plan, or spent a week
/// offline, or was restored from a backup, converges on the next pass exactly as one that was
/// never interrupted.
///
/// **It does not take the safe state**, which is worth a sentence because §33.3's sketch shows the
/// check inside the loop. Writing it that way produced an unused parameter, and the reason turned
/// out to be a real property rather than an oversight: what an action *requires* is a fact about the
/// action, and whether the robot is there is a fact about right now. Separating them means a robot
/// that reaches a safe state does not have to re-plan — it re-evaluates the gates it already has —
/// and it means the plan two operators are looking at is the same plan whatever the robot is doing
/// while they read it.
pub fn plan(desired: &DesiredState, actual: &Actual) -> Plan {
    let mut actions = Vec::new();

    // ── 1. The OS image ─────────────────────────────────────────────────────
    //
    // Staging is always safe: fetching and verifying an image touches nothing the robot is doing.
    // Applying it reboots, so it waits — and "never apply mid-task" is §12's first rule, measured
    // by `ota.safe_state.respected`.
    if !desired.os_version.is_empty() && desired.os_version != actual.os.active {
        if actual.os.staged.as_deref() == Some(desired.os_version.as_str()) {
            actions.push(Action::new(
                1,
                ActionKind::ApplyOsImage {
                    version: desired.os_version.clone(),
                },
                Gate::SafeState,
            ));
        } else {
            actions.push(Action::new(
                1,
                ActionKind::StageOsImage {
                    version: desired.os_version.clone(),
                },
                Gate::Open,
            ));
        }
    }

    // ── 2–3. Drivers, then everything else ──────────────────────────────────
    //
    // One walk over the packages, sorted into stages, because a driver is a package with a
    // different stage and a different restart policy rather than a different kind of object.
    for package in &desired.packages {
        let stage = if package.kind == PackageKind::Driver {
            2
        } else {
            3
        };
        let installed = actual.installed(&package.id);

        match installed {
            None => actions.push(Action::new(
                stage,
                ActionKind::FetchPackage {
                    id: package.id.clone(),
                    digest: package.digest.clone(),
                },
                Gate::Open,
            )),
            Some(entry) if entry.digest != package.digest => {
                // An upgrade: the new version is fetched alongside the old (§37.2), so the fetch is
                // free and only the swap waits.
                actions.push(Action::new(
                    stage,
                    ActionKind::FetchPackage {
                        id: package.id.clone(),
                        digest: package.digest.clone(),
                    },
                    Gate::Open,
                ));
                actions.push(Action::new(
                    stage,
                    ActionKind::ActivatePackage {
                        id: package.id.clone(),
                        digest: package.digest.clone(),
                    },
                    activation_gate(package.kind, &package.id, actual),
                ));
            }
            Some(entry) if !entry.activated => actions.push(Action::new(
                stage,
                ActionKind::ActivatePackage {
                    id: package.id.clone(),
                    digest: package.digest.clone(),
                },
                activation_gate(package.kind, &package.id, actual),
            )),
            Some(_) => {}
        }
    }

    // Anything installed that the desired state no longer names. Removal is not gated: a package
    // that is not running and not wanted is a blob, and deleting a blob changes nothing the robot
    // is doing. One that *is* running is stopped first, at stage 5.
    for entry in &actual.installed {
        if desired.package(&entry.id).is_none() && !actual.is_running(&entry.id) {
            actions.push(Action::new(
                3,
                ActionKind::RemovePackage {
                    id: entry.id.clone(),
                },
                Gate::Open,
            ));
        }
    }

    // ── 4. Profile and grants ───────────────────────────────────────────────
    //
    // §38.1: an unsigned profile is not applied. The reconciler emits no action for one rather than
    // emitting an action that would be refused — a plan is a statement about what will happen, and
    // an entry nobody intends to apply makes it a worse one. The unverified profile is a health
    // finding and an incident, which is where §38.1 puts it.
    if let Some(profile) = &desired.profile {
        let differs = actual.profile_digest.as_deref() != Some(profile.digest.as_str());
        if differs && profile.verified {
            // §33.4's asymmetry, applied to limits. A profile that allows no more than the one in
            // force on every ceiling is a narrowing and takes effect immediately; anything else
            // waits. Where no profile is in force there is nothing to compare against, so the new
            // one is treated as a widening — which it is, relative to whatever the vendor defaults
            // allow.
            let gate = match &actual.profile_limits {
                Some(current) if profile.limits.narrows(current) => Gate::Open,
                _ => Gate::SafeState,
            };
            actions.push(Action::new(
                4,
                ActionKind::ApplyProfile {
                    name: profile.name.clone(),
                    digest: profile.digest.clone(),
                },
                gate,
            ));
        }
    }

    // Revocations first, and unconditionally. Taking a capability away is the safe direction and
    // never waits — including from a running package, which is the whole point: an operator
    // revoking a grant from a policy that is misbehaving must not be told to wait for it to stop.
    for (id, token) in &actual.grants {
        let wanted = desired
            .package(id)
            .is_some_and(|package| package.grants.iter().any(|g| g == token));
        if !wanted {
            actions.push(Action::new(
                4,
                ActionKind::RevokeCapability {
                    id: id.clone(),
                    token: token.clone(),
                },
                Gate::Open,
            ));
        }
    }

    for package in &desired.packages {
        for token in &package.grants {
            let held = actual
                .grants
                .iter()
                .any(|(id, existing)| id == &package.id && existing == token);
            if !held {
                actions.push(Action::new(
                    4,
                    ActionKind::GrantCapability {
                        id: package.id.clone(),
                        token: token.clone(),
                    },
                    Gate::PackageStopped(package.id.clone()),
                ));
            }
        }
    }

    // ── 5. Start and stop ───────────────────────────────────────────────────
    //
    // Stopping is always allowed. Starting waits, because a policy that begins producing commands
    // is a change in what the robot is doing.
    for entry in &actual.units {
        let wanted = desired.package(&entry.id).map(|package| package.state);
        let should_run = wanted == Some(RunState::Running);
        // A running package whose activation is pending has to be stopped, or the swap its own
        // gate is waiting for never becomes possible. Without this an upgrade to a *running*
        // policy is a plan that defers for ever: nothing wants it stopped, so nothing stops it,
        // so the activation never opens. Found by upgrading one on a machine with real systemd,
        // where it is the ordinary case rather than the interesting one.
        let swap_pending = swap_pending(desired, actual, &entry.id);
        // A unit a live skill invocation started is owned by that invocation, not by this
        // plan: desired state never names it, so `!should_run` is true for ever, and without
        // this clause the reconciler kills every skill-started unit on the pass after it
        // starts — holding a skill's lifetime to one reconcile interval on every robot. The
        // lifecycle stops its own unit when the invocation ends (`Effect::Stop`); a unit left
        // over with no live invocation — the daemon died mid-skill — is reaped by this same
        // rule on the next pass, because the set is empty again. Two writers, one story.
        let skill_owned = actual.live_skills.contains(&entry.id);
        match entry.state {
            UnitState::Running if (!should_run || swap_pending) && !skill_owned => {
                actions.push(Action::new(
                    5,
                    ActionKind::StopPackage {
                        id: entry.id.clone(),
                    },
                    Gate::Open,
                ))
            }
            // A failed unit is left alone. §33.5: after the crash-loop threshold the package is
            // marked failed and left alone, and a reconciler that restarted it would be the
            // infinite restart loop the threshold exists to stop — one iteration slower.
            UnitState::Failed => {}
            // In flight. The previous action has not finished, and acting again is how two starts
            // race each other.
            UnitState::Transitioning => {}
            _ => {}
        }
    }

    for package in &desired.packages {
        if package.state != RunState::Running {
            continue;
        }
        // A package cannot start before it is activated, and the plan is ordered rather than
        // conditional: the activation is at stage 2 or 3, this is at stage 5, and a pass that could
        // not activate simply does not reach a state where starting is emitted.
        let ready = actual
            .installed(&package.id)
            .is_some_and(|entry| entry.activated && entry.digest == package.digest);
        let state = actual.unit(&package.id).map(|unit| unit.state);
        if ready
            && !matches!(
                state,
                Some(UnitState::Running) | Some(UnitState::Failed) | Some(UnitState::Transitioning)
            )
        {
            actions.push(Action::new(
                5,
                ActionKind::StartPackage {
                    id: package.id.clone(),
                },
                Gate::SafeState,
            ));
        }
    }

    // ── 6. Leases ───────────────────────────────────────────────────────────
    //
    // Follows from 5: a lease held by something that is not running is a lease nothing can be using,
    // and leaving it held would refuse the capability to the package that should have it (§25.2).
    for (id, token) in &actual.leases {
        // A lease held by something that is not running is a lease nothing can be using, and
        // leaving it held refuses the capability to the package that should have it. So is one
        // held for a token the holder is no longer granted — a revocation at stage 4 that left the
        // lease standing would take the permission away and keep the exclusivity.
        let running = actual.is_running(id);
        let granted = desired
            .package(id)
            .is_some_and(|package| package.grants.iter().any(|g| g == token));
        if !running || !granted {
            actions.push(Action::new(
                6,
                ActionKind::ReleaseLease {
                    id: id.clone(),
                    token: token.clone(),
                },
                Gate::Open,
            ));
        }
    }

    // Stable sort: the stage decides, and within a stage the order the walks produced is kept. A
    // plan that reordered itself between two runs over the same inputs would make a diff of two
    // reports meaningless, and an operator comparing them would be reading noise.
    actions.sort_by_key(|action| action.stage);

    Plan { actions }
}

/// Whether this package has an activation waiting on it stopping.
///
/// The stop is emitted for it rather than the gate being relaxed, and the difference matters. §33.4
/// gates an activation on a safe state; this edition additionally waits for the package to stop,
/// because a `current` link pointing at a version different from the one executing is a report that
/// lies to every operator who reads it. §33.5 is what makes that affordable: a restart is never a
/// motion event, because the kernel holds throughout.
fn swap_pending(desired: &DesiredState, actual: &Actual, id: &str) -> bool {
    let Some(package) = desired.package(id) else {
        return false;
    };
    actual
        .installed(id)
        .is_some_and(|entry| !entry.activated || entry.digest != package.digest)
}

/// What gates activating a package of this kind.
///
/// A driver always waits: §33.4 says changing the driver changes the body. Everything else waits
/// only while it is running — activating a policy nobody is running is a symlink swap on an inert
/// blob.
fn activation_gate(kind: PackageKind, id: &str, actual: &Actual) -> Gate {
    if kind == PackageKind::Driver {
        Gate::SafeState
    } else if actual.is_running(id) {
        Gate::PackageStopped(id.to_string())
    } else {
        Gate::Open
    }
}
