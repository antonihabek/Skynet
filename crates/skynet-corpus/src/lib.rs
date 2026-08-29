//! The conformance corpus runner.
//!
//! > An edition that has not passed the corpus is not an edition. It cannot carry the name, appear
//! > in the matrix, or be installed by `skynetd` — which is the same mechanism SCTS applies to
//! > robots, pointed at implementations. (§31.4)
//!
//! This crate is that mechanism for the Rust edition. It reads `corpus/**/*.json`, runs each case's
//! input through [`skynet_contracts`] and [`skynet_kernel`], serialises the result in canonical
//! form, and compares it to the case's `expected` **string** — byte for byte, not field by field.
//!
//! # Why byte-identical, and not "equivalent"
//!
//! Because equivalence is where drift hides. Two implementations that emit the same warnings in a
//! different order are equivalent today and are computing a different clamp the day somebody
//! writes code that reads the first warning. The corpus refuses to have that conversation: the
//! bytes match or the edition has diverged, and which of the two is right is then a question about
//! the specification rather than a question about the diff.
//!
//! # What a failure means
//!
//! Not necessarily a bug in this crate. A divergence is *either* a specification ambiguity *or* an
//! implementation defect, and §31.4 is explicit that it blocks the release of the edition that
//! diverged and never of the others. [`Failure`] therefore reports both strings in full and points
//! at the first differing byte, because the useful question is which of the two is right.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use skynet_contracts::admission::admit_package;
use skynet_contracts::certification::award_certification;
use skynet_contracts::json;
use skynet_contracts::readiness::{evaluate_readiness, summarize_readiness, ReadinessReport};
use skynet_contracts::roles::parse_role;
use skynet_contracts::transport::resolve_transport;
use skynet_json::Json;

mod limits;

pub use limits::{BorrowedChannels, CycleOwned, LimitsOwned};

/// One case that did not reproduce.
#[derive(Debug, Clone)]
pub struct Failure {
    /// Which corpus file.
    pub file: String,
    /// Which case in it.
    pub id: String,
    /// Why the case exists, carried through so a failure explains itself.
    pub note: String,
    /// What the corpus says this edition must emit.
    pub expected: String,
    /// What it emitted.
    pub actual: String,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} :: {}", self.file, self.id)?;
        writeln!(f, "  note:     {}", self.note)?;
        writeln!(f, "  expected: {}", self.expected)?;
        writeln!(f, "  actual:   {}", self.actual)?;
        if let Some(at) = first_difference(&self.expected, &self.actual) {
            writeln!(f, "  first difference at byte {at}:")?;
            writeln!(f, "    expected …{}", window(&self.expected, at))?;
            writeln!(f, "    actual   …{}", window(&self.actual, at))?;
        }
        Ok(())
    }
}

fn first_difference(a: &str, b: &str) -> Option<usize> {
    a.bytes()
        .zip(b.bytes())
        .position(|(x, y)| x != y)
        .or(if a.len() == b.len() {
            None
        } else {
            Some(a.len().min(b.len()))
        })
}

fn window(text: &str, at: usize) -> String {
    let start = at.saturating_sub(24);
    let end = (at + 48).min(text.len());
    // Snap to character boundaries so a multi-byte character cannot panic the reporter.
    let start = (start..=at)
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(at);
    let end = (at..=end)
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(at);
    text[start..end].to_string()
}

/// What a corpus run concluded.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Cases that reproduced exactly.
    pub passed: usize,
    /// Cases that did not.
    pub failures: Vec<Failure>,
    /// Cases whose `corpus` name this runner does not implement.
    ///
    /// Reported rather than skipped silently: an edition that quietly ignores a whole corpus is an
    /// edition that passes by not looking, which is the same failure as a conformance suite
    /// treating an absent result as a pass.
    pub unsupported: Vec<String>,
    /// Files read.
    pub files: usize,
}

impl Report {
    /// Whether this edition reproduced the whole corpus.
    pub fn ok(&self) -> bool {
        self.failures.is_empty() && self.unsupported.is_empty()
    }
}

/// Every `.json` under `root`, sorted, so a run is reproducible.
pub fn corpus_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(root, &mut found);
    found.sort();
    found
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "json") {
            out.push(path);
        }
    }
}

