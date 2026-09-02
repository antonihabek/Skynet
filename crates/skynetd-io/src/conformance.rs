//! §15's suite, run by the robot on itself.
//!
//! §73's conformance row has said the same thing for as long as it has existed:
//!
//! > No runner executes the suite on a robot, and nothing signs a result.
//!
//! This is the first half. `skynet driver check` runs the *driver-facing* sim tests against a
//! shared object and lists, by name, the eight it cannot cover — the ones that are properties of
//! the **runtime** rather than of a driver. Those eight are this module's subject, because the
//! runtime is what this daemon is.
//!
//! # The eight sentences, most of which had gone stale
//!
//! `NOT_COVERED` in `skynet-cli` explains each omission with a "needs X". Five of those sentences
//! were true when they were written and are not now:
//!
//! | Test | Said | Since |
//! |---|---|---|
//! | `package.signature.required` | needs the package installer | §37.1's installer, and five signature states |
//! | `package.grants.gate` | needs the permission model | grants, admission, `effective_grants` |
//! | `ota.safety_gate` | needs the updater | §12, and §75's bundle route |
//! | `telemetry.replay.deterministic` | needs the black box | ring, MCAP, sealing, replay |
//! | `security.credential.scope` | needs device tokens | `skd_` credentials and their scopes |
//!
//! This is finding 49 at scale — *"which we do not" is a sentence to check, not to believe* — and
//! the value of a runner is as much in re-asking those questions as in answering them. Two of the
//! five turn out to be answerable **right now**, one is answerable and is not this machine's to
//! answer, and two need something that still does not exist.
//!
//! # A check that runs, or a reason. Never a pass by omission
//!
//! §15's first award rule is *"absence is not a pass"*, and it applies to the runner harder than to
//! anything it runs: a conformance runner that reported `pass` for a test it did not perform would
//! be the single most damaging component in this repository. So every entry here is one of:
//!
//! * a check this daemon **executed**, against its own installed code, with a real verdict;
//! * [`SctsOutcome::Skipped`], with a sentence naming what is missing and whose it is.
//!
//! # The uncomfortable result, which is the point
//!
//! `package.signature.required` runs, and this edition **fails it**. An unsigned package stages and
//! installs today — `crate::admit` says so in its own header — because nothing on a robot can
//! produce a signature and refusing every locally-built package would refuse the golden path.
//!
//! A runner that reported that as `skipped` would be hiding a known gap behind an unmeasured one.
//! It is a `fail`, it caps this robot's evidence below guest, and that is the honest reading: the
//! platform measuring itself and reporting that its own installer does not yet meet its own bar.

use skynet_contracts::admission::{
    admit_package, AdmissionParams, EmbodimentRequirement, PackageManifest, ResourceBudget,
    SignatureState,
};
use skynet_contracts::certification::{
    award_certification, CertificationInput, SctsOutcome, SctsResult,
};
use skynet_contracts::ota::{admit_bundle, BundleManifest, UpdateSubject};
use skynet_json::Json;

use crate::capacity::Capacity;
use crate::embodiment::Body;

/// The eight §15 leaves to the runtime, in the order they are reported.
///
/// Taken from `skynet-cli`'s `NOT_COVERED` rather than restated, in spirit: these are exactly the
/// tests that command lists as beyond it. Keeping the two lists the same length is the check that
/// nothing falls between them — a test neither command covers is a test nobody runs, and it would
/// be invisible from either side.
pub const RUNTIME_TESTS: [&str; 8] = [
    "safety.kernel.parity",
    "safety.policy.isolation",
    "package.signature.required",
    "package.grants.gate",
    "ota.safety_gate",
    "telemetry.replay.deterministic",
    "security.identity.attested",
    "security.credential.scope",
];

/// What this robot concluded about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// One entry per [`RUNTIME_TESTS`], in that order.
    pub results: Vec<SctsResult>,
    /// How many were actually executed rather than reported as skipped.
    pub executed: usize,
}

