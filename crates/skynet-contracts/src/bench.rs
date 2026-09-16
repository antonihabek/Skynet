//! A bench result, written down — §15's second suite, as a record rather than an assertion.
//!
//! §15 has three suites and, until this module, one of them had no wire form at all.
//! [`certification::award_certification`](crate::certification::award_certification) takes a
//! `Vec<SctsResult>` and asks no questions about where a result came from, and every producer in
//! this workspace fills that vector from a run it just performed: `skynet driver check` measures
//! the `sim` half against a shared object, `skynetd-io::conformance` measures the runtime half
//! against the machine it is on. A `bench` result is by construction *not* a run the caller just
//! performed — §15 prices it at a human, a robot and a day — so it has to arrive as a **document**,
//! and there was nowhere for one to come from.
//!
//! The consequence, which is why this is worth a module rather than a struct: every bench test is
//! `missing` on every award this workspace can compute, one of them is required from `guest`, and
//! so **nothing here can award any tier at all**. `DRY-COVERAGE-PLAN.md` F1 records finding that
//! out; this is the half of the answer that is a format.
//!
//! # The digest is the whole mechanism
//!
//! A bench result is a measurement of *some particular bytes*. §12 already knows this and says so
//! in the narrowest possible way — [`ota::BenchPass`](crate::ota::BenchPass) carries an
//! `image_digest`, and [`BundleFaultCode::BenchPassStale`](crate::ota::BundleFaultCode) is the
//! refusal for *"a bench pass exists and is for other bytes"*. That refusal is the entire reason a
//! gated OTA is a gate rather than a checkbox.
//!
//! This module is the general form of the same rule. A record names the artefacts it was taken
//! against, by path and by sha256; [`BenchSubject::digest`] is one digest over that whole list; and
//! [`evidence_for`] will not hand a caller a single result until the caller's own view of those
//! bytes hashes to the same thing. Rebuild the driver and the record stops applying to it — not by
//! anybody remembering to re-run the bench session, but because the arithmetic no longer works.
//!
//! Two smaller rules fall out of the same place:
//!
//! * The record carries **both** the file list and the digest, and parsing recomputes one from the
//!   other. A document that names one set of bytes and binds to another is refused rather than
//!   believed on whichever half a reader happened to look at.
//! * The domain separator is [`crate::digest::BENCH_SUBJECT_DOMAIN`], not §37.1's. A digest over a
//!   package and a digest over a bench subject can never collide, however similar the inputs look.
//!
//! # The witness has no default, for the same reason `SignatureState` has none
//!
//! §15's price for a bench result is *"the physical robot, once, with a human present"*. A record
//! that will not say who was present is not evidence, it is an assertion — so [`Witness`] is a
//! required field with two spellings and no zero value, exactly as
//! [`SignatureState`](crate::admission::SignatureState) is required on an admission because
//! `absent` would be a caller who forgot the field claiming nobody signed.
//!
//! [`Witness::Machine`] exists because `DRY-COVERAGE-PLAN.md` *Decisions* 4 decided one case on
//! purpose. `offline.control.independent` measures §11's invariant — no control decision requires a
//! network round trip — and that needs *a machine rather than a model*: a real kernel, real
//! scheduling, a real absence of a route. It does not need a body, and it does not need a person to
//! watch it. So a machine may witness it, and the record says *machine* rather than quietly
//! borrowing a person's authority.
//!
//! **This module does not decide which tests a machine may witness.** It records who was there and
//! refuses to lose the answer; weighing it is the awarding party's, and §15.1 is explicit that the
//! awarding party is the Hub. What a reader gets here is the fact, spelled the same way every time.
//!
//! # Nothing here is signed, and that is §19 rather than an omission
//!
//! §19 forbids a robot holding a signing key, and §15.1 says what follows: a robot does not sign
//! its own results, so what a runner produces is a *report*. This record is a report too. It is
//! tamper-evident about the **bytes it describes** — change them and it stops applying — and
//! entirely silent about who wrote it down. Closing that is the Hub countersigning what a device
//! credential delivered, which §80 says is a different claim from the one §15 asks for and must not
//! be built as though it were the same.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

use crate::certification::{SctsOutcome, SctsResult, SCTS_TESTS};
use crate::digest::{subject_digest, DigestError, DigestFile};

/// The one schema string a record may carry.
///
/// Checked rather than assumed. A document with no `schema` is a document whose shape nobody
/// promised, and reading one as though it were this shape is how a format acquires a second
/// meaning.
pub const BENCH_RECORD_SCHEMA: &str = "skynet.bench.v1";

