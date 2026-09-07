//! The operating state machine (§27, extended by §48.1).
//!
//! The Rust edition of `reference/typescript/opstate.ts`. Read that file for the design; this one
//! records the port.
//!
//! Six sections referred to this machine and none of them declared it. `post.rs` returns "the state
//! §48.1's machine should be in after this" as a string. §49.1's warning tone took two strings, with
//! a comment saying why. §33.4's barrier takes a boolean a caller hands it. §54.4's skill lifecycle
//! waits on it. Two sessions recorded that as a finding; this closes it.
//!
//! # Three things the port had to decide
//!
//! **The defaults are in the reader, not in the caller.** [`Conditions`] is a struct of plain
//! fields here and an object of optional ones in TypeScript, so "absent" has to be spelled. Every
//! field defaults to the least capable reading — no kernel, no RHAL, no POST, homing pending — and
//! [`Conditions::default`] is therefore a robot that may not move. That is the same value the
//! reference produces for `{}`, and it is the only default that is safe to get wrong.
//!
//! **`stop` is an `Option<StopCause>`** rather than a bool and a cause, for the reason the whole
//! repository keeps arriving at: two fields that must agree are two fields that eventually do not.
//!
//! **`cleared_by` is a `&str`, empty for absent.** The reference treats an empty clearance as no
//! clearance — a caller with nobody to name should not have one — and matching that here is what
//! keeps `""` from being a way to unstick `SAFE` in one edition and not the other.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

/// §27's states, ordered least capable first.
///
/// The order is the one [`evaluate`]'s rules descend through. Reproduced into
/// `corpus/vocabulary/closed-sets.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OperatingState {
    /// Drives disabled. Requires an explicit human action to leave.
    Safe,
    /// A controlled ramp per the stop category.
    Stopping,
    /// Booting; the self-test has not established that this robot may run.
    Post,
    /// Joint positions are unknown, so every pose-dependent guard is unavailable (§48.1).
    Unhomed,
    /// Setpoints held, brakes as configured. The safe resting state.
    Hold,
    /// Runs, reports, and refuses any package whose grants depend on the failed guard.
    Degraded,
    /// Normal operation.
    Running,
}

impl OperatingState {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            OperatingState::Safe => "safe",
            OperatingState::Stopping => "stopping",
            OperatingState::Post => "post",
            OperatingState::Unhomed => "unhomed",
            OperatingState::Hold => "hold",
            OperatingState::Degraded => "degraded",
            OperatingState::Running => "running",
        }
    }

    /// Read a state back, for a report that crossed a wire.
    ///
    /// Exact rather than case-insensitive. The old string-typed `warning_required` compared
    /// case-insensitively and that was a kindness to callers rather than a rule anything declared;
    /// a state spelled `Running` is a caller with a bug, and answering it is how the bug survives.
    pub fn parse(text: &str) -> Option<Self> {
        OPERATING_STATES
            .iter()
            .copied()
            .find(|state| state.as_str() == text)
    }

    /// Whether §33.4's barrier may open at this state.
    ///
    /// > A safe state: nothing is held, nothing is mid-task.
    ///
    /// [`OperatingState::Stopping`] is not one, and it is the interesting entry: a robot mid-ramp
    /// most looks like it is about to be safe and most certainly is not.
    pub fn is_safe_state(self) -> bool {
        matches!(
            self,
            OperatingState::Post
                | OperatingState::Unhomed
                | OperatingState::Hold
                | OperatingState::Safe
        )
    }

    /// Whether a policy is commanding the body in this state.
    ///
    /// [`OperatingState::Running`] and [`OperatingState::Degraded`]. §27's `DEGRADED` row says the
    /// robot "**runs**, reports, and refuses any package whose grants depend on the failed guard" —
    /// it is the impaired running state rather than a third resting one, which is why the degraded
    /// rule sits below the policy rules in [`evaluate`].
    pub fn commands_autonomously(self) -> bool {
        matches!(self, OperatingState::Running | OperatingState::Degraded)
    }

    /// Whether leaving this state requires an explicit human action (§27). Exactly one does.
    pub fn is_sticky(self) -> bool {
        self == OperatingState::Safe
    }
}

/// Every state, least capable first.
pub const OPERATING_STATES: [OperatingState; 7] = [
    OperatingState::Safe,
    OperatingState::Stopping,
    OperatingState::Post,
    OperatingState::Unhomed,
    OperatingState::Hold,
    OperatingState::Degraded,
    OperatingState::Running,
];

/// What §48.2's self-test concluded, as the one input §27's machine takes from it.
///
/// Three answers rather than §48.4's four outcomes, because the four are per **check** and this is
/// the report. `Incomplete` is not a softening of `Failed`: §48.4 is explicit that a robot whose
/// checks could not run "does not need a human. It needs the rest of the operating system."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PostVerdict {
    /// Every check passed.
    Passed,
    /// Something failed. §48.2's `SAFE`.
    Failed,
    /// Neither: a check established nothing, or the self-test has not run.
    #[default]
    Incomplete,
}

impl PostVerdict {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            PostVerdict::Passed => "passed",
            PostVerdict::Failed => "failed",
            PostVerdict::Incomplete => "incomplete",
        }
    }
}

/// Every POST verdict.
pub const POST_VERDICTS: [PostVerdict; 3] = [
    PostVerdict::Passed,
    PostVerdict::Failed,
    PostVerdict::Incomplete,
];

/// Whether this body knows where its joints are (§48.3).
///
/// `NotRequired` is a declaration, not an absence: "A body that needs no homing declares so and the
/// state is skipped." The default is [`HomingState::Pending`], because a body that has said nothing
/// is a body whose joint positions are unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HomingState {
    /// The body declares it needs none, and the state is skipped.
    NotRequired,
    /// Joint positions are unknown.
    #[default]
    Pending,
    /// Homed, and the joint-zero calibration record written or confirmed (§45.2).
    Complete,
}

impl HomingState {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            HomingState::NotRequired => "not_required",
            HomingState::Pending => "pending",
            HomingState::Complete => "complete",
        }
    }
}