impl Run {
    /// The tier this robot's **own** evidence supports, claiming the most it could.
    ///
    /// Claiming `certified` rather than the tier the robot thinks it is, on purpose: this asks
    /// "what does the evidence support", and the answer is only interesting if nothing capped it
    /// beforehand. §15's rule is that the evidence caps the award rather than the claim, so a low
    /// claim would produce a low award for a reason that has nothing to do with the machine.
    ///
    /// **Never a certificate.** This is what a robot can say about itself from what it ran, and it
    /// omits every bench and field result — which is most of the suite — so the answer is always at
    /// or below what a real award would be. §19 keeps it that way from the other end: a robot
    /// verifies and never signs, so nothing here is evidence anybody else has to accept.
    pub fn attainable(
        &self,
        simulator: bool,
    ) -> skynet_contracts::certification::CertificationVerdict {
        award_certification(&CertificationInput {
            claimed: "certified".to_string(),
            results: self.results.clone(),
            // Not `Some(1.0)`. No fleet data exists on the robot that produced it, and §15's floor
            // is checked against this — a runner substituting a flattering 1.0 would be awarding on
            // evidence nobody collected, which is the sentence `conformance_of` is written around.
            field_conformance: None,
            bench_age_days: None,
            simulator,
        })
    }
}

/// A result that was measured.
fn ran(id: &str, outcome: SctsOutcome, at: &str, detail: impl Into<String>) -> SctsResult {
    SctsResult {
        test_id: id.to_string(),
        outcome,
        value: None,
        ran_at: at.to_string(),
        detail: Some(detail.into()),
    }
}

/// A result that was not, and why.
///
/// `ran_at` is **empty**, and that is not a formality: §15 decays an award from the age of the
/// oldest bench result, and a skipped test carrying today's date would be a gap that looked fresh.
fn skipped(id: &str, detail: impl Into<String>) -> SctsResult {
    SctsResult {
        test_id: id.to_string(),
        outcome: SctsOutcome::Skipped,
        value: None,
        ran_at: String::new(),
        detail: Some(detail.into()),
    }
}

/// Run the eight.
///
/// `at` is an ISO-8601 timestamp from §35.1's correlation clock — the one thing wall time is for
/// here, because a result is a record a Hub places against other robots' records. Nothing in this
/// module decides anything with it.
pub fn run(capacity: &Capacity, body: &Body, at: &str) -> Run {
    let mut results = Vec::with_capacity(RUNTIME_TESTS.len());

    results.push(skipped(
        "safety.kernel.parity",
        "an edition property rather than a machine one: it asks whether this implementation \
         reproduces §31.4's conformance corpus, which `cargo test -p skynet-corpus` measures at \
         build time. A robot cannot run it, and a robot reporting it would be reporting on its \
         compiler.",
    ));

    results.push(skipped(
        "safety.policy.isolation",
        "a sandboxed package holding an actuator descriptor is refused — and nothing on this \
         edition sandboxes a package, because no policy runtime exists to sandbox one. This is the \
         one entry in this list whose stated reason has not gone stale.",
    ));

    results.push(signature_required(capacity, at));
    results.push(skipped(
        "package.grants.gate",
        "half of this is built and it is not the half the test measures. Admission refuses a \
         package requesting a capability the operator did not grant, at install (§8). The test is \
         about a package that *commands* one at run time and the `mode_gate` guard refusing it — \
         which is the Safety Kernel's, through a path §33.1 denies this daemon.",
    ));

    results.push(ota_safety_gate(at));

    results.push(skipped(
        "telemetry.replay.deterministic",
        "the black box, the sealing and `replay()` are all written, and what is missing between \
         them is the resolved limits: replay is a function of the recording *and* the envelope, \
         the profile and the transport clamp it ran under, and nothing on this daemon calls \
         `resolve_limits` to produce them. Handing replay the wrong limits would be a replay that \
         diverged for a reason that is not the robot's.",
    ));

    results.push(skipped(
        "security.identity.attested",
        "the descriptor's digest and the running image are reported at every check-in (§10), and \
         what `attested` asks for is a key bound to a TPM or a secure element. No key is generated \
         on a robot on this edition, so there is nothing to bind.",
    ));

    results.push(skipped(
        "security.credential.scope",
        "answerable, and not by this machine. A credential lifted from this robot cannot read \
         repositories or write to another namespace *if the Hub enforces the scopes on it*, and \
         only the Hub can measure that. This robot can say which scopes its credential carries — \
         `GET /identity` does — and a robot reporting a pass here would be reporting on somebody \
         else's enforcement.",
    ));

    let executed = results
        .iter()
        .filter(|result| result.outcome != SctsOutcome::Skipped)
        .count();
    let _ = body;
    Run { results, executed }
}

