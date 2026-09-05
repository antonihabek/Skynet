"""§31.5's readiness — what a robot has declared, and what tier it could attempt.

Transcribed from ``reference/typescript/readiness.ts``. Every check, message and ordering decision
is read from that file, never from ``corpus/``.

Pure. Every check is a question about a declared value, and **a check whose input is absent fails
rather than passing** — the same rule ``award_certification`` applies to a missing test result, for
the same reason: absence is not evidence of compliance.

``attainable`` is never a claim about the machine. It is the highest tier this robot could
*attempt*, given what has been declared, and the walk **stops** at the first tier with an unmet
requirement rather than skipping it: a robot that satisfies every certified-tier check but lacks a
watchdog window is not "certified with a caveat", it is a robot that cannot attempt the host tier.

── Where Python is quietly not JavaScript, here ─────────────────────────────

truthy details beside strict ``ok``
    Several checks decide ``ok`` with ``typeof x === 'number'`` and choose their *sentence* with a
    plain truthy test on the same field. A joint rated at **0** therefore passes the check and gets
    the failure sentence. That is the reference's behaviour and it is transcribed rather than
    reconciled — the two spellings are in the file, and quietly agreeing them here would be this
    edition disagreeing with the specification in order to be tidy.

``Object.keys(compute).length > 0``
    A declared but empty ``compute`` object is not onboard compute. ``{}`` is falsy in neither
    language and truthy in Python, so ``if compute:`` would be wrong twice over.

``limits?.estop === true``
    Strict. A hardware stop is fitted or it is not; ``1`` is not a stop.

``.sort()`` with a comparator
    Stable in both languages since ES2019, so the blockers keep their check order within a tier.
"""

from __future__ import annotations

import math
from typing import Any, Final

__all__ = [
    "READINESS_CHECKS",
    "evaluate_readiness",
    "fleet_to_json",
    "readiness_to_json",
    "summarize_readiness",
]

READINESS_CHECKS: Final = (
    "adapter",
    "kinematics",
    "control_modes",
    "control_rate",
    "envelope",
    "envelope_source",
    "watchdog",
    "joint_ratings",
    "estop_software",
    "estop_hardware",
    "onboard_compute",
)

_TIER_ORDER: Final = ("guest", "host", "certified")


def _js_number(value: Any) -> str:
    if isinstance(value, bool):
        return "true" if value else "false"
    if value is None:
        return "null"
    if isinstance(value, float):
        if value != value:
            return "NaN"
        if value in (math.inf, -math.inf):
            return "Infinity" if value > 0 else "-Infinity"
        if value.is_integer() and abs(value) < 1e21:
            return str(int(value))
    return str(value)


def _is_number(value: Any) -> bool:
    """``typeof x === 'number'`` — false for a boolean, and **true for NaN**, which is why the
    checks that use it pair it with a ``> 0`` rather than relying on it alone."""
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _truthy(value: Any) -> bool:
    """JavaScript truthiness: ``0``, ``''``, ``null``, ``undefined``, ``NaN`` and ``false`` are not.

    Notably **not** Python's: an empty list and an empty dict are truthy in JavaScript.
    """
    if value is None or value is False:
        return False
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return not (value == 0 or value != value)
    if isinstance(value, str):
        return value != ""
    return True


def _get(obj: Any, key: str) -> Any:
    """``obj?.key`` — ``None`` when the object is missing, which is what ``?.`` yields."""
    return obj.get(key) if isinstance(obj, dict) else None


