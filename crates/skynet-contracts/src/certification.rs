//! SCTS — the Skynet Compatibility Test Suite, and the award derived from it.
//!
//! This is the module that decides whether "universal" is a claim or a fact.
//!
//! Android did not become universal by running everywhere. It became universal because Google
//! wrote down what an Android device must do, built a suite that measured it, and refused the
//! trademark and the app store to anything that failed. "Is this Android?" stopped being a
//! marketing question and became a test run with a pass or a fail at the end of it.
//!
//! Robotics has no equivalent. "ROS 2 compatible" is a sentence anybody may write about anything.
//! SCTS is the missing mechanism, and the reason certification sits in the operating system rather
//! than in the marketplace: only the thing running on the robot can measure estop latency.
//!
//! # Three suites, because they measure different kinds of truth
//!
//! | | Where | Cost | Proves |
//! |---|---|---|---|
//! | `sim` | CI, on every commit | free | **the contract** |
//! | `bench` | the physical robot, once, with a human present | a day | **the machine** |
//! | `field` | continuous telemetry, forever | free | **that it stayed true** |
//!
//! The third is the one nobody else does, and it is where certification stops being a document and
//! becomes a live property. A certificate that cannot expire is a certificate that lies with
//! increasing confidence over time.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Which suite a test belongs to.
pub const SCTS_SUITES: [&str; 3] = ["sim", "bench", "field"];

/// Areas the suite covers.
pub const SCTS_AREAS: [&str; 9] = [
    "rhal",
    "embodiment",
    "safety",
    "package",
    "ota",
    "telemetry",
    "offline",
    "security",
    "power",
];

/// One test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SctsTest {
    /// Stable identifier, used in results and in failure reports.
    pub id: &'static str,
    /// Which suite runs it.
    pub suite: &'static str,
    /// What it is about.
    pub area: &'static str,
    /// What is measured, in the terms a vendor engineer would use to reproduce it.
    pub measures: &'static str,
    /// Lowest tier that must pass this.
    pub required_from: &'static str,
    /// Whether a pass is a measured number rather than a boolean.
    pub quantitative: bool,
    /// Unit of the measurement, when quantitative.
    pub unit: Option<&'static str>,
}

const fn test(
    id: &'static str,
    suite: &'static str,
    area: &'static str,
    required_from: &'static str,
    measures: &'static str,
) -> SctsTest {
    SctsTest {
        id,
        suite,
        area,
        measures,
        required_from,
        quantitative: false,
        unit: None,
    }
}

const fn quant(
    id: &'static str,
    suite: &'static str,
    area: &'static str,
    required_from: &'static str,
    unit: &'static str,
    measures: &'static str,
) -> SctsTest {
    SctsTest {
        id,
        suite,
        area,
        measures,
        required_from,
        quantitative: true,
        unit: Some(unit),
    }
}

