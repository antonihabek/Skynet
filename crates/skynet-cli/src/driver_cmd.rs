//! `skynet driver new` and `skynet driver check` — §63.1 and §63.3.

use std::path::{Path, PathBuf};

use skynet_contracts::bench::{results_from_json, results_to_json, ResultsDocument};
use skynet_contracts::certification::{SctsOutcome, SctsResult, SCTS_TESTS};

use crate::args::{Args, Spec};
use crate::check;
use crate::scaffold;
use crate::term;
use skynet_dl::Library;

/// What `--help` prints for this command.
pub const USAGE: &str = "\
skynet driver new <name> [--lang c|rust] [--dir <path>]
skynet driver check <library> [--config <json>]

  new    Scaffold a driver that builds, loads, and fails conformance in exactly
         the places where only you know the answer. The failures are the to-do
         list, and each one carries the SCTS id it stands for.

  check  Run the driver-facing sim tests against a built shared object — the ABI,
         state completeness, command rejection, descriptor validity, the estop
         path. Turning up to a bench session with these already green is what
         makes bench time cheap.

         --config <json>   passed verbatim to the driver's open(). Defaults to
                           an empty string.

         --results <f>     write these results to <f> as a results document
                           (§15), so the sim half of an award can be carried to
                           wherever `skynet bench award` computes one. Until
                           this existed the results were rendered for a person
                           and kept.

         --descriptor <f>  write the body this driver describes to <f>, in the
                           canonical form §28 pins at state/embodiment.json. This
                           is how a bench robot is provisioned from its own
                           driver: §33.1 forbids `skynetd` loading one to ask, and
                           this command has already loaded it to check it.

         The sim tests that are properties of the runtime rather than of a driver
         are listed as not covered, by name, on every run.

  Exit code for `check` is 0 only when nothing failed and nothing went unmeasured:
  absence is not a pass (§15).";

const NEW_SPEC: Spec = Spec {
    values: &["lang", "dir"],
    switches: &["force"],
};

const CHECK_SPEC: Spec = Spec {
    values: &["config", "descriptor", "results"],
    switches: &[],
};

/// Dispatch a `driver` subcommand.
pub fn run(rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("new") => new(&rest[1..]),
        Some("check") => run_check(&rest[1..]),
        Some("--help") | Some("-h") | None => {
            println!("{USAGE}");
            0
        }
        Some(other) => term::fail(&format!(
            "`driver {other}` is not a subcommand. They are `new` and `check`."
        )),
    }
}

fn new(rest: &[String]) -> i32 {
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return 0;
    }
    let args = match Args::parse(rest, &NEW_SPEC) {
        Ok(args) => args,
        Err(message) => return term::fail(&message),
    };
    let [name] = args.positional.as_slice() else {
        return term::fail(&format!(
            "expected exactly one driver name, got {}.\n\n{USAGE}",
            args.positional.len()
        ));
    };

    let language = args.value("lang").unwrap_or("c");
    let target: PathBuf = match args.value("dir") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(name),
    };

    if target.exists() {
        let occupied = std::fs::read_dir(&target)
            .map(|entries| entries.count() > 0)
            .unwrap_or(false);
        if occupied && !args.flag("force") {
            return term::fail(&format!(
                "{} already exists and is not empty. Pick another name, or pass --force to write \
                 into it anyway — which overwrites files with the same names and leaves everything \
                 else alone.",
                target.display()
            ));
        }
    }

    let header = scaffold::header_path(&target);
    let files = match scaffold::build(name, language, &header) {
        Ok(files) => files,
        Err(message) => return term::fail(&message),
    };

    for file in &files {
        let path = target.join(&file.path);
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                return term::fail(&format!("cannot create {}: {err}", parent.display()));
            }
        }
        if let Err(err) = std::fs::write(&path, &file.body) {
            return term::fail(&format!("cannot write {}: {err}", path.display()));
        }
    }

    use std::io::Write;
    let out = &mut std::io::stderr();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {} — {} files in {}",
        term::bold(name),
        files.len(),
        target.display()
    );
    for file in &files {
        let _ = writeln!(out, "    {}", term::dim(&file.path.display().to_string()));
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "  {}", term::bold("Next"));
    let build_line = if language == "rust" {
        "cargo build --release"
    } else {
        "make"
    };
    let _ = writeln!(out, "    cd {} && {build_line}", target.display());
    let _ = writeln!(out, "    skynet driver check <the built library>");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {}",
        term::wrap(
            "It will fail `embodiment.roles.coverage` at 0% and report \
             `embodiment.descriptor.signed` as not measured. Both are correct on day one: roles are \
             yours to assign, and a driver cannot sign its own descriptor. Everything else marked \
             TODO in the source is the rest of the list.",
            2
        )
    );
    let _ = writeln!(out);
    0
}

