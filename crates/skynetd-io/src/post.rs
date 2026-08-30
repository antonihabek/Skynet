//! The power-on self-test (§48.2).
//!
//! §48.1's reason for it is blunt: with incremental encoders — still common on arms — joint
//! positions at power-on are unknown, forward kinematics is meaningless, every pose-dependent guard
//! is unavailable, and the robot must not be permitted to run a policy. So the state machine gains
//! a state before `HOLD`, and POST is what decides whether the robot may leave it.
//!
//! # Three outcomes, and the third is the one this edition mostly produces
//!
//! §48.2 gives two: pass, or **`SAFE`, not `DEGRADED`** — sticky, requiring a human, because a robot
//! that failed its own self-test should not be talked into running by a reconcile loop.
//!
//! There is a third and it is not a softening. A check whose *input does not exist* has not passed
//! and has not failed: nothing is broken and nothing was established. [`Outcome::Unavailable`]
//! reports that, and it does **not** permit running either — [`Report::may_run`] is true only when
//! every check passed. The distinction is entirely about where it sends a person: a *failed* estop
//! check means the stop path is broken and somebody must look at the machine; an *unavailable* one
//! means this edition has no RHAL driver to assert a stop through, and there is nothing on the
//! machine to look at.
//!
//! Collapsing them would produce the two worst sentences available: "self-test passed" on a robot
//! that tested almost nothing, or "self-test failed" on a robot with nothing wrong with it.
//!
//! # Half of §48.2's table is not `skynetd`'s to answer, by construction
//!
//! This is the interesting structural fact and it falls out of §33.1. `skynetd` never touches an
//! actuator — CI asserts it cannot even *depend* on `skynet-rhal` — and that property is what
//! licenses everything else about the daemon. Four of §48.2's eight checks require exactly what
//! that property denies:
//!
//! | Check | Owner |
//! |---|---|
//! | RHAL ABI version matches | The kernel, which loads the driver |
//! | `describe()` parses; joint count matches | The kernel |
//! | Estop asserted **and observed** | The kernel, through the RHAL |
//! | Encoder sanity | The kernel, through the RHAL |
//! | Calibration expiry evaluated (§45) | **`skynetd`** |
//! | Black box writable, reserve intact | **`skynetd`** |
//! | Kernel corpus self-check | The kernel |
//! | Watchdog window nesting valid (§46.3) | **`skynetd`** |
//!
//! So this module runs three and reports the other five as belonging elsewhere — *not* as
//! unavailable-because-unwritten, which would be a different and wrong sentence. A POST that
//! silently omitted the five would be a supervisor claiming to have proved a stop path it is
//! forbidden from touching.

use skynet_json::Json;

use crate::layout::Layout;

/// What one check concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The check ran and the robot is good on this point.
    Pass,
    /// The check ran and the robot is not.
    Fail,
    /// The input does not exist. Not a pass, and not a fault in the machine.
    Unavailable,
    /// Somebody else's check: the kernel's, through a path §33.1 denies this process.
    Elsewhere,
}

impl Outcome {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Pass => "pass",
            Outcome::Fail => "fail",
            Outcome::Unavailable => "unavailable",
            Outcome::Elsewhere => "elsewhere",
        }
    }
}

/// One row of §48.2's table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// A code a console can translate.
    pub check: &'static str,
    /// What it concluded.
    pub outcome: Outcome,
    /// What a failure would mean, from §48.2's second column. Present whatever the outcome, because
    /// it is what makes an `elsewhere` row worth showing at all.
    pub failure_means: &'static str,
    /// What actually happened here.
    pub detail: String,
}

/// The whole self-test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Every check, in §48.2's order.
    pub checks: Vec<Check>,
}