/// Run every case under `root`.
pub fn run(root: &Path) -> Report {
    let mut report = Report::default();

    for path in corpus_files(root) {
        report.files += 1;
        let label = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");

        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                report
                    .unsupported
                    .push(format!("{label}: unreadable: {error}"));
                continue;
            }
        };
        let document = match Json::parse(&text) {
            Ok(document) => document,
            Err(error) => {
                report
                    .unsupported
                    .push(format!("{label}: unparseable: {error}"));
                continue;
            }
        };

        let corpus = document.get("corpus").and_then(Json::as_str).unwrap_or("");
        for case in document.arr_or_empty("cases") {
            let id = case.get("id").and_then(Json::as_str).unwrap_or("<unnamed>");
            let note = case.get("note").and_then(Json::as_str).unwrap_or("");
            let expected = case.get("expected").and_then(Json::as_str).unwrap_or("");
            let Some(input) = case.get("input") else {
                report
                    .unsupported
                    .push(format!("{label} :: {id}: no input"));
                continue;
            };

            match evaluate(corpus, &label, input) {
                Ok(actual) => {
                    if actual == expected {
                        report.passed += 1;
                    } else {
                        report.failures.push(Failure {
                            file: label.clone(),
                            id: id.to_string(),
                            note: note.to_string(),
                            expected: expected.to_string(),
                            actual,
                        });
                    }
                }
                Err(reason) => report
                    .unsupported
                    .push(format!("{label} :: {id}: {reason}")),
            }
        }
    }

    report
}

/// Run one case and return the canonical bytes this edition produces.
fn evaluate(corpus: &str, label: &str, input: &Json) -> Result<String, String> {
    let value = match corpus {
        "roles" => {
            let role = input.get_present("role").and_then(Json::as_str);
            json::role_to_json(&parse_role(role))
        }

        "transport" | "fuzz" if label.contains("transport") => {
            let source = json::source_from_json(input.get("source").ok_or("no `source`")?)
                .map_err(|e| e.to_string())?;
            let target = json::descriptor_from_json(input.get("target").ok_or("no `target`")?)
                .map_err(|e| e.to_string())?;
            let options = json::options_from_json(input.get("options"));
            json::plan_to_json(&resolve_transport(&source, &target, options))
        }

        "cycle" | "fuzz" => {
            let owned = LimitsOwned::from_json(input.get("limits").ok_or("no `limits`")?)?;
            let cycle_owned = CycleOwned::from_json(input.get("cycle").ok_or("no `cycle`")?)?;
            let last_sequence = input.get_present("last_sequence").and_then(Json::as_f64);

            let per_channel = owned.per_channel_refs();
            let params = owned.params(&per_channel);
            let limits = skynet_kernel::resolve_limits(&params);
            let (desired, previous) = cycle_owned.refs();
            let cycle = cycle_owned.cycle(&desired, previous.as_deref());
            let envelope = owned.envelope();
            let verdict = skynet_kernel::evaluate_cycle(
                &cycle,
                &limits,
                envelope.as_ref(),
                last_sequence.map(|n| n as u64),
            );
            skynet_contracts::safety::verdict_to_json(&verdict)
        }

        "limits" => {
            let owned = LimitsOwned::from_json(input)?;
            let per_channel = owned.per_channel_refs();
            let params = owned.params(&per_channel);
            let limits = skynet_kernel::resolve_limits(&params);
            skynet_contracts::safety::limits_to_json(&limits, owned.profile_name())
        }

        "admission" => {
            let params = json::admission_params_from_json(input).map_err(|e| e.to_string())?;
            json::admission_to_json(&admit_package(&params))
        }

        "frames" => {
            // An absent or null graph is a body with no tree, which is a question for the guard
            // that wanted one rather than for the validator — so it is a broken case file here
            // rather than a silent pass.
            let graph = json::frames_from_json(input.get("graph")).ok_or("no `graph`")?;
            json::frame_fault_to_json(&skynet_contracts::embodiment::validate_frames(&graph))
        }

        "certification" => {
            let cert_input =
                json::certification_input_from_json(input).map_err(|e| e.to_string())?;
            json::certification_to_json(&award_certification(&cert_input))
        }

        "readiness" if input.get("subjects").is_some() => {
            let mut reports: Vec<ReadinessReport> = Vec::new();
            for subject in input.arr_or_empty("subjects") {
                let subject =
                    json::readiness_subject_from_json(subject).map_err(|e| e.to_string())?;
                reports.push(evaluate_readiness(&subject));
            }
            json::fleet_to_json(&summarize_readiness(&reports))
        }

        "readiness" => {
            let subject = json::readiness_subject_from_json(input).map_err(|e| e.to_string())?;
            json::readiness_to_json(&evaluate_readiness(&subject))
        }

        other => return Err(format!("no runner for corpus `{other}`")),
    };

    value.to_canonical().map_err(|e| e.to_string())
}

/// The corpus directory, resolved from this crate's location.
pub fn default_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("corpus")
}