fn run_check(rest: &[String]) -> i32 {
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return 0;
    }
    let args = match Args::parse(rest, &CHECK_SPEC) {
        Ok(args) => args,
        Err(message) => return term::fail(&message),
    };
    let [path] = args.positional.as_slice() else {
        return term::fail(&format!(
            "expected exactly one library path, got {}.\n\n{USAGE}",
            args.positional.len()
        ));
    };

    let path = Path::new(path);
    if !path.exists() {
        return term::fail(&format!(
            "{} does not exist. Build the driver first; `skynet driver new` writes a build file \
             that produces the right kind of artifact for your platform.",
            path.display()
        ));
    }

    let library = match Library::open(path) {
        Ok(library) => library,
        Err(err) => return term::fail(&err.to_string()),
    };

    let report = check::run(&library, args.value("config").unwrap_or(""));

    // Before the report is printed, so a write that fails is a refusal rather than a line after a
    // clean run that somebody scrolls past.
    if let Some(target) = args.value("descriptor") {
        if let Err(message) = write_descriptor(&report, Path::new(target)) {
            return term::fail(&message);
        }
    }
    if let Some(target) = args.value("results") {
        if let Err(message) = write_results(&report, path, Path::new(target)) {
            return term::fail(&message);
        }
    }

    check::print(&report)
}

/// Write what this run measured, as a results document (§15).
///
/// # Why this is worth a flag
///
/// The command computed a verdict per SCTS id and then rendered it for a person. That is this
/// repository's four-times defect — a value that is correct, tested and reached by nothing anybody
/// downstream reads — and it is the reason an award computed from a bench record would have had no
/// sim half to compute it against. `skynet bench award` is the reader.
///
/// # What does not go in it
///
/// Only ids §15 has, and only those this run actually reached. `NOT_COVERED` is deliberately absent
/// rather than written as `skipped`: those tests are properties of the runtime and of the edition,
/// this command has said so by name on every run since it existed, and a document listing them
/// would be this command reporting on tests it is not the runner for. An id with no entry here is
/// `missing` to an award, which is exactly what it is.
fn write_results(report: &check::Report, library: &Path, target: &Path) -> Result<(), String> {
    let mut results: Vec<SctsResult> = Vec::new();
    for test in SCTS_TESTS.iter().filter(|test| test.suite == "sim") {
        let Some(outcome) = report.outcome_for(test.id) else {
            continue;
        };
        let outcome = match outcome {
            check::Outcome::Pass => SctsOutcome::Pass,
            check::Outcome::Fail => SctsOutcome::Fail,
            check::Outcome::NotApplicable => SctsOutcome::NotApplicable,
            // §15's own word for it, and the one `award_certification` reads as `missing`. A check
            // this command could not run is not a check that passed.
            check::Outcome::Missing => SctsOutcome::Skipped,
        };
        let ran_at = if outcome == SctsOutcome::Skipped {
            String::new()
        } else {
            now()
        };
        results.push(SctsResult {
            test_id: test.id.to_string(),
            outcome,
            // §15 marks one sim test quantitative, and `CheckResult::with_value` is where the
            // number survives the sentence it used to be spent on.
            value: report
                .results
                .iter()
                .find(|result| result.id == test.id && result.value.is_some())
                .and_then(|result| result.value),
            ran_at,
            detail: report
                .results
                .iter()
                .find(|result| result.id == test.id && !result.detail.is_empty())
                .map(|result| result.detail.clone()),
        });
    }
    if results.is_empty() {
        return Err(format!(
            "this run reached no SCTS test at all, so there is nothing to write to {}. The report \
             below says why — a driver that will not load is the usual reason.",
            target.display()
        ));
    }

    let document = results_to_json(&ResultsDocument {
        source: format!("skynet driver check {}", library.display()),
        results,
    });
    // Read back by the reader that will judge it, for `bench_cmd`'s reason: a writer that can emit
    // a file its own parser refuses is two implementations of one format.
    results_from_json(&document)
        .map_err(|err| format!("this document would not read back: {err}"))?;
    let text = document
        .to_canonical()
        .map_err(|error| format!("the results could not be serialised: {error:?}"))?;
    std::fs::write(target, format!("{text}\n"))
        .map_err(|err| format!("{}: {err}", target.display()))
}

