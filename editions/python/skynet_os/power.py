"""§36's power and thermal assessment, Python edition.

Transcribed from ``reference/typescript/power.ts``. Every constant, rule and message string is read
from that file, never from ``corpus/``.

Two ideas hold the module together.

**A state and a fault answer different questions**, and an operator needs both: the state says how
much energy is left, and a fault says whether that number can be believed. So they are computed
separately and reported side by side rather than collapsed into one worse number.

**Every absence produces a named unavailability rather than an assumption.** An empty
``brownout_fault_codes`` means *this vendor has not said which of its codes mean sag* — not *this
body cannot brown out*.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``Number.isFinite``
    False for a boolean, and it guards every reading. A ``state_of_charge`` of ``true`` is not 1.0.

``Math.floor(ms / 1000)``
    Floors toward −∞ in both languages, but ``/`` is float division in the reference and would be
    integer division under a naive transcription — which agrees for every non-negative input and
    stops agreeing the moment somebody removes the clamp above it.

``${charge}`` in a fault detail
    ``String(1.4)`` is ``"1.4"`` and ``str(1.4)`` is ``"1.4"``, but ``String(2)`` is ``"2"`` where
    ``str(2.0)`` is ``"2.0"``. The out-of-range charge is interpolated verbatim, so it goes through
    :func:`_js_number`.

``codes.includes(fault_code)``
    ``[1, 2].includes(True)`` is **false** in JavaScript and ``True in [1, 2]`` is **true** in
    Python, because ``True == 1``. A vendor fault code of ``true`` would trigger §36.4's brownout
    override — the one rule that ignores the gauge and stops the robot immediately — in a naive
    edition and be ignored by the reference.
"""

from __future__ import annotations

import math
from typing import Any, Final

__all__ = [
    "LOW_BAND",
    "NEVER_SHED",
    "POWER_FAULT_CODES",
    "POWER_REASONS",
    "POWER_STATES",
    "RESERVE_BAND",
    "RESERVE_TERMS",
    "SHED_ORDER",
    "SUSTAINED_THROTTLE_MS",
    "THERMAL_PRESSURES",
    "THROTTLE_DEPTH",
    "assess",
    "budget_missed",
    "claims_a_pack",
    "degraded",
    "grants_new_leases",
    "power_to_json",
    "pressure",
    "reserve",
    "reserve_complete",
    "shed",
    "state_grants_new_leases",
    "state_notifies_operator",
    "state_requires_safe_state",
    "state_stops_immediately",
]

#: §36.2's terms, in the order that section lists them.
RESERVE_TERMS: Final = ("declared_energy", "payload", "distance_to_safe_state")

#: Where the ``reserve`` band ends, as a multiple of the reserve. A quarter of a reserve wide,
#: because §36.1's two rows are otherwise a single point and everything under it — and a gauge
#: reporting within a percent of the boundary would chatter between *begin a controlled stop* and
#: *stop immediately*, on a robot that is mid-ramp.
RESERVE_BAND: Final = 1.25

#: Where the ``low`` band ends. **One whole reserve above the reserve band**, and that width is the
#: argument: ``low``'s only behaviour is that a running task may finish and no new one may start, so
#: the warning has to arrive with at least one further safe stop's worth of energy in hand.
LOW_BAND: Final = 2.25

#: §36.1's states, most capable first, plus the one §36.1 does not have.
POWER_STATES: Final = ("normal", "low", "reserve", "critical", "unmetered")

POWER_REASONS: Final = (
    "brownout", "below_reserve", "at_reserve", "approaching_reserve", "above_reserve",
    "no_pack", "no_reading", "no_reserve", "no_capacity", "reading_unusable",
)

POWER_FAULT_CODES: Final = (
    "gauge_silent", "capacity_undeclared", "reserve_undeclared", "brownout_undeclarable",
    "charge_out_of_range", "thermal_sustained", "thermal_critical", "thermal_undeclared",
)

#: Thirty seconds. §36.3 raises DEGRADED on *sustained* throttling and does not say how long, and
#: both mistakes are real: one cycle makes every heavy motion a degraded robot, and ten minutes
#: makes DEGRADED a state a thermally limited robot never reaches.
SUSTAINED_THROTTLE_MS: Final = 30_000

#: §36.3's ladder, weakest claim first.
SHED_ORDER: Final = (
    "telemetry", "ota", "blackbox_compression", "inference_rate", "non_essential_sensors",
)

