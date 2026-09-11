"""§54's skills, Python edition.

Transcribed from ``reference/typescript/skill.ts``. Every rule, code and message string is read
from that file, never from ``corpus/``.

Five sections in one module because they are one subject: what a skill must be true before it may
start (§54.2), who decides whether it worked (§54.3), when it is stopped (§54.4), how deep it may
call (§54.5), and the state machine that carries an invocation between them.

Two sentences shape the rest:

    A skill that judges its own success is a skill that always succeeds — and on a platform where
    success rates feed a marketplace, self-reported outcomes are worth nothing. (§54.3)

    A phase advances on evidence, and absence of evidence is not advancement.

The second is why almost every default here is "not gathered": ``entry_permitted`` is null until
§54.2 has actually run, ``outcome`` is null until §54.3 has judged, and both hold the lifecycle
where it is rather than moving it on.

── Where Python is quietly not JavaScript, here ──────────────────────────────

``Math.round``
    Rounds half toward **+∞**: ``Math.round(2.5)`` is 3 and ``Math.round(-0.5)`` is ``-0``. Python's
    ``round`` is half-to-**even**, so it gives 2 for the first — and the value being rounded is a
    *timeout in milliseconds*, so the disagreement is a skill stopped a millisecond apart in two
    editions and, at the boundary, a different verdict.

``record.hold === true`` and ``record.predicate && typeof … === 'object'``
    Strict and truthy respectively, in the same reader. ``hold: 1`` is **not** a hold, and a
    predicate of ``[]`` **is** an object to JavaScript — an array passes ``typeof x === 'object'``,
    so a clause whose predicate is a list is enforceable-but-malformed rather than unenforceable.
    Python's ``isinstance(x, dict)`` would silently take the other branch on both.

``typeof record.statement === 'string'``
    A clause whose statement is a number gets the empty string, not the number. The statement is
    what an operator reads on a refusal, and a `42` there is worse than a blank.

``path.includes(next)``
    Cycle detection is over the *current chain*, not over everything visited. A diamond — two paths
    to one skill — is not a cycle, and a ``set`` of visited nodes would refuse it.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .js import js_trim as _js_trim
from .predicate import evaluate_predicate_outcome

__all__ = [
    "CLAUSE_ORIGINS",
    "CLAUSE_OUTCOMES",
    "COMPOSITION_FAULTS",
    "DEFAULT_OVERRUN_FACTOR",
    "LEASE_STATES",
    "LIFECYCLE_REASONS",
    "MAX_SKILL_DEPTH",
    "PHASES",
    "SKILL_OUTCOMES",
    "SKILL_PHASES",
    "advance",
    "bounded_deadline",
    "bounded_deadline_to_json",
    "composition_to_json",
    "evaluate_preconditions",
    "judge_success",
    "lifecycle_to_json",
    "preconditions_to_json",
    "read_calls",
    "read_clauses",
    "resolve_clauses",
    "skill_deadline",
    "success_to_json",
    "timeout_to_json",
    "validate_composition",
]

#: Both origins, publisher first — which is also the order clauses are evaluated in.
CLAUSE_ORIGINS: Final = ("skill", "operator")

#: ``unenforceable`` is the fourth value the predicate engine does not have, and it belongs here
#: rather than there: the engine is asked about a predicate, and this is the case where there is no
#: predicate to ask about.
CLAUSE_OUTCOMES: Final = ("satisfied", "unsatisfied", "unevaluable", "unenforceable")

#: §54.2's two moments: before the skill starts, and every cycle while it runs.
PHASES: Final = ("entry", "hold")

#: §54.3/§54.4's terminal vocabulary, which is the Hub's ``EpisodeOutcome`` and not a second one.
SKILL_OUTCOMES: Final = ("success", "failure", "aborted", "unlabeled")

#: How far past its own estimate a skill may run before it is stopped. An operator setting with a
#: default rather than a manifest field — a publisher choosing how long past their own estimate they
#: may run is a publisher with no timeout.
DEFAULT_OVERRUN_FACTOR: Final = 2

#: §54.5's default. A skill four deep is a skill whose author can still describe what it does. The
#: root counts as depth 1, so a limit of 4 permits three levels of nesting beneath it.
MAX_SKILL_DEPTH: Final = 4

COMPOSITION_FAULTS: Final = ("cycle", "too_deep")

SKILL_PHASES: Final = (
    "idle", "preconditions", "acquiring", "running", "verifying", "terminal",
)

#: §25.2's arbitration, as the one fact the lifecycle needs from it.
LEASE_STATES: Final = ("pending", "held", "denied")

LIFECYCLE_REASONS: Final = (
    "not_invoked",
    "invoked",
    "preconditions_pending",
    "preconditions_refused",
    "preconditions_met",
    "leases_held_by_root",
    "lease_pending",
    "lease_denied",
    "power_withheld",
    "leases_acquired",
    "timeout",
    "hold_lost",
    "guard_tripped",
    "robot_stopped",
    "running",
    "verifying",
    "verdict_pending",
    "judged",
    "terminal",
)


class _Absent:
    """JavaScript's ``undefined``."""

    def __repr__(self) -> str:  # pragma: no cover — diagnostics only
        return "ABSENT"

    def __bool__(self) -> bool:
        return False


