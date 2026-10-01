//! `skynet skill check <dir>` — §54's contract, held against a package before anybody publishes it.
//!
//! ```text
//! skynet skill check ./skills/openrobot/park
//! ```
//!
//! # Why this command exists on the workstation and not only on the robot
//!
//! Every rule it checks is already enforced somewhere. §37.1 refuses an install whose digest does
//! not match, [`skynetd_io::admit`] refuses a package that over-requests, §54.2 refuses a skill
//! whose precondition is false or unresolvable, and §54.5 refuses a call graph with a cycle in it.
//! A publisher without this command finds every one of them out in the same place — on a robot, at
//! install time, from a fleet that has already fetched the blob.
//!
//! The ones worth catching earlier are not the interesting ones. They are the typos:
//!
//! * `runtime.entrypoint` naming a file the package does not contain, which is a skill that
//!   installs, activates, and fails to start with a message about the manifest;
//! * a precondition reading `robot.state.batery_percent`, which is **unresolvable on every robot
//!   that will ever exist** and therefore refuses the skill for ever — and does so with a message
//!   that correctly names a path nobody can supply;
//! * `requests` naming a token `embodiment.capabilities` never declared, which admission calls
//!   `over_request` and refuses.
//!
//! None of those need a robot to find. All of them cost a fleet deployment to find without this.
//!
//! # The one source of truth rule, and how it is kept here
//!
//! Nothing in this module decides anything. Every verdict is computed by the function that decides
//! it on the robot — [`skynet_contracts::digest::version_digest`] for identity,
//! [`skynetd_io::admit::composition_fault`] for §54.5, [`skynet_contracts::skill::read_clauses`]
//! and [`skynet_contracts::predicate::evaluate`] for §54.2,
//! [`skynet_contracts::skill::skill_deadline`] for §54.4's timeout. A second implementation of any
//! of them would be a workstation that passes packages the robot refuses, which is worse than no
//! command at all.
//!
//! The one thing this module does that the robot does not is [`addressable_paths`]: the vocabulary of
//! path prefixes a precondition may read. It is **derived** by building a context with
//! [`live_context`] from inputs with every branch populated and reading back the keys it produced,
//! rather than by retyping §54.2's table. A root added to the live context is addressable here on
//! the same commit, and one removed stops being addressable, with nothing to keep in step.
//!
//! # What it cannot check, and says so rather than passing
//!
//! * **Whether the entry point runs.** It is a file, and this is a workstation. §15's rule holds:
//!   absence is not a pass, so the check is reported as not measured with the reason.
//! * **Whether a precondition is *true*.** That is a property of a robot at a moment. What is
//!   checked is that the clause is decidable in principle — a well-formed predicate naming paths
//!   the context publishes.
//! * **Whether the transport plan is acceptable.** That needs a body. `--body <descriptor.json>`
//!   supplies one and the check runs; without one it is reported as not measured, naming the flag.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Every check id this module can emit, each spelled once. The emission sites reference these
/// rather than repeating the strings, so a check id exists in exactly one place and the pin
/// test below fails the moment the catalog stops explaining one of them.
pub(crate) const CHECK_PACKAGE_MANIFEST_PRESENT: &str = "package.manifest.present";
pub(crate) const CHECK_PACKAGE_MANIFEST_DECODES: &str = "package.manifest.decodes";
pub(crate) const CHECK_PACKAGE_KIND_SKILL: &str = "package.kind.skill";
pub(crate) const CHECK_PACKAGE_ISOLATION_SANDBOXED: &str = "package.isolation.sandboxed";
pub(crate) const CHECK_PACKAGE_ENTRYPOINT_DECLARED: &str = "package.entrypoint.declared";
pub(crate) const CHECK_PACKAGE_ENTRYPOINT_INSIDE: &str = "package.entrypoint.inside";
pub(crate) const CHECK_PACKAGE_ENTRYPOINT_PRESENT: &str = "package.entrypoint.present";
pub(crate) const CHECK_PACKAGE_ENTRYPOINT_RUNS: &str = "package.entrypoint.runs";
pub(crate) const CHECK_PACKAGE_DIGEST_COMPUTABLE: &str = "package.digest.computable";
pub(crate) const CHECK_SKILL_REQUESTS_DECLARED: &str = "skill.requests.declared";
pub(crate) const CHECK_SKILL_CAPABILITIES_CANONICAL: &str = "skill.capabilities.canonical";
pub(crate) const CHECK_SKILL_PRECONDITIONS_DECIDABLE: &str = "skill.preconditions.decidable";
pub(crate) const CHECK_SKILL_PRECONDITIONS_ENFORCEABLE: &str = "skill.preconditions.enforceable";
pub(crate) const CHECK_SKILL_SUCCESS_DECIDABLE: &str = "skill.success.decidable";
pub(crate) const CHECK_SKILL_SUCCESS_INDEPENDENT: &str = "skill.success.independent";
pub(crate) const CHECK_SKILL_TIMEOUT_DECLARED: &str = "skill.timeout.declared";
pub(crate) const CHECK_SKILL_COMPOSITION_ACYCLIC: &str = "skill.composition.acyclic";
pub(crate) const CHECK_SKILL_SAFETY_NOTES_PRESENT: &str = "skill.safety_notes.present";
pub(crate) const CHECK_SKILL_TRANSPORT_RESOLVES: &str = "skill.transport.resolves";
pub(crate) const CHECK_SKILL_CONTEXT_SUPPLIED: &str = "skill.context.supplied";
pub(crate) const CHECK_SKILL_CONTEXT_ON_BODY: &str = "skill.context.on_body";