#: What is never given up, at any pressure (§36.3).
NEVER_SHED: Final = ("rhal", "kernel", "control_loop")

#: The first three rungs. The line between three and four is the meaningful one: telemetry, OTA and
#: black-box compression are housekeeping, and giving them up costs the robot nothing it was hired
#: to do. Rungs four and five change what it can *do*.
THROTTLE_DEPTH: Final = 3

THERMAL_PRESSURES: Final = ("nominal", "throttling", "critical")


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


def _is_finite(value: Any) -> bool:
    """``Number.isFinite`` — false for a boolean."""
    if isinstance(value, bool):
        return False
    return isinstance(value, (int, float)) and math.isfinite(value)


def _js_number(value: Any) -> str:
    """A number inside a template literal."""
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


def _strict_includes(haystack: Any, needle: Any) -> bool:
    """``Array.prototype.includes`` — which compares by ``SameValueZero``, so ``true`` is not ``1``.

    ``True in [1, 2]`` is true in Python and ``[1, 2].includes(true)`` is false. This one guards
    §36.4's brownout override: the only rule that ignores the gauge and stops the robot immediately.
    """
    if not isinstance(haystack, (list, tuple)):
        return False
    for item in haystack:
        if isinstance(item, bool) != isinstance(needle, bool):
            continue
        if item == needle:
            return True
    return False


def claims_a_pack(profile: dict[str, Any]) -> bool:
    """Whether this body claims a battery at all.

    A pack is claimed by declaring either its capacity or the energy a safe stop costs — the second
    because an energy budget in watt-hours is a statement about a pack even when nobody wrote the
    capacity down. A body that declares neither is a mains machine, and ``unmetered`` is the correct
    reading of its silence.
    """
    return (
        _field(profile, "pack_capacity_wh") is not _ABSENT
        or _field(profile, "safe_stop_energy_wh") is not _ABSENT
    )


def reserve_complete(res: dict[str, Any]) -> bool:
    """Whether every term §36.2 names was accounted for. False on every robot today."""
    return res["wh"] is not None and not res["unaccounted"]


def reserve(profile: dict[str, Any]) -> dict[str, Any]:
    """Compute §36.2's reserve budget from what the body declares.

    The arithmetic is deliberately the identity: the reserve *is* ``safe_stop_energy_wh``, because
    the other two terms §36.2 names have no source. Scaling it by a plausible payload factor would
    have been one line and a number nobody could defend — and this is the number that decides when a
    two-metre humanoid stops walking.
    """
    unaccounted: list[str] = []
    declared = _field(profile, "safe_stop_energy_wh")
    usable = declared is not _ABSENT and _is_finite(declared) and declared > 0
    if not usable:
        unaccounted.append(RESERVE_TERMS[0])
    # Neither exists on this platform. Appended unconditionally rather than looked up, so the day one
    # of them arrives this is a visible place to change rather than a silent over-report.
    unaccounted.append(RESERVE_TERMS[1])
    unaccounted.append(RESERVE_TERMS[2])
    return {"wh": declared if usable else None, "unaccounted": unaccounted}


def state_grants_new_leases(state: str) -> bool:
    """Whether §25.2 may grant a new task lease in this state.

    ``unmetered`` grants, and that is the whole reason the state exists: a bench arm on mains must
    not be refused work because it has no battery to report on.
    """
    return state in ("normal", "unmetered")


def state_requires_safe_state(state: str) -> bool:
    return state in ("reserve", "critical")


def state_stops_immediately(state: str) -> bool:
    return state == "critical"


def state_notifies_operator(state: str) -> bool:
    return state in ("low", "reserve", "critical")


def fault_degrades(code: str) -> bool:
    """Whether a fault raises §27's ``DEGRADED``.

    The two thermal ones do, because §36.3 says so in as many words. The declaration gaps do not: a
    descriptor missing a field is a descriptor to fix, and a robot that reported itself degraded for
    it would be reporting a paperwork problem in the same column as a hot motor.
    """
    return code in ("thermal_sustained", "thermal_critical")


def fault_withholds_leases(code: str) -> bool:
    """Whether a fault withholds new task leases.

    A robot that cannot tell whether it is at reserve should not be handed *new* work, and should
    not have taken from it the work it is already doing — which is §36.1's ``low`` behaviour arrived
    at from the other direction.
    """
    return code in (
        "gauge_silent", "capacity_undeclared", "reserve_undeclared",
        "charge_out_of_range", "thermal_critical",
    )


