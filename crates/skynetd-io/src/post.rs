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
//! # Five of §48.2's table is not `skynetd`'s to run, by construction
//!
//! This is the interesting structural fact and it falls out of §33.1. `skynetd` never touches an
//! actuator — CI asserts it cannot even *depend* on `skynet-rhal` — and that property is what
//! licenses everything else about the daemon. Five of §48.2's eight checks require exactly what
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
//! So the kernel runs its five before the loop turns and records each on §13's ring, and this
//! module reads the verdicts back instead of running the checks itself. A POST that silently
//! omitted the five would be a supervisor claiming to have proved a stop path it is forbidden
//! from touching; a POST that ran them itself would be a supervisor touching an actuator. The
//! ring between them is what §48.5's split is.
//!
//! What "no verdict recorded" means is [`Outcome::Unavailable`], *not* `elsewhere` and not a
//! failure: the kernel may not have started yet, and [`Report::may_run`] re-evaluates every pass,
//! so a late kernel is waited on rather than failed. A verdict that *is* recorded is rendered as
//! the kernel rendered it — pass or fail with the kernel's own sentence — because rewording
//! another process's evidence would be claiming it.

use skynet_blackbox::record::{Body, EventCode};
use skynet_blackbox::ring::Ring;
use skynet_contracts::opstate::{OperatingState, PostVerdict};
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
    /// Somebody else's check, whose verdict lives somewhere this process cannot read.
    ///
    /// The vocabulary stays for §48.4's table, which names it. The kernel's five are no longer
    /// reported this way — their verdicts cross on §13's ring, which this process *can* read —
    /// and an unrecorded one is [`Outcome::Unavailable`] rather than this.
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
    /// **Every** check must pass. An unavailable check has established nothing — a verdict the
    /// kernel never recorded is not a check that ran — so it is not a licence either. A robot
    /// with no kernel, no driver and no proven stop path answers false here, and that is the
    /// correct answer for it.
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

    /// The one input §27's machine takes from the self-test.
    ///
    /// This used to be the *whole* answer: a `&'static str` returning `"safe"`, `"unhomed"` or
    /// `"post_incomplete"`, described as "the state §48.1's machine should be in after this". It
    /// was never quite that, and the reason the string survived is that §27's machine was declared
    /// nowhere for it to be a value of. It is declared now, in
    /// [`skynet_contracts::opstate`], and POST supplies **one of its eight conditions**.
    ///
    /// So the tri-state is what leaves this module, and the state is decided by the machine with
    /// the other eight in hand. A self-test cannot know whether the body needs homing, whether the
    /// kernel came up, or whether anybody stopped the robot while it ran.
    pub fn post_verdict(&self) -> PostVerdict {
        if self.failed() {
            PostVerdict::Failed
        } else if self.may_run() {
            PostVerdict::Passed
        } else {
            PostVerdict::Incomplete
        }
    }

    /// §48.4's own word for where this leaves the robot.
    ///
    /// Kept, because §48.4 gives these three spellings and `post_incomplete` is one of them — it is
    /// a *verdict* rather than a state, and the distinction is the section's point: "the verdict for
    /// that state is `post_incomplete` rather than `safe`, because a robot whose checks could not
    /// run does not need a human."
    ///
    /// Derived from [`Report::post_verdict`] rather than computed a second time, so the two cannot
    /// disagree about a robot. `unhomed` is what a passing self-test licenses — the next state in
    /// §48.1's chain, not a claim that the robot is running.
    pub fn verdict(&self) -> &'static str {
        match self.post_verdict() {
            PostVerdict::Failed => OperatingState::Safe.as_str(),
            PostVerdict::Passed => OperatingState::Unhomed.as_str(),
            PostVerdict::Incomplete => "post_incomplete",
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
        // The machine's input, beside §48.4's word for it. Both, because they answer different
        // questions: `verdict` is what §48.4 tells a person, and `post` is what §27's evaluator
        // reads — and a console that showed only the first would be describing a robot's state
        // from one of eight conditions.
        out.set("post", Json::Str(self.post_verdict().as_str().to_string()));
        out.set("may_run", Json::Bool(self.may_run()));
        out.set("failed", Json::Bool(self.failed()));
        // Said in the payload, because a page listing eight checks with three answers would
        // otherwise read as a self-test that mostly ran.
        out.set(
            "partial",
            Json::Str(
                "§33.1 forbids skynetd from touching an actuator, so five of §48.2's checks run in \
                 the kernel before the loop turns, and their verdicts cross on §13's ring. Rows the \
                 kernel never recorded are unavailable, not failed: the loop may not have started"
                    .to_string(),
            ),
        );
        out
    }
}