/// Every check id, for the catalog pin test — and only for it, so it compiles out of
/// the binary it pins.
#[cfg(test)]
pub(crate) const SKILL_CHECK_IDS: &[&str] = &[
    CHECK_PACKAGE_MANIFEST_PRESENT,
    CHECK_PACKAGE_MANIFEST_DECODES,
    CHECK_PACKAGE_KIND_SKILL,
    CHECK_PACKAGE_ISOLATION_SANDBOXED,
    CHECK_PACKAGE_ENTRYPOINT_DECLARED,
    CHECK_PACKAGE_ENTRYPOINT_INSIDE,
    CHECK_PACKAGE_ENTRYPOINT_PRESENT,
    CHECK_PACKAGE_ENTRYPOINT_RUNS,
    CHECK_PACKAGE_DIGEST_COMPUTABLE,
    CHECK_SKILL_REQUESTS_DECLARED,
    CHECK_SKILL_CAPABILITIES_CANONICAL,
    CHECK_SKILL_PRECONDITIONS_DECIDABLE,
    CHECK_SKILL_PRECONDITIONS_ENFORCEABLE,
    CHECK_SKILL_SUCCESS_DECIDABLE,
    CHECK_SKILL_SUCCESS_INDEPENDENT,
    CHECK_SKILL_TIMEOUT_DECLARED,
    CHECK_SKILL_COMPOSITION_ACYCLIC,
    CHECK_SKILL_SAFETY_NOTES_PRESENT,
    CHECK_SKILL_TRANSPORT_RESOLVES,
    CHECK_SKILL_CONTEXT_SUPPLIED,
    CHECK_SKILL_CONTEXT_ON_BODY,
];

use skynet_contracts::admission::{classify_token, TokenClass};
use skynet_contracts::calibration::Validity;
use skynet_contracts::digest::{check_path, version_digest, DigestFile};
use skynet_contracts::embodiment::FrameFault;
use skynet_contracts::json::{descriptor_from_json, manifest_from_json};
use skynet_contracts::live::{
    live_context, BasePose, CalibrationFact, DriveState, JointReading, LiveInputs, NeverFilled,
    DAEMON_NEVER_FILLS,
};
use skynet_contracts::opstate::OperatingState;
use skynet_contracts::predicate::{evaluate, Outcome as Verdict, UnevaluableReason};
use skynet_contracts::skill::{
    read_clauses, skill_deadline, Clause, ClauseOrigin, DEFAULT_OVERRUN_FACTOR,
};
use skynet_contracts::transport::{resolve_transport, ResolveTransportOptions};
use skynet_json::Json;
use skynetd_io::admit::{card_field, composition_fault, composition_refusal, declared_calls};
use skynetd_io::store::MANIFEST_FILE;

use crate::args::{Args, Spec};
use crate::check::{CheckResult, Outcome};
use crate::term;

/// What `--help` prints for this command.
pub const USAGE: &str = "\
skynet skill check <directory> [--body <descriptor.json>]

  Hold a skill package against §54 before it is published. The directory is the
  one you would `skynet push` — a manifest.json at its root and the version's
  files beside it.

  --body <descriptor.json>   Resolve §6's transport against this embodiment, the
                             way §37.1's install stage does. Without it that check
                             is reported as not measured rather than passed.

  Exit code is 0 only when nothing failed and nothing went unmeasured: absence is
  not a pass (§15).";

const CHECK_SPEC: Spec = Spec {
    values: &["body"],
    switches: &[],
};

/// Everything one run produced.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Every check, in the order run.
    pub results: Vec<CheckResult>,
    /// `id` from the manifest, when it carried a parseable one.
    pub id: Option<String>,
    /// `version` from the manifest.
    pub version: Option<String>,
    /// The digest these bytes actually hash to, when one could be computed.
    ///
    /// Reported rather than compared. §37.1 makes the digest the identity a robot installs by, and
    /// a publisher's next command needs the number — `skynet install … --digest <this>`.
    pub digest: Option<String>,
}

impl Report {
    fn push(&mut self, result: CheckResult) {
        self.results.push(result);
    }

    fn tally(&self, outcome: Outcome) -> usize {
        self.results
            .iter()
            .filter(|result| result.outcome == outcome)
            .count()
    }
}

/// Dispatch `skynet skill check`.
pub fn run(rest: &[String]) -> i32 {
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return 0;
    }
    let args = match Args::parse(rest, &CHECK_SPEC) {
        Ok(args) => args,
        Err(message) => return term::fail(&message),
    };
    let [directory] = args.positional.as_slice() else {
        return term::fail(&format!(
            "expected exactly one package directory, got {}.\n\n{USAGE}",
            args.positional.len()
        ));
    };

    let directory = PathBuf::from(directory);
    if !directory.is_dir() {
        return term::fail(&format!(
            "{} is not a directory. `skill check` takes the package directory — the one holding \
             manifest.json — rather than the manifest itself.",
            directory.display()
        ));
    }

    let body = match args.value("body") {
        None => None,
        Some(path) => match read_json(Path::new(path)) {
            Ok(json) => Some(json),
            Err(message) => return term::fail(&message),
        },
    };

    let report = inspect(&directory, body.as_ref());
    print(&report)
}

/// Read and parse a JSON file, with the path in every failure.
fn read_json(path: &Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Json::parse(&text).map_err(|error| format!("{} does not parse: {error:?}", path.display()))
}

/// Every file under `root`, as the store walks one: relative paths with `/` separators.
///
/// Sorted, because the digest's file list is order-dependent and a directory walk is not ordered by
/// the filesystem. `.git` and the editor droppings are **not** skipped: `skynet push` uploads what
/// is in the directory, so a check that ignored a file the upload would send would be checking a
/// different package from the one that gets published. A directory carrying files it should not is
/// a finding, and it shows up as a digest the publisher does not expect.
fn walk(root: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    collect(root, root, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read {}: {error}", dir.display()))?;
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out)?;
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| format!("{} escaped the package root", path.display()))?
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        out.push((relative, bytes));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    String::from_utf8_lossy(&skynet_sha256::hex_bytes(bytes)).into_owned()
}

