//! Skills — preconditions, success, timeout and composition (§54).
//!
//! The Rust edition of `reference/typescript/skill.ts`. Read that file for the design and for the
//! asymmetry at the centre of it; this one records what the port costs.
//!
//! Everything here is pure. The lifecycle that will call it — `idle → preconditions → acquiring →
//! running → verifying → terminal` — needs capability leases and §27's state machine, neither of
//! which exists. These are the decisions that lifecycle will make, frozen now so that when
//! something does loop it is not also inventing the rules.
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
}
