//! Print this edition's corpus result as JSON, for `tools/edition-report.mjs`.
//!
//! The Rust half of what `run_corpus.py --json` does for the Python edition. Both exist for one
//! reason: `/skynet/os/editions` renders a number per edition, and a number on a page has to come
//! from a run rather than from somebody typing it into a template. §32.2's rule about the console —
//! *it must never show a value it did not receive* — is not weaker on the Hub.
//!
//! ```text
//! cargo run -p skynet-corpus --bin corpus-report
//! cargo run -p skynet-corpus --bin corpus-report -- --results parity.json
//! ```
//!
//! `--results` also writes the run as §15's `safety.kernel.parity`, in the results document
//! `skynet driver check --results` and `skynet conformance --results` write, so the one guest test
//! that is an edition's rather than a driver's or a robot's reaches `skynet bench award` too
//! (`skynet_corpus::parity`). The failure is written as faithfully as the pass: evidence that an
//! edition diverged is evidence.
//!
//! Exits non-zero when the edition does not reproduce the corpus, so a report can never be
//! generated from a failing run without somebody choosing to ignore the exit code.

use std::path::PathBuf;

use skynet_contracts::bench::{results_from_json, results_to_json, ResultsDocument};
use skynet_json::Json;

/// How this binary is re-run, which is what a results document's `source` is for.
const SOURCE: &str = "cargo run -p skynet-corpus --bin corpus-report";

fn main() -> std::process::ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let results = match results_target(&argv) {
        Ok(target) => target,
        Err(message) => {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    };

    let root = skynet_corpus::default_root();
    if !root.is_dir() {
        eprintln!("no corpus at {}", root.display());
        return std::process::ExitCode::from(2);
    }

    let report = skynet_corpus::run(&root);

    // Before the report is printed, so a write that fails is a refusal and not a line after a
    // clean-looking report that somebody scrolls past.
    if let Some(target) = results {
        if let Err(message) = write_parity(&report, &target) {
            eprintln!("{message}");
            return std::process::ExitCode::from(2);
        }
    }

    // The same fields the Python runner emits, named the same way. Two editions reporting the same
    // fact under two names would be two facts to a reader.
    let mut out = Json::obj();
    out.set("passed", Json::Num(report.passed as f64));
    out.set("failed", Json::Num(report.failures.len() as f64));
    out.set("unimplemented", Json::Num(report.unsupported.len() as f64));
    out.set(
        "total",
        Json::Num((report.passed + report.failures.len() + report.unsupported.len()) as f64),
    );
    out.set("files", Json::Num(report.files as f64));
    let mut unsupported: Vec<String> = report.unsupported.clone();
    unsupported.sort();
    out.set(
        "missing_handlers",
        Json::Arr(unsupported.into_iter().map(Json::Str).collect()),
    );

    match out.to_canonical() {
        Ok(text) => println!("{text}"),
        Err(error) => {
            eprintln!("the report could not be serialised: {error:?}");
            return std::process::ExitCode::from(2);
        }
    }

    if report.ok() {
        std::process::ExitCode::SUCCESS
    } else {
        eprintln!(
            "{} failure(s), {} unimplemented — this edition does not reproduce the corpus",
            report.failures.len(),
            report.unsupported.len(),
        );
        std::process::ExitCode::FAILURE
    }
}

/// `--results <file>`, the only argument this binary takes.
///
/// Anything else is refused rather than ignored: `tools/edition-report.mjs` runs this with no
/// arguments, and a typo that silently produced no file would be a missing result nobody noticed
/// was missing.
fn results_target(argv: &[String]) -> Result<Option<PathBuf>, String> {
    match argv {
        [] => Ok(None),
        [flag, path] if flag == "--results" && !path.is_empty() && !path.starts_with("--") => {
            Ok(Some(PathBuf::from(path)))
        }
        [flag] if flag == "--results" => Err(
            "--results was given without a file. Name one — `--results parity.json` — or omit it."
                .to_string(),
        ),
        _ => Err(format!(
            "corpus-report takes no arguments but `--results <file>`, and was given {argv:?}."
        )),
    }
}

/// Write the run as `safety.kernel.parity`, read back first by the reader that will judge it.
fn write_parity(report: &skynet_corpus::Report, target: &std::path::Path) -> Result<(), String> {
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| {
            "this machine's clock reads before 1970, so the result cannot be dated. §15 refuses an \
             undated result, so nothing was written."
                .to_string()
        })?
        .as_millis() as u64;
    let result = skynet_corpus::parity::parity_result(
        report,
        concat!(
            "the Rust edition, skynet-corpus ",
            env!("CARGO_PKG_VERSION")
        ),
        &skynet_contracts::time::rfc3339(unix_ms),
    );
    let document = results_to_json(&ResultsDocument {
        source: SOURCE.to_string(),
        results: vec![result],
    });
    results_from_json(&document)
        .map_err(|error| format!("the parity result would not read back: {error}"))?;
    let text = document
        .to_canonical()
        .map_err(|error| format!("the parity result could not be serialised: {error:?}"))?;
    std::fs::write(target, format!("{text}\n"))
        .map_err(|error| format!("cannot write {}: {error}", target.display()))
}