/// The vocabulary a precondition may read, derived from the context builder rather than listed.
///
/// §54.2's table names seven branches and they are not all the same shape. `robot.state.operating`
/// is a key the OS chooses and is the same on every robot; `robot.state.joints.shoulder_pitch` is a
/// key the *body* chooses and this command has not been given a descriptor. A checker that demanded
/// the whole path resolve would refuse every precondition about a real joint; one that stopped at
/// two segments would pass `robot.state.batery_percent`, which resolves on no robot that will ever
/// exist.
///
/// So the two kinds are told apart by **building the context twice** with different body-chosen
/// names, and comparing. An object whose keys differ between the two runs is one whose keys are not
/// the OS's — joint names, calibration set names, and the cell's `env` — so everything below it is
/// open. Every other path is fixed vocabulary and has to resolve exactly.
///
/// Nothing here is retyped from §54.2. A branch added to [`live_context`] is addressable on the
/// same commit, and a leaf renamed there stops being addressable, with nothing to keep in step.
fn addressable_paths() -> (BTreeSet<String>, BTreeSet<String>) {
    let first = probe_context("alpha");
    let second = probe_context("beta");

    let mut fixed_a = BTreeSet::new();
    let mut objects_a = BTreeMap::new();
    describe_paths(&first, "", &mut fixed_a, &mut objects_a);
    let mut fixed_b = BTreeSet::new();
    let mut objects_b = BTreeMap::new();
    describe_paths(&second, "", &mut fixed_b, &mut objects_b);

    let open: BTreeSet<String> = objects_a
        .iter()
        .filter(|(path, keys)| objects_b.get(*path) != Some(keys))
        .map(|(path, _)| path.clone())
        .collect();

    // The intersection rather than either side: a path that appeared in only one run is one of the
    // names that differed, and it is reachable through its open parent rather than as vocabulary.
    let fixed = fixed_a.intersection(&fixed_b).cloned().collect();
    (fixed, open)
}

/// A live context with every branch populated, and every body-chosen name taken from `name`.
fn probe_context(name: &str) -> Json {
    let mut env = Json::obj();
    env.set(name, Json::Bool(true));
    live_context(&LiveInputs {
        joints: vec![JointReading {
            name: name.to_string(),
            position: 0.0,
            velocity: Some(0.0),
            effort: Some(0.0),
        }],
        drive: Some(DriveState {
            estop: false,
            degraded: false,
            braked: false,
            homing: "complete".to_string(),
            fault_code: Some(0.0),
        }),
        gripper: Some(0.0),
        battery_percent: Some(100.0),
        base_pose: Some(BasePose {
            xyz: [0.0; 3],
            quaternion: [0.0, 0.0, 0.0, 1.0],
        }),
        operating_state: Some(OperatingState::Running),
        guards_active: Some(vec!["envelope".to_string()]),
        guards_unavailable: vec!["workspace".to_string()],
        calibration: vec![CalibrationFact {
            set: name.to_string(),
            validity: Validity::Valid {
                checked: Vec::new(),
            },
            age_s: Some(0.0),
        }],
        frames_resolved: vec!["base".to_string()],
        frames_fault: Some(FrameFault {
            code: "cycle".to_string(),
            subjects: Vec::new(),
            detail: String::new(),
        }),
        tier: Some("guest".to_string()),
        grants: vec!["manipulation.arm".to_string()],
        env: Some(env),
    })
    .context
}

/// Collect every dotted path in a document, and the key set of every object in it.
fn describe_paths(
    value: &Json,
    prefix: &str,
    paths: &mut BTreeSet<String>,
    objects: &mut BTreeMap<String, BTreeSet<String>>,
) {
    let Some(fields) = value.as_obj() else {
        return;
    };
    objects.insert(prefix.to_string(), fields.keys().cloned().collect());
    for (key, child) in fields {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        paths.insert(path.clone());
        describe_paths(child, &path, paths, objects);
    }
}

/// Whether a predicate may read this path.
///
/// Fixed vocabulary resolves exactly. Anything under an open branch is permitted at any depth,
/// because the names below it belong to the body or to the operator's cell rather than to the OS.
fn addressable(path: &str, vocabulary: &(BTreeSet<String>, BTreeSet<String>)) -> bool {
    let (fixed, open) = vocabulary;
    if fixed.contains(path) {
        return true;
    }
    let mut prefix = String::new();
    for segment in path.split('.') {
        if !prefix.is_empty() {
            prefix.push('.');
        }
        prefix.push_str(segment);
        if open.contains(&prefix) {
            return true;
        }
    }
    false
}

/// Every leaf path in a document: a key whose value is not a non-empty object.
fn leaf_paths(value: &Json, prefix: &str, into: &mut BTreeSet<String>) {
    match value.as_obj() {
        Some(fields) if !fields.is_empty() => {
            for (key, child) in fields {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                leaf_paths(child, &path, into);
            }
        }
        _ => {
            if !prefix.is_empty() {
                into.insert(prefix.to_string());
            }
        }
    }
}