/// Who was present when the measurement was taken.
///
/// No `Default`. See the module header: the whole value of the field is that it cannot be omitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Witness {
    /// A person, at the machine, named. §15's price for a bench result.
    Human {
        /// What they are called, as they gave it. Not validated — a name is not a syntax.
        name: String,
    },
    /// A machine, with nobody watching.
    ///
    /// Legitimate for exactly the class of test that needs a real machine rather than a real body
    /// (*Decisions* 4), and it says so about itself so that the awarding party can weigh it.
    Machine {
        /// What the machine is, in whatever terms the producer had — a hostname, a kernel string.
        host: String,
    },
}

impl Witness {
    /// The wire spelling of the kind.
    pub fn kind(&self) -> &'static str {
        match self {
            Witness::Human { .. } => "human",
            Witness::Machine { .. } => "machine",
        }
    }
}

/// The bytes a record was taken against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchSubject {
    /// What these bytes are, in one clause, for a person reading the record — *"the mock driver"*,
    /// *"the control loop and its driver"*. Free text, and never parsed.
    pub what: String,
    /// Every artefact the measurement ran against, by path and sha256.
    ///
    /// Kept in the record rather than reduced to the digest, because a digest that does not match
    /// is a dead end unless the reader can see *what* it was over. This is the list [`digest`]
    /// is computed from, and parsing checks the two agree.
    ///
    /// [`digest`]: BenchSubject::digest
    pub artefacts: Vec<DigestFile>,
    /// The digest over [`artefacts`](BenchSubject::artefacts), under
    /// [`crate::digest::BENCH_SUBJECT_DOMAIN`].
    pub digest: String,
}

/// One bench session, written down.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchRecord {
    /// The bytes it was taken against.
    pub subject: BenchSubject,
    /// Who was there.
    pub witness: Witness,
    /// When the session ran, RFC 3339. §15 decays an award from the **oldest** bench result, so a
    /// consumer computes an age from this and a record without one is refused rather than read as
    /// fresh.
    pub ran_at: String,
    /// What it concluded, one entry per test. Every entry is a `bench` test.
    pub results: Vec<SctsResult>,
}

/// Why a record was refused.
///
/// Every variant names the offending input rather than saying the record is invalid, because the
/// next action differs completely between *"this file is for another build"* and *"this file
/// records a sim test as though a bench session had produced it"*.
#[derive(Debug, Clone, PartialEq)]
pub enum BenchError {
    /// Not this schema, or no schema at all.
    Schema {
        /// What the document carried, or the empty string when it carried nothing.
        found: String,
    },
    /// A required field is absent, or is not the type it has to be.
    Field {
        /// Dotted path to the field.
        name: String,
        /// What is wrong with it.
        detail: String,
    },
    /// A `witness.kind` this vocabulary does not have.
    WitnessKind {
        /// The spelling found.
        found: String,
    },
    /// The subject names no artefacts, so there is nothing for the result to be about.
    NoArtefacts,
    /// The artefact list cannot be hashed — a path that is not one, a hash that is not one.
    Subject(DigestError),
    /// The record's declared digest is not the digest of the list it carries.
    SubjectMismatch {
        /// What the record claims.
        declared: String,
        /// What its own artefact list hashes to.
        computed: String,
    },
    /// The record concludes nothing.
    NoResults,
    /// A result for a test §15 does not have.
    UnknownTest {
        /// The id as written.
        test_id: String,
    },
    /// A result in the wrong document.
    ///
    /// The sharpest refusal here, and it fires in both directions. A `sim` result inside a bench
    /// record is a claim that a free run cost a day and a human, which folded into an award would
    /// be §15's own worst case laundered through a file format. A `bench` result inside a results
    /// document is the same claim with the digest binding removed, which is worse, because nothing
    /// downstream could tell.
    WrongSuite {
        /// The id as written.
        test_id: String,
        /// The suite §15 actually puts it in.
        suite: &'static str,
        /// Which document refused it.
        admits: Admits,
    },
    /// Two results for one test. The award takes the last, and a record that cannot say which run
    /// came second is a record whose meaning depends on a sorter.
    DuplicateTest {
        /// The id that appears twice.
        test_id: String,
    },
    /// A quantitative test passed with no number.
    ///
    /// §15's point about these is that *"a driver returning success is not evidence"*: the value of
    /// `safety.estop.latency` is the milliseconds, and a bare `pass` on it is the sentence the test
    /// exists to refuse.
    Unmeasured {
        /// The id.
        test_id: String,
        /// The unit the number would have been in.
        unit: &'static str,
    },
    /// A result carrying an outcome this vocabulary does not have.
    Outcome {
        /// The id it belongs to.
        test_id: String,
        /// The spelling found.
        found: String,
    },
    /// A result that ran but will not say when.
    Undated {
        /// The id.
        test_id: String,
    },
    /// A skipped result carrying a date.
    ///
    /// `skynetd-io::conformance` writes an empty `ran_at` for a skip on purpose, because §15 decays
    /// an award from the oldest bench result and *"a skipped test carrying today's date would be a
    /// gap that looked fresh"*. A record may not undo that by hand.
    SkippedButDated {
        /// The id.
        test_id: String,
        /// The date it should not have.
        ran_at: String,
    },
    /// The record is for other bytes than the ones the caller holds. §12's `BenchPassStale`, in
    /// general form.
    Stale {
        /// What the record was taken against.
        recorded: String,
        /// What the caller's own bytes hash to.
        observed: String,
    },
}