/// Every homing state.
pub const HOMING_STATES: [HomingState; 3] = [
    HomingState::NotRequired,
    HomingState::Pending,
    HomingState::Complete,
];

/// What the RHAL is reporting, as far as the kernel can tell.
///
/// `Lost` is separate from `Degraded` because §27 puts them in different rows and the difference is
/// the whole of `SAFE`: a degraded RHAL is a body that is still there and still answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RhalState {
    /// Present and nominal.
    Present,
    /// The RHAL reports `RHAL_DEGRADED`.
    Degraded,
    /// The kernel lost it.
    #[default]
    Lost,
}

impl RhalState {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            RhalState::Present => "present",
            RhalState::Degraded => "degraded",
            RhalState::Lost => "lost",
        }
    }
}

/// Every RHAL state.
pub const RHAL_STATES: [RhalState; 3] = [RhalState::Present, RhalState::Degraded, RhalState::Lost];

/// §27's four causes of a controlled stop, and no fifth.
///
/// Closed on purpose. A fifth cause is a fifth thing that stops a robot, and it should be added by
/// somebody who found this enumeration rather than by somebody who found a boolean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopCause {
    /// §46's chain fired.
    Watchdog,
    /// Something came inside the stop distance (§7.1).
    Proximity,
    /// The balance guard.
    Balance,
    /// A person asked.
    Operator,
    /// The hardware emergency-stop chain is open (§24's `SKR_FLAG_ESTOP`).
    ///
    /// The odd one out, and deliberately not folded into [`StopCause::Operator`]. The first four
    /// are the **kernel's own decisions** — it evaluated a guard, or a person asked, and it
    /// commanded a stop. This one is the body reporting that the chain is open, which happened
    /// whether or not any software agreed.
    ///
    /// §24 gives it one bit, so a robot can say the chain is open and cannot say what opened it.
    /// `operator` means *a person asked*, and an estop chain is also opened by a light curtain, a
    /// door interlock and a safety relay noticing itself fail. Reporting those as a person having
    /// asked would put a name on an event that has none.
    Estop,
}

impl StopCause {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            StopCause::Watchdog => "watchdog",
            StopCause::Proximity => "proximity",
            StopCause::Balance => "balance",
            StopCause::Operator => "operator",
            StopCause::Estop => "estop",
        }
    }

    /// The reason a stop in progress reports.
    ///
    /// A `match` rather than a formatted string, so an added cause is a compile error here rather
    /// than a reason nothing declares.
    fn stopping_reason(self) -> StateReason {
        match self {
            StopCause::Watchdog => StateReason::StoppingWatchdog,
            StopCause::Proximity => StateReason::StoppingProximity,
            StopCause::Balance => StateReason::StoppingBalance,
            StopCause::Operator => StateReason::StoppingOperator,
            StopCause::Estop => StateReason::StoppingEstop,
        }
    }
}

/// Every stop cause.
pub const STOP_CAUSES: [StopCause; 5] = [
    StopCause::Watchdog,
    StopCause::Proximity,
    StopCause::Balance,
    StopCause::Operator,
    StopCause::Estop,
];

/// Whether there is a policy the robot could be running (§27's `RUNNING` row).
///
/// `Held` is a policy that exists and is not ready: staged, refused a lease (§25.2), or waiting on
/// a transport plan. Distinct from `None` because the two send an operator to different places.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PolicyState {
    /// Nothing to run.
    #[default]
    None,
    /// Something to run that is not ready to.
    Held,
    /// Lease granted and plan resolved.
    Ready,
}

impl PolicyState {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            PolicyState::None => "none",
            PolicyState::Held => "held",
            PolicyState::Ready => "ready",
        }
    }
}

/// Every policy state.
pub const POLICY_STATES: [PolicyState; 3] =
    [PolicyState::None, PolicyState::Held, PolicyState::Ready];

/// Whether the Safety Kernel is live.
///
/// §27: "a robot with a policy and no kernel must not exist, so the runtime refuses to start if the
/// kernel is not live".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KernelState {
    /// Running, and it started before any policy.
    Live,
    /// Not. The default, because nobody has said otherwise.
    #[default]
    Down,
}

impl KernelState {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            KernelState::Live => "live",
            KernelState::Down => "down",
        }
    }
}

/// Every kernel state.
pub const KERNEL_STATES: [KernelState; 2] = [KernelState::Live, KernelState::Down];

/// Everything the machine looks at, and nothing it remembers.
///
/// Every field is observed rather than inferred, which is the discipline §33.3 imposes on the
/// reconciler's `actual`. Every default is the least capable reading: a caller who has not observed
/// something has not established it, and a machine that read "nobody told me about the kernel" as
/// "the kernel is live" would be the single most dangerous line in this repository.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Conditions {
    /// §48.2's report, rolled up.
    pub post: PostVerdict,
    /// §48.3.
    pub homing: HomingState,
    /// §27.
    pub kernel: KernelState,
    /// §27.
    pub rhal: RhalState,
    /// §27's `STOPPING` row. `None` means no stop is commanded.
    pub stop: Option<StopCause>,
    /// Whether the ramp has finished. Read only when a stop is commanded, so a caller that left it
    /// true between stops cannot pin the machine in `SAFE`.
    ///
    /// **For [`StopCause::Estop`] it is not observable on any edition that exists**, and that is a
    /// property of the evidence rather than an omission. §24 gives the hardware chain one bit,
    /// which says the chain is open and says nothing about whether the body has come to rest — so a
    /// supplier setting this true for a hardware stop would be claiming to have watched a robot
    /// stop moving. Left false it derives `stopping`, which can hold a robot and can never release
    /// one.
    pub stop_settled: bool,
    /// §27's `HOLD` row: "a guard `hold`".
    pub guard_hold: bool,
    /// §27's `DEGRADED` row: "a guard is unavailable". §45.5 is the usual source.
    pub guards_unavailable: bool,
    /// §27's `RUNNING` row: "lease granted, plan resolved".
    pub policy: PolicyState,
    /// The explicit human action §27 requires to leave `SAFE`, by name.
    ///
    /// Recorded rather than counted: §13 takes the transition, and "somebody cleared the safety
    /// stop" is worth exactly as much as the name of the somebody. Empty is not a clearance.
    pub cleared_by: String,
}

