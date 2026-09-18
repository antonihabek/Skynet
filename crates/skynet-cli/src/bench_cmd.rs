//! `skynet bench record` and `skynet bench award` — the two ends of §15's bench evidence.
//!
//! [`skynet_contracts::bench`] is the format. This is the pair of commands that writes one and
//! reads one, and between them they are the answer to a sentence `DRY-COVERAGE-PLAN.md` has carried
//! since Phase 0: *"until it exists a green `check-offline` is a property demonstrated and not
//! evidence recorded."*
//!
//! # Why a producer is a command rather than a library call
//!
//! The measurement is not made here and cannot be. §15's bench suite is a human, a robot and a day,
//! and the one case *Decisions* 4 carves out — `offline.control.independent`, which needs a real
//! machine rather than a real body — is measured by a shell script that puts the whole control loop
//! in a network namespace with nothing in it. What neither of those can do is hash the bytes they
//! ran against and write a document nothing downstream can misread.
//!
//! So `record` does exactly that and nothing else. It does not run a test, it does not decide an
//! outcome, and every field it writes came from an argument or from a file it read. The one thing
//! it computes is the digest, which is the one thing a person cannot.
//!
//! # Why the consumer re-hashes
//!
//! `award` reads the artefact list out of each record and hashes those files **again**, from disk,
//! before it will use a single result. §12 does this with a bundle and calls the failure
//! `BenchPassStale`; the same rule here means a rebuilt driver silently stops having bench evidence
//! rather than silently keeping it. That is the whole mechanism, and it is two lines of arithmetic
//! in [`skynet_contracts::bench::evidence_for`].
//!
//! # What this does not do
//!
//! It does not sign anything and cannot. §19 forbids a robot holding a signing key and §15.1 draws
//! the conclusion: the Hub awards, from evidence a person witnessed. What `award` prints is what
//! [`award_certification`] would say about this evidence — a pre-flight a vendor can run before
//! spending a day on a bench, not a certificate.

use std::path::{Path, PathBuf};

use skynet_contracts::bench::{
    bench_record_from_json, bench_record_to_json, evidence_for, results_from_json, BenchRecord,
    BenchSubject, Witness, BENCH_RECORD_SCHEMA, RESULTS_SCHEMA,
};
use skynet_contracts::certification::{
    award_certification, CertificationInput, SctsOutcome, SctsResult, SCTS_TESTS,
};
use skynet_contracts::digest::{subject_digest, DigestFile};
use skynet_json::Json;

use crate::args::{Args, Spec};
use crate::term;

/// What `--help` prints for this command.
pub const USAGE: &str = "\
skynet bench record <artefact>... --what <text> --test <id> --outcome <o>
                    (--human <name> | --machine <host>) --out <file>
                    [--value <n>] [--detail <text>] [--ran-at <rfc3339>]
                    [--root <dir>] [--append]
skynet bench award <file>... --claim <tier>
                    [--root <dir>] [--simulator] [--field-conformance <0..1>]

  record  Write down one bench measurement, bound to the bytes it was taken
          against. The artefacts are hashed here; everything else you say.

          --what        what those bytes are, for a person reading the record
          --test        the SCTS id (§15). It must be a `bench` test
          --outcome     pass | fail | skipped | not_applicable
          --value       the measured number, required for a quantitative pass
          --detail      the conditions of the measurement, or the failure
          --human       the person who was present (§15 prices a bench result
                        at one), or
          --machine     the machine, when nobody was — legitimate only for a
                        test that needs a machine rather than a body
          --ran-at      when it was measured. Defaults to now, and is printed
          --root        directory the artefact paths are relative to (default .)
          --append      add this result to an existing record, refusing if it
                        was taken against other bytes

  award   What §15 would award for the evidence in these files. Bench records
          are re-hashed against the bytes on disk and refused if they have
          moved; results documents (`skynet driver check --results`) are read
          as written, and may not carry bench results at all.

          --claim       the tier being claimed. The evidence caps the award
          --simulator   the body declares itself simulated (§35.3)
          --field-conformance   the fleet figure, when one exists

  Nothing here signs anything (§19). `award` is what a vendor can run before
  spending a day on a bench, not a certificate — the Hub awards (§15.1).

  Exit code for `award` is 0 only when the evidence supports the claim.";

const RECORD_SPEC: Spec = Spec {
    values: &[
        "what", "test", "outcome", "value", "detail", "human", "machine", "ran-at", "out", "root",
    ],
    switches: &["append"],
};

const AWARD_SPEC: Spec = Spec {
    values: &["claim", "root", "field-conformance"],
    switches: &["simulator"],
};