/// Now, RFC 3339, from the one clock this workspace reads.
fn now() -> String {
    skynet_contracts::time::rfc3339(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or(0),
    )
}

/// Write the descriptor the driver returned, in the canonical form §28 pins.
///
/// # Why this is a flag on `check` and not a command of its own
///
/// Getting the descriptor means loading the driver and calling `describe()`, which is exactly what
/// `check` has already done by the time it has an answer. A second command would load the same
/// shared object a second time to ask the same question — and would be a route that returns a body
/// **without** having checked that the vtable agrees with it, which is the one thing that makes a
/// descriptor worth writing down.
///
/// # Canonical, because the digest is over the canonical form
///
/// §28 pins `state/embodiment.json` and pairs it with a detached signature over its digest. A file
/// written with any other key order hashes differently, so a descriptor pretty-printed here would
/// be one no signature could ever match.
fn write_descriptor(report: &check::Report, target: &Path) -> Result<(), String> {
    let Some(descriptor) = report.descriptor.as_ref() else {
        return Err(format!(
            "this driver returned no descriptor that decodes, so there is nothing to write to {}. \
             The report below says why; `embodiment.descriptor.valid` is the line that matters.",
            target.display()
        ));
    };

    let text = descriptor
        .to_canonical()
        .map_err(|error| format!("the descriptor could not be serialised: {error:?}"))?;

    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
    }
    std::fs::write(target, format!("{text}\n"))
        .map_err(|error| format!("cannot write {}: {error}", target.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use skynet_json::Json;

    fn temp(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("skynet-driver-cmd-{}-{label}", std::process::id()))
    }

    /// The property the flag exists for: §28 pairs the descriptor with a detached signature over
    /// its digest, and a digest is over the **canonical** form. A file written any other way is one
    /// no signature could ever match.
    #[test]
    fn the_descriptor_is_written_canonically_with_a_trailing_newline() {
        let mut descriptor = Json::obj();
        descriptor.set("robot_slug", Json::from("mock-arm"));
        descriptor.set("base", Json::from("fixed"));
        descriptor.set("dof", Json::Num(3.0));
        // Out of order on purpose: canonical form sorts keys, so a writer that preserved insertion
        // order would pass a test whose fixture happened to be sorted already.
        descriptor.set("appended", Json::Bool(true));

        let report = check::Report {
            descriptor: Some(descriptor),
            ..Default::default()
        };
        let target = temp("canonical");
        let _ = std::fs::remove_file(&target);
        write_descriptor(&report, &target).expect("write");

        let written = std::fs::read_to_string(&target).expect("read");
        assert_eq!(
            written,
            "{\"appended\":true,\"base\":\"fixed\",\"dof\":3,\"robot_slug\":\"mock-arm\"}\n"
        );
        let _ = std::fs::remove_file(&target);
    }

    /// A driver whose descriptor did not decode has nothing to write, and saying so beats leaving a
    /// zero-byte file at the path §28 pins — which is the state a robot reads as *malformed* rather
    /// than as *absent*, and sends somebody to debug a generator that never ran.
    #[test]
    fn a_driver_with_no_descriptor_is_refused_rather_than_writing_an_empty_file() {
        let target = temp("absent");
        let _ = std::fs::remove_file(&target);

        let error = write_descriptor(&check::Report::default(), &target).unwrap_err();
        assert!(error.contains("embodiment.descriptor.valid"), "{error}");
        assert!(!target.exists(), "nothing should have been written");
    }

    /// The directory is created, because the path somebody passes is usually somewhere that does
    /// not exist yet — `/data/state/embodiment.json` on a robot being provisioned, for instance.
    #[test]
    fn a_parent_directory_that_does_not_exist_is_created() {
        let mut descriptor = Json::obj();
        descriptor.set("robot_slug", Json::from("mock-arm"));
        let report = check::Report {
            descriptor: Some(descriptor),
            ..Default::default()
        };

        let dir = temp("nested");
        let _ = std::fs::remove_dir_all(&dir);
        let target = dir.join("state").join("embodiment.json");
        write_descriptor(&report, &target).expect("write");
        assert!(target.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
