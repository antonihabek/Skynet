//! `safety.kernel.parity` as a result, rather than as a test that passed and was forgotten.
//!
//! §15 lists `safety.kernel.parity` as a `sim` test required from **guest**: *"the on-robot kernel
//! and the reference implementation return identical verdicts across the full cycle-vector
//! corpus."* For an edition that is exactly "this edition reproduces the corpus", and
//! `cargo test -p skynet-corpus` has measured it on every commit since the corpus existed.
//!
//! Nothing wrote it down. `skynet driver check` names it as an edition property and does not run
//! it; `skynet conformance` skips it on a robot for the same reason, and says so. So an award
//! computed from every document this platform can produce still read it as `missing`, and guest
//! was unreachable for one more reason than `DRY-COVERAGE-PLAN.md` F21 counted — found by building
//! the runtime's producer and reading what came back (F42).
//!
//! # What the result is about
//!
//! The source tree it was built from, and nothing else. A robot runs `skynetk` compiled from the
//! same crates this run exercised, and the result says which edition and which version — but it is
//! a results document and so bound to nothing, which is right for a run that costs seconds to
//! repeat and wrong to lean on across a rebuild of a different commit.

use skynet_contracts::certification::{SctsOutcome, SctsResult};

use crate::Report;

/// The SCTS id this module produces.
pub const TEST_ID: &str = "safety.kernel.parity";

/// What a corpus run concludes about `safety.kernel.parity`, measured at `ran_at`.
///
/// A pass only for a run that reproduced every case and implemented every handler: an edition that
/// skips a corpus file it does not recognise has not shown parity on it, which is the reason
/// [`Report::ok`] counts `unsupported` as a failure. An empty run is a failure too — a corpus of
/// nothing reproduces trivially, and vacuous truth is the failure mode of every conformance
/// predicate.
pub fn parity_result(report: &Report, edition: &str, ran_at: &str) -> SctsResult {
    let total = report.passed + report.failures.len() + report.unsupported.len();
    let (outcome, detail) = if report.ok() && report.passed > 0 {
        (
            SctsOutcome::Pass,
            format!(
                "{edition}: {} of {total} cases across {} corpus files reproduced byte for byte \
                 (§31.4) — the cycle vectors the kernel is held to, and everything else the corpus \
                 holds.",
                report.passed, report.files
            ),
        )
    } else if total == 0 {
        (
            SctsOutcome::Fail,
            format!(
                "{edition}: the corpus held no cases, so nothing was compared. A run over nothing \
                 is not parity."
            ),
        )
    } else {
        let first = report
            .failures
            .first()
            .map(|failure| format!("{} :: {}", failure.file, failure.id))
            .or_else(|| report.unsupported.first().cloned())
            .unwrap_or_default();
        (
            SctsOutcome::Fail,
            format!(
                "{edition}: {} of {total} cases diverged and {} are not implemented. The first is \
                 {first}; `cargo test -p skynet-corpus` prints each with the byte it differs at.",
                report.failures.len(),
                report.unsupported.len()
            ),
        )
    };
    SctsResult {
        test_id: TEST_ID.to_string(),
        outcome,
        value: None,
        ran_at: ran_at.to_string(),
        detail: Some(detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Failure;

    const AT: &str = "2026-09-18T09:00:00Z";

    fn clean(passed: usize) -> Report {
        Report {
            passed,
            failures: Vec::new(),
            unsupported: Vec::new(),
            files: 3,
        }
    }

    #[test]
    fn a_full_reproduction_is_a_dated_pass_that_says_how_much_was_compared() {
        let result = parity_result(&clean(1424), "the Rust edition", AT);
        assert_eq!(result.test_id, "safety.kernel.parity");
        assert_eq!(result.outcome, SctsOutcome::Pass);
        assert_eq!(result.ran_at, AT);
        let detail = result.detail.unwrap_or_default();
        assert!(detail.contains("1424 of 1424"), "{detail}");
    }

    /// The control for the pass: one divergence and the same run is a failure naming the case.
    #[test]
    fn one_divergent_case_fails_and_names_it() {
        let mut report = clean(1423);
        report.failures.push(Failure {
            file: "cycle/modes.json".to_string(),
            id: "a-single-mode-body-sends-no-per-joint-array".to_string(),
            note: String::new(),
            expected: "{}".to_string(),
            actual: "{\"x\":1}".to_string(),
        });
        let result = parity_result(&report, "the Rust edition", AT);
        assert_eq!(result.outcome, SctsOutcome::Fail);
        let detail = result.detail.unwrap_or_default();
        assert!(
            detail.contains("cycle/modes.json :: a-single-mode-body-sends-no-per-joint-array"),
            "{detail}"
        );
    }

    /// Not implementing a corpus is not reproducing it.
    #[test]
    fn an_unimplemented_handler_is_a_failure_not_a_smaller_pass() {
        let mut report = clean(1400);
        report.unsupported.push("watchdog/chains.json".to_string());
        let result = parity_result(&report, "the Rust edition", AT);
        assert_eq!(result.outcome, SctsOutcome::Fail);
        assert!(result
            .detail
            .unwrap_or_default()
            .contains("watchdog/chains.json"));
    }

    #[test]
    fn a_corpus_of_nothing_is_not_parity() {
        let result = parity_result(&clean(0), "the Rust edition", AT);
        assert_eq!(result.outcome, SctsOutcome::Fail);
    }
}