/// The kernel's five checks, in §48.2's order: the wire-stable name and what a failure means.
const KERNEL_CHECKS: [(&str, &str); 5] = [
    ("rhal_abi", "wrong driver for this OS"),
    (
        "describe_matches_descriptor",
        "driver and descriptor disagree about the body",
    ),
    (
        "estop_asserted_and_observed",
        "the stop path is not proven — nothing runs",
    ),
    ("encoder_sanity", "sensor or bus fault"),
    (
        "kernel_corpus_selfcheck",
        "the kernel on this machine is not the kernel that was certified",
    ),
];

/// What the kernel recorded on §13's ring: its five verdicts and its worst-case cycle.
pub struct KernelPost {
    /// The newest verdict per kernel check, as (passed, detail), keyed by check name.
    pub verdicts: std::collections::BTreeMap<String, (bool, String)>,
    /// The newest worst-case measurement and how many cycles it stands over, in milliseconds.
    pub timing: Option<(f64, u64)>,
    /// Why `timing` is `None`, or why verdicts may be missing: no ring, an unreadable ring, or
    /// a ring the kernel has not written its verdicts to yet.
    pub ring_note: String,
}

/// Read the kernel's verdicts and worst-case off the ring.
///
/// Newest wins per check and for the timing: older verdicts are earlier boots, superseded the
/// moment the kernel records again. What this does *not* do is gate on the kernel being alive —
/// a verdict outlives the process that recorded it, and a dead kernel's pass still reads as a
/// pass here. That is safe rather than merely convenient: the robot's state comes from all eight
/// conditions together, and a kernel that is not turning holds the robot through `homing` and
/// `kernel` (§27's fifth and sixth rules) whatever POST says. `GET /post` can therefore show a
/// pass beside a dead kernel; the verdicts render verbatim so a reader sees the kernel's own
/// sentence, and `/state` beside it says the kernel is down.
///
/// The whole ring is read, every pass this runs on, but only its events are decoded: a slot
/// holding a control cycle is skipped on its kind byte. Decoding every slot to build a sorted
/// `Vec` of the whole ring, which is what this did until `DRY-COVERAGE-PLAN.md` F289, took eight
/// seconds a pass in a debug build against the daemon's default 64 MiB ring — sixteen intervals
/// of the half second `e2e.sh` runs it at — and the earlier claim here that it "matches what the
/// sightings already cost" had never been measured. What it reads is the kernel's: the verdicts
/// are rewritten as the ring turns (`Recorder::restate_post`), so the newest of each is never
/// more than a quarter of the ring old.
pub fn read_kernel_post(ring: &std::path::Path) -> KernelPost {
    let empty = |note: String| KernelPost {
        verdicts: std::collections::BTreeMap::new(),
        timing: None,
        ring_note: note,
    };
    let store = match std::fs::read(ring) {
        Ok(store) => store,
        Err(_) => return empty(format!("no ring at {}", ring.display())),
    };
    let opened = match Ring::open(&store) {
        Ok(opened) => opened,
        Err(error) => {
            return empty(format!(
                "the ring at {} does not open: {error:?}",
                ring.display()
            ));
        }
    };
    let mut verdicts = std::collections::BTreeMap::new();
    let mut timing: Option<(f64, u64)> = None;
    for record in opened.events(&store).iter().rev() {
        let Body::Event(event) = &record.body else {
            continue;
        };
        match event.code {
            EventCode::PostPassed => {
                verdicts
                    .entry(event.subject.clone())
                    .or_insert((true, event.detail.clone()));
            }
            EventCode::PostFailed => {
                verdicts
                    .entry(event.subject.clone())
                    .or_insert((false, event.detail.clone()));
            }
            EventCode::KernelTiming if timing.is_none() => {
                // Newest first, and unparseable measurements are skipped rather than kept:
                // an older readable timing beats a newer garbled one, and a ring with no
                // readable timing at all reports the window unavailable.
                if let Some(measured) = parse_timing(&event.detail) {
                    timing = Some(measured);
                }
            }
            _ => {}
        }
    }
    KernelPost {
        verdicts,
        timing,
        ring_note: "the kernel has recorded no verdict on this ring yet".to_string(),
    }
}