/// The suite.
///
/// Thirty-six tests across nine areas, and the count is the same in the prose, the oracle and here.
///
/// Five of them were added when §52 was reconciled: that table named a measure for every descriptor
/// field Parts IV and V added, and five of those names referred to nothing. A table row pointing at
/// a test that does not exist is a schema obligation with no way to fail.
///
/// `embodiment.capabilities.declared` was added with the field it measures rather than after it,
/// which is the rule that expansion was meant to establish.
///
/// Deliberately short and load-bearing. A conformance suite that tests three hundred things is a
/// suite nobody runs and every vendor games; one that tests the two dozen properties the rest of
/// the system actually depends on is one that can be *required*. Every test here exists because
/// some other part of Skynet OS would be unsound without it.
///
/// The measurements that matter most are the ones taken **at the joint rather than reported by the
/// driver**. A driver returning success is not evidence.
pub const SCTS_TESTS: [SctsTest; 36] = [
    // ── RHAL: the vendor interface ──────────────────────────────────────────
    test("rhal.abi.version", "sim", "rhal", "guest",
        "The driver reports an RHAL ABI version the running OS supports, and refuses to load against one it does not."),
    test("rhal.state.completeness", "sim", "rhal", "guest",
        "Every joint in the embodiment descriptor appears in the state the driver returns, with matching names."),
    quant("rhal.state.rate", "bench", "rhal", "host", "Hz",
        "Sustained state-read rate over ten minutes, and the worst-case gap between two successive reads."),
    quant("rhal.command.latency", "bench", "rhal", "host", "ms",
        "Time from a command accepted at the RHAL boundary to measurable joint motion. The number the descriptor must declare."),
    test("rhal.command.rejection", "sim", "rhal", "guest",
        "The driver rejects a command naming a joint that does not exist, rather than ignoring it."),
    quant("rhal.sensor.latency", "bench", "rhal", "host", "ms",
        "Per sensor: time from the observed event to the timestamp the frame carries. A backend that stamps arrival rather than capture fails, because a queue delay folded into a timestamp is a number the policy treats as ground truth (§47.2)."),
    // ── Embodiment: the body as an interface ────────────────────────────────
    test("embodiment.descriptor.valid", "sim", "embodiment", "guest",
        "The descriptor parses, its degrees of freedom match its joint list, and every declared role is well formed."),
    test("embodiment.descriptor.signed", "sim", "embodiment", "host",
        "The descriptor carries a signature that verifies against a key registered to the vendor namespace."),
    test("embodiment.limits.sourced", "sim", "embodiment", "host",
        "Every declared limit cites a source. A rating with no provenance fails, because the safety kernel would be enforcing a rumour."),
    quant("embodiment.limits.honest", "bench", "embodiment", "certified", "%",
        "Measured joint velocity and torque against the declared ratings under load. A body that cannot reach what it claims fails as surely as one that exceeds it."),
    quant("embodiment.roles.coverage", "sim", "embodiment", "host", "%",
        "Share of actuated joints carrying a canonical role. Below full coverage, cross-body transport degrades to exact-name matching."),
    test("embodiment.capabilities.declared", "sim", "embodiment", "host",
        "Every capability token the descriptor declares is one the driver can actually service, and every token a resident package requires appears in the list. The set is what admission checks a package against (§8), so a body claiming a capability it does not have admits a package that will command hardware nobody fitted."),
    test("embodiment.frames.complete", "sim", "embodiment", "host",
        "The descriptor carries a §44.3 frame graph that is a single rooted tree, every dynamic node names a joint the descriptor declares, every mimic names a driver present in the tree, and every collision shape is one this edition can evaluate. Absent or null is not a failure of this test — it is the two pose guards reporting unavailable, which §44.5 permits — but a graph that is present and wrong is."),
    // ── Safety: the claims that must not be wrong ───────────────────────────
    test("safety.kernel.parity", "sim", "safety", "guest",
        "The on-robot kernel and the reference implementation return identical verdicts across the full cycle-vector corpus."),
    quant("safety.watchdog.fires", "bench", "safety", "host", "ms",
        "Commands are stopped mid-motion; time until the robot reaches a controlled stop, against the declared watchdog window."),
    test("safety.envelope.clamps", "bench", "safety", "host",
        "A command at 150% of a joint rating is issued; the joint never exceeds its rating, measured at the joint rather than reported by the driver."),
    quant("safety.estop.latency", "bench", "safety", "host", "ms",
        "Time from estop assertion to zero commanded torque. Measured at the actuator; a driver reporting success is not evidence."),
    quant("safety.estop.independent", "bench", "safety", "certified", "ms",
        "The application processor is halted mid-motion. The robot must still stop, proving the stop path does not run through it."),
    test("safety.ratelimit.enforced", "bench", "safety", "host",
        "A single-cycle step from zero to full scale produces a ramp at the configured jerk ceiling, not a step."),
    test("safety.command_loss.behaviour", "bench", "safety", "host",
        "Commands stop and the drives are observed at the joint. What they do must be what `on_command_loss` declares (§46.2). A body observed to coast may not claim host, because the watchdog chain’s own recovery path is, on that machine, a fall."),
    test("safety.homing.envelope", "bench", "safety", "host",
        "The driver-provided homing procedure is run and the joints are watched. No joint exceeds the restricted homing envelope the descriptor declares (§48.3). Homing is not an exemption from the kernel; it is a narrower envelope. A body that declares it needs no homing records not_applicable."),
    test("safety.policy.isolation", "sim", "safety", "guest",
        "A sandboxed package holding an actuator file descriptor is refused; the sandbox has no path to one."),
    // ── Packages ────────────────────────────────────────────────────────────
    test("package.signature.required", "sim", "package", "guest",
        "An unsigned package, and one whose bytes do not match its digest, are both refused."),
    test("package.budget.enforced", "bench", "package", "host",
        "A package exceeding its declared memory budget is stopped rather than allowed to exhaust the robot."),
    test("package.grants.gate", "sim", "package", "guest",
        "A package commands a capability the operator did not grant; the mode gate refuses it and the refusal is logged."),
    // ── OTA ─────────────────────────────────────────────────────────────────
    test("ota.rollback.works", "bench", "ota", "host",
        "A deliberately broken image is staged; the robot boots it, fails its health check, and returns to the previous slot unattended."),
    test("ota.safe_state.respected", "bench", "ota", "host",
        "An update issued mid-task is staged and not applied until the robot reaches a declared safe state."),
    test("ota.safety_gate", "sim", "ota", "host",
        "An update changing the safety kernel or the envelope is refused without a fresh bench pass. OTA must not be a route around certification."),
    // ── Telemetry and the black box ─────────────────────────────────────────
    test("telemetry.blackbox.complete", "bench", "telemetry", "host",
        "Every cycle in a ten-minute run appears in the black box with observation hash, policy output, kernel verdict and final command."),
    test("telemetry.replay.deterministic", "sim", "telemetry", "host",
        "Replaying a sealed recording through the same policy digest and embodiment digest reproduces the same actions."),
    test("telemetry.blackbox.survives", "bench", "telemetry", "certified",
        "Power is cut mid-motion; the black box up to the last completed cycle is intact after reboot."),
    // ── Offline: the property the whole design rests on ─────────────────────
    test("offline.control.independent", "bench", "offline", "guest",
        "The network is removed mid-task. The robot completes the task. No control decision may require a round trip."),
    test("offline.reconcile.resumes", "bench", "offline", "host",
        "After a week offline the robot reconciles to desired state and uploads its backlog without operator action."),
    // ── Security ────────────────────────────────────────────────────────────
    test("security.identity.attested", "sim", "security", "host",
        "The device key is held in a secure element or TPM and check-ins carry an attestation of the running image digest."),
    test("security.credential.scope", "sim", "security", "guest",
        "A device credential lifted from the robot cannot read repositories, claim jobs, or write to another namespace."),
    // ── Power: the energy the robot keeps back ─────────────────────────
    quant("power.safe_state.reachable", "bench", "power", "host", "s",
        "From full rated load, the robot is taken to its nearest safe state and timed, against the `time_to_safe_state_s` it declares (§36.2). The reserve budget is computed from that figure, so a body that declares two seconds and takes six has a third of the reserve it needs."),
];

