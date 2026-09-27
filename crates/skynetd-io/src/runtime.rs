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
    evaluate as evaluate_state, Conditions, KernelState, OperatingState, RhalState, StateReason,
    Transition,
};
use skynet_json::Json;
use skynetd::actual::Actual;
use skynetd::desired::{DesiredState, PackageKind, RunState};
use skynetd::health::{Health, PackageHealth, Readiness};
use skynetd::lease::LeaseTable;
use skynetd::reconcile::{Action, ActionKind, Plan, SafeState};
use skynetd::supervise::{CrashPolicy, RestartDecision, Supervision};

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
    /// Nothing was done, nothing is wrong, and the next pass will try again.
    ///
    /// The fifth outcome, and it arrived with the bundle fetcher (§12, §34.3). §34.1 assumes the
    /// robot is offline, so *"no link is up"* and *"the only link charges by the megabyte"* are the
    /// ordinary state of a fleet rather than faults in a machine — and reporting them as
    /// [`Outcome::Failed`] would put a red line on a console every ten seconds for a robot that is
    /// behaving exactly as designed. It is not [`Outcome::Unimplemented`] either: nothing is
    /// missing. Something is being waited for, and §34.3 requires that the wait be visible.
    Waiting {
        /// What is being waited for.
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
            Self::Waiting { .. } => "waiting",
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
    /// Which run of each remote policy's host was last heard from, and whether it said it ended
    /// (§90.2, §74.6). Across windows, in memory, for `counting`'s reason.
    pub provenance: crate::provenance::Continuity,
    /// Where §13's ring had got to when §54.4's lifecycle was last advanced.
    ///
    /// A second cursor beside `counting.opened_sequence` rather than a shared one, because the two
    /// windows are different: telemetry's is between two *telemetry* closes and this is between two
    /// *skill* passes, and a shared cursor would make each one's read consume the other's interval.
    ///
    /// Zero is a robot that has not advanced a skill yet — or has no recorder — and reads as "count
    /// the whole recording", which is what a first pass should see.
    pub skills_sequence: u64,
    /// Where §27's `kernel` condition was counted from, advanced every pass.
    ///
    /// Separate from [`skills_sequence`](Self::skills_sequence) even though both are cursors over
    /// the same ring, because they answer over different intervals and sharing one would make each
    /// consume the other's evidence: a pass that counted interventions for an episode would leave
    /// the kernel observation nothing to see, and the robot would report its kernel down.
    pub kernel_sequence: u64,
    /// What the last pass could see of the Safety Kernel, kept so `GET /state` reports the reading
    /// rather than re-deriving one from a ring the route would have to open.
    pub kernel_sighting: crate::blackbox::KernelSighting,
    /// What the last pass could see of the RHAL, kept for the same reason.
    pub rhal_sighting: crate::blackbox::RhalSighting,
    /// What the last pass could see of §24's emergency-stop chain, kept for the same reason.
    pub stop_sighting: crate::blackbox::StopSighting,
    /// What the last pass could see of §27's guard hold, kept for the same reason.
    pub hold_sighting: crate::blackbox::HoldSighting,
    /// What the last pass could see of §27's guard **set**, kept for the same reason.
    ///
    /// The one sighting that is not off §13's ring: §7.6's partition resolves on this daemon, and
    /// §45.5's expiry list is beside it. Both are re-read each pass rather than cached from
    /// start-up, because a calibration expires on the clock and a profile arrives from the Hub.
    pub guards_sighting: crate::limits::GuardsSighting,
    /// What the last pass could see of §27's `policy` condition.
    ///
    /// Off the store and the unit this daemon launched, never off §25.2's lease table — which is
    /// why the one case that turns on a lease reports itself unobserved rather than guessing.
    pub policy_sighting: crate::shape::PolicySighting,
    /// What the last pass could see of §27's `homing` condition.
    ///
    /// Off §24's two-bit field on the same newest cycle as `stop`, and refused on the same terms:
    /// §26.4 fills a failed read's flags from the previous cycle, so they are not a reading.
    pub homing_sighting: crate::blackbox::HomingSighting,
    /// What the last pass could see of §80's proximity reading.
    ///
    /// Not one of §27's conditions — it feeds §7.2's partition, which §27 then reads as
    /// `guards_unavailable`. Kept here for the same reason the others are: the routes resolve
    /// limits on demand and must resolve them against the pass's reading rather than take their
    /// own, or `GET /limits` and the operating state could disagree about one instant.
    pub proximity_sighting: crate::blackbox::ProximitySighting,
    /// What the last pass's newest cycle measured of the body, for §54.2's `robot.state.*` (F147).
    ///
    /// Off the same newest cycle as `rhal`, `stop` and `homing`, so a skill's precondition and
    /// §27's state are readings of one instant.
    pub body_reading: crate::blackbox::BodyReading,
    /// Who asked this daemon to stop, when somebody has (§82.4).
    ///
    /// A **request** rather than an action, and the split is the whole design. The route that takes
    /// it cannot stop the daemon: it is serving a connection inside the loop, the mDNS responder is
    /// on the other thread, and the recording is the loop's to seal. So it records who asked and
    /// the loop performs it at the top of its next turn, which is the same shape §33.3 gives every
    /// other change — the API states an intent and the loop converges on it.
    pub shutdown_requested: Option<String>,
    /// §27: one named clearance, consumed against the next observed interval, never saved for later.
    pub safe_clearance_requested: Option<String>,
    /// The last clearance result, including refusals, for the operator who requested it.
    pub safe_clearance_note: Option<String>,
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
    /// This process's own counters: passes, overruns, requests.
    ///
    /// Beside everything else the routes read rather than in the binary, because the loop that
    /// increments them and the route that serves them both hold the daemon — a `Metrics` in
    /// `main.rs` would have to be threaded through every `Context` construction to reach the
    /// one route that reads it.
    pub metrics: crate::metrics::Metrics,
    /// What the kernel host died of, once this process has noticed (§64).
    ///
    /// Set by [`Daemon::notice_kernel_crash`], which each pass calls and which takes the file
    /// the first time it finds it. The report then lives here until the next restart — the
    /// file is already gone, so this binding is the only copy, and `GET /health` serves it
    /// as `kernel_exit` for as long as this process runs.
    pub last_kernel_crash: Option<skynet_crash::CrashReport>,
    /// What installs an image and tells the bootloader what it thinks of a slot (§12).
    ///
    /// Boxed rather than a third type parameter, and the reason is proportion: `Launcher` and
    /// `Slots` are read on every pass by code that is generic over them anyway, and an updater is
    /// touched twice in the life of an image. Threading a parameter through every `Daemon<L, S>` in
    /// the API, the suite and the binary to describe two calls would be a lot of signature for very
    /// little, and §33.1 already licenses this half to allocate.
    pub updater: Box<dyn crate::ota::Updater>,
    /// What goes and gets an image (§12, §34.3).
    ///
    /// Boxed for `updater`'s reason and used the same way — a handful of calls in the life of an
    /// image, against a pair of type parameters read on every pass.
    pub fetcher: Box<dyn crate::bundle::Fetcher>,
    /// §49.1's warning channel: what makes the noise before autonomous motion.
    ///
    /// Boxed for `updater`'s reason, and used far less often than either of the other two — a tone
    /// sounds on a transition into a commanding state, which on a working robot is a handful of
    /// times a shift.
    pub sounder: Box<dyn crate::audio::Sounder>,
    /// What the last warning did, when one was called for.
    ///
    /// In memory rather than on disk, and it is not a record: §38.3's records are the audit log and
    /// the ring, both written at the moment. This is what the surfaces read, and a robot that had
    /// never entered a commanding state has nothing to say rather than a stale `ok`.
    pub last_warning: Option<crate::audio::Sounded>,
    /// §36's sensors, and the one instant they remember (§36.3's "sustained").
    ///
    /// Here for `counting`'s reason: a throttle window is a duration, a duration is not observable
    /// in one reading, and the instant it opened has to live somewhere. In memory rather than on
    /// disk, and by the same argument — a throttle window that survived a reboot would be a robot
    /// reporting an hour of continuous throttling across a power cycle it spent switched off.
    pub power: crate::power::Sensors,
    /// The fraction §34.4's backoff is jittered by, supplied rather than drawn (§35.2).
    ///
    /// A field rather than an argument threaded through `reconcile`, `apply` and six action arms to
    /// reach the one that fetches. The daemon loop sets it per pass from the monotonic clock; a test
    /// sets it once and gets a curve it can assert on. The default is the top of the window, which
    /// is the un-jittered delay: a single robot retrying on the full curve is correct, and it is a
    /// *fleet* arriving in lockstep that the jitter exists to break up.
    pub jitter: f64,
    /// The programs a remote-policy package runs (§90.2), resolved when the daemon starts. A field so
    /// a test can name programs that exist on the machine running it.
    pub remote_programs: crate::remote::RemotePrograms,
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
            provenance: crate::provenance::Continuity::default(),
            skills_sequence: 0,
            kernel_sequence: 0,
            // Nothing has been read yet, and a daemon that started with `Silent` would be
            // reporting an observation it has not made.
            kernel_sighting: crate::blackbox::KernelSighting::Unobservable(
                "no reconcile pass has looked at the recording yet",
            ),
            rhal_sighting: crate::blackbox::RhalSighting::Unobservable(
                "no reconcile pass has looked at the recording yet",
            ),
            stop_sighting: crate::blackbox::StopSighting::Unobservable(
                "no reconcile pass has looked at the recording yet",
            ),
            hold_sighting: crate::blackbox::HoldSighting::Unobservable(
                "no reconcile pass has looked at the recording yet",
            ),
            guards_sighting: crate::limits::GuardsSighting::Unobservable(
                "no reconcile pass has resolved the guard set yet",
            ),
            // Not `Nothing`: a daemon that has not looked at its store must not report that the
            // store is empty. `LeaseUnknown` is the variant that reads as unobserved, and the
            // package name is replaced by the first pass.
            policy_sighting: crate::shape::PolicySighting::LeaseUnknown {
                package: String::new(),
            },
            homing_sighting: crate::blackbox::HomingSighting::Unobservable(
                "no reconcile pass has looked at the recording yet",
            ),
            proximity_sighting: crate::blackbox::ProximitySighting::Unobservable(
                "no reconcile pass has looked at the recording yet",
            ),
            body_reading: crate::blackbox::BodyReading::Unobservable(
                "no reconcile pass has looked at the recording yet",
            ),
            shutdown_requested: None,
            safe_clearance_requested: None,
            safe_clearance_note: None,
            // From the kernel's own interfaces, not through the layout — for `cpus`'s reason two
            // fields above: the pack and the thermal zones are properties of the machine and live
            // at `/sys` on every one of them, whatever `/data` a store was rooted at.
            power: crate::power::Sensors::new(),
            last_release_pull: None,
            desired_source,
            desired: nothing(),
            skills: crate::skill::Runtime::new(),
            metrics: crate::metrics::Metrics::default(),
            last_kernel_crash: None,
            updater: Box::new(crate::ota::RaucUpdater),
            fetcher: Box::new(crate::bundle::CurlFetcher),
            sounder: Box::new(crate::audio::CommandSounder),
            last_warning: None,
            jitter: 1.0,
            remote_programs: crate::remote::RemotePrograms::resolve(),
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
    /// §54.5's declared call graph, refused at the moment a package becomes usable.
    ///
    /// Beside the driver signature and for the same structural reason: a staged blob is inert
    /// (§37.1) and the symlink swap is the consequential moment, so that is where a refusal that
    /// must hold belongs. The install path *reports* the same fault to whoever ran the command,
    /// which is where they can act on it; this is what makes it stick when nobody is watching.
    ///
    /// Refused rather than deferred, for `driver_signature_refusal`'s reason: the same manifest
    /// gives the same answer on every pass, and reporting it as waiting would suggest otherwise.
    pub(crate) fn composition_refusal(&self, id: &str, digest: &str) -> Option<Outcome> {
        let manifest = self.store.manifest(digest).ok()?;
        let fault = crate::admit::composition_fault(id, &manifest)?;
        Some(Outcome::Failed {
            reason: format!(
                "{} The blob stays staged and inert; publish a graph that terminates.",
                crate::admit::composition_refusal(id, &fault)
            ),
        })
    }

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
        if verdict.state.admits() {
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

    /// The six readings this daemon last took, gathered.
    ///
    /// Two routes and the start-up banner all describe one pass, and before `crate::observed` they
    /// disagreed about how much of it was observed. This is the other half of that fix: they take
    /// the same six from the same place rather than each naming six fields.
    pub fn sightings(&self) -> crate::observed::Sightings<'_> {
        crate::observed::Sightings {
            kernel: &self.kernel_sighting,
            rhal: &self.rhal_sighting,
            stop: &self.stop_sighting,
            hold: &self.hold_sighting,
            guards: &self.guards_sighting,
            policy: &self.policy_sighting,
            homing: &self.homing_sighting,
        }
    }

    /// Re-evaluate §27's operating state over what this process can observe.
    ///
    /// **This method's own history is the argument for how it is written.** It began with one of
    /// §27's eight conditions supplied and seven left at their defaults, because §33.1 keeps the
    /// daemon away from the actuator path — CI asserts it cannot even depend on `skynet-rhal` —
    /// and every other condition was filed as the kernel's. Seven of the eight are supplied now,
    /// and **not one of them arrived by relaxing §33.1**. Each was a case of the reason being a
    /// true sentence about the wrong place: what decides whether a supervisor may observe a
    /// condition is not whose fact it is, but whether the fact was written down somewhere this
    /// process already holds — §13's ring for four of them, §7.6's own partition for the fifth,
    /// and this daemon's store and units for the sixth.
    ///
    /// `homing` is the last, and it is the one where the original reason survives contact: §24
    /// gives the ABI no flag for it, so it is not written down anywhere at all.
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
    ///
    /// # The second condition, and how it arrives
    ///
    /// `kernel` is now supplied too, and the sentence above about §33.1 needed narrowing rather
    /// than deleting. The line §33.1 draws is around the **command path**: this process must never
    /// be between a policy and a joint. Reading a file the kernel wrote is not that, and it is not
    /// asking the kernel anything either — a turning control loop records a cycle every pass (§26)
    /// as a side effect of doing its job, and this daemon already holds the recording.
    ///
    /// It arrives as a [`KernelSighting`](crate::blackbox::KernelSighting) from the caller rather
    /// than being read here, for the reason `post` is a report and not a syscall: this method is a
    /// function over facts, and one that opened a ring could not be driven by a suite.
    ///
    /// The direction is the safety argument. Cycles seen is `Live` and **everything else** —
    /// silence, an unreadable ring, no ring at all — is `Down`. A wrong answer can only hold a
    /// robot that could have been commanding; it can never raise one.
    ///
    /// `rhal` arrives the same way and from the same records, and the pair matters more than
    /// either: a robot whose kernel is turning while its driver has stopped answering has to be
    /// able to report exactly that. `stop` and `guard_hold` are read off that same newest cycle,
    /// `guards` off §7.6's partition and §45.5's expiry, and `policy` off the store and the unit.
    ///
    /// # What seven of eight does not buy
    ///
    /// A robot that commands. `homing` is rule 5 of `derive` and defaults to `Pending`, so this
    /// edition still resolves `unhomed` however good the six readings below it are — and that is
    /// the honest size of the step rather than a disappointment. What changed is that the state now
    /// rests on readings, and the conditions a console shows beside it are measurements rather than
    /// defaults dressed as measurements.
    pub fn refresh_operating_state(
        &mut self,
        wall_s: Option<f64>,
        seen: &crate::observed::Sightings<'_>,
    ) -> Transition {
        let crate::observed::Sightings {
            kernel,
            rhal,
            stop,
            hold,
            guards,
            policy,
            homing,
        } = seen;
        let report = crate::post::run(self.store.layout(), wall_s);
        let mut conditions = Conditions {
            post: report.post_verdict(),
            // §27's second observed condition. `Down` unless a control loop wrote cycles, which is
            // the only evidence that separates a turning kernel from a hung one — an active unit is
            // both.
            kernel: kernel.state(),
            // §27's third observed condition, off the newest cycle rather than a count: `rhal` is
            // a state, so what decides it is the last thing the loop observed. See `RhalSighting`
            // for the mapping, and in particular for why a faulted drive is not a lost one.
            rhal: rhal.state(),
            // §27's fourth observed condition, off §24's flags on the same newest cycle. A cause
            // of its own rather than `operator`, because the chain gives one bit and a light
            // curtain opens it too — see `StopSighting`.
            stop: stop.cause(),
            // §27's fifth observed condition, off the guard verdict in that same newest cycle.
            // True only for a hold the kernel **enforced**: §7.4 lets a tier that cannot promise a
            // clamp record a verdict and forward the command anyway, and every robot on this
            // edition is one — so `HoldSighting::held` is false for an advisory hold and the
            // reading is still reported as an observation. A robot that said `HOLD` while its
            // command went through would be the failure §7.4 exists to prevent.
            guard_hold: hold.held(),
            // §27's sixth observed condition, and the only one not off §13's ring: §7.6's
            // partition resolves here and §45.5's expiry list sits beside it. It is what the
            // guards *can* do given what the body declares and what a calibration still believes,
            // never a reading of what the kernel had inputs for last cycle — see `GuardsSighting`,
            // which says so where the mapping is rather than here.
            guards_unavailable: guards.unavailable(),
            // §27's seventh, off the store and the unit rather than off §25.2's lease table. Never
            // `Ready` on any input — the one value that asserts a lease is the one this process
            // must not produce — and never the `None` the default used to supply, which said "there
            // is nothing to run" about a robot whose store this daemon holds and knows otherwise.
            policy: policy.state(),
            // §27's eighth, and the last to stop being a default. Off §24's homing field on the
            // same newest cycle as `stop`, and everything that is not a reported state — an
            // unreadable ring, a failed body read, a driver that declines to answer — collapses
            // onto `Pending`, which is the value that holds the robot. See `HomingSighting`, and
            // in particular for why a driver declining to answer is still an observation.
            homing: homing.state(),
            // `stop_settled` is deliberately left false. For a hardware stop no edition can
            // observe it — the bit says the chain is open and says nothing about whether the body
            // came to rest — so the machine derives `stopping` and never `stop_complete`. A robot
            // whose chain has been open for an hour goes on reporting `stopping`, which is the
            // strongest thing this evidence supports.
            //
            // Everything below is §33.1's line. Left at the default rather than assigned a
            // hopeful value, because a supervisor that reported a plausible fleet status from
            // nothing is the exact failure this project is arranged around.
            ..Conditions::default()
        };
        // §27's named clearance is one attempt against this interval, never a future fault.
        if let Some(actor) = self.safe_clearance_requested.take() {
            let eligible = self.operating_state == OperatingState::Safe
                && !actor.trim().is_empty()
                && report.may_run()
                && conditions.kernel == KernelState::Live
                && conditions.rhal == RhalState::Present
                && matches!(stop, crate::blackbox::StopSighting::Clear);
            let detail = if eligible {
                format!("SAFE clearance by {actor} applied to this observed interval (§27)")
            } else {
                format!("SAFE clearance by {actor} refused: need SAFE, a passing POST, a live kernel, a nominal RHAL and an observed clear stop; inspect /state and /post, then request again (§27)")
            };
            let at = wall_s
                .filter(|s| s.is_finite() && *s >= 0.0)
                .map(|s| skynet_contracts::time::rfc3339((s * 1000.0) as u64))
                .unwrap_or_else(|| "unknown".to_string());
            match crate::audit::Audit::of(self.store.layout()).append(
                &at,
                &actor,
                if eligible {
                    "state.clear.applied"
                } else {
                    "state.clear.refused"
                },
                "safe",
                &detail,
                Some(self.state_reason.as_str()),
            ) {
                Ok(_) => {
                    if eligible {
                        conditions.cleared_by = actor;
                    }
                    self.safe_clearance_note = Some(detail);
                }
                Err(error) => {
                    self.safe_clearance_note = Some(format!(
                        "SAFE clearance refused: audit write failed: {error}; repair the audit log and request again (§27, §38.3)"
                    ));
                }
            }
        }
        let transition = evaluate_state(self.operating_state, &conditions);

        // §49.1, before the state is committed rather than after. On this edition the daemon is
        // the thing that decides §27's state, so sounding here really is sounding before the
        // transition takes effect in this process — which is the strongest ordering available from
        // this side of §33.1's line and is not the guarantee §49.1 asks for. See `audio`'s header.
        self.warn_for_transition(&transition);

        self.operating_state = transition.to;
        self.state_reason = transition.reason;
        self.kernel_sighting = (*kernel).clone();
        self.rhal_sighting = (*rhal).clone();
        self.stop_sighting = (*stop).clone();
        self.hold_sighting = (*hold).clone();
        self.guards_sighting = (*guards).clone();
        self.policy_sighting = (*policy).clone();
        self.homing_sighting = (*homing).clone();
        transition
    }

    /// Sound §49.1's warning if this transition demands one, and report a failure to.
    ///
    /// A method rather than four lines inside [`Daemon::refresh_operating_state`], and the reason is
    /// that the interesting half of it is unreachable from there **on this edition**: §33.1 leaves
    /// five of §27's eight conditions unobserved — the kernel and the RHAL joined the self-test on
    /// 2026-09-07 — so the machine derives `safe` every pass and
    /// `warning_required` is never true. A robot here never enters a commanding state, so the tone
    /// never sounds and the incident is never raised — for the same reason §74.6's counters report
    /// nothing on a shipping robot.
    ///
    /// Split out so the consequence can be driven directly by the suite. The alternative was a path
    /// that only runs on a robot that does not exist yet, which is how a safety control ships
    /// untested.
    ///
    /// # Raised, because it cannot be refused
    ///
    /// §49.1 makes the tone a control an operator's risk assessment relies on. This process cannot
    /// decline to enter a commanding state — §27's state is *derived from conditions*, not granted
    /// by a supervisor — so the strongest available answer to "the robot is about to move and
    /// nobody was warned" is an incident, which §34.4 never drops, plus a line on the console and a
    /// field on `GET /state`. That is weaker than refusing and it is the honest answer for a
    /// process that does not hold the switch.
    pub fn warn_for_transition(
        &mut self,
        transition: &Transition,
    ) -> Option<crate::audio::Sounded> {
        let channel = crate::audio::Channel::of(self.store.layout());
        let sounded = crate::audio::warn_for(self.sounder.as_mut(), &channel, transition)?;
        if !sounded.ok() {
            let mut extra = Json::obj();
            extra.set("detail", Json::Str(sounded.detail()));
            extra.set("from", Json::from(transition.from.as_str()));
            extra.set("to", Json::from(transition.to.as_str()));
            extra.set("channel", channel.to_json());
            self.raise_incident("warning_did_not_sound", extra);
            eprintln!(
                "skynetd: autonomous motion without a warning ({} -> {}): {}",
                transition.from.as_str(),
                transition.to.as_str(),
                sounded.detail()
            );
        }
        self.last_warning = Some(sounded.clone());
        Some(sounded)
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
        let mut incident = Json::obj();
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
        self.raise_incident("profile_signature_failed", incident);
    }

    /// Seal the black box and queue one incident (§13, §34.4).
    ///
    /// **The seal happens here rather than at the call site**, so that every incident this daemon
    /// ever raises carries a window and none of them has to remember to. §13 is unambiguous —
    /// *"sealed and uploaded on any fault, estop, or operator request"* — and the half that was
    /// missing was not the sealing, which has existed since the sixth session. It was that nothing
    /// connected a fault to a recording: `skynet_incidents.artifact_id` has been NULL on every row
    /// the platform can hold, and an investigator reading one had no way to know a window even
    /// existed.
    ///
    /// # It names the recording; it does not send it
    ///
    /// §17 treats this route as a document and the document is what crosses. A sealed window is
    /// tens to hundreds of megabytes and §34.3 puts it in the bulk class, which wants resume,
    /// progress and metering on an **upload** — the mirror of §75's fetch, and not written. So the
    /// incident carries the window's name, size, digest and record count with `uploaded: false`,
    /// which is a true statement a Hub can render: *"this incident has a 4 MB recording on the
    /// robot that nothing has collected."* The alternative — saying nothing — is what leaves an
    /// operator assuming the recording was never taken.
    ///
    /// # Nothing here can prevent the incident
    ///
    /// A seal that fails is reported **inside** the document and the incident still goes. §34.4
    /// makes this the one class that is never dropped, and a robot that swallowed an alarm because
    /// its own recorder was broken would be losing the report and the recording together — which is
    /// exactly the pair of failures that arrive at the same time.
    fn raise_incident(&self, kind: &str, extra: Json) {
        let layout = self.store.layout();
        let queue = crate::outbound::Queue::of(layout);
        let mut incident = self.incident_context();
        incident.set("kind", Json::Str(kind.to_string()));
        if let Some(fields) = extra.as_obj() {
            for (key, value) in fields {
                incident.set(key, value.clone());
            }
        }

        // The monotonic clock is not reachable from here — `reload` takes no time — and §35.2's
        // rule is that a function which reaches for one cannot be tested. Zero is the sealer's
        // documented "no instant given", and it lands in the file name, where it reads as an
        // unknown rather than as a plausible timestamp.
        match crate::blackbox::seal_existing(layout, skynet_blackbox::seal::Reason::Fault, 0) {
            Ok(Some(recording)) => {
                // §80: a window an incident names is never reclaimed. The link is recorded here,
                // by the process that just put the name into the document, rather than inferred
                // later from the seal reason — every raise on this path seals for `Fault` today,
                // so the inference would agree today and be one new incident path away from
                // freeing the evidence for an incident that names it.
                //
                // Best effort. A citation that cannot be written is a window the reclaim may take
                // once the disk is under the floor, which is bad; refusing to raise the incident
                // over it would be worse, and it is the same trade `write_manifest` makes.
                if let Err(error) =
                    crate::retention::cite(layout, &recording.name, &recording.digest, kind)
                {
                    eprintln!(
                        "skynetd: `{kind}` sealed {} and its citation could not be written \
                         ({error}); §80's reclaim may treat that window as routine",
                        recording.name
                    );
                }
                incident.set("recording", recording.to_json());
            }
            Ok(None) => {
                incident.set(
                    "recording",
                    crate::blackbox::unsealed(
                        "this robot has no black box ring, so there was no window to seal",
                    ),
                );
            }
            Err(error) => {
                // Reported in the document rather than only in a log, because the party who needs
                // it is the one reading the incident months later.
                incident.set("recording", crate::blackbox::unsealed(error.to_string()));
                eprintln!(
                    "skynetd: an incident was raised and the black box would not seal: {error}"
                );
            }
        }

        let Ok(text) = incident.to_canonical() else {
            return;
        };
        if let Err(error) = queue.enqueue(crate::outbound::Class::Incident, &text) {
            eprintln!(
                "skynetd: `{kind}` was raised and the incident could not be queued: {}",
                error.reason()
            );
        }
    }

    /// Observe the machine.
    ///
    /// Plus the one fact no outside reading can supply: which packages have a live skill
    /// invocation, which the reconciler must not stop (§54.4, and the spare clause in the
    /// plan it feeds). Read off this daemon's own skill table on the observing pass — the
    /// same discipline as every other field, only the source is a table rather than a file.
    /// Terminal invocations are not in it: their units are the lifecycle's to stop, and a
    /// leftover with no live invocation is the reconciler's to reap.
    pub fn observe(&self, now_ms: u64) -> Actual {
        let mut actual = observe::observe(&self.store, &self.launcher, &self.slots, now_ms);
        actual.live_skills = self
            .skills
            .invocations()
            .iter()
            .filter(|invocation| !invocation.terminal())
            .map(|invocation| invocation.package.clone())
            .collect();
        actual
    }

    /// Notice the kernel host's crash file, if it has appeared since the last look (§64).
    ///
    /// Called once per pass, and effective at most once per crash: the first call that finds
    /// the file takes it, logs it at error, records `kernel_crashed`, and stashes the report
    /// for `GET /health`. Later calls find the field set and do nothing — a crash is
    /// reported once, however many passes run after it. Per pass rather than per boot,
    /// because the kernel restarts in two seconds and a supervisor that only looked at its
    /// own boot would report a Tuesday crash on Friday.
    pub fn notice_kernel_crash(&mut self, recorder: Option<&mut crate::blackbox::Recorder>) {
        if self.last_kernel_crash.is_some() {
            return;
        }
        let Some(report) = skynet_crash::take_report(&self.store.layout().kernel_crash_file())
        else {
            return;
        };
        crate::log_error!(
            "the kernel host (pid {}) panicked: {}{}. The kernel_crashed event below carries it \
             into this recording; `GET /health` carries it as `kernel_exit` until the next \
             restart.",
            report.pid,
            report.message,
            if report.location.is_empty() {
                String::new()
            } else {
                format!(" at {}", report.location)
            },
        );
        if let Some(recorder) = recorder {
            let _ = recorder.event(
                0,
                skynet_blackbox::record::EventCode::KernelCrashed,
                &report.binary,
                &format!(
                    "{}{}",
                    report.message,
                    if report.location.is_empty() {
                        String::new()
                    } else {
                        format!(" at {}", report.location)
                    },
                ),
            );
        }
        self.last_kernel_crash = Some(report);
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

        // §36's gauge, read in the pass whose health it goes into. A `Pass` is the record of one
        // turn of the loop, so a battery figure in it that came from a different turn would be the
        // one thing this struct exists not to be.
        let reading = self.read_power(now_ms);
        let headroom = self.read_headroom();
        let health = self.health(&actual, now_ms, Some(&reading), headroom);
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
    /// The budget is what stops it spinning. An action nobody can apply — an OS update whose
    /// release manifest this robot has never been sent — stays in the plan for ever by design, so
    /// a loop with no bound would turn for ever making no progress. [`MAX_PASSES`] is the bound and
    /// it is generous: §33.4 has six stages, so a plan whose actions each enable the next arrives
    /// in six.
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
            ActionKind::StageOsImage { version } => self.stage_image(version, now_ms),
            ActionKind::ApplyOsImage { version } => self.apply_image(version),

            ActionKind::FetchPackage { id, digest } => {
                // Already present is the answer §37.1 wants — "skipping any blob already present" —
                // and it is what makes an offline install work: `skynet install` stages a blob, and
                // the next pass finds the fetch already done.
                if self.store.layout().blob(digest).is_dir() {
                    Outcome::AlreadyTrue
                } else {
                    self.fetch_package(id, digest, now_ms)
                }
            }

            ActionKind::ActivatePackage { id, digest } => {
                let Ok(parsed) = PackageId::parse(id) else {
                    return refuse_id(id);
                };
                if let Some(refusal) = self.driver_signature_refusal(id, digest) {
                    return refusal;
                }
                // §54.5, before anything can invoke it. The runtime's own check walks the *live*
                // ancestry and is the one that would otherwise catch this — at runtime, on a robot,
                // with leases held, which is the sentence the rule is written against.
                if let Some(refusal) = self.composition_refusal(id, digest) {
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

        // §90.2: a remote-policy package runs none of its publisher's code here. The daemon starts
        // the policy host in its place, and the policy host starts the session bridge.
        let remote = if kind == PackageKind::Policy {
            crate::remote::remote_policy(&manifest)
        } else {
            None
        };
        let unit = match remote {
            Some(Err(reason)) => {
                return Outcome::Failed {
                    reason: format!("{id} cannot run: {reason}"),
                }
            }
            Some(Ok(policy)) => {
                let Some(bridge) = self.remote_programs.bridge.clone() else {
                    return Outcome::Failed {
                        reason: format!(
                            "{id} is a remote policy and no session bridge is installed, so it \
                             could never open a session. `pip install skynet-sdk[hosted]` puts \
                             `{}` on PATH, or set SKYNET_REMOTE_BRIDGE_COMMAND for skynetd (§90.2)",
                            crate::remote::BRIDGE_PROGRAM
                        ),
                    };
                };
                let layout = self.store.layout();
                if let Err(error) = std::fs::create_dir_all(layout.remote_run()) {
                    return Outcome::Failed {
                        reason: format!(
                            "{} could not be created for {id}'s session bridge: {error}",
                            layout.remote_run().display()
                        ),
                    };
                }
                let mut env = crate::shape::rendezvous(&self.store, id);
                let sandboxing = self.sandboxing_of(id, kind, &manifest);
                env.extend(crate::remote::environment(layout, id, &policy, &bridge));
                Unit {
                    id: id.to_string(),
                    kind,
                    sandboxing,
                    program: self.remote_programs.policy_host.clone(),
                    args: vec![
                        "run".to_string(),
                        "--backend".to_string(),
                        "remote".to_string(),
                    ],
                    env,
                    working_directory: blob,
                    limits: Limits::from_manifest(&manifest),
                    cpus: self.cpus.clone(),
                }
            }
            None => match self.local_unit(id, kind, blob, &manifest) {
                Ok(unit) => unit,
                Err(outcome) => return outcome,
            },
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

    /// Which sandbox a package's unit runs in, and whether §25's socket is shared with it.
    ///
    /// The one place it is decided, for a launch and for §15's `safety.policy.isolation`, which
    /// reads a running unit back against what it was launched under — two copies of this decision
    /// would be a conformance test comparing a unit against a sandbox it was never given.
    pub(crate) fn sandboxing_of(
        &self,
        id: &str,
        kind: PackageKind,
        manifest: &Json,
    ) -> crate::launch::Sandboxing {
        let remote =
            kind == PackageKind::Policy && crate::remote::remote_policy(manifest).is_some();
        let ring = !crate::shape::rendezvous(&self.store, id).is_empty();
        crate::launch::Sandboxing {
            sandbox: crate::launch::Sandbox::of(kind, remote, ring),
            ring,
        }
    }

    /// The unit for a package that runs from its own entry point: every package but a remote policy.
    fn local_unit(
        &self,
        id: &str,
        kind: PackageKind,
        blob: std::path::PathBuf,
        manifest: &Json,
    ) -> Result<Unit, Outcome> {
        let entrypoint = manifest
            .get("runtime")
            .and_then(|runtime| runtime.get_present("entrypoint"))
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        if entrypoint.is_empty() {
            return Err(Outcome::Failed {
                reason: format!(
                    "{id} declares no `runtime.entrypoint`, so there is nothing to run. A package \
                     with no entry point is not a package that failed to start; it is one that was \
                     never runnable"
                ),
            });
        }
        // The entry point is a path inside the package, and it is checked with the same rule the
        // digest uses. A manifest naming `../../bin/sh` would otherwise be a package that runs
        // something else entirely, under this package's grants.
        if skynet_contracts::digest::check_path(&entrypoint).is_err() {
            return Err(Outcome::Failed {
                reason: format!(
                    "{id} declares an entry point that is not a path inside the package: \
                     `{entrypoint}`"
                ),
            });
        }

        let sandboxing = self.sandboxing_of(id, kind, manifest);
        if sandboxing.sandbox != crate::launch::Sandbox::Native {
            if let Some(reason) = crate::launch::accelerator_refusal(id, manifest) {
                return Err(Outcome::Failed { reason });
            }
        }

        Ok(Unit {
            id: id.to_string(),
            kind,
            sandboxing,
            program: blob.join(&entrypoint),
            args: manifest
                .get("runtime")
                .map(|runtime| runtime.str_arr("args"))
                .unwrap_or_default(),
            env: crate::shape::rendezvous(&self.store, id),
            working_directory: blob,
            limits: Limits::from_manifest(manifest),
            cpus: self.cpus.clone(),
        })
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

    /// The digest the manifest for one version names, when this robot holds that manifest.
    ///
    /// Through the assessment rather than by reading the file, so that one place decides what
    /// document answers for a version — including the refusal of a manifest filed under a name it
    /// disagrees with, which is exactly the case where a second reader would return a digest.
    pub fn assess_image_digest(&self, version: &str) -> Option<String> {
        if version.trim().is_empty() {
            return None;
        }
        self.assess_image(version).digest
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

    /// Stage an image: assess it, fetch it, check the bytes, then install it into the inactive slot.
    ///
    /// **Assessed before it is fetched, not after.** §12's third rule refuses an update that
    /// changes the Safety Kernel without a bench pass, and refusing it *after* the bytes are in the
    /// other slot would leave a robot one `mark-good` away from running something nobody certified.
    /// Staging is also where the refusal is cheapest: nothing has rebooted, the operator reads one
    /// report rather than discovering it at the safe-state barrier, and — now that this fetches —
    /// nobody has paid for a hundred megabytes to arrive at a refusal that needed no bytes at all.
    ///
    /// **And checked before it is installed, however it arrived.** Every gate above is bound to the
    /// manifest's digest, and until [`crate::bundle::verify_file`] existed nothing compared that
    /// digest against the bytes RAUC was handed. A hand-placed bundle — which was the only way to
    /// update a robot before the fetcher — went in unchecked. See the module header.
    fn stage_image(&mut self, version: &str, now_ms: u64) -> Outcome {
        let assessment = self.assess_image(version);
        if !assessment.admitted() {
            return Outcome::Failed {
                reason: refusal_text(&assessment),
            };
        }
        let layout = self.store.layout().clone();
        let digest = assessment.digest.clone().unwrap_or_default();

        // A bundle the manifest names by filename, put here by hand or by an earlier edition. Kept
        // ahead of the fetch so an offline install still works, and no longer trusted on sight.
        let bundle = match assessment.bundle_path.clone().filter(|path| path.is_file()) {
            Some(path) => path,
            None => {
                if digest.trim().is_empty() {
                    // `admit_bundle` refuses an undigested manifest, so this is unreachable through
                    // an admitted release. Checked anyway: the alternative is fetching bytes with
                    // nothing to check them against.
                    return Outcome::Failed {
                        reason: format!(
                            "release `{version}` is admitted and names no digest, so there is \
                             nothing to fetch and nothing to check what arrived against"
                        ),
                    };
                }
                match self.fetch_bundle(&layout, &digest, now_ms) {
                    Ok(path) => path,
                    Err(outcome) => return outcome,
                }
            }
        };

        // §12 binds every gate it applies to the digest. This is where the bytes are held to it.
        match crate::bundle::verify_file(&bundle, &digest, self.declared_size(version)) {
            Err(error) => return Outcome::Failed { reason: error },
            Ok(check) if !check.ok => {
                if check.discard {
                    let _ = std::fs::remove_file(&bundle);
                }
                return Outcome::Failed {
                    reason: format!(
                        "{} is not release `{version}`. {}",
                        bundle.display(),
                        check.detail
                    ),
                };
            }
            Ok(_) => {}
        }

        match self.updater.install(&bundle) {
            Ok(()) => Outcome::Applied,
            Err(error) => Outcome::Failed {
                reason: error.to_string(),
            },
        }
    }

    /// Move what can be moved of one image's bytes this pass (§12, §34.3).
    ///
    /// `Ok` only when the whole checked bundle is on the disk. Everything else is an outcome for the
    /// pass, and the three of them are deliberately different sentences: waiting for an unmetered
    /// link is not a failure, a fetch on an unpaired robot is not one either, and a digest mismatch
    /// very much is.
    fn fetch_bundle(
        &mut self,
        layout: &crate::layout::Layout,
        digest: &str,
        now_ms: u64,
    ) -> Result<std::path::PathBuf, Outcome> {
        let Some((endpoint, _device, credential)) = crate::identity::credential_for_sending(layout)
        else {
            return Err(Outcome::Waiting {
                reason: format!(
                    "this robot holds the manifest for these bytes and has nowhere to ask for \
                     them: it is not paired (§10), so there is no endpoint. `skynet pair` fixes \
                     it, or put the bundle at {} by hand.",
                    layout.bundle(digest).display()
                ),
            });
        };
        let version = self.desired.os_version.clone();
        let url = crate::bundle::url_for(&endpoint, &version);
        let table = crate::connectivity::Table::of(layout);

        let size = self.declared_size(&version);
        let jitter = self.jitter;
        let step = crate::bundle::advance(
            layout,
            self.fetcher.as_mut(),
            digest,
            size,
            &table,
            &url,
            Some(&credential),
            now_ms,
            jitter,
        );
        match step {
            crate::bundle::Step::Have { path, .. } => Ok(path),
            crate::bundle::Step::Waiting { detail } => Err(Outcome::Waiting { reason: detail }),
            crate::bundle::Step::Moved { detail, .. } => Err(Outcome::Waiting { reason: detail }),
            crate::bundle::Step::Failed { detail } => Err(Outcome::Failed { reason: detail }),
        }
    }

    /// Fetch what can be fetched of one package version this pass, and stage it when it is all here
    /// (§37.1, §91).
    ///
    /// Four outcomes, and they are deliberately different sentences. A robot that is not paired, or
    /// cannot reach its Hub, or is on a metered link, is **waiting**, and nothing is wrong with it:
    /// §34.1 makes being offline the ordinary state. A Hub that refused, a document about other
    /// bytes and bytes that are not the ones named are **failures**, and each holds the next
    /// attempt back on §34.4's curve. **Applied** is a blob in the store, checked twice.
    fn fetch_package(&mut self, id: &str, digest: &str, now_ms: u64) -> Outcome {
        let layout = self.store.layout().clone();
        let Some(wanted) = self
            .desired
            .packages
            .iter()
            .find(|package| package.id == id && package.digest == digest)
        else {
            return Outcome::Failed {
                reason: format!(
                    "the plan names {id} at {digest} and the desired state no longer carries it"
                ),
            };
        };
        let want = crate::fetch::Want {
            id: id.to_string(),
            kind: wanted.kind.as_str().to_string(),
            version: wanted.version.clone(),
            digest: digest.to_string(),
        };

        let Some((endpoint, device_id, credential)) =
            crate::identity::credential_for_sending(&layout)
        else {
            return Outcome::Waiting {
                reason: format!(
                    "{id} {} is not on this robot and there is nowhere to ask for it: the robot is \
                     not paired (§10), so there is no Hub. `skynet pair` fixes that, or copy the \
                     package over and stage it with `skynet install`.",
                    want.version
                ),
            };
        };

        // The backoff before the Hub is asked at all, so a Hub that refused is not asked again
        // every pass.
        if let Some((next_ms, error)) = crate::fetch::held_until(&layout, digest, now_ms) {
            return Outcome::Waiting {
                reason: format!(
                    "the last attempt at {id} failed and the next is held for another {} s \
                     (§34.4's curve). {}",
                    (next_ms - now_ms).div_ceil(1_000),
                    error.unwrap_or_default()
                ),
            };
        }

        let url = crate::fetch::url_for(&endpoint, &device_id, digest);
        let answer = match crate::connectivity::get_json(&url, Some(&credential)) {
            Ok(answer) => answer,
            // Unreachable is not refused (§34.1): the robot carries on and asks again next pass.
            Err(detail) => {
                return Outcome::Waiting {
                    reason: format!("{id}'s files wait for the Hub: {detail}"),
                }
            }
        };
        if !(200..300).contains(&answer.status) {
            let said = Json::parse(&answer.body)
                .ok()
                .and_then(|document| {
                    document
                        .get("error")
                        .and_then(Json::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_default();
            let reason = format!(
                "the Hub refused {id} {}: HTTP {}{}",
                want.version,
                answer.status,
                if said.is_empty() {
                    String::new()
                } else {
                    format!(" — {said}")
                }
            );
            crate::fetch::note_failure(&layout, digest, &reason, now_ms, self.jitter);
            return Outcome::Failed { reason };
        }
        let index = match crate::fetch::read_index(&answer.body, &want) {
            Ok(index) => index,
            Err(reason) => {
                crate::fetch::note_failure(&layout, digest, &reason, now_ms, self.jitter);
                return Outcome::Failed { reason };
            }
        };

        let table = crate::connectivity::Table::of(&layout);
        let jitter = self.jitter;
        match crate::fetch::advance(
            &layout,
            &self.store,
            self.fetcher.as_mut(),
            &index,
            &table,
            now_ms,
            jitter,
        ) {
            crate::fetch::Step::Staged { .. } => Outcome::Applied,
            crate::fetch::Step::Waiting { detail } | crate::fetch::Step::Moved { detail } => {
                Outcome::Waiting { reason: detail }
            }
            crate::fetch::Step::Failed { detail } => Outcome::Failed { reason: detail },
        }
    }

    /// The size the signed manifest declares for one release's bundle, when it declares one.
    ///
    /// `None` for every manifest a Hub serves today: §12's five-field manifest carries a digest and
    /// no size. It is read through one function rather than passed as `None` at three call sites so
    /// that the day the field exists, the checks that want it — *"a partial cannot be larger than
    /// what it is part of"*, and a progress line that is a fraction rather than a count — become
    /// true everywhere at once.
    pub fn declared_size(&self, version: &str) -> Option<u64> {
        crate::observe::read_json(&self.store.layout().release(version))?
            .get_present("size")
            .and_then(Json::as_f64)
            .filter(|value| *value >= 0.0)
            .map(|value| value as u64)
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
        let reading = self.read_power(now_ms);
        let health = self.health(actual, now_ms, Some(&reading), self.read_headroom());
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
    /// **Some of §54.2's branches are not `skynetd`'s to observe, and this method is written so that
    /// shows** — the same shape [`Daemon::refresh_operating_state`] takes, and for the same reason.
    /// §33.1 keeps the daemon off the actuator path, so the gripper, the base pose and the set of
    /// guards actually running live on the far side of a channel that does not exist. They are left
    /// absent rather than filled with plausible values, and §54.2's first rule then refuses the
    /// preconditions that named them, *with the path named*.
    ///
    /// What this process does have is the branches below, and every one of them is a file or a
    /// decision it already holds:
    ///
    /// | Branch | Source |
    /// |---|---|
    /// | `robot.state.operating` | §27's machine, as of the last observation |
    /// | `robot.state.joints`, `velocity`, `effort`, and the flags: `estop`, `braked`, `degraded`, `homing`, `fault_code` | the newest control cycle in §13's ring, as the last pass read it (F147) |
    /// | `robot.state.battery_percent` | §36's gauge, supplied by the caller |
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
    pub fn live_inputs(
        &self,
        wall_s: Option<f64>,
        power: Option<&crate::power::Reading>,
    ) -> LiveInputs {
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
        // The subject is this robot's own `robot_slug` (§87.1), which is the identifier `board()`
        // already uses and not a second one. A cell that names no robots answers `env.robot` with
        // the empty string whatever this machine calls itself, so a single-robot site is unchanged.
        let subject = body
            .known()
            .map(|known| known.descriptor.robot_slug.clone());
        let env = observe::read_json(&layout.active_profile())
            .and_then(|profile| profile.get_present("cell").cloned())
            .map(|document| cell_environment(&cell_from_json(&document), subject.as_deref()));

        // §45.5's unavailable set, which this robot computes on every pass for `GET /limits` and
        // carries to the Hub on every check-in (§7.6). It is derived from what the **body declares**
        // — a chassis with no IMU can never run `balance`, whatever a kernel is doing — so it is
        // answerable on this side of §33.1's line, and it is a different question from
        // `guards_active` below rather than the other half of one.
        let guards_unavailable = crate::limits::report(
            &crate::limits::read(
                layout,
                &body,
                crate::limits::ROBOT_TIER,
                &self.proximity_sighting,
            )
            .with_session(crate::api::live_session(layout)),
        )
        .unavailable
        .into_iter()
        .map(|entry| entry.guard)
        .collect();

        LiveInputs {
            operating_state: Some(self.operating_state),
            // §36's gauge, reaching §54.2 at last. The robot has read its own state of charge on
            // every pass since §36 was built — `GET /power` prints it, §33.6's health column
            // renders it, the check-in carries it — and the one place it did not arrive was the
            // context a precondition reads. So `robot.state.battery_percent > 20`, which is the
            // example §54.2's own documentation uses, refused every skill on a robot that knew the
            // answer and named the path as unresolvable.
            //
            // Supplied rather than read here, for the reason `Pass` gives about the same number: a
            // battery figure taken in a different turn of the loop than the decision it informs is
            // the one thing that struct exists not to be. `None` where the caller had no reading,
            // which is absent rather than zero — §54.2's first rule, and the whole reason this
            // field is an `Option`.
            //
            // A **fraction** on the reading and a **percentage** here, because that is what the two
            // consumers are: §33.6 renders a bar and a precondition is written by a person. The
            // conversion is one multiplication and it belongs at the boundary rather than in either
            // consumer's head.
            battery_percent: power
                .and_then(|reading| reading.charge_fraction)
                .map(|fraction| fraction * 100.0),
            // Not `Some(vec![])`. §33.1: there is no kernel channel, so nobody has asked which
            // guards are running, and an empty list would be this daemon claiming none are.
            //
            // `guards_unavailable` beside it is filled, and the asymmetry is the point rather than
            // an oversight. *Running* is a fact about a kernel and this process has no channel to
            // one. *Unavailable* is a fact about a body: §45.5 makes a guard unavailable when its
            // input is missing, and whether this chassis declares an IMU is written in a descriptor
            // on this disk. One is unobservable from here and the other is already computed here
            // twice a pass.
            guards_active: None,
            // §24's flags and the joint readings beside them, off the newest cycle the last pass
            // read (F147). They are a driver's facts, and this daemon does not ask a driver for
            // them: the control loop records what the driver returned on every cycle, and this is
            // that record. A comment here used to leave both absent *"until §26.2's cycle channels
            // reach this side"*, when four of §27's conditions were already being read off them.
            //
            // Absent, not filled, whenever the newest cycle is not a measurement — no cycle in the
            // interval, a failed read, a ring with no action channels — and the joints are absent
            // too when the record and the descriptor disagree about how many there are. §54.2 then
            // refuses a precondition that names one, with the path named.
            joints: body
                .known()
                .map(|known| {
                    let names: Vec<&str> = known
                        .descriptor
                        .joints
                        .iter()
                        .map(|joint| joint.name.as_str())
                        .collect();
                    self.body_reading.joints(&names)
                })
                .unwrap_or_default(),
            drive: self.body_reading.drive(),
            guards_unavailable,
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
        interventions_since: Option<u64>,
    ) -> Vec<crate::skill::Report> {
        // One reading, used twice. §36.1's refusal and §54.2's `battery_percent` are two
        // consumers of the same measurement, and reading the gauge separately for each would let a
        // skill be refused a lease for a battery level its own precondition had just cleared.
        let reading = self.read_power(now_ms);
        let inputs = self.live_inputs(wall_s, Some(&reading));
        let running = crate::skill::running_set(actual);
        let grants = actual.grants.clone();
        let power_withheld = self.power_refusal_from(&reading);

        let mut runtime = core::mem::take(&mut self.skills);
        let reports = {
            let mut cycle = crate::skill::Cycle {
                now_ms,
                inputs: &inputs,
                robot: self.operating_state,
                // §13's ring, counted over the interval since the last pass by the caller —
                // which holds the recorder, because the ring has exactly one writer and this
                // struct is not it. `None` is a robot with no recording to count, and it keeps
                // `interventions` out of the episode rather than in it as a zero.
                interventions_since,
                // §35.1's correlation clock, for the one field that needs it: the episode's
                // `recorded_at`. Zero means the platform could not answer, which is not a time.
                wall_ms: wall_s
                    .filter(|seconds| *seconds > 0.0)
                    .map(|seconds| (seconds * 1_000.0) as u64),
                running: &running,
                leases: &mut self.leases,
                grants: &grants,
                power_withheld,
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
    pub fn stop_skills(&mut self, now_ms: u64, wall_ms: Option<u64>) -> Vec<crate::skill::Report> {
        let mut runtime = core::mem::take(&mut self.skills);
        let reports = runtime.stop_all(now_ms, wall_ms);
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
        let decision = record.on_exit(now_ms, jitter);
        // §74's `package_restarts`, which had no source and needed none it did not already have:
        // the supervisor is what decides a restart, and this is where it decides one.
        //
        // Counted at the **decision** rather than at the start it leads to. The two are one pass
        // apart, and a decision whose start never happened is a robot in worse trouble rather than
        // better — a counter that waited for the start would under-report exactly the robot a fleet
        // page is looking for. `Failed` and `AwaitOperator` are not restarts and are not counted;
        // they are a package that stopped coming back, which the readiness column already says.
        if matches!(decision, RestartDecision::RestartAt(_)) {
            self.counting.restarts += 1;
        }
        // Whatever it held, it is not holding now.
        self.leases.release_all(id);
        let _ = self.write_leases();
    }

    /// One §36 reading, against the descriptor as it is on disk right now (§36).
    ///
    /// The descriptor is loaded per call rather than held on the daemon, for the reason `actual` is
    /// observed rather than remembered (§33.3): a profile cached at start-up would survive a
    /// descriptor somebody replaced on a running robot, and the thresholds in it are what decide
    /// when this robot stops.
    pub fn read_power(&mut self, now_ms: u64) -> crate::power::Reading {
        let layout = self.store.layout().clone();
        let body = crate::embodiment::load(&layout);
        // §36.4's input, off §27's `rhal` sighting rather than off the RHAL. The sighting is the
        // pass's own reading of §13's ring, so the bus reading and the operating state cannot
        // disagree about one instant — which is the rule `Recorder::newest_cycle` states.
        let bus = crate::power::BusReading::of(&self.rhal_sighting);
        let (_, reading) = crate::power::observe(&mut self.power, &layout, &body, bus, now_ms);
        reading
    }

    /// §33.6's `storage_headroom`: the fraction of the black box's volume still free.
    ///
    /// Beside [`Runtime::read_power`] and shaped like it — the I/O is here, and the rollup is
    /// handed the number. Which volume is the question this answers by choosing one: the black
    /// box's, because that is the disk §28 reserves and the one §80's reclaim would free.
    pub fn read_headroom(&self) -> Option<f64> {
        crate::retention::headroom_of(self.store.layout())
    }

    /// §36.1's `low` row as a sentence, or `None` when new task leases may be granted.
    ///
    /// The sentence names *which* of §36's two reasons is in force, because they send an operator
    /// to different places: a body below its reserve wants a charger, and a body whose gauge
    /// nothing can read wants a look at the machine. Both withhold, and a shared sentence would
    /// send half the operators to the wrong one (§64).
    ///
    /// A robot with no power subsystem wired up is `unmetered`, grants leases, and returns `None`
    /// here — so this changes nothing on a body that declares no pack, which is what every test in
    /// this suite and every development host is.
    ///
    /// **Takes the reading rather than the gauge, and that is the whole interface.** A pass has two
    /// consumers of one measurement — this and §54.2's `battery_percent` — and reading the gauge
    /// twice would let a skill be refused a lease for a battery level its own precondition had just
    /// cleared. It is the rule `Pass` states about the health figure, applied to the two places that
    /// would otherwise disagree.
    ///
    /// A `power_refusal(now_ms)` that read the gauge itself sat here until 2026-09-16, reached by
    /// nothing. It was deleted rather than wired, because the convenience it offered was exactly the
    /// second read the paragraph above exists to prevent — see `DRY-COVERAGE-PLAN.md` *Decisions* 11.
    /// A caller that genuinely holds no reading calls [`Runtime::read_power`] first, on its own line,
    /// where the second measurement is visible to whoever reads the pass.
    pub fn power_refusal_from(&self, reading: &crate::power::Reading) -> Option<String> {
        let layout = self.store.layout().clone();
        let body = crate::embodiment::load(&layout);
        let profile = crate::power::profile_of(&body);
        let assessment = skynet_contracts::power::assess(
            &profile,
            &reading.observation,
            Some(body.known().map(|known| known.descriptor.base.as_str())),
        );
        if assessment.grants_new_leases() {
            return None;
        }

        let mut sentence = format!(
            "§36.1 withholds new task leases: this robot's power state is `{}` ({})",
            assessment.state.as_str(),
            assessment.reason.as_str()
        );
        // The faults are what make the two cases distinguishable. A robot at `low` has none and the
        // state is the whole story; a robot reporting `unmetered` has one, and it is the sentence
        // that says whether to fetch a charger or a screwdriver.
        for fault in &assessment.faults {
            if fault.code.withholds_leases() {
                sentence.push_str(". ");
                sentence.push_str(&fault.detail);
            }
        }
        Some(sentence)
    }

    /// Aggregate health (§33.6).
    ///
    /// Readiness is [`Readiness::Unknown`] for every running package, and that is the correct
    /// report rather than a gap in this function: readiness means "producing commands at its
    /// declared rate with an advancing sequence", the sequence comes from the ring the policy
    /// runtime posts into, and the policy runtime is not written. An `Unknown` on the console reads
    /// as "running, nobody has measured whether it is producing", which is true. A `Ready` would be
    /// the green tick §33.6 exists to prevent.
    ///
    /// `power` is §36's reading, taken by the caller. **Passed in rather than taken here** for two
    /// reasons that point the same way: this method is `&self` and the throttle window is the one
    /// thing §36 remembers, and a health rollup that opened `/sys` would be doing I/O behind a name
    /// that promises an aggregate. `None` is a caller that did not read one, which reports the two
    /// fields absent — as they were on every check-in until §36 was written.
    ///
    /// `headroom` is §33.6's free-disk fraction, taken by the caller for exactly the reason `power`
    /// is: measuring it is a syscall, and a rollup that made one behind a name promising an
    /// aggregate is the shape this method was written against. `None` is a caller that did not
    /// measure, or a disk `skynet-statfs` refused to believe — both report the field absent, which
    /// is what every check-in did before this crate existed.
    pub fn health(
        &self,
        actual: &Actual,
        now_ms: u64,
        power: Option<&crate::power::Reading>,
        headroom: Option<f64>,
    ) -> Health {
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
            storage_headroom: headroom,
            guard_trip_rate: None,
            // §36's two, and this is their only source. `GET /health` and the check-in both read
            // this struct, so a robot cannot report one battery figure on its console and another
            // to its Hub — which is what filling them in two places would eventually produce.
            battery: power.and_then(|reading| reading.charge_fraction),
            temperature_c: power.and_then(|reading| reading.observation.temperature_c),
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
    // §90.2's two settings, when the profile makes them — and only from here, so a bound or an
    // `automatic` resume in force is one somebody signed. `resume` is always written with the block:
    // absent in the document is `acknowledge`, and the reader should not have to know that.
    if let Some(remote) = &profile.remote {
        let mut block = Json::obj();
        block
            .set_opt("max_chunk_age_ms", remote.max_chunk_age_ms.map(Json::Num))
            .set(
                "resume",
                Json::from(if remote.resume_automatic {
                    "automatic"
                } else {
                    "acknowledge"
                }),
            );
        document.set("remote", block);
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