_ABSENT: Final = _Absent()


def _field(obj: Any, key: str) -> Any:
    if not isinstance(obj, dict):
        return _ABSENT
    return obj[key] if key in obj else _ABSENT


def _js_object(value: Any) -> bool:
    """``value && typeof value === 'object'`` — **arrays included**, ``null`` excluded.

    A clause whose predicate is a list therefore reaches the evaluator rather than being called
    unenforceable, and the evaluator refuses it as malformed. Two different reports, and the reader
    is not the place that decides which.
    """
    return isinstance(value, (dict, list)) and value is not None


def _js_round(value: float) -> int:
    """``Math.round`` — half toward **+∞**, which is neither of Python's roundings.

    ``round(2.5)`` is 2 in Python and 3 here. The value is a timeout in milliseconds, so the
    difference is a skill stopped a millisecond apart in two editions.
    """
    if not math.isfinite(value):
        return 0
    return math.floor(value + 0.5)


def _is_finite_number(value: Any) -> bool:
    """``typeof v === 'number' && Number.isFinite(v)`` — false for a boolean."""
    if isinstance(value, bool):
        return False
    return isinstance(value, (int, float)) and math.isfinite(value)


# ── §54.2 Clauses and preconditions ──────────────────────────────────────────


def read_clauses(value: Any) -> list[dict[str, Any]]:
    """Read a manifest's ``preconditions`` or ``success_criteria``, in either spelling.

    A bare string is prose with no predicate, which is what every skill published before §54.2
    carries and what this accepts so that reading one is not an error. It is not the same as
    *absent*: a skill that declares no preconditions has nothing to check, and a skill that declares
    three sentences has three things the OS cannot check. Those are different reports.
    """
    if not isinstance(value, list):
        return []
    clauses: list[dict[str, Any]] = []
    for entry in value:
        if isinstance(entry, str):
            clauses.append({"statement": entry})
            continue
        if not entry or not isinstance(entry, dict):
            continue
        statement = entry["statement"] if isinstance(entry.get("statement"), str) else ""
        predicate = entry.get("predicate") if _js_object(entry.get("predicate")) else None
        clauses.append(
            {"statement": statement, "predicate": predicate, "hold": entry.get("hold") is True}
        )
    return clauses


