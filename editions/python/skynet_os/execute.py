"""§88 — what admits a motion request on a program-level body. Python edition of
``reference/typescript/execute.ts``.

§54.1 ended with two sentences that stood from the moment Part VI was written: *"``execute({ skill,
arguments, timeout_s })`` already existed on the Hub side as the invocation point. Nothing routed
it."* For a joint-commanded body that stopped being true once §54.4's lifecycle was driven; for
§59.2's machines it was still true, and §84 removed the first of Phase 4's two blockers by giving a
motion a place in the descriptor.

# The second blocker dissolved rather than being bridged

§33.1 keeps ``skynetd`` off the path that holds a driver, so a routed skill's destination is
``skynetk``'s and there was no channel from the supervisor to it. Both obvious remedies spend
§33.1's licence. §88.1's answer is that the route already exists: a skill over a program-level body
is a policy process emitting motion requests into the ring it would post joint commands into, and
what was missing was not a channel but the **admission rule** for a request arriving on one.

# It is a gate, and calling it a guard would be a claim

§7's guards clamp a *value* against a resolved ceiling. There is no ceiling for ``move_l``. What is
checkable is **authority, state, and the body's own declared table** — the last being the half §84.1
created and nothing consumed.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``Number.isFinite``
    A ``bool`` is an ``int`` in Python, so a parameter of ``true`` would be commanded as 1 without
    this. ``NaN`` is caught here rather than by the bounds comparison, which is false for every
    operator and would pass it.

``Object.prototype.hasOwnProperty.call``
    ``name in parameters`` is the same question, and it is the right one: a parameter supplied as
    ``null`` is *supplied*, and is refused as not finite rather than treated as omitted.
"""

from __future__ import annotations

import math
from typing import Any, Final

__all__ = [
    "MOTION_PROGRESS",
    "MOTION_READY_STATES",
    "MOTION_REFUSALS",
    "admit_motion",
    "motion_progress",
    "motion_to_json",
    "progress_to_json",
]

#: §27's states in which a body will take a command.
#:
#: ``running`` and ``degraded``, and the second is not a softening: §27 puts ``DEGRADED`` on a body
#: that is *still there and still answering*, and §36.3's whole ladder is about a robot that keeps
#: working while giving things up.
MOTION_READY_STATES: Final = ("running", "degraded")

#: Why a motion request was refused, in the order :func:`admit_motion` evaluates them.
#:
#: Declaration order **is** precedence. The first four are about the robot and the last six about
#: the request: a caller reading a refusal from the first group has nothing to fix in its own bytes.
MOTION_REFUSALS: Final = (
    "no_lease",
    "estop",
    "not_active",
    "busy",
    "unknown_motion",
    "deadline_undeclared",
    "parameter_unknown",
    "parameter_missing",
    "parameter_not_finite",
    "parameter_out_of_bounds",
)

MOTION_PROGRESS: Final = ("running", "overdue")


