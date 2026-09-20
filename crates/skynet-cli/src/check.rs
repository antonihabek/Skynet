//! `skynet driver check` — the sim half of SCTS, run against a real shared object (§63.3).
//!
//! > Runs the **sim half of SCTS** against the driver locally — ABI version handling, state
//! > completeness, command rejection, descriptor validity, kernel parity. It reports which tests
//! > fail and why, in the terms of §15, so the bench session is spent on the things only hardware
//! > can prove.
//!
//! Turning up to a bench session with the sim suite already green is what makes bench time cheap.
//!
//! # What this covers, and — just as important — what it does not
//!
//! The sim suite has entries that are properties of the *runtime*, not of a driver:
//! `safety.policy.isolation`, `package.*`, `ota.safety_gate`, `telemetry.replay.deterministic`,
//! `security.*`. A driver cannot pass or fail them, and reporting them as passing here would let a
//! vendor arrive at certification believing they had evidence they do not have. So they are listed
//! as **not covered**, by name, at the end of every run.
//!
//! **Where they are covered is now a place rather than a hope.** Each entry's sentence said "needs
//! X" for as long as this list existed, and five of the eight named an X the platform has since
//! built — so every one of them pointed at a gap that had closed. They point at
//! `skynet conformance` now, which runs them on a robot against that robot's own installed code
//! (`skynetd-io::conformance`), and the ones that remain genuinely open say what they are waiting
//! for rather than naming a component that exists. A sentence explaining an absence is a claim, and
//! it decays exactly as fast as the absence does.
//!
//! Three of the eight execute there today. `telemetry.replay.deterministic` was the third and it
//! arrived the same way the others did: its sentence said *"pending resolved limits"*, and the
//! resolution turned out to be one module away — `skynetd-io::limits`, the first caller of
//! `resolve_limits` that is a robot.
//!
//! `safety.kernel.parity` is the interesting one: for an *edition* it means "reproduces the
//! conformance corpus", which `cargo test -p skynet-corpus` measures and this command cannot. It is
//! named as covered elsewhere rather than silently omitted.
//!
//! # Absence is not a pass (§15)
//!
//! A check that could not be run reports [`Outcome::Missing`], never a quiet success. The one
//! exception is [`Outcome::NotApplicable`], which is reserved for cases where inapplicability is a
//! declared property of the body rather than a gap in the evidence.

use std::ffi::{c_char, CStr};

use skynet_contracts::admission::{classify_token, TokenClass};
use skynet_contracts::certification::SCTS_TESTS;
use skynet_contracts::embodiment::{
    canonical_motion, validate_frame_bindings, validate_frames, validate_motions, FrameGraph,
};
use skynet_contracts::json::descriptor_from_json;
use skynet_contracts::kinematics::flatten;
use skynet_contracts::parse_role;
use skynet_contracts::roles::chains_of;
use skynet_contracts::safety::kinematics::FrameSet;
use skynet_contracts::watchdog::{
    resolve_command_loss, CommandLossParams, COMMAND_LOSS_BEHAVIOURS,
};
use skynet_json::Json;
use skynet_rhal::{flags, status, Command, Ctx, Frame, Mode, RawDriver, State};

use crate::term;
use skynet_dl::Library;

/// How one check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Measured, and it holds.
    Pass,
    /// Measured, and it does not.
    Fail,
    /// Could not be measured. **Never** a silent pass.
    Missing,
    /// Genuinely inapplicable to this body, which §15 counts as a pass.
    NotApplicable,
}

/// One check.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    /// The SCTS id this stands for, from §15.
    pub id: &'static str,
    /// What was asserted, in one clause.
    pub what: String,
    /// How it came out.
    pub outcome: Outcome,
    /// The measured value or the reason, for anything that is not a plain pass.
    pub detail: String,
    /// The number, for the one test §15 marks quantitative and this command can measure.
    ///
    /// `None` on every other check, and that is not laziness: §15 marks eight bench tests and one
    /// sim test as quantitative, and a boolean where a number belongs is precisely what §15 means
    /// by *"a driver returning success is not evidence"*. The field exists because the number was
    /// being computed and then spent on a sentence — see [`with_value`](CheckResult::with_value).
    pub value: Option<f64>,
}

impl CheckResult {
    /// A result with every field named.
    pub fn new(
        id: &'static str,
        what: impl Into<String>,
        outcome: Outcome,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id,
            what: what.into(),
            outcome,
            detail: detail.into(),
            value: None,
        }
    }

    /// The same result, carrying the number it measured.
    ///
    /// Added when `skynet driver check --results` was written and its own reader refused the
    /// document: `embodiment.roles.coverage` is the only quantitative test in the sim suite, this
    /// command has computed the percentage since it was written, and the percentage reached a
    /// sentence and stopped. A results document that carried a bare `pass` for it would be one
    /// `award_certification` accepted and nobody could check.
    pub fn with_value(mut self, value: f64) -> Self {
        self.value = Some(value);
        self
    }

    /// Measured, and it holds. A pass carries no detail: there is nothing to explain.
    pub fn pass(id: &'static str, what: impl Into<String>) -> Self {
        Self::new(id, what, Outcome::Pass, String::new())
    }

    /// Measured, and it does not. The detail is what the reader has to act on.
    pub fn fail(id: &'static str, what: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::new(id, what, Outcome::Fail, detail)
    }

    /// A pass or a fail from a boolean, with the detail used only on the failing side.
    pub fn assert(
        id: &'static str,
        what: impl Into<String>,
        held: bool,
        detail: impl Into<String>,
    ) -> Self {
        if held {
            Self::pass(id, what)
        } else {
            Self::fail(id, what, detail)
        }
    }
}

/// Everything one run produced.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Every check, in the order run.
    pub results: Vec<CheckResult>,
    /// `driver_name`, when the vtable declared one.
    pub driver_name: Option<String>,
    /// `driver_version`, when the vtable declared one.
    pub driver_version: Option<String>,
    /// The descriptor the driver returned, when it returned a parseable one.
    pub descriptor: Option<Json>,
    /// Whether the body declared itself simulated (§35.3, §70).
    ///
    /// Not a check outcome, because it is not a defect. It is a fact about the body that caps the
    /// tier, and the report says so rather than letting a reader wonder why a driver that passed
    /// everything is still at guest.
    pub simulated: bool,
}

impl Report {
    fn push(&mut self, result: CheckResult) {
        self.results.push(result);
    }

    /// How many checks came out each way.
    pub fn tally(&self, outcome: Outcome) -> usize {
        self.results.iter().filter(|r| r.outcome == outcome).count()
    }

    /// The worst outcome recorded against one SCTS id.
    ///
    /// One id is exercised by several checks here — `rhal.abi.version` covers the export, the
    /// version, the entry points and the open — and the id passes only if all of them do.
    pub fn outcome_for(&self, id: &str) -> Option<Outcome> {
        let mut worst: Option<Outcome> = None;
        for result in self.results.iter().filter(|result| result.id == id) {
            worst = Some(match (worst, result.outcome) {
                (Some(Outcome::Fail), _) | (_, Outcome::Fail) => Outcome::Fail,
                (Some(Outcome::Missing), _) | (_, Outcome::Missing) => Outcome::Missing,
                (_, other) => other,
            });
        }
        worst
    }

