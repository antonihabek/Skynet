"""The operating state machine (§27, extended by §48.1), Python edition.

Transcribed from ``reference/typescript/opstate.ts``. Every rule, reason and ordering decision is
read from that file, never from ``corpus/``.

── It is a function of what is observed, not a table of edges ───────────────

§27 gives a table of **entry conditions**, not a list of edges, and that is the shape it keeps.
:func:`evaluate` is a pure function of the conditions observed right now, in a fixed priority order,
and every state is reachable from every other by the conditions changing. There is no edge to miss,
no transition anybody has to remember to fire, and recovery is not a special case — a guard coming
back is the same evaluation over a different input.

── The machine has exactly one bit of memory, and that bit is ``safe`` ──────

    `SAFE` is deliberately sticky. A robot that recovers itself from a safety stop is a robot that
    resumes moving while somebody is standing next to it working out what happened.

── The ordering is §33.4's asymmetry, stated out loud ───────────────────────

    Anything that reduces what the robot may do applies immediately … because making a robot less
    capable is never the dangerous direction.

So the rules are tried least-capable-first. Where two conditions both hold, the one that stops the
robot wins, and the reason reported is that one — which is also what an operator needs, because the
condition holding the robot is the condition they have to clear.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``conditions.post ?? 'incomplete'``
    Nullish, not truthy, and every default here is **the least capable reading**. A caller who has
    not observed something has not established it, and a machine that read "nobody told me about the
    kernel" as "the kernel is live" would be the single most dangerous line in this repository.
    ``dict.get(key)`` gives ``None`` for both absent and null, which is right for these — but the
    defaults it must fall back to are the *pessimistic* ones, and writing ``or`` instead of an
    explicit ``is None`` would make ``post: ''`` read as unobserved rather than as malformed.

``conditions.stop_settled === true`` and ``guard_hold === true``
    Strict. A truthy non-boolean does not settle a stop, and does not hold a robot.

``typeof cleared_by === 'string' && length > 0``
    A clearance is a **named** human action. A caller with nobody to name should not have one, so an
    empty string is not a clearance — and neither is a number, which ``if conditions.get(...)``
    would accept.
"""

from __future__ import annotations

from typing import Any, Final

__all__ = [
    "HOMING_STATES",
    "KERNEL_STATES",
    "OPERATING_STATES",
    "POLICY_STATES",
    "POST_VERDICTS",
    "RHAL_STATES",
    "STATE_REASONS",
    "STOP_CAUSES",
    "commands_autonomously",
    "evaluate",
    "is_safe_state",
    "is_sticky",
    "properties_to_json",
    "transition_to_json",
    "warning_matrix_to_json",
    "warning_required",
]

#: §27's states, ordered **least capable first** — the order the priority rules descend through.
OPERATING_STATES: Final = (
    "safe", "stopping", "post", "unhomed", "hold", "degraded", "running",
)

#: What §48.2's self-test concluded. ``incomplete`` is not a softening of ``failed``; it is the state
#: of a robot whose checks could not run, and such a robot "does not need a human. It needs the rest
#: of the operating system."
POST_VERDICTS: Final = ("passed", "failed", "incomplete")

#: ``not_required`` is a declaration, not an absence. A body that has said nothing is one whose
#: joint positions are unknown, so the default is ``pending``.
HOMING_STATES: Final = ("not_required", "pending", "complete")

#: ``lost`` is separate from ``degraded`` because the difference is the whole of ``safe``: a degraded
#: RHAL is a body that is still there and still answering, and a lost one is a body the kernel can no
#: longer command or observe.
RHAL_STATES: Final = ("present", "degraded", "lost")

#: §27's causes of a controlled stop. The list is closed on purpose: a cause should be added by
#: somebody who found this constant rather than a boolean called ``stopping``.
#:
#: The first four are the **kernel's own decisions** — it evaluated a guard, or a person asked, and
#: it commanded a stop. ``estop`` is not one of those: it is the body reporting that the hardware
#: emergency-stop chain is open, which happened whether or not any software agreed. §24 gives it one
#: bit, so a robot can say the chain is open and cannot say what opened it — which is why it is not
#: folded into ``operator``. An estop chain is also opened by a light curtain, a door interlock and a
#: safety relay noticing itself fail, and reporting those as a person having asked would put a name
#: on an event that has none.
STOP_CAUSES: Final = ("watchdog", "proximity", "balance", "operator", "estop")