/// Parse `worst <ms> ms over <cycles> cycles`, strictly.
///
/// Anything else is not a measurement: the supervisor refuses to guess at a figure the watchdog
/// nesting checks against, and an unreadable timing is an unavailable window rather than a
/// number.
fn parse_timing(detail: &str) -> Option<(f64, u64)> {
    let rest = detail.strip_prefix("worst ")?;
    let (ms, rest) = rest.split_once(" ms over ")?;
    let (cycles, rest) = rest.split_once(" cycles")?;
    if !rest.is_empty() {
        return None;
    }
    let ms: f64 = ms.parse().ok()?;
    let cycles: u64 = cycles.parse().ok()?;
    if !ms.is_finite() || ms < 0.0 {
        return None;
    }
    Some((ms, cycles))
}

/// Run the self-test (§48.2).
///
/// `wall_s` is supplied for the same reason it is everywhere else: calibration expiry is a
/// comparison against a clock, and a function that reads one cannot be tested against a fixed one.
pub fn run(layout: &Layout, wall_s: Option<f64>) -> Report {
    let kernel = read_kernel_post(&layout.blackbox().join("ring"));
    let mut checks = Vec::with_capacity(8);
    for (name, failure_means) in KERNEL_CHECKS.into_iter().take(4) {
        checks.push(kernel_check(&kernel, name, failure_means));
    }
    checks.push(calibration_check(layout, wall_s));
    checks.push(blackbox_check(layout));
    let (name, failure_means) = KERNEL_CHECKS[4];
    checks.push(kernel_check(&kernel, name, failure_means));
    checks.push(nesting_check(layout, &kernel));

    Report { checks }
}

/// One kernel check, rendered as the kernel rendered it — or unavailable, when it recorded
/// nothing.
fn kernel_check(kernel: &KernelPost, check: &'static str, failure_means: &'static str) -> Check {
    match kernel.verdicts.get(check) {
        Some((passed, detail)) => Check {
            check,
            outcome: if *passed {
                Outcome::Pass
            } else {
                Outcome::Fail
            },
            failure_means,
            detail: detail.clone(),
        },
        None => Check {
            check,
            outcome: Outcome::Unavailable,
            failure_means,
            detail: format!(
                "{} — the loop may not have started, and the kernel's own log names the reason",
                kernel.ring_note,
            ),
        },
    }
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
    // `ring`, which is what both recorders write — this checked `ring.bin`, a file nothing has
    // ever written, until 2026-09-26 (F156), so the check was unavailable on every robot with a
    // recording and nobody noticed, because `post_incomplete` was already the answer.
    let ring = path.join("ring");
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
///
/// The first window — the kernel's own worst-case cycle — is the kernel's to measure (§46.3)
/// and arrives on the ring beside its verdicts. Until it does the window is missing and the
/// check is unavailable, which is what holds a robot in `post` through its first hundred cycles:
/// the loop has not proven its timing yet. That was unconditionally true until 2026-09-26
/// (F155) — the window had no source at all, so no robot could ever leave `post`.
fn nesting_check(layout: &Layout, kernel: &KernelPost) -> Check {
    let body = crate::embodiment::load(layout);
    let known = body.known();
    let declared = known.and_then(|known| known.descriptor.watchdog.as_ref());
    let windows = skynet_contracts::watchdog::Windows {
        kernel_cycle_worst_case_ms: kernel.timing.map(|(ms, _)| ms),
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
        let missing = report.missing.join(", ");
        let measured = match kernel.timing {
            // Said, because a page that only listed the missing windows would not say the kernel
            // half arrived: the wait is on the declaration, not the loop.
            Some((ms, cycles)) => format!(
                "the kernel measured its worst at {ms:.3} ms over {cycles} cycles; the wait is on \
                 the declaration, not the loop"
            ),
            None => kernel.ring_note.clone(),
        };
        (
            Outcome::Unavailable,
            format!(
                "{} of 4 windows are undeclared ({}), so the nesting is not established. {measured}",
                report.missing.len(),
                missing,
            ),
        )
    } else {
        // `missing` is empty, so the kernel window was measured: it is one of the four, and a
        // `None` would be listed above. The fallback names no figure rather than panicking,
        // because a detail formatter must not be what fails a boot.
        let (ms, cycles) = kernel.timing.unwrap_or((f64::NAN, 0));
        (
            Outcome::Pass,
            format!(
                "all four windows are declared and nest, so every layer of the chain is reachable \
                 (kernel worst {ms:.3} ms over {cycles} cycles)"
            ),
        )
    };

    Check {
        check: "watchdog_nesting",
        outcome,
        failure_means: "a layer of the chain is unreachable",
        detail,
    }
}