def pressure(profile: dict[str, Any], temperature_c: Any = _ABSENT) -> str:
    """The pressure a temperature implies, given what the body declares.

    A temperature with no declared threshold is ``nominal`` **and a fault**, not a silent pass:
    nobody has said what hot is on this body. Guessing 85 °C would be right for a CPU package and
    wrong for a harmonic drive by fifty degrees.
    """
    if temperature_c is _ABSENT or not _is_finite(temperature_c):
        return "nominal"
    critical = _field(profile, "thermal_critical_c")
    if critical is not _ABSENT and temperature_c >= critical:
        return "critical"
    throttle = _field(profile, "thermal_throttle_c")
    if throttle is not _ABSENT and temperature_c >= throttle:
        return "throttling"
    return "nominal"


def shed(level: str) -> list[str]:
    """What is given up at this pressure, in ``SHED_ORDER`` order (§36.3).

    Never returns anything in ``NEVER_SHED``: there is no thermal situation in which giving up the
    safety envelope is the right trade, and inference is throttled before control rather than beside
    it.
    """
    depth = 0 if level == "nominal" else (THROTTLE_DEPTH if level == "throttling" else len(SHED_ORDER))
    return list(SHED_ORDER[:depth])


def budget_missed(declared_ms: Any = _ABSENT, observed_ms: Any = _ABSENT) -> str | None:
    """Whether a package's declared inference budget can still be met (§36.3's last sentence).

    The comparison is against the **observed** loop time rather than against the thermal state,
    which is the honest coupling: thermal throttling is the usual cause of a missed budget and not
    the only one, and a robot that blocked a package for being warm while its loop was comfortably
    inside budget would be enforcing a proxy instead of the thing itself.
    """
    if declared_ms is _ABSENT or not _is_finite(declared_ms) or declared_ms <= 0:
        return None
    if observed_ms is _ABSENT or not _is_finite(observed_ms):
        return None
    if observed_ms <= declared_ms:
        return None
    return (
        f"this package declares an inference budget of {_js_number(declared_ms)} ms and the loop "
        f"is taking {_js_number(observed_ms)} ms. §36.3 throttles inference before control, so the "
        "loop is late rather than the robot being unsafe — but a policy quietly missing its own "
        "declared deadline is what the budget exists to prevent, so it is refused until the "
        "measurement comes back inside it."
    )


def grants_new_leases(assessment: dict[str, Any]) -> bool:
    """The state's answer, narrowed by any fault."""
    return state_grants_new_leases(assessment["state"]) and not any(
        fault_withholds_leases(f["code"]) for f in assessment["faults"]
    )


def degraded(assessment: dict[str, Any]) -> bool:
    """Whether §27's ``DEGRADED`` is raised by power or thermal (§36.3)."""
    return any(fault_degrades(f["code"]) for f in assessment["faults"])


def _remaining_wh(profile: dict[str, Any], observation: dict[str, Any]) -> float | None:
    """The energy the gauge accounts for, watt-hours.

    ``energy_remaining_wh`` wins over a fraction times a capacity, because a gauge reporting energy
    has been told the pack's capacity by something closer to the pack than a descriptor field. A
    fraction outside 0–1 produces ``None`` rather than a clamp.
    """
    energy = _field(observation, "energy_remaining_wh")
    if energy is not _ABSENT and _is_finite(energy) and energy >= 0:
        return energy
    charge = _field(observation, "state_of_charge")
    if charge is _ABSENT or not _is_finite(charge) or charge < 0 or charge > 1:
        return None
    capacity = _field(profile, "pack_capacity_wh")
    if capacity is _ABSENT or not _is_finite(capacity) or capacity <= 0:
        return None
    return charge * capacity


def _charge_unusable(observation: dict[str, Any]) -> bool:
    """Whether the gauge produced a state of charge that is not a state of charge."""
    charge = _field(observation, "state_of_charge")
    if charge is _ABSENT:
        return False
    return not _is_finite(charge) or charge < 0 or charge > 1


def _band(remaining: float, budget: float) -> str:
    """§36.1's bands."""
    if remaining < budget:
        return "critical"
    if remaining < budget * RESERVE_BAND:
        return "reserve"
    if remaining < budget * LOW_BAND:
        return "low"
    return "normal"