impl Conditions {
    /// Whether a clearance has been given. An empty name is not one.
    fn cleared(&self) -> bool {
        !self.cleared_by.is_empty()
    }
}

/// Why the machine is where it is, in the vocabulary a console renders.
///
/// One reason per rule, so the ordering is legible from the output: a robot reporting `no_policy`
/// is one for which every rule above it did not fire. `Hold` for a guard hold and `Hold` for no
/// policy are the same state and completely different situations, and an operator staring at a
/// robot that will not move needs the second fact rather than the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateReason {
    /// The kernel lost the RHAL.
    RhalLost,
    /// `SAFE` is sticky and nobody has cleared it.
    SafeSticky,
    /// §48.2's failed self-test.
    PostFailed,
    /// The ramp finished.
    StopComplete,
    /// §46's chain fired.
    StoppingWatchdog,
    /// Something came inside the stop distance.
    StoppingProximity,
    /// The balance guard.
    StoppingBalance,
    /// A person asked.
    StoppingOperator,
    /// The hardware emergency-stop chain is open, and what opened it is not knowable (§24).
    StoppingEstop,
    /// §48.4: the checks could not run, so nothing was established.
    PostIncomplete,
    /// §48.1: joint positions are unknown.
    HomingPending,
    /// The Safety Kernel is not live.
    KernelDown,
    /// A guard asked for a hold.
    GuardHold,
    /// There is nothing to run.
    NoPolicy,
    /// There is something to run and it has no lease or no plan.
    PolicyNotReady,
    /// A guard is unavailable (§45.5's usual case).
    GuardsUnavailable,
    /// The RHAL reports `RHAL_DEGRADED`.
    RhalDegraded,
    /// Nothing is wrong.
    Nominal,
}

impl StateReason {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            StateReason::RhalLost => "rhal_lost",
            StateReason::SafeSticky => "safe_sticky",
            StateReason::PostFailed => "post_failed",
            StateReason::StopComplete => "stop_complete",
            StateReason::StoppingWatchdog => "stopping_watchdog",
            StateReason::StoppingProximity => "stopping_proximity",
            StateReason::StoppingBalance => "stopping_balance",
            StateReason::StoppingOperator => "stopping_operator",
            StateReason::StoppingEstop => "stopping_estop",
            StateReason::PostIncomplete => "post_incomplete",
            StateReason::HomingPending => "homing_pending",
            StateReason::KernelDown => "kernel_down",
            StateReason::GuardHold => "guard_hold",
            StateReason::NoPolicy => "no_policy",
            StateReason::PolicyNotReady => "policy_not_ready",
            StateReason::GuardsUnavailable => "guards_unavailable",
            StateReason::RhalDegraded => "rhal_degraded",
            StateReason::Nominal => "nominal",
        }
    }
}

/// Every reason, in rule order.
pub const STATE_REASONS: [StateReason; 18] = [
    StateReason::RhalLost,
    StateReason::SafeSticky,
    StateReason::PostFailed,
    StateReason::StopComplete,
    StateReason::StoppingWatchdog,
    StateReason::StoppingProximity,
    StateReason::StoppingBalance,
    StateReason::StoppingOperator,
    StateReason::StoppingEstop,
    StateReason::PostIncomplete,
    StateReason::HomingPending,
    StateReason::KernelDown,
    StateReason::GuardHold,
    StateReason::NoPolicy,
    StateReason::PolicyNotReady,
    StateReason::GuardsUnavailable,
    StateReason::RhalDegraded,
    StateReason::Nominal,
];

/// One evaluation of the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    /// Where it was.
    pub from: OperatingState,
    /// Where it is.
    pub to: OperatingState,
    /// Why.
    pub reason: StateReason,
    /// Whether the state changed. Named rather than left to a comparison at every call site.
    pub changed: bool,
    /// §49.1's tone. See [`warning_required`].
    pub warning: bool,
}

/// Whether a warning tone must sound on this transition (§49.1).
///
/// > Standards for robots operating around people expect an audible indication before autonomous
/// > motion. That makes the warning tone a **system function, not an application feature**:
/// > `skynetd` owns it, it sounds on transition into `RUNNING` and on resumption after a stop, and
/// > **a package cannot suppress it.**
///
/// This lived in `admission.rs` beside the reserved-token half of §49.1, took two strings, and
/// compared them to the literal `"running"`. It was under-inclusive, and the comment explaining why
/// it took strings is what makes that findable: `hold → degraded` is a policy beginning to command
/// a body and is not a transition into `running`, so the tone did not sound. A robot whose guard set
/// is imperfect — every robot with a lapsed calibration, §45.5's ordinary case — would have begun
/// autonomous motion silently.
///
/// Phrased over [`OperatingState::commands_autonomously`] the two spellings cannot come apart, and
/// the rule matches the sentence rather than one example from it. "Resumption after a stop" falls
/// out: `stopping` and `safe` command nothing, so leaving either for a commanding state sounds.
pub fn warning_required(from: OperatingState, to: OperatingState) -> bool {
    to.commands_autonomously() && !from.commands_autonomously()
}