/// Dispatch a `bench` subcommand.
pub fn run(rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("record") => guarded(&rest[1..], record),
        Some("award") => guarded(&rest[1..], award),
        Some("--help") | Some("-h") | None => {
            println!("{USAGE}");
            0
        }
        Some(other) => term::fail(&format!(
            "`bench {other}` is not a subcommand. They are `record` and `award`."
        )),
    }
}

fn guarded(rest: &[String], run: fn(&[String]) -> i32) -> i32 {
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return 0;
    }
    run(rest)
}

// ── record ───────────────────────────────────────────────────────────────────

fn record(rest: &[String]) -> i32 {
    let args = match Args::parse(rest, &RECORD_SPEC) {
        Ok(args) => args,
        Err(message) => return term::fail(&message),
    };

    if args.positional.is_empty() {
        return term::fail(
            "no artefacts. A bench record is bound to the bytes it was taken against, so name \
             them: `skynet bench record ./libmock.so --what ...`.",
        );
    }
    let Some(out) = args.value("out") else {
        return term::fail("`--out <file>` is required — this command writes a record.");
    };

    let witness = match (args.value("human"), args.value("machine")) {
        (Some(name), None) => Witness::Human {
            name: name.to_string(),
        },
        (None, Some(host)) => Witness::Machine {
            host: host.to_string(),
        },
        (None, None) => {
            return term::fail(
                "say who was there: `--human <name>` when a person witnessed the measurement, \
                 which is what §15 prices a bench result at, or `--machine <host>` when nobody \
                 did. There is no default, because a record that will not say is not evidence.",
            )
        }
        (Some(_), Some(_)) => {
            return term::fail(
                "`--human` and `--machine` are the two answers to one question. Give one.",
            )
        }
    };

    let root = PathBuf::from(args.value("root").unwrap_or("."));
    let mut artefacts: Vec<DigestFile> = Vec::new();
    for path in &args.positional {
        match hash_file(&root, path) {
            Ok(file) => artefacts.push(file),
            Err(message) => return term::fail(&message),
        }
    }
    let digest = match subject_digest(&artefacts) {
        Ok(digest) => digest,
        Err(err) => {
            return term::fail(&format!(
                "these artefacts cannot be hashed as a subject: {err}"
            ))
        }
    };

    let result = match one_result(&args) {
        Ok(result) => result,
        Err(message) => return term::fail(&message),
    };

    // `--append` is how a session that measured several tests reaches one record without this
    // command growing a way to say several things at once. It re-reads what is there and refuses a
    // record taken against other bytes, so the binding is checked on the way in as well as on the
    // way out.
    let mut record =
        if args.flag("append") {
            match read_json(Path::new(out)).and_then(|document| {
                bench_record_from_json(&document).map_err(|err| format!("{out}: {err}"))
            }) {
                Ok(existing) if existing.subject.digest != digest => {
                    return term::fail(&format!(
                    "{out} was taken against {} and these artefacts hash to {digest}. Appending \
                     would put two measurements of different bytes in one record — write a new \
                     one.",
                    existing.subject.digest
                ))
                }
                Ok(existing) => existing,
                Err(message) => return term::fail(&message),
            }
        } else {
            if Path::new(out).exists() {
                return term::fail(&format!(
                "{out} already exists. Use `--append` to add this result to it, or name another \
                 file — overwriting a bench record silently would destroy a measurement that cost \
                 a day."
            ));
            }
            BenchRecord {
                subject: BenchSubject {
                    what: match args.value("what") {
                        Some(what) => what.to_string(),
                        None => return term::fail(
                            "`--what <text>` is required: one clause saying what these bytes are, \
                             for whoever reads the record without the command line beside them.",
                        ),
                    },
                    artefacts,
                    digest,
                },
                witness,
                ran_at: String::new(),
                results: Vec::new(),
            }
        };

    let ran_at = match args.value("ran-at") {
        Some(given) => given.to_string(),
        None => now(),
    };
    if !args.flag("append") {
        record.ran_at = ran_at.clone();
    }
    record.results.push(SctsResult {
        ran_at: if result.outcome == SctsOutcome::Skipped {
            String::new()
        } else {
            ran_at
        },
        ..result
    });

    // The producer's own document, read back by the reader that will judge it. A writer that can
    // emit a file its own parser refuses is two implementations of one format, and this is the
    // cheapest possible way not to be.
    let document = bench_record_to_json(&record);
    if let Err(err) = bench_record_from_json(&document) {
        return term::fail(&format!(
            "this record would not read back: {err}\n\nNothing was written."
        ));
    }
    let text = match document.to_canonical() {
        Ok(text) => text,
        Err(err) => return term::fail(&format!("the record will not serialise: {err:?}")),
    };
    if let Err(err) = std::fs::write(out, format!("{text}\n")) {
        return term::fail(&format!("{out}: {err}"));
    }

    println!(
        "  {} {out}",
        term::ok(if args.flag("append") {
            "appended"
        } else {
            "wrote"
        })
    );
    println!("  {} {}", term::dim("subject "), record.subject.digest);
    println!(
        "  {} {} artefact(s), {} result(s), witnessed by {}",
        term::dim("        "),
        record.subject.artefacts.len(),
        record.results.len(),
        record.witness.kind()
    );
    println!(
        "\n  {}",
        term::dim(
            "Unsigned, and it stays that way: §19 forbids a robot holding a signing key, so this \
             is a report about bytes rather than a certificate about a robot (§15.1)."
        )
    );
    0
}