def resolve_clauses(
    published: list[dict[str, Any]], operator: list[dict[str, Any]] | None = None
) -> list[dict[str, Any]]:
    """The skill's clauses, then the site's. §54.2's narrowing-only rule, made structural.

    There is no argument by which the operator's list can shorten the result, because the operator's
    list is only ever appended — a removal would need a mechanism, and not building one is the
    enforcement. The publisher's clauses come first so that a refusal names the skill's own
    requirement before the site's where both fail, which is the order that tells an operator whether
    to call the publisher or walk to the cell.
    """
    if operator is None:
        operator = []
    return [{**clause, "origin": "skill"} for clause in published] + [
        {**clause, "origin": "operator"} for clause in operator
    ]


def _judge_clause(clause: dict[str, Any], index: int, context: Any) -> dict[str, Any]:
    base = {
        "index": index,
        "statement": clause.get("statement"),
        "origin": clause["origin"],
        "hold": clause.get("hold") is True,
    }
    if not clause.get("predicate"):
        return {**base, "outcome": "unenforceable", "reason": None, "path": None, "op": None}
    outcome = evaluate_predicate_outcome(clause["predicate"], context)
    return {
        **base,
        "outcome": outcome.outcome,
        "reason": outcome.reason,
        "path": outcome.path,
        "op": outcome.op,
    }


def evaluate_preconditions(
    clauses: list[dict[str, Any]], context: Any, phase: str = "entry"
) -> dict[str, Any]:
    """§54.2 — may this skill start, or continue?

    ``entry`` evaluates every clause. ``hold`` evaluates only those the manifest marked, because
    that is what runs every cycle. Every clause is evaluated even after one has failed, because an
    operator fixing four problems one attempt at a time is the failure ``admitPackage`` already
    refuses to cause; ``refused`` names the first, which is the one to put on a screen.
    """
    results: list[dict[str, Any]] = []
    for index, clause in enumerate(clauses):
        if phase == "hold" and clause.get("hold") is not True:
            continue
        results.append(_judge_clause(clause, index, context))

    refused = next((r for r in results if r["outcome"] != "satisfied"), None)
    return {
        "phase": phase,
        "permitted": refused is None,
        "refused": refused,
        "results": results,
        "unenforceable": sum(1 for r in results if r["outcome"] == "unenforceable"),
    }


# ── §54.3 Success ────────────────────────────────────────────────────────────


def judge_success(clauses: list[dict[str, Any]], post_state: Any) -> dict[str, Any]:
    """§54.3 — did this skill succeed? Judged by the OS, over post-state.

    Three answers, and the order they are decided in is the content:

    1. **Any criterion that is definitely false is a failure**, even if another could not be read.
       Evidence of failure beats absence of evidence.
    2. **Otherwise, anything unread is ``unlabeled``.** Many criteria genuinely are not
       machine-evaluable, and inventing a pass for those is how a leaderboard becomes fiction.
    3. **Only when every criterion held is it a success.**

    A skill that declares no criteria at all is ``unlabeled``, not ``success``. Nothing was measured,
    and a skill that scores itself by declaring nothing is the same gaming §54.3 exists to stop.
    """
    results = [_judge_clause(clause, index, post_state) for index, clause in enumerate(clauses)]

    failed = next((r for r in results if r["outcome"] == "unsatisfied"), None)
    if failed is not None:
        return {"outcome": "failure", "deciding": failed, "results": results}

    unread = next((r for r in results if r["outcome"] != "satisfied"), None)
    if unread is not None:
        return {"outcome": "unlabeled", "deciding": unread, "results": results}

    if not results:
        return {"outcome": "unlabeled", "deciding": None, "results": results}
    return {"outcome": "success", "deciding": None, "results": results}


# ── §54.4 Timeout ────────────────────────────────────────────────────────────