/// The state and reason the conditions imply, before stickiness.
fn derive(conditions: &Conditions) -> (OperatingState, StateReason) {
    // 1. The kernel lost the body. §27 puts this in the `SAFE` row beside a completed stop, and it
    //    is first because it is the condition under which nothing else can be known: a robot whose
    //    RHAL is gone has no observed joint state, no estop reading and no guard that can evaluate.
    if conditions.rhal == RhalState::Lost {
        return (OperatingState::Safe, StateReason::RhalLost);
    }

    // 2. A failed self-test. §48.2: `SAFE`, not `DEGRADED`, "because a robot that failed its own
    //    self-test should not be talked into running by a reconcile loop".
    if conditions.post == PostVerdict::Failed {
        return (OperatingState::Safe, StateReason::PostFailed);
    }

    // 3. A stop, complete or in progress. Above POST because an operator stop is honoured whatever
    //    the robot is doing, including booting.
    if let Some(cause) = conditions.stop {
        return if conditions.stop_settled {
            (OperatingState::Safe, StateReason::StopComplete)
        } else {
            (OperatingState::Stopping, cause.stopping_reason())
        };
    }

    // 4. The self-test has not established that this robot may run. Not a fault: §48.4's
    //    `post_incomplete`, which is a robot waiting on the rest of the operating system.
    if conditions.post != PostVerdict::Passed {
        return (OperatingState::Post, StateReason::PostIncomplete);
    }

    // 5. §48.1. Joint positions are unknown, forward kinematics is meaningless, and every
    //    pose-dependent guard is unavailable — so a policy must not run, and homing is the only
    //    motion permitted (§48.3).
    if conditions.homing == HomingState::Pending {
        return (OperatingState::Unhomed, StateReason::HomingPending);
    }

    // 6. §27: "a robot with a policy and no kernel must not exist, so the runtime refuses to start
    //    if the kernel is not live". Refusing to start is `HOLD` — the safe resting state — rather
    //    than `SAFE`: nothing has gone wrong with the robot, and a sticky state needing a human
    //    would be the wrong answer for a kernel that is merely still starting.
    if conditions.kernel != KernelState::Live {
        return (OperatingState::Hold, StateReason::KernelDown);
    }

    // 7. §27's `HOLD` row, first half. A guard asking for a hold outranks a policy that would like
    //    to run, which is the whole point of a guard.
    if conditions.guard_hold {
        return (OperatingState::Hold, StateReason::GuardHold);
    }

    // 8. §27's `HOLD` row, second half: "no policy". Split in two, because "there is nothing to
    //    run" and "there is something and it has not got a lease" are different sentences and the
    //    second is the one with a remedy — §25.2's arbitration is where an operator goes to find
    //    out who else is holding the arm.
    match conditions.policy {
        PolicyState::None => return (OperatingState::Hold, StateReason::NoPolicy),
        PolicyState::Held => return (OperatingState::Hold, StateReason::PolicyNotReady),
        PolicyState::Ready => {}
    }

    // 9. §27's `DEGRADED` row, and it is below the policy rules deliberately — see
    //    [`OperatingState::commands_autonomously`]. Reaching here means a policy is ready to
    //    command, so `degraded` is the impaired running state rather than a resting one.
    if conditions.guards_unavailable {
        return (OperatingState::Degraded, StateReason::GuardsUnavailable);
    }
    if conditions.rhal == RhalState::Degraded {
        return (OperatingState::Degraded, StateReason::RhalDegraded);
    }

    (OperatingState::Running, StateReason::Nominal)
}

/// The state this robot is in, given where it was and what is true now.
///
/// Pure, clockless and total. Every combination of conditions produces a state, including
/// combinations nobody anticipated, which is the property §33.3 asks of anything that converges.
pub fn evaluate(previous: OperatingState, conditions: &Conditions) -> Transition {
    let (derived_state, derived_reason) = derive(conditions);

    // §27's stickiness, applied after the derivation rather than instead of it. A robot in `SAFE`
    // whose conditions would put it back in `SAFE` reports the *condition's* reason rather than
    // `safe_sticky`, because "the estop is still pressed" is what an operator needs and "you have
    // not cleared it yet" is not.
    let stuck = previous == OperatingState::Safe
        && !conditions.cleared()
        && derived_state != OperatingState::Safe;

    let to = if stuck {
        OperatingState::Safe
    } else {
        derived_state
    };
    let reason = if stuck {
        StateReason::SafeSticky
    } else {
        derived_reason
    };

    Transition {
        from: previous,
        to,
        reason,
        changed: to != previous,
        warning: warning_required(previous, to),
    }
}

/// A transition, for `GET /state`, the console and the black box.
pub fn transition_to_json(transition: &Transition) -> Json {
    let mut out = Json::obj();
    out.set("from", Json::Str(transition.from.as_str().to_string()));
    out.set("to", Json::Str(transition.to.as_str().to_string()));
    out.set("reason", Json::Str(transition.reason.as_str().to_string()));
    out.set("changed", Json::Bool(transition.changed));
    out.set("warning", Json::Bool(transition.warning));
    out
}

/// Read conditions out of a document, for the corpus and for a report that crossed a wire.
///
/// Every unreadable field falls back to its default, which is the least capable reading. A caller
/// that sent `"kernel": "LIVE"` gets a held robot, which is the direction to fail in.
pub fn conditions_from_json(input: &Json) -> Conditions {
    fn word<'a>(input: &'a Json, key: &str) -> Option<&'a str> {
        input.get(key).and_then(Json::as_str)
    }
    fn flag(input: &Json, key: &str) -> bool {
        matches!(input.get(key), Some(Json::Bool(true)))
    }

    let post = match word(input, "post") {
        Some("passed") => PostVerdict::Passed,
        Some("failed") => PostVerdict::Failed,
        _ => PostVerdict::Incomplete,
    };
    let homing = match word(input, "homing") {
        Some("not_required") => HomingState::NotRequired,
        Some("complete") => HomingState::Complete,
        _ => HomingState::Pending,
    };
    let kernel = match word(input, "kernel") {
        Some("live") => KernelState::Live,
        _ => KernelState::Down,
    };
    let rhal = match word(input, "rhal") {
        Some("present") => RhalState::Present,
        Some("degraded") => RhalState::Degraded,
        _ => RhalState::Lost,
    };
    let stop = word(input, "stop").and_then(|cause| {
        STOP_CAUSES
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == cause)
    });
    let policy = match word(input, "policy") {
        Some("ready") => PolicyState::Ready,
        Some("held") => PolicyState::Held,
        _ => PolicyState::None,
    };

    Conditions {
        post,
        homing,
        kernel,
        rhal,
        stop,
        stop_settled: flag(input, "stop_settled"),
        guard_hold: flag(input, "guard_hold"),
        guards_unavailable: flag(input, "guards_unavailable"),
        policy,
        cleared_by: word(input, "cleared_by").unwrap_or("").to_string(),
    }
}