#: ``held`` is a policy that exists and is not ready — staged, refused a lease, or waiting on a plan.
#: Distinct from ``none`` because the two send an operator to different places, and neither runs.
POLICY_STATES: Final = ("none", "held", "ready")

KERNEL_STATES: Final = ("live", "down")

#: One reason per rule, so the ordering is legible from the output: a robot reporting ``no_policy``
#: is one for which every rule above it did not fire, which is a stronger statement than the state.
STATE_REASONS: Final = (
    "rhal_lost",
    "safe_sticky",
    "post_failed",
    "stop_complete",
    "stopping_watchdog",
    "stopping_proximity",
    "stopping_balance",
    "stopping_operator",
    "stopping_estop",
    "post_incomplete",
    "homing_pending",
    "kernel_down",
    "guard_hold",
    "no_policy",
    "policy_not_ready",
    "guards_unavailable",
    "rhal_degraded",
    "nominal",
)


def _or(value: Any, default: Any) -> Any:
    """``value ?? default`` — nullish, so ``0`` and ``''`` survive it."""
    return default if value is None else value


def is_safe_state(state: str) -> bool:
    """Whether a state is one §33.4's barrier may open at: nothing held, nothing mid-task.

    ``stopping`` is not one, and it is the interesting entry: a robot mid-ramp is the state that most
    looks like it is about to be safe and most certainly is not — swapping a driver during the ramp
    would change the meaning of the deceleration in flight. ``safe``, ``post`` and ``unhomed`` are,
    which is what makes an OS update apply on a robot that failed its self-test: the remedy for a
    robot in ``safe`` is often the update.
    """
    return state in ("post", "unhomed", "hold", "safe")


def commands_autonomously(state: str) -> bool:
    """Whether a policy is commanding the body in this state.

    ``running`` and ``degraded``, and the second is the one that had been getting this wrong. §27's
    ``DEGRADED`` row says the robot "**runs**, reports, and refuses any package whose grants depend
    on the failed guard" — it is the impaired *running* state, not a third resting one.
    """
    return state in ("running", "degraded")


def is_sticky(state: str) -> bool:
    """Whether a state requires an explicit human action to leave (§27). Exactly one does."""
    return state == "safe"


def warning_required(from_state: str, to_state: str) -> bool:
    """Whether a warning tone must sound on this transition (§49.1).

    Phrased over :func:`commands_autonomously` so the two spellings cannot come apart. The version
    this replaced compared against the literal ``'running'`` and was under-inclusive: ``hold →
    degraded`` is a policy beginning to command a body, so a robot with a lapsed calibration — §45.5's
    ordinary case — would have begun autonomous motion silently. "Resumption after a stop" falls out,
    because ``stopping`` and ``safe`` command nothing.
    """
    return commands_autonomously(to_state) and not commands_autonomously(from_state)


def _cleared(conditions: dict[str, Any]) -> bool:
    """A clearance is a named human action. An empty name is not one, and neither is a number."""
    value = conditions.get("cleared_by")
    return isinstance(value, str) and len(value) > 0


def _stopping_reason(cause: str) -> str:
    """The reason a stop is in progress, by cause.

    A closed mapping rather than a template literal, so an added cause is a case the corpus rejects
    instead of a reason string nothing declares.
    """
    reasons = {
        "watchdog": "stopping_watchdog",
        "proximity": "stopping_proximity",
        "balance": "stopping_balance",
        "operator": "stopping_operator",
        "estop": "stopping_estop",
    }
    if cause not in reasons:
        raise AssertionError(f"unknown stop cause {cause!r}")
    return reasons[cause]


