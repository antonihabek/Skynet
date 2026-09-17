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
use skynet_contracts::safety::kinematics::FrameSet;
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

fn tier_rank(tier: &str) -> usize {
    TIERS.iter().position(|known| *known == tier).unwrap_or(0) + 1
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
    command_checks(
        &mut report,
        write_command,
        ctx,
        expected_joints,
        commandable,
    );
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

    report
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

    fn state(&mut self) -> State {
        State {
            stamp_ns: 0,
            seq: 0,
            joint_count: 0,
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
) {
    let Some(joints) = expected_joints.filter(|count| *count > 0) else {
        report.push(CheckResult::new(
            "rhal.command.rejection",
            "malformed commands are refused",
            Outcome::Missing,
            "the descriptor did not parse, so no correctly shaped command could be built.",
        ));
        return;
    };

    let values = vec![0.0f32; joints];
    let mask = vec![0u8; joints]; // every joint held: the safest command that is still well-formed
    let mut seq = 1u32;
    let command = |mode: u8, joint_count: u16, seq: u32| Command {
        stamp_ns: 1,
        seq,
        mode,
        _pad: [0; 3],
        joint_count,
        values: values.as_ptr(),
        mask: mask.as_ptr(),
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
    // rather than from the command would read past the end of the caller's array.
    let short = command(Mode::Position as u8, (joints as u16).saturating_sub(1), seq);
    // SAFETY: the arrays are longer than the `joint_count` declared, so a conformant driver reads
    // within bounds and a non-conformant one still cannot run off the end.
    let rc = unsafe { write_command(ctx, &short, 0) };
    report.push(CheckResult::assert(
        "rhal.command.rejection",
        "rejects a command with the wrong joint count",
        rc == status::EINVAL,
        format!(
            "a command declaring {} joints against a {joints}-joint body returned {rc}, and \
             SKR_EINVAL ({}) was expected. A driver that accepts a short command is one that reads \
             past the end of the caller's array.",
            joints.saturating_sub(1),
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