/// The branches of §54.2's context the daemon never fills, each with the leaf paths it would add.
///
/// [`addressable_paths`] answers whether a path is in §54.2's grammar, and it answers from a context
/// with every branch populated, so it has always said yes to `robot.state.gripper`. This answers the
/// other question — *does a robot supply it?* — from [`DAEMON_NEVER_FILLS`], which is the daemon's
/// own account of itself and which a test in `skynetd-io` holds to what the daemon does. The paths
/// are derived by building the context with and without each field, so a leaf renamed in
/// [`live_context`] is renamed here with nothing to keep in step (item 12.F.3).
fn unsupplied_branches() -> Vec<(NeverFilled, BTreeSet<String>)> {
    let mut empty = BTreeSet::new();
    leaf_paths(
        &live_context(&LiveInputs::default()).context,
        "",
        &mut empty,
    );
    DAEMON_NEVER_FILLS
        .iter()
        .map(|row| {
            let mut inputs = LiveInputs::default();
            (row.fill)(&mut inputs);
            let mut filled = BTreeSet::new();
            leaf_paths(&live_context(&inputs).context, "", &mut filled);
            (*row, filled.difference(&empty).cloned().collect())
        })
        .collect()
}

/// The reads in these clauses that name a branch the daemon never fills, one line each.
fn unsupplied_reads(
    clauses: &[Clause],
    unsupplied: &[(NeverFilled, BTreeSet<String>)],
    what: &str,
) -> Vec<String> {
    let mut lines = Vec::new();
    for (index, clause) in clauses.iter().enumerate() {
        let Some(predicate) = &clause.predicate else {
            continue;
        };
        let mut paths = Vec::new();
        paths_in(predicate, &mut paths);
        for path in paths {
            for (row, leaves) in unsupplied {
                let hit = leaves
                    .iter()
                    .any(|leaf| path == *leaf || path.starts_with(&format!("{leaf}.")));
                if hit {
                    lines.push(format!(
                        "{what} [{index}] {}: `{path}` — {}",
                        clause.statement, row.why
                    ));
                }
            }
        }
    }
    lines
}

/// The joints these clauses read by name, as (clause index, path, joint name).
fn joint_reads(clauses: &[Clause]) -> Vec<(usize, String, String)> {
    const BRANCHES: [&str; 3] = [
        "robot.state.joints.",
        "robot.state.velocity.",
        "robot.state.effort.",
    ];
    let mut reads = Vec::new();
    for (index, clause) in clauses.iter().enumerate() {
        let Some(predicate) = &clause.predicate else {
            continue;
        };
        let mut paths = Vec::new();
        paths_in(predicate, &mut paths);
        for path in paths {
            for branch in BRANCHES {
                if let Some(name) = path.strip_prefix(branch) {
                    reads.push((
                        index,
                        path.clone(),
                        name.split('.').next().unwrap_or("").to_string(),
                    ));
                }
            }
        }
    }
    reads
}

/// Every `path` a predicate reads, in the order the document declares them.
///
/// A walk of the JSON rather than a call into the evaluator, because the evaluator answers about
/// *one* context and stops at the first clause that decides. `and` short-circuits, so a malformed
/// path in the second operand of a satisfied `and` is never reached — and it is exactly the sort of
/// typo that then refuses the skill on the first robot where the first operand is false.
fn paths_in(predicate: &Json, into: &mut Vec<String>) {
    match predicate {
        Json::Obj(fields) => {
            for (key, value) in fields {
                if key == "path" {
                    if let Some(text) = value.as_str() {
                        into.push(text.to_string());
                    }
                    continue;
                }
                paths_in(value, into);
            }
        }
        Json::Arr(items) => {
            for item in items {
                paths_in(item, into);
            }
        }
        _ => {}
    }
}

/// A structural fault in a predicate: one visible without a robot.
///
/// [`UnevaluableReason`] has seven variants and they divide cleanly. Four are properties of the
/// document — there is no predicate here, this operator does not exist, nothing to read, nothing to
/// negate — and a publisher can fix every one of them from the manifest alone. The other three
/// (`unresolved_path`, `not_a_number`, `not_a_version`) are properties of the *robot* the clause is
/// evaluated against, and a workstation that called them defects would refuse every precondition
/// about a battery on a package with no battery in front of it.
fn structural(reason: UnevaluableReason) -> bool {
    matches!(
        reason,
        UnevaluableReason::Malformed
            | UnevaluableReason::UnknownOp
            | UnevaluableReason::MissingPath
            | UnevaluableReason::MissingOperand
    )
}

/// Check one set of clauses, returning the results and the count that declare no predicate.
fn clause_faults(
    clauses: &[Clause],
    vocabulary: &(BTreeSet<String>, BTreeSet<String>),
) -> (Vec<String>, usize) {
    let mut faults = Vec::new();
    let mut prose = 0;
    for (index, clause) in clauses.iter().enumerate() {
        let Some(predicate) = &clause.predicate else {
            prose += 1;
            continue;
        };

        // Evaluated against the empty object on purpose. Every path is then unresolved, which is
        // one of the three reasons this deliberately ignores — so anything that comes back
        // structural is structural whatever robot it meets.
        let outcome = evaluate(Some(predicate), &Json::obj());
        if outcome.outcome == Verdict::Unevaluable {
            if let Some(reason) = outcome.reason {
                if structural(reason) {
                    faults.push(format!(
                        "[{index}] {}: {}",
                        clause.statement,
                        describe(reason, outcome.op.as_deref())
                    ));
                    continue;
                }
            }
        }

        let mut paths = Vec::new();
        paths_in(predicate, &mut paths);
        if paths.is_empty() {
            faults.push(format!(
                "[{index}] {}: the predicate reads no path, so it decides the same thing on every \
                 robot",
                clause.statement
            ));
            continue;
        }
        for path in paths {
            if !addressable(&path, vocabulary) {
                faults.push(format!(
                    "[{index}] {}: `{path}` is not a path §54.2 publishes, so this clause is \
                     unresolvable on every robot and refuses the skill for ever",
                    clause.statement
                ));
            }
        }
    }
    (faults, prose)
}