def _derive(conditions: dict[str, Any]) -> tuple[str, str]:
    """The state and reason the conditions imply, before stickiness."""
    rhal = _or(conditions.get("rhal"), "lost")
    stop = _or(conditions.get("stop"), None)

    # 1. The kernel lost the body. First because it is the condition under which nothing else can be
    #    known: a robot whose RHAL is gone has no observed joint state, no estop reading and no guard
    #    that can evaluate.
    if rhal == "lost":
        return "safe", "rhal_lost"

    # 2. A failed self-test. §48.2: `safe`, not `degraded`, "because a robot that failed its own
    #    self-test should not be talked into running by a reconcile loop".
    if conditions.get("post") == "failed":
        return "safe", "post_failed"

    # 3. A stop, complete or in progress. Above POST because an operator stop is honoured whatever
    #    the robot is doing, including booting.
    if stop is not None:
        if conditions.get("stop_settled") is True:
            return "safe", "stop_complete"
        return "stopping", _stopping_reason(stop)

    # 4. The self-test has not established that this robot may run. Not a fault: §48.4's
    #    `post_incomplete`, which is a robot waiting on the rest of the operating system.
    if _or(conditions.get("post"), "incomplete") != "passed":
        return "post", "post_incomplete"

    # 5. §48.1. Joint positions are unknown, forward kinematics is meaningless, and every
    #    pose-dependent guard is unavailable — so a policy must not run, and homing is the only
    #    motion permitted (§48.3).
    if _or(conditions.get("homing"), "pending") == "pending":
        return "unhomed", "homing_pending"

    # 6. §27: "a robot with a policy and no kernel must not exist". Refusing to start is `hold` — the
    #    safe resting state — rather than `safe`: nothing has gone wrong with the robot, and a sticky
    #    state needing a human would be the wrong answer for a kernel that is merely still starting.
    if _or(conditions.get("kernel"), "down") != "live":
        return "hold", "kernel_down"

    # 7. §27's `HOLD` row, first half. A guard asking for a hold outranks a policy that would like to
    #    run, which is the whole point of a guard.
    if conditions.get("guard_hold") is True:
        return "hold", "guard_hold"

    # 8. §27's `HOLD` row, second half. Split in two, because "there is nothing to run" and "there is
    #    something and it has not got a lease" are different sentences and the second is the one with
    #    a remedy — §25.2's arbitration is where an operator finds out who else is holding the arm.
    policy = _or(conditions.get("policy"), "none")
    if policy == "none":
        return "hold", "no_policy"
    if policy != "ready":
        return "hold", "policy_not_ready"

    # 9. §27's `DEGRADED` row, below the policy rules deliberately. Reaching here means a policy is
    #    ready to command, so `degraded` is the impaired running state rather than a resting one.
    if conditions.get("guards_unavailable") is True:
        return "degraded", "guards_unavailable"
    if rhal == "degraded":
        return "degraded", "rhal_degraded"

    return "running", "nominal"


def evaluate(previous: str, conditions: dict[str, Any] | None = None) -> dict[str, Any]:
    """The state this robot is in, given where it was and what is true now.

    Pure, clockless and total. Every combination of conditions produces a state, including
    combinations nobody anticipated, which is the property §33.3 asks of anything that converges.
    """
    if conditions is None:
        conditions = {}
    state, reason = _derive(conditions)

    # §27's stickiness, applied after the derivation rather than instead of it. A robot in `safe`
    # whose conditions would put it back in `safe` reports the *condition's* reason rather than
    # `safe_sticky`, because "the estop is still pressed" is what an operator needs and "you have not
    # cleared it yet" is not.
    stuck = previous == "safe" and not _cleared(conditions) and state != "safe"
    to = "safe" if stuck else state
    final_reason = "safe_sticky" if stuck else reason

    return {
        "from": previous,
        "to": to,
        "reason": final_reason,
        "changed": to != previous,
        "warning": warning_required(previous, to),
    }


# ── The wire shapes ──────────────────────────────────────────────────────────


def transition_to_json(case_input: dict[str, Any]) -> Any:
    """``opstate/evaluate`` — one evaluation of §27's machine."""
    transition = evaluate(case_input["previous"], case_input.get("conditions") or {})
    return {
        "from": transition["from"],
        "to": transition["to"],
        "reason": transition["reason"],
        "changed": transition["changed"],
        "warning": transition["warning"],
    }


def properties_to_json(_case_input: dict[str, Any]) -> Any:
    """``opstate/properties`` — the three predicates, over every state, frozen together.

    Pinned as a table rather than case by case because the three are read by different sections and
    an edition could get one right and another wrong for the same state without any single caller
    noticing.
    """
    return [
        {
            "state": state,
            "safe_state": is_safe_state(state),
            "commands": commands_autonomously(state),
            "sticky": is_sticky(state),
        }
        for state in OPERATING_STATES
    ]


def warning_matrix_to_json(_case_input: dict[str, Any]) -> Any:
    """``opstate/warning`` — §49.1's tone over every ordered pair of states.

    All forty-nine, because the rule that was wrong before was wrong on one pair and right on the
    one everybody tested.
    """
    return [
        {"from": from_state, "to": to_state, "warning": warning_required(from_state, to_state)}
        for from_state in OPERATING_STATES
        for to_state in OPERATING_STATES
    ]