    /// The highest tier this evidence supports, and what stands between it and the next one.
    ///
    /// §15's second award rule: **the evidence caps the award, not the claim.** A driver that
    /// passes every guest-tier sim test and has no signed descriptor is a guest-tier driver, which
    /// is a true statement about it rather than a failure — and saying so keeps a vendor in the
    /// programme instead of sending them off to write "ROS 2 compatible" on a web page.
    ///
    /// Only the **sim** tests this command actually measures count. A bench test is not failed
    /// here, it is simply not in evidence, and it caps the tier from above until a bench session
    /// supplies it.
    pub fn attainable_tier(&self) -> (&'static str, Vec<(&'static str, Outcome)>) {
        let mut attained = "none";
        let mut blockers: Vec<(&'static str, Outcome)> = Vec::new();

        for tier in TIERS {
            let mut held = true;
            let mut tier_blockers: Vec<(&'static str, Outcome)> = Vec::new();
            for test in SCTS_TESTS.iter().filter(|test| {
                test.suite == "sim" && tier_rank(test.required_from) <= tier_rank(tier)
            }) {
                match self.outcome_for(test.id) {
                    Some(Outcome::Pass) | Some(Outcome::NotApplicable) => {}
                    Some(other) => {
                        held = false;
                        tier_blockers.push((test.id, other));
                    }
                    // Not measured here at all — the runtime and edition properties in NOT_COVERED.
                    // They are a real ceiling on what this command can conclude, and are listed
                    // separately rather than counted as failures of the driver.
                    None => {}
                }
            }
            if held {
                attained = tier;
            } else {
                blockers = tier_blockers;
                break;
            }
        }

        // §35.3 and §70. A simulator declares itself one and the declaration caps the award at
        // guest, whatever the driver's own ceiling is — a simulator driver written in C++ has no
        // language ceiling to fall back on, so the cap has to come from the body rather than from
        // the edition. Applied after the walk, so the report can still say what the evidence would
        // have supported.
        if self.simulated && tier_rank(attained) > tier_rank("guest") {
            attained = "guest";
        }

        (attained, blockers)
    }

    /// Whether the driver reached the tier every driver must reach to run at all.
    pub fn green(&self) -> bool {
        tier_rank(self.attainable_tier().0) >= tier_rank("guest")
    }
}

/// Why a driver reached no tier, in the words each cause deserves.
///
/// Kept apart from its caller so the four shapes can be tested without building a report, which is
/// also what makes it possible to assert the thing that matters: that no two of them read alike.
fn no_tier_detail(failed: usize, unmeasured: usize) -> String {
    match (failed, unmeasured) {
        // Neither. The ceiling is something this command does not measure at all — the runtime and
        // edition properties in NOT_COVERED — so naming a count here would invent one.
        (0, 0) => "this driver does not yet reach guest tier".to_string(),
        (0, n) => format!(
            "nothing here failed — {n} guest-tier test(s) have no result yet, and absence is not \
             a pass (§15)"
        ),
        (n, 0) => format!("{n} guest-tier test(s) failed"),
        (n, m) => format!(
            "{n} guest-tier test(s) failed and {m} more have no result — two different problems, \
             and both cap the tier"
        ),
    }
}

/// Tiers, weakest first.
const TIERS: [&str; 3] = ["guest", "host", "certified"];

/// Where a tier sits in [`TIERS`], counting from one — and `"none"`, or any name that is not a tier,
/// at zero, below guest.
///
/// It put them *at* guest until 2026-09-19 (`DRY-COVERAGE-PLAN.md` F82): `position` finds no
/// `"none"` in [`TIERS`], `unwrap_or(0) + 1` made that 1, and 1 is guest's rank. So
/// [`Report::green`] held for a driver that reached no tier at all, and `skynet driver check`
/// exited 0 on every driver that failed guest since the command was written — printing
/// *"no tier"* above an exit code that said the opposite, to every `make check` and CI step that
/// read the code rather than the sentence.
fn tier_rank(tier: &str) -> usize {
    TIERS
        .iter()
        .position(|known| *known == tier)
        .map_or(0, |index| index + 1)
}

/// Sim-suite tests this command cannot measure, and where the evidence for each actually comes from.
///
/// Printed on every run. A vendor arriving at certification believing they had evidence they do not
/// have is a failure of this list, not of their diligence.
pub const NOT_COVERED: &[(&str, &str)] = &[
    (
        "safety.kernel.parity",
        "an edition property — `cargo test -p skynet-corpus`, §31.4",
    ),
    (
        "safety.policy.isolation",
        "a runtime property — `skynet conformance`, against the unit systemd is running",
    ),
    (
        "package.signature.required",
        "a runtime property — `skynet conformance`, and it fails there today",
    ),
    (
        "package.grants.gate",
        "a runtime property — `skynet conformance`, where it runs and reports the tier",
    ),
    (
        "ota.safety_gate",
        "a runtime property — `skynet conformance`, where it passes",
    ),
    (
        "telemetry.replay.deterministic",
        "a runtime property — `skynet conformance`, where it runs on this robot's own limits",
    ),
    (
        "security.identity.attested",
        "a runtime property — `skynet conformance`, pending a TPM to bind a key to",
    ),
    (
        "security.credential.scope",
        "a runtime property, and the Hub's rather than a robot's — see `skynet conformance`",
    ),
];

/// Load a driver and run the driver-facing sim checks against it.
pub fn run(library: &Library, config: &str) -> Report {
    let mut report = Report::default();

    // ── rhal.abi.version ────────────────────────────────────────────────────
    // SAFETY: the caller loaded this library from a path a person named, and asserts it was built
    // against `bindings/c/skynet_rhal.h`. That assertion is unverifiable in principle, and the ABI
    // version check below is what catches the version-skew case specifically.
    let entry = match unsafe { library.entry() } {
        Ok(entry) => entry,
        Err(err) => {
            report.push(CheckResult::fail(
                "rhal.abi.version",
                "exports skynet_rhal_entry",
                err.to_string(),
            ));
            return report;
        }
    };
    report.push(CheckResult::pass(
        "rhal.abi.version",
        "exports skynet_rhal_entry",
    ));

    // SAFETY: `entry` is a function of the declared signature; calling it runs driver code that is
    // documented to return a pointer to a statically allocated vtable.
    let raw = unsafe { entry() };
    if raw.is_null() {
        report.push(CheckResult::fail(
            "rhal.abi.version",
            "the entry point returns a vtable",
            "skynet_rhal_entry() returned null. It must return a pointer to a statically allocated \
             skr_driver — a null return leaves the OS with nothing to refuse and nothing to load.",
        ));
        return report;
    }
    // SAFETY: non-null, and pointing at a `skr_driver` the driver keeps alive for the life of the
    // process — the header requires the instance be statically allocated.
    let driver: &RawDriver = unsafe { &*raw };

    report.driver_name = cstr(driver.driver_name);
    report.driver_version = cstr(driver.driver_version);

    report.push(CheckResult::assert(
        "rhal.abi.version",
        "declares the ABI it targets",
        driver.abi_version == skynet_rhal::ABI_VERSION,
        format!(
            "declares ABI {} and this OS speaks {}. A driver built against a different major is \
             refused rather than partially loaded; rebuild against this repository's \
             bindings/c/skynet_rhal.h.",
            driver.abi_version,
            skynet_rhal::ABI_VERSION
        ),
    ));

    match skynet_rhal::accepts(driver) {
        Ok(()) => report.push(CheckResult::pass(
            "rhal.abi.version",
            "implements every mandatory entry point",
        )),
        Err(err) => report.push(CheckResult::fail(
            "rhal.abi.version",
            "implements every mandatory entry point",
            err.to_string(),
        )),
    }

    report.push(CheckResult::assert(
        "rhal.abi.version",
        "names itself",
        report
            .driver_name
            .as_deref()
            .is_some_and(|name| !name.is_empty())
            && report
                .driver_version
                .as_deref()
                .is_some_and(|version| !version.is_empty()),
        "driver_name and driver_version are what a black box records and a fleet console groups by. \
         A driver with neither is one nobody can attribute an incident to.",
    ));

    // Nothing below can run without an openable body. Everything not attempted reports Missing.
    let (Some(open), Some(close), Some(read_state), Some(write_command), Some(describe)) = (
        driver.open,
        driver.close,
        driver.read_state,
        driver.write_command,
        driver.describe,
    ) else {
        report.push(CheckResult::new(
            "rhal.state.completeness",
            "the body can be opened and read",
            Outcome::Missing,
            "a mandatory entry point is null, so nothing further could be measured.",
        ));
        return report;
    };

    let config_c = std::ffi::CString::new(config).unwrap_or_default();
    let mut ctx: *mut Ctx = std::ptr::null_mut();
    // SAFETY: `open` came from a vtable that passed `accepts`, `config_c` is NUL-terminated and
    // outlives the call, and `ctx` is a valid out-pointer.
    let rc = unsafe { open(config_c.as_ptr(), &mut ctx) };
    if rc != status::OK || ctx.is_null() {
        report.push(CheckResult::fail(
            "rhal.abi.version",
            "opens",
            format!(
                "open(\"{config}\") returned {rc} and a {} context. Pass a configuration the driver \
                 understands with --config, or fix the driver's handling of this one.",
                if ctx.is_null() { "null" } else { "non-null" }
            ),
        ));
        return report;
    }
    report.push(CheckResult::pass("rhal.abi.version", "opens"));

    descriptor_checks(&mut report, describe, ctx);
    let expected_joints = report
        .descriptor
        .as_ref()
        .and_then(|json| json.get("joints"))
        .and_then(Json::as_arr)
        .map(<[Json]>::len);

    state_checks(&mut report, read_state, ctx, expected_joints);
    // After the state reads, deliberately. The OS's own order is state then sensors, and a driver
    // that acquires a camera frame as part of a cycle has legitimately produced nothing before the
    // first one — asking earlier would fail a driver for the checker's ordering rather than its own.
    sensor_consistency(&mut report, driver, ctx);
    // §89.4, after the single read above, for the same reason that one comes after the state
    // reads: a driver whose first frame is produced by a cycle has produced one by now.
    concurrent_read_checks(&mut report, driver, ctx, expected_joints);
    // Whether the body says it can be commanded at all. Empty is a claim rather than an absence —
    // see `commandable` below.
    let commandable = report
        .descriptor
        .as_ref()
        .and_then(|json| json.get("control"))
        .and_then(|control| control.get("modes"))
        .and_then(Json::as_arr)
        .map(|modes| !modes.is_empty())
        .unwrap_or(true);
    // §86.1. Whether this driver reads the per-joint mode array, which decides whether the two
    // checks below it apply. Absent reads as `false`: a driver written before §86 says nothing here
    // and is not measured against a field it has never heard of.
    let per_joint_modes = report
        .descriptor
        .as_ref()
        .and_then(|json| json.get("control"))
        .and_then(|control| control.get("per_joint_modes"))
        .and_then(Json::as_bool)
        .unwrap_or(false);
    command_checks(
        &mut report,
        write_command,
        ctx,
        expected_joints,
        commandable,
        per_joint_modes,
    );
    motion_checks(&mut report, driver, ctx);
    estop_checks(&mut report, driver, read_state, ctx, expected_joints);

    // SAFETY: `ctx` is the live context from `open`, closed exactly once, and nothing below uses it.
    let rc = unsafe { close(ctx) };
    report.push(CheckResult::assert(
        "rhal.abi.version",
        "closes",
        rc == status::OK,
        format!(
            "close() returned {rc}; it releases what open() acquired and should return SKR_OK."
        ),
    ));

    // §4's stop, as amended (`DRY-COVERAGE-PLAN.md` F76): the latch is the session's. Asked of a
    // second session opened after the first was stopped and closed — which this suite never did
    // until 2026-09-19, and which is how a CANopen driver that appended a second session's axes to
    // the first's (F73) and two drivers whose latch outlived its session (F76) passed it.
    let rated_stop = report
        .descriptor
        .as_ref()
        .and_then(|json| json.get("envelope"))
        .and_then(|envelope| envelope.get("estop"))
        .and_then(Json::as_bool)
        .unwrap_or(false);
    second_session_checks(
        &mut report,
        SecondSession {
            open,
            close,
            read_state,
            write_command,
            config: &config_c,
            expected_joints,
            commandable,
            rated_stop,
        },
    );

    report
}

/// What [`second_session_checks`] needs: the lifecycle entry points, the configuration the first
/// session was opened with, and three facts the first session established.
struct SecondSession<'a> {
    open: unsafe extern "C" fn(*const c_char, *mut *mut Ctx) -> i32,
    close: unsafe extern "C" fn(*mut Ctx) -> i32,
    read_state: unsafe extern "C" fn(*mut Ctx, *mut State, u64) -> i32,
    write_command: unsafe extern "C" fn(*mut Ctx, *const Command, u64) -> i32,
    config: &'a std::ffi::CStr,
    /// The descriptor's joint count, which the second session must report too.
    expected_joints: Option<usize>,
    /// Whether the body declares any control mode, as `command_checks` read it.
    commandable: bool,
    /// Whether the descriptor's envelope declares a rated stop fitted (`envelope.estop`).
    rated_stop: bool,
}

