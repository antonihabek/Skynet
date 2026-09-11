//! Skills — preconditions, success, timeout and composition (§54).
//!
//! The Rust edition of `reference/typescript/skill.ts`. Read that file for the design and for the
//! asymmetry at the centre of it; this one records what the port costs.
//!
//! Everything here is pure, **including the lifecycle**. `idle → preconditions → acquiring →
//! running → verifying → terminal` was recorded for two sessions as waiting on capability leases
//! and on §27's state machine. Half of that was already wrong when it was written —
//! `skynetd::lease` has held §25.2's table since the fifteenth session — and the other half stopped
//! being true when [`crate::opstate`] was declared. So [`advance`] is here, and it is a function of
//! a phase and the evidence gathered so far rather than a loop.
//!
//! What is still missing is the loop itself: something that calls this every cycle, runs the
//! skill's policy, and writes the black box events. The decisions it will make are frozen and
//! testable, so when something does loop it is not also inventing the rules.
//!
//! # The asymmetry, restated because it is the thing to get right
//!
//! A precondition and a success criterion are the same shape, and *not knowing* about them is
//! treated in opposite ways:
//!
//! * **A precondition nobody can evaluate refuses the skill** (§54.2). Before the skill runs,
//!   ignorance is a reason not to act.
//! * **A criterion nobody can evaluate makes the outcome `unlabeled`** (§54.3), never `failure`.
//!   After it runs, ignorance is a reason not to *claim*: calling an unmeasured run a failure
//!   poisons a dataset exactly as thoroughly as calling it a success.
//!
//! # What the port had to be careful about
//!
//! * `Math.round` rounds half toward positive infinity, and the crate's `round_half_away` — which
//!   is what a `no_std` edition has, since `f64::round` needs `libm` — rounds half away from zero.
//!   They agree on positive values only, which is why the duration and the factor are both checked
//!   for sign *before* the multiplication rather than after it.
//! * A `predicate` field that is present but is not an object or an array is **not** a predicate:
//!   the reference reaches it through `value && typeof value === 'object'`, so a number or a string
//!   there means the clause is unenforceable rather than malformed. An array *is* an object to
//!   JavaScript and does reach the evaluator, which answers `malformed` — a different report, and
//!   the corpus pins both.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

// §54.4's `running` row: "a skill is not an exemption from the kernel". The lifecycle reads §27's
// machine rather than keeping a second opinion about whether the robot is commanding.
use crate::js::js_trim;
use crate::opstate::OperatingState;
use crate::predicate::{self, Outcome, UnevaluableReason};
use crate::round_half_away;

/// Where a clause came from. §54.2: the operator may add preconditions; the skill may not remove
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClauseOrigin {
    /// The publisher's manifest.
    Skill,
    /// The site's profile (§38).
    Operator,
}

impl ClauseOrigin {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            ClauseOrigin::Skill => "skill",
            ClauseOrigin::Operator => "operator",
        }
    }
}

/// Both origins, publisher first — which is also the order clauses are evaluated in.
pub const CLAUSE_ORIGINS: [ClauseOrigin; 2] = [ClauseOrigin::Skill, ClauseOrigin::Operator];

/// What happened to one clause.
///
/// `Unenforceable` is the value the predicate engine does not have, and it belongs here rather than
/// there: the engine is asked about a predicate, and this is the case where there is none to ask
/// about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClauseOutcome {
    /// The clause holds.
    Satisfied,
    /// The clause does not hold.
    Unsatisfied,
    /// The predicate could not be decided.
    Unevaluable,
    /// There is no predicate. Prose, which the OS cannot enforce.
    Unenforceable,
}

impl ClauseOutcome {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            ClauseOutcome::Satisfied => "satisfied",
            ClauseOutcome::Unsatisfied => "unsatisfied",
            ClauseOutcome::Unevaluable => "unevaluable",
            ClauseOutcome::Unenforceable => "unenforceable",
        }
    }
}

/// Every clause outcome, in the reference's declaration order.
pub const CLAUSE_OUTCOMES: [ClauseOutcome; 4] = [
    ClauseOutcome::Satisfied,
    ClauseOutcome::Unsatisfied,
    ClauseOutcome::Unevaluable,
    ClauseOutcome::Unenforceable,
];

/// §54.2's two moments: before the skill starts, and every cycle while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Every clause, before the skill may start.
    Entry,
    /// Only the clauses marked `hold`, every cycle.
    Hold,
}

impl Phase {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            Phase::Entry => "entry",
            Phase::Hold => "hold",
        }
    }
}

/// Both phases.
pub const PHASES: [Phase; 2] = [Phase::Entry, Phase::Hold];

/// One precondition or success criterion.
///
/// `statement` is what a human reads and is never dropped: the predicate is what the OS enforces,
/// and an operator staring at a refused skill needs the sentence its author wrote.
#[derive(Debug, Clone)]
pub struct Clause {
    /// The sentence the publisher or the operator wrote.
    pub statement: String,
    /// What the OS evaluates. `None` means the OS cannot enforce this clause at all.
    pub predicate: Option<Json>,
    /// §54.2: re-checked every cycle, not only at entry. Meaningless on a success criterion.
    pub hold: bool,
    /// Where it came from. Assigned by [`resolve_clauses`], never declared.
    pub origin: ClauseOrigin,
}

/// What happened to one clause, with everything a refusal has to name.
#[derive(Debug, Clone)]
pub struct ClauseResult {
    /// Position in the resolved list, so a refusal can be pointed at.
    pub index: usize,
    /// The sentence.
    pub statement: String,
    /// Publisher or site.
    pub origin: ClauseOrigin,
    /// Whether this one is re-checked every cycle.
    pub hold: bool,
    /// What happened.
    pub outcome: ClauseOutcome,
    /// Why, when the predicate could not be decided.
    pub reason: Option<UnevaluableReason>,
    /// The path that could not be resolved, when there is one.
    pub path: Option<String>,
    /// The operator that could not decide, when there was one.
    pub op: Option<String>,
}

/// §54.2's answer: may this skill start, or continue?
#[derive(Debug, Clone)]
pub struct PreconditionReport {
    /// Which moment this report is about.
    pub phase: Phase,
    /// Only every evaluated clause being satisfied permits it.
    pub permitted: bool,
    /// The first clause that did not hold, which is what a refusal names.
    pub refused: Option<ClauseResult>,
    /// Every clause this phase evaluated, in order.
    pub results: Vec<ClauseResult>,
    /// How many declare no predicate — the publisher's to-do list, counted.
    pub unenforceable: usize,
}

/// Read a manifest's `preconditions` or `success_criteria`, in either spelling.
///
/// A bare string is prose with no predicate, which is what every skill published before §54.2
/// carries. It is not the same as *absent*: a skill that declares no preconditions has nothing to
/// check, and a skill that declares three sentences has three things the OS cannot check.
pub fn read_clauses(input: Option<&Json>, origin: ClauseOrigin) -> Vec<Clause> {
    let Some(Json::Arr(entries)) = input else {
        return Vec::new();
    };
    let mut clauses = Vec::new();
    for entry in entries {
        match entry {
            Json::Str(statement) => clauses.push(Clause {
                statement: statement.clone(),
                predicate: None,
                hold: false,
                origin,
            }),
            Json::Obj(_) => {
                let statement = entry
                    .get("statement")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string();
                // `value && typeof value === 'object'`: an array reaches the evaluator and answers
                // malformed; a number or a string never was a predicate.
                let predicate = match entry.get("predicate") {
                    Some(json @ (Json::Obj(_) | Json::Arr(_))) => Some(json.clone()),
                    _ => None,
                };
                clauses.push(Clause {
                    statement,
                    predicate,
                    hold: entry.get("hold") == Some(&Json::Bool(true)),
                    origin,
                });
            }
            // Anything else is not a clause and is skipped rather than invented.
            _ => continue,
        }
    }
    clauses
}

/// The skill's clauses, then the site's. §54.2's narrowing-only rule, made structural.
///
/// > The operator may add preconditions; the skill may not remove them.
///
/// There is no argument by which the operator's list can shorten the result, because the operator's
/// list is only ever appended — a removal would need a mechanism, and not building one is the
/// enforcement.
pub fn resolve_clauses(published: Vec<Clause>, operator: Vec<Clause>) -> Vec<Clause> {
    let mut resolved = Vec::with_capacity(published.len() + operator.len());
    for mut clause in published {
        clause.origin = ClauseOrigin::Skill;
        resolved.push(clause);
    }
    for mut clause in operator {
        clause.origin = ClauseOrigin::Operator;
        resolved.push(clause);
    }
    resolved
}

fn judge_clause(clause: &Clause, index: usize, context: &Json) -> ClauseResult {
    let Some(predicate) = &clause.predicate else {
        return ClauseResult {
            index,
            statement: clause.statement.clone(),
            origin: clause.origin,
            hold: clause.hold,
            outcome: ClauseOutcome::Unenforceable,
            reason: None,
            path: None,
            op: None,
        };
    };
    let outcome = predicate::evaluate(Some(predicate), context);
    ClauseResult {
        index,
        statement: clause.statement.clone(),
        origin: clause.origin,
        hold: clause.hold,
        outcome: match outcome.outcome {
            Outcome::Satisfied => ClauseOutcome::Satisfied,
            Outcome::Unsatisfied => ClauseOutcome::Unsatisfied,
            Outcome::Unevaluable => ClauseOutcome::Unevaluable,
        },
        reason: outcome.reason,
        path: outcome.path,
        op: outcome.op,
    }
}

/// §54.2 — may this skill start, or continue?
///
/// [`Phase::Entry`] evaluates every clause. [`Phase::Hold`] evaluates only those the manifest
/// marked, because that is what runs every cycle:
///
/// > Preconditions are re-checked, not just entry-checked. A subset marked `hold` in the manifest
/// > is evaluated every cycle; battery dropping below reserve mid-skill should stop the skill, not
/// > be discovered at the end.
///
/// Every clause is evaluated even after one has failed, because an operator fixing four problems
/// one attempt at a time is the failure `admit_package` already refuses to cause. `refused` names
/// the first, which is the one to put on a screen.
pub fn evaluate_preconditions(
    clauses: &[Clause],
    context: &Json,
    phase: Phase,
) -> PreconditionReport {
    let mut results = Vec::new();
    for (index, clause) in clauses.iter().enumerate() {
        if phase == Phase::Hold && !clause.hold {
            continue;
        }
        results.push(judge_clause(clause, index, context));
    }

    let refused = results
        .iter()
        .find(|result| result.outcome != ClauseOutcome::Satisfied)
        .cloned();
    let unenforceable = results
        .iter()
        .filter(|result| result.outcome == ClauseOutcome::Unenforceable)
        .count();

    PreconditionReport {
        phase,
        permitted: refused.is_none(),
        refused,
        results,
        unenforceable,
    }
}

/// §54.3 and §54.4's terminal vocabulary, which is the Hub's `EpisodeOutcome` and not a second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillOutcome {
    /// Every criterion held.
    Success,
    /// A criterion was definitely false.
    Failure,
    /// Stopped by the OS — a timeout, a guard trip, a `hold` precondition lost.
    Aborted,
    /// Nothing was measured, or not everything could be.
    Unlabeled,
}

impl SkillOutcome {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            SkillOutcome::Success => "success",
            SkillOutcome::Failure => "failure",
            SkillOutcome::Aborted => "aborted",
            SkillOutcome::Unlabeled => "unlabeled",
        }
    }
}

/// Every terminal outcome, in the episode schema's order.
pub const SKILL_OUTCOMES: [SkillOutcome; 4] = [
    SkillOutcome::Success,
    SkillOutcome::Failure,
    SkillOutcome::Aborted,
    SkillOutcome::Unlabeled,
];

/// §54.3's answer.
#[derive(Debug, Clone)]
pub struct SuccessJudgement {
    /// One of `success`, `failure` or `unlabeled`. `aborted` comes from the lifecycle, not from here.
    pub outcome: SkillOutcome,
    /// The criterion that decided it: the first false one, or the first unreadable one.
    pub deciding: Option<ClauseResult>,
    /// Every criterion, in order.
    pub results: Vec<ClauseResult>,
}