impl Report {
    /// Whether this robot may leave `POST`.
    ///
    /// **Every** check must pass. An unavailable check has established nothing and a check owned by
    /// the kernel has not been run by anybody this process can ask, so neither is a licence — which
    /// on this edition means `may_run` is false, and that is the correct answer for a robot with no
    /// kernel, no driver and no proven stop path.
    pub fn may_run(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.outcome == Outcome::Pass)
    }

    /// Whether anything is actually wrong, as against merely unestablished.
    ///
    /// This is what decides `SAFE`. §48.2 sends a *failed* POST to `SAFE` because `SAFE` is sticky
    /// and needs a human; a robot whose checks could not run does not need a human, it needs the
    /// rest of the operating system.
    pub fn failed(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.outcome == Outcome::Fail)
    }

    /// The state §48.1's machine should be in after this.
    pub fn verdict(&self) -> &'static str {
        if self.failed() {
            "safe"
        } else if self.may_run() {
            "unhomed"
        } else {
            "post_incomplete"
        }
    }

    /// The report, for `GET /post` and the console.
    pub fn to_json(&self) -> Json {
        let mut checks = Json::arr();
        for check in &self.checks {
            let mut row = Json::obj();
            row.set("check", Json::Str(check.check.to_string()));
            row.set("outcome", Json::Str(check.outcome.as_str().to_string()));
            row.set("failure_means", Json::Str(check.failure_means.to_string()));
            row.set("detail", Json::Str(check.detail.clone()));
            checks.push(row);
        }

        let mut out = Json::obj();
        out.set("checks", checks);
        out.set("verdict", Json::Str(self.verdict().to_string()));
        out.set("may_run", Json::Bool(self.may_run()));
        out.set("failed", Json::Bool(self.failed()));
        // Said in the payload, because a page listing eight checks with three answers would
        // otherwise read as a self-test that mostly ran.
        out.set(
            "partial",
            Json::Str(
                "§33.1 forbids skynetd from touching an actuator, and four of §48.2's checks — the \
                 RHAL ABI, describe(), the estop assert-and-observe, and encoder sanity — need \
                 exactly that. They are the kernel's, and §27's kernel is unwritten, so nobody has \
                 run them on this robot"
                    .to_string(),
            ),
        );
        out
    }
}

/// A check this process cannot run, and never will.
fn elsewhere(check: &'static str, failure_means: &'static str, owner: &str) -> Check {
    Check {
        check,
        outcome: Outcome::Elsewhere,
        failure_means,
        detail: format!(
            "{owner}. §33.1 keeps skynetd away from the actuator path, which is what licenses \
             everything else about this daemon, so this check is not one it can run — and §27's \
             kernel, which can, is unwritten"
        ),
    }
}

/// Run the self-test (§48.2).
///
/// `wall_s` is supplied for the same reason it is everywhere else: calibration expiry is a
/// comparison against a clock, and a function that reads one cannot be tested against a fixed one.
pub fn run(layout: &Layout, wall_s: Option<f64>) -> Report {
    let checks = vec![
        elsewhere(
            "rhal_abi",
            "wrong driver for this OS",
            "the kernel loads the driver and compares the ABI version",
        ),
        elsewhere(
            "describe_matches_descriptor",
            "driver and descriptor disagree about the body",
            "the kernel calls describe() and compares the joint count",
        ),
        elsewhere(
            "estop_asserted_and_observed",
            "the stop path is not proven — nothing runs",
            "the kernel asserts the stop through the RHAL and observes that it was reported",
        ),
        elsewhere(
            "encoder_sanity",
            "sensor or bus fault",
            "the kernel reads the joints through the RHAL",
        ),
        calibration_check(layout, wall_s),
        blackbox_check(layout),
        elsewhere(
            "kernel_corpus_selfcheck",
            "the kernel on this machine is not the kernel that was certified",
            "the kernel runs a sample of §31.4's corpus against itself",
        ),
        nesting_check(layout),
    ];

    Report { checks }
}