def evaluate_readiness(subject: dict[str, Any]) -> dict[str, Any]:
    """Evaluate one robot."""
    caps = set(subject.get("capabilities") or [])
    limits = subject.get("limits")
    compute = subject.get("compute")
    compute = {} if compute is None else compute
    adapters = subject["adapter_count"]
    dof = subject.get("dof")
    modes = subject.get("control_modes") or []
    control_hz = subject.get("control_hz")

    source = _get(limits, "source")
    watchdog = _get(limits, "watchdog_ms")
    velocity = _get(limits, "joint_velocity_max_rad_s")
    torque = _get(limits, "joint_torque_max_nm")
    onboard = "platform.onboard_compute" in caps or len(compute) > 0

    checks: list[dict[str, Any]] = [
        {
            "check": "adapter",
            "ok": adapters > 0,
            "required_for": "guest",
            "detail": (
                f"{_js_number(adapters)} published adapter{'' if adapters == 1 else 's'}."
                if adapters > 0
                else "No published adapter, so nothing can talk to this robot yet."
            ),
        },
        {
            "check": "kinematics",
            "ok": _is_number(dof) and dof > 0,
            "required_for": "guest",
            "detail": (
                f"{_js_number(dof)} degrees of freedom declared."
                if _truthy(dof)
                else "No degree-of-freedom count declared."
            ),
        },
        {
            "check": "control_modes",
            "ok": len(modes) > 0,
            "required_for": "guest",
            "detail": (
                f"Accepts {', '.join(modes)}."
                if len(modes) > 0
                else "No control modes declared, so no policy can be matched to it."
            ),
        },
        {
            "check": "control_rate",
            "ok": _is_number(control_hz) and control_hz > 0,
            "required_for": "host",
            "detail": (
                f"Control loop runs at {_js_number(control_hz)} Hz."
                if _truthy(control_hz)
                else "No control rate declared, so loop stability cannot be checked against a policy."
            ),
        },
        {
            "check": "envelope",
            # Null and `{}` are deliberately different: nobody looked, versus somebody looked and
            # found nothing. Only the first blocks, and the message says which.
            "ok": limits is not None,
            "required_for": "host",
            "detail": (
                "No safety envelope declared. There is nothing for the kernel to enforce."
                if limits is None
                else "A safety envelope has been declared."
            ),
        },
        {
            "check": "envelope_source",
            "ok": bool(_truthy(source)),
            "required_for": "host",
            "detail": (
                f"Limits cite {source}."
                if _truthy(source)
                else "Limits cite no datasheet. A safety limit without provenance is a rumour."
            ),
        },
        {
            "check": "watchdog",
            "ok": _is_number(watchdog) and watchdog > 0,
            "required_for": "host",
            "detail": (
                f"Controller faults after {_js_number(watchdog)} ms without a command."
                if _truthy(watchdog)
                else "No watchdog window declared, so the watchdog guard cannot run."
            ),
        },
        {
            "check": "joint_ratings",
            "ok": _is_number(velocity) and _is_number(torque),
            "required_for": "host",
            "detail": (
                f"Rated to {_js_number(velocity)} rad/s and {_js_number(torque)} Nm."
                if _truthy(velocity) and _truthy(torque)
                else "Velocity and torque ceilings are not both declared, so commands cannot be "
                "clamped."
            ),
        },
        {
            "check": "estop_software",
            "ok": "safety.estop" in caps,
            "required_for": "guest",
            "detail": (
                "Exposes a software emergency stop."
                if "safety.estop" in caps
                else "No software emergency stop capability declared."
            ),
        },
        {
            "check": "estop_hardware",
            "ok": _get(limits, "estop") is True,
            "required_for": "certified",
            "detail": (
                "A hardware emergency stop is fitted."
                if _get(limits, "estop") is True
                else "No hardware emergency stop declared. The certified tier needs a stop path "
                "off the application processor."
            ),
        },
        {
            "check": "onboard_compute",
            "ok": onboard,
            "required_for": "host",
            "detail": (
                "Runs inference onboard."
                if onboard
                else "No onboard compute declared, so the OS cannot own the control loop here."
            ),
        },
    ]

    blockers = sorted(
        (check for check in checks if not check["ok"]),
        key=lambda check: _TIER_ORDER.index(check["required_for"]),
    )

    return {
        "slug": subject["slug"],
        "name": subject["name"],
        "attainable": _highest_attainable(checks),
        "checks": checks,
        "blockers": blockers,
        "completeness": sum(1 for c in checks if c["ok"]) / len(checks),
    }


def _highest_attainable(checks: list[dict[str, Any]]) -> str | None:
    """Walk upward and **stop** at the first tier with an unmet requirement.

    Stopping rather than skipping matters: tiers are cumulative, and a robot that satisfies every
    certified-tier check but lacks a watchdog window cannot attempt the host tier.
    """
    best: str | None = None
    for tier in _TIER_ORDER:
        if any(check["required_for"] == tier and not check["ok"] for check in checks):
            return best
        best = tier
    return best


def summarize_readiness(reports: list[dict[str, Any]]) -> dict[str, Any]:
    """Aggregate across the registry.

    ``common_blockers`` is the number worth putting on a screen: it says whether the ecosystem's
    problem is missing adapters or missing datasheets, and those imply completely different work.
    """
    by_tier: dict[str, int] = {"none": 0, "guest": 0, "host": 0, "certified": 0}
    # Insertion-ordered, because the sort below is stable and ties therefore keep first-seen order.
    counts: dict[str, int] = {}

    for report in reports:
        attainable = report["attainable"]
        by_tier["none" if attainable is None else attainable] += 1
        for blocker in report["blockers"]:
            counts[blocker["check"]] = counts.get(blocker["check"], 0) + 1

    return {
        "total": len(reports),
        "by_tier": by_tier,
        "common_blockers": sorted(
            ({"check": check, "robots": robots} for check, robots in counts.items()),
            key=lambda entry: -entry["robots"],
        ),
    }


# ── The wire shapes ──────────────────────────────────────────────────────────


def _shape_check(check: dict[str, Any]) -> Any:
    return {
        "check": check["check"],
        "ok": check["ok"],
        "required_for": check["required_for"],
        "detail": check["detail"],
    }


def readiness_to_json(case_input: dict[str, Any]) -> Any:
    """``readiness/robot`` — one robot's checks, blockers and attainable tier."""
    report = evaluate_readiness(case_input)
    return {
        "slug": report["slug"],
        "name": report["name"],
        "attainable": report["attainable"],
        "checks": [_shape_check(c) for c in report["checks"]],
        "blockers": [_shape_check(c) for c in report["blockers"]],
        "completeness": report["completeness"],
    }


def fleet_to_json(case_input: dict[str, Any]) -> Any:
    """``readiness/fleet`` — where the ecosystem is actually stuck."""
    reports = [evaluate_readiness(subject) for subject in case_input["subjects"]]
    fleet = summarize_readiness(reports)
    return {
        "total": fleet["total"],
        "by_tier": fleet["by_tier"],
        "common_blockers": [
            {"check": entry["check"], "robots": entry["robots"]}
            for entry in fleet["common_blockers"]
        ],
    }