/// One sentence per structural reason, naming what to do about it.
fn describe(reason: UnevaluableReason, op: Option<&str>) -> String {
    match reason {
        UnevaluableReason::Malformed => {
            "not a predicate — an object with a string `op` is the shape §54.2 evaluates"
                .to_string()
        }
        UnevaluableReason::UnknownOp => format!(
            "`{}` is not an operator this OS implements",
            op.unwrap_or("(unnamed)")
        ),
        UnevaluableReason::MissingPath => {
            "names no `path`, so there is nothing to read".to_string()
        }
        UnevaluableReason::MissingOperand => "a `not` with nothing under it".to_string(),
        other => format!("unevaluable: {}", other.as_str()),
    }
}

/// Run every check over a package directory.
pub fn inspect(directory: &Path, body: Option<&Json>) -> Report {
    let mut report = Report::default();

    // ── package.manifest.present ────────────────────────────────────────────
    let manifest_path = directory.join(MANIFEST_FILE);
    let manifest = match read_json(&manifest_path) {
        Ok(manifest) => {
            report.push(CheckResult::pass(
                CHECK_PACKAGE_MANIFEST_PRESENT,
                "manifest.json at the package root",
            ));
            manifest
        }
        Err(detail) => {
            report.push(CheckResult::fail(
                CHECK_PACKAGE_MANIFEST_PRESENT,
                "manifest.json at the package root",
                detail,
            ));
            // Nothing below this reads anything but the manifest, so there is no point continuing
            // and every remaining check would report the same one fact a second time.
            return report;
        }
    };

    // ── package.manifest.decodes ────────────────────────────────────────────
    let decoded = manifest_from_json(&manifest);
    match &decoded {
        Ok(decoded) => {
            report.id = Some(decoded.id.clone());
            report.version = Some(decoded.version.clone());
            report.push(CheckResult::pass(
                CHECK_PACKAGE_MANIFEST_DECODES,
                "decodes as a package manifest (§8)",
            ));
        }
        Err(error) => report.push(CheckResult::fail(
            CHECK_PACKAGE_MANIFEST_DECODES,
            "decodes as a package manifest (§8)",
            format!(
                "{error:?} — a manifest a robot cannot decode is a package admission never sees"
            ),
        )),
    }

    let kind = manifest
        .get_present("kind")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let version = manifest
        .get_present("version")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let id = manifest
        .get_present("id")
        .and_then(Json::as_str)
        .unwrap_or_default();

    // ── package.kind.skill ──────────────────────────────────────────────────
    report.push(CheckResult::assert(
        CHECK_PACKAGE_KIND_SKILL,
        "`kind` is `skill`",
        kind == "skill",
        format!(
            "`{kind}` is a different package kind with different rules — §54's preconditions, \
             success criteria and timeout are a skill's, and this command checks those. A policy \
             or a driver is checked by `skynet install` and `skynet driver check` respectively."
        ),
    ));

    // ── package.isolation.sandboxed ─────────────────────────────────────────
    // §8.2's rule, and the one invariant behind it: only a driver runs unsandboxed, because the
    // Safety Kernel is the only writer to an actuator and a native skill would be a second one.
    let isolation = manifest
        .get_present("isolation")
        .and_then(Json::as_str)
        .unwrap_or_default();
    report.push(CheckResult::assert(
        CHECK_PACKAGE_ISOLATION_SANDBOXED,
        "`isolation` is `sandboxed`",
        isolation == "sandboxed",
        format!(
            "`{isolation}` — admission refuses a non-driver that asks to run unsandboxed with \
             `isolation_violation`. Only drivers touch hardware directly."
        ),
    ));

    // ── package.entrypoint.* ────────────────────────────────────────────────
    let entrypoint = manifest
        .get("runtime")
        .and_then(|runtime| runtime.get_present("entrypoint"))
        .and_then(Json::as_str)
        .unwrap_or_default();

    if entrypoint.is_empty() {
        report.push(CheckResult::fail(
            CHECK_PACKAGE_ENTRYPOINT_DECLARED,
            "`runtime.entrypoint` names a file",
            "a skill with no entry point is not a skill that fails to start; it is one that was \
             never runnable, and the supervisor says exactly that",
        ));
    } else {
        report.push(CheckResult::pass(
            CHECK_PACKAGE_ENTRYPOINT_DECLARED,
            "`runtime.entrypoint` names a file",
        ));
        report.push(CheckResult::assert(
            CHECK_PACKAGE_ENTRYPOINT_INSIDE,
            "the entry point is a path inside the package",
            check_path(entrypoint).is_ok(),
            format!(
                "`{entrypoint}` — a manifest naming a path outside the package would run something \
                 else entirely under this package's grants, so the robot refuses it"
            ),
        ));
        report.push(CheckResult::assert(
            CHECK_PACKAGE_ENTRYPOINT_PRESENT,
            "the entry point is in the directory",
            directory.join(entrypoint).is_file(),
            format!("no file at `{entrypoint}` — this package installs and then fails to start"),
        ));
        // §15's rule, stated rather than skipped. A file on a workstation is not a program on a
        // robot, and reporting this as a pass would be the exact claim the suite forbids.
        report.push(CheckResult::new(
            CHECK_PACKAGE_ENTRYPOINT_RUNS,
            "the entry point is executable on the robot",
            Outcome::Missing,
            "not measurable here: the store makes the declared entry point the one executable file \
             at stage time, and whether it runs is a property of the robot's architecture and \
             interpreter, not of this directory",
        ));
    }

    // ── package.digest.computable ───────────────────────────────────────────
    match walk(directory) {
        Err(detail) => report.push(CheckResult::fail(
            CHECK_PACKAGE_DIGEST_COMPUTABLE,
            "the package hashes to a version digest",
            detail,
        )),
        Ok(files) => {
            let digest_files: Vec<DigestFile> = files
                .iter()
                .filter(|(path, _)| path.as_str() != MANIFEST_FILE)
                .map(|(path, bytes)| DigestFile {
                    path: path.clone(),
                    sha256: hex(bytes),
                })
                .collect();
            match version_digest(kind, version, &manifest, &digest_files) {
                Ok(digest) => {
                    report.digest = Some(digest);
                    report.push(CheckResult::pass(
                        CHECK_PACKAGE_DIGEST_COMPUTABLE,
                        "the package hashes to a version digest",
                    ));
                }
                Err(error) => report.push(CheckResult::fail(
                    CHECK_PACKAGE_DIGEST_COMPUTABLE,
                    "the package hashes to a version digest",
                    format!("{error} — the robot computes the same number and refuses the install"),
                )),
            }
        }
    }

    // ── skill.requests.declared ─────────────────────────────────────────────
    // §8's narrowing rule: a package cannot request a permission for something it never declared it
    // needs, which is what stops "grant everything, use some" from being the easy path.
    if let Ok(decoded) = &decoded {
        let declared: BTreeSet<&str> = decoded
            .embodiment
            .capabilities
            .iter()
            .map(String::as_str)
            .collect();
        let over: Vec<&str> = decoded
            .requests
            .iter()
            .map(String::as_str)
            .filter(|token| !declared.contains(token))
            .collect();
        report.push(CheckResult::assert(
            CHECK_SKILL_REQUESTS_DECLARED,
            "every requested token is one the package declared it needs",
            over.is_empty(),
            format!(
                "{} — admission refuses this as `over_request`",
                over.join(", ")
            ),
        ));

        // ── skill.capabilities.canonical ────────────────────────────────────────
        // §85: the refusal a robot would give, given here by name, so a typo is fixed at the desk
        // rather than read on a robot as a capability the body lacks.
        let mut unknown: Vec<String> = Vec::new();
        for token in decoded
            .embodiment
            .capabilities
            .iter()
            .chain(decoded.requests.iter())
        {
            let class = classify_token(token);
            let named = format!("{token} ({})", class.as_str());
            if class != TokenClass::Canonical && !unknown.contains(&named) {
                unknown.push(named);
            }
        }
        for token in &decoded.embodiment.extensions {
            let class = classify_token(token);
            let named = format!("{token} (an extension, and {})", class.as_str());
            if class != TokenClass::Vendor && !unknown.contains(&named) {
                unknown.push(named);
            }
        }
        report.push(CheckResult::assert(
            CHECK_SKILL_CAPABILITIES_CANONICAL,
            "every capability is one §85 defines, and every extension a vendor's",
            unknown.is_empty(),
            format!(
                "{} — admission refuses this as `capability_unknown`",
                unknown.join(", ")
            ),
        ));
    }

    // ── skill.preconditions.* and skill.success.* ───────────────────────────
    let vocabulary = addressable_paths();

    let preconditions = read_clauses(card_field(&manifest, "preconditions"), ClauseOrigin::Skill);
    let (fault_lines, prose) = clause_faults(&preconditions, &vocabulary);
    report.push(CheckResult::assert(
        CHECK_SKILL_PRECONDITIONS_DECIDABLE,
        "every precondition predicate is well formed and addressable",
        fault_lines.is_empty(),
        fault_lines.join("; "),
    ));
    report.push(CheckResult::new(
        CHECK_SKILL_PRECONDITIONS_ENFORCEABLE,
        "preconditions the OS can enforce",
        if prose == 0 {
            Outcome::Pass
        } else {
            Outcome::Missing
        },
        format!(
            "{prose} of {} declare no predicate, so §54.2 cannot check them and they are \
             documentation. That is legal and it is the publisher's to-do list, counted.",
            preconditions.len()
        ),
    ));

    let criteria = read_clauses(
        card_field(&manifest, "success_criteria"),
        ClauseOrigin::Skill,
    );
    let (criteria_faults, criteria_prose) = clause_faults(&criteria, &vocabulary);
    report.push(CheckResult::assert(
        CHECK_SKILL_SUCCESS_DECIDABLE,
        "every success criterion predicate is well formed and addressable",
        criteria_faults.is_empty(),
        criteria_faults.join("; "),
    ));
    report.push(CheckResult::new(
        CHECK_SKILL_SUCCESS_INDEPENDENT,
        "success is judged by the OS rather than by the skill",
        if criteria.is_empty() {
            Outcome::Fail
        } else if criteria_prose == criteria.len() {
            Outcome::Missing
        } else {
            Outcome::Pass
        },
        if criteria.is_empty() {
            "no `success_criteria` at all — §54.3 then records every invocation `unlabeled`, which \
             is honest and means this skill contributes no outcome to the marketplace it publishes \
             to"
            .to_string()
        } else {
            format!(
                "{criteria_prose} of {} declare no predicate. §54.3 records those `unlabeled` \
                 rather than inventing a pass.",
                criteria.len()
            )
        },
    ));

    // ── skill.context.supplied ──────────────────────────────────────────────
    // `decidable` asks whether a path is in §54.2's grammar and says yes to every branch the
    // context builder has; this asks whether a robot's daemon puts a value there. A path it never
    // fills is refused with the path named on every robot, which is a package that can never start
    // and not one with a typo.
    let unsupplied = unsupplied_branches();
    let mut unsupplied_lines = unsupplied_reads(&preconditions, &unsupplied, "precondition");
    unsupplied_lines.extend(unsupplied_reads(
        &criteria,
        &unsupplied,
        "success criterion",
    ));
    report.push(CheckResult::assert(
        CHECK_SKILL_CONTEXT_SUPPLIED,
        "every context path a precondition or success criterion reads is one the daemon fills",
        unsupplied_lines.is_empty(),
        format!(
            "{} — §54.2's first rule refuses a clause whose path is absent, and for a precondition \
             that means the skill never starts, on any robot running this OS. Do not weaken the \
             clause to get past this: the branch needs a source, and the row in \
             `skynet_contracts::live::DAEMON_NEVER_FILLS` says where that decision is written",
            unsupplied_lines.join("; ")
        ),
    ));

    // ── skill.context.on_body ───────────────────────────────────────────────
    context_on_body(&mut report, &preconditions, &criteria, body);

    // ── skill.timeout.declared ──────────────────────────────────────────────
    let expected = card_field(&manifest, "expected_duration_seconds").and_then(Json::as_f64);
    let deadline = skill_deadline(expected, DEFAULT_OVERRUN_FACTOR);
    report.push(CheckResult::assert(
        CHECK_SKILL_TIMEOUT_DECLARED,
        "`expected_duration_seconds` bounds the invocation",
        deadline.enforceable,
        "no duration declared, so §54.4's timeout has no input and nothing stops an overrunning \
         invocation. A skill is bounded by definition — that is what makes it a skill and not a \
         policy."
            .to_string(),
    ));

    // ── skill.composition.acyclic ───────────────────────────────────────────
    let calls = declared_calls(&manifest);
    if calls.is_empty() {
        report.push(CheckResult::new(
            CHECK_SKILL_COMPOSITION_ACYCLIC,
            "the declared call graph is acyclic and within the depth limit",
            Outcome::NotApplicable,
            "this skill calls no other skill",
        ));
    } else {
        match composition_fault(id, &manifest) {
            None => report.push(CheckResult::pass(
                CHECK_SKILL_COMPOSITION_ACYCLIC,
                "the declared call graph is acyclic and within the depth limit",
            )),
            Some(fault) => report.push(CheckResult::fail(
                CHECK_SKILL_COMPOSITION_ACYCLIC,
                "the declared call graph is acyclic and within the depth limit",
                composition_refusal(id, &fault),
            )),
        }
    }

    // ── skill.safety_notes.present ──────────────────────────────────────────
    let notes = card_field(&manifest, "safety_notes")
        .and_then(Json::as_str)
        .map(str::trim)
        .unwrap_or_default();
    report.push(CheckResult::new(
        CHECK_SKILL_SAFETY_NOTES_PRESENT,
        "there is a safety note for an operator to acknowledge",
        if notes.is_empty() {
            Outcome::Missing
        } else {
            Outcome::Pass
        },
        "§54.2 displays the notes at grant time and records the acknowledgement against their \
         digest. A skill with none asks an operator to agree to a blank warning, which is the \
         version of that rule worth nothing.",
    ));

    // ── skill.transport.resolves ────────────────────────────────────────────
    transport(&mut report, &manifest, body);

    report
}