def _band_reason(remaining: float, budget: float) -> str:
    """Kept beside :func:`_band` rather than derived from the state, so the distinction between
    *at reserve* and *below it* — the one §36.1 spends two rows on — survives."""
    return {
        "critical": "below_reserve",
        "reserve": "at_reserve",
        "low": "approaching_reserve",
    }.get(_band(remaining, budget), "above_reserve")


def assess(profile: dict[str, Any], observation: dict[str, Any]) -> dict[str, Any]:
    """Assess §36 for one cycle.

    The order of the rules is the design, and the first one is what §36.4 is written for.

    1. **Brownout.** The bus said sag; the gauge is not consulted, "because a gauge that disagrees
       with the bus is a gauge that is wrong". The only rule that ignores a reading it has.
    2. **No pack declared.** A mains machine, correctly reporting that its energy is not metered.
    3. **No reading.** A body that claims a pack and got nothing back. Not ``critical``, and the
       fault is what makes it distinguishable from rule 2.
    4. **No reserve, or no capacity.** A reading arrived and cannot be compared against anything.
    5. **The bands.** The only rule that looks at a number, and last for that reason.
    """
    budget = reserve(profile)
    temperature = _field(observation, "temperature_c")
    thermal = pressure(profile, temperature)
    faults: list[dict[str, Any]] = []
    remaining = _remaining_wh(profile, observation)
    claims_pack = claims_a_pack(profile)
    codes = _field(profile, "brownout_fault_codes")
    codes = [] if codes is _ABSENT or codes is None else codes
    fault_code = _field(observation, "fault_code")
    no_reading = (
        _field(observation, "state_of_charge") is _ABSENT
        and _field(observation, "energy_remaining_wh") is _ABSENT
    )

    # Rule 1, before the faults are gathered because §36.4's override does not depend on any of
    # them. A brownout with a silent gauge is still a brownout.
    brownout = (
        observation.get("rhal_degraded") is True
        and fault_code is not _ABSENT
        and _strict_includes(codes, fault_code)
    )

    if brownout:
        state, reason = "critical", "brownout"
    elif not claims_pack:
        state, reason = "unmetered", "no_pack"
    elif no_reading:
        state, reason = "unmetered", "no_reading"
    elif remaining is None:
        # Two ways to arrive with no energy figure, and they send an operator to two different
        # places: a gauge whose reading cannot be believed, and a descriptor with no capacity.
        state = "unmetered"
        reason = "reading_unusable" if _charge_unusable(observation) else "no_capacity"
    elif budget["wh"] is None:
        state, reason = "unmetered", "no_reserve"
    else:
        state = _band(remaining, budget["wh"])
        reason = _band_reason(remaining, budget["wh"])

    # The faults, in POWER_FAULT_CODES order so a console renders them the same way every time.
    if claims_pack and no_reading:
        faults.append(
            {
                "code": "gauge_silent",
                "detail": (
                    "this body declares a pack and nothing on this robot read one. The state is "
                    "reported as unmetered rather than critical — §36.4 overrides the gauge on a "
                    "positive reading from the bus, and a missing reading is not one — and no new "
                    "task lease is granted until something answers for the pack (§36.1)."
                ),
            }
        )
    if (
        claims_pack
        and _field(observation, "energy_remaining_wh") is _ABSENT
        and _field(observation, "state_of_charge") is not _ABSENT
        and _field(profile, "pack_capacity_wh") is _ABSENT
    ):
        faults.append(
            {
                "code": "capacity_undeclared",
                "detail": (
                    "the gauge reports a fraction and this body declares no `pack_capacity_wh` to "
                    "turn it into watt-hours, so there is nothing to compare against the reserve. "
                    "Declare the capacity, or report `energy_remaining_wh` from the gauge (§36.2)."
                ),
            }
        )
    if claims_pack and budget["wh"] is None:
        faults.append(
            {
                "code": "reserve_undeclared",
                "detail": (
                    "this body declares a pack and no `safe_stop_energy_wh`, so §36.2's reserve "
                    "cannot be computed and the four states have no boundary. It is the figure "
                    "`power.safe_state.reachable` measures, and the one that decides when this "
                    "robot stops."
                ),
            }
        )
    if observation.get("rhal_degraded") is True and len(codes) == 0:
        faults.append(
            {
                "code": "brownout_undeclarable",
                "detail": (
                    "the RHAL is reporting `RHAL_DEGRADED` and this body declares no "
                    "`brownout_fault_codes`, so nothing here can tell a voltage sag from a dead "
                    "encoder. §24 makes a fault code vendor-specific and surfaced verbatim, which "
                    "is precisely why the vendor rather than the OS has to name the codes that "
                    "mean sag (§36.4)."
                ),
            }
        )
    charge = _field(observation, "state_of_charge")
    if charge is not _ABSENT and (not _is_finite(charge) or charge < 0 or charge > 1):
        faults.append(
            {
                "code": "charge_out_of_range",
                "detail": (
                    f"the gauge reports a state of charge of {_js_number(charge)}, and a state of "
                    "charge is a fraction between 0 and 1. The reading is discarded rather than "
                    "clamped: a clamped 1.4 is a full battery, and this one is a gauge nobody "
                    "should be trusting yet (§36.1)."
                ),
            }
        )
    throttled_for = _field(observation, "throttled_for_ms")
    throttled_for = 0 if throttled_for is _ABSENT or throttled_for is None else throttled_for
    if thermal == "throttling" and throttled_for >= SUSTAINED_THROTTLE_MS:
        faults.append(
            {
                "code": "thermal_sustained",
                "detail": (
                    f"this body has been throttling for "
                    f"{_js_number(math.floor(throttled_for / 1000))} s. §36.3 raises DEGRADED on "
                    f"sustained throttling, and {_js_number(THROTTLE_DEPTH)} of the five rungs are "
                    "given up — never the RHAL, the kernel or the control loop."
                ),
            }
        )
    if thermal == "critical":
        faults.append(
            {
                "code": "thermal_critical",
                "detail": (
                    "this body is at or above its declared critical temperature. Every rung of "
                    "§36.3's ladder is given up, inference included; the RHAL, the kernel and the "
                    "control loop are not, because there is no thermal situation in which giving "
                    "up the safety envelope is the right trade."
                ),
            }
        )
    if (
        temperature is not _ABSENT
        and _is_finite(temperature)
        and _field(profile, "thermal_throttle_c") is _ABSENT
        and _field(profile, "thermal_critical_c") is _ABSENT
    ):
        faults.append(
            {
                "code": "thermal_undeclared",
                "detail": (
                    "something is reading a temperature and this body declares neither "
                    "`thermal_throttle_c` nor `thermal_critical_c`, so nothing here knows what hot "
                    "is on this machine. The reading is reported and nothing is shed: 85 °C is "
                    "right for a CPU package and fifty degrees wrong for a harmonic drive (§36.3)."
                ),
            }
        )

    return {
        "state": state,
        "reason": reason,
        "reserve": budget,
        "remaining_wh": remaining,
        "thermal": thermal,
        "shed": shed(thermal),
        "faults": faults,
    }