/// Every state's three properties, as data.
///
/// The corpus pins this beside [`evaluate`] because the three are what other sections read off the
/// machine — §33.4's barrier, §49.1's tone, §27's stickiness — and an edition could agree about
/// every transition while disagreeing about what a state *means*, which would be a divergence no
/// evaluation case could see.
pub fn state_properties_to_json() -> Json {
    let mut out = Json::arr();
    for state in OPERATING_STATES {
        let mut row = Json::obj();
        row.set("state", Json::Str(state.as_str().to_string()));
        row.set("safe_state", Json::Bool(state.is_safe_state()));
        row.set("commands", Json::Bool(state.commands_autonomously()));
        row.set("sticky", Json::Bool(state.is_sticky()));
        out.push(row);
    }
    out
}

/// Every ordered pair of states and whether it sounds the tone (§49.1).
///
/// Forty-nine rows, which is small enough to freeze in full and is the only way to be sure both
/// editions agree about a predicate whose failure mode is silence.
pub fn warning_matrix_to_json() -> Json {
    let mut out = Json::arr();
    for from in OPERATING_STATES {
        for to in OPERATING_STATES {
            let mut row = Json::obj();
            row.set("from", Json::Str(from.as_str().to_string()));
            row.set("to", Json::Str(to.as_str().to_string()));
            row.set("warning", Json::Bool(warning_required(from, to)));
            out.push(row);
        }
    }
    out
}

