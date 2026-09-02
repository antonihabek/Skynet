//! The loop (§33.3), and what happens when the daemon itself is in trouble (§33.8).
//!
//! ```text
//! loop:
//!   desired ← cached desired state on disk        (refreshed from the Hub when reachable)
//!   actual  ← observe()
//!   plan    ← diff(desired, actual)               ordered, §33.4
//!   for action in plan:
//!       if action.gate is not open: defer(action); continue
//!       apply(action); observe(); record()
//!   report(generation, actual, health, attestation)
//! ```
//!
//! [`Daemon::reconcile`] is that, once. It is called on a timer and on any event, and running it
//! twice changes nothing the first run already did — which is a property of [`skynetd::plan`] and
//! is tested there, over a model of the machine. What this module adds is the half that can fail:
//! files that are not there, processes that will not start, and a specification whose §12 and §34
//! are not written yet.
//!
//! # An action nobody can apply is reported, never skipped
//!
//! [`Outcome::Unimplemented`] exists because OTA (§12) and the outbound fetcher (§34) are not
//! written. A supervisor that dropped those actions would converge on a lie: the plan would come
//! back empty and an operator would read "up to date" about a robot that had ignored an OS update.
//! So they stay in the plan, they are reported with the reason, and the generation is **not**
//! advanced past them.
//!
//! # §33.8's ladder, and why a decode failure is not an empty desired state
//!
//! | Failure | Behaviour |
//! |---|---|
//! | Crash | systemd restarts it; it re-observes and reconciles from disk. Nothing is lost, because nothing important lived only in memory. |
//! | Corrupt desired state | Falls back to the last-known-good copy, then to "run nothing". It never invents one. |
//!
//! [`load_desired`] is that ladder, and it reports which rung it landed on. "The Hub says run
//! nothing" and "we could not read what the Hub said" are the same *plan* and completely different
//! *facts*, and an operator has to be able to tell them apart.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use skynet_contracts::cell::{cell_environment, cell_from_json};
use skynet_contracts::live::{CalibrationFact, LiveInputs};
use skynet_contracts::opstate::{
    evaluate as evaluate_state, Conditions, OperatingState, StateReason, Transition,
};
use skynet_json::Json;
use skynetd::actual::Actual;
use skynetd::desired::{DesiredState, PackageKind, RunState};
use skynetd::health::{Health, PackageHealth, Readiness};
use skynetd::lease::LeaseTable;
use skynetd::reconcile::{Action, ActionKind, Plan, SafeState};
use skynetd::supervise::{CrashPolicy, Supervision};

use crate::launch::{Cpus, LaunchError, Launcher, Limits, Unit};
use crate::layout::{Layout, PackageId};
use crate::observe::{self, Slots};
use crate::store::{Store, StoreError};

/// Which rung of §33.8's ladder the desired state came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesiredSource {
    /// The current document, read and decoded.
    Current,
    /// The current document did not decode; this is the last one that did.
    LastKnownGood {
        /// Why the current one was refused, naming the field.
        because: String,
    },
    /// Neither decoded, or neither exists. The desired state is "run nothing", which is the safe
    /// default and is **not** the same fact as the Hub asking for nothing.
    RunNothing {
        /// Why, in a sentence an operator can act on.
        because: String,
    },
}

impl DesiredSource {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::LastKnownGood { .. } => "last_known_good",
            Self::RunNothing { .. } => "run_nothing",
        }
    }

    /// The reason, where there is one.
    pub fn because(&self) -> Option<&str> {
        match self {
            Self::Current => None,
            Self::LastKnownGood { because } | Self::RunNothing { because } => Some(because),
        }
    }
}

/// How many passes [`Daemon::converge`] will run before giving the loop back.
///
/// §33.4 has six ordered stages, so a plan whose every action enables the next arrives in six.
/// Eight leaves room for a stage that needs two passes — removing a running package does, because
/// the removal is emitted only once the stop has been observed.
pub const MAX_PASSES: usize = 8;

/// An empty desired state: no OS version, no packages, no profile.
pub fn nothing() -> DesiredState {
    DesiredState {
        generation: 0,
        os_channel: String::new(),
        os_version: String::new(),
        packages: Vec::new(),
        profile: None,
    }
}

/// Walk §33.8's ladder.
///
/// A document that decodes is also **promoted** to the last-known-good copy, which is the only
/// place that file is written. Promoting on read rather than on fetch means the fallback is a
/// document this robot has actually reconciled against, not one it merely received.
pub fn load_desired(layout: &Layout) -> (DesiredState, DesiredSource) {
    match read_desired(&layout.desired()) {
        Ok(desired) => {
            if let Ok(bytes) = fs::read(layout.desired()) {
                let _ = observe::write_atomic(&layout.desired_last_good(), &bytes);
            }
            (desired, DesiredSource::Current)
        }
        Err(current) => match read_desired(&layout.desired_last_good()) {
            Ok(desired) => (desired, DesiredSource::LastKnownGood { because: current }),
            Err(fallback) => (
                nothing(),
                DesiredSource::RunNothing {
                    because: format!("{current}; and the last-known-good copy {fallback}"),
                },
            ),
        },
    }
}

fn read_desired(path: &Path) -> Result<DesiredState, String> {
    let text = fs::read_to_string(path).map_err(|error| format!("could not be read: {error}"))?;
    let document = Json::parse(&text).map_err(|error| format!("does not parse: {error}"))?;
    DesiredState::from_json(&document).map_err(|error| format!("does not decode: {error}"))
}

/// What applying one action did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It was applied.
    Applied,
    /// It was already true. A level-triggered reconciler emits this constantly and it is not news.
    AlreadyTrue,
    /// This edition cannot perform it, and says which subsystem is missing.
    ///
    /// Distinct from [`Outcome::Failed`], because the operator's next step differs completely:
    /// a failure is something to investigate on this robot, and this is something nobody has built.
    Unimplemented {
        /// Which subsystem, and what it would take.
        reason: String,
    },
    /// It was attempted and did not work.
    Failed {
        /// Why.
        reason: String,
    },
}

impl Outcome {
    /// Whether the machine now matches what the action asked for.
    pub fn converged(&self) -> bool {
        matches!(self, Self::Applied | Self::AlreadyTrue)
    }

    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::AlreadyTrue => "already_true",
            Self::Unimplemented { .. } => "unimplemented",
            Self::Failed { .. } => "failed",
        }
    }
}