/// §54.3 — did this skill succeed? Judged by the OS, over post-state.
///
/// > A skill that judges its own success is a skill that always succeeds — and on a platform where
/// > success rates feed a marketplace, self-reported outcomes are worth nothing.
///
/// Three answers, and the order they are decided in is the content:
///
/// 1. **Any criterion that is definitely false is a failure**, even if another could not be read.
///    Evidence of failure beats absence of evidence.
/// 2. **Otherwise, anything unread is `unlabeled`.** Many criteria genuinely are not
///    machine-evaluable — "the cup is upright" needs perception — and inventing a pass for those is
///    how a leaderboard becomes fiction.
/// 3. **Only when every criterion held is it a success.**
///
/// A skill that declares no criteria at all is `unlabeled`, not `success`. Nothing was measured,
/// and a skill that scores itself by declaring nothing is the same gaming §54.3 exists to stop.
pub fn judge_success(clauses: &[Clause], post_state: &Json) -> SuccessJudgement {
    let results: Vec<ClauseResult> = clauses
        .iter()
        .enumerate()
        .map(|(index, clause)| judge_clause(clause, index, post_state))
        .collect();

    if let Some(failed) = results
        .iter()
        .find(|result| result.outcome == ClauseOutcome::Unsatisfied)
    {
        return SuccessJudgement {
            outcome: SkillOutcome::Failure,
            deciding: Some(failed.clone()),
            results,
        };
    }

    if let Some(unread) = results
        .iter()
        .find(|result| result.outcome != ClauseOutcome::Satisfied)
    {
        return SuccessJudgement {
            outcome: SkillOutcome::Unlabeled,
            deciding: Some(unread.clone()),
            results,
        };
    }

    if results.is_empty() {
        return SuccessJudgement {
            outcome: SkillOutcome::Unlabeled,
            deciding: None,
            results,
        };
    }
    SuccessJudgement {
        outcome: SkillOutcome::Success,
        deciding: None,
        results,
    }
}

/// How far past its own estimate a skill may run before it is stopped.
///
/// §54.4 says "times a declared overrun factor" and does not say where it is declared, which makes
/// it an operator setting with a default rather than a manifest field — a publisher choosing how
/// long past their own estimate they may run is a publisher with no timeout.
pub const DEFAULT_OVERRUN_FACTOR: f64 = 2.0;

/// When a running skill is stopped and recorded `aborted`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Deadline {
    /// Milliseconds from the start of the skill, or `None` when there is nothing to enforce.
    pub deadline_ms: Option<f64>,
    /// Whether the OS can enforce a timeout on this skill at all.
    pub enforceable: bool,
    /// The factor actually used, which is the default when the one supplied was not usable.
    pub factor: f64,
}

/// §54.4 — the timeout the OS enforces.
///
/// A skill that declares no duration produces `enforceable: false` rather than a guessed ceiling.
/// **This function does not decide what the runtime does about that**, and the specification does
/// not either: refusing an unbounded skill and permitting one are both defensible, and inventing
/// the answer inside a pure function is how a policy nobody agreed to ends up load-bearing.
pub fn skill_deadline(expected_duration_seconds: Option<f64>, overrun_factor: f64) -> Deadline {
    let factor = if overrun_factor.is_finite() && overrun_factor >= 1.0 {
        overrun_factor
    } else {
        DEFAULT_OVERRUN_FACTOR
    };
    match expected_duration_seconds {
        Some(seconds) if seconds.is_finite() && seconds > 0.0 => Deadline {
            // Both operands are positive, so rounding half away from zero is JavaScript's
            // `Math.round` exactly. That is checked above rather than assumed, because the two
            // disagree on negative halves and a deadline is the wrong place to find that out.
            deadline_ms: Some(round_half_away(seconds * 1000.0 * factor)),
            enforceable: true,
            factor,
        },
        _ => Deadline {
            deadline_ms: None,
            enforceable: false,
            factor,
        },
    }
}

/// One declared skill-to-skill call.
#[derive(Debug, Clone)]
pub struct SkillCall {
    /// The caller.
    pub from: String,
    /// The callee.
    pub to: String,
}

/// Read declared calls, skipping anything that is not one.
///
/// A reader rather than a cast, and it exists so that both editions discard the same malformed
/// edges at the same boundary: a typed [`SkillCall`] cannot hold `{ "from": 5 }`, so if the
/// filtering lived inside [`validate_composition`] the two editions would filter in different
/// places and a corpus case for a malformed edge would be testing two different functions.
pub fn read_calls(input: Option<&Json>) -> Vec<SkillCall> {
    let Some(Json::Arr(entries)) = input else {
        return Vec::new();
    };
    let mut calls = Vec::new();
    for entry in entries {
        let (Some(from), Some(to)) = (
            entry.get("from").and_then(Json::as_str),
            entry.get("to").and_then(Json::as_str),
        ) else {
            continue;
        };
        calls.push(SkillCall {
            from: from.to_string(),
            to: to.to_string(),
        });
    }
    calls
}

/// §54.5's default. A skill four deep is a skill whose author can still describe what it does.
///
/// The root counts as depth 1, so a limit of 4 permits three levels of nesting beneath it.
pub const MAX_SKILL_DEPTH: usize = 4;

/// What is wrong with a call graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionFaultCode {
    /// A skill is already in its own call chain.
    Cycle,
    /// The chain is longer than the limit.
    TooDeep,
}

impl CompositionFaultCode {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            CompositionFaultCode::Cycle => "cycle",
            CompositionFaultCode::TooDeep => "too_deep",
        }
    }
}

/// Both fault codes.
pub const COMPOSITION_FAULTS: [CompositionFaultCode; 2] =
    [CompositionFaultCode::Cycle, CompositionFaultCode::TooDeep];

/// A refused call graph, with the chain that reached the fault.
#[derive(Debug, Clone)]
pub struct CompositionFault {
    /// What is wrong.
    pub code: CompositionFaultCode,
    /// The chain that reached it, root first. A cycle's repeated element is its last entry.
    pub path: Vec<String>,
    /// What to tell an operator.
    pub detail: String,
}

/// §54.5 — is this call graph installable?
///
/// > Depth-limited (default 4) and acyclic — a cycle is refused at install by walking the declared
/// > graph, not discovered at runtime.
///
/// Refusing at install is the whole point. A cycle discovered at runtime is discovered by a robot
/// that has already taken a lease and started moving.
///
/// The walk is depth-first in declaration order, so the chain a fault names is reproducible: an
/// edition that iterated its edges differently would refuse the same graphs and blame different
/// skills, which is the half an operator reads.
pub fn validate_composition(
    root: &str,
    calls: &[SkillCall],
    max_depth: usize,
) -> Option<CompositionFault> {
    let limit = if max_depth >= 1 {
        max_depth
    } else {
        MAX_SKILL_DEPTH
    };

    // Declaration order is preserved within each node's edge list, which is what makes a fault's
    // path the same in every edition.
    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for call in calls {
        edges
            .entry(call.from.as_str())
            .or_default()
            .push(call.to.as_str());
    }

    let mut fault = None;
    let mut path = Vec::new();
    path.push(root);
    walk(root, &mut path, &edges, limit, &mut fault);
    fault
}

fn walk<'a>(
    node: &str,
    path: &mut Vec<&'a str>,
    edges: &BTreeMap<&'a str, Vec<&'a str>>,
    limit: usize,
    fault: &mut Option<CompositionFault>,
) {
    if fault.is_some() {
        return;
    }
    if path.len() > limit {
        *fault = Some(CompositionFault {
            code: CompositionFaultCode::TooDeep,
            path: path.iter().map(ToString::to_string).collect(),
            detail: format!(
                "This call chain is {} skills deep; the limit is {limit}.",
                path.len()
            ),
        });
        return;
    }
    let Some(next_nodes) = edges.get(node) else {
        return;
    };
    for next in next_nodes {
        if fault.is_some() {
            return;
        }
        if path.contains(next) {
            let mut chain: Vec<String> = path.iter().map(ToString::to_string).collect();
            chain.push((*next).to_string());
            *fault = Some(CompositionFault {
                code: CompositionFaultCode::Cycle,
                path: chain,
                detail: format!("{next} is already in this call chain, so it would call itself."),
            });
            return;
        }
        path.push(next);
        walk(next, path, edges, limit, fault);
        path.pop();
    }
}

// ── Wire shapes ──────────────────────────────────────────────────────────────

fn clause_result_to_json(result: &ClauseResult) -> Json {
    let mut out = Json::obj();
    out.set("index", Json::Num(result.index as f64))
        .set("statement", Json::Str(result.statement.clone()))
        .set("origin", Json::Str(result.origin.as_str().to_string()))
        .set("hold", Json::Bool(result.hold))
        .set("outcome", Json::Str(result.outcome.as_str().to_string()))
        .set(
            "reason",
            match &result.reason {
                Some(reason) => Json::Str(reason.as_str().to_string()),
                None => Json::Null,
            },
        )
        .set(
            "path",
            match &result.path {
                Some(path) => Json::Str(path.clone()),
                None => Json::Null,
            },
        )
        .set(
            "op",
            match &result.op {
                Some(op) => Json::Str(op.clone()),
                None => Json::Null,
            },
        );
    out
}

/// The wire shape of a [`PreconditionReport`].
pub fn precondition_report_to_json(report: &PreconditionReport) -> Json {
    let mut out = Json::obj();
    out.set("phase", Json::Str(report.phase.as_str().to_string()))
        .set("permitted", Json::Bool(report.permitted))
        .set(
            "refused",
            match &report.refused {
                Some(result) => clause_result_to_json(result),
                None => Json::Null,
            },
        )
        .set(
            "results",
            Json::Arr(report.results.iter().map(clause_result_to_json).collect()),
        )
        .set("unenforceable", Json::Num(report.unenforceable as f64));
    out
}

/// The wire shape of a [`SuccessJudgement`].
pub fn success_to_json(judgement: &SuccessJudgement) -> Json {
    let mut out = Json::obj();
    out.set("outcome", Json::Str(judgement.outcome.as_str().to_string()))
        .set(
            "deciding",
            match &judgement.deciding {
                Some(result) => clause_result_to_json(result),
                None => Json::Null,
            },
        )
        .set(
            "results",
            Json::Arr(
                judgement
                    .results
                    .iter()
                    .map(clause_result_to_json)
                    .collect(),
            ),
        );
    out
}

/// The wire shape of a [`Deadline`].
pub fn deadline_to_json(deadline: &Deadline) -> Json {
    let mut out = Json::obj();
    out.set(
        "deadline_ms",
        match deadline.deadline_ms {
            Some(ms) => Json::Num(ms),
            None => Json::Null,
        },
    )
    .set("enforceable", Json::Bool(deadline.enforceable))
    .set("factor", Json::Num(deadline.factor));
    out
}

/// The wire shape of a [`CompositionFault`], `{"fault":null}` when the graph is usable.
pub fn composition_to_json(fault: &Option<CompositionFault>) -> Json {
    let mut out = Json::obj();
    match fault {
        None => {
            out.set("fault", Json::Null);
        }
        Some(fault) => {
            let mut body = Json::obj();
            body.set("code", Json::Str(fault.code.as_str().to_string()))
                .set(
                    "path",
                    Json::Arr(fault.path.iter().cloned().map(Json::Str).collect()),
                )
                .set("detail", Json::Str(fault.detail.clone()));
            out.set("fault", body);
        }
    }
    out
}

// ── §54.4 The lifecycle ──────────────────────────────────────────────────────
//
//     idle → preconditions → acquiring → running → verifying → succeeded | failed | aborted
//               ↓ refused        ↓ denied     ↓ timeout / guard trip / precondition lost
//
// The Rust edition of the same half of `reference/typescript/skill.ts`. Read that file for the
// design; the notes here are the port's.
//
// Two things this edition had to decide, and both are the same decision the rest of the crate keeps
// making: **the defaults live in the reader, not in the caller.** [`Progress`] is a struct of plain
// fields here and an object of optional ones in TypeScript, so "not gathered yet" has to be
// spelled — and every field spells it as the value that does *not* advance the machine.
//
// `entry_permitted` is therefore an `Option<bool>` rather than a `bool`, which looks like
// fastidiousness and is not: a runtime that has not evaluated §54.2 yet must not be able to start a
// skill by failing to ask. `Progress::default()` is an invocation nobody has made, and it stays in
// `idle`.