/// The joints a skill's clauses name, held against the joints the body has.
///
/// §54.2's `robot.state.joints.<name>` is keyed by the descriptor's own joint names, which §5.1
/// leaves vendor-specific on purpose, so a clause written on one body's names resolves on that body
/// and is refused — with the path named, which is right and not helpful — on any other. Without a
/// body this says it did not measure, as the transport check does; with one it names each joint the
/// body lacks and what the body calls its own (item 12.F.3). Why this is a check and not yet a
/// mapping is `docs/issues/skill-clauses-name-one-bodys-joints.md`.
fn context_on_body(
    report: &mut Report,
    preconditions: &[Clause],
    criteria: &[Clause],
    body: Option<&Json>,
) {
    const WHAT: &str =
        "every joint a precondition or success criterion names is a joint of this body";

    let mut reads: Vec<(&str, usize, String, String)> = Vec::new();
    for (what, clauses) in [
        ("precondition", preconditions),
        ("success criterion", criteria),
    ] {
        for (index, path, name) in joint_reads(clauses) {
            reads.push((what, index, path, name));
        }
    }
    if reads.is_empty() {
        report.push(CheckResult::new(
            CHECK_SKILL_CONTEXT_ON_BODY,
            WHAT,
            Outcome::NotApplicable,
            "no clause names a joint, so nothing about it depends on what a body calls its joints",
        ));
        return;
    }

    let Some(body) = body else {
        report.push(CheckResult::new(
            CHECK_SKILL_CONTEXT_ON_BODY,
            WHAT,
            Outcome::Missing,
            format!(
                "no body given, and {} clause read{} name a joint. `robot.state.joints.<name>` \
                 resolves only on a body that has a joint of that name; `--body <descriptor.json>` \
                 is what says whether this one does.",
                reads.len(),
                if reads.len() == 1 { "" } else { "s" }
            ),
        ));
        return;
    };

    let target = match descriptor_from_json(body) {
        Ok(target) => target,
        Err(error) => {
            report.push(CheckResult::fail(
                CHECK_SKILL_CONTEXT_ON_BODY,
                WHAT,
                format!("the body does not decode as an embodiment descriptor: {error:?}"),
            ));
            return;
        }
    };
    let names: BTreeSet<&str> = target
        .joints
        .iter()
        .map(|joint| joint.name.as_str())
        .collect();
    let mut lines: Vec<String> = Vec::new();
    for (what, index, path, name) in &reads {
        if !names.contains(name.as_str()) && !lines.iter().any(|line| line.contains(path.as_str()))
        {
            lines.push(format!("{what} [{index}]: `{path}`"));
        }
    }
    let mut own: Vec<&str> = names.iter().copied().take(12).collect();
    if names.len() > own.len() {
        own.push("…");
    }
    report.push(CheckResult::assert(
        CHECK_SKILL_CONTEXT_ON_BODY,
        WHAT,
        lines.is_empty(),
        format!(
            "{} — this body ({}) has no joint of that name; its joints are {}. §54.2's first rule \
             refuses each clause on this body with the path named",
            lines.join("; "),
            target.robot_slug,
            own.join(", ")
        ),
    ));
}