impl core::fmt::Display for BenchError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Schema { found } if found.is_empty() => write!(
                f,
                "no `schema` field — a bench record must say it is one, and this document says \
                 nothing. Expected `{BENCH_RECORD_SCHEMA}`"
            ),
            Self::Schema { found } => write!(
                f,
                "`schema` is `{found}`, not `{BENCH_RECORD_SCHEMA}`. Nothing here can read that \
                 shape, and guessing at it would be worse than refusing"
            ),
            Self::Field { name, detail } => write!(f, "`{name}`: {detail}"),
            Self::WitnessKind { found } => write!(
                f,
                "`witness.kind` is `{found}`; it is `human` (a person was present, §15) or \
                 `machine` (nobody was, which is legitimate for a test that needs a machine rather \
                 than a body — see DRY-COVERAGE-PLAN.md Decisions 4)"
            ),
            Self::NoArtefacts => write!(
                f,
                "`subject.artefacts` is empty, so this record is bound to nothing. A bench result \
                 that does not say which bytes it measured cannot be checked against any"
            ),
            Self::Subject(err) => write!(f, "`subject.artefacts`: {err}"),
            Self::SubjectMismatch { declared, computed } => write!(
                f,
                "`subject.digest` is {declared}, and the artefact list beside it hashes to \
                 {computed}. The record names one set of bytes and binds to another — regenerate \
                 it rather than correcting either half by hand"
            ),
            Self::NoResults => write!(
                f,
                "`results` is empty. A session that concluded nothing is not evidence, and §15 \
                 reads an absent result as `missing` already"
            ),
            Self::UnknownTest { test_id } => write!(
                f,
                "`{test_id}` is not an SCTS test. §15 has 36 and this is none of them — check the \
                 spelling against `skynet driver check`, which prints every id it touches"
            ),
            Self::WrongSuite {
                test_id,
                suite,
                admits: Admits::OnlyBench,
            } => write!(
                f,
                "`{test_id}` is a `{suite}` test, and this is a bench record. A {suite} run costs \
                 nothing and a bench run costs a day; recording one as the other is the loophole \
                 §15's first award rule exists to close. Put it in a results document instead"
            ),
            Self::WrongSuite { test_id, .. } => write!(
                f,
                "`{test_id}` is a bench test, and this is a results document, which is bound to no \
                 bytes. Bench evidence only travels bound to what it measured — write it with \
                 `skynet bench record` instead"
            ),
            Self::DuplicateTest { test_id } => write!(
                f,
                "`{test_id}` has two results. An award takes the last of them, so which one counts \
                 would depend on the order they happen to be written in — record one session per \
                 file"
            ),
            Self::Unmeasured { test_id, unit } => write!(
                f,
                "`{test_id}` passed with no `value`, and it is measured in {unit}. §15's point \
                 about this test is that a success with no number is not evidence"
            ),
            Self::Outcome { test_id, found } => write!(
                f,
                "`{test_id}` has outcome `{found}`; it is one of pass, fail, skipped, \
                 not_applicable"
            ),
            Self::Undated { test_id } => write!(
                f,
                "`{test_id}` ran and carries no `ran_at`. An award decays from the oldest bench \
                 result, so a result with no date is one nothing can expire"
            ),
            Self::SkippedButDated { test_id, ran_at } => write!(
                f,
                "`{test_id}` was skipped and carries `ran_at` {ran_at}. A skip is an absence, and \
                 an absence with a fresh date on it is a gap that looks measured — leave it empty"
            ),
            Self::Stale { recorded, observed } => write!(
                f,
                "this record was taken against {recorded} and the bytes here hash to {observed}. \
                 It is evidence about another build (§12's `BenchPassStale`), so none of it is \
                 applied — re-run the bench session against these bytes"
            ),
        }
    }
}

/// The SCTS entry for an id, if §15 has one.
fn scts_test(test_id: &str) -> Option<&'static crate::certification::SctsTest> {
    SCTS_TESTS.iter().find(|test| test.id == test_id)
}

