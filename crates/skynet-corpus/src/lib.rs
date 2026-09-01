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

use skynet_contracts::admission::{self, admit_package};
use skynet_contracts::cell;
use skynet_contracts::certification;
use skynet_contracts::certification::award_certification;
use skynet_contracts::embodiment as contracts_embodiment;
use skynet_contracts::json;
use skynet_contracts::opstate;
use skynet_contracts::predicate;
use skynet_contracts::readiness::{evaluate_readiness, summarize_readiness, ReadinessReport};
use skynet_contracts::roles;
use skynet_contracts::roles::parse_role;
use skynet_contracts::semver;
use skynet_contracts::skill;
use skynet_contracts::transport;
use skynet_contracts::transport::resolve_transport;
use skynet_json::Json;

mod blackbox;
mod limits;

pub use limits::{BindingsOwned, BorrowedChannels, CycleOwned, LimitsOwned};

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

        // Guarded on the label, and it was not always. `"cycle" | "fuzz"` sat here as a catch-all,
        // which meant a *new* fuzz family did not arrive as an unsupported corpus — the thing this
        // runner reports loudly and refuses to skip — but as forty-eight cycle vectors with no
        // `limits`. The report was still a failure, so nothing shipped wrong; it named the wrong
        // problem, which on a quieter day is how a whole corpus comes to be dispatched into an arm
        // that was never written for it. Every arm now says which files it is for.
        "cycle" | "fuzz" if label.contains("cycle") => {
            let owned = LimitsOwned::from_json(input.get("limits").ok_or("no `limits`")?)?;
            let cycle_owned = CycleOwned::from_json(input.get("cycle").ok_or("no `cycle`")?)?;
            let last_sequence = input.get_present("last_sequence").and_then(Json::as_f64);
            let bindings_owned = BindingsOwned::from_json(input.get_present("joints"));

            let per_channel = owned.per_channel_refs();
            let frames = owned.frame_refs();
            let volumes = owned.volume_refs();
            let layers = owned.layer_refs(&volumes);
            let params = owned.params(&per_channel, frames.as_deref(), &layers);
            let grants = owned.grant_refs();
            let limits = skynet_kernel::resolve_limits(&params).with_grants(&grants);
            let (desired, previous) = cycle_owned.refs();
            let cycle = cycle_owned.cycle(&desired, previous.as_deref());
            let envelope = owned.envelope();
            let bindings = bindings_owned.refs();
            let mut scratch = skynet_kernel::kinematics::Kinematics::new();
            let verdict = skynet_kernel::evaluate_cycle_with_body(
                &cycle,
                &limits,
                envelope.as_ref(),
                last_sequence.map(|n| n as u64),
                Some(skynet_kernel::Body {
                    joints: &bindings,
                    scratch: &mut scratch,
                }),
            );
            skynet_contracts::safety::verdict_to_json(&verdict)
        }

        "limits" => {
            let owned = LimitsOwned::from_json(input)?;
            let per_channel = owned.per_channel_refs();
            let frames = owned.frame_refs();
            let volumes = owned.volume_refs();
            let layers = owned.layer_refs(&volumes);
            let params = owned.params(&per_channel, frames.as_deref(), &layers);
            let limits = skynet_kernel::resolve_limits(&params);
            skynet_contracts::safety::limits_to_json(&limits, owned.profile_name())
        }

        // Forward kinematics on its own, with the poses themselves as the expected value.
        //
        // A separate corpus from `cycle` because it pins something `cycle` cannot: a trip is a
        // boolean and two editions could agree on every one of them while disagreeing about every
        // pose by a millimetre. Thirty digits of translation do not permit that — and they are what
        // holds the two editions' sine, cosine and square root to each other (`skynet-kernel`'s
        // `real` module, and `reference/typescript/kinematics.ts`).
        "kinematics" => {
            let graph = skynet_contracts::json::frames_from_json(input.get("frames"))
                .ok_or("no `frames`")?;
            let flat = skynet_contracts::kinematics::flatten(&graph)
                .map_err(|fault| format!("{}: {}", fault.code, fault.detail))?;
            let frames = flat.frames();
            let set = skynet_kernel::kinematics::FrameSet::new(&frames)
                .map_err(|fault| fault.as_str().to_string())?;
            let bindings_owned = BindingsOwned::from_json(input.get_present("joints"));
            let bindings = bindings_owned.refs();
            let command = limits::channel_pairs(input.get_present("command"));
            let mut solved = skynet_kernel::kinematics::Kinematics::new();
            solved.solve(&set, &bindings, |name| {
                command
                    .iter()
                    .find(|(channel, _)| channel == name)
                    .map(|(_, value)| *value)
            });
            skynet_contracts::kinematics::solve_to_json(&set, &solved)
        }

        // The black box's four corpora. Dispatched on the file rather than only on the corpus
        // name, because they hold four different kinds of artifact — a schema document, a record,
        // a header, a whole file — and one arm reaching for all four would have to guess from the
        // input's shape.
        "blackbox" if label.contains("schemas") => return blackbox::schemas(input),
        "blackbox" if label.contains("records") => return blackbox::records(input),
        "blackbox" if label.contains("header") => return blackbox::header(input),
        "blackbox" if label.contains("seal") => return blackbox::seal(input),

        // §54.2's predicates, over a live context.
        //
        // The case pins the boolean **and** the three-valued outcome, and this arm calls the two
        // entry points separately to produce them. `evaluate_bool` is defined as `satisfied(…)` of
        // `evaluate`, so calling both is how an edition that had quietly reimplemented one of them
        // gets caught disagreeing with itself rather than with the corpus.
        "predicate" => {
            let context = input.get("context").cloned().unwrap_or(Json::obj());
            let outcome = predicate::evaluate(input.get("predicate"), &context);
            let satisfied = predicate::evaluate_bool(input.get("predicate"), &context);
            predicate::outcome_to_json(&outcome, satisfied)
        }

        // Range satisfaction, which became a contract when §54.2 made `semver_satisfies` an
        // operator the robot evaluates. Two files: what a range admits, and how two versions order.
        "semver" if label.contains("satisfies") => {
            let version = input
                .get("version")
                .and_then(Json::as_str)
                .ok_or("no `version`")?;
            let range = input
                .get("range")
                .and_then(Json::as_str)
                .ok_or("no `range`")?;
            semver::satisfies_to_json(version, range)
        }

        "semver" if label.contains("compare") => {
            let a = input.get("a").and_then(Json::as_str).ok_or("no `a`")?;
            let b = input.get("b").and_then(Json::as_str).ok_or("no `b`")?;
            let mut out = Json::obj();
            out.set(
                "compare",
                Json::Num(f64::from(semver::compare_version_strings(a, b))),
            )
            .set("a", semver::version_to_json(&semver::parse_version(a)))
            .set("b", semver::version_to_json(&semver::parse_version(b)));
            out
        }

        // §54's four pure decisions. Four files rather than one runner, because they are four
        // questions with four inputs — and one arm reaching for all of them would have to guess
        // which from the shape of the case, which is how a whole corpus ends up dispatched into an
        // arm nobody wrote for it.
        "skill" if label.contains("preconditions") => {
            let published =
                skill::read_clauses(input.get_present("published"), skill::ClauseOrigin::Skill);
            let operator =
                skill::read_clauses(input.get_present("operator"), skill::ClauseOrigin::Operator);
            let clauses = skill::resolve_clauses(published, operator);
            let context = input.get("context").cloned().unwrap_or(Json::obj());
            let phase = match input.get("phase").and_then(Json::as_str) {
                Some("hold") => skill::Phase::Hold,
                _ => skill::Phase::Entry,
            };
            skill::precondition_report_to_json(&skill::evaluate_preconditions(
                &clauses, &context, phase,
            ))
        }

        "skill" if label.contains("success") => {
            let criteria =
                skill::read_clauses(input.get_present("criteria"), skill::ClauseOrigin::Skill);
            let post_state = input.get("post_state").cloned().unwrap_or(Json::obj());
            skill::success_to_json(&skill::judge_success(&criteria, &post_state))
        }

        "skill" if label.contains("timeout") => {
            let expected = input
                .get_present("expected_duration_seconds")
                .and_then(Json::as_f64);
            // An absent factor is the default, which is what the reference's optional parameter
            // means. A *present* one that is not a number is a caller who tried and failed, and
            // both editions fall back rather than refusing.
            let factor = input
                .get_present("overrun_factor")
                .and_then(Json::as_f64)
                .unwrap_or(skill::DEFAULT_OVERRUN_FACTOR);
            skill::deadline_to_json(&skill::skill_deadline(expected, factor))
        }

        "skill" if label.contains("composition") => {
            let root = input
                .get("root")
                .and_then(Json::as_str)
                .ok_or("no `root`")?;
            let calls = skill::read_calls(input.get_present("calls"));
            let max_depth = input
                .get_present("max_depth")
                .and_then(Json::as_f64)
                .map(|depth| depth as usize)
                .unwrap_or(skill::MAX_SKILL_DEPTH);
            skill::composition_to_json(&skill::validate_composition(root, &calls, max_depth))
        }

        // §54.4's lifecycle. Every case pins the phase, the reason, the outcome and
        // `release_leases` together, because they are one answer — and an edition could get the
        // phase right while releasing a lease at the wrong moment, which is the failure with no
        // symptom until the next invocation is refused the arm.
        "skill" if label.contains("lifecycle") => {
            let phase = input
                .get("phase")
                .and_then(Json::as_str)
                .and_then(skill::SkillPhase::parse)
                .ok_or("no readable `phase`")?;
            let progress = skill::progress_from_json(input.get("progress").unwrap_or(&Json::Null));
            skill::transition_to_json(&skill::advance(phase, &progress))
        }

        // §54.5's third limit, which is arithmetic rather than a machine and is pinned separately
        // for that reason: a bound that was wrong by a factor would still walk the diagram.
        "skill" if label.contains("bounded-deadline") => {
            let child = skill::skill_deadline(
                input
                    .get_present("expected_duration_seconds")
                    .and_then(Json::as_f64),
                skill::DEFAULT_OVERRUN_FACTOR,
            );
            let remaining = input
                .get_present("root_remaining_ms")
                .and_then(Json::as_f64);
            skill::deadline_to_json(&skill::bounded_deadline(&child, remaining))
        }

        // §55.2's cell descriptor. Two files, because the document has two consumers and an
        // edition could agree about one while disagreeing about the other: `validate` is what an
        // operator sees when they save it, `context` is what a precondition and the workspace guard
        // read out of it afterwards.
        "cell" if label.contains("validate") => {
            let subject = cell::cell_from_json(input.get("cell").ok_or("no `cell`")?);
            cell::cell_faults_to_json(&cell::validate_cell(&subject))
        }

        "cell" if label.contains("context") => {
            let subject = cell::cell_from_json(input.get("cell").ok_or("no `cell`")?);
            let layer = cell::cell_layer_to_json(&cell::cell_workspace_layer(&subject));
            let mut out = Json::obj();
            out.set("env", cell::cell_environment(&subject));
            out.set("layer", layer.get("layer").cloned().unwrap_or(Json::Null));
            out
        }

        // §27's machine, extended by §48.1. Three files, because the machine has three separable
        // ways to be wrong: where it goes and why, what a state *means* to the sections that read
        // it, and §49.1's tone over every ordered pair. An edition could agree about the first and
        // disagree about either of the others, and only the first is a computation.
        "opstate" if label.contains("evaluate") => {
            let previous = input
                .get("previous")
                .and_then(Json::as_str)
                .and_then(opstate::OperatingState::parse)
                .ok_or("no readable `previous`")?;
            let conditions =
                opstate::conditions_from_json(input.get("conditions").unwrap_or(&Json::Null));
            opstate::transition_to_json(&opstate::evaluate(previous, &conditions))
        }

        "opstate" if label.contains("properties") => opstate::state_properties_to_json(),

        "opstate" if label.contains("warning") => opstate::warning_matrix_to_json(),

        // The closed vocabularies, as data rather than as a computation.
        //
        // Every other arm holds this edition to an *answer*. This one holds it to a **word list**,
        // and it is here because a divergence in one lived through a full green test run: a
        // rejection code was added to the Rust and not to the reference, and nothing could see it.
        // The mirror pins guard the reference's bytes; the corpus guards the cases it holds; and a
        // vocabulary member nobody had written a case for was outside both.
        //
        // Emitted in declaration order rather than sorted, because order is part of the agreement
        // for several of these — tiers and SCTS areas are ranked, and an edition that sorted them
        // would be one whose "next tier up" arithmetic silently differed.
        "vocabulary" => {
            let set = input.get("set").and_then(Json::as_str).ok_or("no `set`")?;
            let members: Vec<&str> = match set {
                "package_rejection_codes" => admission::PACKAGE_REJECTION_CODES
                    .iter()
                    .map(|code| code.as_str())
                    .collect(),
                "signature_states" => admission::SIGNATURE_STATES
                    .iter()
                    .map(|state| state.as_str())
                    .collect(),
                "package_kinds" => admission::PACKAGE_KINDS.to_vec(),
                "isolation_modes" => admission::ISOLATION_MODES.to_vec(),
                "reserved_capabilities" => admission::RESERVED_CAPABILITIES.to_vec(),
                "transport_verdicts" => transport::TRANSPORT_VERDICTS
                    .iter()
                    .map(|verdict| verdict.as_str())
                    .collect(),
                "transport_methods" => transport::TRANSPORT_METHODS.to_vec(),
                "transport_warning_codes" => transport::TRANSPORT_WARNING_CODES
                    .iter()
                    .map(|code| code.as_str())
                    .collect(),
                "action_kinds" => contracts_embodiment::ACTION_KINDS.to_vec(),
                "control_modes" => contracts_embodiment::CONTROL_MODES.to_vec(),
                "joint_types" => contracts_embodiment::JOINT_TYPES.to_vec(),
                "base_kinds" => contracts_embodiment::BASE_KINDS.to_vec(),
                "embodiment_modalities" => contracts_embodiment::EMBODIMENT_MODALITIES.to_vec(),
                "canonical_chains" => roles::CANONICAL_CHAINS.to_vec(),
                "canonical_axes" => roles::CANONICAL_AXES.to_vec(),
                "os_tiers" => skynet_kernel::OS_TIERS
                    .iter()
                    .map(|tier| tier.as_str())
                    .collect(),
                "enforcement_modes" => skynet_kernel::ENFORCEMENT_MODES
                    .iter()
                    .map(|mode| mode.as_str())
                    .collect(),
                "guards" => skynet_kernel::GUARDS
                    .iter()
                    .map(|guard| guard.as_str())
                    .collect(),
                "fault_responses" => skynet_kernel::FAULT_RESPONSES
                    .iter()
                    .map(|response| response.as_str())
                    .collect(),
                "predicate_ops" => predicate::PREDICATE_OPS.to_vec(),
                "clause_outcomes" => skill::CLAUSE_OUTCOMES
                    .iter()
                    .map(|outcome| outcome.as_str())
                    .collect(),
                "clause_origins" => skill::CLAUSE_ORIGINS
                    .iter()
                    .map(|origin| origin.as_str())
                    .collect(),
                "precondition_phases" => skill::PHASES.iter().map(|phase| phase.as_str()).collect(),
                "skill_outcomes" => skill::SKILL_OUTCOMES
                    .iter()
                    .map(|outcome| outcome.as_str())
                    .collect(),
                "composition_faults" => skill::COMPOSITION_FAULTS
                    .iter()
                    .map(|code| code.as_str())
                    .collect(),
                "cell_faults" => cell::CELL_FAULTS.iter().map(|code| code.as_str()).collect(),
                "operating_states" => opstate::state_names(),
                "state_reasons" => opstate::STATE_REASONS
                    .iter()
                    .map(|reason| reason.as_str())
                    .collect(),
                "stop_causes" => opstate::STOP_CAUSES
                    .iter()
                    .map(|cause| cause.as_str())
                    .collect(),
                "post_verdicts" => opstate::POST_VERDICTS
                    .iter()
                    .map(|verdict| verdict.as_str())
                    .collect(),
                "homing_states" => opstate::HOMING_STATES
                    .iter()
                    .map(|state| state.as_str())
                    .collect(),
                "rhal_states" => opstate::RHAL_STATES
                    .iter()
                    .map(|state| state.as_str())
                    .collect(),
                "policy_states" => opstate::POLICY_STATES
                    .iter()
                    .map(|state| state.as_str())
                    .collect(),
                "kernel_states" => opstate::KERNEL_STATES
                    .iter()
                    .map(|state| state.as_str())
                    .collect(),
                "skill_phases" => skill::SKILL_PHASES
                    .iter()
                    .map(|phase| phase.as_str())
                    .collect(),
                "lifecycle_reasons" => skill::LIFECYCLE_REASONS
                    .iter()
                    .map(|reason| reason.as_str())
                    .collect(),
                "lease_states" => skill::LEASE_STATES
                    .iter()
                    .map(|state| state.as_str())
                    .collect(),
                "predicate_outcomes" => predicate::OUTCOMES
                    .iter()
                    .map(|outcome| outcome.as_str())
                    .collect(),
                "unevaluable_reasons" => predicate::UNEVALUABLE_REASONS
                    .iter()
                    .map(|reason| reason.as_str())
                    .collect(),
                "scts_suites" => certification::SCTS_SUITES.to_vec(),
                "scts_areas" => certification::SCTS_AREAS.to_vec(),
                // Not a skip. A set the corpus names and this edition does not is exactly the
                // divergence this file exists to catch, and answering "unsupported" is how it gets
                // reported rather than passed over.
                other => return Err(format!("no vocabulary named `{other}` in this edition")),
            };
            let mut out = Json::arr();
            for member in members {
                out.push(Json::Str(member.to_string()));
            }
            out
        }

        "admission" => {
            let params = json::admission_params_from_json(input).map_err(|e| e.to_string())?;
            json::admission_to_json(&admit_package(&params))
        }

        "digest" => {
            let kind = input
                .get_present("kind")
                .and_then(Json::as_str)
                .unwrap_or("");
            let version = input
                .get_present("version")
                .and_then(Json::as_str)
                .unwrap_or("");
            // An absent manifest is `{}` rather than an error: the Hub's `manifest ?? {}` says the
            // same thing, and a version with nothing declared still has an identity.
            let manifest = input
                .get_present("manifest")
                .cloned()
                .unwrap_or(Json::obj());
            let files = json::digest_files_from_json(input.get_present("files"));
            json::digest_to_json(&skynet_contracts::digest::version_digest(
                kind, version, &manifest, &files,
            ))
        }

        // The joint list is what separates the two frame corpora, and it is the whole of the
        // difference: `validate_frames` is given a tree and asked whether the tree is well formed,
        // and `validate_frame_bindings` is given the same tree and the body it came out of.
        "frames" if label.contains("bindings") => {
            let graph = json::frames_from_json(input.get("graph")).ok_or("no `graph`")?;
            let joints: Vec<&str> = input
                .arr_or_empty("joints")
                .iter()
                .filter_map(Json::as_str)
                .collect();
            json::frame_fault_to_json(&skynet_contracts::embodiment::validate_frame_bindings(
                &graph, &joints,
            ))
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

        // §45.7's three conformance additions, in one runner because they are one question asked
        // of one input: given these records and this moment, what is believed and which guards
        // lose their input. Splitting them would let an edition pass `expiry_enforced` while
        // disagreeing about which guard the expired set feeds, which is the half that matters to
        // whoever is standing next to the robot.
        // §46. One runner for the whole chain, for the same reason §45.7's three cases are one
        // corpus: the chain, the command-loss verdict and the nesting are one question — is this
        // robot's watchdog chain sound — and pinning them separately would let an edition agree
        // about the inversion and disagree about what it costs.
        "watchdog" => {
            let windows = skynet_contracts::watchdog::Windows {
                kernel_cycle_worst_case_ms: input
                    .get("windows")
                    .and_then(|w| w.get("kernel_cycle_worst_case_ms"))
                    .and_then(Json::as_f64),
                envelope_watchdog_ms: input
                    .get("windows")
                    .and_then(|w| w.get("envelope_watchdog_ms"))
                    .and_then(Json::as_f64),
                soc_wdt_ms: input
                    .get("windows")
                    .and_then(|w| w.get("soc_wdt_ms"))
                    .and_then(Json::as_f64),
                drive_command_timeout_ms: input
                    .get("windows")
                    .and_then(|w| w.get("drive_command_timeout_ms"))
                    .and_then(Json::as_f64),
            };
            json::watchdog_to_json(
                input.get("on_command_loss").and_then(Json::as_str),
                &windows,
            )
        }

        // §25.1. Two files and one parser, because the two questions are asked of exactly the same
        // input: given these streams, what goes in the frame, and which instant should a live loop
        // aim at in the first place. An edition could agree about every assembled frame and still
        // pick a different target every cycle, which is a disagreement about every frame in the
        // field and none in the corpus — so the target is pinned beside the frames rather than
        // trusted to follow from them.
        "observation" if label.contains("target") => {
            let streams = observation_streams(input)?;
            let now = input
                .get("now_ns")
                .and_then(Json::as_f64)
                .ok_or("no `now_ns`")? as u64;
            let mut out = Json::obj();
            out.set(
                "target_ns",
                match skynet_contracts::observation::latest_common_instant(now, &streams) {
                    Some(instant) => Json::Num(instant as f64),
                    None => Json::Null,
                },
            );
            out
        }

        "observation" | "fuzz" if label.contains("observation") => {
            let streams = observation_streams(input)?;
            let target = input
                .get("target_ns")
                .and_then(Json::as_f64)
                .ok_or("no `target_ns`")? as u64;
            skynet_contracts::observation::assemble(target, &streams).to_json()
        }

        "calibration" => {
            let now = json::conditions_from_json(input.get_present("now"));
            let mut records = Vec::new();
            for raw in input.arr_or_empty("records") {
                let record = json::calibration_from_json(raw).map_err(|e| e.to_string())?;
                let validity = skynet_contracts::calibration::evaluate(&record, &now);
                records.push((record.set.clone(), validity));
            }

            let applicable: Vec<String> = if input.get_present("applicable").is_some() {
                input.str_arr("applicable")
            } else {
                skynet_contracts::calibration::all_sets()
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            };
            let applicable_refs: Vec<&str> = applicable.iter().map(String::as_str).collect();

            let mut out = Json::obj();
            out.set(
                "records",
                Json::Arr(
                    records
                        .iter()
                        .map(|(set, validity)| json::validity_to_json(set, validity))
                        .collect(),
                ),
            );
            out.set(
                "unavailable",
                Json::Arr(
                    skynet_contracts::calibration::guards_unavailable(&records, &applicable_refs)
                        .iter()
                        .map(json::guard_unavailable_to_json)
                        .collect(),
                ),
            );
            out
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

/// The stream list §25.1's two corpora share.
///
/// `latency_ms`, `uncertainty_ms` and `budget_ms` are read with [`Json::get`] rather than
/// [`Json::get_present`], so an explicit `null` and an absent key arrive as the same `None`. That is
/// the right collapse *here* and only here: on the wire the two are different facts, and the
/// assembler's answer to both is identical — nobody declared it — which is exactly what the
/// reference edition does with `?? null`.
fn observation_streams(input: &Json) -> Result<Vec<skynet_contracts::observation::Stream>, String> {
    let mut streams = Vec::new();
    for raw in input.arr_or_empty("streams") {
        let mut samples = Vec::new();
        for sample in raw.arr_or_empty("samples") {
            samples.push(skynet_contracts::observation::Sample {
                stamp_ns: sample
                    .get("stamp_ns")
                    .and_then(Json::as_f64)
                    .ok_or("a sample with no `stamp_ns`")? as u64,
                seq: sample.get("seq").and_then(Json::as_f64).unwrap_or(0.0) as u64,
            });
        }
        streams.push(skynet_contracts::observation::Stream {
            name: raw
                .get("name")
                .and_then(Json::as_str)
                .ok_or("a stream with no `name`")?
                .to_string(),
            modality: raw
                .get("modality")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string(),
            latency_ms: raw.get("latency_ms").and_then(Json::as_f64),
            uncertainty_ms: raw.get("uncertainty_ms").and_then(Json::as_f64),
            budget_ms: raw.get("budget_ms").and_then(Json::as_f64),
            samples,
        });
    }
    Ok(streams)
}