def skill_deadline(
    expected_duration_seconds: Any, overrun_factor: Any = DEFAULT_OVERRUN_FACTOR
) -> dict[str, Any]:
    """§54.4 — when a running skill is stopped and recorded ``aborted``.

    A skill that declares no duration produces ``enforceable: false`` rather than a guessed ceiling.
    **This does not decide what the runtime does about that**, and the specification does not
    either: refusing an unbounded skill and permitting one are both defensible, and inventing the
    answer inside a pure function is how a policy nobody agreed to ends up load-bearing.
    """
    factor = (
        overrun_factor
        if _is_finite_number(overrun_factor) and overrun_factor >= 1
        else DEFAULT_OVERRUN_FACTOR
    )
    declared = _is_finite_number(expected_duration_seconds) and expected_duration_seconds > 0
    if not declared:
        return {"deadline_ms": None, "enforceable": False, "factor": factor}
    return {
        "deadline_ms": _js_round(expected_duration_seconds * 1000 * factor),
        "enforceable": True,
        "factor": factor,
    }


# ── §54.5 Composition ────────────────────────────────────────────────────────


def read_calls(value: Any) -> list[dict[str, Any]]:
    """Read declared calls, skipping anything that is not one.

    A reader rather than a cast, and it exists so that both editions discard the same malformed
    edges at the same boundary: a typed ``SkillCall[]`` in Rust cannot hold ``{from: 5}``, so if this
    filtering lived inside :func:`validate_composition` the two editions would be filtering in
    different places and a corpus case for a malformed edge would be testing two different functions.
    """
    if not isinstance(value, list):
        return []
    calls: list[dict[str, Any]] = []
    for entry in value:
        if not entry or not isinstance(entry, dict):
            continue
        if not isinstance(entry.get("from"), str) or not isinstance(entry.get("to"), str):
            continue
        calls.append({"from": entry["from"], "to": entry["to"]})
    return calls


def validate_composition(
    root: str, calls: list[dict[str, Any]], max_depth: Any = MAX_SKILL_DEPTH
) -> dict[str, Any] | None:
    """§54.5 — is this call graph installable?

    Refusing at install is the whole point. A cycle discovered at runtime is discovered by a robot
    that has already taken a lease and started moving.

    The walk is depth-first in declaration order, so the chain a fault names is reproducible: an
    edition that iterated its edges differently would refuse the same graphs and blame different
    skills, which is the half an operator reads.
    """
    limit = (
        math.floor(max_depth)
        if _is_finite_number(max_depth) and max_depth >= 1
        else MAX_SKILL_DEPTH
    )

    # Declaration order is preserved so a fault's path is the same in every edition.
    edges: dict[str, list[str]] = {}
    for call in calls:
        if not call or not isinstance(call.get("from"), str) or not isinstance(call.get("to"), str):
            continue
        edges.setdefault(call["from"], []).append(call["to"])

    fault: dict[str, Any] | None = None

    def walk(node: str, path: list[str]) -> None:
        nonlocal fault
        if fault:
            return
        if len(path) > limit:
            fault = {
                "code": "too_deep",
                "path": list(path),
                "detail": f"This call chain is {len(path)} skills deep; the limit is {limit}.",
            }
            return
        for nxt in edges.get(node, []):
            if fault:
                return
            # Over the *current chain*, not everything visited: a diamond is not a cycle, and a
            # visited-set would refuse one.
            if nxt in path:
                fault = {
                    "code": "cycle",
                    "path": [*path, nxt],
                    "detail": f"{nxt} is already in this call chain, so it would call itself.",
                }
                return
            walk(nxt, [*path, nxt])

    walk(root, [root])
    return fault


def bounded_deadline(child: dict[str, Any], root_remaining_ms: Any) -> dict[str, Any]:
    """§54.5's third limit: the root's timeout bounds the whole tree.

    The child's own deadline, or what is left of the root's, whichever is sooner. Two properties
    fall out and both are the point: a child that declares **no** duration is still bounded by its
    parent, and a root with nothing left bounds its child at zero rather than at a negative number —
    so the child is stopped rather than handed a deadline already in the past that some other
    arithmetic could read as "no deadline".
    """
    if root_remaining_ms is None or not _is_finite_number(root_remaining_ms):
        return child
    remaining = max(0, root_remaining_ms)
    if not child["enforceable"] or child["deadline_ms"] is None:
        return {"deadline_ms": remaining, "enforceable": True, "factor": child["factor"]}
    return {
        "deadline_ms": min(child["deadline_ms"], remaining),
        "enforceable": True,
        "factor": child["factor"],
    }