def _is_finite(value: Any) -> bool:
    """``Number.isFinite``. A ``bool`` is not a number, whatever ``isinstance`` says."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return False
    return not (math.isnan(float(value)) or math.isinf(float(value)))


def _refuse(refusal: str, detail: str) -> dict[str, Any]:
    return {
        "accepted": False,
        "refusal": refusal,
        "detail": detail,
        "arguments": [],
        "unbounded": [],
    }


def _number_text(value: Any) -> str:
    """A number rendered as JavaScript renders one inside a template string.

    ``canonical.format_number`` is that rule, and it is reused rather than reimplemented: this is
    the one place §88's messages carry a figure, and a second number-to-string in this edition is
    the divergence §78 is about, one digit wide and only in a refusal nobody reads twice.
    """
    from .canonical import format_number

    return format_number(value)


def admit_motion(gate: dict[str, Any], request: dict[str, Any]) -> dict[str, Any]:
    """Whether this request may be handed to the driver, and with what.

    Authority before state, state before content. A request from a holder that does not hold the
    body is refused **without the platform reading what it asked for**, because reading it would
    mean answering a question nobody was entitled to ask.

    **``parameter_unknown`` is a refusal here and a warning at plan time, and the asymmetry is the
    point.** §84.3 raises ``motion_parameter_unused`` on a transport plan and does not refuse,
    because the controller does take the motion. At command time the same fact is a value about to
    be handed to a controller that will either interpret it or reject it on a moving robot, and
    §25.2's argument applies unchanged.
    """
    held = gate.get("lease_held_by")
    holder = request.get("holder")
    held = held if isinstance(held, str) else None
    holder = holder if isinstance(holder, str) else None

    if held is None:
        return _refuse(
            "no_lease",
            "Nothing holds this body, so there is nobody whose motion this would be (§25.2).",
        )
    if holder is None or held != holder:
        return _refuse(
            "no_lease",
            f"This body is held by `{held}` and the request came from "
            f"`{holder if holder is not None else 'nobody'}` (§25.2).",
        )

    if gate.get("estop") is True:
        return _refuse(
            "estop",
            "The emergency-stop chain is open (§24). What opened it is not something the body can "
            "say, so the only correct response is to command nothing.",
        )

    state = gate.get("state")
    if state not in MOTION_READY_STATES:
        return _refuse(
            "not_active",
            f"§27 has this body in `{'' if state is None else state}`, which is not a state in "
            "which it will run a program. Accepting the request would be queueing work behind "
            "whatever the body is doing instead — a controlled stop, a self-test, or waiting to be "
            "homed.",
        )

    executing = gate.get("executing")
    if isinstance(executing, str) and executing != "":
        return _refuse(
            "busy",
            f"The controller is executing `{executing}`. A program-level controller owns the servo "
            "loop (§59.2), so it finishes or it is aborted; there is no second motion to interleave "
            "with the first.",
        )

    name = request.get("motion")
    motion = next(
        (
            candidate
            for candidate in (gate.get("motions") or [])
            if isinstance(candidate, dict) and candidate.get("name") == name
        ),
        None,
    )
    if motion is None:
        return _refuse(
            "unknown_motion",
            f"This body declares no motion called `{'' if name is None else name}`. The name a "
            "request carries is the **body's** spelling, which §84.3's plan resolved at install — a "
            "policy's own name reaching here unresolved is the defect this refusal usually means.",
        )

    deadline = request.get("deadline_ms")
    if not _is_finite(deadline) or deadline <= 0:
        return _refuse(
            "deadline_undeclared",
            "A motion takes seconds, during which §7.1's watchdog guard has nothing to watch: a "
            "correct policy produces no command at all. The deadline is what replaces it (§88.3), "
            "so a request without one is a program nothing would ever conclude had failed.",
        )

    # First wins, as §84.1's reader does with a repeated name: `duplicate_parameter` is the
    # descriptor's fault. A gate that preferred the later one would command against a bound the
    # check never looked at.
    declared: list[dict[str, Any]] = []
    for parameter in motion.get("parameters") or []:
        if not isinstance(parameter, dict):
            continue
        if not any(known.get("name") == parameter.get("name") for known in declared):
            declared.append(parameter)

    parameters = request.get("parameters")
    parameters = parameters if isinstance(parameters, dict) else {}

    # The request's own names first: a parameter the motion has no slot for means the policy and the
    # body disagree about the motion's *shape*, which subsumes arguing about its values.
    from .js import utf16_key

    for supplied in sorted(parameters, key=utf16_key):
        if any(known.get("name") == supplied for known in declared):
            continue
        return _refuse(
            "parameter_unknown",
            f"`{motion.get('name')}` does not take `{supplied}`. §84.3 warned about this at "
            "install — `motion_parameter_unused` — and declined to refuse a plan over it, because "
            "the controller does take the motion. Here it is a value about to be sent to a "
            "controller that has no slot for it, and §25.2 is clear that commanding with the parts "
            "of a command a machine happened to understand is worse than refusing.",
        )

    arguments: list[dict[str, Any]] = []
    unbounded: list[str] = []

    for parameter in declared:
        pname = parameter.get("name")
        if pname not in parameters:
            # §84.1's fail-closed reading: anything but an explicit `false` is required.
            if parameter.get("required") is not False:
                return _refuse(
                    "parameter_missing",
                    f"`{motion.get('name')}` requires `{pname}` and the request does not carry it. "
                    "§84.1 reads a requirement fail-closed, so a parameter whose `required` the "
                    "descriptor never declared is required here too.",
                )
            # Absent rather than zero. §7.2's rule, and the controller owns its own defaults.
            continue

        value = parameters[pname]
        if not _is_finite(value):
            return _refuse(
                "parameter_not_finite",
                f"`{pname}` is not a finite number. A controller handed one would either fault or "
                "move somewhere nobody chose.",
            )

        minimum = parameter.get("min")
        maximum = parameter.get("max")
        has_min = _is_finite(minimum)
        has_max = _is_finite(maximum)
        if has_min and value < minimum:
            return _refuse(
                "parameter_out_of_bounds",
                f"`{pname}` is {_number_text(value)} and `{motion.get('name')}` declares a minimum "
                f"of {_number_text(minimum)}.",
            )
        if has_max and value > maximum:
            return _refuse(
                "parameter_out_of_bounds",
                f"`{pname}` is {_number_text(value)} and `{motion.get('name')}` declares a maximum "
                f"of {_number_text(maximum)}.",
            )
        if not has_min and not has_max:
            unbounded.append(pname)

        arguments.append({"name": pname, "value": value})

    detail = (
        f"`{motion.get('name')}` accepted, with every supplied parameter inside a bound this body "
        "declared."
        if not unbounded
        else f"`{motion.get('name')}` accepted. "
        + ("One parameter was" if len(unbounded) == 1 else "Some parameters were")
        + " not checked against anything, because this body declares no bound for "
        + ", ".join(unbounded)
        + " — which §5.2 reads as *nobody declared one*, never as unbounded."
    )

    return {
        "accepted": True,
        "refusal": None,
        "detail": detail,
        "arguments": arguments,
        "unbounded": unbounded,
    }


def motion_progress(deadline_ms: float, started_at_ms: float, now_ms: float) -> dict[str, Any]:
    """Whether a motion in flight has run past its deadline (§88.3).

    Separate from :func:`admit_motion` because one is asked **once per request** and this is asked
    **every cycle** until the controller answers.
    """
    elapsed = now_ms - started_at_ms
    over = elapsed - deadline_ms
    # `>` rather than `>=`, as §87.4's lease is: the instant the deadline falls is still inside it.
    if over > 0:
        return {
            "progress": "overdue",
            "overdue_by_ms": over,
            "detail": f"The controller has held this motion for {_number_text(elapsed)} ms against "
            f"a {_number_text(deadline_ms)} ms deadline. §7.1's watchdog has nothing to watch on "
            "this path (§88.3), so this is the bound that fires: abort the motion, enter §27's "
            "fault path, and record it.",
        }
    return {
        "progress": "running",
        "overdue_by_ms": over,
        "detail": f"Inside its deadline, with {_number_text(-over)} ms left.",
    }


# ── The wire shapes ──────────────────────────────────────────────────────────


def motion_to_json(case_input: dict[str, Any]) -> Any:
    """``execute/motion`` — §88's verdict over one request.

    ``arguments`` is an **array in the motion's declaration order**, which is the whole reason it is
    on the verdict rather than left to the caller: a program-level controller's motion is a call
    with positional arguments, and an object would be alphabetised by canonical JSON into an
    argument list no controller agreed to.
    """
    verdict = admit_motion(case_input.get("gate") or {}, case_input.get("request") or {})
    return {
        "accepted": verdict["accepted"],
        "refusal": verdict["refusal"],
        "detail": verdict["detail"],
        "arguments": [
            {"name": entry["name"], "value": entry["value"]} for entry in verdict["arguments"]
        ],
        "unbounded": list(verdict["unbounded"]),
    }


def progress_to_json(case_input: dict[str, Any]) -> Any:
    """``execute/progress`` — §88.3's per-cycle question about a motion already in flight."""
    report = motion_progress(
        case_input.get("deadline_ms") or 0,
        case_input.get("started_at_ms") or 0,
        case_input.get("now_ms") or 0,
    )
    return {
        "progress": report["progress"],
        "overdue_by_ms": report["overdue_by_ms"],
        "detail": report["detail"],
    }