/// One pass's record.
#[derive(Debug, Clone, Default)]
pub struct Pass {
    /// When it ran.
    pub at_ms: u64,
    /// Everything the plan held.
    pub plan: Plan,
    /// Actions applied, with what happened.
    pub applied: Vec<(Action, Outcome)>,
    /// Actions whose gate was closed, with the reason (§33.4).
    pub deferred: Vec<(Action, &'static str)>,
    /// The machine as last observed.
    pub actual: Actual,
    /// Aggregate health.
    pub health: Health,
    /// How many actions a plan taken *after* this pass still holds.
    ///
    /// This is the convergence test, and getting it wrong is how a robot claims to be running
    /// something it is not. A pass whose every action succeeded has not necessarily arrived: §33.3
    /// computes one plan from one observation, so an action that *enables* a later one — activating
    /// a package the next stage would start — does not put that later one in this pass. The only
    /// honest question is the level-triggered one asked again: given what is true now, is there
    /// anything left to do?
    pub remaining: usize,
}

impl Pass {
    /// Whether the robot has reached the desired state.
    ///
    /// One condition, because it subsumes the others: a deferred action is still in the next plan,
    /// an unimplemented one is still in the next plan, and a failed one is still in the next plan.
    /// Asking whether anything remains is therefore both simpler and stricter than checking each
    /// outcome, and it cannot be made to answer "yes" by an outcome nobody thought to enumerate.
    pub fn converged(&self) -> bool {
        self.remaining == 0
    }
}

/// The daemon: the brain, plus everything that can touch the machine.
pub struct Daemon<L: Launcher, S: Slots> {
    /// The package store.
    pub store: Store,
    /// What starts and stops things.
    pub launcher: L,
    /// What says which image is running.
    pub slots: S,
    /// The cores a package may run on (§40.2).
    pub cpus: Cpus,
    /// Restart bookkeeping, per package (§33.5).
    pub supervision: BTreeMap<String, Supervision>,
    /// Exclusive capability leases (§25.2).
    pub leases: LeaseTable,
    /// Whether the robot is somewhere a capability change may be applied.
    ///
    /// Supplied rather than inferred. The safe state is a property of the *body* and the body is
    /// the kernel's, which this daemon deliberately cannot reach (§33.1) — so a `skynetd` that
    /// decided for itself whether the robot was at a safe state would be a supervisor guessing
    /// about the one fact §33.4's barrier rests on. Until the kernel channel exists it is set by
    /// the operator, and [`SafeState::No`] is the default because deferring is the direction that
    /// reduces what the robot does.
    pub safe_state: SafeState,
    /// Where §27's machine says this robot is, as of the last observation.
    ///
    /// Starts at [`OperatingState::Post`], which is where §48.1 puts a robot that has just booted
    /// and is the only starting value that is not a claim. It is a *field* rather than a
    /// recomputation at every read because §27's machine has exactly one bit of memory — `SAFE` is
    /// sticky — and the bit has to live somewhere.
    pub operating_state: OperatingState,
    /// Why it is there, from the last observation.
    ///
    /// Kept beside the state rather than recomputed, because the two must describe one evaluation.
    /// A console showing `safe` with a reason from a different pass would be worse than one showing
    /// no reason at all — and `safe` on its own reads as a robot somebody stopped rather than as a
    /// robot nothing has observed.
    pub state_reason: StateReason,
    /// The last pass, for `GET /plan` and `skynet doctor`.
    pub last_pass: Option<Pass>,
    /// What the last §11 pull did, for `GET /intent`.
    ///
    /// `None` until one has been attempted, which on an unpaired robot is for ever — and that is a
    /// different fact from a pull that failed, which is why the route reports the two apart.
    pub last_pull: Option<crate::intent::Pulled>,
    /// §74's open telemetry window: what has been counted since the last one closed.
    ///
    /// In memory rather than on disk, deliberately. A counter that survived a reboot would be a
    /// running total in disguise, and §74.1 rules those out — a drop becomes invisible because the
    /// next total absorbs it. A reboot ends a window, and the *gap* is what the Hub sees.
    pub counting: crate::telemetry::Counting,
    /// The last release-catalogue pull (§12, §57).
    ///
    /// Kept here for `last_pull`'s reason: `GET /releases` reports what happened rather than making
    /// a request of its own, so a read on the local API can never make this robot talk to a Hub.
    pub last_release_pull: Option<crate::releases::Pulled>,
    /// Which rung the desired state came from.
    pub desired_source: DesiredSource,
    /// The desired state in force.
    pub desired: DesiredState,
    /// §54.4's invocations, and the loop that advances them.
    pub skills: crate::skill::Runtime,
    /// What installs an image and tells the bootloader what it thinks of a slot (§12).
    ///
    /// Boxed rather than a third type parameter, and the reason is proportion: `Launcher` and
    /// `Slots` are read on every pass by code that is generic over them anyway, and an updater is
    /// touched twice in the life of an image. Threading a parameter through every `Daemon<L, S>` in
    /// the API, the suite and the binary to describe two calls would be a lot of signature for very
    /// little, and §33.1 already licenses this half to allocate.
    pub updater: Box<dyn crate::ota::Updater>,
}

impl<L: Launcher, S: Slots> Daemon<L, S> {
    /// Build a daemon over a store, a launcher and a slot reader.
    pub fn new(store: Store, launcher: L, slots: S) -> Self {
        // From `/`, not from the layout's root. A [`Layout`] roots `/data` and `/run` so a store can
        // be exercised against a temporary directory; the CPU topology is a property of the machine
        // and lives at `/sys` on every one of them. Reading it through the layout meant a daemon
        // under a test root found no CPUs and refused to launch anything, which is a plausible
        // shape for a real bug on a robot whose `/data` is a separate mount.
        let cpus = Cpus::observe(Path::new("/"));
        let desired_source = DesiredSource::RunNothing {
            because: "nothing has been loaded yet".to_string(),
        };
        Self {
            store,
            launcher,
            slots,
            cpus,
            supervision: BTreeMap::new(),
            leases: LeaseTable::new(),
            safe_state: SafeState::No,
            operating_state: OperatingState::Post,
            state_reason: StateReason::PostIncomplete,
            last_pass: None,
            last_pull: None,
            counting: crate::telemetry::Counting::default(),
            last_release_pull: None,
            desired_source,
            desired: nothing(),
            skills: crate::skill::Runtime::new(),
            updater: Box::new(crate::ota::RaucUpdater),
        }
    }