/// The single result a `record` invocation carries.
fn one_result(args: &Args) -> Result<SctsResult, String> {
    let Some(test_id) = args.value("test") else {
        return Err("`--test <id>` is required. It is an SCTS id from §15.".to_string());
    };
    let Some(test) = SCTS_TESTS.iter().find(|test| test.id == test_id) else {
        return Err(format!(
            "`{test_id}` is not an SCTS test. §15 has {}, and `skynet driver check` prints every \
             id it touches.",
            SCTS_TESTS.len()
        ));
    };
    if test.suite != "bench" {
        return Err(format!(
            "`{test_id}` is a `{}` test. This command writes bench evidence, and a {} run is \
             recorded with `skynet driver check --results <file>` instead.",
            test.suite, test.suite
        ));
    }

    let Some(outcome_text) = args.value("outcome") else {
        return Err(
            "`--outcome <o>` is required: pass, fail, skipped or not_applicable. There is no \
             default, because the only safe one would be `fail` and the convenient one would be \
             `pass`."
                .to_string(),
        );
    };
    let Some(outcome) = SctsOutcome::parse(outcome_text) else {
        return Err(format!(
            "`{outcome_text}` is not an outcome. They are pass, fail, skipped, not_applicable."
        ));
    };

    let value = match args.value("value") {
        Some(text) => match text.parse::<f64>() {
            Ok(number) if number.is_finite() => Some(number),
            _ => return Err(format!("`--value {text}` is not a finite number.")),
        },
        None => None,
    };
    if outcome == SctsOutcome::Pass && test.quantitative && value.is_none() {
        return Err(format!(
            "`{test_id}` is measured in {} and `--value` was not given. §15's point about this \
             test is that a success with no number is not evidence.",
            test.unit.unwrap_or("a measured quantity")
        ));
    }

    Ok(SctsResult {
        test_id: test_id.to_string(),
        outcome,
        value,
        ran_at: String::new(),
        detail: args.value("detail").map(str::to_string),
    })
}

// ── award ────────────────────────────────────────────────────────────────────

