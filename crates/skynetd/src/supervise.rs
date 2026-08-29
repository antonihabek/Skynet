//! Supervision and crash policy (§33.5).
//!
//! Three properties, and the first is what licenses the other two:
//!
//! * **The kernel holds through every restart.** A restarting policy stops posting commands, the
//!   watchdog observes it, and the robot comes to a controlled stop. Restart is therefore never a
//!   motion event, and *that* is why the restart policy can be liberal.
//! * **Crash loops stop.** Infinite restart is how a robot spends a night power-cycling a driver
//!   and logs ten million lines. After the threshold the package is marked `failed` and left alone.
//! * **Every restart is a black box event**, with the exit status and the last verdict before exit,
//!   so a crash that correlates with a particular command is findable afterwards.
//!
//! # A driver is not restarted, and that is not a missing feature
//!
//! | Kind | On exit | Backoff | Crash loop |
//! |---|---|---|---|
//! | `policy` | Restart | 1 s, ×2, jittered, capped at 60 s | 5 failures in 10 min → `failed` |
//! | `service` | Restart | as above | as above |
//! | `driver` | **Do not restart automatically** | — | Recovery is a deliberate act |
//!
//! A driver that crashed left hardware in an unknown state: a bus mid-transaction, a drive enabled
//! with no command source, brakes in whichever position the crash found them. Restarting it would
//! re-open that hardware and begin commanding it again on the strength of a process having exited,
//! which is not evidence about the machine.
//!
//! # No clock is read here
//!
//! [`Supervision::on_exit`] and [`Supervision::next_start`] take `now_ms`, for the reason the Safety
//! Kernel takes no clock (§35.2): a state machine that reads a clock cannot be replayed, and a
//! crash-loop policy nobody can replay is one nobody can debug from a black box recording.
//!
//! The jitter is likewise supplied rather than generated. Backoff without jitter is how a fleet of
//! two hundred robots that lost the same dependency all retry in the same millisecond; a supervisor
//! that produced its own randomness would be a supervisor whose behaviour cannot be reproduced.

use crate::desired::PackageKind;

/// The numbers, as defaults an operator profile may narrow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrashPolicy {
    /// First backoff, milliseconds.
    pub base_ms: u64,
    /// Multiplier applied on each successive failure.
    pub factor: u32,
    /// Ceiling on the backoff.
    pub cap_ms: u64,
    /// Failures inside `window_ms` that mark the package failed.
    pub threshold: u32,
    /// The window the threshold counts within.
    pub window_ms: u64,
}

impl Default for CrashPolicy {
    fn default() -> Self {
        Self {
            base_ms: 1_000,
            factor: 2,
            cap_ms: 60_000,
            threshold: 5,
            window_ms: 600_000,
        }
    }
}

/// What the supervisor decided to do about an exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    /// Start it again at the returned time.
    RestartAt(u64),
    /// Do not restart. The package is marked failed and left alone (§33.5).
    Failed,
    /// Do not restart, and this is not a failure: a driver's recovery is a deliberate act.
    AwaitOperator,
}

/// What state the supervisor believes one package is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisionState {
    /// Running, or expected to be.
    Up,
    /// Waiting out a backoff, and will start at this monotonic millisecond.
    Backoff(u64),
    /// Abandoned. Nothing will start it but an operator.
    Failed,
}

/// One package's supervision record.
///
/// Deliberately small and copyable: the whole point of §33.1 is that losing this is survivable, and
/// a record that could not be reconstructed from an observation would be a record `skynetd` was
/// obliged to persist correctly.
#[derive(Debug, Clone, PartialEq)]
pub struct Supervision {
    /// Package id.
    pub id: String,
    /// Which restart rules apply.
    pub kind: PackageKind,
    /// The numbers.
    pub policy: CrashPolicy,
    /// Where the package is.
    pub state: SupervisionState,
    /// Exit times inside the window, oldest first.
    ///
    /// A bounded list rather than a counter, because a counter cannot answer "five failures in ten
    /// minutes" — it answers "five failures ever", which marks a package failed after five crashes
    /// spread over a year of uptime.
    pub failures: Vec<u64>,
}