/// Read a record, refusing everything it cannot vouch for.
///
/// The order of the checks is the order a reader's questions come in — is this that kind of
/// document, what is it about, who says so, what does it conclude — and every one of them is a
/// refusal rather than a warning, because a partially-read bench record is a set of results with an
/// unknown subject.
pub fn bench_record_from_json(value: &Json) -> Result<BenchRecord, BenchError> {
    let schema = value.get_present("schema").and_then(Json::as_str);
    match schema {
        Some(BENCH_RECORD_SCHEMA) => {}
        other => {
            return Err(BenchError::Schema {
                found: other.unwrap_or_default().to_string(),
            })
        }
    }

    let subject_json = value
        .get_present("subject")
        .filter(|item| item.as_obj().is_some())
        .ok_or_else(|| BenchError::Field {
            name: "subject".to_string(),
            detail: "absent, or not an object".to_string(),
        })?;

    let what = required_str(subject_json, "what", "subject.what")?;

    let mut artefacts: Vec<DigestFile> = Vec::new();
    for (index, item) in subject_json.arr_or_empty("artefacts").iter().enumerate() {
        artefacts.push(DigestFile {
            path: required_str(item, "path", &format!("subject.artefacts[{index}].path"))?,
            sha256: required_str(
                item,
                "sha256",
                &format!("subject.artefacts[{index}].sha256"),
            )?,
        });
    }
    if artefacts.is_empty() {
        return Err(BenchError::NoArtefacts);
    }

    let declared = required_str(subject_json, "digest", "subject.digest")?;
    let computed = subject_digest(&artefacts).map_err(BenchError::Subject)?;
    if declared != computed {
        return Err(BenchError::SubjectMismatch { declared, computed });
    }

    let witness_json = value
        .get_present("witness")
        .filter(|item| item.as_obj().is_some())
        .ok_or_else(|| BenchError::Field {
            name: "witness".to_string(),
            detail:
                "absent, or not an object. §15 prices a bench result at a human being present, \
                     so a record that will not say who was there is not one"
                    .to_string(),
        })?;
    let witness = match required_str(witness_json, "kind", "witness.kind")?.as_str() {
        "human" => Witness::Human {
            name: required_str(witness_json, "name", "witness.name")?,
        },
        "machine" => Witness::Machine {
            host: required_str(witness_json, "host", "witness.host")?,
        },
        found => {
            return Err(BenchError::WitnessKind {
                found: found.to_string(),
            })
        }
    };

    let ran_at = required_str(value, "ran_at", "ran_at")?;

    let results = read_results(value, Admits::OnlyBench)?;

    Ok(BenchRecord {
        subject: BenchSubject {
            what,
            artefacts,
            digest: declared,
        },
        witness,
        ran_at,
        results,
    })
}

/// Write a record.
///
/// The inverse of [`bench_record_from_json`] exactly: everything the reader requires is written,
/// and nothing it refuses can be produced here. A producer that emitted a document its own parser
/// rejects would be two records of one format.
pub fn bench_record_to_json(record: &BenchRecord) -> Json {
    let mut artefacts = Json::arr();
    for file in &record.subject.artefacts {
        let mut item = Json::obj();
        item.set("path", Json::Str(file.path.clone()));
        item.set("sha256", Json::Str(file.sha256.clone()));
        artefacts.push(item);
    }

    let mut subject = Json::obj();
    subject.set("what", Json::Str(record.subject.what.clone()));
    subject.set("digest", Json::Str(record.subject.digest.clone()));
    subject.set("artefacts", artefacts);

    let mut witness = Json::obj();
    witness.set("kind", Json::Str(record.witness.kind().to_string()));
    match &record.witness {
        Witness::Human { name } => witness.set("name", Json::Str(name.clone())),
        Witness::Machine { host } => witness.set("host", Json::Str(host.clone())),
    };

    let mut out = Json::obj();
    out.set("schema", Json::Str(BENCH_RECORD_SCHEMA.to_string()));
    out.set("subject", subject);
    out.set("witness", witness);
    out.set("ran_at", Json::Str(record.ran_at.clone()));
    out.set("results", write_results(&record.results));
    out
}

/// Which half of §15 a document is allowed to carry.
///
/// The two documents in this module are distinguished by exactly this, and it is the reason there
/// are two: a `bench` result costs a day and a human and so arrives **bound to the bytes it
/// measured**; everything else costs a CI run and arrives as itself. A results document that could
/// carry a bench result would be a free way to write down an expensive claim, which is §15's first
/// award rule read backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admits {
    /// A bench record. Every entry is a `bench` test.
    OnlyBench,
    /// A results document. No entry is.
    ExceptBench,
}