    /// §19's gate: a driver activates only if its signature verifies against a trusted key.
    ///
    /// Returns the refusal when there is one, and `None` when activation may proceed.
    ///
    /// **Why only drivers, and why here.** §19 is specific: *"a driver runs native and must
    /// therefore be signed by a key the operator has trusted."* A driver is the one kind admission
    /// lets out of the sandbox (§8.3), so it is the one kind where an unverified signature is a
    /// stranger's code with the safety argument removed. Every other kind carries its findings and
    /// installs, because nothing on a robot can *produce* a signature — the Hub signs releases —
    /// and refusing them would refuse every local install and turn this gate into a flag.
    ///
    /// At activation rather than at stage, because a staged blob is inert (§37.1) and the symlink
    /// swap is the consequential moment. And the trust store is read on the pass that needs it, so
    /// a key an operator adds takes effect on the next reconcile rather than at the next restart.
    ///
    /// Refused, not deferred. [`Outcome::Failed`] is a decision that will not change on its own,
    /// which is the truth here: the same bytes and the same trust store give the same answer every
    /// pass, and reporting it as "waiting" would suggest otherwise.
    pub(crate) fn driver_signature_refusal(&self, id: &str, digest: &str) -> Option<Outcome> {
        let package = self.desired.package(id)?;
        if package.kind != PackageKind::Driver {
            return None;
        }

        let trust = crate::trust::TrustStore::load(self.store.layout());
        let signed = package
            .signature
            .as_ref()
            .and_then(|raw| crate::trust::SignedBy::from_hex(&raw.key, &raw.value).ok());
        let verdict = trust.check(&crate::trust::package_message(digest), signed.as_ref());
        if verdict.state == skynet_contracts::admission::SignatureState::Verified {
            return None;
        }

        // The malformed case is worth its own sentence: a signature that arrived and could not be
        // read is not the same fact as no signature, and reporting it as `absent` would send
        // somebody to ask a publisher for what they already sent.
        let malformed = package.signature.is_some() && signed.is_none();
        let detail = if malformed {
            "a signature accompanied it and could not be read — the key or the signature is not \
             the hex it should be"
                .to_string()
        } else {
            verdict.detail.clone()
        };

        Some(Outcome::Failed {
            reason: format!(
                "§19: {id} is a driver, so it runs native and must be signed by a key this robot \
                 trusts. Its signature is `{}` — {detail}. The blob stays staged and inert; add \
                 the signer to state/trust.json, or install a build somebody you trust has signed",
                if malformed {
                    "failed"
                } else {
                    verdict.state.as_str()
                }
            ),
        })
    }

    /// Re-read the operator's safe-state declaration (§33.4).
    ///
    /// **This is a stand-in, and it is important that it reads like one.** Whether the robot is at a
    /// safe state is a fact about the *body*, and the body is the Safety Kernel's — which this
    /// daemon deliberately cannot reach (§33.1). Until that channel exists there is nobody to ask,
    /// so an operator writes `yes` into a file and takes responsibility for it, and everything that
    /// reports the safe state says `declared` beside it rather than presenting it as measured.
    ///
    /// Read every pass rather than at start-up, because §33.4's barrier is re-evaluated every pass:
    /// a robot that reaches a safe state must not have to be restarted before anything notices.
    ///
    /// Anything that is not exactly `yes` is [`SafeState::No`], including an unreadable file and a
    /// file holding something nobody anticipated. That asymmetry is §33.4's throughout: the failure
    /// mode of a typo is an update that waits, and the failure mode of a lenient parse is a driver
    /// swapped under a moving arm.
    pub fn refresh_safe_state(&mut self) {
        let path = self.store.layout().safe_state();
        let declared = match fs::read_to_string(&path) {
            Ok(text) if text.trim() == "yes" => SafeState::Yes,
            _ => SafeState::No,
        };
        // Narrowed by §27's machine, never widened by it. Both sources may close the barrier and
        // neither may open it alone — see [`SafeState::and`]. On this edition the machine says
        // `safe`, because there is no kernel and no RHAL, so the operator's declaration is still
        // what decides; when the kernel channel exists and reports a policy commanding, the
        // declaration stops being able to outvote it.
        self.safe_state = declared.and(SafeState::from_state(self.operating_state));
    }

    /// Re-evaluate §27's operating state over what this process can observe.
    ///
    /// **Most of §27's conditions are not `skynetd`'s to observe, and this method is written so
    /// that shows.** §33.1 keeps the daemon away from the actuator path — CI asserts it cannot even
    /// depend on `skynet-rhal` — and the kernel, the RHAL, the guard set and the homing state are
    /// all on the far side of that line, exactly as §48.5 found for five of §48.2's eight checks.
    /// So they are left at their defaults, which are the least capable readings, and the machine
    /// answers `safe` with `rhal_lost`.
    ///
    /// That is not a placeholder producing a plausible-looking state. It is the true one:
    /// [`crate::post`] already says as much about the same edition — "`may_run` is false, and that
    /// is the correct answer for a robot with no kernel, no driver and no proven stop path". What
    /// this method adds is that the answer now has a *reason* attached which names the missing
    /// input, rather than being a string three modules spell differently.
    ///
    /// The self-test is the one condition this process owns, and it is supplied. `wall_s` is passed
    /// for the reason it is everywhere else: calibration expiry is a comparison against a clock,
    /// and a function that read one could not be tested against a fixed one (§35.2).
    pub fn refresh_operating_state(&mut self, wall_s: Option<f64>) -> Transition {
        let report = crate::post::run(self.store.layout(), wall_s);
        let conditions = Conditions {
            post: report.post_verdict(),
            // Everything below is §33.1's line. Left at the default rather than assigned a
            // hopeful value, because a supervisor that reported a plausible fleet status from
            // nothing is the exact failure this project is arranged around.
            ..Conditions::default()
        };
        let transition = evaluate_state(self.operating_state, &conditions);
        self.operating_state = transition.to;
        self.state_reason = transition.reason;
        transition
    }

    /// Re-read the desired state from disk, walking §33.8's ladder.
    pub fn reload(&mut self) {
        let (mut desired, source) = load_desired(self.store.layout());
        self.verify_profile(&mut desired);
        self.desired = desired;
        self.desired_source = source;
    }

    /// §38.1's verification, done here because it is the only place that can do it.
    ///
    /// `DesiredState::from_json` sets `verified` to `false` whatever the document says, and this is
    /// what may set it to `true`: the profile's signature, over `skynet.profile.v1` and the
    /// profile's digest, checked against the trust store on this robot.
    ///
    /// It has to be here and not in the decoder for the reason [`skynetd`] is pure at all — the
    /// trust store is a file — and it has to happen *at all* because the desired state lives on the
    /// robot's disk, which is exactly where §38.1 says the interesting attacker is. A decoder that
    /// believed the document's own `"verified": true` let anybody who could edit the limits also
    /// edit the assertion that they had been checked.
    ///
    /// The unverified case emits no action at all (`reconcile.rs` step 4), so the profile is simply
    /// not applied. §38.1 also wants an incident raised for a signature that *fails* rather than one
    /// that is merely missing, and that is not written: there is no incident path yet (§34.4's
    /// outbound queue). Recorded rather than silently skipped.
    fn verify_profile(&self, desired: &mut DesiredState) {
        let Some(profile) = desired.profile.as_mut() else {
            return;
        };
        let trust = crate::trust::TrustStore::load(self.store.layout());
        let signed = profile
            .signature
            .as_ref()
            .and_then(|raw| crate::trust::SignedBy::from_hex(&raw.key, &raw.value).ok());
        let verdict = trust.check(
            &crate::trust::profile_message(&profile.digest),
            signed.as_ref(),
        );
        profile.verified = verdict.state == skynet_contracts::admission::SignatureState::Verified;

        // §38.1 asks for an incident when a profile signature *fails*, as against one merely
        // missing, and it has been recorded as unraised for two sessions for want of anywhere to
        // put it. §34.4's queue is that place.
        //
        // `Failed` only, deliberately. `absent` is a robot nobody has signed a profile for and
        // `untrusted` is a key nobody has added — both are administrative gaps, both are already
        // visible, and raising an incident for either would train an operator to close incidents
        // without reading them. `failed` is the one state where a signature was made over *other
        // bytes*, which is what §38.1's attacker — "somebody editing limits on a robot's disk" —
        // leaves behind.
        if verdict.state == skynet_contracts::admission::SignatureState::Failed {
            self.raise_profile_incident(&profile.digest, &verdict.detail);
        }
    }