fn award(rest: &[String]) -> i32 {
    let args = match Args::parse(rest, &AWARD_SPEC) {
        Ok(args) => args,
        Err(message) => return term::fail(&message),
    };
    if args.positional.is_empty() {
        return term::fail(
            "no evidence. Name the files: bench records from `skynet bench record`, results \
             documents from `skynet driver check --results`.",
        );
    }
    let Some(claimed) = args.value("claim") else {
        return term::fail(
            "`--claim <tier>` is required — guest, host or certified. The evidence caps the award \
             and the claim caps how far it is walked, so there is nothing to compute without one.",
        );
    };
    let field_conformance = match args.value("field-conformance") {
        Some(text) => match text.parse::<f64>() {
            Ok(number) if (0.0..=1.0).contains(&number) => Some(number),
            _ => {
                return term::fail(&format!(
                    "`--field-conformance {text}` is not a ratio between 0 and 1."
                ))
            }
        },
        None => None,
    };
    let root = PathBuf::from(args.value("root").unwrap_or("."));

    let mut results: Vec<SctsResult> = Vec::new();
    let mut oldest: Option<i64> = None;
    let mut provenance: Vec<String> = Vec::new();

    for path in &args.positional {
        let document = match read_json(Path::new(path)) {
            Ok(document) => document,
            Err(message) => return term::fail(&message),
        };
        match document.get_present("schema").and_then(Json::as_str) {
            Some(BENCH_RECORD_SCHEMA) => {
                let record = match bench_record_from_json(&document) {
                    Ok(record) => record,
                    Err(err) => return term::fail(&format!("{path}: {err}")),
                };
                let mut observed: Vec<DigestFile> = Vec::new();
                for artefact in &record.subject.artefacts {
                    match hash_file(&root, &artefact.path) {
                        Ok(file) => observed.push(file),
                        Err(message) => {
                            return term::fail(&format!(
                                "{path} was taken against `{}`, and {message}",
                                artefact.path
                            ))
                        }
                    }
                }
                let evidence = match evidence_for(&record, &observed) {
                    Ok(evidence) => evidence,
                    Err(err) => return term::fail(&format!("{path}: {err}")),
                };
                for result in evidence {
                    if let Some(day) = skynet_contracts::time::day_number(&result.ran_at) {
                        oldest = Some(oldest.map_or(day, |seen: i64| seen.min(day)));
                    }
                }
                provenance.push(format!(
                    "{path}: {} bench result(s) against {}, witnessed by {} — {}",
                    evidence.len(),
                    &record.subject.digest[..12],
                    record.witness.kind(),
                    record.subject.what
                ));
                results.extend(evidence.iter().cloned());
            }
            Some(RESULTS_SCHEMA) => {
                let read = match results_from_json(&document) {
                    Ok(read) => read,
                    Err(err) => return term::fail(&format!("{path}: {err}")),
                };
                provenance.push(format!(
                    "{path}: {} result(s), bound to nothing — {}",
                    read.results.len(),
                    read.source
                ));
                results.extend(read.results);
            }
            Some(other) => {
                return term::fail(&format!(
                    "{path} carries `schema` {other}. This command reads \
                     {BENCH_RECORD_SCHEMA} and {RESULTS_SCHEMA}."
                ))
            }
            None => {
                return term::fail(&format!(
                    "{path} has no `schema`, so nothing here knows what shape it is. Bench \
                     records come from `skynet bench record`; results documents come from \
                     `skynet driver check --results`."
                ))
            }
        }
    }

    // §15 decays an award from the **oldest** bench result. `None` here is not "fresh" — it is a
    // set of evidence with no bench result in it at all, which `award_certification` already reads
    // as every bench test missing.
    let bench_age_days = oldest.map(|day| (today() - day) as f64);

    let verdict = award_certification(&CertificationInput {
        claimed: claimed.to_string(),
        results,
        field_conformance,
        bench_age_days,
        simulator: args.flag("simulator"),
    });

    println!("\n  {}", term::bold("Evidence"));
    for line in &provenance {
        println!("    {line}");
    }
    if let Some(age) = bench_age_days {
        println!("    oldest bench result: {age:.0} day(s) old");
    } else {
        println!(
            "    {}",
            term::dim("no bench result in any of these files — every bench test is missing")
        );
    }

    println!("\n  {}", term::bold("Verdict"));
    let awarded = verdict.awarded.as_str();
    println!(
        "    claimed {claimed}, awarded {}",
        if awarded == "none" {
            term::bad(awarded)
        } else if awarded == claimed {
            term::ok(awarded)
        } else {
            term::warn(awarded)
        }
    );
    if !verdict.failures.is_empty() {
        println!(
            "    {} {}",
            term::bad("failed  "),
            verdict.failures.join(", ")
        );
    }
    if !verdict.missing.is_empty() {
        println!(
            "    {} {}",
            term::warn("missing "),
            verdict.missing.join(", ")
        );
    }
    for reason in &verdict.reasons {
        println!("    {}", term::dim(reason));
    }
    if let Some(days) = verdict.expires_in_days {
        println!("    expires in {days:.0} day(s)");
    }

    if awarded == claimed {
        0
    } else {
        println!(
            "\n  {}",
            term::dim(
                "Absence is not a pass (§15): a required test with no result is missing, never a \
                 silent success."
            )
        );
        1
    }
}

// ── shared ───────────────────────────────────────────────────────────────────

/// Hash one artefact, keeping the path as the record spells it.
fn hash_file(root: &Path, path: &str) -> Result<DigestFile, String> {
    let full = root.join(path);
    let bytes = std::fs::read(&full).map_err(|err| format!("{}: {err}", full.display()))?;
    Ok(DigestFile {
        path: path.to_string(),
        sha256: String::from_utf8(skynet_sha256::hex_bytes(&bytes).to_vec())
            .expect("hex is ascii by construction"),
    })
}

fn read_json(path: &Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    Json::parse(&text).map_err(|err| format!("{}: {err:?}", path.display()))
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

/// Today's day number, for the age of a bench result.
fn today() -> i64 {
    skynet_contracts::time::day_number(&now()).unwrap_or(0)
}