/// What a test run concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SctsOutcome {
    /// Measured, and inside the threshold.
    Pass,
    /// Measured, and outside it.
    Fail,
    /// Not run. **Never a pass.**
    Skipped,
    /// Genuinely inapplicable to this body — the balance guard on a wheeled base. Passes.
    NotApplicable,
}

impl SctsOutcome {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            SctsOutcome::Pass => "pass",
            SctsOutcome::Fail => "fail",
            SctsOutcome::Skipped => "skipped",
            SctsOutcome::NotApplicable => "not_applicable",
        }
    }

    /// Parse the wire spelling.
    pub fn parse(text: &str) -> Option<SctsOutcome> {
        match text {
            "pass" => Some(SctsOutcome::Pass),
            "fail" => Some(SctsOutcome::Fail),
            "skipped" => Some(SctsOutcome::Skipped),
            "not_applicable" => Some(SctsOutcome::NotApplicable),
            _ => None,
        }
    }
}

/// One result.
#[derive(Debug, Clone, PartialEq)]
pub struct SctsResult {
    /// Which test.
    pub test_id: String,
    /// What it concluded.
    pub outcome: SctsOutcome,
    /// The measured figure, for quantitative tests.
    pub value: Option<f64>,
    /// When it ran. Certification decays from the *oldest* bench result, not the newest.
    pub ran_at: String,
    /// Free text from the runner — the failure, or the conditions of the measurement.
    pub detail: Option<String>,
}

