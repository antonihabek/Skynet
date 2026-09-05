"""Calibration validity — §45, Python edition.

Transcribed from ``reference/typescript/calibration.ts``. Every rule and message string is read from
that file, never from ``corpus/``.

Calibration is the only state in this system that is **safety-relevant, individual to one machine,
and drifts over time**. The descriptor is signed and static; profiles are signed and operator-set.
Calibration is measured, it decays, and several guards are built on it.

The failure this module exists to prevent is stated in §45.1 and it is not a dramatic one:

    a camera extrinsic off by four centimetres means the ``proximity`` guard measures separation to
    the wrong place while reporting perfectly healthy.

Nothing trips. Nothing looks wrong. The number is simply about the world as it was last spring.

── Expiry degrades; it never silently continues ─────────────────────────────

§45.5, which is §7.2's rule applied to a measurement rather than to a sensor: an expired record
makes the guards that depend on it report **unavailable, naming the expired parameter set**. Not
"assume it is probably still fine", which is what every system without an expiry policy does by
omission — not by decision, which is the point.

── A residual is a threshold, not a decoration ──────────────────────────────

§45.3: *"a hand-eye calibration with twelve millimetres of reprojection error is a failed
calibration, and accepting it silently produces a workspace guard that is precisely twelve
millimetres wrong forever."* A record whose residual exceeds its own declared threshold is
``failed`` the moment it is read, at any age. It never becomes valid and it never expires — it was
never a calibration.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``toFixed``
    Every number in every detail sentence goes through it, and the sentences are compared byte for
    byte. Ties go **away from zero** in ECMAScript and to **even** in Python's ``format``, so
    ``18.5 °C`` renders as ``19`` there and ``18`` here unless the rule is written out —
    :func:`skynet_os.js.to_fixed` is that rule, and it is one function rather than three because
    this edition already had two of it.

``if (!record.method)``
    Falsy, so an empty string is as missing as a null. That is the reading §45.3 wants: a method of
    ``""`` is not a method somebody recorded.

``Number.isFinite``
    :func:`_present` rejects ``NaN`` and the infinities, which ``isinstance(x, float)`` does not,
    and rejects booleans, which ``isinstance(True, int)`` lets through. A ``max_age_s`` of ``true``
    is not a validity of one second.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .js import to_fixed as _to_fixed

__all__ = [
    "EXPIRY_REASONS",
    "INVALIDATING_EVENTS",
    "PARAMETER_SETS",
    "all_sets",
    "calibration_to_json",
    "evaluate",
    "guards_fed_by",
    "guards_unavailable",
    "sets_for",
    "validity_detail",
]

#: The parameter sets §45.2 names, in the order that table gives them.
#:
#: A closed vocabulary rather than a free string, for the reason §26's event codes are closed: an
#: open one is how ``camera_extrinsics`` and ``camera-extrinsic`` end up naming the same measurement
#: on two robots in one fleet, and the guard mapping below then answers differently for each.
PARAMETER_SETS: Final = (
    "joint_zeros",
    "camera_intrinsics",
    "camera_extrinsics",
    "force_torque_bias",
    "imu_bias",
    "tool_offset",
    "gravity_vector",
)

#: §45.2's table, as data: which guards lose their input when a set expires.
#:
#: **This table is the deliverable of §45.2.** It converts "we should calibrate the cameras
#: sometime" into "an expired extrinsic disables the proximity guard", which is a sentence an
#: operator can act on and a fleet console can count.
#:
#: ``camera_intrinsics`` feeds ``proximity`` *indirectly*, through projection, and is listed rather
#: than omitted: a guard whose input is two transforms away is still measuring the wrong thing when
#: either is wrong.
_GUARDS_FED_BY: Final = {
    "joint_zeros": ("workspace", "self_collision"),
    "camera_intrinsics": ("proximity",),
    "camera_extrinsics": ("proximity", "workspace"),
    "force_torque_bias": ("envelope",),
    "imu_bias": ("balance",),
    "tool_offset": ("workspace",),
    "gravity_vector": ("balance", "envelope"),
}


def guards_fed_by(parameter_set: str) -> tuple[str, ...]:
    return _GUARDS_FED_BY.get(parameter_set, ())


#: The events §45.4 says invalidate a calibration outright.
#:
#:     "A robot that has just hit something has, by definition, possibly moved its cameras, and
#:     continuing to trust yesterday's extrinsics is exactly the wrong response to an impact."
INVALIDATING_EVENTS: Final = (
    "collision",
    "estop_under_load",
    "tool_change",
    "driver_replaced",
    "mainboard_swap",
)

#: The expiry reasons, as a closed vocabulary a console can translate.
EXPIRY_REASONS: Final = ("age", "joint_travel", "temperature", "event")


def validity_detail(validity: dict[str, Any]) -> str:
    state = validity["state"]
    if state == "valid":
        return ""
    if state == "absent":
        # Phrased to compose: this is read on its own in a table row that already names the set, and
        # inside "the `set` calibration is absent — …" in the guard list.
        return "nobody has ever measured it on this robot"
    return validity["detail"]


def _present(value: Any) -> bool:
    """``value !== null && value !== undefined && Number.isFinite(value)``."""
    if value is None or isinstance(value, bool) or not isinstance(value, (int, float)):
        return False
    number = float(value)
    return not (math.isnan(number) or math.isinf(number))


def evaluate(record: dict[str, Any], now: dict[str, Any]) -> dict[str, Any]:
    """Judge one record against the present (§45.4, §45.5).

    ── Order, and why it is this order ─────────────────────────────────────

    1. **Provenance**, because §45.3 makes it mandatory and a record without a method cannot be
       judged on anything else — "valid for another 30 days" would be a claim about a measurement
       nobody described.
    2. **Residual**, because a failed calibration was never valid and its age is irrelevant.
    3. **Events**, because §45.4 calls them the triggers that matter and they are a step rather than
       a slope: a robot that hit something has a bad extrinsic *now*, whatever its age says.
    4. Then the three gradual conditions, in the order §45.4's table gives them.

    A missing *current* reading does not extend validity. If the policy names a temperature delta
    and the robot cannot say what temperature it is, the record expires — the alternative is a rule
    that silently stops applying on exactly the robot whose sensor has failed.
    """
    policy = record.get("policy") or {}
    taken = record.get("conditions") or {}

    missing: list[str] = []
    if not record.get("method"):
        missing.append("method")
    if not _present(record.get("residual")):
        missing.append("residual")
    if missing:
        return {
            "state": "unusable",
            "detail": "§45.3 makes provenance mandatory and this record has no "
            f"{' and no '.join(missing)}. A calibration whose method nobody recorded is a rumour "
            "with six decimal places",
        }

    residual = record.get("residual")
    threshold = record.get("residual_threshold")
    if _present(residual) and _present(threshold) and residual > threshold:
        return {
            "state": "failed",
            "detail": f"the method reported a residual of {_number(residual)} against its own "
            f"threshold of {_number(threshold)}, so this was a failed calibration rather than one "
            "that has since expired",
        }

    invalidating = policy.get("invalidating_events") or []
    for event in record.get("events_since") or []:
        if event in invalidating:
            return {
                "state": "expired",
                "reason": "event",
                "detail": f"a `{event}` was recorded against this set since it was measured, and "
                "§45.4 invalidates it outright: continuing to trust the previous values is exactly "
                "the wrong response to one",
            }

    checked: list[str] = []

    max_age = policy.get("max_age_s")
    if _present(max_age):
        if _present(taken.get("at_s")) and _present(now.get("at_s")):
            age = now["at_s"] - taken["at_s"]
            if age > max_age:
                return {
                    "state": "expired",
                    "reason": "age",
                    "detail": f"measured {_to_fixed(age, 0)} s ago against a validity of "
                    f"{_to_fixed(max_age, 0)} s",
                }
            checked.append(f"age {_to_fixed(age, 0)}s of {_to_fixed(max_age, 0)}s")
        else:
            return {
                "state": "expired",
                "reason": "age",
                "detail": "this record expires with time and either it or this robot cannot say "
                "when it was taken, so its age cannot be established",
            }

    max_travel = policy.get("max_joint_travel_rad")
    if _present(max_travel):
        if _present(taken.get("joint_travel_rad")) and _present(now.get("joint_travel_rad")):
            travelled = now["joint_travel_rad"] - taken["joint_travel_rad"]
            if travelled > max_travel:
                return {
                    "state": "expired",
                    "reason": "joint_travel",
                    "detail": f"{_to_fixed(travelled, 1)} rad of joint travel since it was "
                    f"measured, against a validity of {_to_fixed(max_travel, 1)} rad",
                }
            checked.append(
                f"travel {_to_fixed(travelled, 1)} of {_to_fixed(max_travel, 1)} rad"
            )
        else:
            return {
                "state": "expired",
                "reason": "joint_travel",
                "detail": "this record expires with joint travel and no travel counter is "
                "available, so how far this robot has moved since cannot be established",
            }

    max_delta = policy.get("max_temperature_delta_c")
    if _present(max_delta):
        if _present(taken.get("temperature_c")) and _present(now.get("temperature_c")):
            delta = abs(now["temperature_c"] - taken["temperature_c"])
            if delta > max_delta:
                return {
                    "state": "expired",
                    "reason": "temperature",
                    "detail": f"{_to_fixed(delta, 1)} °C from the "
                    f"{_to_fixed(taken['temperature_c'], 1)} °C it was measured at, against a "
                    f"validity of {_to_fixed(max_delta, 1)} °C",
                }
            checked.append(
                f"temperature Δ{_to_fixed(delta, 1)} of {_to_fixed(max_delta, 1)} °C"
            )
        else:
            return {
                "state": "expired",
                "reason": "temperature",
                "detail": "this record expires with temperature and no reading is available, so "
                "how far this robot has drifted from its calibration conditions cannot be "
                "established",
            }

    return {"state": "valid", "checked": checked}


def _number(value: Any) -> str:
    """A number inside a template literal. ``String(0.4)`` is ``"0.4"``; ``str(0.4)`` agrees."""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float):
        if value != value:
            return "NaN"
        if math.isinf(value):
            return "Infinity" if value > 0 else "-Infinity"
        if value.is_integer() and abs(value) < 1e21:
            return str(int(value))
    return str(value)


def guards_unavailable(
    records: list[tuple[str, dict[str, Any]]], applicable: list[str]
) -> list[dict[str, Any]]:
    """Which guards lose their input, given what has been measured (§45.5).

    One entry per (guard, set) pair, deliberately: ``workspace`` disabled by expired joint zeros
    *and* an expired tool offset is two measurements somebody has to take, and collapsing them to
    one line would hide the second until the first was done.

    ``applicable`` names the sets that mean anything on this body — a fixed-base arm with no IMU
    should not report ``balance`` unavailable for want of an IMU bias it can never have.
    """
    out: list[dict[str, Any]] = []
    for parameter_set in applicable:
        held = next((entry for entry in records if entry[0] == parameter_set), None)
        validity = held[1] if held else {"state": "absent"}
        if validity["state"] == "valid":
            continue
        for guard in guards_fed_by(parameter_set):
            out.append(
                {
                    "guard": guard,
                    "set": parameter_set,
                    "state": validity["state"],
                    "detail": f"the `{parameter_set}` calibration is {validity['state']} — "
                    f"{validity_detail(validity)}",
                }
            )
    return out


def all_sets() -> list[str]:
    """Every parameter set, for a robot whose body does not say which apply."""
    return list(PARAMETER_SETS)


def sets_for(capabilities: list[str], base: str | None) -> list[str]:
    """The sets a body can actually hold, from what it has declared.

    Derived from the capability tokens and the base kind a descriptor already carries rather than
    from a new field, so a robot does not declare the same fact twice.

    ── Two different absences, and they are not treated the same ────────────

    **Undeclared capabilities** mean nobody said what this body senses, so every sensing-derived set
    is included: over-reporting what is missing is the conservative direction, and quietly deciding
    a guard was never relevant is not.

    **A declared ``fixed`` base** is not an absence at all — it is a positive statement that this
    body cannot fall over. ``balance`` is meaningless on a bolted-down arm, so an IMU bias and a
    gravity vector are not measurements it is missing; they are measurements it will never take, and
    reporting ``balance`` unavailable on one is a permanent red mark nobody can clear.

    An **unknown** base is mobile, because that is the absence again and it goes the other way. A
    body that does declare an IMU keeps both sets whatever its base, since it evidently has one.
    """
    mobile = True if base is None else base != "fixed"
    declared = len(capabilities) > 0

    def has(prefix: str) -> bool:
        return any(isinstance(c, str) and c.startswith(prefix) for c in capabilities)

    sets = ["joint_zeros"]
    if not declared or has("sensing.rgb") or has("sensing.depth") or has("sensing.camera"):
        sets.extend(("camera_intrinsics", "camera_extrinsics"))
    if not declared or has("sensing.force") or has("manipulation"):
        sets.extend(("force_torque_bias", "tool_offset"))
    if mobile or has("sensing.imu"):
        sets.extend(("imu_bias", "gravity_vector"))
    return sets


# ── The wire shapes ──────────────────────────────────────────────────────────


def calibration_to_json(case_input: dict[str, Any]) -> Any:
    """``calibration/*`` — every record's validity, and the guards that lose their input."""
    now = case_input.get("now") or {}
    records = [
        (record.get("set"), evaluate(record, now)) for record in case_input.get("records") or []
    ]
    applicable = (
        list(case_input["applicable"])
        if case_input.get("applicable") is not None
        else all_sets()
    )
    return {
        "records": [
            {
                "set": parameter_set,
                "state": validity["state"],
                "detail": validity_detail(validity),
                "reason": validity["reason"] if validity["state"] == "expired" else None,
                "checked": list(validity["checked"]) if validity["state"] == "valid" else [],
                "guards": list(guards_fed_by(parameter_set)),
            }
            for parameter_set, validity in records
        ],
        "unavailable": guards_unavailable(records, applicable),
    }


def sets_to_json(case_input: dict[str, Any]) -> Any:
    """``calibration/sets`` — which sets a body can hold at all (§45.2)."""
    return {
        "sets": sets_for(list(case_input.get("capabilities") or []), case_input.get("base")),
    }