/// §37.1's transport stage, which needs a body and says so when it has none.
fn transport(report: &mut Report, manifest: &Json, body: Option<&Json>) {
    const WHAT: &str = "the action space reaches this body (§6, §37.1)";

    let Some(body) = body else {
        report.push(CheckResult::new(
            CHECK_SKILL_TRANSPORT_RESOLVES,
            WHAT,
            Outcome::Missing,
            "no body given. `--body <descriptor.json>` resolves the transport plan the way the \
             install stage does; without one, nothing about this skill's fit to any robot has been \
             checked.",
        ));
        return;
    };

    if manifest.get_present("source_embodiment").is_none() {
        report.push(CheckResult::new(
            CHECK_SKILL_TRANSPORT_RESOLVES,
            WHAT,
            Outcome::Missing,
            "the manifest declares no `source_embodiment`, so the transport resolver has no source \
             and raises `no_source_embodiment`: nothing about this skill's fit to this body can be \
             checked. A skill that commands a body should declare the body it was built against.",
        ));
        return;
    }

    let decoded = match manifest_from_json(manifest) {
        Ok(decoded) => decoded,
        Err(error) => {
            report.push(CheckResult::fail(
                CHECK_SKILL_TRANSPORT_RESOLVES,
                WHAT,
                format!(
                    "the manifest does not decode, so there is no source to resolve: {error:?}"
                ),
            ));
            return;
        }
    };
    let Some(source) = decoded.source_embodiment.as_ref() else {
        report.push(CheckResult::fail(
            CHECK_SKILL_TRANSPORT_RESOLVES,
            WHAT,
            "`source_embodiment` is present and does not decode. That is not the same fact as \
             absent: an action space nobody could parse is a package nobody has checked.",
        ));
        return;
    };

    let target = match descriptor_from_json(body) {
        Ok(target) => target,
        Err(error) => {
            report.push(CheckResult::fail(
                CHECK_SKILL_TRANSPORT_RESOLVES,
                WHAT,
                format!("the body does not decode as an embodiment descriptor: {error:?}"),
            ));
            return;
        }
    };

    // `allow_retarget: false` is the same value a reconcile pass uses, for the reason §6.1 gives:
    // retargeting changes what the skill is doing, so it is a decision somebody makes at install
    // rather than one a publish-time check grants itself. `requested_hz` is the rate the loop will
    // actually run at — the package's own budget rather than the rate it was built against.
    let plan = resolve_transport(
        source,
        &target,
        ResolveTransportOptions {
            allow_retarget: false,
            requested_hz: decoded.budget.control_hz,
        },
    );
    let verdict = plan.verdict.as_str();
    report.push(CheckResult::new(
        CHECK_SKILL_TRANSPORT_RESOLVES,
        WHAT,
        if verdict == "refused" {
            Outcome::Fail
        } else {
            Outcome::Pass
        },
        format!(
            "verdict `{verdict}` — §37.1 fails the install on `refused`, and anything below \
             `direct` is fidelity this skill loses on this body"
        ),
    ));
}