    /// The context every incident carries: what this robot was, **at the fault**.
    ///
    /// §13 seals a recording at the moment something goes wrong and §34.4 delivers it whenever the
    /// link comes back — which can be hours later, or after a reboot, or after the robot has moved
    /// on two generations. So the moment the incident describes and the moment the Hub receives it
    /// are different moments, and until this existed the Hub filled the gap from
    /// `skynet_device_state` at receipt: a statement about *now*, attached to an event in the past.
    ///
    /// The two fields here are the ones a supervisor can state exactly and cheaply without an
    /// observation pass. **The three digests are not among them**, and that absence is the reason
    /// this is a method with a comment rather than three more `set` calls: §10's attestation needs a
    /// trust store, a release manifest and a descriptor read, and an incident path that did all
    /// three would be doing work on the one code path §34.4 says is never dropped.
    ///
    /// So an incident says which generation and which §27 state, and does not say which policy. The
    /// Hub reports that as a gap rather than substituting the current one.
    fn incident_context(&self) -> Json {
        let mut context = Json::obj();
        // The generation in force when this happened, not the one the robot reaches later.
        context.set("generation", Json::Num(self.desired.generation as f64));
        // §27's state at the fault. `SAFE` is sticky, so this is frequently the single most useful
        // field on the row: it says whether the robot had already stopped when the incident was
        // raised or was still moving.
        context.set("state", Json::from(self.operating_state.as_str()));
        context
    }

    /// Queue §38.1's alarm. Best effort, and loud when it fails.
    ///
    /// The limits are not applied either way — `verified` is false, so the ceilings stay where they
    /// were — so a queue that could not take this is a robot that is *safe and silent*, which is a
    /// worse thing to be quiet about than the original event. Hence the log line: the alarm not
    /// being raised is itself the thing an operator needs to know.
    fn raise_profile_incident(&self, digest: &str, detail: &str) {
        let queue = crate::outbound::Queue::of(self.store.layout());
        let mut incident = self.incident_context();
        incident.set("kind", Json::Str("profile_signature_failed".to_string()));
        incident.set("digest", Json::Str(digest.to_string()));
        incident.set("detail", Json::Str(detail.to_string()));
        incident.set(
            "note",
            Json::Str(
                "§38.1: the limits were NOT applied. A signature that verifies against nothing was \
                 made over other bytes, which is what editing limits on a robot's disk leaves \
                 behind"
                    .to_string(),
            ),
        );
        let Ok(text) = incident.to_canonical() else {
            return;
        };
        if let Err(error) = queue.enqueue(crate::outbound::Class::Incident, &text) {
            eprintln!(
                "skynetd: a profile signature FAILED and the incident could not be queued: {}",
                error.reason()
            );
        }
    }

    /// Observe the machine.
    pub fn observe(&self, now_ms: u64) -> Actual {
        observe::observe(&self.store, &self.launcher, &self.slots, now_ms)
    }