/// `package.signature.required` — *"An unsigned package, and one whose bytes do not match its
/// digest, are both refused."*
///
/// Run against this robot's **real** capacity rather than a synthetic one, so the answer is about
/// this machine. Two synthetic manifests, because the test names two refusals and a runner that
/// checked one would be reporting a pass it had half measured.
///
/// A robot whose capacity could not be assembled reports `skipped`: admission over an invented
/// capacity would be a conformance result about a robot that does not exist.
fn signature_required(capacity: &Capacity, at: &str) -> SctsResult {
    let id = "package.signature.required";
    let Some(capacity) = capacity.known() else {
        return skipped(
            id,
            "this robot's capacity could not be assembled, so admission cannot be run against it. \
             Running it against an invented capacity would produce a conformance result about a \
             machine that does not exist (`GET /capacity` says what is missing).",
        );
    };

    let digest = "a".repeat(64);
    let manifest = PackageManifest {
        schema_version: 1,
        id: "skynet/conformance-probe".to_string(),
        version: "0.0.0".to_string(),
        kind: "policy".to_string(),
        digest: format!("sha256:{digest}"),
        isolation: "sandboxed".to_string(),
        budget: ResourceBudget::default(),
        embodiment: EmbodimentRequirement::default(),
        requests: Vec::new(),
        runtime: None,
        depends_on: Vec::new(),
        min_os_version: None,
        source_embodiment: None,
    };

    let unsigned = admit_package(&AdmissionParams {
        manifest: manifest.clone(),
        capacity: capacity.clone(),
        signature: SignatureState::Absent,
        observed_digest: format!("sha256:{digest}"),
        transport_verdict: None,
        executes_on_load: false,
        trust_remote_code: false,
    });
    let mismatched = admit_package(&AdmissionParams {
        manifest,
        capacity: capacity.clone(),
        signature: SignatureState::Verified,
        // The bytes hash to something else, which is the second half of the test.
        observed_digest: format!("sha256:{}", "b".repeat(64)),
        transport_verdict: None,
        executes_on_load: false,
        trust_remote_code: false,
    });

    // Both halves have to refuse, and `admit_package` refuses both. The gap is downstream of it:
    // §37.1 puts admission between `verify` and `stage`, and on this edition it **reports** there
    // rather than gating, for every kind but a driver.
    if !unsigned.admitted && !mismatched.admitted {
        return ran(
            id,
            SctsOutcome::Fail,
            at,
            "admission refuses both — an unsigned package and one whose bytes do not match its \
             digest — and the install proceeds anyway. §37.1's `admit` stage reports on this \
             edition rather than gating, for every package kind but a driver (§19), because \
             nothing on a robot can produce a signature and refusing every locally-built package \
             would refuse the golden path. The decision is correct and the enforcement is absent, \
             and this test measures the enforcement.",
        );
    }

    ran(
        id,
        SctsOutcome::Fail,
        at,
        format!(
            "admission did not refuse both halves: unsigned admitted={}, digest-mismatched \
             admitted={}. Both must be refused (§15).",
            unsigned.admitted, mismatched.admitted
        ),
    )
}