/// §54.4's stages.
///
/// `terminal` is a phase rather than three, because §54.4's own table makes it one — *"Leases
/// released, outcome recorded, episode emitted"* — and the outcome that distinguishes its three
/// diagram arrows is [`SkillOutcome`], which is the Hub's `EpisodeOutcome` and not a second
/// vocabulary. Reproduced into `corpus/vocabulary/closed-sets.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillPhase {
    /// Nothing has asked for this skill.
    Idle,
    /// §54.2's predicates are being evaluated.
    Preconditions,
    /// §25.2's leases are being taken.
    Acquiring,
    /// The skill's policy runs, under the transport plan and the resolved clamps.
    Running,
    /// §54.3's criteria are being evaluated over post-state.
    Verifying,
    /// Leases released, outcome recorded, episode emitted.
    Terminal,
}

impl SkillPhase {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            SkillPhase::Idle => "idle",
            SkillPhase::Preconditions => "preconditions",
            SkillPhase::Acquiring => "acquiring",
            SkillPhase::Running => "running",
            SkillPhase::Verifying => "verifying",
            SkillPhase::Terminal => "terminal",
        }
    }

    /// Read a phase back, for a report that crossed a wire. Exact, never case-insensitive.
    pub fn parse(text: &str) -> Option<Self> {
        SKILL_PHASES.iter().copied().find(|p| p.as_str() == text)
    }
}

/// Every phase, in the order §54.4's diagram runs them.
pub const SKILL_PHASES: [SkillPhase; 6] = [
    SkillPhase::Idle,
    SkillPhase::Preconditions,
    SkillPhase::Acquiring,
    SkillPhase::Running,
    SkillPhase::Verifying,
    SkillPhase::Terminal,
];

/// §25.2's arbitration, as the one fact the lifecycle needs from it.
///
/// Deliberately **not** a copy of `skynetd::lease::PreemptionOutcome`. That enumeration answers a
/// richer question — acquired, pre-empted, already held, denied and why — and this one answers the
/// only part the lifecycle acts on. Two definitions of a lease outcome would be §31.2's failure in
/// miniature; a projection of one onto the other is a mapping, and it lives at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LeaseState {
    /// Not taken yet. The default, because nobody has said otherwise.
    #[default]
    Pending,
    /// Every token in `required_capabilities` is leased to this invocation.
    Held,
    /// Something is held by somebody else, and §25.2 refuses rather than interleaves.
    Denied,
}

impl LeaseState {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            LeaseState::Pending => "pending",
            LeaseState::Held => "held",
            LeaseState::Denied => "denied",
        }
    }
}

/// Every lease state.
pub const LEASE_STATES: [LeaseState; 3] =
    [LeaseState::Pending, LeaseState::Held, LeaseState::Denied];

/// Why the lifecycle is where it is.
///
/// One per rule, as in §27's machine and for the same reason: four reasons end in `terminal` with
/// `aborted`, and only the reason tells an operator whether to call the publisher, look at the
/// cell, find the package holding the arm, or read the black box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleReason {
    /// Nothing has asked for this skill.
    NotInvoked,
    /// An invocation arrived.
    Invoked,
    /// §54.2 has not been evaluated, which is not a pass.
    PreconditionsPending,
    /// A precondition was false. §54.6's `skill.preconditions.enforced`.
    PreconditionsRefused,
    /// Every evaluated precondition held.
    PreconditionsMet,
    /// §54.5: a nested call runs under the root's leases and does not acquire.
    LeasesHeldByRoot,
    /// The leases have not been taken.
    LeasePending,
    /// §25.2 refused one. Denied if held.
    LeaseDenied,
    /// §36.1 withholds new task leases: the robot has not the energy to be given more work.
    ///
    /// Its own reason rather than a second [`LifecycleReason::LeaseDenied`], for the reason this
    /// enumeration has eighteen entries: only the reason tells an operator whether to call the
    /// publisher, find the package holding the arm, or **put the robot on a charger**. Three
    /// errands, and a shared word sends two of them to the wrong one.
    PowerWithheld,
    /// Every required capability is leased.
    LeasesAcquired,
    /// §54.4's timeout. `skill.timeout.enforced`.
    Timeout,
    /// A `hold` precondition stopped being true. `skill.preconditions.rechecked`.
    HoldLost,
    /// A guard tripped, which includes a guard asking for a hold.
    GuardTripped,
    /// §27 says the robot is not in a state a policy commands in.
    RobotStopped,
    /// Still running.
    Running,
    /// The skill returned and has not been judged.
    Verifying,
    /// §54.3 has not judged, and §54.3 will not let the skill judge itself.
    VerdictPending,
    /// §54.3 judged it.
    Judged,
    /// Already over.
    Terminal,
}

impl LifecycleReason {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            LifecycleReason::NotInvoked => "not_invoked",
            LifecycleReason::Invoked => "invoked",
            LifecycleReason::PreconditionsPending => "preconditions_pending",
            LifecycleReason::PreconditionsRefused => "preconditions_refused",
            LifecycleReason::PreconditionsMet => "preconditions_met",
            LifecycleReason::LeasesHeldByRoot => "leases_held_by_root",
            LifecycleReason::LeasePending => "lease_pending",
            LifecycleReason::LeaseDenied => "lease_denied",
            LifecycleReason::PowerWithheld => "power_withheld",
            LifecycleReason::LeasesAcquired => "leases_acquired",
            LifecycleReason::Timeout => "timeout",
            LifecycleReason::HoldLost => "hold_lost",
            LifecycleReason::GuardTripped => "guard_tripped",
            LifecycleReason::RobotStopped => "robot_stopped",
            LifecycleReason::Running => "running",
            LifecycleReason::Verifying => "verifying",
            LifecycleReason::VerdictPending => "verdict_pending",
            LifecycleReason::Judged => "judged",
            LifecycleReason::Terminal => "terminal",
        }
    }
}

/// Every reason, in rule order.
pub const LIFECYCLE_REASONS: [LifecycleReason; 19] = [
    LifecycleReason::NotInvoked,
    LifecycleReason::Invoked,
    LifecycleReason::PreconditionsPending,
    LifecycleReason::PreconditionsRefused,
    LifecycleReason::PreconditionsMet,
    LifecycleReason::LeasesHeldByRoot,
    LifecycleReason::LeasePending,
    LifecycleReason::LeaseDenied,
    LifecycleReason::PowerWithheld,
    LifecycleReason::LeasesAcquired,
    LifecycleReason::Timeout,
    LifecycleReason::HoldLost,
    LifecycleReason::GuardTripped,
    LifecycleReason::RobotStopped,
    LifecycleReason::Running,
    LifecycleReason::Verifying,
    LifecycleReason::VerdictPending,
    LifecycleReason::Judged,
    LifecycleReason::Terminal,
];

/// Everything one invocation has established so far.
///
/// Every field is evidence somebody gathered, and every default is "not gathered".
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// Somebody called `execute({ skill, arguments, timeout_s })` (§54.1).
    pub invoked: bool,
    /// Whether this is the root invocation (§54.5). A nested call does not acquire.
    pub root: bool,
    /// §54.2's entry evaluation. `None` until it has been run — which is not a pass.
    pub entry_permitted: Option<bool>,
    /// The clause a refusal names, from [`PreconditionReport::refused`].
    pub refused_clause: String,
    /// §25.2.
    pub leases: LeaseState,
    /// The token that was refused, and who holds it.
    pub lease_token: String,
    /// Who holds it.
    pub lease_holder: String,
    /// §36.1's `low` row, when it is in force: the sentence, or `None`.
    ///
    /// One field rather than a boolean and a reason beside it, for the reason
    /// [`crate::opstate::Conditions::stop`] is an `Option<StopCause>`: two fields that must agree
    /// are two fields that eventually do not.
    ///
    /// The sentence is composed by the runtime rather than here, because the runtime is where §36's
    /// assessment is in hand — a robot may withhold leases for being nearly flat *or* for having a
    /// gauge nothing can read, and those are different sentences for the same rule.
    ///
    /// **`None` is the default and it is the permissive one**, which is unlike almost everything
    /// else in this crate. It is not a guess: §36's answer for a body nobody has observed is
    /// `unmetered`, which grants leases — so `None` is what the power contract itself computes for
    /// an unobserved robot, not an assumption made in its absence. A default of "withholding" would
    /// refuse every skill on every robot with no power subsystem wired up, silently.
    pub power_withheld: Option<String>,
    /// Milliseconds since the skill entered `running`. `None` while it has not.
    pub elapsed_ms: Option<f64>,
    /// §54.4's deadline, from [`skill_deadline`].
    pub deadline: Deadline,
    /// A `hold` precondition falsified mid-run (§54.2's second rule).
    pub hold_lost: bool,
    /// A guard tripped, which includes a guard asking for a hold.
    pub guard_tripped: bool,
    /// Where §27's machine says the robot is.
    pub robot: OperatingState,
    /// The skill's own code returned.
    pub finished: bool,
    /// §54.3's judgement. `None` until it has been made — which is not a success.
    pub outcome: Option<SkillOutcome>,
}

impl Default for Progress {
    /// An invocation nobody has made, on a robot nobody has observed.
    ///
    /// `root` defaults to **true**, which is the one field whose safe default is the permissive
    /// one — and it is worth saying why. A root invocation *acquires*; a nested one is trusted to
    /// be running under leases somebody else took. Defaulting to nested would let a caller that
    /// forgot the field skip §25.2's arbitration entirely, which is two policies driving one elbow.
    /// Defaulting to root costs a redundant acquisition that §25.2 answers `AlreadyHeld`.
    fn default() -> Self {
        Self {
            invoked: false,
            root: true,
            entry_permitted: None,
            refused_clause: String::new(),
            leases: LeaseState::Pending,
            lease_token: String::new(),
            lease_holder: String::new(),
            power_withheld: None,
            elapsed_ms: None,
            deadline: Deadline {
                deadline_ms: None,
                enforceable: false,
                factor: DEFAULT_OVERRUN_FACTOR,
            },
            hold_lost: false,
            guard_tripped: false,
            robot: OperatingState::Post,
            finished: false,
            outcome: None,
        }
    }
}

/// One evaluation of the lifecycle.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillTransition {
    /// Where it was.
    pub from: SkillPhase,
    /// Where it is.
    pub to: SkillPhase,
    /// Why.
    pub reason: LifecycleReason,
    /// The episode outcome, present only on entering `terminal`.
    pub outcome: Option<SkillOutcome>,
    /// What a console shows beside the reason: the clause, the token, the state.
    pub detail: String,
    /// §54.4's terminal row: leases released. True only on the transition **into** `terminal`.
    pub release_leases: bool,
    /// Whether the phase changed.
    pub changed: bool,
}

/// Whether a robot in this state has stopped a skill that was running.
///
/// `hold` is deliberately absent, and it is the one exclusion worth writing down: it is where §27's
/// machine sits between a lease being granted and a policy actually commanding, so aborting on it
/// would kill every skill in the moment it started. A guard asking for a hold reaches the lifecycle
/// as [`Progress::guard_tripped`], which is the input that means it.
///
/// `degraded` is absent for the opposite reason: §27's `DEGRADED` row runs, and a skill refusing to
/// proceed on a robot with one unavailable guard would refuse on most real robots (§45.5).
fn robot_stopped_commanding(state: OperatingState) -> bool {
    matches!(
        state,
        OperatingState::Stopping
            | OperatingState::Safe
            | OperatingState::Post
            | OperatingState::Unhomed
    )
}

/// Whether the deadline has passed. An unenforceable deadline never has.
fn overran(progress: &Progress) -> bool {
    let (Some(deadline), true) = (progress.deadline.deadline_ms, progress.deadline.enforceable)
    else {
        return false;
    };
    match progress.elapsed_ms {
        Some(elapsed) if elapsed.is_finite() => elapsed >= deadline,
        _ => false,
    }
}