/// What a vendor is claiming, and the evidence for it.
#[derive(Debug, Clone, PartialEq)]
pub struct CertificationInput {
    /// Tier the vendor is claiming.
    pub claimed: String,
    /// Every result submitted.
    pub results: Vec<SctsResult>,
    /// Field conformance over the reporting window, 0–1. `None` when no fleet data exists yet.
    pub field_conformance: Option<f64>,
    /// Age of the oldest bench result in days.
    pub bench_age_days: Option<f64>,
    /// Whether the body is simulated — [`crate::embodiment::EmbodimentDescriptor::simulator`].
    ///
    /// §35.3 and §70. Read from the descriptor rather than from the driver's declared ceiling,
    /// because a simulator driver written in C++ has no language ceiling to fall back on.
    pub simulator: bool,
}

/// What was awarded, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct CertificationVerdict {
    /// Tier actually awarded. Never above what was claimed, often below.
    pub awarded: String,
    /// Tests required for the claimed tier that did not pass.
    pub failures: Vec<String>,
    /// Required tests with no result at all. **Absence is not a pass.**
    pub missing: Vec<String>,
    /// Why the award landed where it did, for the badge's tooltip and the audit log.
    pub reasons: Vec<String>,
    /// Whether the award needs a re-run to stay valid, and by when.
    pub expires_in_days: Option<f64>,
}

const TIER_ORDER: [&str; 3] = ["guest", "host", "certified"];

fn tier_index(tier: &str) -> Option<usize> {
    TIER_ORDER.iter().position(|t| *t == tier)
}

/// Tests required at a tier: its own, plus every tier below it.
pub fn tests_required_for(tier: &str) -> Vec<SctsTest> {
    let Some(ceiling) = tier_index(tier) else {
        return Vec::new();
    };
    SCTS_TESTS
        .iter()
        .filter(|test| tier_index(test.required_from).is_some_and(|index| index <= ceiling))
        .copied()
        .collect()
}

/// How long a bench result stands before it must be re-measured.
///
/// A year, because bench testing a humanoid is expensive and a shorter window would make
/// certification something vendors avoid rather than seek. Field conformance is what carries the
/// claim in between — it is measured continuously and can revoke the award long before the year is
/// up.
pub const BENCH_VALIDITY_DAYS: f64 = 365.0;

/// Field conformance below which an award is suspended.
///
/// 0.999 rather than 1.0: a fleet of a hundred robots running at 100 Hz produces tens of millions
/// of cycles a day, and demanding literal perfection would suspend every honest vendor on sensor
/// noise while telling operators nothing. What this threshold actually catches is a systematic
/// drift — a robot whose real envelope has moved away from its declared one.
pub const FIELD_CONFORMANCE_FLOOR: f64 = 0.999;