/// Read a `results` array, with one rule about which suite may appear in it.
///
/// One reader for both documents, deliberately. Two would agree until one of them gained a field —
/// and the field they would disagree about is `value`, whose absence on a quantitative test is the
/// thing §15 says is not evidence.
fn read_results(value: &Json, admits: Admits) -> Result<Vec<SctsResult>, BenchError> {
    let mut results: Vec<SctsResult> = Vec::new();
    for (index, item) in value.arr_or_empty("results").iter().enumerate() {
        let test_id = required_str(item, "test_id", &format!("results[{index}].test_id"))?;
        let Some(test) = scts_test(&test_id) else {
            return Err(BenchError::UnknownTest { test_id });
        };
        match (admits, test.suite) {
            (Admits::OnlyBench, "bench") => {}
            (Admits::ExceptBench, suite) if suite != "bench" => {}
            (_, suite) => {
                return Err(BenchError::WrongSuite {
                    test_id,
                    suite,
                    admits,
                })
            }
        }
        if results.iter().any(|earlier| earlier.test_id == test_id) {
            return Err(BenchError::DuplicateTest { test_id });
        }

        let found = required_str(item, "outcome", &format!("results[{index}].outcome"))?;
        let Some(outcome) = SctsOutcome::parse(&found) else {
            return Err(BenchError::Outcome { test_id, found });
        };

        let value_of = item.get_present("value").and_then(Json::as_f64);
        if outcome == SctsOutcome::Pass && test.quantitative && value_of.is_none() {
            return Err(BenchError::Unmeasured {
                test_id,
                unit: test.unit.unwrap_or("a measured quantity"),
            });
        }

        let result_ran_at = item
            .get_present("ran_at")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string();
        match outcome {
            SctsOutcome::Skipped if !result_ran_at.is_empty() => {
                return Err(BenchError::SkippedButDated {
                    test_id,
                    ran_at: result_ran_at,
                })
            }
            SctsOutcome::Skipped => {}
            _ if result_ran_at.is_empty() => return Err(BenchError::Undated { test_id }),
            _ => {}
        }

        results.push(SctsResult {
            test_id,
            outcome,
            value: value_of,
            ran_at: result_ran_at,
            detail: item
                .get_present("detail")
                .and_then(Json::as_str)
                .map(ToString::to_string),
        });
    }
    if results.is_empty() {
        return Err(BenchError::NoResults);
    }
    Ok(results)
}

/// Write a `results` array. The inverse of [`read_results`], and the only writer of one.
fn write_results(results: &[SctsResult]) -> Json {
    let mut out = Json::arr();
    for result in results {
        let mut item = Json::obj();
        item.set("test_id", Json::Str(result.test_id.clone()));
        item.set("outcome", Json::Str(result.outcome.as_str().to_string()));
        item.set_opt("value", result.value.map(Json::Num));
        item.set("ran_at", Json::Str(result.ran_at.clone()));
        item.set_opt("detail", result.detail.clone().map(Json::Str));
        out.push(item);
    }
    out
}

/// The one schema string a results document may carry.
///
/// A separate document from [`BENCH_RECORD_SCHEMA`], and separate on purpose: this one is bound to
/// nothing, because the runs it records cost nothing to repeat. `skynet driver check` writes one so
/// that the sim half of an award can be carried to wherever the award is computed, which until now
/// it could not be — the command rendered its results for a person and kept them.
pub const RESULTS_SCHEMA: &str = "skynet.scts-results.v1";

/// Results from a run that needed no bench session.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultsDocument {
    /// What produced it, in the terms somebody could re-run — *"skynet driver check ./libmock.so"*.
    /// Free text, never parsed, and required: a set of results whose origin nobody wrote down is a
    /// set of results nobody can re-take.
    pub source: String,
    /// What it concluded. No entry is a `bench` test.
    pub results: Vec<SctsResult>,
}

/// Read a results document.
pub fn results_from_json(value: &Json) -> Result<ResultsDocument, BenchError> {
    match value.get_present("schema").and_then(Json::as_str) {
        Some(RESULTS_SCHEMA) => {}
        other => {
            return Err(BenchError::Schema {
                found: other.unwrap_or_default().to_string(),
            })
        }
    }
    Ok(ResultsDocument {
        source: required_str(value, "source", "source")?,
        results: read_results(value, Admits::ExceptBench)?,
    })
}

/// Write a results document.
pub fn results_to_json(document: &ResultsDocument) -> Json {
    let mut out = Json::obj();
    out.set("schema", Json::Str(RESULTS_SCHEMA.to_string()));
    out.set("source", Json::Str(document.source.clone()));
    out.set("results", write_results(&document.results));
    out
}

/// The results this record may contribute, given what the caller's own bytes hash to.
///
/// This is the whole point of the module, and it is one comparison. A caller holding a driver, a
/// bundle or a pair of binaries hashes them the same way the producer did and passes the list; a
/// record taken against anything else contributes **nothing**, rather than contributing the subset
/// that happens to look plausible.
///
/// Borrowed rather than cloned so that a caller cannot end up holding evidence detached from the
/// record it came out of — the lifetime is the check's memory.
pub fn evidence_for<'a>(
    record: &'a BenchRecord,
    observed: &[DigestFile],
) -> Result<&'a [SctsResult], BenchError> {
    let observed_digest = subject_digest(observed).map_err(BenchError::Subject)?;
    if observed_digest != record.subject.digest {
        return Err(BenchError::Stale {
            recorded: record.subject.digest.clone(),
            observed: observed_digest,
        });
    }
    Ok(&record.results)
}