# ── The wire shape ───────────────────────────────────────────────────────────


def power_to_json(case_input: dict[str, Any]) -> Any:
    """``power/*`` — the assessment, and every predicate a caller reads off it.

    The predicates are rendered rather than left to the caller because they are what §25.2, §27 and
    §36.1 each ask, and an edition could agree about the state while disagreeing about what it
    licenses.
    """
    profile = case_input.get("profile") or {}
    observation = case_input.get("observation") or {}
    assessment = assess(profile, observation)
    declared = _field(case_input, "declared_budget_ms")
    observed = _field(case_input, "observed_inference_ms")
    # The corpus writes an absent optional as null, because JSON has no `undefined`.
    declared = _ABSENT if declared is None else declared
    observed = _ABSENT if observed is None else observed
    return {
        "state": assessment["state"],
        "reason": assessment["reason"],
        "remaining_wh": assessment["remaining_wh"],
        "reserve": {
            "wh": assessment["reserve"]["wh"],
            "unaccounted": list(assessment["reserve"]["unaccounted"]),
            "complete": reserve_complete(assessment["reserve"]),
        },
        "thermal": assessment["thermal"],
        "shed": list(assessment["shed"]),
        "faults": [{"code": f["code"], "detail": f["detail"]} for f in assessment["faults"]],
        "degraded": degraded(assessment),
        "grants_new_leases": grants_new_leases(assessment),
        "requires_safe_state": state_requires_safe_state(assessment["state"]),
        "stops_immediately": state_stops_immediately(assessment["state"]),
        "budget_missed": budget_missed(declared, observed),
    }