    /// One pass.
    pub fn reconcile(&mut self, now_ms: u64) -> Pass {
        let mut actual = self.observe(now_ms);
        let plan = skynetd::plan(&self.desired, &actual);

        let deferred: Vec<(Action, &'static str)> = plan
            .deferred(self.safe_state, &actual)
            .map(|(action, reason)| (action.clone(), reason))
            .collect();
        let ready: Vec<Action> = plan.ready(self.safe_state, &actual).cloned().collect();

        let mut applied = Vec::new();
        for action in ready {
            let outcome = self.apply(&action, now_ms);
            // Re-observe after every action, per §33.3's `apply(); observe(); record()`. It is not
            // free, and the reason it is worth it is the gate: an action later in the plan may be
            // gated on a package having stopped, and a supervisor working from an observation taken
            // before the stop would apply it against a robot that no longer looks like that.
            actual = self.observe(now_ms);
            applied.push((action, outcome));
        }

        // Ask the level-triggered question again. Cheap — it is a read and a pure function — and it
        // is the only thing that can distinguish "every action succeeded" from "the robot is where
        // the Hub asked for".
        let remaining = skynetd::plan(&self.desired, &actual).actions.len();

        let health = self.health(&actual, now_ms);
        let pass = Pass {
            at_ms: now_ms,
            plan,
            applied,
            deferred,
            actual,
            health,
            remaining,
        };

        // The generation is reported as applied only when the robot actually reached it. §17: the
        // platform never claims a robot is running something it has not heard from, and a daemon
        // that stamped the generation after a pass with an unimplemented action would be making
        // exactly that claim to itself.
        if pass.converged() {
            let mut document = Json::obj();
            document.set("generation", Json::Num(self.desired.generation as f64));
            if let Ok(text) = document.to_canonical() {
                let _ = observe::write_atomic(
                    &self.store.layout().state().join("generation.json"),
                    text.as_bytes(),
                );
            }
        }

        self.last_pass = Some(pass.clone());
        pass
    }

    /// Reconcile until nothing is left to do, or until the budget runs out.
    ///
    /// §33.3 computes **one** plan per iteration of the loop, from one observation, and applies it.
    /// That is deliberate and this does not change it — every pass here is a full observe, plan and
    /// apply. What it changes is how often the loop turns: a robot whose package needs activating
    /// and then starting would otherwise take two reconcile intervals to arrive, and an interval is
    /// ten seconds.
    ///
    /// The budget is what stops it spinning. An action nobody can apply — an OS update while §12 is
    /// unwritten — stays in the plan for ever by design, so a loop with no bound would turn for
    /// ever making no progress. [`MAX_PASSES`] is the bound and it is generous: §33.4 has six
    /// stages, so a plan whose actions each enable the next arrives in six.
    pub fn converge(&mut self, now_ms: u64) -> Vec<Pass> {
        let mut passes = Vec::new();
        for _ in 0..MAX_PASSES {
            let pass = self.reconcile(now_ms);
            let done = pass.converged();
            // No progress means the remaining actions are ones this pass could not apply, and
            // running the same plan again would produce the same nothing.
            let progressed = pass
                .applied
                .iter()
                .any(|(_, outcome)| matches!(outcome, Outcome::Applied));
            passes.push(pass);
            if done || !progressed {
                break;
            }
        }
        passes
    }

    /// Apply one action.
    fn apply(&mut self, action: &Action, now_ms: u64) -> Outcome {
        match &action.kind {
            ActionKind::StageOsImage { version } => self.stage_image(version),
            ActionKind::ApplyOsImage { version } => self.apply_image(version),
            #[allow(unreachable_patterns)]
            ActionKind::StageOsImage { .. } | ActionKind::ApplyOsImage { .. } => {
                Outcome::Unimplemented {
                    reason:
                        "OTA (§12) is not written: there is no bundle fetcher, no RAUC install \
                             and no health gate. The action stays in the plan rather than being \
                             dropped, so the robot does not report itself up to date"
                            .to_string(),
                }
            }

            ActionKind::FetchPackage { id, digest } => {
                // Already present is the answer §37.1 wants — "skipping any blob already present" —
                // and it is what makes an offline install work: `skynet install` stages a blob, and
                // the next pass finds the fetch already done.
                if self.store.layout().blob(digest).is_dir() {
                    Outcome::AlreadyTrue
                } else {
                    Outcome::Unimplemented {
                        reason: format!(
                            "no blob for {digest} and no fetcher: §34's outbound path is not \
                             written, so {id} can only be staged locally with `skynet install`"
                        ),
                    }
                }
            }

            ActionKind::ActivatePackage { id, digest } => {
                let Ok(parsed) = PackageId::parse(id) else {
                    return refuse_id(id);
                };
                if let Some(refusal) = self.driver_signature_refusal(id, digest) {
                    return refusal;
                }
                match self.store.activate(&parsed, digest) {
                    Ok(()) => Outcome::Applied,
                    Err(error) => failed(error),
                }
            }

            ActionKind::RemovePackage { id } => {
                let Ok(parsed) = PackageId::parse(id) else {
                    return refuse_id(id);
                };
                match self.store.remove(&parsed) {
                    // GC runs after any activation or removal (§37.3). Collecting here rather than
                    // on a timer alone means a robot that removed a package under disk pressure
                    // gets the space back on the same pass.
                    Ok(()) => match self.store.collect() {
                        Ok(_) => Outcome::Applied,
                        Err(error) => failed(error),
                    },
                    Err(error) => failed(error),
                }
            }

            ActionKind::ApplyProfile { name, digest } => {
                let Some(profile) = self.desired.profile.clone() else {
                    return Outcome::Failed {
                        reason: "the plan names a profile the desired state no longer carries"
                            .to_string(),
                    };
                };
                match write_profile(self.store.layout(), name, digest, &profile) {
                    Ok(()) => Outcome::Applied,
                    Err(error) => Outcome::Failed {
                        reason: error.to_string(),
                    },
                }
            }

            ActionKind::GrantCapability { id, token } => self.edit_grants(id, token, true),
            ActionKind::RevokeCapability { id, token } => self.edit_grants(id, token, false),

            ActionKind::StartPackage { id } => self.start(id, now_ms),

            ActionKind::StopPackage { id } => match self.launcher.stop(id) {
                Ok(()) => {
                    // A package that stopped holding the arm must not keep holding it (§25.2). This
                    // is the half of leases that only matters when something goes wrong, which is
                    // the half worth doing eagerly.
                    let released = self.leases.release_all(id);
                    if !released.is_empty() {
                        let _ = self.write_leases();
                    }
                    Outcome::Applied
                }
                Err(error) => Outcome::Failed {
                    reason: error.to_string(),
                },
            },

            ActionKind::ReleaseLease { id, token } => {
                self.leases.release(id, token);
                match self.write_leases() {
                    Ok(()) => Outcome::Applied,
                    Err(error) => Outcome::Failed {
                        reason: error.to_string(),
                    },
                }
            }
        }
    }

    fn edit_grants(&mut self, id: &str, token: &str, add: bool) -> Outcome {
        let layout = self.store.layout().clone();
        let mut grants = observe::read_grants(&layout);
        let entry = (id.to_string(), token.to_string());
        let held = grants.contains(&entry);
        if add == held {
            return Outcome::AlreadyTrue;
        }
        if add {
            grants.push(entry);
            grants.sort();
        } else {
            grants.retain(|pair| pair != &entry);
            // Revoking a grant must take the lease with it. §33.4 stages the two separately and the
            // reconciler emits a release on the next pass, but doing it here means a revocation is
            // never a permission taken away with the exclusivity left standing — not even for one
            // pass.
            self.leases.release(id, token);
            let _ = self.write_leases();
        }
        match observe::write_pairs(&layout.grants(), &grants) {
            Ok(()) => Outcome::Applied,
            Err(error) => Outcome::Failed {
                reason: error.to_string(),
            },
        }
    }

    fn write_leases(&self) -> std::io::Result<()> {
        let pairs: Vec<(String, String)> = self
            .leases
            .leases()
            .iter()
            .map(|lease| (lease.holder.clone(), lease.token.clone()))
            .collect();
        observe::write_pairs(&self.store.layout().state().join("leases.json"), &pairs)
    }

    /// Build the unit for a package and start it.
    pub(crate) fn start(&mut self, id: &str, now_ms: u64) -> Outcome {
        let Ok(parsed) = PackageId::parse(id) else {
            return refuse_id(id);
        };
        let Some(digest) = self.store.active_digest(&parsed) else {
            return Outcome::Failed {
                reason: format!("{id} has no activated version"),
            };
        };
        let manifest = match self.store.manifest(&digest) {
            Ok(manifest) => manifest,
            Err(error) => return failed(error),
        };

        let kind = manifest
            .get_present("kind")
            .and_then(Json::as_str)
            .and_then(PackageKind::parse)
            .unwrap_or(PackageKind::Service);

        // A backoff that has not expired is not a failure to start; it is the supervisor doing what
        // §33.5 says. Reported as already-true would be a lie, and as failed would make an operator
        // chase a package that is about to start on its own.
        if let Some(record) = self.supervision.get(id) {
            if record.is_failed() {
                return Outcome::Failed {
                    reason: format!(
                        "{id} is marked failed after crash-looping (§33.5). Nothing will start it \
                         but an operator"
                    ),
                };
            }
            if let Some(at) = record.next_start() {
                if now_ms < at {
                    return Outcome::Failed {
                        reason: format!("{id} is in backoff for another {} ms", at - now_ms),
                    };
                }
            }
        }

        let blob = self.store.layout().blob(&digest);
        let entrypoint = manifest
            .get("runtime")
            .and_then(|runtime| runtime.get_present("entrypoint"))
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        if entrypoint.is_empty() {
            return Outcome::Failed {
                reason: format!(
                    "{id} declares no `runtime.entrypoint`, so there is nothing to run. A package \
                     with no entry point is not a package that failed to start; it is one that was \
                     never runnable"
                ),
            };
        }
        // The entry point is a path inside the package, and it is checked with the same rule the
        // digest uses. A manifest naming `../../bin/sh` would otherwise be a package that runs
        // something else entirely, under this package's grants.
        if skynet_contracts::digest::check_path(&entrypoint).is_err() {
            return Outcome::Failed {
                reason: format!(
                    "{id} declares an entry point that is not a path inside the package: \
                     `{entrypoint}`"
                ),
            };
        }

        let unit = Unit {
            id: id.to_string(),
            kind,
            program: blob.join(&entrypoint),
            args: manifest
                .get("runtime")
                .map(|runtime| runtime.str_arr("args"))
                .unwrap_or_default(),
            working_directory: blob,
            limits: Limits::from_manifest(&manifest),
            cpus: self.cpus.clone(),
        };

        match self.launcher.start(&unit) {
            Ok(()) => {
                self.supervision
                    .entry(id.to_string())
                    .or_insert_with(|| Supervision::new(id, kind, CrashPolicy::default()))
                    .on_start();
                Outcome::Applied
            }
            Err(error) => {
                self.note_exit(id, kind, now_ms, 0.0);
                Outcome::Failed {
                    reason: LaunchError {
                        id: id.to_string(),
                        detail: error.detail,
                    }
                    .to_string(),
                }
            }
        }
    }

    /// The board this machine is, for §12's compatibility check.
    ///
    /// The embodiment descriptor's `robot_slug`, because that is the identifier a vendor already
    /// puts on a body and §12 does not introduce a second one. `None` on a robot that has not been
    /// told what it is — which `admit_bundle` treats as *"no mismatch established"* rather than as
    /// a refusal, so an unknown machine stays updatable.
    pub fn board(&self) -> Option<String> {
        crate::embodiment::load(self.store.layout())
            .known()
            .map(|known| known.descriptor.robot_slug.clone())
    }

    /// §12's third rule, run against the documents this robot holds.
    pub fn assess_image(&self, version: &str) -> crate::ota::Assessment {
        let layout = self.store.layout();
        crate::ota::assess(
            layout,
            &crate::trust::TrustStore::load(layout),
            version,
            &self.slots.read().active,
            self.board(),
            &self.desired.os_channel,
        )
    }

    /// Stage an image: assess it, then install it into the inactive slot.
    ///
    /// **Assessed before it is fetched, not after.** §12's third rule refuses an update that
    /// changes the Safety Kernel without a bench pass, and refusing it *after* the bytes are in the
    /// other slot would leave a robot one `mark-good` away from running something nobody certified.
    /// Staging is also where the refusal is cheapest: nothing has rebooted, and the operator reads
    /// one report rather than discovering it at the safe-state barrier.
    fn stage_image(&mut self, version: &str) -> Outcome {
        let assessment = self.assess_image(version);
        if !assessment.admitted() {
            return Outcome::Failed {
                reason: refusal_text(&assessment),
            };
        }
        let Some(bundle) = assessment.bundle_path.clone() else {
            return Outcome::Unimplemented {
                reason: format!(
                    "release `{version}` is admitted and this robot has no bundle to install. \
                     §34's inbound fetcher is not written, so an image has to be put at {} by \
                     hand",
                    self.store.layout().os().join("bundles").display()
                ),
            };
        };
        if !bundle.exists() {
            return Outcome::Unimplemented {
                reason: format!(
                    "release `{version}` names {} and no such file is on this robot. §34's \
                     inbound fetcher is not written; nothing here can go and get it",
                    bundle.display()
                ),
            };
        }
        match self.updater.install(&bundle) {
            Ok(()) => Outcome::Applied,
            Err(error) => Outcome::Failed {
                reason: error.to_string(),
            },
        }
    }

    /// Apply a staged image: assess it again, mark the next boot as the one that must prove itself,
    /// and reboot.
    ///
    /// **Assessed again**, and not because anything is expected to have changed. A stage and an
    /// apply are separated by a safe-state barrier that can be hours wide (§33.4), and in that gap
    /// an operator can revoke a key, a bench pass can be replaced, or a desired state can move the
    /// robot to another channel. Re-running the gate costs nothing and is the difference between a
    /// rule that holds and one that held when the bytes arrived.
    ///
    /// **The reboot is not here.** `skynetd` marks the slot pending and returns; what reboots is
    /// the operator or the unit's own restart policy. A supervisor that rebooted a robot from
    /// inside a reconcile pass would be a supervisor whose most destructive action has no
    /// confirmation and no audit entry of its own.
    fn apply_image(&mut self, version: &str) -> Outcome {
        let assessment = self.assess_image(version);
        if !assessment.admitted() {
            return Outcome::Failed {
                reason: refusal_text(&assessment),
            };
        }
        let budget = crate::ota::read_pending(self.store.layout())
            .map(|pending| pending.budget_ms)
            .unwrap_or(skynet_contracts::ota::DEFAULT_BOOT_BUDGET_MS);
        match crate::ota::mark_pending(self.store.layout(), version, budget) {
            Ok(()) => Outcome::Applied,
            Err(error) => Outcome::Failed {
                reason: format!(
                    "release `{version}` is admitted and the pending-boot marker could not be \
                     written: {error}. §12's health gate is what makes the rollback unattended, \
                     and applying an image with no marker would be applying it with no gate"
                ),
            },
        }
    }

    /// §12's second rule, once a pass.
    ///
    /// Called after the reconcile, because two of the three health inputs — whether the wanted
    /// packages are running and whether the plan converged — are what the pass just established.
    pub fn confirm_boot(
        &mut self,
        started_ms: u64,
        now_ms: u64,
        wall_s: Option<f64>,
        actual: &Actual,
        converged: bool,
    ) -> crate::ota::Confirmation {
        let layout = self.store.layout().clone();

        // §12's budget is a *boot* budget. A daemon that restarted twice while a driver
        // crash-looped would otherwise reset the clock on a rollback meant to be unattended, so the
        // machine's uptime is preferred and the fallback says which was used.
        let (elapsed_ms, clock) = match crate::ota::boot_elapsed_ms(layout.root()) {
            Some(elapsed) => (elapsed, "boot"),
            None => ((now_ms.saturating_sub(started_ms)) as f64, "daemon"),
        };

        let post = crate::post::run(&layout, wall_s);
        let health = self.health(actual, now_ms);
        let wanted_running = !health.packages.is_empty()
            && health
                .packages
                .iter()
                .all(|package| package.readiness != Readiness::NotRunning);

        // `Some(true)` only when there is something to be true *about*. A robot the desired state
        // asks nothing of has no package health to report, and reporting one would confirm a slot
        // on the strength of an empty list — which is how an update that broke every driver looks
        // exactly like an update that installed nothing.
        let packages_healthy = if health.packages.is_empty() {
            None
        } else {
            Some(wanted_running)
        };

        let mut updater = core::mem::replace(
            &mut self.updater,
            Box::new(crate::ota::RecordingUpdater::default()),
        );
        let confirmation = crate::ota::confirm(
            &layout,
            updater.as_mut(),
            &self.slots.read().active,
            elapsed_ms,
            clock,
            &crate::ota::BootHealth {
                post_passed: Some(post.may_run()),
                packages_healthy,
                converged: Some(converged),
            },
        );
        self.updater = updater;
        confirmation
    }

    /// §54.2's context, as far as this daemon can honestly fill it in.
    ///
    /// **Most of §54.2's branches are not `skynetd`'s to observe, and this method is written so that
    /// shows** — the same shape [`Daemon::refresh_operating_state`] takes, and for the same reason.
    /// §33.1 keeps the daemon off the actuator path, so joint positions, the gripper, the battery,
    /// the base pose and the guard set all live on the far side of a channel that does not exist.
    /// They are left absent rather than filled with plausible values, and §54.2's first rule then
    /// refuses the preconditions that named them, *with the path named*.
    ///
    /// What this process does own is the four branches below, and every one of them is a file or a
    /// decision it already makes:
    ///
    /// | Branch | Source |
    /// |---|---|
    /// | `robot.state.operating` | §27's machine, as of the last observation |
    /// | `robot.calibration.*` | [`crate::calibration::status`] over `/data/state/calibration` |
    /// | `robot.frames.*` | the embodiment descriptor's graph, flattened (§44) |
    /// | `env.*` | §55.2's cell, out of the **signed** operator profile (§38.1) |
    ///
    /// `robot.tier` is absent, and that is a finding rather than an omission: nothing on this robot
    /// decides its tier. §15 awards one from conformance evidence and §11 delivers it in desired
    /// state, and neither path is written — so a `tier` here would be this module choosing one,
    /// which is precisely the invented fact the whole arrangement exists to prevent.
    ///
    /// `skill.grants` is filled per invocation by the runtime, because a grant belongs to a package
    /// and two skills on one robot do not hold the same tokens.
    pub fn live_inputs(&self, wall_s: Option<f64>) -> LiveInputs {
        let layout = self.store.layout();
        let body = crate::embodiment::load(layout);

        // §44's graph, flattened. A graph that does not flatten reaches a precondition as the
        // validator's own fault — code, subjects and sentence — beside no resolved list at all.
        let (frames_resolved, frames_fault) =
            match body.known().and_then(|k| k.descriptor.frames.as_ref()) {
                None => (Vec::new(), None),
                Some(graph) => match skynet_contracts::kinematics::flatten(graph) {
                    Ok(flat) => (
                        flat.frames()
                            .iter()
                            .map(|frame| frame.name.to_string())
                            .collect(),
                        None,
                    ),
                    Err(fault) => (Vec::new(), Some(fault)),
                },
            };

        // §45's verdicts, flattened to the two facts §54.2 asks for: valid, and how old.
        let calibration = crate::calibration::status(layout, wall_s)
            .held
            .iter()
            .map(|held| CalibrationFact {
                set: held.set.clone(),
                validity: held.validity.clone(),
                // Both readings or nothing. An age computed against a clock this robot does not
                // have would make every record look as though it were taken this second.
                age_s: match (wall_s, held.record.as_ref().and_then(|r| r.conditions.at_s)) {
                    (Some(now), Some(taken)) if now >= taken => Some(now - taken),
                    _ => None,
                },
            })
            .collect();

        // §55.2's cell, from the profile in force. `active.json` is written only for a profile
        // whose signature verified (`reconcile.rs` step 4), so this is the signed document §55.2
        // asks for and not a second, unsigned path into the precondition context.
        let env = observe::read_json(&layout.active_profile())
            .and_then(|profile| profile.get_present("cell").cloned())
            .map(|document| cell_environment(&cell_from_json(&document)));

        LiveInputs {
            operating_state: Some(self.operating_state),
            // Not `Some(vec![])`. §33.1: there is no kernel channel, so nobody has asked which
            // guards are running, and an empty list would be this daemon claiming none are.
            guards_active: None,
            calibration,
            frames_resolved,
            frames_fault,
            env,
            ..LiveInputs::default()
        }
    }

    /// Advance §54.4's lifecycle for every live invocation, and do what it asks.
    ///
    /// Called from the same loop as [`Daemon::reconcile`] and after it, because the runtime reads
    /// which units are running and the reconciler is what changes that. Running it before would
    /// mean every skill saw the previous pass's process table, and a skill that had just exited
    /// would take one extra pass to notice.
    ///
    /// Returns what happened, so the caller can record it. The black box write is the caller's for
    /// the reason every other one is: the ring has exactly one writer and this daemon is not
    /// always it.
    pub fn advance_skills(
        &mut self,
        now_ms: u64,
        wall_s: Option<f64>,
        actual: &Actual,
    ) -> Vec<crate::skill::Report> {
        let inputs = self.live_inputs(wall_s);
        let running = crate::skill::running_set(actual);
        let grants = actual.grants.clone();

        let mut runtime = core::mem::take(&mut self.skills);
        let reports = {
            let mut cycle = crate::skill::Cycle {
                now_ms,
                inputs: &inputs,
                robot: self.operating_state,
                // §33.1 again. Supplied rather than observed, and `guards_observable` is what keeps
                // the false from being counted as an observation of nothing having tripped.
                guard_tripped: false,
                guards_observable: false,
                running: &running,
                leases: &mut self.leases,
                grants: &grants,
            };
            runtime.advance_all(&mut cycle)
        };
        self.skills = runtime;

        for report in &reports {
            self.perform(&report.effects, now_ms);
        }
        if reports.iter().any(|report| !report.effects.is_empty()) {
            let _ = self.write_leases();
        }
        reports
    }

    /// End every live invocation, releasing its leases and emitting its episode.
    ///
    /// The skill half of `POST /stop`, and it runs **before** the units are stopped. Stopping the
    /// processes first would leave the lifecycles to discover on the next pass that their skills
    /// had "returned", and §54.3 would judge them over a post-state nothing had changed —
    /// recording a `failure` for a robot an operator stopped, which is a defensible-looking number
    /// and a lie a marketplace would consume.
    pub fn stop_skills(&mut self, now_ms: u64) -> Vec<crate::skill::Report> {
        let mut runtime = core::mem::take(&mut self.skills);
        let reports = runtime.stop_all(now_ms);
        self.skills = runtime;
        for report in &reports {
            self.perform(&report.effects, now_ms);
        }
        if !reports.is_empty() {
            let _ = self.write_leases();
        }
        reports
    }

    /// Do what §54.4's lifecycle asked for.
    ///
    /// Every failure here is loud rather than swallowed, and the reason differs per effect. A
    /// `Start` that fails leaves a skill in `running` with nothing running, which the next pass
    /// reads as a skill that finished — so it is reported, and the invocation ends with the
    /// judgement §54.3 makes over a post-state nothing changed. An `Episode` that cannot be queued
    /// is the one §34.4 already has an answer for.
    fn perform(&mut self, effects: &[crate::skill::Effect], now_ms: u64) {
        use crate::skill::Effect;
        for effect in effects {
            match effect {
                Effect::Start { package } => {
                    if let Outcome::Failed { reason } = self.start(package, now_ms) {
                        eprintln!("skynetd: a skill unit would not start: {reason}");
                    }
                }
                Effect::Stop { package } => {
                    if let Err(error) = self.launcher.stop(package) {
                        eprintln!("skynetd: a skill unit would not stop: {error}");
                    }
                }
                Effect::ReleaseLeases { holder } => {
                    self.leases.release_all(holder);
                }
                Effect::Episode { body } => {
                    let queue = crate::outbound::Queue::of(self.store.layout());
                    let Ok(text) = body.to_canonical() else {
                        continue;
                    };
                    if let Err(error) = queue.enqueue(crate::outbound::Class::Episode, &text) {
                        eprintln!(
                            "skynetd: a skill episode could not be queued: {}",
                            error.reason()
                        );
                    }
                }
            }
        }
    }

    /// Record that a package exited, and let §33.5 decide what happens next.
    pub fn note_exit(&mut self, id: &str, kind: PackageKind, now_ms: u64, jitter: f64) {
        let record = self
            .supervision
            .entry(id.to_string())
            .or_insert_with(|| Supervision::new(id, kind, CrashPolicy::default()));
        record.on_exit(now_ms, jitter);
        // Whatever it held, it is not holding now.
        self.leases.release_all(id);
        let _ = self.write_leases();
    }

    /// Aggregate health (§33.6).
    ///
    /// Readiness is [`Readiness::Unknown`] for every running package, and that is the correct
    /// report rather than a gap in this function: readiness means "producing commands at its
    /// declared rate with an advancing sequence", the sequence comes from the ring the policy
    /// runtime posts into, and the policy runtime is not written. An `Unknown` on the console reads
    /// as "running, nobody has measured whether it is producing", which is true. A `Ready` would be
    /// the green tick §33.6 exists to prevent.
    pub fn health(&self, actual: &Actual, now_ms: u64) -> Health {
        let packages = actual
            .units
            .iter()
            .map(|unit| PackageHealth {
                id: unit.id.clone(),
                live: unit.state == skynetd::actual::UnitState::Running,
                readiness: if unit.state == skynetd::actual::UnitState::Running {
                    Readiness::Unknown
                } else {
                    Readiness::NotRunning
                },
                last_sequence: None,
                age_ms: now_ms.saturating_sub(actual.observed_at_ms),
            })
            .collect();

        Health {
            packages,
            // Generation lag against the Hub needs the Hub. §34.1: not reaching it is not a
            // failure, and `None` is a different fact from a lag of zero.
            generation_lag: None,
            storage_headroom: None,
            guard_trip_rate: None,
            battery: None,
            temperature_c: None,
        }
    }

    /// Stop every running package — the software stop §32.2 puts on the console.
    ///
    /// Not an emergency stop of the actuators, and the difference is worth being exact about.
    /// `skynetd` never touches an actuator (§33.1). What this does is stop the processes producing
    /// commands; the kernel then sees no fresh commands, the watchdog observes it, and the robot
    /// comes to a controlled stop. That is the whole mechanism, and it is why this is safe to expose
    /// to an unauthenticated caller (§39.1) — it can only ever reduce what the robot does.
    pub fn stop_all(&mut self) -> Vec<(String, Outcome)> {
        let running: Vec<String> = self
            .launcher
            .observe()
            .unwrap_or_default()
            .into_iter()
            .filter(|unit| unit.state != skynetd::actual::UnitState::Stopped)
            .map(|unit| unit.id)
            .collect();

        let mut outcomes = Vec::new();
        for id in running {
            let outcome = match self.launcher.stop(&id) {
                Ok(()) => {
                    self.leases.release_all(&id);
                    Outcome::Applied
                }
                Err(error) => Outcome::Failed {
                    reason: error.to_string(),
                },
            };
            outcomes.push((id, outcome));
        }
        let _ = self.write_leases();
        outcomes
    }
}

/// A bundle refusal, as one sentence with the findings under it.
///
/// §64: the refusal names the fix, and for an OS update there are usually several — a signature to
/// trust *and* a bench pass to book. Reporting the first would send an operator round the loop
/// once per finding, at a reboot each.
fn refusal_text(assessment: &crate::ota::Assessment) -> String {
    if let Some(reason) = &assessment.because {
        return reason.clone();
    }
    let mut text = format!("release `{}` was refused", assessment.version);
    if let Some(verdict) = &assessment.verdict {
        for fault in &verdict.faults {
            if fault.severity == skynet_contracts::admission::Severity::Error {
                text.push_str(&format!("\n  {}: {}", fault.code.as_str(), fault.message));
            }
        }
    }
    if let Some(note) = &assessment.bench_note {
        text.push_str(&format!("\n  bench pass: {note}"));
    }
    text
}

fn refuse_id(id: &str) -> Outcome {
    Outcome::Failed {
        reason: format!(
            "`{id}` is not a package id this robot will build a path from. The desired state names \
             packages that become directories under /data, so an id is checked before it is used"
        ),
    }
}

fn failed(error: StoreError) -> Outcome {
    Outcome::Failed {
        reason: error.to_string(),
    }
}

/// Write the operator profile in force (§38.3).
///
/// The whole profile rather than only its ceilings, because §55.2 puts the cell descriptor in this
/// document and a writer that dropped it would give the cell a second, unsigned delivery path — or
/// no delivery path at all, which is where it had been for two sessions. Only a **verified** profile
/// reaches here (`reconcile.rs` step 4 emits no action for an unverified one), so a cell in
/// `active.json` is by construction a cell somebody signed.
fn write_profile(
    layout: &Layout,
    name: &str,
    digest: &str,
    profile: &skynetd::desired::DesiredProfile,
) -> std::io::Result<()> {
    let limits = &profile.limits;
    let mut body = Json::obj();
    body.set_opt(
        "joint_velocity_max_rad_s",
        limits.joint_velocity_max_rad_s.map(Json::Num),
    )
    .set_opt(
        "joint_torque_max_nm",
        limits.joint_torque_max_nm.map(Json::Num),
    )
    .set_opt(
        "base_velocity_max_m_s",
        limits.base_velocity_max_m_s.map(Json::Num),
    )
    .set_opt(
        "max_delta_per_cycle",
        limits.max_delta_per_cycle.map(Json::Num),
    );

    let mut document = Json::obj();
    document
        .set("name", Json::from(name))
        .set("digest", Json::from(digest))
        .set("limits", body);
    // Absent when the profile carries none. An empty object would be a cell with no name and no
    // volumes, which `validate_cell` reports as unnamed and `cell_workspace_layer` correctly draws
    // no layer for — but which a reader would have to interpret, where an absent key needs no
    // interpretation at all.
    if let Some(cell) = &profile.cell {
        document.set("cell", cell.clone());
    }

    let text = document
        .to_canonical()
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    observe::write_atomic(&layout.active_profile(), text.as_bytes())
}

/// Whether a package should be running, for a report.
pub fn wanted_state(desired: &DesiredState, id: &str) -> Option<RunState> {
    desired.package(id).map(|package| package.state)
}