# ── §54.4 The lifecycle ──────────────────────────────────────────────────────


def _robot_stopped_commanding(state: str) -> bool:
    """Whether a robot in this state has stopped a skill that was running.

    ``hold`` is deliberately **not** here: it is where §27's machine sits between a lease being
    granted and a policy actually commanding, so aborting on it would kill every skill in the moment
    it started. A guard asking for a hold reaches this machine as ``guard_tripped`` instead.
    """
    return state in ("stopping", "safe", "post", "unhomed")


def _overran(progress: dict[str, Any]) -> bool:
    """Whether the deadline has passed. An unenforceable deadline never has."""
    deadline = progress.get("deadline")
    if not deadline or not deadline.get("enforceable") or deadline.get("deadline_ms") is None:
        return False
    elapsed = progress.get("elapsed_ms")
    if not _is_finite_number(elapsed):
        return False
    return elapsed >= deadline["deadline_ms"]


def _abort_reason(progress: dict[str, Any]) -> dict[str, str] | None:
    """Why a running skill is over, or ``None`` if it is not.

    **Causes before effects.** A timeout makes the OS stop the skill under a controlled stop, and a
    guard trip stops the robot; both therefore *produce* a robot that is no longer commanding.
    Reporting the robot state first would hide the cause behind its own consequence.
    """
    if _overran(progress):
        deadline = progress.get("deadline") or {}
        return {
            "reason": "timeout",
            "detail": (
                f"past {_js_number(deadline.get('deadline_ms'))} ms, which is the declared "
                f"estimate times {_js_number(deadline.get('factor'))}"
            ),
        }
    if progress.get("hold_lost") is True:
        return {
            "reason": "hold_lost",
            "detail": (
                "a precondition marked `hold` stopped being true, and §54.2 re-checks those every "
                "cycle rather than only at entry"
            ),
        }
    if progress.get("guard_tripped") is True:
        return {
            "reason": "guard_tripped",
            "detail": "a guard tripped, and a skill is not an exemption",
        }
    robot = progress.get("robot")
    robot = "post" if robot is None else robot
    if _robot_stopped_commanding(robot):
        return {
            "reason": "robot_stopped",
            "detail": f"§27 says the robot is `{robot}`, which is not a state a policy commands in",
        }
    return None


def _js_number(value: Any) -> str:
    """A number inside a template literal. ``String(5000)`` is ``"5000"``, not ``"5000.0"``."""
    if value is None:
        return "undefined" if value is _ABSENT else "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float):
        if value != value:
            return "NaN"
        if value in (math.inf, -math.inf):
            return "Infinity" if value > 0 else "-Infinity"
        if value.is_integer() and abs(value) < 1e21:
            return str(int(value))
    return str(value)


def _lease_denied_detail(progress: dict[str, Any]) -> str:
    """What a denied acquisition tells the operator, which is two different errands.

    §25.2 keeps a *grant* and a *lease* apart, so a refusal has two shapes that send an operator to
    opposite places: a named holder means somebody else is using it, and no holder means this
    package was never granted it at all. The version this replaced said "is held by another package"
    for both, which sends an operator hunting for a package that does not exist.
    """
    if not progress.get("lease_token"):
        return "a required capability could not be leased, and nothing named which"
    if not progress.get("lease_holder"):
        return (
            f"`{progress['lease_token']}` was never granted to this package, so §25.2 will not let "
            "it take the lease — nobody is holding it. Grant it, or take it out of "
            "`required_capabilities`."
        )
    return f"`{progress['lease_token']}` is held by {progress['lease_holder']}"