/// A second session, after the first was stopped and closed (§4 as amended — F76).
///
/// **The software stop's latch is the session's.** `open` begins a session, and a session begins
/// with no stop asserted by an earlier one. How long a *robot* stays stopped is not the driver's to
/// decide and is not decided here: it is §27's `SAFE`, which a human leaves, and a driver cannot see
/// that human. What a driver can do wrong is the opposite pair, and each has been done in this
/// tree: keep the latch, so a body refuses every command of a session nothing stopped (F76's Franka
/// and UR drivers), or keep the *session*, so a second `open` describes a body with twice the joints
/// (F73's CANopen driver).
///
/// A body whose envelope declares a **rated** stop is the exception, and the reason is F26's: a
/// hardware latch is evidence about why a robot stopped, it is reported in the state's flags, and
/// `open` must never clear it. So on such a body a second session reporting a stop is honest, and
/// only the joint count is asked of it.
fn second_session_checks(report: &mut Report, session: SecondSession<'_>) {
    let SecondSession {
        open,
        close,
        read_state,
        write_command,
        config,
        expected_joints,
        commandable,
        rated_stop,
    } = session;

    let mut ctx: *mut Ctx = std::ptr::null_mut();
    // SAFETY: as the first `open` — the same entry point, the same NUL-terminated configuration,
    // and a valid out-pointer. The first session's context was closed above and is not used again.
    let rc = unsafe { open(config.as_ptr(), &mut ctx) };
    if rc != status::OK || ctx.is_null() {
        report.push(CheckResult::fail(
            "rhal.abi.version",
            "opens a second session after the first was stopped and closed",
            format!(
                "open() returned {rc} and a {} context the second time. A driver that can be opened \
                 once per process is a robot that cannot be restarted without restarting the OS \
                 around it.",
                if ctx.is_null() { "null" } else { "non-null" }
            ),
        ));
        return;
    }
    report.push(CheckResult::pass(
        "rhal.abi.version",
        "opens a second session after the first was stopped and closed",
    ));

    if let Some(joints) = expected_joints {
        let mut buffer = StateBuffer::new(joints.max(1) * 4);
        let mut state = buffer.state();
        // SAFETY: `ctx` is the live second context; the buffers are four times the declared joint
        // count, so a driver that reports more joints than it declares still writes within them.
        let rc = unsafe { read_state(ctx, &mut state, 0) };
        report.push(CheckResult::assert(
            "rhal.state.completeness",
            "the second session reports the body the descriptor declares",
            rc == status::OK && state.joint_count as usize == joints,
            format!(
                "read_state() in the second session returned {rc} with {} joints, and the \
                 descriptor declares {joints}. A session that carries the last one's state over is \
                 a body that grows every time it is opened.",
                state.joint_count
            ),
        ));

        if rated_stop {
            report.push(CheckResult::new(
                "safety.estop.latency",
                "the first session's stop does not outlive it",
                Outcome::NotApplicable,
                "the envelope declares a rated stop, and a latch the hardware keeps is reported \
                 rather than cleared by open() (F26), so a stop in the second session is honest \
                 here."
                    .to_string(),
            ));
        } else {
            report.push(CheckResult::assert(
                "safety.estop.latency",
                "the first session's stop does not outlive it",
                rc == status::OK && state.flags & flags::ESTOP == 0,
                "the second session's first state still sets SKR_FLAG_ESTOP, and this body declares \
                 no rated stop, so the latch is the driver's own and outlived the session that \
                 set it. How long a robot stays stopped is §27's SAFE, which a human leaves; a \
                 driver keeping a software latch across open() refuses every command of a session \
                 nothing stopped. If the body's own stop circuit is open, close it and run again.",
            ));
        }

        // Under `rhal.command.rejection`, beside the first session's "accepts a well-formed
        // command" and for the same reason: it is that assertion, in the session after a stop.
        // Filed there rather than under the stop's own id because a stop's id is a bench test and
        // does not move the award (§15) — and a driver that refuses every command of a session
        // nothing stopped is not a guest-tier driver, whatever its stop measures on a bench.
        if rated_stop {
            report.push(CheckResult::new(
                "rhal.command.rejection",
                "the second session accepts a well-formed command",
                Outcome::NotApplicable,
                "the envelope declares a rated stop, and a body whose hardware still holds the first session's stop refuses a command honestly (F26)."
                    .to_string(),
            ));
        } else if !commandable {
            report.push(CheckResult::new(
                "rhal.command.rejection",
                "the second session accepts a well-formed command",
                Outcome::NotApplicable,
                "this body declares no control modes, so no joint command is accepted in any session (§59.2)."
                    .to_string(),
            ));
        } else {
            let values = vec![0.0f32; joints];
            let mask = vec![0u8; joints];
            let well_formed = Command {
                stamp_ns: 1,
                seq: 1,
                mode: Mode::Position as u8,
                flags: 0,
                _pad: [0; 2],
                joint_count: joints as u16,
                values: values.as_ptr(),
                mask: mask.as_ptr(),
                modes: core::ptr::null(),
            };
            // SAFETY: `values` and `mask` are `joints` long and outlive the call.
            let rc = unsafe { write_command(ctx, &well_formed, 0) };
            report.push(CheckResult::assert(
                "rhal.command.rejection",
                "the second session accepts a well-formed command",
                rc == status::OK,
                format!(
                    "a position command over {joints} held joints returned {rc} in the second session, and the same command is accepted in the first. A refusal here is the first session's stop, still latched: §4 makes the latch the session's (F76)."
                ),
            ));
        }
    }

    // SAFETY: the live second context, closed exactly once.
    let rc = unsafe { close(ctx) };
    report.push(CheckResult::assert(
        "rhal.abi.version",
        "closes the second session",
        rc == status::OK,
        format!("close() returned {rc} the second time."),
    ));
}