/// §45's expiry, evaluated *before* start so the guards that will be unavailable are known.
///
/// Note what this check is and is not. An expired calibration does **not** fail POST: §45.5 is
/// explicit that expiry *degrades* — the robot keeps running with fewer guards and says so — and a
/// self-test that refused to boot on a stale camera extrinsic would be a robot nobody could get
/// into a state where they could recalibrate it. What POST does is make the list known at power-on
/// rather than discovered at the first command, which is what §48.2's row actually asks for.
fn calibration_check(layout: &Layout, wall_s: Option<f64>) -> Check {
    let status = crate::calibration::status(layout, wall_s);
    let starved = status.unavailable();
    let detail = if starved.is_empty() {
        "every applicable parameter set is believed, so no guard starts short of one".to_string()
    } else {
        let mut guards: Vec<&str> = starved.iter().map(|entry| entry.guard.as_str()).collect();
        guards.sort_unstable();
        guards.dedup();
        format!(
            "{} guard(s) will be unavailable for want of a calibration: {}. §45.5 degrades rather \
             than refuses, so this is known at power-on rather than discovered at the first command",
            guards.len(),
            guards.join(", ")
        )
    };
    Check {
        check: "calibration_expiry",
        outcome: Outcome::Pass,
        failure_means: "guards that will be unavailable are not known before start",
        detail,
    }
}

/// The recorder, and the reserve §28 preallocates so a full disk cannot silence it.
fn blackbox_check(layout: &Layout) -> Check {
    let path = layout.blackbox();
    let ring = path.join("ring.bin");
    let (outcome, detail) = if !ring.exists() {
        (
            Outcome::Unavailable,
            format!(
                "no ring at {} — this robot has not opened a recorder, so nothing has established \
                 whether it could",
                ring.display()
            ),
        )
    } else {
        match std::fs::OpenOptions::new().write(true).open(&ring) {
            Ok(_) => {
                let queue = crate::outbound::Queue::of(layout);
                let status = queue.status();
                (
                    Outcome::Pass,
                    format!(
                        "the ring is writable and {} of the outbound queue's {} byte budget is \
                         reserved for incidents",
                        status.reserve, status.budget
                    ),
                )
            }
            Err(error) => (
                Outcome::Fail,
                format!("the ring exists and cannot be written: {error}"),
            ),
        }
    };
    Check {
        check: "blackbox_writable",
        outcome,
        failure_means: "the recorder cannot record",
        detail,
    }
}

/// §46.3's nesting, at the moment §48.2 asks for it: before anything moves.
fn nesting_check(layout: &Layout) -> Check {
    let body = crate::embodiment::load(layout);
    let known = body.known();
    let declared = known.and_then(|known| known.descriptor.watchdog.as_ref());
    let windows = skynet_contracts::watchdog::Windows {
        kernel_cycle_worst_case_ms: None,
        envelope_watchdog_ms: known
            .and_then(|known| known.descriptor.envelope.as_ref())
            .and_then(|envelope| envelope.watchdog_ms),
        soc_wdt_ms: declared.and_then(|w| w.soc_wdt_ms),
        drive_command_timeout_ms: declared.and_then(|w| w.drive_command_timeout_ms),
    };
    let report = skynet_contracts::watchdog::check_nesting(&windows);

    // An inversion is a **fail**: §46.3 says getting the order wrong makes an upper layer
    // unreachable, and a robot booting with an unreachable watchdog layer is precisely the thing
    // §48.2 wants caught before anything moves. An undeclared window is not — nothing contradicts,
    // and a body nobody has measured is not a body that is wrong.
    let (outcome, detail) = if !report.faults.is_empty() {
        (
            Outcome::Fail,
            report
                .faults
                .iter()
                .map(|fault| fault.detail.clone())
                .collect::<Vec<String>>()
                .join(" "),
        )
    } else if !report.missing.is_empty() {
        (
            Outcome::Unavailable,
            format!(
                "{} of 4 windows are undeclared ({}), so the nesting is not established. Nothing \
                 contradicts and nothing is proven",
                report.missing.len(),
                report.missing.join(", ")
            ),
        )
    } else {
        (
            Outcome::Pass,
            "all four windows are declared and nest, so every layer of the chain is reachable"
                .to_string(),
        )
    };

    Check {
        check: "watchdog_nesting",
        outcome,
        failure_means: "a layer of the chain is unreachable",
        detail,
    }
}