/// Award a tier from the evidence.
///
/// Two rules make this worth having. **Absence is not a pass** — a required test with no result is
/// `missing`, never a silent success, which is the loophole every self-certification scheme
/// eventually dies of. And **the award is capped by the evidence, not by the claim** — a vendor
/// claiming the certified tier whose independent stop path was never measured is awarded `host`,
/// with the reason recorded, rather than refused outright. Downgrading tells the truth and keeps
/// the vendor in the programme; refusing sends them off to write "ROS 2 compatible" on a web page
/// instead, which helps nobody.
pub fn award_certification(input: &CertificationInput) -> CertificationVerdict {
    let mut reasons: Vec<String> = Vec::new();
    let mut best = "none".to_string();
    let mut failures: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();

    let claimed_index = tier_index(&input.claimed);

    // Walk upward and keep the highest tier fully satisfied, so the evidence sets the ceiling.
    if let Some(claimed_index) = claimed_index {
        for &tier in TIER_ORDER.iter().take(claimed_index + 1) {
            let required = tests_required_for(tier);
            let mut tier_failures: Vec<String> = Vec::new();
            let mut tier_missing: Vec<String> = Vec::new();

            for test in &required {
                // **Last wins**, not first. The reference builds a `Map` from the results and a
                // later entry overwrites an earlier one, so a re-run supersedes what it re-ran.
                // This was `.find(..)` — first wins — which is the same answer on every input
                // where a test is reported once, and awards a tier on a *stale pass* when a
                // vendor reports a failing re-run alongside the pass it replaced.
                // `certification/award.json :: duplicate-results-keep-the-last` is that input.
                let result = input.results.iter().rev().find(|r| r.test_id == test.id);
                match result {
                    None
                    | Some(SctsResult {
                        outcome: SctsOutcome::Skipped,
                        ..
                    }) => tier_missing.push(test.id.to_string()),
                    Some(SctsResult {
                        outcome: SctsOutcome::Fail,
                        ..
                    }) => tier_failures.push(test.id.to_string()),
                    // `not_applicable` passes: the balance guard on a wheeled base is genuinely
                    // inapplicable, and forcing a fail there would push vendors to fake a result
                    // rather than declare a shape.
                    _ => {}
                }
            }

            if tier_failures.is_empty() && tier_missing.is_empty() {
                best = tier.to_string();
                failures = Vec::new();
                missing = Vec::new();
            } else {
                failures = tier_failures.clone();
                missing = tier_missing.clone();
                reasons.push(format!(
                    "{tier}: {} failing, {} unmeasured.",
                    tier_failures.len(),
                    tier_missing.len()
                ));
                break;
            }
        }
    }

    if best != "none" && input.claimed != best {
        reasons.push(format!(
            "Claimed {}; the evidence supports {}.",
            input.claimed, best
        ));
    }

    // §35.3 and §70: a simulator RHAL driver declares itself one, and the declaration caps the
    // award at guest. Not because a simulated run is worthless — the sim half of SCTS is
    // load-bearing infrastructure and cannot run without these drivers — but because no simulated
    // run may ever be presented as bench evidence, and a `host` badge on a body that does not exist
    // is exactly that.
    //
    // Applied *after* the evidence walk rather than by refusing to walk, so the verdict still
    // records which tier the tests would have supported. "The evidence supports host; the body is
    // simulated, so guest" is a more useful sentence to a vendor than a silent guest, and it is the
    // same downgrade-and-say-why rule the whole award is built on.
    if input.simulator && (best == "host" || best == "certified") {
        reasons.push(format!(
            "The body is simulated (§35.3), so the award is capped at guest; the evidence supports {best}."
        ));
        best = "guest".to_string();
    }

    // Field conformance suspends an award rather than lowering it. A robot whose commands have
    // been leaving its envelope is not a lower-tier robot — it is one whose certificate no longer
    // describes it, and the honest state is suspended pending re-measurement.
    if let Some(conformance) = input.field_conformance {
        if conformance < FIELD_CONFORMANCE_FLOOR && best != "none" {
            reasons.push(format!(
                "Field conformance {}% is below the {}% floor; the award is suspended pending re-measurement.",
                crate::json::fixed(conformance * 100.0, 3),
                crate::json::fixed(FIELD_CONFORMANCE_FLOOR * 100.0, 1)
            ));
            best = "none".to_string();
        }
    }

    let mut expires: Option<f64> = None;
    if best != "none" {
        if let Some(age) = input.bench_age_days {
            let remaining = BENCH_VALIDITY_DAYS - age;
            if remaining <= 0.0 {
                reasons
                    .push("Bench results are older than a year; the award has lapsed.".to_string());
                best = "none".to_string();
                expires = Some(0.0);
            } else {
                expires = Some(remaining);
            }
        }
    }

    CertificationVerdict {
        awarded: best,
        failures,
        missing,
        reasons,
        expires_in_days: expires,
    }
}