fn descriptor_checks(
    report: &mut Report,
    describe: unsafe extern "C" fn(*mut Ctx, *mut c_char, usize) -> i32,
    ctx: *mut Ctx,
) {
    // The retry protocol from §4: a buffer that is too small is an error, never a truncation, and
    // the OS retries with a larger one. Checking the refusal first is the point — a driver that
    // truncates produces a descriptor that parses and is wrong.
    let mut tiny = [0i8; 8];
    // SAFETY: `tiny` is a valid writable buffer of the length passed.
    let rc = unsafe { describe(ctx, tiny.as_mut_ptr() as *mut c_char, tiny.len()) };
    report.push(CheckResult::assert(
        "embodiment.descriptor.valid",
        "refuses a buffer that is too small rather than truncating",
        rc == status::EINVAL,
        format!(
            "describe() with an 8-byte buffer returned {rc}, and SKR_EINVAL ({}) was expected. Half \
             a parsed body description is worse than a failed one.",
            status::EINVAL
        ),
    ));

    let mut size = 4096usize;
    let json = loop {
        let mut buffer = vec![0i8; size];
        // SAFETY: `buffer` is a valid writable buffer of exactly `size` bytes.
        let written = unsafe { describe(ctx, buffer.as_mut_ptr() as *mut c_char, size) };
        if written == status::EINVAL && size < 1 << 20 {
            size *= 4;
            continue;
        }
        if written <= 0 {
            report.push(CheckResult::fail(
                "embodiment.descriptor.valid",
                "returns a descriptor",
                format!(
                    "describe() returned {written} with a {size}-byte buffer. It returns the number \
                     of bytes written, or SKR_EINVAL when the buffer is too small."
                ),
            ));
            return;
        }
        let bytes: Vec<u8> = buffer[..written as usize]
            .iter()
            .map(|b| *b as u8)
            .collect();
        match String::from_utf8(bytes) {
            Ok(text) => break text,
            Err(_) => {
                report.push(CheckResult::fail(
                    "embodiment.descriptor.valid",
                    "returns UTF-8",
                    "describe() wrote bytes that are not UTF-8. The descriptor is JSON, and JSON is \
                     UTF-8.",
                ));
                return;
            }
        }
    };

    let parsed = match Json::parse(&json) {
        Ok(parsed) => parsed,
        Err(err) => {
            report.push(CheckResult::fail(
                "embodiment.descriptor.valid",
                "returns valid JSON",
                format!(
                    "describe() wrote {} bytes and they do not parse: {err}",
                    json.len()
                ),
            ));
            return;
        }
    };

    match descriptor_from_json(&parsed) {
        Ok(descriptor) => {
            report.push(CheckResult::pass(
                "embodiment.descriptor.valid",
                "the descriptor decodes",
            ));
            report.push(CheckResult::assert(
                "embodiment.descriptor.valid",
                "names the body it describes",
                !descriptor.robot_slug.is_empty(),
                "robot_slug is empty. The descriptor is how a body is identified everywhere else.",
            ));
            // A program-level body may report no joints and still be commanded (§84.1), so what
            // this asks is whether *anything* is declared a policy could be bound to.
            report.push(CheckResult::assert(
                "embodiment.descriptor.valid",
                "declares at least one joint or motion",
                !descriptor.joints.is_empty() || !descriptor.motions.is_empty(),
                "joints[] and motions[] are both empty, so there is nothing for a policy to be \
                 bound to.",
            ));

            // §84.1's motion list, which is part of the schema this test reads. A body that
            // declares none is not commanded that way, and that is a property of the body rather
            // than a gap in the evidence — §15's own definition of not_applicable.
            report.push(if descriptor.motions.is_empty() {
                CheckResult::new(
                    "embodiment.descriptor.valid",
                    "the motion list, where there is one",
                    Outcome::NotApplicable,
                    "no motions are declared: this body is not commanded by named motions (§84.1).",
                )
            } else {
                match validate_motions(&descriptor.motions) {
                    None => CheckResult::new(
                        "embodiment.descriptor.valid",
                        "the motion list, where there is one",
                        Outcome::Pass,
                        format!(
                            "{} motion(s), {} of them canonical.",
                            descriptor.motions.len(),
                            descriptor
                                .motions
                                .iter()
                                .filter(|m| canonical_motion(m.motion.as_deref()).is_some())
                                .count()
                        ),
                    ),
                    Some(fault) => CheckResult::fail(
                        "embodiment.descriptor.valid",
                        "the motion list, where there is one",
                        format!(
                            "{} ({}): {} A command naming a motion is checked against this list \
                             at install time, and a malformed one would be checked against \
                             something that does not say what the controller takes.",
                            fault.code,
                            fault.subjects.join(", "),
                            fault.detail
                        ),
                    ),
                }
            });

            // §87.3. What this body's drives do when commands stop, however it declared it — one
            // value for the body, or one per §5.1 chain. `resolve_command_loss` is the one place
            // that answers *"may this body attempt host"* for both, which is why this reads its
            // report rather than the fields: §78 is the record of what two spellings of one fact
            // cost, and *"host is refused"* is exactly the kind that acquires a second one when a
            // field gains a per-something variant.
            //
            // Coverage is measured against the chains this body's **joints** are on rather than
            // against the map's own keys, or a map naming chains the body does not have would
            // measure itself complete.
            let chains = chains_of(descriptor.joints.iter().map(|joint| joint.role.as_deref()));
            let loss = resolve_command_loss(&CommandLossParams {
                body: descriptor.on_command_loss.as_deref(),
                by_chain: descriptor.on_command_loss_by_chain.clone(),
                chains,
            });
            // A **malformed** declaration is a failure and a *refusal* is not, and telling them
            // apart is the whole of what this check decides. §46.2's silence and a declared `coast`
            // are both correct descriptors of real machines — they refuse the host tier and leave
            // guest untouched, which is §87.5's own sentence about what this section does not
            // change. What fails is a declaration that cannot be read: both fields at once, a map
            // over a body with no chains, a chain the map does not cover, or a word §46.2 has no
            // meaning for.
            let known = |behaviour: &Option<String>| {
                behaviour
                    .as_deref()
                    .is_some_and(|value| COMMAND_LOSS_BEHAVIOURS.contains(&value))
            };
            let malformed = if loss.per_chain {
                loss.body.is_some()
                    || loss.chains.is_empty()
                    || loss.chains.iter().any(|(_, behaviour)| !known(behaviour))
            } else {
                loss.body.is_some() && !known(&loss.body)
            };
            report.push(match (&loss.host_blocked, malformed) {
                (_, true) => CheckResult::fail(
                    "embodiment.descriptor.valid",
                    "what the drives do when commands stop",
                    format!(
                        "{} This one cannot be read at all, which is a different thing from a body \
                         that declares something host does not allow.",
                        loss.host_blocked.as_deref().unwrap_or_default()
                    ),
                ),
                (None, false) => CheckResult::new(
                    "embodiment.descriptor.valid",
                    "what the drives do when commands stop",
                    Outcome::Pass,
                    if loss.per_chain {
                        format!(
                            "declared per chain, and every one of this body's {} chain(s) is \
                             covered: {}.{}",
                            loss.chains.len(),
                            loss.chains
                                .iter()
                                .map(|(chain, behaviour)| format!(
                                    "{chain} {}",
                                    behaviour.as_deref().unwrap_or("—")
                                ))
                                .collect::<Vec<String>>()
                                .join(", "),
                            if loss.unknown_chains.is_empty() {
                                String::new()
                            } else {
                                format!(
                                    " It also names {}, which this body does not have — \
                                     over-declaration rather than a fault, and the way a descriptor \
                                     copied between two robots shows it (§87.3).",
                                    loss.unknown_chains.join(", ")
                                )
                            }
                        )
                    } else {
                        format!(
                            "`on_command_loss: {}` for the whole body (§46.2).",
                            loss.body.as_deref().unwrap_or("—")
                        )
                    },
                ),
                // Sound, and host is refused on it. A **pass**, because the question this test asks
                // is whether the descriptor says what it means: §87.5 moves two robot classes from
                // "not designed" to "refused for a named reason", and a checker that scored the
                // second as a defect would be asking a guest-tier body to make a host-tier claim.
                (Some(blocked), false) => CheckResult::new(
                    "embodiment.descriptor.valid",
                    "what the drives do when commands stop",
                    Outcome::Pass,
                    format!("{blocked} Guest is unaffected; host is what this refuses."),
                ),
            });

            // §85. A body's tokens are what admission compares a package's against, so a token that
            // is not one — a typo, a vendor word filed as canonical — is a capability no package can
            // ever match, and the refusal it causes names the package rather than this file.
            let misfiled: Vec<String> = descriptor
                .capabilities
                .iter()
                .filter(|token| classify_token(token) != TokenClass::Canonical)
                .map(|token| format!("{token} ({})", classify_token(token).as_str()))
                .chain(
                    descriptor
                        .extensions
                        .iter()
                        .filter(|token| classify_token(token) != TokenClass::Vendor)
                        .map(|token| {
                            format!(
                                "{token} (an extension, and {})",
                                classify_token(token).as_str()
                            )
                        }),
                )
                .collect();
            report.push(CheckResult::assert(
                "embodiment.descriptor.valid",
                "capability tokens are §85's, and extensions are a vendor's",
                misfiled.is_empty(),
                format!(
                    "{}. A capability must be one §85 lists; a vendor's own token belongs under \
                     `extensions`, and a canonical one under `capabilities`.",
                    misfiled.join(", ")
                ),
            ));

            report.simulated = descriptor.simulator;

            // §44.3's frame graph, checked against the shape the specification pins rather than
            // against whichever generator wrote this file. Absent is not a failure — most bodies
            // have no graph yet, and §44.5 already says what that means: the pose guards report
            // unavailable with the frame they wanted named.
            //
            // Filed under `embodiment.frames.complete` rather than `embodiment.descriptor.valid`,
            // which is what §52 names it and is also the more honest tier. A guest-tier body with
            // no graph is ordinary; a *malformed* graph is a real defect, but one whose consequence
            // §44.5 already bounds — the two pose guards go unavailable and the other six still
            // run. Blocking guest on it would refuse a body that is safe by the same argument the
            // rest of the design is safe by.
            match &descriptor.frames {
                None => report.push(CheckResult::new(
                    "embodiment.frames.complete",
                    "the frame graph, where there is one",
                    Outcome::NotApplicable,
                    "no frame graph is declared. The workspace and self-collision guards will \
                     report unavailable rather than run against a tree they guessed (§44.5).",
                )),
                Some(graph) => match validate_frames(graph) {
                    None => report.push(CheckResult::new(
                        "embodiment.frames.complete",
                        "the frame graph, where there is one",
                        Outcome::Pass,
                        format!(
                            "{} frames, rooted at `{}`, {}.",
                            graph.nodes.len(),
                            graph.root,
                            graph.convention
                        ),
                    )),
                    Some(fault) => report.push(CheckResult::fail(
                        "embodiment.frames.complete",
                        "the frame graph, where there is one",
                        format!("{} [{}]", fault.detail, fault.code),
                    )),
                },
            }
            // The half a graph-only validator cannot perform, and the reason this test needs
            // the descriptor rather than the tree: `joint` and `mimics` name entries in the joint
            // list, and a name that does not resolve there leaves the frame unresolvable on every
            // cycle — which both pose guards correctly trip on, forever, for a reason that is a
            // spelling in a file. §44.5 wants that reported when the lease is granted rather than
            // in cycle, and this is the only place it can be.
            //
            // A driver is exactly where it goes wrong, because the graph and the joint list are
            // commonly produced by two different code paths in it.
            if let Some(graph) = &descriptor.frames {
                let joints: Vec<&str> = descriptor.joints.iter().map(|j| j.name.as_str()).collect();
                let driven = graph
                    .nodes
                    .iter()
                    .filter(|node| node.joint.is_some())
                    .count();
                report.push(match validate_frame_bindings(graph, &joints) {
                    None => CheckResult::new(
                        "embodiment.frames.complete",
                        "every joint the frame graph names is one the body declares",
                        Outcome::Pass,
                        format!(
                            "{driven} of {} frames move with a declared joint.",
                            graph.nodes.len()
                        ),
                    ),
                    Some(fault) => CheckResult::fail(
                        "embodiment.frames.complete",
                        "every joint the frame graph names is one the body declares",
                        format!("{} [{}]", fault.detail, fault.code),
                    ),
                });
            }

            // "Valid" and "usable" are different claims, and the second is the one a driver author
            // needs. A well-formed tree with one collision capsule has nothing for
            // `self_collision` to test; a tree whose limbs hang off a ball joint has nothing for
            // `workspace` to constrain. Both are §7.2's unavailability.
            //
            // Its own result rather than detail on the one above, because the report prints detail
            // only for a result that is not a plain pass — and "the guard cannot run" is exactly
            // the sentence that must not be swallowed.
            if let Some(graph) = &descriptor.frames {
                if let Some((usable, summary)) = pose_guard_summary(graph) {
                    report.push(CheckResult::new(
                        "embodiment.frames.complete",
                        "what the pose guards can do with it",
                        if usable {
                            Outcome::Pass
                        } else {
                            Outcome::NotApplicable
                        },
                        summary,
                    ));
                }
            }

            report.push(CheckResult::assert(
                "embodiment.descriptor.valid",
                "dof agrees with the joint list",
                (descriptor.dof - descriptor.joints.len() as f64).abs() < 0.5,
                format!(
                    "declares dof={} with {} joints. They may legitimately differ when an end \
                     effector carries its own degrees of freedom; check which is right.",
                    descriptor.dof,
                    descriptor.joints.len()
                ),
            ));

            // §15's rule about limits, and the one this whole project turns on: an envelope with no
            // provenance is a rumour, and a driver inventing one is worse than a driver with none.
            match &descriptor.envelope {
                None => report.push(CheckResult::new(
                    "embodiment.limits.sourced",
                    "declares no unsourced safety envelope",
                    Outcome::Pass,
                    "no envelope is declared, which is the honest state until a sourced figure \
                     exists. Guests run; the guards that need one report unavailable.",
                )),
                Some(envelope) => report.push(CheckResult::assert(
                    "embodiment.limits.sourced",
                    "the declared envelope names its source",
                    envelope.source.as_deref().is_some_and(|s| !s.is_empty()),
                    "the envelope declares limits and no `source`. A safety limit with no \
                     provenance is a rumour; name the datasheet, standard or risk assessment it \
                     came from, or declare no envelope at all.",
                )),
            }

            let with_roles = descriptor
                .joints
                .iter()
                .filter(|joint| parse_role(joint.role.as_deref()).is_some())
                .count();
            let coverage = if descriptor.joints.is_empty() {
                0.0
            } else {
                100.0 * with_roles as f64 / descriptor.joints.len() as f64
            };
            report.push(
                CheckResult::new(
                    "embodiment.roles.coverage",
                    "joints carry a parseable canonical role",
                    if with_roles == descriptor.joints.len() {
                        Outcome::Pass
                    } else {
                        Outcome::Fail
                    },
                    format!(
                    "{coverage:.0}% ({with_roles} of {}). A joint with no parseable role can only \
                     ever be matched by exact name, so it cannot participate in cross-body \
                     remapping — which is truthful, and is also the ceiling on what this body can \
                     run.",
                        descriptor.joints.len()
                    ),
                )
                .with_value(coverage),
            );

            report.push(CheckResult::new(
                "embodiment.descriptor.signed",
                "the descriptor is digested and signed",
                if descriptor.verified && descriptor.digest.is_some() {
                    Outcome::Pass
                } else {
                    Outcome::Missing
                },
                "digest or verified is absent. A driver cannot sign its own descriptor — the \
                 signature is made with the vendor's namespace key when the descriptor is \
                 published — so this is expected locally and is required from the host tier up.",
            ));

            report.descriptor = Some(parsed);
        }
        Err(err) => report.push(CheckResult::fail(
            "embodiment.descriptor.valid",
            "the descriptor decodes",
            format!("{err:?}. It needs at least `robot_slug`, `base` and `dof`."),
        )),
    }
}