/// Print a report, and give back the exit code.
///
/// Zero only when nothing failed **and** nothing went unmeasured, which is `driver check`'s rule and
/// §15's: a package whose entry point nobody could check is not a package that passed.
pub fn print(report: &Report) -> i32 {
    use std::io::Write;
    let out = &mut std::io::stderr();

    let _ = writeln!(out);
    match (&report.id, &report.version) {
        (Some(id), Some(version)) => {
            let _ = writeln!(out, "  {} {}", term::bold(id), term::dim(version));
        }
        _ => {
            let _ = writeln!(out, "  {}", term::bold("(the manifest declares no id)"));
        }
    }
    let _ = writeln!(out);

    let mut areas: Vec<&str> = Vec::new();
    for result in &report.results {
        let area = result.id.split('.').next().unwrap_or("");
        if !areas.contains(&area) {
            areas.push(area);
        }
    }
    for area in areas {
        let _ = writeln!(out, "  {}", term::dim(area));
        for result in report
            .results
            .iter()
            .filter(|result| result.id.starts_with(area))
        {
            let mark = match result.outcome {
                Outcome::Pass => term::ok("pass"),
                Outcome::Fail => term::bad("FAIL"),
                Outcome::Missing => term::warn("miss"),
                Outcome::NotApplicable => term::dim("n/a "),
            };
            let _ = writeln!(out, "    {mark}  {:<32} {}", result.id, result.what);
            if !result.detail.is_empty() && result.outcome != Outcome::Pass {
                let _ = writeln!(out, "          {}", term::wrap(&result.detail, 10));
            }
        }
    }

    let _ = writeln!(out);
    if let Some(digest) = &report.digest {
        // Printed because it is what the publisher's next command needs, and because §37.1 installs
        // by this number rather than by the version label beside it.
        let _ = writeln!(out, "  {}  {}", term::dim("digest"), digest);
        let _ = writeln!(
            out,
            "  {}",
            term::dim(&term::wrap(
                "this is the identity §37.1 installs by. The manifest's own `digest` field cannot \
                 hold it — a manifest is hashed into its own digest, so a field carrying the answer \
                 changes it.",
                2
            ))
        );
        let _ = writeln!(out);
    }

    let _ = writeln!(
        out,
        "  {} passed, {} failed, {} not measured, {} not applicable",
        report.tally(Outcome::Pass),
        report.tally(Outcome::Fail),
        report.tally(Outcome::Missing),
        report.tally(Outcome::NotApplicable),
    );
    let _ = writeln!(out);

    if report.tally(Outcome::Fail) == 0 && report.tally(Outcome::Missing) == 0 {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests;