/// `ota.safety_gate` — *"An update changing the safety kernel or the envelope is refused without a
/// fresh bench pass. OTA must not be a route around certification."*
///
/// The one test in this list this edition passes outright, and it is worth saying why it is a real
/// check rather than a restatement: `admit_bundle` is the function §12 gates on, this runs the
/// exact function the updater runs, over a bundle built to be the case the test names, against a
/// subject holding no bench pass. A pass here is *this robot's installed updater refusing it*.
fn ota_safety_gate(at: &str) -> SctsResult {
    let id = "ota.safety_gate";
    let bundle = BundleManifest {
        version: "9.9.9".to_string(),
        channel: "stable".to_string(),
        digest: format!("sha256:{}", "c".repeat(64)),
        size: None,
        compatible: Vec::new(),
        // The two components §12 gates on, together, because the test names both.
        changes: alloc_changes(),
    };
    let verdict = admit_bundle(
        &bundle,
        &UpdateSubject {
            running_version: "0.0.0".to_string(),
            board: None,
            channel: "stable".to_string(),
            // Verified, so the refusal that follows is about the gate and not about the signature.
            // A probe that failed for two reasons would measure neither.
            signature: SignatureState::Verified,
            bench: None,
        },
    );

    if verdict.admitted {
        return ran(
            id,
            SctsOutcome::Fail,
            at,
            "this robot's updater admitted an image changing the safety kernel and the envelope \
             with no bench pass behind it. OTA is then a route around certification (§12).",
        );
    }
    ran(
        id,
        SctsOutcome::Pass,
        at,
        format!(
            "an image changing the safety kernel and the envelope, with no bench pass, is refused \
             by the same `admit_bundle` the updater runs: {}",
            verdict
                .faults
                .iter()
                .map(|fault| fault.code.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}

/// The two components §12 gates on.
fn alloc_changes() -> Vec<String> {
    vec!["safety_kernel".to_string(), "envelope".to_string()]
}

/// The run, for the API and for a terminal.
pub fn to_json(run: &Run, simulator: bool) -> Json {
    let verdict = run.attainable(simulator);

    let mut results = Json::arr();
    for result in &run.results {
        let mut row = Json::obj();
        row.set("test_id", Json::from(result.test_id.as_str()))
            .set("outcome", Json::from(result.outcome.as_str()))
            .set(
                "detail",
                match &result.detail {
                    Some(detail) => Json::from(detail.as_str()),
                    None => Json::Null,
                },
            )
            .set(
                "ran_at",
                if result.ran_at.is_empty() {
                    Json::Null
                } else {
                    Json::from(result.ran_at.as_str())
                },
            );
        results.push(row);
    }

    let mut out = Json::obj();
    out.set("results", results)
        .set("executed", Json::Num(run.executed as f64))
        .set("of", Json::Num(RUNTIME_TESTS.len() as f64))
        .set("attainable", Json::from(verdict.awarded.as_str()))
        .set(
            "failures",
            Json::Arr(
                verdict
                    .failures
                    .iter()
                    .map(|id| Json::from(id.as_str()))
                    .collect(),
            ),
        )
        .set(
            "missing",
            Json::Arr(
                verdict
                    .missing
                    .iter()
                    .map(|id| Json::from(id.as_str()))
                    .collect(),
            ),
        )
        .set(
            "reasons",
            Json::Arr(
                verdict
                    .reasons
                    .iter()
                    .map(|reason| Json::from(reason.as_str()))
                    .collect(),
            ),
        )
        // Said on every run. §19 is the reason and it is not a limitation of this edition: a robot
        // verifies and never signs, so a result produced here is a robot's report about itself and
        // not a certificate. The Hub is what awards a tier, from bench evidence a human witnessed
        // (§15) — and this run carries none of that, which is why `attainable` is at or below what
        // a real award would be, always.
        .set(
            "note",
            Json::from(
                "These are the eight §15 tests that are properties of the runtime rather than of a \
                 driver — the ones `skynet driver check` lists as not covered. A result here is \
                 this robot reporting on itself: it is not signed, it is not a certificate, and it \
                 carries no bench or field evidence. §19: a robot verifies and never signs.",
            ),
        );
    out
}

/// The run, for a terminal.
pub fn to_lines(run: &Run, simulator: bool) -> Vec<String> {
    let verdict = run.attainable(simulator);
    let mut lines = Vec::new();
    lines.push(format!(
        "conformance: {} of {} runtime tests executed; the evidence here supports `{}`",
        run.executed,
        RUNTIME_TESTS.len(),
        verdict.awarded
    ));
    for result in &run.results {
        lines.push(format!(
            "  {:<32} {}",
            result.test_id,
            result.outcome.as_str()
        ));
        if let Some(detail) = &result.detail {
            lines.push(format!("      {detail}"));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use skynet_contracts::admission::DeviceCapacity;

    fn capacity() -> Capacity {
        Capacity::Known(Box::new(DeviceCapacity {
            ram_mb: 8192.0,
            os_version: "0.1.0".to_string(),
            ..Default::default()
        }))
    }

    fn outcome(run: &Run, id: &str) -> SctsOutcome {
        run.results
            .iter()
            .find(|result| result.test_id == id)
            .unwrap_or_else(|| panic!("{id} is not in the run"))
            .outcome
    }

    #[test]
    fn every_runtime_test_gets_exactly_one_entry_in_order() {
        let run = run(&capacity(), &Body::Absent, "2026-09-02T00:00:00Z");
        let ids: Vec<&str> = run
            .results
            .iter()
            .map(|result| result.test_id.as_str())
            .collect();
        assert_eq!(ids, RUNTIME_TESTS);
    }

    /// The same eight `skynet driver check` says it cannot cover. A test in one list and not the
    /// other is a test nobody runs, and it would be invisible from either side.
    #[test]
    fn the_two_lists_are_the_same_eight() {
        let mut mine: Vec<&str> = RUNTIME_TESTS.to_vec();
        mine.sort();
        // Restated here rather than imported, because `skynet-cli` is a binary crate this one does
        // not depend on — so the check is that a human keeping two lists in step has not slipped,
        // which is exactly the failure mode a shared constant would remove and a comment would not.
        let mut theirs = vec![
            "safety.kernel.parity",
            "safety.policy.isolation",
            "package.signature.required",
            "package.grants.gate",
            "ota.safety_gate",
            "telemetry.replay.deterministic",
            "security.identity.attested",
            "security.credential.scope",
        ];
        theirs.sort();
        assert_eq!(mine, theirs);
    }

    /// The uncomfortable one. This edition stages an unsigned package, so it fails a guest-tier
    /// test, and the runner says so rather than reporting the gap as unmeasured.
    #[test]
    fn an_unsigned_package_installs_so_this_edition_fails_its_own_guest_bar() {
        let run = run(&capacity(), &Body::Absent, "2026-09-02T00:00:00Z");
        assert_eq!(
            outcome(&run, "package.signature.required"),
            SctsOutcome::Fail
        );
        let detail = run.results[2].detail.clone().unwrap_or_default();
        assert!(
            detail.contains("reports on this edition rather than gating"),
            "{detail}"
        );
        assert!(detail.contains("§19"), "{detail}");
    }

    /// The one it passes, and it is a real refusal by the function the updater runs.
    #[test]
    fn the_safety_gate_refuses_a_kernel_change_with_no_bench_pass() {
        let run = run(&capacity(), &Body::Absent, "2026-09-02T00:00:00Z");
        assert_eq!(outcome(&run, "ota.safety_gate"), SctsOutcome::Pass);
        assert!(run.results[4]
            .detail
            .as_ref()
            .is_some_and(|detail| detail.contains("bench")));
    }

    #[test]
    fn two_of_the_eight_are_executed_and_six_are_named() {
        let run = run(&capacity(), &Body::Absent, "2026-09-02T00:00:00Z");
        assert_eq!(run.executed, 2);
        for result in &run.results {
            assert!(
                result.detail.as_ref().is_some_and(|d| !d.is_empty()),
                "{} carries no sentence, and a skipped test with no reason is the shape §15's \
                 first rule exists against",
                result.test_id
            );
        }
    }

    /// §15's first rule, from the runner's side: a skipped test is dated with nothing, because an
    /// award decays from the age of the oldest result and a gap that looked fresh would postpone
    /// the decay it should be causing.
    #[test]
    fn a_skipped_test_carries_no_date() {
        let run = run(&capacity(), &Body::Absent, "2026-09-02T00:00:00Z");
        for result in &run.results {
            if result.outcome == SctsOutcome::Skipped {
                assert!(result.ran_at.is_empty(), "{}", result.test_id);
            } else {
                assert_eq!(result.ran_at, "2026-09-02T00:00:00Z");
            }
        }
    }

    /// A robot whose capacity is unknown cannot run admission, and says so rather than guessing at
    /// one — the same rule `crate::capacity` applies to the install path.
    #[test]
    fn a_robot_with_no_capacity_skips_the_admission_check_rather_than_inventing_one() {
        let run = run(
            &Capacity::Unavailable {
                detail: "/proc/meminfo could not be read".to_string(),
            },
            &Body::Absent,
            "2026-09-02T00:00:00Z",
        );
        assert_eq!(
            outcome(&run, "package.signature.required"),
            SctsOutcome::Skipped
        );
        assert_eq!(run.executed, 1, "only the safety gate ran");
    }

    /// The award over this robot's own evidence is never a certificate, and with six of eight
    /// skipped and one failed it cannot be anything but the floor.
    #[test]
    fn the_attainable_tier_is_capped_by_what_was_not_run() {
        let run = run(&capacity(), &Body::Absent, "2026-09-02T00:00:00Z");
        let verdict = run.attainable(false);
        assert_ne!(verdict.awarded, "certified");
        assert!(
            verdict
                .missing
                .contains(&"safety.estop.latency".to_string())
                || !verdict.missing.is_empty(),
            "every bench test is missing from a run that took none: {:?}",
            verdict.missing
        );
        assert!(verdict
            .failures
            .contains(&"package.signature.required".to_string()));
    }

    #[test]
    fn the_wire_says_it_is_not_a_certificate() {
        let run = run(&capacity(), &Body::Absent, "2026-09-02T00:00:00Z");
        let document = to_json(&run, false);
        assert_eq!(document.arr_or_empty("results").len(), 8);
        assert_eq!(document.get("executed"), Some(&Json::Num(2.0)));
        assert!(document
            .get("note")
            .and_then(Json::as_str)
            .is_some_and(|note| note.contains("never signs")));
        // A skipped test's date is null on the wire rather than an empty string, so a consumer
        // sorting by it does not sort eight empty strings to the top.
        assert_eq!(
            document.arr_or_empty("results")[0].get("ran_at"),
            Some(&Json::Null)
        );
    }
}
