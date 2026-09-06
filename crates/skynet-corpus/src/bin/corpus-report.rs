//! Print this edition's corpus result as JSON, for `tools/edition-report.mjs`.
//!
//! The Rust half of what `run_corpus.py --json` does for the Python edition. Both exist for one
//! reason: `/skynet/os/editions` renders a number per edition, and a number on a page has to come
//! from a run rather than from somebody typing it into a template. §32.2's rule about the console —
//! *it must never show a value it did not receive* — is not weaker on the Hub.
//!
//! ```text
//! cargo run -p skynet-corpus --bin corpus-report
//! ```
//!
//! Exits non-zero when the edition does not reproduce the corpus, so a report can never be
//! generated from a failing run without somebody choosing to ignore the exit code.

use skynet_json::Json;

fn main() -> std::process::ExitCode {
    let root = skynet_corpus::default_root();
    if !root.is_dir() {
        eprintln!("no corpus at {}", root.display());
        return std::process::ExitCode::from(2);
    }

    let report = skynet_corpus::run(&root);

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