/// The descriptor and the vtable have to agree about sensors.
///
/// `read_sensor` is optional, and null is a perfectly good answer — cameras are commonly read
/// straight from V4L2 or GenICam by the runtime rather than through the RHAL. What is not a good
/// answer is a descriptor advertising sensors the vtable has no way to deliver: a policy declaring
/// an observation requirement would bind against a stream that cannot be read, and the failure
/// would arrive at run time on a robot rather than here.
fn sensor_consistency(report: &mut Report, driver: &RawDriver, ctx: *mut Ctx) {
    let names: Vec<String> = report
        .descriptor
        .as_ref()
        .and_then(|json| json.get("sensors"))
        .and_then(Json::as_arr)
        .map(|sensors| {
            sensors
                .iter()
                .filter_map(|sensor| sensor.get("name").and_then(Json::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let declared = names.len();

    match (declared, driver.read_sensor.is_some()) {
        (0, false) => report.push(CheckResult::new(
            "embodiment.descriptor.valid",
            "sensors in the descriptor are readable through the RHAL",
            Outcome::NotApplicable,
            "the body declares no sensors and the driver implements no read_sensor, which agree.",
        )),
        (0, true) => report.push(CheckResult::pass(
            "embodiment.descriptor.valid",
            "sensors in the descriptor are readable through the RHAL",
        )),
        (_, true) => {
            // Not just "the entry point exists" — read one. A declared stream whose read path is
            // wired up and broken is indistinguishable from one that works until a policy asks for
            // it, and by then it is on a robot.
            report.push(read_one_sensor(driver, ctx, &names[0]));
        }
        (count, false) => report.push(CheckResult::fail(
            "embodiment.descriptor.valid",
            "sensors in the descriptor are readable through the RHAL",
            format!(
                "the descriptor declares {count} sensors and read_sensor is null, so nothing can \
                 read them. Either implement read_sensor, or remove the sensors the runtime reads \
                 by another route — a declared stream that cannot be read binds a policy's \
                 observation to nothing."
            ),
        )),
    }
}

/// Read one declared sensor and check the frame it produced describes itself consistently.
///
/// The interesting assertion is `stride * height <= size`: a frame whose declared geometry does not
/// fit in the bytes behind it is a buffer overrun waiting for the first consumer that trusts the
/// header, and it is the single most common defect in a hand-written capture path.
fn read_one_sensor(driver: &RawDriver, ctx: *mut Ctx, name: &str) -> CheckResult {
    const ID: &str = "embodiment.descriptor.valid";
    const WHAT: &str = "sensors in the descriptor are readable through the RHAL";

    let Some(read_sensor) = driver.read_sensor else {
        return CheckResult::new(ID, WHAT, Outcome::Missing, "read_sensor went away");
    };
    let Ok(c_name) = std::ffi::CString::new(name) else {
        return CheckResult::fail(
            ID,
            WHAT,
            format!("the descriptor names a sensor `{name}` containing a NUL byte."),
        );
    };

    let mut frame = Frame {
        stamp_ns: 0,
        width: 0,
        height: 0,
        stride: 0,
        format: 0,
        data: std::ptr::null(),
        size: 0,
    };
    // SAFETY: `read_sensor` came from a vtable that passed `accepts`, `c_name` is NUL-terminated and
    // outlives the call, and `frame` is a valid out-pointer the driver fills and does not retain.
    let rc = unsafe { read_sensor(ctx, c_name.as_ptr(), &mut frame) };

    if rc == status::ENOSYS || rc == status::ENODEV {
        return CheckResult::fail(
            ID,
            WHAT,
            format!(
                "the descriptor declares a sensor `{name}` and read_sensor answered {rc} for it. \
                 The descriptor is the source of truth for the body, so either the sensor is not \
                 really there or the read path does not know its name."
            ),
        );
    }
    if rc != status::OK {
        return CheckResult::fail(
            ID,
            WHAT,
            format!(
                "read_sensor(\"{name}\") returned {rc}. A declared stream that cannot be read binds \
                 a policy's observation to nothing."
            ),
        );
    }
    if frame.data.is_null() || frame.size == 0 {
        return CheckResult::fail(
            ID,
            WHAT,
            format!("read_sensor(\"{name}\") succeeded and returned an empty frame."),
        );
    }
    let needed = (frame.stride as usize).saturating_mul(frame.height as usize);
    if needed > frame.size {
        return CheckResult::fail(
            ID,
            WHAT,
            format!(
                "sensor `{name}` declares {}x{} at stride {} — {needed} bytes — and handed back {}. \
                 A consumer that trusts the header reads past the end of the buffer.",
                frame.width, frame.height, frame.stride, frame.size
            ),
        );
    }
    if frame.stamp_ns == 0 {
        return CheckResult::fail(
            ID,
            WHAT,
            format!(
                "sensor `{name}` produced a frame with no timestamp. Observation assembly pairs \
                 frames with joint state by time (§25.1), and an unstamped frame cannot be paired."
            ),
        );
    }
    CheckResult::pass(ID, WHAT)
}

/// How long §89.4 reads a declared sensor concurrently with the body, per sensor.
///
/// A bound rather than a duration somebody tuned: long enough that a driver sharing one scratch
/// buffer between its two paths is caught on nearly every run, short enough that a vendor checking
/// six sensors waits under two seconds. It is stated on the terminal, in the clause the check
/// reports, because §89.4's own sentence is that **a race it does not provoke is not found**.
const CONCURRENT_READ_MS: u64 = 250;

/// A driver context crossing a thread boundary, which is the whole of what §89.1 declares.
///
/// The pointer is not `Send` and must not become `Send` in general: for every sensor that does not
/// declare `concurrent_read`, handing it to a second thread is the data race §89.3 describes. This
/// wrapper exists so that the one place the platform is *entitled* to do it is a place with a name,
/// and the entitlement is the driver's declaration rather than this type's existence.
struct DeclaredCtx(*mut Ctx);

// SAFETY: constructed only from a sensor whose descriptor declares `concurrent_read: true`, which
// §89.1 defines as the driver's promise that `read_sensor` for that sensor may be called from
// another thread while `read_state` and `write_command` run. The thread that receives it calls
// `read_sensor` and nothing else, and is joined before the context is closed — which is the
// runtime's half of §89.1's bargain, made here so the check exercises the same arrangement the
// loop provides rather than a stricter or looser one.
unsafe impl Send for DeclaredCtx {}

/// §89.4 — a `concurrent_read` declaration, exercised rather than trusted.
///
/// For each sensor declaring it, `read_sensor` runs on a second thread while this one calls
/// `read_state`, for [`CONCURRENT_READ_MS`]. The declaration fails if either side returns anything
/// but success, if a frame's geometry does not fit its bytes, or if the state stops advancing.
///
/// **A pass is bounded evidence and the clause says so.** §89.3 is explicit that the price of a
/// false declaration is a data race inside a driver, in a robot's control path, with no error
/// anywhere — so what this can do is provoke one, and what it cannot do is prove there is none.
/// That is why §89.1 makes the declaration a promise with a named owner in the first place.
fn concurrent_read_checks(
    report: &mut Report,
    driver: &RawDriver,
    ctx: *mut Ctx,
    expected_joints: Option<usize>,
) {
    const ID: &str = "embodiment.descriptor.valid";

    let declared: Vec<String> = report
        .descriptor
        .as_ref()
        .and_then(|json| json.get("sensors"))
        .and_then(Json::as_arr)
        .map(|sensors| {
            sensors
                .iter()
                // §89.1: exactly `true` declares it. `as_bool` is the same reading the contract
                // and every edition give the field, so a string "true" is undeclared here as well.
                .filter(|sensor| {
                    sensor
                        .get("concurrent_read")
                        .and_then(Json::as_bool)
                        .unwrap_or(false)
                })
                .filter_map(|sensor| sensor.get("name").and_then(Json::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    if declared.is_empty() {
        // Not a gap in the evidence: a body that declares nothing is read on the control thread,
        // which is §89.2 and is measured by the loop rather than here.
        report.push(CheckResult::new(
            ID,
            "no sensor declares concurrent_read (§89.1)",
            Outcome::NotApplicable,
            String::new(),
        ));
        return;
    }

    let (Some(read_sensor), Some(read_state)) = (driver.read_sensor, driver.read_state) else {
        report.push(CheckResult::fail(
            ID,
            "a concurrent_read declaration is exercised (§89.4)",
            format!(
                "{} sensor(s) declare `concurrent_read` and the vtable has no read_sensor to call \
                 concurrently. The declaration promises something about a function that is not \
                 there.",
                declared.len()
            ),
        ));
        return;
    };

    let joints = expected_joints.unwrap_or(1).max(1);
    for name in &declared {
        report.push(one_concurrent_read(
            read_sensor,
            read_state,
            ctx,
            name,
            joints,
        ));
    }
}

/// One declared sensor, read on a second thread for [`CONCURRENT_READ_MS`] while this one reads
/// the body.
fn one_concurrent_read(
    read_sensor: unsafe extern "C" fn(*mut Ctx, *const c_char, *mut Frame) -> i32,
    read_state: unsafe extern "C" fn(*mut Ctx, *mut State, u64) -> i32,
    ctx: *mut Ctx,
    name: &str,
    joints: usize,
) -> CheckResult {
    const ID: &str = "embodiment.descriptor.valid";
    let what = format!("sensor `{name}` reads concurrently for {CONCURRENT_READ_MS} ms (§89.4)");

    let Ok(c_name) = std::ffi::CString::new(name) else {
        return CheckResult::fail(
            ID,
            what,
            format!("the descriptor names a sensor `{name}` containing a NUL byte."),
        );
    };

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(CONCURRENT_READ_MS);
    let carried = DeclaredCtx(ctx);
    let sensor = std::thread::spawn(move || {
        // Moved rather than borrowed: the pointer is what crosses, and naming it here is what makes
        // the `unsafe impl Send` above answerable for exactly one thing.
        let carried = carried;
        let mut reads = 0u64;
        let mut refusal = None;
        while std::time::Instant::now() < deadline {
            let mut frame = Frame {
                stamp_ns: 0,
                width: 0,
                height: 0,
                stride: 0,
                format: 0,
                data: std::ptr::null(),
                size: 0,
            };
            // SAFETY: `read_sensor` came from a vtable that passed `accepts`, `c_name` outlives the
            // call, `frame` is a valid out-pointer, and the sensor named declares §89.1's promise
            // that this may run while the body is read.
            let rc = unsafe { read_sensor(carried.0, c_name.as_ptr(), &mut frame) };
            reads += 1;
            if rc != status::OK {
                refusal = Some(format!("read_sensor returned {rc} on read {reads}"));
                break;
            }
            let needed = (frame.stride as usize).saturating_mul(frame.height as usize);
            if frame.data.is_null() || frame.size == 0 {
                refusal = Some(format!("read {reads} came back empty"));
                break;
            }
            if needed > frame.size {
                refusal = Some(format!(
                    "read {reads} declares {}x{} at stride {} — {needed} bytes — in a buffer of {}",
                    frame.width, frame.height, frame.stride, frame.size
                ));
                break;
            }
        }
        (reads, refusal)
    });

    // Deliberately oversized, exactly as `state_checks` does it: a driver that writes more joints
    // than it declares is a mismatch to report and not a buffer overrun to suffer.
    let mut buffer = StateBuffer::new(joints * 4);
    let mut reads = 0u64;
    let mut body = None;
    let mut first_seq = None;
    let mut last_seq = 0u32;
    while std::time::Instant::now() < deadline {
        let mut state = buffer.state();
        // SAFETY: `read_state` came from a vtable that passed `accepts`, and `state` points at
        // arrays of four times the declared joint count.
        let rc = unsafe { read_state(ctx, &mut state, 0) };
        reads += 1;
        if rc != status::OK {
            body = Some(format!("read_state returned {rc} on read {reads}"));
            break;
        }
        if first_seq.is_none() {
            first_seq = Some(state.seq);
        } else if state.seq < last_seq {
            body = Some(format!(
                "the sequence went backwards on read {reads}: {last_seq} then {}",
                state.seq
            ));
            break;
        }
        last_seq = state.seq;
    }

    let (sensor_reads, sensor_refusal) =
        match sensor.join() {
            Ok(outcome) => outcome,
            // A panic here is this checker's, not the driver's, and saying so is the difference between
            // a vendor fixing their driver and a vendor fixing ours.
            Err(_) => return CheckResult::new(
                ID,
                what,
                Outcome::Missing,
                "the thread reading the sensor panicked, which is a defect in this checker rather \
                 than a measurement of the driver."
                    .to_string(),
            ),
        };

    if let Some(detail) = sensor_refusal {
        return CheckResult::fail(
            ID,
            what,
            format!(
                "the sensor declares `concurrent_read`, and reading it on a second thread while \
                 the body was read failed: {detail}. §89.1's declaration is a promise that this \
                 works; remove it, or make the sensor's read path independent of the body's."
            ),
        );
    }
    if let Some(detail) = body {
        return CheckResult::fail(
            ID,
            what,
            format!(
                "the sensor declares `concurrent_read`, and reading the body while it was read on \
                 a second thread failed: {detail}. That is the data race §89.3 names — in a \
                 robot it is in the control path, with no error anywhere."
            ),
        );
    }
    // Neither side failed, so a sensor nobody could read and a body that never advanced are the
    // two remaining ways for this to have measured nothing.
    if sensor_reads == 0 || reads == 0 {
        return CheckResult::new(
            ID,
            what,
            Outcome::Missing,
            format!(
                "the interval elapsed with {sensor_reads} sensor read(s) and {reads} state \
                 read(s), so nothing ran concurrently with anything."
            ),
        );
    }
    match first_seq {
        Some(first) if last_seq > first => CheckResult::pass(ID, what),
        Some(first) => CheckResult::fail(
            ID,
            what,
            format!(
                "the body's sequence stood still at {first} for {reads} reads while the sensor was \
                 read on another thread. §24's sequence advances per sample, so a driver whose \
                 state stops moving under a concurrent read is one whose read paths are not \
                 independent — which is what the declaration says they are."
            ),
        ),
        None => CheckResult::new(
            ID,
            what,
            Outcome::Missing,
            "no state was read during the interval.".to_string(),
        ),
    }
}

/// A caller-allocated state buffer, reused across cycles exactly as the OS does.
struct StateBuffer {
    position: Vec<f32>,
    velocity: Vec<f32>,
    effort: Vec<f32>,
}

impl StateBuffer {
    fn new(joints: usize) -> Self {
        Self {
            position: vec![f32::NAN; joints],
            velocity: vec![f32::NAN; joints],
            effort: vec![f32::NAN; joints],
        }
    }

    /// One `skr_state` over those buffers, filled the way the **runtime** fills it.
    ///
    /// `joint_count` on the way in is the caller's capacity — `skynet_dl::Driver::read_state` sets
    /// it to `position.len()` on every cycle, and `skr_state`'s own comment annotates each array
    /// `[joint_count]`, so a driver bounding its writes by the number it was handed is reading the
    /// ABI correctly. This command passed **zero** for as long as it existed (F45), which is the
    /// same call with a different meaning: a driver that honours it writes no joints, and one that
    /// validates it refuses the read outright and fails a conformance run it would pass on a robot.
    fn state(&mut self) -> State {
        State {
            stamp_ns: 0,
            seq: 0,
            joint_count: self.position.len() as u16,
            flags: 0,
            position_rad: self.position.as_mut_ptr(),
            velocity_rad_s: self.velocity.as_mut_ptr(),
            effort_nm: self.effort.as_mut_ptr(),
            base_pose: [f32::NAN; 7],
            base_twist: [0.0; 6],
            fault_code: 0,
        }
    }
}

fn state_checks(
    report: &mut Report,
    read_state: unsafe extern "C" fn(*mut Ctx, *mut State, u64) -> i32,
    ctx: *mut Ctx,
    expected_joints: Option<usize>,
) {
    let Some(joints) = expected_joints else {
        report.push(CheckResult::new(
            "rhal.state.completeness",
            "state is complete",
            Outcome::Missing,
            "the descriptor did not parse, so there is no joint count to size the buffers to.",
        ));
        return;
    };

    // Deliberately oversized. If the driver writes more joints than its descriptor declares, that
    // is a mismatch to report rather than a buffer overrun to suffer.
    let mut buffer = StateBuffer::new(joints.max(1) * 4);
    let mut first = buffer.state();
    // SAFETY: `read_state` came from a vtable that passed `accepts`; `first` points at arrays of at
    // least `joints * 4` elements, which is more than the declared joint count.
    let rc = unsafe { read_state(ctx, &mut first, 0) };
    if rc != status::OK {
        report.push(CheckResult::fail(
            "rhal.state.completeness",
            "read_state succeeds",
            format!("read_state() returned {rc} on a freshly opened body."),
        ));
        return;
    }
    report.push(CheckResult::pass(
        "rhal.state.completeness",
        "read_state succeeds",
    ));

    report.push(CheckResult::assert(
        "rhal.state.completeness",
        "joint_count matches the descriptor",
        first.joint_count as usize == joints,
        format!(
            "read_state reports {} joints and the descriptor declares {joints}. The descriptor is \
             the source of truth for the body, so one of the two is wrong about the hardware.",
            first.joint_count
        ),
    ));

    report.push(CheckResult::assert(
        "rhal.state.completeness",
        "timestamps are stamped at acquisition",
        first.stamp_ns != 0,
        "stamp_ns is zero. It is CLOCK_MONOTONIC_RAW at acquisition, never at return and never \
         unset — a timestamp applied on return folds the driver's own latency into every downstream \
         budget.",
    ));

    let count = (first.joint_count as usize).min(buffer.position.len());
    let finite_positions = buffer.position[..count]
        .iter()
        .all(|value| value.is_finite());
    report.push(CheckResult::assert(
        "rhal.state.completeness",
        "every joint reports a finite position",
        finite_positions,
        "a position is NaN or infinite. Velocity and effort may be NaN where the joint does not \
         measure them; position may not, because every guard that bounds travel reads it.",
    ));

    let mut second = buffer.state();
    // SAFETY: as above; the buffers are unchanged and still large enough.
    let rc = unsafe { read_state(ctx, &mut second, 0) };
    report.push(CheckResult::assert(
        "rhal.state.completeness",
        "the sequence advances between reads",
        rc == status::OK && second.seq != first.seq,
        format!(
            "two consecutive reads returned seq={} and seq={}. A repeated sequence is how the \
             watchdog guard recognises a stalled producer, so a driver that never advances it is \
             indistinguishable from a dead bus.",
            first.seq, second.seq
        ),
    ));
}

/// `rhal.command.rejection`, and the one place this command asks a body to *accept* something.
///
/// `commandable` is whether the descriptor's `control.modes` is non-empty. It decides one of the
/// four assertions below and nothing else, and it exists because of §59.2:
///
/// > Most of the world's installed industrial robots do not offer that. Behind a Fanuc, KUKA, ABB
/// > or older Universal Robots controller you upload a *program* — the controller does the
/// > interpolation, owns the servo loop, and enforces its own safety.
///
/// Such a body declares `control.modes: []` and every joint `control_modes: []`, which is the
/// honest shape of it in the schema as it stands. Run against one — `drivers/pendant` is that body
/// — the *acceptance* assertion fails, and it fails for no defect: the driver is refusing a command
/// its machine genuinely does not take. §15 requires `rhal.command.rejection` from `guest`, so the
/// consequence was that **every robot §59.2 describes reached no tier at all**, while §59.2 itself
/// says in its own words that they are *"Guest tier permanently"*. Two sentences in one document,
/// disagreeing, with the code following the stricter one by accident.
///
/// The resolution needs no amendment, because §15 already has the word for it: `not_applicable` is
/// *"reserved for cases where inapplicability is a declared property of the body rather than a gap
/// in the evidence"*, and a body declaring no control modes has declared exactly that. So the
/// acceptance assertion reports `not_applicable` there, which §15 counts as a pass.
///
/// **The three rejection assertions still run**, and that is deliberate. A driver that ignores a
/// short command reads past the end of the caller's array whether or not its machine takes
/// commands, and a body that refused *everything* with one code would hide a real defect behind a
/// true statement. `drivers/pendant` checks the shape before the permission for that reason.
fn command_checks(
    report: &mut Report,
    write_command: unsafe extern "C" fn(*mut Ctx, *const Command, u64) -> i32,
    ctx: *mut Ctx,
    expected_joints: Option<usize>,
    commandable: bool,
    per_joint_modes: bool,
) {
    let Some(joints) = expected_joints else {
        report.push(CheckResult::new(
            "rhal.command.rejection",
            "malformed commands are refused",
            Outcome::Missing,
            "the descriptor did not parse, so no correctly shaped command could be built.",
        ));
        return;
    };
    // A body with no joints is still commanded badly the same ways: a count it does not have
    // and a mode the ABI does not define. The well-formed command below carries zero joints —
    // and where the acceptance assertion goes for one is the `commandable` branch's business,
    // not this filter's. (Before: zero joints reported Missing with the descriptor's excuse,
    // which capped every jointless program-level body at no tier for a parse that succeeded.)

    // Sized for the wrong-count probe below, not just the well-formed one: against a
    // zero-joint body the probe declares one joint, and the arrays must still be readable so a
    // non-conformant driver cannot run off the end of an empty allocation.
    let values = vec![0.0f32; joints.max(1)];
    let mask = vec![0u8; joints.max(1)]; // every joint held: the safest command that is still well-formed
    let mut seq = 1u32;
    // §86.1: no per-joint array and the flag clear, which is what a body-wide command is. This
    // check measures whether a driver refuses a malformed command, and a mixed-mode body is not
    // what it is about — `drivers/mock`'s own suite exercises the array, against a body it knows.
    let command = |mode: u8, joint_count: u16, seq: u32| Command {
        stamp_ns: 1,
        seq,
        mode,
        flags: 0,
        _pad: [0; 2],
        joint_count,
        values: values.as_ptr(),
        mask: mask.as_ptr(),
        modes: core::ptr::null(),
    };

    let well_formed = command(Mode::Position as u8, joints as u16, seq);
    // SAFETY: `values` and `mask` are `joints` long and outlive the call; `joint_count` matches.
    let rc = unsafe { write_command(ctx, &well_formed, 0) };
    report.push(if commandable {
        CheckResult::assert(
            "rhal.command.rejection",
            "accepts a well-formed command",
            rc == status::OK,
            format!("a position command over {joints} held joints returned {rc}."),
        )
    } else {
        CheckResult::new(
            "rhal.command.rejection",
            "accepts a well-formed command",
            Outcome::NotApplicable,
            format!(
                "this body declares no control modes, so there is no mode in which a joint command \
                 is accepted — §59.2's program-level controller, which owns its own servo loop. The \
                 command returned {rc}; the three rejection checks below still apply, because a \
                 driver that ignores a malformed command is dangerous whatever its machine does \
                 with a well-formed one."
            ),
        )
    });
    seq += 1;

    // A short command is the dangerous one: a driver reading `joint_count` from its own state
    // rather than from the command would read past the end of the caller's array. Against a
    // zero-joint body the probe declares one joint instead — there is no shorter malformed
    // count than that.
    let wrong_count = if joints == 0 { 1u16 } else { joints as u16 - 1 };
    let short = command(Mode::Position as u8, wrong_count, seq);
    // SAFETY: the arrays are longer than the `joint_count` declared, so a conformant driver reads
    // within bounds and a non-conformant one still cannot run off the end.
    let rc = unsafe { write_command(ctx, &short, 0) };
    report.push(CheckResult::assert(
        "rhal.command.rejection",
        "rejects a command with the wrong joint count",
        rc == status::EINVAL,
        format!(
            "a command declaring {wrong_count} joints against a {joints}-joint body returned {rc}, and \
             SKR_EINVAL ({}) was expected. A driver that accepts a short command is one that reads \
             past the end of the caller's array.",
            status::EINVAL
        ),
    ));
    seq += 1;

    let unknown_mode = command(200, joints as u16, seq);
    // SAFETY: as above; only `mode` differs and it is a plain integer field.
    let rc = unsafe { write_command(ctx, &unknown_mode, 0) };
    report.push(CheckResult::assert(
        "rhal.command.rejection",
        "rejects an unknown control mode",
        rc == status::EINVAL,
        format!(
            "mode 200 returned {rc}, and SKR_EINVAL ({}) was expected. An unrecognised mode that is \
             quietly treated as position is a torque command that became a position command.",
            status::EINVAL
        ),
    ));
    seq += 1;

    // ── §86.1's per-joint array ─────────────────────────────────────────────
    //
    // Only for a driver that says it reads the array. One case, and it is the one the second
    // numbering exists for: a bare `SKR_MODE_*` written where an `SKR_JMODE_*` goes. A driver that
    // accepts it has taken `SKR_MODE_TORQUE` (2) for a value in a vocabulary that starts at 0x10,
    // and whatever it does next it is not what the caller asked for.
    //
    // There is deliberately no case here for *a mode this joint does not offer*. Which joint offers
    // what is the descriptor's, this checker would be asserting a body's own table back at it, and
    // a body all of whose joints share a mode has no such joint to name. `drivers/mock`'s suite
    // covers it against a body it knows; §15 asks only that a malformed command be refused.
    if !per_joint_modes {
        report.push(CheckResult::new(
            "rhal.command.rejection",
            "rejects a base mode value in the per-joint array",
            Outcome::NotApplicable,
            "this driver does not declare `control.per_joint_modes`, so it never reads the array \
             (§86.1) and a command carrying one is a command it is right to ignore."
                .to_string(),
        ));
    } else {
        let bad_modes = vec![Mode::Torque as u8; joints];
        let malformed = Command {
            stamp_ns: 1,
            seq,
            mode: Mode::Position as u8,
            flags: skynet_rhal::cmd_flags::PER_JOINT_MODES,
            _pad: [0; 2],
            joint_count: joints as u16,
            values: values.as_ptr(),
            mask: mask.as_ptr(),
            modes: bad_modes.as_ptr(),
        };
        // SAFETY: all three arrays are `joints` long and outlive the call; `joint_count` matches.
        let rc = unsafe { write_command(ctx, &malformed, 0) };
        report.push(CheckResult::assert(
            "rhal.command.rejection",
            "rejects a base mode value in the per-joint array",
            rc == status::EINVAL,
            format!(
                "a `modes` array of SKR_MODE_TORQUE ({}) returned {rc}, and SKR_EINVAL ({}) was \
                 expected. Per-joint modes are SKR_JMODE_* and start at 0x10 precisely so that this \
                 transcription error is a refusal rather than a joint moving in the wrong units.",
                Mode::Torque as u8,
                status::EINVAL
            ),
        ));
    }
}

/// §88's route: what a body commanded by named motions must refuse (§88.2, §4).
///
/// **No §15 test is added and none is needed**, which is §88.4's own sentence: `rhal.command.rejection`
/// already asks a driver to refuse what it must refuse, and a motion is one more thing it must. What
/// this adds is the two refusals that only exist on this path.
///
/// *A motion the body does not declare.* §88.2's fifth refusal is the platform's, evaluated before
/// anything reaches a controller — and it is checked here too, on the driver itself, because the
/// platform's gate is only ever as good as the descriptor it read. A controller that accepts a name
/// its own descriptor does not list has a table nobody can command it from.
///
/// *A token this body does not offer.* §4 guards the escape hatch with a capability token precisely
/// so that a policy using a manufacturer feature declares the dependency rather than reaching for
/// it, and a driver answering any token at all has made the guard decorative.
///
/// A body that declares no motions is `not_applicable`, which is §15's word for inapplicability
/// that is a declared property of the body rather than a gap in the evidence.
fn motion_checks(report: &mut Report, driver: &RawDriver, ctx: *mut Ctx) {
    const ID: &str = "rhal.command.rejection";

    let descriptor = report
        .descriptor
        .as_ref()
        .and_then(|json| descriptor_from_json(json).ok());
    let declares_motions = descriptor
        .as_ref()
        .is_some_and(|descriptor| !descriptor.motions.is_empty());

    if !declares_motions {
        report.push(CheckResult::new(
            ID,
            "refuses a motion this body does not declare",
            Outcome::NotApplicable,
            "this body declares no motions, so it is not commanded by name (§84.1) and there \
             is no motion route to exercise."
                .to_string(),
        ));
        return;
    }

    let Some(extension) = driver.extension else {
        report.push(CheckResult::fail(
            ID,
            "refuses a motion this body does not declare",
            "this body declares motions and the driver implements no `extension`, so nothing \
             can command it: §88.1 routes an admitted motion to §4's escape hatch and there \
             is none."
                .to_string(),
        ));
        return;
    };

    // The token is the descriptor's own. A body declaring several has not said which one its
    // motions go to, and guessing would be this checker inventing a rule §88 does not have.
    let tokens = descriptor
        .as_ref()
        .map(|descriptor| descriptor.extensions.clone())
        .unwrap_or_default();
    let [token] = tokens.as_slice() else {
        report.push(CheckResult::fail(
            ID,
            "refuses a motion this body does not declare",
            format!(
                "this body declares motions and {} capability token(s) under `extensions`. §88.1 \
                 routes a motion to one of them and §4 guards it by token, so exactly one is \
                 what says which — declare the one this controller takes its motions on.",
                tokens.len()
            ),
        ));
        return;
    };
    let (Ok(token_c), Ok(other)) = (
        std::ffi::CString::new(token.as_str()),
        std::ffi::CString::new("vendor.skynet-driver-check.not-a-token"),
    ) else {
        report.push(CheckResult::new(
            ID,
            "refuses a motion this body does not declare",
            Outcome::Missing,
            "the declared token contains a NUL and cannot cross §4's ABI.".to_string(),
        ));
        return;
    };

    // A name no motion in the table has, built from the table rather than guessed, so a controller
    // that happens to accept `no_such_motion` is not what is being measured.
    let absent = format!(
        "{}_skynet_driver_check",
        descriptor
            .as_ref()
            .map(|descriptor| descriptor.motions[0].name.clone())
            .unwrap_or_default()
    );
    let mut reply = [0u8; 1024];
    // SAFETY: a vtable that passed `accepts`, the live context, NUL-terminated tokens, and a
    // request and reply buffer each passed with exactly its own length.
    let rc = unsafe {
        extension(
            ctx,
            token_c.as_ptr(),
            absent.as_ptr() as *const std::ffi::c_void,
            absent.len(),
            reply.as_mut_ptr() as *mut std::ffi::c_void,
            reply.len(),
        )
    };
    report.push(CheckResult::assert(
        ID,
        "refuses a motion this body does not declare",
        rc < 0,
        format!(
            "`{absent}` returned {rc}, and a negative code was expected. A controller that \
             accepts a name its own descriptor does not list has a motion table nothing can be \
             checked against — and §88.2's fifth refusal is then the only thing standing between \
             a policy and a program nobody declared."
        ),
    ));

    // SAFETY: as above.
    let rc = unsafe {
        extension(
            ctx,
            other.as_ptr(),
            absent.as_ptr() as *const std::ffi::c_void,
            absent.len(),
            reply.as_mut_ptr() as *mut std::ffi::c_void,
            reply.len(),
        )
    };
    report.push(CheckResult::assert(
        ID,
        "refuses a capability token this body does not offer",
        rc < 0,
        format!(
            "a token this body never declared returned {rc}, and a negative code was expected. \
             §4 guards the escape hatch by token so a policy using a manufacturer feature \
             declares the dependency; a driver that answers any token has made the guard \
             decorative."
        ),
    ));
}

fn estop_checks(
    report: &mut Report,
    driver: &RawDriver,
    read_state: unsafe extern "C" fn(*mut Ctx, *mut State, u64) -> i32,
    ctx: *mut Ctx,
    expected_joints: Option<usize>,
) {
    let Some(emergency_stop) = driver.emergency_stop else {
        report.push(CheckResult::new(
            "safety.estop.latency",
            "emergency_stop is observable in state",
            Outcome::Missing,
            "emergency_stop is null, which `accepts` has already refused.",
        ));
        return;
    };

    // SAFETY: `ctx` is the live context. The header requires this be callable from a signal handler
    // and from a dying process, so calling it on a healthy one is the easy case.
    let rc = unsafe { emergency_stop(ctx) };
    report.push(CheckResult::assert(
        "safety.estop.latency",
        "emergency_stop returns",
        rc == status::OK,
        format!("emergency_stop() returned {rc}. It never blocks and never fails on a live body."),
    ));

    let Some(joints) = expected_joints else {
        return;
    };
    let mut buffer = StateBuffer::new(joints.max(1) * 4);
    let mut state = buffer.state();
    // SAFETY: as in `state_checks`.
    let rc = unsafe { read_state(ctx, &mut state, 0) };
    report.push(CheckResult::assert(
        "safety.estop.latency",
        "an asserted stop is visible in the next state",
        rc == status::OK && state.flags & flags::ESTOP != 0,
        "read_state after emergency_stop does not set SKR_FLAG_ESTOP. A stop the OS cannot observe \
         is a stop it cannot report, log or reconcile against. The *latency* of the stop is a bench \
         measurement; that it happened at all is measurable here.",
    ));
}

fn cstr(pointer: *const c_char) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    // SAFETY: non-null, and the header requires these be NUL-terminated string literals with static
    // lifetime.
    Some(
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Render a report to stderr and return the process exit code.
pub fn print(report: &Report) -> i32 {
    use std::io::Write;
    let out = &mut std::io::stderr();

    let _ = writeln!(out);
    match (&report.driver_name, &report.driver_version) {
        (Some(name), Some(version)) => {
            let _ = writeln!(out, "  {} {}", term::bold(name), term::dim(version));
        }
        _ => {
            let _ = writeln!(out, "  {}", term::bold("(driver declares no name)"));
        }
    }
    let _ = writeln!(out);

    // Grouped by SCTS area rather than by execution order. §15 organises the suite into eight
    // areas and a reader comparing this against it should not have to reassemble them; the order
    // checks happen to run in is an implementation detail of the harness.
    let mut areas: Vec<&str> = Vec::new();
    for result in &report.results {
        let area = result.id.split('.').next().unwrap_or("");
        if !areas.contains(&area) {
            areas.push(area);
        }
    }
    for area in areas {
        let _ = writeln!(out, "  {}", term::dim(area));
        for result in report
            .results
            .iter()
            .filter(|result| result.id.starts_with(area))
        {
            let mark = match result.outcome {
                Outcome::Pass => term::ok("pass"),
                Outcome::Fail => term::bad("FAIL"),
                Outcome::Missing => term::warn("miss"),
                Outcome::NotApplicable => term::dim("n/a "),
            };
            let _ = writeln!(out, "    {mark}  {:<34} {}", result.id, result.what);
            if !result.detail.is_empty() && result.outcome != Outcome::Pass {
                let _ = writeln!(out, "          {}", term::wrap(&result.detail, 10));
            }
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "  {}", term::dim("not covered here"));
    for (id, where_from) in NOT_COVERED {
        let _ = writeln!(out, "    {}  {id:<34} {where_from}", term::dim("----"));
    }

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {} passed, {} failed, {} not measured, {} not applicable",
        report.tally(Outcome::Pass),
        report.tally(Outcome::Fail),
        report.tally(Outcome::Missing),
        report.tally(Outcome::NotApplicable),
    );

    // §15: the evidence caps the award, not the claim. A tier is reported rather than a verdict,
    // because "this is a guest-tier driver" is a true statement about a driver with no signed
    // descriptor, and "failed" is not.
    let (tier, blockers) = report.attainable_tier();
    let _ = writeln!(out);
    if tier == "none" {
        // §80's `inert` decision, applied to an award. "Nothing was measured" and "something was
        // measured and failed" both arrive here as `none`, and folding them into one sentence is
        // the tempting simplification and the wrong answer, for the reason §80 gives: the two send
        // a vendor to do different things. One goes and runs the suite; the other reads their code.
        //
        // The per-test blockers printed underneath have always carried the distinction. The
        // headline did not, and the headline is the line that gets read.
        let failed = blockers
            .iter()
            .filter(|(_, outcome)| *outcome == Outcome::Fail)
            .count();
        let unmeasured = blockers
            .iter()
            .filter(|(_, outcome)| *outcome == Outcome::Missing)
            .count();
        let _ = writeln!(
            out,
            "  {} — {}",
            term::bad("no tier"),
            no_tier_detail(failed, unmeasured)
        );
    } else {
        let _ = writeln!(
            out,
            "  {} {}",
            term::dim("sim evidence supports"),
            if tier == "guest" {
                term::ok(tier)
            } else {
                term::bold(&term::ok(tier))
            }
        );
    }
    if report.simulated {
        // Said here rather than left to be inferred from a driver that passed everything and is
        // still at guest. §35.3 forces the cap so that no simulated run reaches bench evidence,
        // and a vendor reading this needs to know the cap is the body rather than their code.
        let _ = writeln!(
            out,
            "    {} {}",
            term::warn("↑"),
            term::dim(&term::wrap(
                "the body declares simulator: true, which caps the tier at guest whatever the \
                 evidence supports (§35.3). No simulated run is bench evidence.",
                6
            ))
        );
    }
    for (id, outcome) in &blockers {
        let label = match outcome {
            Outcome::Fail => "failed",
            Outcome::Missing => "not measured",
            _ => "unresolved",
        };
        let _ = writeln!(out, "    {} {id} — {label}", term::warn("↑"));
    }
    if !blockers.is_empty() {
        let _ = writeln!(
            out,
            "    {}",
            term::dim(&term::wrap(
                "Absence is not a pass (§15), so a test with no result caps the tier exactly as a \
                 failure does. The bench suite and the runtime tests above cap it further.",
                4
            ))
        );
    }
    let _ = writeln!(out);

    i32::from(!report.green())
}

/// What the two pose guards will be able to do with this tree (§44.5).
///
/// `None` when the graph could not be flattened — the validation above has already reported that,
/// and saying it twice puts one fault in two places for a reader to reconcile.
///
/// The boolean is "both guards could run on this body": every frame placeable, and two or more
/// collision shapes to test against each other.
fn pose_guard_summary(graph: &FrameGraph) -> Option<(bool, String)> {
    let flat = flatten(graph).ok()?;
    let frames = flat.frames();
    let set = FrameSet::new(&frames).ok()?;

    let placeable = (0..set.len())
        .filter(|index| set.statically_resolvable(*index))
        .count();
    let bodies = set.collision_bodies();

    let mut out = format!("{placeable} of {} frames can be placed", set.len());
    if placeable < set.len() {
        out.push_str(
            "; the rest sit below a ball or floating joint, which one scalar cannot describe, so \
             workspace cannot constrain them",
        );
    }
    if bodies < 2 {
        out.push_str(&format!(
            ". {bodies} frames carry a collision shape this kernel can place, so self_collision \
             has no pair to test and reports unavailable (§7.2). `collision: true` with no \
             `collision_shape` is a mesh nobody has reduced to a primitive, which is a real body \
             and not a defect in this driver."
        ));
    } else {
        out.push_str(&format!(
            ". {bodies} collision shapes, {} pairs per cycle",
            set.collision_pairs()
        ));
    }
    Some((placeable == set.len() && bodies >= 2, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A report that reaches no tier is not green, and one that reaches guest is (F82).
    ///
    /// The regression is the first half: `"none"` ranked as guest, so a driver failing a
    /// guest-tier test printed *"no tier"* and exited 0. The second half is the control that the
    /// fix did not move the line the other way.
    #[test]
    fn no_tier_is_not_green() {
        let guest_ids: Vec<&str> = SCTS_TESTS
            .iter()
            .filter(|test| test.suite == "sim" && test.required_from == "guest")
            .map(|test| test.id)
            .collect();
        assert!(
            !guest_ids.is_empty(),
            "§15 requires some sim test from guest"
        );

        let mut passing = Report::default();
        for id in guest_ids.iter().copied() {
            passing.push(CheckResult::pass(id, "measured"));
        }
        // Guest or above: the tests above guest are absent from this report, and a test this
        // command does not measure is "not covered here" rather than a failure of the driver.
        assert_ne!(passing.attainable_tier().0, "none");
        assert!(passing.green(), "every guest-tier sim test passed");

        let mut failing = passing.clone();
        failing.push(CheckResult::fail(guest_ids[0], "measured", "and failed"));
        assert_eq!(failing.attainable_tier().0, "none");
        assert!(
            !failing.green(),
            "a guest-tier failure is no tier, and no tier is not green"
        );

        assert!(tier_rank("none") < tier_rank("guest"));
        assert!(tier_rank("not-a-tier") < tier_rank("guest"));
        assert!(
            tier_rank("guest") < tier_rank("host") && tier_rank("host") < tier_rank("certified")
        );
    }

    /// The four causes of `none`, and that no two of them render the same sentence.
    ///
    /// The pair that matters is the middle two: a driver whose tests merely have no result must not
    /// be told something failed, and a driver with real failures must not be offered "nothing here
    /// failed". Both were true of the single sentence this replaced.
    #[test]
    fn no_evidence_and_a_failure_do_not_read_alike() {
        let nothing = no_tier_detail(0, 0);
        let unmeasured = no_tier_detail(0, 3);
        let failed = no_tier_detail(2, 0);
        let both = no_tier_detail(2, 3);

        assert!(unmeasured.contains("nothing here failed"), "{unmeasured}");
        assert!(unmeasured.contains("no result yet"), "{unmeasured}");
        assert!(failed.contains("2 guest-tier test(s) failed"), "{failed}");
        assert!(!failed.contains("nothing here failed"), "{failed}");
        assert!(both.contains('2') && both.contains('3'), "{both}");

        let all = [&nothing, &unmeasured, &failed, &both];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b, "two causes rendering one sentence");
            }
        }
    }
}