def advance(phase: str, progress: dict[str, Any] | None = None) -> dict[str, Any]:
    """§54.4's lifecycle, advanced by one evaluation.

    Pure, clockless and total: ``elapsed_ms`` is supplied for the same reason the kernel takes
    ``now_ms`` as a parameter (§35.2), which is what lets a timeout be tested without waiting.
    """
    if progress is None:
        progress = {}

    def done(to: str, reason: str, outcome: str | None, detail: str) -> dict[str, Any]:
        return {
            "from": phase,
            "to": to,
            "reason": reason,
            "outcome": outcome,
            "detail": detail,
            # Only on the way in. A skill already terminal releases nothing, because it has nothing.
            "release_leases": to == "terminal" and phase != "terminal",
            "changed": to != phase,
        }

    if phase == "idle":
        return (
            done("preconditions", "invoked", None, "an invocation arrived")
            if progress.get("invoked") is True
            else done("idle", "not_invoked", None, "nothing has asked for this skill")
        )

    if phase == "preconditions":
        # Absent is not permitted. A runtime that had not evaluated §54.2 yet must not be able to
        # start a skill by failing to ask — which is the shape of every bug this repository is
        # arranged around.
        entry = progress.get("entry_permitted")
        if entry is not True and entry is not False:
            return done(
                "preconditions",
                "preconditions_pending",
                None,
                "the entry predicates have not been evaluated, and an unevaluated precondition is "
                "not a satisfied one",
            )
        if entry is False:
            return done(
                "terminal",
                "preconditions_refused",
                "aborted",
                f"refused by: {progress['refused_clause']}"
                if progress.get("refused_clause")
                else "refused by a precondition",
            )
        # §54.5. A nested invocation does not acquire; the root holds the leases for the whole tree.
        return (
            done(
                "running",
                "leases_held_by_root",
                None,
                "a nested call runs under the root invocation's leases (§54.5)",
            )
            if progress.get("root") is False
            else done("acquiring", "preconditions_met", None, "every evaluated precondition held")
        )

    if phase == "acquiring":
        # §36.1's `low` row, before the table is consulted: a refusal for want of energy is a fact
        # about the *robot* and a refusal for a held token is a fact about another package, so when
        # both are true the robot-wide one is what an operator needs first. Terminal rather than a
        # wait — an invocation parked hoping for a charger would burn §54.4's timeout and abort with
        # the wrong reason. And an empty sentence is not a refusal: §64 makes a refusal name its fix.
        withheld_raw = progress.get("power_withheld")
        withheld = _js_trim("" if withheld_raw is None else str(withheld_raw))
        if withheld:
            return done("terminal", "power_withheld", "aborted", withheld)
        leases = progress.get("leases")
        leases = "pending" if leases is None else leases
        if leases == "denied":
            return done("terminal", "lease_denied", "aborted", _lease_denied_detail(progress))
        if leases != "held":
            return done(
                "acquiring",
                "lease_pending",
                None,
                "the leases for `required_capabilities` have not been taken",
            )
        return done("running", "leases_acquired", None, "every required capability is leased")

    if phase == "running":
        aborted = _abort_reason(progress)
        if aborted:
            return done("terminal", aborted["reason"], "aborted", aborted["detail"])
        return (
            done("verifying", "verifying", None, "the skill returned; §54.3 has not judged it yet")
            if progress.get("finished") is True
            else done("running", "running", None, "the skill is running")
        )

    if phase == "verifying":
        # A skill can still be stopped while its post-state is being judged, and the abort outranks
        # the verdict: a skill whose robot fell over during verification did not succeed at anything.
        aborted = _abort_reason(progress)
        if aborted:
            return done("terminal", aborted["reason"], "aborted", aborted["detail"])
        outcome = progress.get("outcome")
        if outcome is None:
            return done(
                "verifying",
                "verdict_pending",
                None,
                "the success criteria have not been judged, and §54.3 will not let the skill judge "
                "itself",
            )
        return done("terminal", "judged", outcome, f"§54.3 judged this {outcome}")

    if phase == "terminal":
        return done("terminal", "terminal", progress.get("outcome"), "this invocation is over")

    raise AssertionError(f"unknown phase {phase!r}")