/// A string field that has to be there and has to be non-empty.
///
/// `key` is what to look up and `name` is what to call it in a refusal — the two differ wherever a
/// field is nested or indexed, and deriving the second from the first is how a message ends up
/// naming a key rather than a place.
///
/// Empty is refused alongside absent, deliberately: every field here is an identifier, a date or a
/// name, and an empty one is a producer that had nothing to say filling the shape in anyway.
fn required_str(value: &Json, key: &str, name: &str) -> Result<String, BenchError> {
    match value.get_present(key).and_then(Json::as_str) {
        Some(text) if !text.is_empty() => Ok(text.to_string()),
        Some(_) => Err(BenchError::Field {
            name: name.to_string(),
            detail: "present and empty".to_string(),
        }),
        None => Err(BenchError::Field {
            name: name.to_string(),
            detail: "absent, or not a string".to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// sha256 of the empty string, and of `b"a"` — two real digests, so nothing here is testing a
    /// placeholder past the `is_hex_sha256` check.
    const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const A_SHA: &str = "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb";

    fn artefacts() -> Vec<DigestFile> {
        vec![
            DigestFile {
                path: "libmock.so".to_string(),
                sha256: EMPTY_SHA.to_string(),
            },
            DigestFile {
                path: "skynetk".to_string(),
                sha256: A_SHA.to_string(),
            },
        ]
    }

    fn record() -> BenchRecord {
        let files = artefacts();
        BenchRecord {
            subject: BenchSubject {
                what: "the control loop and its driver".to_string(),
                digest: subject_digest(&files).expect("these paths and hashes are well formed"),
                artefacts: files,
            },
            witness: Witness::Machine {
                host: "vm 6.18.44".to_string(),
            },
            ran_at: "2026-09-16T10:00:00Z".to_string(),
            results: vec![SctsResult {
                test_id: "offline.control.independent".to_string(),
                outcome: SctsOutcome::Pass,
                value: None,
                ran_at: "2026-09-16T10:00:00Z".to_string(),
                detail: Some("250 cycles in a namespace holding only loopback".to_string()),
            }],
        }
    }

    /// The document a test wants to break one field of.
    fn json() -> Json {
        bench_record_to_json(&record())
    }

    fn refuse(document: &Json) -> BenchError {
        match bench_record_from_json(document) {
            Ok(_) => panic!("this document should not have parsed"),
            Err(err) => err,
        }
    }

    #[test]
    fn a_record_survives_the_round_trip() {
        assert_eq!(bench_record_from_json(&json()), Ok(record()));
    }

    #[test]
    fn a_human_witness_survives_it_too() {
        let mut original = record();
        original.witness = Witness::Human {
            name: "A. Habek".to_string(),
        };
        let written = bench_record_to_json(&original);
        assert_eq!(
            written.get_present("witness").unwrap().get("kind"),
            Some(&Json::Str("human".to_string()))
        );
        assert_eq!(bench_record_from_json(&written), Ok(original));
    }

    #[test]
    fn a_document_that_does_not_say_what_it_is_is_refused() {
        let mut document = json();
        let Json::Obj(map) = &mut document else {
            unreachable!()
        };
        map.remove("schema");
        assert_eq!(
            refuse(&document),
            BenchError::Schema {
                found: String::new()
            }
        );

        document.set("schema", Json::Str("skynet.bench.v2".to_string()));
        assert_eq!(
            refuse(&document),
            BenchError::Schema {
                found: "skynet.bench.v2".to_string()
            }
        );
    }

    #[test]
    fn a_record_bound_to_nothing_is_refused() {
        let mut document = json();
        let mut subject = document.get_present("subject").unwrap().clone();
        subject.set("artefacts", Json::arr());
        document.set("subject", subject);
        assert_eq!(refuse(&document), BenchError::NoArtefacts);
    }

    #[test]
    fn a_record_that_names_one_set_of_bytes_and_binds_to_another_is_refused() {
        let mut document = json();
        let mut subject = document.get_present("subject").unwrap().clone();
        let declared = subject.get("digest").unwrap().as_str().unwrap().to_string();
        // One artefact's hash changed, and the digest left as it was. Believing either half would
        // be believing a record about bytes nobody has.
        let mut list = Json::arr();
        list.push({
            let mut item = Json::obj();
            item.set("path", Json::Str("libmock.so".to_string()));
            item.set("sha256", Json::Str(A_SHA.to_string()));
            item
        });
        subject.set("artefacts", list);
        document.set("subject", subject);
        match refuse(&document) {
            BenchError::SubjectMismatch {
                declared: d,
                computed,
            } => {
                assert_eq!(d, declared);
                assert_ne!(computed, declared);
            }
            other => panic!("expected a subject mismatch, got {other:?}"),
        }
    }

    #[test]
    fn an_artefact_list_that_cannot_be_hashed_is_refused_as_such() {
        let files = vec![DigestFile {
            path: "../escape".to_string(),
            sha256: EMPTY_SHA.to_string(),
        }];
        let mut original = record();
        original.subject.digest = subject_digest(&artefacts()).unwrap();
        original.subject.artefacts = files;
        // Written by hand rather than through `subject_digest`, which would have refused first —
        // this is the shape a hand-edited file has.
        let document = bench_record_to_json(&original);
        assert!(matches!(refuse(&document), BenchError::Subject(_)));
    }

    #[test]
    fn a_record_with_nobody_behind_it_is_refused() {
        let mut document = json();
        let Json::Obj(map) = &mut document else {
            unreachable!()
        };
        map.remove("witness");
        assert!(matches!(refuse(&document), BenchError::Field { name, .. } if name == "witness"));

        let mut document = json();
        let mut witness = Json::obj();
        witness.set("kind", Json::Str("simulator".to_string()));
        document.set("witness", witness);
        assert_eq!(
            refuse(&document),
            BenchError::WitnessKind {
                found: "simulator".to_string()
            }
        );

        let mut document = json();
        let mut witness = Json::obj();
        witness.set("kind", Json::Str("human".to_string()));
        document.set("witness", witness);
        assert!(
            matches!(refuse(&document), BenchError::Field { name, .. } if name == "witness.name")
        );
    }

    #[test]
    fn a_session_with_no_date_is_refused() {
        let mut document = json();
        document.set("ran_at", Json::Str(String::new()));
        assert!(matches!(
            refuse(&document),
            BenchError::Field { name, detail } if name == "ran_at" && detail == "present and empty"
        ));
    }

    #[test]
    fn a_session_that_concluded_nothing_is_refused() {
        let mut document = json();
        document.set("results", Json::arr());
        assert_eq!(refuse(&document), BenchError::NoResults);
    }

    /// The one that matters most. A `sim` result inside a bench record is a free run claiming to
    /// have cost a day and a human, and it is the exact shape §15's first award rule closes.
    #[test]
    fn a_sim_result_may_not_arrive_as_bench_evidence() {
        let mut original = record();
        original.results[0].test_id = "rhal.abi.version".to_string();
        assert_eq!(
            refuse(&bench_record_to_json(&original)),
            BenchError::WrongSuite {
                test_id: "rhal.abi.version".to_string(),
                suite: "sim",
                admits: Admits::OnlyBench,
            }
        );
    }

    #[test]
    fn a_test_fifteen_does_not_have_is_refused() {
        let mut original = record();
        original.results[0].test_id = "offline.control.independant".to_string();
        assert_eq!(
            refuse(&bench_record_to_json(&original)),
            BenchError::UnknownTest {
                test_id: "offline.control.independant".to_string()
            }
        );
    }

    #[test]
    fn two_results_for_one_test_are_refused() {
        let original = record();
        let mut document = bench_record_to_json(&original);
        let mut results = document.get_present("results").unwrap().clone();
        results.push(results.as_arr().unwrap()[0].clone());
        document.set("results", results);
        assert_eq!(
            refuse(&document),
            BenchError::DuplicateTest {
                test_id: "offline.control.independent".to_string()
            }
        );
    }

    #[test]
    fn a_measured_test_that_passed_without_a_number_is_refused() {
        let mut original = record();
        original.results[0] = SctsResult {
            test_id: "safety.estop.latency".to_string(),
            outcome: SctsOutcome::Pass,
            value: None,
            ran_at: "2026-09-16T10:00:00Z".to_string(),
            detail: None,
        };
        assert_eq!(
            refuse(&bench_record_to_json(&original)),
            BenchError::Unmeasured {
                test_id: "safety.estop.latency".to_string(),
                unit: "ms"
            }
        );

        // With the number, the same record parses — so the refusal is about the absence and not
        // about the test.
        original.results[0].value = Some(12.0);
        assert!(bench_record_from_json(&bench_record_to_json(&original)).is_ok());
    }

    #[test]
    fn an_outcome_outside_the_vocabulary_is_refused() {
        let mut document = json();
        let mut results = document.get_present("results").unwrap().clone();
        let Json::Arr(items) = &mut results else {
            unreachable!()
        };
        items[0].set("outcome", Json::Str("inconclusive".to_string()));
        document.set("results", results);
        assert_eq!(
            refuse(&document),
            BenchError::Outcome {
                test_id: "offline.control.independent".to_string(),
                found: "inconclusive".to_string()
            }
        );
    }

    /// Both directions of §15's date rule, because they are opposite mistakes: a measurement that
    /// nothing can expire, and a gap that looks measured.
    #[test]
    fn a_date_is_required_of_a_run_and_refused_of_a_skip() {
        let mut original = record();
        original.results[0].ran_at = String::new();
        assert_eq!(
            refuse(&bench_record_to_json(&original)),
            BenchError::Undated {
                test_id: "offline.control.independent".to_string()
            }
        );

        let mut original = record();
        original.results[0].outcome = SctsOutcome::Skipped;
        assert_eq!(
            refuse(&bench_record_to_json(&original)),
            BenchError::SkippedButDated {
                test_id: "offline.control.independent".to_string(),
                ran_at: "2026-09-16T10:00:00Z".to_string()
            }
        );

        // A skip with no date is a legitimate record of a test the session did not reach.
        let mut original = record();
        original.results[0].outcome = SctsOutcome::Skipped;
        original.results[0].ran_at = String::new();
        assert!(bench_record_from_json(&bench_record_to_json(&original)).is_ok());
    }

    #[test]
    fn evidence_is_handed_over_only_for_the_bytes_it_was_taken_against() {
        let original = record();
        assert_eq!(
            evidence_for(&original, &artefacts()),
            Ok(original.results.as_slice())
        );

        // One byte of one artefact different — which is what a rebuild looks like from here.
        let mut rebuilt = artefacts();
        rebuilt[0].sha256 = A_SHA.to_string();
        match evidence_for(&original, &rebuilt) {
            Err(BenchError::Stale { recorded, observed }) => {
                assert_eq!(recorded, original.subject.digest);
                assert_ne!(observed, recorded);
            }
            other => panic!("expected a stale refusal, got {other:?}"),
        }

        // And an artefact missing entirely, which is the other way a subject stops matching.
        assert!(matches!(
            evidence_for(&original, &rebuilt[..1]),
            Err(BenchError::Stale { .. })
        ));
    }

    /// The domain separator, asserted rather than assumed. Two digests over one file list must not
    /// be equal, or a package identity and a bench subject would be interchangeable values.
    #[test]
    fn a_bench_subject_and_a_package_version_never_hash_alike() {
        let files = artefacts();
        let subject = subject_digest(&files).unwrap();
        let version = crate::digest::version_digest("driver", "", &Json::obj(), &files).unwrap();
        assert_ne!(subject, version);
        assert!(crate::digest::is_hex_sha256(&subject));
    }

    fn results_document() -> ResultsDocument {
        ResultsDocument {
            source: "skynet driver check ./libmock.so".to_string(),
            results: vec![SctsResult {
                test_id: "rhal.abi.version".to_string(),
                outcome: SctsOutcome::Pass,
                value: None,
                ran_at: "2026-09-16T10:00:00Z".to_string(),
                detail: None,
            }],
        }
    }

    #[test]
    fn a_results_document_survives_the_round_trip() {
        let original = results_document();
        assert_eq!(results_from_json(&results_to_json(&original)), Ok(original));
    }

    /// The other direction of the suite rule, and the one that would be invisible downstream: a
    /// bench result in a document that is bound to no bytes at all.
    #[test]
    fn a_bench_result_may_not_arrive_unbound() {
        let mut original = results_document();
        original.results[0].test_id = "safety.estop.independent".to_string();
        let err = results_from_json(&results_to_json(&original)).unwrap_err();
        assert_eq!(
            err,
            BenchError::WrongSuite {
                test_id: "safety.estop.independent".to_string(),
                suite: "bench",
                admits: Admits::ExceptBench,
            }
        );
        let text = format!("{err}");
        assert!(text.contains("skynet bench record"), "{text}");
    }

    #[test]
    fn a_results_document_says_where_it_came_from() {
        let mut document = results_to_json(&results_document());
        document.set("source", Json::Str(String::new()));
        assert!(matches!(
            results_from_json(&document),
            Err(BenchError::Field { name, .. }) if name == "source"
        ));
    }

    /// The two documents cannot be read as each other, in both directions, which is what keeps the
    /// digest binding from being optional.
    #[test]
    fn neither_document_parses_as_the_other() {
        assert!(matches!(
            results_from_json(&json()),
            Err(BenchError::Schema { found }) if found == BENCH_RECORD_SCHEMA
        ));
        assert!(matches!(
            bench_record_from_json(&results_to_json(&results_document())),
            Err(BenchError::Schema { found }) if found == RESULTS_SCHEMA
        ));
    }

    #[test]
    fn a_refusal_says_what_to_do_next() {
        // Not a spelling assertion — each of these is the sentence a person reads when a bench
        // record will not load, and a refusal that named only the field would send them to the
        // wrong half of the problem.
        let stale = BenchError::Stale {
            recorded: EMPTY_SHA.to_string(),
            observed: A_SHA.to_string(),
        };
        let text = format!("{stale}");
        assert!(text.contains(EMPTY_SHA) && text.contains(A_SHA), "{text}");
        assert!(text.contains("re-run the bench session"), "{text}");

        let not_bench = format!(
            "{}",
            BenchError::WrongSuite {
                test_id: "rhal.abi.version".to_string(),
                suite: "sim",
                admits: Admits::OnlyBench,
            }
        );
        assert!(not_bench.contains("costs a day"), "{not_bench}");
    }
}