impl Supervision {
    /// A package the supervisor has just started.
    pub fn new(id: impl Into<String>, kind: PackageKind, policy: CrashPolicy) -> Self {
        Self {
            id: id.into(),
            kind,
            policy,
            state: SupervisionState::Up,
            failures: Vec::new(),
        }
    }

    /// Record an exit and decide what happens next.
    ///
    /// `now_ms` is monotonic; `jitter` is a fraction in `[0, 1)` supplied by the caller, and it is
    /// applied as a *subtraction* from the computed delay rather than an addition. Backoff that
    /// jitters upward can exceed its own cap, and a cap that is sometimes exceeded is not a cap.
    pub fn on_exit(&mut self, now_ms: u64, jitter: f64) -> RestartDecision {
        // A driver that crashed left hardware in an unknown state. Recovery is a deliberate act, so
        // the record notes the exit and nothing restarts it.
        if self.kind == PackageKind::Driver {
            self.failures.push(now_ms);
            self.state = SupervisionState::Failed;
            return RestartDecision::AwaitOperator;
        }

        self.failures
            .retain(|at| now_ms.saturating_sub(*at) < self.policy.window_ms);
        self.failures.push(now_ms);

        if self.failures.len() as u32 >= self.policy.threshold {
            self.state = SupervisionState::Failed;
            return RestartDecision::Failed;
        }

        let delay = self.backoff_ms(self.failures.len() as u32, jitter);
        let at = now_ms.saturating_add(delay);
        self.state = SupervisionState::Backoff(at);
        RestartDecision::RestartAt(at)
    }

    /// The delay before the *n*th consecutive restart, jittered.
    ///
    /// Saturating throughout: an exponent large enough to overflow is a package that has been
    /// crashing for longer than the window allows, which the threshold has already caught, and a
    /// panic in the supervisor is a worse answer to that than a capped delay.
    pub fn backoff_ms(&self, failures: u32, jitter: f64) -> u64 {
        let steps = failures.saturating_sub(1).min(32);
        let scaled = self
            .policy
            .base_ms
            .saturating_mul(u64::from(self.policy.factor).saturating_pow(steps));
        let capped = scaled.min(self.policy.cap_ms);
        let jitter = jitter.clamp(0.0, 1.0);
        // Down to half, never up: see `on_exit`.
        let reduced = (capped as f64) * (1.0 - 0.5 * jitter);
        reduced as u64
    }

    /// When this package may next be started, if ever.
    pub fn next_start(&self) -> Option<u64> {
        match self.state {
            SupervisionState::Up => None,
            SupervisionState::Backoff(at) => Some(at),
            SupervisionState::Failed => None,
        }
    }

    /// Whether the package is abandoned.
    pub fn is_failed(&self) -> bool {
        self.state == SupervisionState::Failed
    }

    /// Record a successful start.
    ///
    /// The failure history is **kept**, not cleared. A policy that crashes, restarts, runs for
    /// ninety seconds and crashes again is crash-looping, and a supervisor that forgot the earlier
    /// crashes on every successful start would restart it for ever — the loop the threshold exists
    /// to stop, one restart slower. The window is what forgets, and it forgets by time.
    pub fn on_start(&mut self) {
        self.state = SupervisionState::Up;
    }

    /// An operator clearing a failed package back to a startable state.
    ///
    /// The history goes with it, because an operator who has looked at a crashing driver and
    /// decided to try again is making a judgement the supervisor does not have, and starting them
    /// one crash away from the threshold would spend that judgement immediately.
    pub fn reset(&mut self) {
        self.failures.clear();
        self.state = SupervisionState::Up;
    }
}