# ── The wire shapes ──────────────────────────────────────────────────────────


def _shape_clause_result(result: dict[str, Any]) -> Any:
    return {
        "index": result["index"],
        "statement": result["statement"],
        "origin": result["origin"],
        "hold": result["hold"],
        "outcome": result["outcome"],
        "reason": result["reason"],
        "path": result["path"],
        "op": result["op"],
    }


def preconditions_to_json(case_input: dict[str, Any]) -> Any:
    """``skill/preconditions`` — may this skill start, or continue?"""
    clauses = resolve_clauses(
        read_clauses(case_input.get("published")), read_clauses(case_input.get("operator"))
    )
    report = evaluate_preconditions(
        clauses, case_input.get("context"), case_input.get("phase", "entry")
    )
    return {
        "phase": report["phase"],
        "permitted": report["permitted"],
        "refused": None if report["refused"] is None else _shape_clause_result(report["refused"]),
        "results": [_shape_clause_result(r) for r in report["results"]],
        "unenforceable": report["unenforceable"],
    }


def success_to_json(case_input: dict[str, Any]) -> Any:
    """``skill/success`` — judged by the OS over post-state, never by the skill."""
    clauses = resolve_clauses(read_clauses(case_input.get("criteria")))
    judgement = judge_success(clauses, case_input.get("post_state"))
    return {
        "outcome": judgement["outcome"],
        "deciding": (
            None if judgement["deciding"] is None else _shape_clause_result(judgement["deciding"])
        ),
        "results": [_shape_clause_result(r) for r in judgement["results"]],
    }


def _shape_deadline(deadline: dict[str, Any]) -> Any:
    return {
        "deadline_ms": deadline["deadline_ms"],
        "enforceable": deadline["enforceable"],
        "factor": deadline["factor"],
    }


def timeout_to_json(case_input: dict[str, Any]) -> Any:
    """``skill/timeout`` — §54.4's deadline from a declared duration."""
    factor = case_input.get("overrun_factor")
    deadline = (
        skill_deadline(case_input.get("expected_duration_seconds"))
        if factor is None
        else skill_deadline(case_input.get("expected_duration_seconds"), factor)
    )
    return _shape_deadline(deadline)


def bounded_deadline_to_json(case_input: dict[str, Any]) -> Any:
    """``skill/bounded-deadline`` — §54.5: a child cannot extend its parent's budget."""
    child = skill_deadline(case_input.get("expected_duration_seconds"), DEFAULT_OVERRUN_FACTOR)
    return _shape_deadline(bounded_deadline(child, case_input.get("root_remaining_ms")))


def composition_to_json(case_input: dict[str, Any]) -> Any:
    """``skill/composition`` — is this call graph installable?"""
    calls = read_calls(case_input.get("calls"))
    max_depth = case_input.get("max_depth")
    fault = (
        validate_composition(case_input["root"], calls)
        if max_depth is None
        else validate_composition(case_input["root"], calls, max_depth)
    )
    return {
        "fault": None
        if fault is None
        else {"code": fault["code"], "path": list(fault["path"]), "detail": fault["detail"]}
    }


def lifecycle_to_json(case_input: dict[str, Any]) -> Any:
    """``skill/lifecycle`` — one evaluation of §54.4's machine."""
    transition = advance(case_input["phase"], case_input.get("progress") or {})
    return {
        "from": transition["from"],
        "to": transition["to"],
        "reason": transition["reason"],
        "outcome": transition["outcome"],
        "detail": transition["detail"],
        "release_leases": transition["release_leases"],
        "changed": transition["changed"],
    }