/// Why a running skill is over, or `None` if it is not.
///
/// **Causes before effects.** A timeout makes the OS stop the skill under a controlled stop, and a
/// guard trip stops the robot; both therefore produce a robot that is no longer commanding.
/// Reporting the robot state first would hide the cause behind its own consequence, so the specific
/// reasons are tried first and `robot_stopped` is the fallback that catches whatever this machine
/// could not name.
fn abort_reason(progress: &Progress) -> Option<(LifecycleReason, String)> {
    if overran(progress) {
        return Some((
            LifecycleReason::Timeout,
            format!(
                "past {} ms, which is the declared estimate times {}",
                progress.deadline.deadline_ms.unwrap_or(0.0),
                progress.deadline.factor
            ),
        ));
    }
    if progress.hold_lost {
        return Some((
            LifecycleReason::HoldLost,
            "a precondition marked `hold` stopped being true, and §54.2 re-checks those every \
             cycle rather than only at entry"
                .to_string(),
        ));
    }
    if progress.guard_tripped {
        return Some((
            LifecycleReason::GuardTripped,
            "a guard tripped, and a skill is not an exemption".to_string(),
        ));
    }
    if robot_stopped_commanding(progress.robot) {
        return Some((
            LifecycleReason::RobotStopped,
            format!(
                "§27 says the robot is `{}`, which is not a state a policy commands in",
                progress.robot.as_str()
            ),
        ));
    }
    None
}

/// What a denied acquisition tells the operator, which is two different errands.
///
/// §25.2 keeps a *grant* and a *lease* apart: holding a grant makes a package eligible to take an
/// exclusive lease, and two packages may hold the same grant while only one holds the lease. So a
/// refusal has two shapes and they send an operator to opposite places —
///
/// * **a holder is named:** somebody else is using it. Stop that package, or wait.
/// * **no holder:** nobody has it, and this package was never granted it. Grant it, or take it out
///   of `required_capabilities`.
///
/// `Acquisition::into_progress` has always reported the second as `holder: None`, and this function
/// is where that survives into the sentence. The version it replaced said "is held by another
/// package" for both, which is a refusal that sends an operator hunting for a package that does not
/// exist — §64's failure exactly, and one no corpus case had caught because a case written from the
/// document imagines the contended arm rather than the unconfigured one.
fn lease_denied_detail(progress: &Progress) -> String {
    if progress.lease_token.is_empty() {
        return "a required capability could not be leased, and nothing named which".to_string();
    }
    if progress.lease_holder.is_empty() {
        return format!(
            "`{}` was never granted to this package, so §25.2 will not let it take the lease — \
             nobody is holding it. Grant it, or take it out of `required_capabilities`.",
            progress.lease_token
        );
    }
    format!(
        "`{}` is held by {}",
        progress.lease_token, progress.lease_holder
    )
}

/// §54.4's lifecycle, advanced by one evaluation.
///
/// Pure, clockless and total: `elapsed_ms` is supplied for the same reason the kernel takes `now_ms`
/// as a parameter (§35.2), which is what lets a timeout be tested without waiting for one.
pub fn advance(phase: SkillPhase, progress: &Progress) -> SkillTransition {
    let done = |to: SkillPhase,
                reason: LifecycleReason,
                outcome: Option<SkillOutcome>,
                detail: String| SkillTransition {
        from: phase,
        to,
        reason,
        outcome,
        detail,
        // Only on the way in. A skill already terminal releases nothing, because it has nothing.
        release_leases: to == SkillPhase::Terminal && phase != SkillPhase::Terminal,
        changed: to != phase,
    };

    match phase {
        SkillPhase::Idle => {
            if progress.invoked {
                done(
                    SkillPhase::Preconditions,
                    LifecycleReason::Invoked,
                    None,
                    "an invocation arrived".to_string(),
                )
            } else {
                done(
                    SkillPhase::Idle,
                    LifecycleReason::NotInvoked,
                    None,
                    "nothing has asked for this skill".to_string(),
                )
            }
        }

        SkillPhase::Preconditions => match progress.entry_permitted {
            // Absent is not permitted. A runtime that had not evaluated §54.2 yet must not be able
            // to start a skill by failing to ask, which is the shape of every bug this repository
            // is arranged around.
            None => done(
                SkillPhase::Preconditions,
                LifecycleReason::PreconditionsPending,
                None,
                "the entry predicates have not been evaluated, and an unevaluated precondition is \
                 not a satisfied one"
                    .to_string(),
            ),
            Some(false) => done(
                SkillPhase::Terminal,
                LifecycleReason::PreconditionsRefused,
                Some(SkillOutcome::Aborted),
                if progress.refused_clause.is_empty() {
                    "refused by a precondition".to_string()
                } else {
                    format!("refused by: {}", progress.refused_clause)
                },
            ),
            // §54.5. A nested invocation does not acquire; the root holds the leases for the tree.
            Some(true) if !progress.root => done(
                SkillPhase::Running,
                LifecycleReason::LeasesHeldByRoot,
                None,
                "a nested call runs under the root invocation's leases (§54.5)".to_string(),
            ),
            Some(true) => done(
                SkillPhase::Acquiring,
                LifecycleReason::PreconditionsMet,
                None,
                "every evaluated precondition held".to_string(),
            ),
        },

        // §36.1's `low` row, before the table is consulted. Two reasons for the order: a refusal
        // for want of energy is a fact about the *robot* and a refusal for a held token is a fact
        // about another package, so when both are true the robot-wide one is what an operator needs
        // first — and the runtime does not ask the table at all when this is set, so `leases` would
        // read `Pending` and pin the invocation in `acquiring` until its deadline.
        //
        // Terminal rather than a wait. "No new task leases granted" is a refusal, and an invocation
        // parked in `acquiring` hoping for a charger would burn §54.4's timeout and then abort with
        // the wrong reason on it.
        SkillPhase::Acquiring if progress.power_withheld.is_some() => done(
            SkillPhase::Terminal,
            LifecycleReason::PowerWithheld,
            Some(SkillOutcome::Aborted),
            progress
                .power_withheld
                .clone()
                .unwrap_or_else(|| "§36.1 withholds new task leases".to_string()),
        ),

        SkillPhase::Acquiring => match progress.leases {
            LeaseState::Denied => done(
                SkillPhase::Terminal,
                LifecycleReason::LeaseDenied,
                Some(SkillOutcome::Aborted),
                lease_denied_detail(progress),
            ),
            LeaseState::Pending => done(
                SkillPhase::Acquiring,
                LifecycleReason::LeasePending,
                None,
                "the leases for `required_capabilities` have not been taken".to_string(),
            ),
            LeaseState::Held => done(
                SkillPhase::Running,
                LifecycleReason::LeasesAcquired,
                None,
                "every required capability is leased".to_string(),
            ),
        },

        SkillPhase::Running => match abort_reason(progress) {
            Some((reason, detail)) => done(
                SkillPhase::Terminal,
                reason,
                Some(SkillOutcome::Aborted),
                detail,
            ),
            None if progress.finished => done(
                SkillPhase::Verifying,
                LifecycleReason::Verifying,
                None,
                "the skill returned; §54.3 has not judged it yet".to_string(),
            ),
            None => done(
                SkillPhase::Running,
                LifecycleReason::Running,
                None,
                "the skill is running".to_string(),
            ),
        },

        // A skill can still be stopped while its post-state is being judged, and the abort outranks
        // the verdict: a skill whose robot fell over during verification did not succeed at
        // anything, whatever a criterion evaluated to on the way down.
        SkillPhase::Verifying => match abort_reason(progress) {
            Some((reason, detail)) => done(
                SkillPhase::Terminal,
                reason,
                Some(SkillOutcome::Aborted),
                detail,
            ),
            None => match progress.outcome {
                None => done(
                    SkillPhase::Verifying,
                    LifecycleReason::VerdictPending,
                    None,
                    "the success criteria have not been judged, and §54.3 will not let the skill \
                     judge itself"
                        .to_string(),
                ),
                Some(outcome) => done(
                    SkillPhase::Terminal,
                    LifecycleReason::Judged,
                    Some(outcome),
                    format!("§54.3 judged this {}", outcome.as_str()),
                ),
            },
        },

        SkillPhase::Terminal => done(
            SkillPhase::Terminal,
            LifecycleReason::Terminal,
            progress.outcome,
            "this invocation is over".to_string(),
        ),
    }
}

/// §54.5's third limit: *"The root's timeout bounds the whole tree. A child cannot extend its
/// parent's budget."*
///
/// The child's own deadline, or what is left of the root's, whichever is sooner. Two properties
/// fall out and both are the point:
///
/// * a child that declares **no** duration is still bounded, by its parent, so an unbounded skill
///   nested under a bounded one does not make the tree unbounded;
/// * a root with nothing left bounds its child at zero rather than at a negative number, so the
///   child is stopped rather than handed a deadline already in the past that some other arithmetic
///   could read as "no deadline".
pub fn bounded_deadline(child: &Deadline, root_remaining_ms: Option<f64>) -> Deadline {
    let Some(remaining) = root_remaining_ms.filter(|value| value.is_finite()) else {
        return *child;
    };
    let remaining = if remaining > 0.0 { remaining } else { 0.0 };
    match (child.enforceable, child.deadline_ms) {
        (true, Some(own)) => Deadline {
            deadline_ms: Some(if own < remaining { own } else { remaining }),
            enforceable: true,
            factor: child.factor,
        },
        _ => Deadline {
            deadline_ms: Some(remaining),
            enforceable: true,
            factor: child.factor,
        },
    }
}

/// A transition, for the black box, `GET /skills` and the console.
pub fn transition_to_json(transition: &SkillTransition) -> Json {
    let mut out = Json::obj();
    out.set("from", Json::Str(transition.from.as_str().to_string()));
    out.set("to", Json::Str(transition.to.as_str().to_string()));
    out.set("reason", Json::Str(transition.reason.as_str().to_string()));
    out.set(
        "outcome",
        match transition.outcome {
            Some(outcome) => Json::Str(outcome.as_str().to_string()),
            // Nullable rather than absent: "this transition is not terminal" is a fact, and a
            // consumer that had to distinguish a missing key from a null would get it wrong once.
            None => Json::Null,
        },
    );
    out.set("detail", Json::Str(transition.detail.clone()));
    out.set("release_leases", Json::Bool(transition.release_leases));
    out.set("changed", Json::Bool(transition.changed));
    out
}