/// The states, as strings, for a vocabulary case.
pub fn state_names() -> Vec<&'static str> {
    OPERATING_STATES
        .iter()
        .map(|state| state.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// A robot with nothing wrong with it: booted, homed, kernel live, body present, policy ready.
    ///
    /// Every test below starts here and breaks exactly one thing, which is the only way to be sure
    /// a rule fired for its own reason rather than because three conditions happened to agree.
    fn nominal() -> Conditions {
        Conditions {
            post: PostVerdict::Passed,
            homing: HomingState::Complete,
            kernel: KernelState::Live,
            rhal: RhalState::Present,
            policy: PolicyState::Ready,
            ..Conditions::default()
        }
    }

    fn state(previous: OperatingState, conditions: &Conditions) -> OperatingState {
        evaluate(previous, conditions).to
    }

    fn reason(previous: OperatingState, conditions: &Conditions) -> StateReason {
        evaluate(previous, conditions).reason
    }

    // -- The defaults --------------------------------------------------------

    /// The single most important assertion in this file.
    ///
    /// A caller that has observed nothing has established nothing, and the machine must say so. A
    /// default that read as "kernel live, body present, self-test passed" would be a robot licensed
    /// to run by a struct literal.
    #[test]
    fn a_robot_nobody_has_observed_is_not_a_robot_that_may_run() {
        let blank = Conditions::default();
        assert_eq!(state(OperatingState::Post, &blank), OperatingState::Safe);
        assert_eq!(reason(OperatingState::Post, &blank), StateReason::RhalLost);
        assert!(!state(OperatingState::Post, &blank).commands_autonomously());
    }

    #[test]
    fn an_empty_document_reads_as_the_default_conditions() {
        let parsed = conditions_from_json(&Json::parse("{}").expect("parses"));
        assert_eq!(parsed, Conditions::default());
    }

    /// Misspelling a field is a caller with a bug, and the answer is the least capable reading
    /// rather than a rejection: this function is on the path a report takes off a wire, and a robot
    /// that refused to have a state would be worse than one that reports a held state.
    #[test]
    fn unreadable_fields_fall_back_to_the_least_capable_reading() {
        let parsed = conditions_from_json(
            &Json::parse(
                r#"{"kernel":"LIVE","rhal":"Present","post":"pass","homing":"done",
                    "policy":"READY","stop":"emergency"}"#,
            )
            .expect("parses"),
        );
        assert_eq!(parsed.kernel, KernelState::Down);
        assert_eq!(parsed.rhal, RhalState::Lost);
        assert_eq!(parsed.post, PostVerdict::Incomplete);
        assert_eq!(parsed.homing, HomingState::Pending);
        assert_eq!(parsed.policy, PolicyState::None);
        // An unrecognised stop cause is *no* stop rather than a stop with no cause. The robot is
        // held by the RHAL rule above it either way, and inventing a cause here would put a reason
        // on a console that `STOP_CAUSES` does not contain.
        //
        // The value used to be `"estop"`, chosen because it was a plausible word that the
        // vocabulary did not contain — and on 2026-09-07 the vocabulary grew to contain it, so this
        // test started asserting that a real cause parsed to nothing. `"emergency"` replaces it and
        // the lesson is kept: a fixture whose whole point is that a string is *not* in a closed set
        // has to be reread whenever the set changes, because nothing else will notice.
        assert_eq!(parsed.stop, None);
    }

    // -- The priority order, rule by rule ------------------------------------

    #[test]
    fn a_lost_rhal_is_safe_from_anywhere_and_outranks_everything() {
        let conditions = Conditions {
            rhal: RhalState::Lost,
            ..nominal()
        };
        for previous in OPERATING_STATES {
            assert_eq!(state(previous, &conditions), OperatingState::Safe);
            assert_eq!(reason(previous, &conditions), StateReason::RhalLost);
        }
    }

    #[test]
    fn a_failed_post_is_safe_rather_than_degraded() {
        let conditions = Conditions {
            post: PostVerdict::Failed,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Post, &conditions),
            OperatingState::Safe
        );
        assert_eq!(
            reason(OperatingState::Post, &conditions),
            StateReason::PostFailed
        );
    }

    /// The distinction §48.4 spends its length on. An incomplete self-test does not need a human;
    /// it needs the rest of the operating system.
    #[test]
    fn an_incomplete_post_holds_in_post_rather_than_going_safe() {
        let conditions = Conditions {
            post: PostVerdict::Incomplete,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Post, &conditions),
            OperatingState::Post
        );
        assert_eq!(
            reason(OperatingState::Post, &conditions),
            StateReason::PostIncomplete
        );
        assert!(!OperatingState::Post.is_sticky());
    }

    #[test]
    fn each_stop_cause_reports_itself() {
        for (cause, expected) in [
            (StopCause::Watchdog, StateReason::StoppingWatchdog),
            (StopCause::Proximity, StateReason::StoppingProximity),
            (StopCause::Balance, StateReason::StoppingBalance),
            (StopCause::Operator, StateReason::StoppingOperator),
            (StopCause::Estop, StateReason::StoppingEstop),
        ] {
            let conditions = Conditions {
                stop: Some(cause),
                ..nominal()
            };
            assert_eq!(
                state(OperatingState::Running, &conditions),
                OperatingState::Stopping
            );
            assert_eq!(reason(OperatingState::Running, &conditions), expected);
        }
    }

    #[test]
    fn a_settled_stop_is_safe() {
        let conditions = Conditions {
            stop: Some(StopCause::Operator),
            stop_settled: true,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Stopping, &conditions),
            OperatingState::Safe
        );
        assert_eq!(
            reason(OperatingState::Stopping, &conditions),
            StateReason::StopComplete
        );
    }

    /// `stop_settled` is read only when a stop is commanded. A caller that left it set between
    /// stops would otherwise pin every robot on the fleet in `SAFE`.
    #[test]
    fn a_settled_flag_with_no_stop_does_nothing() {
        let conditions = Conditions {
            stop_settled: true,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Running, &conditions),
            OperatingState::Running
        );
    }

    /// An operator stop is honoured whatever the robot is doing, including booting -- which is why
    /// the stop rule sits above the POST rules rather than below them.
    #[test]
    fn a_stop_during_post_stops() {
        let conditions = Conditions {
            post: PostVerdict::Incomplete,
            stop: Some(StopCause::Operator),
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Post, &conditions),
            OperatingState::Stopping
        );
    }

    #[test]
    fn a_body_that_needs_homing_and_has_not_had_it_is_unhomed() {
        let conditions = Conditions {
            homing: HomingState::Pending,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Post, &conditions),
            OperatingState::Unhomed
        );
        assert_eq!(
            reason(OperatingState::Post, &conditions),
            StateReason::HomingPending
        );
    }

    /// §48.3: "A body that needs no homing declares so and the state is skipped."
    #[test]
    fn a_body_that_declares_it_needs_no_homing_skips_the_state() {
        let conditions = Conditions {
            homing: HomingState::NotRequired,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Post, &conditions),
            OperatingState::Running
        );
    }

    /// §27: "a robot with a policy and no kernel must not exist, so the runtime refuses to start if
    /// the kernel is not live". Refusing to start is `HOLD`, not `SAFE` -- nothing has gone wrong.
    #[test]
    fn a_dead_kernel_holds_rather_than_going_safe() {
        let conditions = Conditions {
            kernel: KernelState::Down,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Hold, &conditions),
            OperatingState::Hold
        );
        assert_eq!(
            reason(OperatingState::Hold, &conditions),
            StateReason::KernelDown
        );
    }

    /// And it outranks a ready policy in both directions: a policy ready to command a body with no
    /// kernel between it and the joints is the one arrangement §27 refuses outright.
    #[test]
    fn a_dead_kernel_outranks_a_ready_policy_and_an_unavailable_guard() {
        let conditions = Conditions {
            kernel: KernelState::Down,
            guards_unavailable: true,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Running, &conditions),
            OperatingState::Hold
        );
        assert_eq!(
            reason(OperatingState::Running, &conditions),
            StateReason::KernelDown
        );
    }

    #[test]
    fn a_guard_hold_outranks_a_ready_policy() {
        let conditions = Conditions {
            guard_hold: true,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Running, &conditions),
            OperatingState::Hold
        );
        assert_eq!(
            reason(OperatingState::Running, &conditions),
            StateReason::GuardHold
        );
    }

    /// "Nothing to run" and "something that has not got a lease" are the same state and different
    /// sentences, and only the second has a remedy -- §25.2's table is where an operator goes.
    #[test]
    fn no_policy_and_a_policy_without_a_lease_are_distinguished() {
        let none = Conditions {
            policy: PolicyState::None,
            ..nominal()
        };
        let held = Conditions {
            policy: PolicyState::Held,
            ..nominal()
        };
        assert_eq!(state(OperatingState::Hold, &none), OperatingState::Hold);
        assert_eq!(state(OperatingState::Hold, &held), OperatingState::Hold);
        assert_eq!(reason(OperatingState::Hold, &none), StateReason::NoPolicy);
        assert_eq!(
            reason(OperatingState::Hold, &held),
            StateReason::PolicyNotReady
        );
    }

    // -- `DEGRADED` is a running state ---------------------------------------

    #[test]
    fn an_unavailable_guard_under_a_ready_policy_is_degraded() {
        let conditions = Conditions {
            guards_unavailable: true,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Running, &conditions),
            OperatingState::Degraded
        );
        assert_eq!(
            reason(OperatingState::Running, &conditions),
            StateReason::GuardsUnavailable
        );
    }

    #[test]
    fn a_degraded_rhal_under_a_ready_policy_is_degraded() {
        let conditions = Conditions {
            rhal: RhalState::Degraded,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Running, &conditions),
            OperatingState::Degraded
        );
        assert_eq!(
            reason(OperatingState::Running, &conditions),
            StateReason::RhalDegraded
        );
    }

    /// The decision the ordering encodes. §27's `DEGRADED` row says the robot "**runs**", so a
    /// robot with an unavailable guard and nothing to run is holding -- and reporting it as degraded
    /// would tell a console a robot is running when it is standing still.
    #[test]
    fn an_unavailable_guard_with_no_policy_is_hold_and_not_degraded() {
        let conditions = Conditions {
            guards_unavailable: true,
            policy: PolicyState::None,
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Hold, &conditions),
            OperatingState::Hold
        );
        assert_eq!(
            reason(OperatingState::Hold, &conditions),
            StateReason::NoPolicy
        );
    }

    /// A guard the robot lost outranks a body merely reporting itself impaired, because the first
    /// names something specific an operator can look up and the second does not.
    #[test]
    fn an_unavailable_guard_is_reported_before_a_degraded_rhal() {
        let conditions = Conditions {
            guards_unavailable: true,
            rhal: RhalState::Degraded,
            ..nominal()
        };
        assert_eq!(
            reason(OperatingState::Running, &conditions),
            StateReason::GuardsUnavailable
        );
    }

    // -- Stickiness ----------------------------------------------------------

    /// §27: "A robot that recovers itself from a safety stop is a robot that resumes moving while
    /// somebody is standing next to it working out what happened."
    #[test]
    fn safe_does_not_recover_itself_however_good_the_conditions_are() {
        let conditions = nominal();
        assert_eq!(
            state(OperatingState::Safe, &conditions),
            OperatingState::Safe
        );
        assert_eq!(
            reason(OperatingState::Safe, &conditions),
            StateReason::SafeSticky
        );
        // And it is the only state that behaves this way.
        for previous in OPERATING_STATES {
            if previous == OperatingState::Safe {
                continue;
            }
            assert_eq!(state(previous, &conditions), OperatingState::Running);
        }
    }

    #[test]
    fn a_named_clearance_lets_it_go() {
        let conditions = Conditions {
            cleared_by: "operator:ana".to_string(),
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Safe, &conditions),
            OperatingState::Running
        );
        assert_eq!(
            reason(OperatingState::Safe, &conditions),
            StateReason::Nominal
        );
    }

    #[test]
    fn an_empty_clearance_is_not_a_clearance() {
        let conditions = Conditions {
            cleared_by: String::new(),
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Safe, &conditions),
            OperatingState::Safe
        );
    }

    /// The property that made it unnecessary to invent a destination for a cleared `SAFE`. Clearing
    /// clears the stickiness; the conditions still decide, so a robot whose estop is still pressed
    /// goes straight back -- with the reason that put it there rather than the one it was cleared by.
    #[test]
    fn clearing_a_robot_whose_condition_persists_returns_it_with_the_real_reason() {
        let conditions = Conditions {
            stop: Some(StopCause::Operator),
            stop_settled: true,
            cleared_by: "operator:ana".to_string(),
            ..nominal()
        };
        assert_eq!(
            state(OperatingState::Safe, &conditions),
            OperatingState::Safe
        );
        assert_eq!(
            reason(OperatingState::Safe, &conditions),
            StateReason::StopComplete
        );
    }

    /// And a robot in `SAFE` whose self-test still fails does not report that somebody has not
    /// cleared it -- `safe_sticky` would send an operator to press a button they already pressed.
    #[test]
    fn a_persisting_condition_is_reported_over_the_stickiness() {
        let conditions = Conditions {
            post: PostVerdict::Failed,
            ..nominal()
        };
        assert_eq!(
            reason(OperatingState::Safe, &conditions),
            StateReason::PostFailed
        );
    }

    // -- §49.1 ---------------------------------------------------------------

    /// The bug the old string comparison froze as correct.
    ///
    /// `hold -> degraded` is a policy beginning to command a body. It is not a transition into
    /// `running`, so the tone did not sound -- on a robot with a lapsed calibration, which §45.5
    /// makes the ordinary case rather than the exotic one.
    #[test]
    fn entering_degraded_sounds_the_tone_which_the_string_version_did_not() {
        assert!(warning_required(
            OperatingState::Hold,
            OperatingState::Degraded
        ));
        assert!(warning_required(
            OperatingState::Unhomed,
            OperatingState::Degraded
        ));
        assert!(warning_required(
            OperatingState::Safe,
            OperatingState::Degraded
        ));
    }

    #[test]
    fn the_tone_sounds_entering_running_and_not_while_already_commanding() {
        assert!(warning_required(
            OperatingState::Hold,
            OperatingState::Running
        ));
        // "Resumption after a stop" is the same transition, which is why one rule covers both.
        assert!(warning_required(
            OperatingState::Stopping,
            OperatingState::Running
        ));
        assert!(warning_required(
            OperatingState::Safe,
            OperatingState::Running
        ));
        assert!(!warning_required(
            OperatingState::Running,
            OperatingState::Running
        ));
        assert!(!warning_required(
            OperatingState::Running,
            OperatingState::Degraded
        ));
        assert!(!warning_required(
            OperatingState::Degraded,
            OperatingState::Running
        ));
        assert!(!warning_required(
            OperatingState::Running,
            OperatingState::Hold
        ));
    }

    #[test]
    fn the_transition_carries_the_tone_and_whether_anything_changed() {
        let transition = evaluate(OperatingState::Hold, &nominal());
        assert_eq!(transition.from, OperatingState::Hold);
        assert_eq!(transition.to, OperatingState::Running);
        assert!(transition.changed);
        assert!(transition.warning);

        let settled = evaluate(OperatingState::Running, &nominal());
        assert!(!settled.changed);
        assert!(!settled.warning);
    }

    // -- The properties other sections read off the machine ------------------

    /// §33.4's barrier. `stopping` is the entry worth asserting: a robot mid-ramp most looks like
    /// it is about to be safe and most certainly is not.
    #[test]
    fn a_ramp_in_progress_is_not_a_safe_state() {
        assert!(!OperatingState::Stopping.is_safe_state());
        assert!(!OperatingState::Running.is_safe_state());
        assert!(!OperatingState::Degraded.is_safe_state());
        assert!(OperatingState::Hold.is_safe_state());
        assert!(OperatingState::Post.is_safe_state());
        assert!(OperatingState::Unhomed.is_safe_state());
        // The remedy for a robot in SAFE is often the update, so the barrier opens there.
        assert!(OperatingState::Safe.is_safe_state());
    }

    /// A state that commands is never a safe state, and the two are defined separately. Asserting
    /// the relation is cheaper than remembering it, and an edit to one that broke it would
    /// otherwise be found by a driver swapped under a moving robot.
    #[test]
    fn nothing_that_commands_is_ever_a_barrier_a_driver_swap_may_cross() {
        for state in OPERATING_STATES {
            assert!(!(state.commands_autonomously() && state.is_safe_state()));
        }
    }

    #[test]
    fn exactly_one_state_is_sticky() {
        let sticky: Vec<&str> = OPERATING_STATES
            .iter()
            .filter(|state| state.is_sticky())
            .map(|state| state.as_str())
            .collect();
        assert_eq!(sticky, vec!["safe"]);
    }

    // -- Totality and round trips --------------------------------------------

    /// The property §33.3 asks of anything that converges: every combination produces a state,
    /// including combinations nobody anticipated. The assertions are only that the machine is
    /// total and self-consistent; the answers themselves are pinned by the corpus.
    #[test]
    fn the_machine_is_total_over_every_combination_of_conditions() {
        let mut evaluated = 0usize;
        for previous in OPERATING_STATES {
            for post in POST_VERDICTS {
                for homing in HOMING_STATES {
                    for kernel in KERNEL_STATES {
                        for rhal in RHAL_STATES {
                            for stop in [None, Some(StopCause::Watchdog), Some(StopCause::Operator)]
                            {
                                for settled in [false, true] {
                                    for guard_hold in [false, true] {
                                        for policy in POLICY_STATES {
                                            let conditions = Conditions {
                                                post,
                                                homing,
                                                kernel,
                                                rhal,
                                                stop,
                                                stop_settled: settled,
                                                guard_hold,
                                                guards_unavailable: false,
                                                policy,
                                                cleared_by: String::new(),
                                            };
                                            let transition = evaluate(previous, &conditions);
                                            assert_eq!(transition.from, previous);
                                            assert_eq!(
                                                transition.changed,
                                                transition.to != previous
                                            );
                                            assert_eq!(
                                                transition.warning,
                                                warning_required(previous, transition.to)
                                            );
                                            // A state nobody has cleared never leaves `SAFE`.
                                            if previous == OperatingState::Safe {
                                                assert_eq!(transition.to, OperatingState::Safe);
                                            }
                                            evaluated += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(evaluated, 7 * 3 * 3 * 2 * 3 * 3 * 2 * 2 * 3);
    }

    /// Evaluating twice over unchanged conditions is a no-op, which is what makes this safe to call
    /// from a level-triggered loop that runs on a timer and on every event.
    #[test]
    fn re_evaluating_from_the_state_it_produced_changes_nothing() {
        for previous in OPERATING_STATES {
            for conditions in [nominal(), Conditions::default()] {
                let first = evaluate(previous, &conditions);
                let second = evaluate(first.to, &conditions);
                assert_eq!(second.to, first.to);
                assert!(!second.changed);
                assert!(!second.warning);
            }
        }
    }

    #[test]
    fn every_state_round_trips_through_its_spelling() {
        for state in OPERATING_STATES {
            assert_eq!(OperatingState::parse(state.as_str()), Some(state));
        }
        assert_eq!(OperatingState::parse("Running"), None);
        assert_eq!(OperatingState::parse("post_incomplete"), None);
    }

    /// Every vocabulary is a set, and every spelling in it is distinct. A duplicate would be a
    /// state the corpus could not tell from another one.
    #[test]
    fn the_vocabularies_have_no_duplicates() {
        fn distinct(words: &[&str]) -> bool {
            words
                .iter()
                .enumerate()
                .all(|(index, word)| !words[..index].contains(word))
        }
        assert!(distinct(&state_names()));
        let reasons: Vec<&str> = STATE_REASONS.iter().map(|r| r.as_str()).collect();
        assert!(distinct(&reasons));
        let causes: Vec<&str> = STOP_CAUSES.iter().map(|c| c.as_str()).collect();
        assert!(distinct(&causes));
    }

    /// Every reason is reachable. A reason nothing produces is a word on a console that never
    /// appears, and the way that happens is a rule being reordered above the one that produced it.
    #[test]
    fn every_reason_is_produced_by_some_condition() {
        let cases = [
            Conditions::default(),
            Conditions {
                post: PostVerdict::Failed,
                ..nominal()
            },
            Conditions {
                post: PostVerdict::Incomplete,
                ..nominal()
            },
            Conditions {
                stop: Some(StopCause::Watchdog),
                ..nominal()
            },
            Conditions {
                stop: Some(StopCause::Proximity),
                ..nominal()
            },
            Conditions {
                stop: Some(StopCause::Balance),
                ..nominal()
            },
            Conditions {
                stop: Some(StopCause::Operator),
                ..nominal()
            },
            Conditions {
                stop: Some(StopCause::Operator),
                stop_settled: true,
                ..nominal()
            },
            Conditions {
                homing: HomingState::Pending,
                ..nominal()
            },
            Conditions {
                kernel: KernelState::Down,
                ..nominal()
            },
            Conditions {
                guard_hold: true,
                ..nominal()
            },
            Conditions {
                policy: PolicyState::None,
                ..nominal()
            },
            Conditions {
                policy: PolicyState::Held,
                ..nominal()
            },
            Conditions {
                guards_unavailable: true,
                ..nominal()
            },
            Conditions {
                rhal: RhalState::Degraded,
                ..nominal()
            },
            Conditions {
                stop: Some(StopCause::Estop),
                ..nominal()
            },
            nominal(),
        ];
        let mut seen: Vec<StateReason> = cases
            .iter()
            .map(|conditions| reason(OperatingState::Hold, conditions))
            .collect();
        // The one reason no condition produces, because it is a property of the previous state.
        seen.push(reason(OperatingState::Safe, &nominal()));

        for expected in STATE_REASONS {
            assert!(
                seen.contains(&expected),
                "no condition produces `{}`",
                expected.as_str()
            );
        }
    }

    #[test]
    fn the_json_shapes_are_what_a_console_reads() {
        let transition = evaluate(OperatingState::Hold, &nominal());
        assert_eq!(
            transition_to_json(&transition)
                .to_canonical()
                .expect("canonical"),
            r#"{"changed":true,"from":"hold","reason":"nominal","to":"running","warning":true}"#
        );
        assert_eq!(
            Json::as_arr(&warning_matrix_to_json()).map(<[Json]>::len),
            Some(49)
        );
        assert_eq!(
            Json::as_arr(&state_properties_to_json()).map(<[Json]>::len),
            Some(7)
        );
    }
}