/// Field conformance from a reporting window.
///
/// The ratio of cycles the kernel did not have to intervene in. **Not** "violations detected",
/// which would reward a robot with a broken detector — a robot reporting zero cycles reports no
/// conformance at all, and the caller gets `None` rather than a flattering 1.0.
pub fn field_conformance(cycles: f64, interventions: f64) -> Option<f64> {
    if !cycles.is_finite() || cycles <= 0.0 {
        return None;
    }
    let ratio = (cycles - interventions) / cycles;
    // Clamped rather than trusted. More interventions than cycles is a robot whose counters
    // disagree with each other, and the honest floor for that is zero rather than a negative
    // conformance figure §15 has no reading for — which matters because §15 checks this number
    // against a floor, and a negative one sails under it into `suspended` for the right reason by
    // accident. Above 1.0 is the dangerous direction and is the one this must never produce.
    Some(ratio.clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing(ids: &[&str]) -> Vec<SctsResult> {
        ids.iter()
            .map(|id| SctsResult {
                test_id: id.to_string(),
                outcome: SctsOutcome::Pass,
                value: None,
                ran_at: "2026-08-29T00:00:00Z".to_string(),
                detail: None,
            })
            .collect()
    }

    fn all_ids_for(tier: &str) -> Vec<&'static str> {
        tests_required_for(tier).iter().map(|t| t.id).collect()
    }

    #[test]
    fn the_suite_is_the_size_the_specification_states() {
        assert_eq!(SCTS_TESTS.len(), 36, "36 tests across 9 areas");
        for area in SCTS_AREAS {
            assert!(
                SCTS_TESTS.iter().any(|t| t.area == area),
                "no test covers {area}"
            );
        }
    }

    /// §52's "Measured by" column, held to the suite.
    ///
    /// §52 collects the descriptor fields Parts IV and V added and names, for each, the SCTS test
    /// that measures it. For three sessions five of those names referred to nothing — the table
    /// read as a complete set of obligations and three of its rows pointed at `frames.complete`,
    /// which no edition implemented. A vendor reading it would have had no way to fail.
    ///
    /// The table is prose in a mirrored document and this is code, so nothing makes them agree
    /// automatically. What this does is make the *second half* of a §52 row impossible to forget:
    /// add a row naming a new test, and this fails until the test exists.
    #[test]
    fn every_test_52_names_as_a_measure_exists() {
        // Transcribed from §52's third column, one entry per distinct test named there.
        const NAMED_BY_52: [&str; 8] = [
            "embodiment.capabilities.declared",
            "embodiment.frames.complete",
            "embodiment.descriptor.valid",
            "safety.command_loss.behaviour",
            "safety.homing.envelope",
            "power.safe_state.reachable",
            "rhal.sensor.latency",
            "rhal.command.latency",
        ];
        for id in NAMED_BY_52 {
            assert!(
                SCTS_TESTS.iter().any(|test| test.id == id),
                "§52 says {id} measures a descriptor field, and no such test exists. A schema \
                 obligation with no way to fail is a field a vendor may fill in with anything."
            );
        }
    }

    #[test]
    fn every_test_id_is_unique() {
        for (i, test) in SCTS_TESTS.iter().enumerate() {
            for other in &SCTS_TESTS[i + 1..] {
                assert_ne!(test.id, other.id, "duplicate id {}", test.id);
            }
        }
    }

    #[test]
    fn tiers_are_cumulative() {
        let guest = tests_required_for("guest").len();
        let host = tests_required_for("host").len();
        let certified = tests_required_for("certified").len();
        assert!(guest < host && host < certified);
        assert_eq!(certified, SCTS_TESTS.len());
    }

    #[test]
    fn a_simulated_body_is_capped_at_guest_and_told_why() {
        // §35.3, §70. The cap is the body's, not the edition's: it applies whatever the driver was
        // written in, and it applies after the walk so the vendor still learns what their evidence
        // was worth.
        let host_tests: Vec<SctsResult> = tests_required_for("host")
            .iter()
            .map(|test| SctsResult {
                test_id: test.id.to_string(),
                outcome: SctsOutcome::Pass,
                value: None,
                ran_at: "2026-08-29T00:00:00Z".to_string(),
                detail: None,
            })
            .collect();

        let real = award_certification(&CertificationInput {
            claimed: "host".to_string(),
            results: host_tests.clone(),
            field_conformance: None,
            bench_age_days: None,
            simulator: false,
        });
        assert_eq!(real.awarded, "host");

        let simulated = award_certification(&CertificationInput {
            claimed: "host".to_string(),
            results: host_tests,
            field_conformance: None,
            bench_age_days: None,
            simulator: true,
        });
        assert_eq!(simulated.awarded, "guest");
        assert!(
            simulated
                .reasons
                .iter()
                .any(|reason| reason.contains("simulated") && reason.contains("host")),
            "the verdict must still say what the evidence supported: {:?}",
            simulated.reasons
        );
    }

    #[test]
    fn the_simulator_cap_never_manufactures_an_award() {
        let verdict = award_certification(&CertificationInput {
            claimed: "host".to_string(),
            results: Vec::new(),
            field_conformance: None,
            bench_age_days: None,
            simulator: true,
        });
        assert_eq!(verdict.awarded, "none", "a cap lowers; it does not award");
    }

    #[test]
    fn absence_is_not_a_pass() {
        let verdict = award_certification(&CertificationInput {
            claimed: "guest".to_string(),
            results: Vec::new(),
            field_conformance: None,
            bench_age_days: None,
            simulator: false,
        });
        assert_eq!(verdict.awarded, "none");
        assert!(!verdict.missing.is_empty());
        assert!(verdict.failures.is_empty(), "unmeasured is not failing");
    }

    #[test]
    fn a_skipped_test_is_missing_not_passing() {
        let mut results = passing(&all_ids_for("guest"));
        results[0].outcome = SctsOutcome::Skipped;
        let verdict = award_certification(&CertificationInput {
            claimed: "guest".to_string(),
            results,
            field_conformance: None,
            bench_age_days: None,
            simulator: false,
        });
        assert_eq!(verdict.awarded, "none");
        assert_eq!(verdict.missing.len(), 1);
    }

    #[test]
    fn not_applicable_passes() {
        let mut results = passing(&all_ids_for("guest"));
        results[0].outcome = SctsOutcome::NotApplicable;
        let verdict = award_certification(&CertificationInput {
            claimed: "guest".to_string(),
            results,
            field_conformance: None,
            bench_age_days: None,
            simulator: false,
        });
        assert_eq!(verdict.awarded, "guest");
    }

    #[test]
    fn the_evidence_caps_the_award_rather_than_the_claim() {
        // A vendor claiming certified, with only the guest evidence in hand.
        let verdict = award_certification(&CertificationInput {
            claimed: "certified".to_string(),
            results: passing(&all_ids_for("guest")),
            field_conformance: None,
            bench_age_days: None,
            simulator: false,
        });
        assert_eq!(verdict.awarded, "guest", "downgraded, not refused");
        assert!(verdict
            .reasons
            .iter()
            .any(|r| r.contains("the evidence supports guest")));
    }

    #[test]
    fn field_conformance_suspends_rather_than_downgrades() {
        let verdict = award_certification(&CertificationInput {
            claimed: "guest".to_string(),
            results: passing(&all_ids_for("guest")),
            field_conformance: Some(0.99),
            bench_age_days: None,
            simulator: false,
        });
        assert_eq!(verdict.awarded, "none");
        assert!(verdict.reasons.iter().any(|r| r.contains("suspended")));
    }

    #[test]
    fn the_floor_tolerates_sensor_noise() {
        let verdict = award_certification(&CertificationInput {
            claimed: "guest".to_string(),
            results: passing(&all_ids_for("guest")),
            field_conformance: Some(0.9995),
            bench_age_days: None,
            simulator: false,
        });
        assert_eq!(verdict.awarded, "guest");
    }

    #[test]
    fn bench_results_lapse_after_a_year() {
        let verdict = award_certification(&CertificationInput {
            claimed: "guest".to_string(),
            results: passing(&all_ids_for("guest")),
            field_conformance: None,
            bench_age_days: Some(400.0),
            simulator: false,
        });
        assert_eq!(verdict.awarded, "none");
        assert_eq!(verdict.expires_in_days, Some(0.0));
        assert!(verdict
            .reasons
            .iter()
            .any(|r| r.contains("older than a year")));
    }

    #[test]
    fn a_fresh_bench_result_reports_its_remaining_life() {
        let verdict = award_certification(&CertificationInput {
            claimed: "guest".to_string(),
            results: passing(&all_ids_for("guest")),
            field_conformance: None,
            bench_age_days: Some(65.0),
            simulator: false,
        });
        assert_eq!(verdict.expires_in_days, Some(300.0));
    }

    #[test]
    fn a_robot_reporting_no_cycles_reports_no_conformance() {
        assert_eq!(field_conformance(0.0, 0.0), None);
        assert_ne!(
            field_conformance(0.0, 0.0),
            Some(1.0),
            "a flattering 1.0 would reward a broken detector"
        );
    }

    #[test]
    fn conformance_is_the_ratio_of_uninterrupted_cycles() {
        assert_eq!(field_conformance(1000.0, 1.0), Some(0.999));
        assert_eq!(field_conformance(100.0, 0.0), Some(1.0));
        assert_eq!(
            field_conformance(100.0, 500.0),
            Some(0.0),
            "clamped, not negative"
        );
    }

    #[test]
    fn an_unknown_claimed_tier_awards_nothing() {
        let verdict = award_certification(&CertificationInput {
            claimed: "platinum".to_string(),
            results: passing(&all_ids_for("certified")),
            field_conformance: None,
            bench_age_days: None,
            simulator: false,
        });
        assert_eq!(verdict.awarded, "none");
    }
}