/// Read progress out of a document, for the corpus and for a report that crossed a wire.
///
/// Every unreadable field falls back to its default, which is the value that does not advance the
/// machine. A caller that sent `"entry_permitted": "yes"` gets a skill that has not been evaluated.
pub fn progress_from_json(input: &Json) -> Progress {
    let flag = |key: &str| matches!(input.get(key), Some(Json::Bool(true)));
    let word = |key: &str| {
        input
            .get(key)
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string()
    };

    let deadline = match input.get("deadline") {
        Some(document) => Deadline {
            deadline_ms: document.get("deadline_ms").and_then(Json::as_f64),
            enforceable: matches!(document.get("enforceable"), Some(Json::Bool(true))),
            factor: document
                .get("factor")
                .and_then(Json::as_f64)
                .unwrap_or(DEFAULT_OVERRUN_FACTOR),
        },
        None => Progress::default().deadline,
    };

    Progress {
        invoked: flag("invoked"),
        // Absent is root, matching `Progress::default` — see the note there.
        root: !matches!(input.get("root"), Some(Json::Bool(false))),
        entry_permitted: match input.get("entry_permitted") {
            Some(Json::Bool(value)) => Some(*value),
            _ => None,
        },
        refused_clause: word("refused_clause"),
        leases: match input.get("leases").and_then(Json::as_str) {
            Some("held") => LeaseState::Held,
            Some("denied") => LeaseState::Denied,
            _ => LeaseState::Pending,
        },
        lease_token: word("lease_token"),
        lease_holder: word("lease_holder"),
        // An empty string is not a refusal. A corpus case that wrote `""` here would otherwise
        // abort a skill with a blank sentence on it, which is the one shape a refusal must never
        // have (§64).
        power_withheld: input
            .get("power_withheld")
            .and_then(Json::as_str)
            .map(js_trim)
            .filter(|sentence| !sentence.is_empty())
            .map(str::to_string),
        elapsed_ms: input.get("elapsed_ms").and_then(Json::as_f64),
        deadline,
        hold_lost: flag("hold_lost"),
        guard_tripped: flag("guard_tripped"),
        robot: input
            .get("robot")
            .and_then(Json::as_str)
            .and_then(OperatingState::parse)
            .unwrap_or(OperatingState::Post),
        finished: flag("finished"),
        outcome: input
            .get("outcome")
            .and_then(Json::as_str)
            .and_then(|word| {
                SKILL_OUTCOMES
                    .iter()
                    .copied()
                    .find(|outcome| outcome.as_str() == word)
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Json {
        Json::parse(
            r#"{"robot":{"state":{"battery_percent":62,"gripper":"open"},
                        "calibration":{"kinematic":{"valid":true}}},
                "env":{"door_closed":true}}"#,
        )
        .expect("the fixture parses")
    }

    fn clauses(source: &str, origin: ClauseOrigin) -> Vec<Clause> {
        let json = Json::parse(source).expect("the clauses parse");
        read_clauses(Some(&json), origin)
    }

    /// The migration §54.2 forces, stated as a test rather than as a paragraph: prose is not
    /// enforceable, and unenforceable is not permitted.
    #[test]
    fn prose_preconditions_refuse() {
        let published = clauses(
            r#"["The gripper is empty","The cell door is closed"]"#,
            ClauseOrigin::Skill,
        );
        let report = evaluate_preconditions(&published, &cell(), Phase::Entry);
        assert!(!report.permitted);
        assert_eq!(report.unenforceable, 2);
        assert_eq!(
            report.refused.as_ref().map(|r| r.outcome),
            Some(ClauseOutcome::Unenforceable)
        );
        // The sentence survives, because it is what an operator reads.
        assert_eq!(
            report.refused.as_ref().map(|r| r.statement.as_str()),
            Some("The gripper is empty")
        );
    }

    #[test]
    fn a_declared_predicate_is_evaluated() {
        let published = clauses(
            r#"[{"statement":"Gripper open",
                 "predicate":{"op":"eq","path":"robot.state.gripper","value":"open"}}]"#,
            ClauseOrigin::Skill,
        );
        assert!(evaluate_preconditions(&published, &cell(), Phase::Entry).permitted);
    }

    /// §54.2's first rule. The two refusals are different facts and the report keeps them apart.
    #[test]
    fn a_false_precondition_and_an_unreadable_one_are_different_refusals() {
        let published = clauses(
            r#"[{"statement":"Door open",
                 "predicate":{"op":"eq","path":"env.door_closed","value":false}},
                {"statement":"Payload under 5 kg",
                 "predicate":{"op":"lte","path":"robot.state.payload_kg","value":5}}]"#,
            ClauseOrigin::Skill,
        );
        let report = evaluate_preconditions(&published, &cell(), Phase::Entry);
        assert_eq!(report.results[0].outcome, ClauseOutcome::Unsatisfied);
        assert_eq!(report.results[0].path, None);
        assert_eq!(report.results[1].outcome, ClauseOutcome::Unevaluable);
        assert_eq!(
            report.results[1].path.as_deref(),
            Some("robot.state.payload_kg")
        );
        assert_eq!(
            report.results[1].reason,
            Some(UnevaluableReason::UnresolvedPath)
        );
    }

    /// §54.2's second rule: a subset is re-checked every cycle, and only that subset.
    #[test]
    fn the_hold_subset_is_what_runs_every_cycle() {
        let published = clauses(
            r#"[{"statement":"Gripper open",
                 "predicate":{"op":"eq","path":"robot.state.gripper","value":"open"}},
                {"statement":"Battery above reserve","hold":true,
                 "predicate":{"op":"gte","path":"robot.state.battery_percent","value":20}}]"#,
            ClauseOrigin::Skill,
        );
        assert_eq!(
            evaluate_preconditions(&published, &cell(), Phase::Entry)
                .results
                .len(),
            2
        );
        let held = evaluate_preconditions(&published, &cell(), Phase::Hold);
        assert_eq!(held.results.len(), 1);
        assert!(held.permitted);
        // The index is the position in the *resolved* list, so a cycle refusal points at the same
        // clause the entry report did.
        assert_eq!(held.results[0].index, 1);

        // And the moment §54.2 describes: the battery falls through reserve mid-skill.
        let drained = Json::parse(
            r#"{"robot":{"state":{"battery_percent":12,"gripper":"closed"}},"env":{}}"#,
        )
        .unwrap();
        assert!(!evaluate_preconditions(&published, &drained, Phase::Hold).permitted);
    }

    /// §54.2's third rule, and the reason it needs no enforcement code.
    #[test]
    fn the_operator_may_add_and_cannot_remove() {
        let published = clauses(
            r#"[{"statement":"Gripper open",
                 "predicate":{"op":"eq","path":"robot.state.gripper","value":"open"}}]"#,
            ClauseOrigin::Skill,
        );
        let operator = clauses(
            r#"[{"statement":"Fixture B is loaded",
                 "predicate":{"op":"eq","path":"env.fixture","value":"jig-b"}}]"#,
            ClauseOrigin::Operator,
        );
        let resolved = resolve_clauses(published.clone(), operator);
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].origin, ClauseOrigin::Skill);
        assert_eq!(resolved[1].origin, ClauseOrigin::Operator);
        // The site's clause refuses a skill whose own preconditions all hold.
        assert!(!evaluate_preconditions(&resolved, &cell(), Phase::Entry).permitted);
        // And there is no operator list that shortens the result.
        assert_eq!(resolve_clauses(published, Vec::new()).len(), 1);
    }

    /// A `predicate` that is not an object is not a broken predicate — it is an absent one.
    #[test]
    fn what_counts_as_a_predicate_at_all() {
        let unenforceable = clauses(r#"[{"statement":"x","predicate":5}]"#, ClauseOrigin::Skill);
        assert_eq!(
            evaluate_preconditions(&unenforceable, &cell(), Phase::Entry).results[0].outcome,
            ClauseOutcome::Unenforceable
        );
        // An array *is* an object to JavaScript, so it reaches the evaluator and is malformed.
        let malformed = clauses(
            r#"[{"statement":"x","predicate":["op"]}]"#,
            ClauseOrigin::Skill,
        );
        let result = &evaluate_preconditions(&malformed, &cell(), Phase::Entry).results[0];
        assert_eq!(result.outcome, ClauseOutcome::Unevaluable);
        assert_eq!(result.reason, Some(UnevaluableReason::Malformed));
        // Neither is a bare number a clause.
        assert_eq!(
            clauses(r#"[null,42,"prose"]"#, ClauseOrigin::Skill).len(),
            1
        );
        assert!(read_clauses(None, ClauseOrigin::Skill).is_empty());
    }

    /// §54.3's ordering, which is the substance of the judgement.
    #[test]
    fn evidence_of_failure_beats_absence_of_evidence() {
        let after = Json::parse(r#"{"robot":{"state":{"gripper":"closed"}}}"#).unwrap();
        // The unreadable criterion is first, and the answer is still `failure`.
        let criteria = clauses(
            r#"[{"statement":"The cup is upright",
                 "predicate":{"op":"eq","path":"robot.perception.upright","value":true}},
                {"statement":"The gripper is open",
                 "predicate":{"op":"eq","path":"robot.state.gripper","value":"open"}}]"#,
            ClauseOrigin::Skill,
        );
        assert_eq!(
            judge_success(&criteria, &after).outcome,
            SkillOutcome::Failure
        );
    }

    #[test]
    fn honest_ignorance_is_unlabeled() {
        let after = Json::parse(r#"{"robot":{"state":{"gripper":"closed"}}}"#).unwrap();
        let unreadable = clauses(
            r#"[{"statement":"The gripper is closed",
                 "predicate":{"op":"eq","path":"robot.state.gripper","value":"closed"}},
                {"statement":"The cup is upright",
                 "predicate":{"op":"eq","path":"robot.perception.upright","value":true}}]"#,
            ClauseOrigin::Skill,
        );
        assert_eq!(
            judge_success(&unreadable, &after).outcome,
            SkillOutcome::Unlabeled
        );
        // Prose is unlabeled for the same reason: nothing was measured.
        let prose = clauses(r#"["The cup is upright"]"#, ClauseOrigin::Skill);
        assert_eq!(
            judge_success(&prose, &after).outcome,
            SkillOutcome::Unlabeled
        );
        // And so is declaring nothing, which is the gaming §54.3 exists to stop.
        assert_eq!(judge_success(&[], &after).outcome, SkillOutcome::Unlabeled);
    }

    #[test]
    fn a_skill_that_met_every_criterion_succeeded() {
        let after = Json::parse(r#"{"robot":{"state":{"gripper":"closed"}}}"#).unwrap();
        let criteria = clauses(
            r#"[{"statement":"The gripper is closed",
                 "predicate":{"op":"eq","path":"robot.state.gripper","value":"closed"}}]"#,
            ClauseOrigin::Skill,
        );
        let judgement = judge_success(&criteria, &after);
        assert_eq!(judgement.outcome, SkillOutcome::Success);
        assert!(judgement.deciding.is_none());
    }

    #[test]
    fn a_deadline_is_the_estimate_times_the_factor() {
        assert_eq!(
            skill_deadline(Some(30.0), DEFAULT_OVERRUN_FACTOR).deadline_ms,
            Some(60_000.0)
        );
        assert_eq!(skill_deadline(Some(30.0), 1.5).deadline_ms, Some(45_000.0));
        // Half rounds up, as JavaScript's `Math.round` does on a positive value.
        assert_eq!(skill_deadline(Some(0.0025), 1.0).deadline_ms, Some(3.0));
    }

    /// The values JSON cannot hold, which is why they are here and not in the corpus.
    #[test]
    fn a_factor_that_is_not_a_factor_falls_back() {
        for factor in [0.5, 0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let deadline = skill_deadline(Some(30.0), factor);
            assert_eq!(deadline.factor, DEFAULT_OVERRUN_FACTOR, "{factor}");
            assert_eq!(deadline.deadline_ms, Some(60_000.0), "{factor}");
        }
        for seconds in [
            None,
            Some(0.0),
            Some(-5.0),
            Some(f64::NAN),
            Some(f64::INFINITY),
        ] {
            let deadline = skill_deadline(seconds, DEFAULT_OVERRUN_FACTOR);
            assert!(!deadline.enforceable, "{seconds:?}");
            assert_eq!(deadline.deadline_ms, None, "{seconds:?}");
        }
    }

    fn calls(source: &str) -> Vec<SkillCall> {
        let json = Json::parse(source).expect("the calls parse");
        read_calls(Some(&json))
    }

    #[test]
    fn a_chain_within_the_limit_installs() {
        let graph = calls(
            r#"[{"from":"pick","to":"approach"},
                {"from":"approach","to":"align"},
                {"from":"align","to":"servo"}]"#,
        );
        assert!(validate_composition("pick", &graph, MAX_SKILL_DEPTH).is_none());
    }

    #[test]
    fn one_link_too_many_is_refused_with_the_chain_named() {
        let graph = calls(
            r#"[{"from":"pick","to":"approach"},
                {"from":"approach","to":"align"},
                {"from":"align","to":"servo"},
                {"from":"servo","to":"nudge"}]"#,
        );
        let fault = validate_composition("pick", &graph, MAX_SKILL_DEPTH).expect("refused");
        assert_eq!(fault.code, CompositionFaultCode::TooDeep);
        assert_eq!(fault.path, ["pick", "approach", "align", "servo", "nudge"]);
    }

    #[test]
    fn a_cycle_is_refused_at_install_rather_than_found_at_runtime() {
        let direct = calls(r#"[{"from":"pick","to":"pick"}]"#);
        let fault = validate_composition("pick", &direct, MAX_SKILL_DEPTH).expect("refused");
        assert_eq!(fault.code, CompositionFaultCode::Cycle);
        assert_eq!(fault.path, ["pick", "pick"]);

        let indirect = calls(
            r#"[{"from":"pick","to":"approach"},
                {"from":"approach","to":"retry"},
                {"from":"retry","to":"pick"}]"#,
        );
        let fault = validate_composition("pick", &indirect, MAX_SKILL_DEPTH).expect("refused");
        assert_eq!(fault.path, ["pick", "approach", "retry", "pick"]);
    }

    /// Two paths reaching one skill is a shared child, not a loop. An edition marking nodes
    /// globally visited rather than tracking the current chain would refuse this, and it is legal.
    #[test]
    fn a_diamond_is_not_a_cycle() {
        let graph = calls(
            r#"[{"from":"pick","to":"left"},
                {"from":"pick","to":"right"},
                {"from":"left","to":"grasp"},
                {"from":"right","to":"grasp"}]"#,
        );
        assert!(validate_composition("pick", &graph, MAX_SKILL_DEPTH).is_none());
    }

    #[test]
    fn a_depth_limit_below_one_is_not_a_limit() {
        let graph = calls(r#"[{"from":"pick","to":"approach"}]"#);
        // Zero would refuse the root itself, which is not a composition rule.
        assert!(validate_composition("pick", &graph, 0).is_none());
        // One is a site that has switched composition off, and that is a real setting.
        assert!(validate_composition("pick", &graph, 1).is_some());
    }

    #[test]
    fn an_edge_whose_ends_are_not_names_is_not_an_edge() {
        let graph = calls(
            r#"[{"from":5,"to":"approach"},{"from":"pick"},null,{"from":"pick","to":"pick"}]"#,
        );
        assert_eq!(graph.len(), 1);
        assert!(read_calls(None).is_empty());
    }

    // ── §54.4's lifecycle ────────────────────────────────────────────────────

    /// A skill that is going to work: invoked, root, preconditions met, leases held, inside its
    /// deadline, on a robot that is running.
    ///
    /// Every test below starts here and breaks exactly one thing.
    fn going_well() -> Progress {
        Progress {
            invoked: true,
            root: true,
            entry_permitted: Some(true),
            leases: LeaseState::Held,
            elapsed_ms: Some(100.0),
            deadline: skill_deadline(Some(10.0), DEFAULT_OVERRUN_FACTOR),
            robot: OperatingState::Running,
            ..Progress::default()
        }
    }

    fn step(phase: SkillPhase, progress: &Progress) -> (SkillPhase, LifecycleReason) {
        let transition = advance(phase, progress);
        (transition.to, transition.reason)
    }

    /// The single most important assertion here, and it is the same one §27's machine opens with.
    ///
    /// A `Progress` nobody has filled in is an invocation nobody has made. Nothing about it advances.
    #[test]
    fn an_invocation_nobody_has_made_stays_idle() {
        let blank = Progress::default();
        assert_eq!(
            step(SkillPhase::Idle, &blank),
            (SkillPhase::Idle, LifecycleReason::NotInvoked)
        );
        assert!(!advance(SkillPhase::Idle, &blank).changed);
        assert!(!advance(SkillPhase::Idle, &blank).release_leases);
    }

    /// §54.2, from the direction that matters most: a runtime that has not evaluated the
    /// preconditions yet must not be able to start a skill by failing to ask.
    #[test]
    fn an_unevaluated_precondition_is_not_a_satisfied_one() {
        let progress = Progress {
            entry_permitted: None,
            ..going_well()
        };
        assert_eq!(
            step(SkillPhase::Preconditions, &progress),
            (
                SkillPhase::Preconditions,
                LifecycleReason::PreconditionsPending
            )
        );
    }

    /// §54.6's `skill.preconditions.enforced`: refused, with the predicate named.
    ///
    /// The naming is shared between two functions and neither is complete alone —
    /// [`evaluate_preconditions`] finds the clause, and the lifecycle carries it into the detail an
    /// operator reads. Asserting the join is what makes the conformance test writable.
    #[test]
    fn a_false_precondition_refuses_the_skill_and_names_the_clause() {
        let progress = Progress {
            entry_permitted: Some(false),
            refused_clause: "the cell door is closed".to_string(),
            ..going_well()
        };
        let transition = advance(SkillPhase::Preconditions, &progress);
        assert_eq!(transition.to, SkillPhase::Terminal);
        assert_eq!(transition.reason, LifecycleReason::PreconditionsRefused);
        assert_eq!(transition.outcome, Some(SkillOutcome::Aborted));
        assert!(transition.detail.contains("the cell door is closed"));
    }

    /// A refusal is `aborted`, never `failure`. A skill that was never allowed to start did not
    /// fail at its task, and a success rate computed over refusals would punish publishers for
    /// operators' cell configurations.
    #[test]
    fn a_refusal_is_never_recorded_as_a_failure() {
        for progress in [
            Progress {
                entry_permitted: Some(false),
                ..going_well()
            },
            Progress {
                leases: LeaseState::Denied,
                ..going_well()
            },
        ] {
            for phase in [SkillPhase::Preconditions, SkillPhase::Acquiring] {
                let transition = advance(phase, &progress);
                assert_ne!(transition.outcome, Some(SkillOutcome::Failure));
                assert_ne!(transition.outcome, Some(SkillOutcome::Success));
            }
        }
    }

    /// §36.1's `low` row: no new task leases, and the refusal carries the sentence the runtime
    /// composed rather than a generic one — because a flat battery and an unreadable gauge both
    /// withhold, and they send an operator to different places.
    #[test]
    fn power_withholds_the_lease_and_the_refusal_carries_its_own_sentence() {
        let progress = Progress {
            power_withheld: Some(
                "§36.1 withholds new task leases: this robot's power state is `reserve`"
                    .to_string(),
            ),
            ..going_well()
        };
        let transition = advance(SkillPhase::Acquiring, &progress);
        assert_eq!(transition.to, SkillPhase::Terminal);
        assert_eq!(transition.reason, LifecycleReason::PowerWithheld);
        assert_eq!(transition.outcome, Some(SkillOutcome::Aborted));
        assert!(
            transition.detail.contains("`reserve`"),
            "{}",
            transition.detail
        );
        assert!(
            transition.release_leases,
            "terminal releases, even from a phase that never took any — the effect is idempotent \
             and the alternative is a rule with an exception in it"
        );
    }

    /// The order matters and it is not arbitrary: both refusals can be true at once, and the one
    /// about the *robot* is the one an operator needs first. It is also the only order that works —
    /// the runtime does not ask §25.2's table when power withholds, so `leases` reads `Pending` and
    /// the other order would pin the invocation in `acquiring` until its deadline.
    #[test]
    fn power_is_answered_before_the_lease_table() {
        let progress = Progress {
            power_withheld: Some("the battery is below its reserve".to_string()),
            leases: LeaseState::Denied,
            lease_token: "manipulation.grasp".to_string(),
            lease_holder: "acme/pick-place".to_string(),
            ..going_well()
        };
        let transition = advance(SkillPhase::Acquiring, &progress);
        assert_eq!(transition.reason, LifecycleReason::PowerWithheld);
    }

    /// And a robot that is not withholding is not affected at all, which is every robot that
    /// declares no pack.
    #[test]
    fn no_power_refusal_leaves_the_acquiring_rules_exactly_as_they_were() {
        let progress = Progress {
            leases: LeaseState::Held,
            ..going_well()
        };
        let transition = advance(SkillPhase::Acquiring, &progress);
        assert_eq!(transition.reason, LifecycleReason::LeasesAcquired);
        assert_eq!(transition.to, SkillPhase::Running);
    }

    /// An empty sentence is not a refusal. §64 makes a refusal name its fix, and a skill aborted
    /// with a blank reason on it is the one shape that cannot.
    #[test]
    fn an_empty_power_sentence_is_not_a_refusal() {
        let progress = progress_from_json(
            &Json::parse(
                r#"{"invoked":true,"entry_permitted":true,"leases":"held","power_withheld":"  "}"#,
            )
            .expect("parses"),
        );
        assert_eq!(progress.power_withheld, None);
        assert_eq!(
            advance(SkillPhase::Acquiring, &progress).reason,
            LifecycleReason::LeasesAcquired
        );
    }

    #[test]
    fn a_denied_lease_names_the_token_and_the_holder() {
        let progress = Progress {
            leases: LeaseState::Denied,
            lease_token: "manipulation.grasp".to_string(),
            lease_holder: "acme/pick-place".to_string(),
            ..going_well()
        };
        let transition = advance(SkillPhase::Acquiring, &progress);
        assert_eq!(transition.to, SkillPhase::Terminal);
        assert_eq!(transition.reason, LifecycleReason::LeaseDenied);
        assert!(transition.detail.contains("manipulation.grasp"));
        assert!(transition.detail.contains("acme/pick-place"));
    }

    /// §25.2 refuses rather than interleaves, and a lease nobody has taken yet is not one that was
    /// granted — the same "absent is not a pass" rule as the preconditions above it.
    #[test]
    fn a_lease_nobody_has_taken_holds_the_skill_in_acquiring() {
        let progress = Progress {
            leases: LeaseState::Pending,
            ..going_well()
        };
        assert_eq!(
            step(SkillPhase::Acquiring, &progress),
            (SkillPhase::Acquiring, LifecycleReason::LeasePending)
        );
    }

    /// §54.5: *"Leases are held by the root invocation, not re-acquired per call. Nested
    /// acquisition is how you get a skill deadlocking against its own child."*
    #[test]
    fn a_nested_call_skips_acquiring_entirely() {
        let progress = Progress {
            root: false,
            ..going_well()
        };
        let transition = advance(SkillPhase::Preconditions, &progress);
        assert_eq!(transition.to, SkillPhase::Running);
        assert_eq!(transition.reason, LifecycleReason::LeasesHeldByRoot);
    }

    /// And the root does not. A default that skipped acquisition would let a caller who forgot the
    /// field past §25.2's arbitration entirely, which is two policies driving one elbow.
    #[test]
    fn a_default_invocation_is_a_root_and_therefore_acquires() {
        assert!(Progress::default().root);
        let progress = Progress {
            entry_permitted: Some(true),
            ..Progress::default()
        };
        assert_eq!(
            step(SkillPhase::Preconditions, &progress),
            (SkillPhase::Acquiring, LifecycleReason::PreconditionsMet)
        );
    }

    // ── What stops a running skill, and in which order ───────────────────────

    /// §54.6's `skill.timeout.enforced`.
    #[test]
    fn an_overrunning_skill_is_stopped_and_recorded_aborted() {
        // 10 s estimate, factor 2 → 20 000 ms.
        let progress = Progress {
            elapsed_ms: Some(20_000.0),
            ..going_well()
        };
        let transition = advance(SkillPhase::Running, &progress);
        assert_eq!(transition.to, SkillPhase::Terminal);
        assert_eq!(transition.reason, LifecycleReason::Timeout);
        assert_eq!(transition.outcome, Some(SkillOutcome::Aborted));
        assert!(transition.release_leases);
    }

    /// The deadline is inclusive, and that is a decision rather than an accident: a skill that has
    /// used exactly its budget has used it.
    #[test]
    fn the_deadline_is_reached_rather_than_passed() {
        let deadline = skill_deadline(Some(10.0), DEFAULT_OVERRUN_FACTOR);
        assert_eq!(deadline.deadline_ms, Some(20_000.0));
        let just_inside = Progress {
            elapsed_ms: Some(19_999.0),
            ..going_well()
        };
        assert_eq!(
            step(SkillPhase::Running, &just_inside).0,
            SkillPhase::Running
        );
    }

    /// A skill that declares no duration produces an unenforceable deadline, and an unenforceable
    /// deadline never expires. `skill_deadline` deliberately does not decide whether an unbounded
    /// skill may run; neither does this.
    #[test]
    fn an_unenforceable_deadline_never_times_out() {
        let progress = Progress {
            deadline: skill_deadline(None, DEFAULT_OVERRUN_FACTOR),
            elapsed_ms: Some(1e12),
            ..going_well()
        };
        assert_eq!(step(SkillPhase::Running, &progress).0, SkillPhase::Running);
    }

    /// §54.6's `skill.preconditions.rechecked`: a `hold` precondition falsified mid-run stops it.
    #[test]
    fn a_hold_precondition_lost_mid_run_stops_the_skill() {
        let progress = Progress {
            hold_lost: true,
            ..going_well()
        };
        let transition = advance(SkillPhase::Running, &progress);
        assert_eq!(transition.reason, LifecycleReason::HoldLost);
        assert_eq!(transition.outcome, Some(SkillOutcome::Aborted));
    }

    #[test]
    fn a_guard_trip_stops_the_skill_because_a_skill_is_not_an_exemption() {
        let progress = Progress {
            guard_tripped: true,
            ..going_well()
        };
        assert_eq!(
            step(SkillPhase::Running, &progress),
            (SkillPhase::Terminal, LifecycleReason::GuardTripped)
        );
    }

    /// §54.4's `running` row read through §27's machine: a robot that has stopped commanding has
    /// ended the skill, whatever stopped it.
    #[test]
    fn a_robot_that_stopped_commanding_ends_the_skill() {
        for state in [
            OperatingState::Stopping,
            OperatingState::Safe,
            OperatingState::Post,
            OperatingState::Unhomed,
        ] {
            let progress = Progress {
                robot: state,
                ..going_well()
            };
            assert_eq!(
                step(SkillPhase::Running, &progress),
                (SkillPhase::Terminal, LifecycleReason::RobotStopped),
                "`{}` is not a state a policy commands in",
                state.as_str()
            );
        }
    }

    /// `hold` is the exclusion worth asserting. It is where §27's machine sits between a lease
    /// being granted and a policy actually commanding, so aborting on it would kill every skill in
    /// the moment it started. And `degraded` runs — §45.5 makes an unavailable guard the ordinary
    /// case, and a skill refusing to proceed on one would refuse on most real robots.
    #[test]
    fn hold_and_degraded_do_not_end_a_running_skill() {
        for state in [OperatingState::Hold, OperatingState::Degraded] {
            let progress = Progress {
                robot: state,
                ..going_well()
            };
            assert_eq!(
                step(SkillPhase::Running, &progress).0,
                SkillPhase::Running,
                "`{}` must not abort a skill",
                state.as_str()
            );
        }
    }

    /// Causes before effects. A timeout makes the OS stop the skill under a controlled stop, and a
    /// guard trip stops the robot — so both *produce* a robot that is no longer commanding, and
    /// reporting the robot state first would hide the cause behind its own consequence.
    #[test]
    fn the_cause_is_reported_rather_than_its_consequence() {
        let everything = Progress {
            elapsed_ms: Some(20_000.0),
            hold_lost: true,
            guard_tripped: true,
            robot: OperatingState::Stopping,
            ..going_well()
        };
        assert_eq!(
            advance(SkillPhase::Running, &everything).reason,
            LifecycleReason::Timeout
        );

        let no_timeout = Progress {
            elapsed_ms: Some(1.0),
            ..everything.clone()
        };
        assert_eq!(
            advance(SkillPhase::Running, &no_timeout).reason,
            LifecycleReason::HoldLost
        );

        let no_hold = Progress {
            hold_lost: false,
            ..no_timeout.clone()
        };
        assert_eq!(
            advance(SkillPhase::Running, &no_hold).reason,
            LifecycleReason::GuardTripped
        );

        let only_the_robot = Progress {
            guard_tripped: false,
            ..no_hold
        };
        assert_eq!(
            advance(SkillPhase::Running, &only_the_robot).reason,
            LifecycleReason::RobotStopped
        );
    }

    // ── Verifying ────────────────────────────────────────────────────────────

    #[test]
    fn a_finished_skill_is_verified_rather_than_believed() {
        let progress = Progress {
            finished: true,
            ..going_well()
        };
        assert_eq!(
            step(SkillPhase::Running, &progress),
            (SkillPhase::Verifying, LifecycleReason::Verifying)
        );
        // And §54.3 has not judged yet, which is not a success.
        assert_eq!(
            step(SkillPhase::Verifying, &progress),
            (SkillPhase::Verifying, LifecycleReason::VerdictPending)
        );
    }

    #[test]
    fn every_judgement_reaches_the_terminal_phase_unchanged() {
        for outcome in SKILL_OUTCOMES {
            let progress = Progress {
                finished: true,
                outcome: Some(outcome),
                ..going_well()
            };
            let transition = advance(SkillPhase::Verifying, &progress);
            assert_eq!(transition.to, SkillPhase::Terminal);
            assert_eq!(transition.reason, LifecycleReason::Judged);
            assert_eq!(transition.outcome, Some(outcome));
        }
    }

    /// A skill whose robot fell over during verification did not succeed at anything, whatever a
    /// criterion evaluated to on the way down.
    #[test]
    fn an_abort_during_verification_outranks_the_verdict() {
        let progress = Progress {
            finished: true,
            outcome: Some(SkillOutcome::Success),
            guard_tripped: true,
            ..going_well()
        };
        let transition = advance(SkillPhase::Verifying, &progress);
        assert_eq!(transition.reason, LifecycleReason::GuardTripped);
        assert_eq!(transition.outcome, Some(SkillOutcome::Aborted));
    }

    // ── Leases, and the one place they are released ──────────────────────────

    /// §54.4's terminal row, on **every** path into it — including the refusals. A skill refused
    /// after its leases were taken must not keep them: that is the path where a lease leaks and the
    /// next invocation is refused the arm with nothing on any screen to say why.
    #[test]
    fn every_path_into_terminal_releases_the_leases() {
        let paths: [(SkillPhase, Progress); 5] = [
            (
                SkillPhase::Preconditions,
                Progress {
                    entry_permitted: Some(false),
                    ..going_well()
                },
            ),
            (
                SkillPhase::Acquiring,
                Progress {
                    leases: LeaseState::Denied,
                    ..going_well()
                },
            ),
            (
                SkillPhase::Running,
                Progress {
                    elapsed_ms: Some(20_000.0),
                    ..going_well()
                },
            ),
            (
                SkillPhase::Running,
                Progress {
                    guard_tripped: true,
                    ..going_well()
                },
            ),
            (
                SkillPhase::Verifying,
                Progress {
                    finished: true,
                    outcome: Some(SkillOutcome::Success),
                    ..going_well()
                },
            ),
        ];
        for (phase, progress) in paths {
            let transition = advance(phase, &progress);
            assert_eq!(transition.to, SkillPhase::Terminal);
            assert!(
                transition.release_leases,
                "`{}` reached terminal without releasing",
                phase.as_str()
            );
        }
    }

    /// And a skill already terminal releases nothing. A release emitted every cycle after a skill
    /// ended would be a release racing the next invocation's acquisition, which is the one way
    /// §25.2's arbitration can be lost by the thing that implements it.
    #[test]
    fn a_skill_already_over_releases_nothing_however_often_it_is_asked() {
        let progress = Progress {
            outcome: Some(SkillOutcome::Success),
            ..going_well()
        };
        for _ in 0..3 {
            let transition = advance(SkillPhase::Terminal, &progress);
            assert_eq!(transition.to, SkillPhase::Terminal);
            assert!(!transition.release_leases);
            assert!(!transition.changed);
            assert_eq!(transition.outcome, Some(SkillOutcome::Success));
        }
    }

    // ── Totality, and the walk a whole invocation takes ──────────────────────

    /// The happy path end to end, one `advance` at a time, with the evidence arriving as it would.
    #[test]
    fn a_whole_invocation_walks_the_diagram() {
        let mut phase = SkillPhase::Idle;
        let mut progress = Progress {
            invoked: true,
            deadline: skill_deadline(Some(10.0), DEFAULT_OVERRUN_FACTOR),
            robot: OperatingState::Running,
            ..Progress::default()
        };

        phase = advance(phase, &progress).to;
        assert_eq!(phase, SkillPhase::Preconditions);

        progress.entry_permitted = Some(true);
        phase = advance(phase, &progress).to;
        assert_eq!(phase, SkillPhase::Acquiring);

        progress.leases = LeaseState::Held;
        phase = advance(phase, &progress).to;
        assert_eq!(phase, SkillPhase::Running);

        progress.elapsed_ms = Some(500.0);
        phase = advance(phase, &progress).to;
        assert_eq!(phase, SkillPhase::Running, "still running is a fixed point");

        progress.finished = true;
        phase = advance(phase, &progress).to;
        assert_eq!(phase, SkillPhase::Verifying);

        progress.outcome = Some(SkillOutcome::Success);
        let last = advance(phase, &progress);
        assert_eq!(last.to, SkillPhase::Terminal);
        assert_eq!(last.outcome, Some(SkillOutcome::Success));
        assert!(last.release_leases);
    }

    /// Total, and self-consistent, over every phase and a wide sweep of evidence. The answers
    /// themselves are pinned by the corpus; this asserts the invariants that hold for all of them.
    #[test]
    fn the_lifecycle_is_total_and_never_moves_backwards() {
        let order = |phase: SkillPhase| {
            SKILL_PHASES
                .iter()
                .position(|candidate| *candidate == phase)
                .expect("every phase is in the list")
        };

        let mut evaluated = 0usize;
        for phase in SKILL_PHASES {
            for invoked in [false, true] {
                for root in [false, true] {
                    for entry in [None, Some(false), Some(true)] {
                        for leases in LEASE_STATES {
                            for elapsed in [None, Some(0.0), Some(1e9)] {
                                for hold_lost in [false, true] {
                                    for finished in [false, true] {
                                        for robot in [OperatingState::Running, OperatingState::Safe]
                                        {
                                            let progress = Progress {
                                                invoked,
                                                root,
                                                entry_permitted: entry,
                                                leases,
                                                elapsed_ms: elapsed,
                                                deadline: skill_deadline(
                                                    Some(10.0),
                                                    DEFAULT_OVERRUN_FACTOR,
                                                ),
                                                hold_lost,
                                                finished,
                                                robot,
                                                ..Progress::default()
                                            };
                                            let transition = advance(phase, &progress);
                                            assert_eq!(transition.from, phase);
                                            assert_eq!(transition.changed, transition.to != phase);
                                            // The diagram has no back edges. A phase that could go
                                            // backwards is a skill that could re-acquire a lease it
                                            // already holds, or re-run its preconditions after it
                                            // started — §54.5's deadlock, arrived at from inside.
                                            assert!(
                                                order(transition.to) >= order(phase),
                                                "{} went back to {}",
                                                phase.as_str(),
                                                transition.to.as_str()
                                            );
                                            // An outcome is a terminal fact and nothing else.
                                            if transition.outcome.is_some() {
                                                assert_eq!(transition.to, SkillPhase::Terminal);
                                            }
                                            assert_eq!(
                                                transition.release_leases,
                                                transition.to == SkillPhase::Terminal
                                                    && phase != SkillPhase::Terminal
                                            );
                                            evaluated += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(evaluated, 6 * 2 * 2 * 3 * 3 * 3 * 2 * 2 * 2);
    }

    /// Every reason is reachable. A reason nothing produces is a word on a console that never
    /// appears, and the way that happens is a rule being reordered above the one that produced it.
    #[test]
    fn every_lifecycle_reason_is_produced_by_some_evidence() {
        let cases: [(SkillPhase, Progress); 19] = [
            (SkillPhase::Idle, Progress::default()),
            (
                SkillPhase::Idle,
                Progress {
                    invoked: true,
                    ..Progress::default()
                },
            ),
            (
                SkillPhase::Preconditions,
                Progress {
                    entry_permitted: None,
                    ..going_well()
                },
            ),
            (
                SkillPhase::Preconditions,
                Progress {
                    entry_permitted: Some(false),
                    ..going_well()
                },
            ),
            (SkillPhase::Preconditions, going_well()),
            (
                SkillPhase::Preconditions,
                Progress {
                    root: false,
                    ..going_well()
                },
            ),
            (
                SkillPhase::Acquiring,
                Progress {
                    leases: LeaseState::Pending,
                    ..going_well()
                },
            ),
            (
                SkillPhase::Acquiring,
                Progress {
                    leases: LeaseState::Denied,
                    ..going_well()
                },
            ),
            (SkillPhase::Acquiring, going_well()),
            (
                SkillPhase::Running,
                Progress {
                    elapsed_ms: Some(1e9),
                    ..going_well()
                },
            ),
            (
                SkillPhase::Running,
                Progress {
                    hold_lost: true,
                    ..going_well()
                },
            ),
            (
                SkillPhase::Running,
                Progress {
                    guard_tripped: true,
                    ..going_well()
                },
            ),
            (
                SkillPhase::Running,
                Progress {
                    robot: OperatingState::Safe,
                    ..going_well()
                },
            ),
            (SkillPhase::Running, going_well()),
            (
                SkillPhase::Running,
                Progress {
                    finished: true,
                    ..going_well()
                },
            ),
            (SkillPhase::Verifying, going_well()),
            (
                SkillPhase::Verifying,
                Progress {
                    outcome: Some(SkillOutcome::Success),
                    ..going_well()
                },
            ),
            (SkillPhase::Terminal, going_well()),
            (
                SkillPhase::Acquiring,
                Progress {
                    power_withheld: Some("the battery is below its reserve".to_string()),
                    ..going_well()
                },
            ),
        ];
        let seen: Vec<LifecycleReason> = cases
            .iter()
            .map(|(phase, progress)| advance(*phase, progress).reason)
            .collect();
        for expected in LIFECYCLE_REASONS {
            assert!(
                seen.contains(&expected),
                "no evidence produces `{}`",
                expected.as_str()
            );
        }
    }

    // ── §54.5's third limit ──────────────────────────────────────────────────

    #[test]
    fn a_child_cannot_extend_its_parents_budget() {
        let child = skill_deadline(Some(60.0), DEFAULT_OVERRUN_FACTOR);
        assert_eq!(child.deadline_ms, Some(120_000.0));
        let bounded = bounded_deadline(&child, Some(5_000.0));
        assert_eq!(bounded.deadline_ms, Some(5_000.0));
        assert!(bounded.enforceable);
    }

    #[test]
    fn a_child_that_is_shorter_than_the_root_keeps_its_own_deadline() {
        let child = skill_deadline(Some(1.0), DEFAULT_OVERRUN_FACTOR);
        assert_eq!(
            bounded_deadline(&child, Some(90_000.0)).deadline_ms,
            Some(2_000.0)
        );
    }

    /// An unbounded skill nested under a bounded one does not make the tree unbounded.
    #[test]
    fn an_unbounded_child_is_still_bounded_by_its_parent() {
        let child = skill_deadline(None, DEFAULT_OVERRUN_FACTOR);
        assert!(!child.enforceable);
        let bounded = bounded_deadline(&child, Some(7_500.0));
        assert_eq!(bounded.deadline_ms, Some(7_500.0));
        assert!(bounded.enforceable);
    }

    /// A root with nothing left bounds its child at zero rather than at a negative number — which
    /// some other arithmetic could read as "no deadline".
    #[test]
    fn a_root_with_nothing_left_bounds_its_child_at_zero() {
        let child = skill_deadline(Some(60.0), DEFAULT_OVERRUN_FACTOR);
        assert_eq!(bounded_deadline(&child, Some(-1.0)).deadline_ms, Some(0.0));
        assert_eq!(bounded_deadline(&child, Some(0.0)).deadline_ms, Some(0.0));
    }

    /// A root that is itself unbounded bounds nothing, and the child keeps whatever it had —
    /// including having nothing.
    #[test]
    fn an_unbounded_root_leaves_its_child_alone() {
        let child = skill_deadline(Some(60.0), DEFAULT_OVERRUN_FACTOR);
        assert_eq!(bounded_deadline(&child, None), child);
        let unbounded = skill_deadline(None, DEFAULT_OVERRUN_FACTOR);
        assert_eq!(bounded_deadline(&unbounded, None), unbounded);
    }

    // ── Reading and rendering ────────────────────────────────────────────────

    #[test]
    fn an_empty_document_reads_as_an_invocation_nobody_has_made() {
        let parsed = progress_from_json(&Json::parse("{}").expect("parses"));
        assert_eq!(parsed, Progress::default());
    }

    /// Unreadable fields fall back to the value that does not advance the machine.
    #[test]
    fn unreadable_fields_do_not_advance_the_machine() {
        let parsed = progress_from_json(
            &Json::parse(
                r#"{"invoked":"yes","entry_permitted":"true","leases":"HELD",
                    "robot":"Running","outcome":"succeeded"}"#,
            )
            .expect("parses"),
        );
        assert!(!parsed.invoked);
        assert_eq!(parsed.entry_permitted, None);
        assert_eq!(parsed.leases, LeaseState::Pending);
        assert_eq!(parsed.robot, OperatingState::Post);
        assert_eq!(parsed.outcome, None);
    }

    #[test]
    fn the_json_shape_is_what_a_black_box_record_carries() {
        let progress = Progress {
            finished: true,
            outcome: Some(SkillOutcome::Success),
            ..going_well()
        };
        let json = transition_to_json(&advance(SkillPhase::Verifying, &progress));
        let text = json.to_canonical().expect("canonical");
        assert!(text.contains(r#""from":"verifying""#));
        assert!(text.contains(r#""to":"terminal""#));
        assert!(text.contains(r#""outcome":"success""#));
        assert!(text.contains(r#""release_leases":true"#));

        // Nullable rather than absent: "this transition is not terminal" is a fact.
        let running = transition_to_json(&advance(SkillPhase::Running, &going_well()));
        assert!(running
            .to_canonical()
            .expect("canonical")
            .contains(r#""outcome":null"#));
    }

    #[test]
    fn every_phase_round_trips_through_its_spelling() {
        for phase in SKILL_PHASES {
            assert_eq!(SkillPhase::parse(phase.as_str()), Some(phase));
        }
        assert_eq!(SkillPhase::parse("Running"), None);
        assert_eq!(SkillPhase::parse("succeeded"), None);
    }

    // ── §54.6's four conformance additions ───────────────────────────────────
    //
    // §54.6 names four tests and every one of them asserts **runtime** behaviour:
    //
    // * `skill.preconditions.enforced` — a skill whose precondition is false is refused, with the
    //   predicate named.
    // * `skill.preconditions.rechecked` — a `hold` precondition falsified mid-run stops the skill.
    // * `skill.timeout.enforced` — an overrunning skill is stopped and recorded `aborted`.
    // * `skill.success.independent` — a skill claiming success against a false criterion is
    //   recorded as failed.
    //
    // There is still no runtime, so these are not those tests. What they are is the half that can
    // exist without one: each drives the **contract** end to end — the evaluation, the judgement
    // and the lifecycle together — and asserts the answer §54.6 requires. When something loops, the
    // conformance test is that loop plus these assertions, rather than these decisions again.
    //
    // They are deliberately **not** added to `SCTS_TESTS`. That catalogue decides what a tier
    // requires, and `award_certification` reports a required test with no result as `missing` —
    // so adding four tests nothing can run would strip the tier from every robot on the strength of
    // evidence nobody can produce. §54.6 also does not say which tier they belong to, and guessing
    // one is how a certification rule nobody agreed to becomes load-bearing.

    /// `skill.preconditions.enforced`, over the two functions that share the job.
    #[test]
    fn scts_skill_preconditions_enforced() {
        let clauses = resolve_clauses(
            read_clauses(
                Some(
                    &Json::parse(
                        r#"[{"statement":"the cell door is closed",
                             "predicate":{"op":"eq","path":"env.door_closed","value":true}}]"#,
                    )
                    .expect("parses"),
                ),
                ClauseOrigin::Skill,
            ),
            Vec::new(),
        );
        let context = Json::parse(r#"{"env":{"door_closed":false}}"#).expect("parses");
        let report = evaluate_preconditions(&clauses, &context, Phase::Entry);

        assert!(!report.permitted);
        let refused = report.refused.as_ref().expect("the refusal names a clause");
        assert_eq!(refused.statement, "the cell door is closed");

        // …and the lifecycle refuses on it, carrying the name into what an operator reads.
        let progress = Progress {
            invoked: true,
            entry_permitted: Some(report.permitted),
            refused_clause: refused.statement.clone(),
            ..Progress::default()
        };
        let transition = advance(SkillPhase::Preconditions, &progress);
        assert_eq!(transition.to, SkillPhase::Terminal);
        assert_eq!(transition.reason, LifecycleReason::PreconditionsRefused);
        assert_eq!(transition.outcome, Some(SkillOutcome::Aborted));
        assert!(transition.detail.contains("the cell door is closed"));
        assert!(transition.release_leases);
    }

    /// `skill.preconditions.rechecked` — §54.2's second rule, which is the one free-text
    /// preconditions cannot have at all: "battery dropping below reserve mid-skill should stop the
    /// skill, not be discovered at the end."
    #[test]
    fn scts_skill_preconditions_rechecked() {
        let clauses = resolve_clauses(
            read_clauses(
                Some(
                    &Json::parse(
                        r#"[{"statement":"battery above reserve","hold":true,
                             "predicate":{"op":"gte","path":"robot.state.battery_percent",
                                          "value":20}}]"#,
                    )
                    .expect("parses"),
                ),
                ClauseOrigin::Skill,
            ),
            Vec::new(),
        );

        let charged = Json::parse(r#"{"robot":{"state":{"battery_percent":62}}}"#).expect("parses");
        let flat = Json::parse(r#"{"robot":{"state":{"battery_percent":11}}}"#).expect("parses");

        // It starts, because at entry the battery is fine.
        assert!(evaluate_preconditions(&clauses, &charged, Phase::Entry).permitted);
        // And the same clause, re-evaluated in the `hold` phase mid-run, is now false.
        let held = evaluate_preconditions(&clauses, &flat, Phase::Hold);
        assert!(!held.permitted);
        assert_eq!(
            held.results.len(),
            1,
            "only the `hold` clauses are re-checked"
        );

        let progress = Progress {
            invoked: true,
            entry_permitted: Some(true),
            leases: LeaseState::Held,
            elapsed_ms: Some(500.0),
            deadline: skill_deadline(Some(30.0), DEFAULT_OVERRUN_FACTOR),
            hold_lost: !held.permitted,
            robot: OperatingState::Running,
            ..Progress::default()
        };
        let transition = advance(SkillPhase::Running, &progress);
        assert_eq!(transition.to, SkillPhase::Terminal);
        assert_eq!(transition.reason, LifecycleReason::HoldLost);
        assert_eq!(transition.outcome, Some(SkillOutcome::Aborted));
    }

    /// `skill.timeout.enforced` — stopped, and recorded `aborted` rather than `failure`, so it
    /// "becomes a labelled training example rather than a gap".
    #[test]
    fn scts_skill_timeout_enforced() {
        let deadline = skill_deadline(Some(4.0), DEFAULT_OVERRUN_FACTOR);
        assert_eq!(deadline.deadline_ms, Some(8_000.0));

        let progress = Progress {
            invoked: true,
            entry_permitted: Some(true),
            leases: LeaseState::Held,
            deadline,
            elapsed_ms: Some(8_001.0),
            // The skill even claims it finished. It still overran.
            finished: true,
            robot: OperatingState::Running,
            ..Progress::default()
        };
        let transition = advance(SkillPhase::Running, &progress);
        assert_eq!(transition.reason, LifecycleReason::Timeout);
        assert_eq!(transition.outcome, Some(SkillOutcome::Aborted));
        assert!(transition.release_leases);
    }

    /// `skill.success.independent` — §54.3: "a skill that judges its own success is a skill that
    /// always succeeds."
    ///
    /// The skill's own claim is not an input to anything here. The only thing that decides is the
    /// criterion evaluated over post-state, and this asserts that a false criterion produces
    /// `failure` all the way into the terminal transition.
    #[test]
    fn scts_skill_success_independent() {
        let criteria = resolve_clauses(
            read_clauses(
                Some(
                    &Json::parse(
                        r#"[{"statement":"the gripper is empty",
                             "predicate":{"op":"eq","path":"robot.state.gripper","value":"open"}}]"#,
                    )
                    .expect("parses"),
                ),
                ClauseOrigin::Skill,
            ),
            Vec::new(),
        );
        let post_state =
            Json::parse(r#"{"robot":{"state":{"gripper":"closed"}}}"#).expect("parses");

        let judgement = judge_success(&criteria, &post_state);
        assert_eq!(judgement.outcome, SkillOutcome::Failure);
        assert_eq!(
            judgement
                .deciding
                .as_ref()
                .map(|result| result.statement.as_str()),
            Some("the gripper is empty")
        );

        let progress = Progress {
            invoked: true,
            entry_permitted: Some(true),
            leases: LeaseState::Held,
            finished: true,
            outcome: Some(judgement.outcome),
            robot: OperatingState::Running,
            ..Progress::default()
        };
        let transition = advance(SkillPhase::Verifying, &progress);
        assert_eq!(transition.to, SkillPhase::Terminal);
        assert_eq!(transition.outcome, Some(SkillOutcome::Failure));
    }

    /// And the other half of §54.3, because a conformance test that only checked the failing case
    /// would pass on an edition that always reported `failure`.
    #[test]
    fn scts_skill_success_independent_does_not_invent_a_pass_either() {
        let unenforceable = resolve_clauses(
            read_clauses(
                Some(&Json::parse(r#"["the cup is upright"]"#).expect("parses")),
                ClauseOrigin::Skill,
            ),
            Vec::new(),
        );
        let judgement = judge_success(&unenforceable, &Json::obj());
        assert_eq!(
            judgement.outcome,
            SkillOutcome::Unlabeled,
            "honest ignorance beats an invented pass"
        );

        let progress = Progress {
            invoked: true,
            entry_permitted: Some(true),
            leases: LeaseState::Held,
            finished: true,
            outcome: Some(judgement.outcome),
            robot: OperatingState::Running,
            ..Progress::default()
        };
        assert_eq!(
            advance(SkillPhase::Verifying, &progress).outcome,
            Some(SkillOutcome::Unlabeled)
        );
    }
}
