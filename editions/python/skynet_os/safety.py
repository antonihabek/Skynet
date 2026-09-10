"""The Safety Kernel contract, Python edition.

Transcribed from ``reference/typescript/safety.ts``. Every number, every message string and every
ordering decision is read from that file; nothing here is read from ``corpus/``, because an edition
generated from the corpus agrees with it by construction and proves nothing.

One invariant holds the whole operating system together:

    **The Safety Kernel is the only component with write access to an actuator.**

A policy does not command a joint. It writes a *desired* command into a buffer the kernel reads, and
the kernel decides what reaches the motors. That is what lets Skynet run untrusted policies at all:
most published checkpoints are pickles, which execute arbitrary code when they are read, and a
system where loading a model can move an arm is a system where every published model is a remote
code execution vulnerability attached to a two-metre humanoid.

── Five ways Python is quietly not JavaScript, in this module ────────────────

Each of these produces a *different verdict*, not a different message, and each is written down
where it is handled rather than here.

``narrowest``
    ``typeof v === 'number'`` is **false** for a boolean and ``isinstance(v, int)`` is **true**, so
    an envelope carrying ``joint_torque_max_nm: true`` would clamp every joint to 1 Nm in this
    edition and be ignored by the reference. :func:`_finite_number` rejects ``bool`` by name.

``undefined`` versus ``null``
    Three rules in this file test for ``undefined`` **strictly** — ``requested !== undefined`` in
    the widening check, ``self_collision_margin_m === undefined`` for the margin notice, and
    ``capability !== undefined`` at the mode gate. A key present and null takes the *other* branch
    in the reference, and ``dict.get`` collapses the two. :data:`_ABSENT` keeps them apart.

``Math.sign``
    Returns ``-0`` for ``-0`` and ``NaN`` for ``NaN``, neither of which ``copysign`` reproduces.

``Object.entries``
    JavaScript emits **array-index-like keys first, in ascending numeric order**, and only then the
    remaining keys in insertion order. Python preserves insertion order throughout. No channel in
    today's corpus is named ``"0"``, so this costs nothing today and reorders ``clamped`` the day a
    fuzz vector names one. :func:`_entries` reproduces the reference's order.

``null > number``
    The widening check reaches ``null > ceiling`` when a profile field is present and null, and
    JavaScript coerces that to ``0 > ceiling``. Transcribed rather than tidied: the reference is the
    specification, and a "sensible" reading of it here is a divergence.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .canonical import OMIT
from .kinematics import (
    adjacent,
    capsule_in_world,
    collision_bodies,
    distance,
    flatten,
    ignores,
    margin,
    segment_distance_squared,
    solve,
    statically_resolvable,
)

__all__ = [
    "ENFORCEMENT_MODES",
    "FAULT_RESPONSES",
    "GUARDS",
    "GUARD_SPECS",
    "OS_TIERS",
    "SAFETY_CLAIM_RULE",
    "TIER_PROFILES",
    "can_start",
    "evaluate_cycle",
    "limits_to_json",
    "resolve_enforcement",
    "resolve_limits",
    "verdict_to_json",
    "with_grants",
]


# ── The absence of a field ───────────────────────────────────────────────────

class _Absent:
    """JavaScript's ``undefined``, which ``dict.get`` cannot express.

    Three tests in this module distinguish *the key is not there* from *the key is there and null*,
    and they take opposite branches on the two. Python has one word for both.
    """

    def __repr__(self) -> str:  # pragma: no cover — diagnostics only
        return "ABSENT"

    def __bool__(self) -> bool:
        return False


_ABSENT: Final = _Absent()


def _field(obj: Any, key: str) -> Any:
    """``obj?.key`` — :data:`_ABSENT` when the object is missing or the key is not present.

    Optional chaining short-circuits on ``null`` as well as ``undefined`` and yields ``undefined``
    for both, which is why a null ``profile`` and an absent one behave identically here.
    """
    if not isinstance(obj, dict):
        return _ABSENT
    return obj[key] if key in obj else _ABSENT


def _or_none(value: Any) -> Any:
    """``value ?? null`` — nullish coalescing, which does **not** fire on ``0`` or ``""``."""
    return None if value is _ABSENT or value is None else value


# ── Deployment tiers ─────────────────────────────────────────────────────────

OS_TIERS: Final = ("guest", "host", "certified")

#: What the kernel does with a violation. ``advisory`` is not a weaker enforcement setting; it is
#: the absence of enforcement, recorded honestly.
ENFORCEMENT_MODES: Final = ("advisory", "enforcing", "enforcing_redundant")

TIER_PROFILES: Final[dict[str, dict[str, Any]]] = {
    "guest": {
        "tier": "guest",
        "summary": "Runs beside the robot's existing stack. Observes and advises; enforces nothing.",
        "substrate": (
            "A container or process on any compute that can reach the robot over IP, serial or CAN."
        ),
        "owns_control_loop": False,
        "enforcement": "advisory",
        "independent_stop_path": False,
        "supports_safety_case": False,
    },
    "host": {
        "tier": "host",
        "summary": "Skynet OS is the robot's onboard operating system and owns the control loop.",
        "substrate": (
            "PREEMPT_RT Linux on the robot's application processor, driving hardware through an "
            "RHAL driver."
        ),
        "owns_control_loop": True,
        "enforcement": "enforcing",
        "independent_stop_path": False,
        "supports_safety_case": False,
    },
    "certified": {
        "tier": "certified",
        "summary": "Host tier plus an independent safety processor with its own stop path.",
        "substrate": (
            "PREEMPT_RT Linux for the policy runtime; the kernel mirrored on a separate safety MCU "
            "wired into the drive-enable line."
        ),
        "owns_control_loop": True,
        "enforcement": "enforcing_redundant",
        "independent_stop_path": True,
        # The tier makes a safety case *possible*. It does not make one. See SAFETY_CLAIM_RULE.
        "supports_safety_case": True,
    },
}

#: The sentence the product is not allowed to shorten. A tier is an architecture, not a certificate.
SAFETY_CLAIM_RULE: Final = (
    "Skynet OS supplies evidence for a safety case. It does not certify a machine. Conformity "
    "assessment against ISO 10218, ISO 13482 or any other standard is performed by a notified "
    "body against a specific robot in a specific application."
)


def resolve_enforcement(tier: str) -> str:
    """Derived, never supplied. A caller cannot pass in the enforcement mode it would prefer."""
    return TIER_PROFILES[tier]["enforcement"]


# ── Guards ───────────────────────────────────────────────────────────────────

#: The guard set, in the order the kernel evaluates it each cycle. Order is part of the
#: specification: the watchdog runs first because a stale command must be discarded rather than
#: clamped and forwarded, and rate limiting runs after magnitude clamping because clamping a
#: magnitude can itself create a step change, which is what the rate limiter turns into a ramp.
GUARDS: Final = (
    "watchdog",
    "envelope",
    "rate_limit",
    "workspace",
    "self_collision",
    "balance",
    "proximity",
    "mode_gate",
)

#: Graduated responses, weakest to strongest, following IEC 60204-1. A kernel whose only response
#: is a hard stop is a kernel operators disable.
FAULT_RESPONSES: Final = (
    "clamp",  # Command reduced to the ceiling and forwarded. Logged.
    "hold",  # Last safe command held; the policy is given one cycle to recover.
    "stop_cat2",  # Controlled ramp to zero velocity, power retained, brakes engaged.
    "stop_cat1",  # Controlled ramp to zero, then drive power removed.
    "stop_cat0",  # Drive power removed immediately, uncontrolled.
)

GUARD_SPECS: Final[dict[str, dict[str, Any]]] = {
    "watchdog": {
        "guard": "watchdog",
        "checks": (
            "A command arrived within the envelope's watchdog window and its sequence number "
            "advanced."
        ),
        "response": "stop_cat2",
        "requires": ["envelope.watchdog_ms"],
    },
    "envelope": {
        "guard": "envelope",
        "checks": (
            "Per-joint position, velocity and torque against the declared rating and the transport "
            "clamp."
        ),
        "response": "clamp",
        "requires": ["embodiment.joints[].limits"],
    },
    "rate_limit": {
        "guard": "rate_limit",
        "checks": (
            "Change per cycle against the jerk ceiling — the guard that turns a policy glitch into "
            "a ramp."
        ),
        "response": "clamp",
        "requires": ["profile.max_delta_per_cycle"],
    },
    "workspace": {
        "guard": "workspace",
        "checks": "Commanded end-effector and base pose against operator-defined allowed volumes.",
        "response": "hold",
        "requires": ["profile.workspace", "embodiment.frames"],
    },
    "self_collision": {
        "guard": "self_collision",
        "checks": (
            "Commanded configuration against the body's own collision model, one step lookahead."
        ),
        "response": "hold",
        "requires": ["embodiment.frames[].collision"],
    },
    "balance": {
        "guard": "balance",
        "checks": (
            "Centre of pressure inside the support polygon; fall-arrest reflex takes the loop when "
            "it leaves."
        ),
        "response": "stop_cat2",
        "requires": ["sensing.imu", "embodiment.base=legged"],
    },
    "proximity": {
        "guard": "proximity",
        "checks": (
            "Speed and separation monitoring: commanded speed against the distance to the nearest "
            "detected person."
        ),
        "response": "stop_cat1",
        "requires": ["profile.proximity", "a sensor that detects people"],
    },
    "mode_gate": {
        "guard": "mode_gate",
        "checks": "That the command's capability is one the operator granted this package, in this mode.",
        "response": "hold",
        "requires": ["package.grants"],
    },
}


# ── JavaScript arithmetic and iteration, reproduced ──────────────────────────


def _finite_number(value: Any) -> bool:
    """``typeof v === 'number' && Number.isFinite(v)``.

    ``bool`` is excluded by name. In Python ``isinstance(True, int)`` is true, so an envelope
    declaring ``joint_torque_max_nm: true`` would otherwise reach :func:`min` as the number 1 and
    clamp every joint to 1 Nm — a limit the reference discards outright.
    """
    if isinstance(value, bool):
        return False
    return isinstance(value, (int, float)) and math.isfinite(value)


def _narrowest(*values: Any) -> float | None:
    """Narrowest of the values present. Absent inputs never widen the result."""
    present = [v for v in values if _finite_number(v)]
    return min(present) if present else None


def _sign(value: float) -> Any:
    """``Math.sign`` — which returns ``-0`` for ``-0`` and ``NaN`` for ``NaN``.

    Returning the input for all three of those cases is what ``math.copysign`` cannot do: it maps
    ``NaN`` onto ``±1`` and would turn an unrepresentable command into a confident clamp.
    """
    if value > 0:
        return 1
    if value < 0:
        return -1
    return value


def _entries(obj: dict[str, Any]) -> list[tuple[str, Any]]:
    """``Object.entries`` — array-index keys first in numeric order, then insertion order.

    Python has one ordering and JavaScript has two. Nothing in today's corpus names a channel
    ``"0"``, so this is identical to ``obj.items()`` on every case that exists; the day a fuzz
    vector generates a numeric channel name it stops being identical, and the difference lands in
    the order of the ``clamped`` array, which the corpus compares element by element.
    """
    index_like: list[str] = []
    rest: list[str] = []
    for key in obj:
        if (
            isinstance(key, str)
            and key.isascii()
            and key.isdigit()
            and (key == "0" or key[0] != "0")
            and int(key) < 2**32 - 1
        ):
            index_like.append(key)
        else:
            rest.append(key)
    index_like.sort(key=int)
    return [(key, obj[key]) for key in index_like + rest]


# ── The resolved limit set ───────────────────────────────────────────────────


def resolve_limits(params: dict[str, Any]) -> dict[str, Any]:
    """Compose every source of ceilings into the single set the kernel enforces.

    Three sources, and the rule between them is always the minimum: the robot's declared envelope,
    the transport plan's per-channel limits, and the operator's profile. Taking the minimum rather
    than a precedence order means no source can raise a limit another source lowered — including a
    malicious or mistaken operator profile, which is why the profile is folded in here rather than
    applied afterwards.
    """
    tier = params["tier"]
    envelope = params.get("envelope")
    transport = params.get("transport")
    profile = params.get("profile")
    session = params.get("session")
    enforcement = resolve_enforcement(tier)
    notices: list[dict[str, Any]] = []

    if enforcement == "advisory":
        notices.append(
            {
                "code": "advisory_only",
                "message": (
                    "At the guest tier Skynet OS does not own the control loop. Violations are "
                    "detected and recorded; commands still reach the robot unchanged. This is not "
                    "a safety function."
                ),
            }
        )

    if not TIER_PROFILES[tier]["independent_stop_path"]:
        notices.append(
            {
                "code": "no_independent_stop",
                "message": (
                    "There is no stop path independent of the application processor. If the "
                    "processor hangs, nothing here stops the robot."
                ),
            }
        )

    if envelope is None:
        notices.append(
            {
                "code": "no_envelope",
                "message": (
                    "No safety envelope has been declared for this robot. Only per-joint ratings "
                    "from the body description are available, and whole-body ceilings are "
                    "unenforced."
                ),
            }
        )
    elif not envelope.get("source"):
        # Falsy, not absent: an envelope citing the empty string has cited nothing. The `source`
        # field below still carries it, because `??` does not fire on `""` — the two disagree on
        # purpose, and the notice is the half an operator reads.
        notices.append(
            {
                "code": "unsourced_limits",
                "message": (
                    "The declared limits cite no datasheet. A safety limit without provenance is a "
                    "rumour."
                ),
            }
        )

    # Widening attempts are recorded, then discarded by the minimum. Silently ignoring one would
    # leave an operator believing a limit they set is in force.
    widened: list[str] = []

    def check_widen(label: str, requested: Any, ceiling: Any) -> None:
        if requested is _ABSENT or ceiling is None or ceiling is _ABSENT:
            return
        # `requested > ceiling` with `requested` null is `0 > ceiling` in the reference. Reproduced
        # rather than tidied: a "sensible" reading of the specification is a divergence from it.
        left = 0 if requested is None else requested
        if left > ceiling:
            widened.append(label)

    check_widen(
        "joint_velocity_max_rad_s",
        _field(profile, "joint_velocity_max_rad_s"),
        _field(envelope, "joint_velocity_max_rad_s"),
    )
    check_widen(
        "joint_torque_max_nm",
        _field(profile, "joint_torque_max_nm"),
        _field(envelope, "joint_torque_max_nm"),
    )
    check_widen(
        "base_velocity_max_m_s",
        _field(profile, "base_velocity_max_m_s"),
        _field(envelope, "base_velocity_max_m_s"),
    )
    if widened:
        name = _field(profile, "name")
        notices.append(
            {
                "code": "profile_widened",
                "message": (
                    f'The operator profile "{_js_string(name)}" asks to raise limits above the '
                    "declared envelope. Those values are ignored."
                ),
                "subjects": widened,
            }
        )

    # §38.2's session layer, checked against **what the layer above it allows** rather than against
    # the envelope alone — that is the section's own wording, and the layer above a session is
    # everything already composed. A session asking for 3 rad/s under a profile allowing 1 has not
    # widened the envelope; it has widened the number actually in force.
    #
    # Reported separately from `profile_widened`: two documents, two sentences, and telling somebody
    # their signed profile over-reached when it was their bench override sends them to the wrong file.
    session_widened: list[str] = []

    def check_session(label: str, requested: Any, ceiling: Any) -> None:
        if requested is _ABSENT or ceiling is None or ceiling is _ABSENT:
            return
        # The same null coercion `check_widen` reproduces, and for the same reason: the reference
        # compares with JavaScript's `>`, where `null > x` is `0 > x`. Tidying it here would be a
        # divergence dressed up as an improvement.
        left = 0 if requested is None else requested
        if left > ceiling:
            session_widened.append(label)

    if session is not None:
        check_session(
            "joint_velocity_max_rad_s",
            _field(session, "joint_velocity_max_rad_s"),
            _narrowest(
                _field(envelope, "joint_velocity_max_rad_s"),
                _field(transport, "joint_velocity_max_rad_s"),
                _field(profile, "joint_velocity_max_rad_s"),
            ),
        )
        check_session(
            "joint_torque_max_nm",
            _field(session, "joint_torque_max_nm"),
            _narrowest(
                _field(envelope, "joint_torque_max_nm"),
                _field(transport, "joint_torque_max_nm"),
                _field(profile, "joint_torque_max_nm"),
            ),
        )
        check_session(
            "base_velocity_max_m_s",
            _field(session, "base_velocity_max_m_s"),
            _narrowest(
                _field(envelope, "base_velocity_max_m_s"),
                _field(transport, "base_velocity_max_m_s"),
                _field(profile, "base_velocity_max_m_s"),
            ),
        )
        check_session(
            "max_delta_per_cycle",
            _field(session, "max_delta_per_cycle"),
            _or_none(_field(profile, "max_delta_per_cycle")),
        )
    if session_widened:
        notices.append(
            {
                "code": "session_widened",
                "message": (
                    "The session override asks to raise limits above what is already in force. "
                    "Those values are ignored; the narrower ceiling still applies."
                ),
                "subjects": session_widened,
            }
        )

    # The session joins the same minimum as every other layer, which is what makes "it only ever
    # narrows" a property of the arithmetic rather than a rule somebody has to remember.
    joint_velocity = _narrowest(
        _field(envelope, "joint_velocity_max_rad_s"),
        _field(transport, "joint_velocity_max_rad_s"),
        _field(profile, "joint_velocity_max_rad_s"),
        _field(session, "joint_velocity_max_rad_s"),
    )
    joint_torque = _narrowest(
        _field(envelope, "joint_torque_max_nm"),
        _field(transport, "joint_torque_max_nm"),
        _field(profile, "joint_torque_max_nm"),
        _field(session, "joint_torque_max_nm"),
    )
    base_velocity = _narrowest(
        _field(envelope, "base_velocity_max_m_s"),
        _field(transport, "base_velocity_max_m_s"),
        _field(profile, "base_velocity_max_m_s"),
        _field(session, "base_velocity_max_m_s"),
    )

    max_delta = _narrowest(
        _field(profile, "max_delta_per_cycle"),
        _field(session, "max_delta_per_cycle"),
    )
    if max_delta is None:
        notices.append(
            {
                "code": "no_rate_limit",
                "message": (
                    "No jerk ceiling is configured, so a single-cycle jump from zero to the "
                    "clamped maximum is permitted. Set max_delta_per_cycle before running a policy "
                    "near people."
                ),
            }
        )

    # §38.2's hazard, stated rather than left to somebody noticing a small number: "a 'maintenance
    # mode, half speed' that outlives the maintenance is a hazard that looks like a setting." A robot
    # under an override announces it, and names it so the sentence says what to clear.
    if session is not None:
        session_name = _field(session, "name")
        notices.append(
            {
                "code": "session_override",
                "message": (
                    f'A session override "{_js_string(session_name)}" is narrowing the limits in '
                    "force. It is a temporary setting rather than this robot's configuration — "
                    "clear it when the work is done."
                ),
                "subjects": [session_name],
            }
        )

    active, unavailable, inert = _partition_guards(params, max_delta)

    # Emitted only when the guard it is about will actually run. A robot with no collision geometry
    # is already being told the guard is unavailable, and adding "and its margin is unset" to that
    # is a second sentence about a thing that is not happening.
    if "self_collision" in active and _field(profile, "self_collision_margin_m") is _ABSENT:
        notices.append(
            {
                "code": "no_collision_margin",
                "message": (
                    "No self-collision margin is configured, so two of this robot's own bodies "
                    "must actually meet before the guard holds. Set self_collision_margin_m to "
                    "stop the command that would bring them together."
                ),
                "guard": "self_collision",
            }
        )

    for entry in unavailable:
        notices.append(
            {
                "code": "guard_unavailable",
                "message": f"The {entry['guard']} guard cannot run on this robot.",
                "subjects": entry["missing"],
            }
        )

    return {
        "tier": tier,
        "enforcement": enforcement,
        "joint_velocity_max_rad_s": joint_velocity,
        "joint_torque_max_nm": joint_torque,
        "base_velocity_max_m_s": base_velocity,
        "min_control_hz": _or_none(_field(transport, "min_control_hz")),
        "max_delta_per_cycle": max_delta,
        "per_channel": _or_default(_field(transport, "per_channel"), {}),
        "proximity": _or_none(_field(profile, "proximity")),
        "frames": _or_none(params.get("frames", _ABSENT)),
        "workspace": _or_default(_field(profile, "workspace"), []),
        "self_collision_margin_m": _or_none(_field(profile, "self_collision_margin_m")),
        "grants": [],
        "active_guards": active,
        "unavailable_guards": unavailable,
        "inert_guards": inert,
        "source": _coalesce(_field(envelope, "source"), _field(transport, "source")),
        "notices": notices,
    }


def _js_string(value: Any) -> str:
    """How a template literal renders a value. ``undefined`` is the six characters, not the empty
    string — a profile with no name reads ``The operator profile "undefined" ...`` in the reference,
    and an edition that printed ``""`` there would produce a different message for the same input.
    """
    if value is _ABSENT:
        return "undefined"
    if value is None:
        return "null"
    return str(value)


def _or_default(value: Any, default: Any) -> Any:
    return default if value is _ABSENT or value is None else value


def _coalesce(*values: Any) -> Any:
    """``a ?? b ?? null`` — the first value that is neither absent nor null, and ``""`` is one."""
    for value in values:
        if value is not _ABSENT and value is not None:
            return value
    return None


def _partition_guards(
    params: dict[str, Any], max_delta: float | None
) -> tuple[list[str], list[dict[str, Any]]]:
    active: list[str] = []
    unavailable: list[dict[str, Any]] = []
    inert: list[dict[str, Any]] = []
    envelope = params.get("envelope")
    profile = params.get("profile")
    available = params.get("available") or {}
    frames = _or_none(params.get("frames", _ABSENT))

    def missing_for(guard: str) -> list[str]:
        if guard == "watchdog":
            # Truthy, not present: a watchdog window of zero is no window at all.
            return [] if _or_default(_field(envelope, "watchdog_ms"), None) else ["envelope.watchdog_ms"]
        if guard == "envelope":
            return []
        if guard == "rate_limit":
            return ["profile.max_delta_per_cycle"] if max_delta is None else []
        # §44.5: a body whose descriptor declares no frame graph, or whose graph does not reach the
        # frame the operator constrained, reports these two unavailable **with the missing frame
        # named** — rather than evaluating them against a frame it guessed.
        if guard == "workspace":
            missing: list[str] = []
            layers = _or_default(_field(profile, "workspace"), [])
            if sum(len(layer["volumes"]) for layer in layers) == 0:
                missing.append("profile.workspace")
            if not frames:
                missing.append("embodiment.frames")
            else:
                for layer in layers:
                    for volume in layer["volumes"]:
                        index = next(
                            (i for i, frame in enumerate(frames) if frame["name"] == volume["frame"]),
                            -1,
                        )
                        if (index < 0 or not statically_resolvable(frames, index)) and volume[
                            "frame"
                        ] not in missing:
                            missing.append(volume["frame"])
            return missing
        if guard == "self_collision":
            if not frames:
                return ["embodiment.frames"]
            # Two, not one. A single collision body has nothing to collide with, and a guard
            # reported active with no pair to test is a guard that passes by not looking.
            return (
                ["embodiment.frames[].collision, on two or more frames"]
                if collision_bodies(frames) < 2
                else []
            )
        if guard == "balance":
            # Not applicable rather than missing on a wheeled base; reported as inactive either way,
            # and the missing list says which, so nobody reads "balance guard off" as a defect.
            if not available.get("legged"):
                return ["a legged base — not applicable to this body"]
            return [] if available.get("imu") else ["an IMU"]
        if guard == "proximity":
            if not _or_default(_field(profile, "proximity"), None):
                return ["profile.proximity"]
            return [] if available.get("people_detection") else ["a sensor that detects people"]
        if guard == "mode_gate":
            return []
        raise AssertionError(f"unknown guard {guard!r}")

    def reading_for(guard: str) -> tuple[str, bool] | None:
        """Which per-cycle reading a guard acts on, where that reading can be absent (§80).

        Only ``proximity`` today: the other seven act on the command, the previous command, the
        clock or the frame graph, all of which the cycle always carries.
        """
        if guard != "proximity":
            return None
        return ("cycle.nearest_person_m", available.get("proximity_reading") is True)

    for guard in GUARDS:
        missing = missing_for(guard)
        if missing:
            unavailable.append({"guard": guard, "missing": missing})
            continue
        # Checked **after** ``missing``, and the order is the point: a guard with no sensor is
        # unavailable, not inert. Reporting a robot that has no person detector as "the reading is
        # not coming" would describe a missing producer on a machine whose problem is a sensor.
        reading = reading_for(guard)
        if reading is not None and not reading[1]:
            inert.append({"guard": guard, "reading": reading[0]})
            continue
        active.append(guard)

    return active, unavailable, inert


# ── The per-cycle decision ───────────────────────────────────────────────────

MAX_POSE_TRIPS: Final = 8

#: Graduated responses, weakest to strongest, following IEC 60204-1.
RESPONSE_ORDER: Final = ["clamp", "hold", "stop_cat2", "stop_cat1", "stop_cat0"]


def _stronger(current: str | None, response: str) -> str:
    if current is None:
        return response
    return current if RESPONSE_ORDER.index(current) >= RESPONSE_ORDER.index(response) else response


def _empty_pose() -> dict[str, Any]:
    return {
        "evaluated": False,
        "lookahead": False,
        "truncated": False,
        "unresolved": [],
        "trips": [],
    }


def with_grants(limits: dict[str, Any], grants: list[str]) -> dict[str, Any]:
    """Attach the capability grants the mode gate checks against."""
    return {**limits, "grants": grants}


def evaluate_cycle(
    cycle: dict[str, Any],
    limits: dict[str, Any],
    envelope: dict[str, Any] | None,
    last_sequence: Any = _ABSENT,
    bindings: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    """Evaluate one control cycle.

    The reference implementation of the hot path. It runs in time linear in the number of channels
    and quadratic in the number of collision bodies; the Rust on the robot allocates nothing and
    runs the same arithmetic against a preallocated frame, and the conformance corpus feeds
    identical cycle vectors to every edition and requires identical verdicts.
    """
    if bindings is None:
        bindings = []
    tripped: list[str] = []
    clamped: list[dict[str, Any]] = []
    advisory = limits["enforcement"] == "advisory"

    # The strongest response any guard *demanded this cycle* — not the strongest response any
    # tripped guard is capable of demanding, which is a different and weaker thing. The difference
    # is `proximity`, whose §7.1 row reads `stop_cat1` because that is what it does inside the stop
    # distance; between the two distances it scales the command and lets it through, and a verdict
    # deriving its response from the guard's row would say "stopped" about a robot that was not.
    demanded: str | None = None

    desired = cycle.get("desired") or {}

    # 1. Watchdog. A stale command is discarded, never clamped and forwarded — clamping it would
    #    keep a dead producer's last intention alive at a legal magnitude, which is the failure mode
    #    the watchdog exists to prevent.
    watchdog_ms = _field(envelope, "watchdog_ms")
    stale = watchdog_ms is not _ABSENT and cycle["now_ms"] - cycle["command_ms"] > watchdog_ms
    stalled = last_sequence is not _ABSENT and cycle["sequence"] <= last_sequence
    if "watchdog" in limits["active_guards"] and (stale or stalled):
        tripped.append("watchdog")
        return {
            "command": dict(desired) if advisory else {},
            "response": "stop_cat2",
            "tripped": tripped,
            "clamped": clamped,
            "advisory": advisory,
            "pose": _empty_pose(),
        }

    command: dict[str, Any] = {}

    # 2. Magnitude clamping against the resolved ceilings.
    for channel, value in _entries(desired):
        ceiling = _ceiling_for(channel, limits)
        next_value = value
        if ceiling is not None and abs(value) > ceiling:
            next_value = _sign(value) * ceiling
            if "envelope" not in tripped:
                tripped.append("envelope")
            demanded = _stronger(demanded, "clamp")
            clamped.append({"channel": channel, "from": value, "to": next_value})
        command[channel] = next_value

    # 3. Rate limiting, after clamping, because clamping creates step changes of its own.
    if limits["max_delta_per_cycle"] is not None and cycle.get("previous"):
        for channel, value in _entries(command):
            previous = _field(cycle.get("previous"), channel)
            if previous is _ABSENT:
                continue
            ceiling = _ceiling_for(channel, limits)
            max_step = (
                abs(previous) * limits["max_delta_per_cycle"]
                if ceiling is None
                else ceiling * limits["max_delta_per_cycle"]
            )
            delta = value - previous
            if abs(delta) > max_step and max_step > 0:
                next_value = previous + _sign(delta) * max_step
                if "rate_limit" not in tripped:
                    tripped.append("rate_limit")
                demanded = _stronger(demanded, "clamp")
                clamped.append({"channel": channel, "from": value, "to": next_value})
                command[channel] = next_value

    # 4 and 5. The pose guards, on the configuration the command above would produce.
    #
    #    After clamping and rate limiting, and that ordering is the point: the pose that matters is
    #    the one the robot will actually be commanded into, not the one the policy asked for and the
    #    envelope refused.
    pose = _empty_pose()
    wants_pose = (
        "workspace" in limits["active_guards"] or "self_collision" in limits["active_guards"]
    )
    if wants_pose and limits["frames"]:
        frames = limits["frames"]
        solved = solve(frames, bindings, command)
        pose["evaluated"] = True
        pose["lookahead"] = solved["lookahead"]
        pose["unresolved"] = [frames[index]["name"] for index in solved["unresolved"]]
        if "workspace" in limits["active_guards"]:
            _check_workspace(frames, solved["poses"], limits["workspace"], pose)
        if "self_collision" in limits["active_guards"]:
            margin_m = limits["self_collision_margin_m"]
            _check_self_collision(
                frames, solved["poses"], 0 if margin_m is None else margin_m, pose
            )
        if len(pose["trips"]) > MAX_POSE_TRIPS:
            pose["trips"] = pose["trips"][:MAX_POSE_TRIPS]
            pose["truncated"] = True
        for trip in pose["trips"]:
            if trip["guard"] not in tripped:
                tripped.append(trip["guard"])
            demanded = _stronger(demanded, "hold")

    # 7. Proximity — speed and separation monitoring, when something is measuring separation.
    #
    #    After the magnitude and rate guards because it scales the command those two produced:
    #    applying it first would let a clamp raise a speed the separation had already reduced, and
    #    the composition rule everywhere in this system is that a later stage may only narrow.
    if "proximity" in limits["active_guards"]:
        distance_m = _field(cycle, "nearest_person_m")
        prox = limits["proximity"]
        if distance_m is not _ABSENT and prox:
            if distance_m <= prox["stop_distance_m"]:
                if "proximity" not in tripped:
                    tripped.append("proximity")
                return {
                    "command": dict(desired) if advisory else {},
                    "response": "stop_cat1",
                    "tripped": tripped,
                    "clamped": clamped,
                    "advisory": advisory,
                    "pose": pose,
                }
            if distance_m <= prox["slow_distance_m"] and prox["slow_factor"] < 1:
                if "proximity" not in tripped:
                    tripped.append("proximity")
                # A reduction, not a stop.
                demanded = _stronger(demanded, "clamp")
                for channel, value in _entries(command):
                    next_value = value * prox["slow_factor"]
                    if next_value != value:
                        clamped.append({"channel": channel, "from": value, "to": next_value})
                        command[channel] = next_value

    # 8. Mode gate — the capability this command exercises must be one the operator granted.
    #
    #    Enforced at every cycle rather than at start, so a revocation takes effect on the next
    #    command rather than on the next restart (§8.2).
    if "mode_gate" in limits["active_guards"] and _field(cycle, "capability") is not _ABSENT:
        if cycle["capability"] not in limits["grants"]:
            if "mode_gate" not in tripped:
                tripped.append("mode_gate")
            demanded = _stronger(demanded, "hold")

    # A hold means the command does not go out. The rule lives here, once, rather than at each
    # guard that demands one: `hold` is the last safe command being held while the policy is given
    # a cycle to recover, and forwarding a clamped command alongside a `hold` verdict would be the
    # opposite of what it reported.
    withheld = demanded is not None and RESPONSE_ORDER.index(demanded) >= RESPONSE_ORDER.index(
        "hold"
    )

    return {
        # In advisory mode the desired command is what reaches the robot. Returning the clamped
        # values here would make a guest-tier deployment look enforced in its own logs.
        "command": dict(desired) if advisory else ({} if withheld else command),
        "response": demanded,
        "tripped": tripped,
        "clamped": clamped,
        "advisory": advisory,
        "pose": pose,
    }


def _check_workspace(
    frames: list[dict[str, Any]],
    poses: list[dict[str, Any] | None],
    layers: list[dict[str, Any]],
    pose: dict[str, Any],
) -> None:
    """§7.1's fourth guard: every constrained frame inside its operator-defined volumes."""
    for index, frame in enumerate(frames):
        # Layers compose by conjunction: the frame must satisfy every layer that names it, and the
        # reported margin is the worst of them. A layer that does not name this frame says nothing
        # about it, so a site rule about one arm does not confine the other.
        constrained = False
        worst: float | None = None
        for layer in layers:
            best: float | None = None
            for volume in layer["volumes"]:
                if volume["frame"] != frame["name"]:
                    continue
                constrained = True
                solved = poses[index]
                if solved:
                    value = margin(volume["shape"], solved["translation"])
                    best = value if best is None or value > best else best
            if best is not None:
                worst = best if worst is None or best < worst else worst

        if not constrained:
            continue
        solved = poses[index]
        if not solved:
            # Constrained and unlocatable. Not a pass: the guard has not shown the command safe.
            pose["trips"].append(
                {"guard": "workspace", "frame": frame["name"], "other": None, "margin_m": None}
            )
            continue
        if worst is not None and worst < 0:
            pose["trips"].append(
                {"guard": "workspace", "frame": frame["name"], "other": None, "margin_m": worst}
            )


def _check_self_collision(
    frames: list[dict[str, Any]],
    poses: list[dict[str, Any] | None],
    margin_m: float,
    pose: dict[str, Any],
) -> None:
    """§7.1's fifth guard: no two collision bodies closer than the operator's margin.

    Quadratic, and the pairs are visited in index order so the reported list is the same on every
    edition. Adjacent frames and declared exceptions are skipped before any arithmetic — a
    humanoid's torso and pelvis overlap by design, and a guard that tripped on the robot standing
    still is a guard an operator switches off.
    """
    clearance = margin_m if margin_m > 0 else 0
    for i in range(len(frames)):
        first = frames[i].get("collision")
        if not first:
            continue
        for j in range(i + 1, len(frames)):
            second = frames[j].get("collision")
            if not second:
                continue
            if adjacent(frames, i, j) or ignores(frames, i, j):
                continue

            pose_i = poses[i]
            pose_j = poses[j]
            if not pose_i or not pose_j:
                # One of the pair has no pose. Reported against the pair rather than silently
                # skipped — a collision test that quietly excludes a limb is a test that passes
                # because it did not look.
                pose["trips"].append(
                    {
                        "guard": "self_collision",
                        "frame": frames[i]["name"],
                        "other": frames[j]["name"],
                        "margin_m": None,
                    }
                )
                continue

            a1, b1 = capsule_in_world(pose_i, first)
            a2, b2 = capsule_in_world(pose_j, second)
            squared = segment_distance_squared(a1, b1, a2, b2)
            threshold = first["radius_m"] + second["radius_m"] + clearance
            # The decision is taken in squared distance, so no square root can move a verdict. The
            # root is taken only for the number the report carries.
            if squared < threshold * threshold:
                pose["trips"].append(
                    {
                        "guard": "self_collision",
                        "frame": frames[i]["name"],
                        "other": frames[j]["name"],
                        "margin_m": distance(squared)
                        - first["radius_m"]
                        - second["radius_m"]
                        - clearance,
                    }
                )


def _ceiling_for(channel: str, limits: dict[str, Any]) -> float | None:
    per_channel = _field(limits["per_channel"], channel)
    if per_channel is not _ABSENT:
        return per_channel
    if channel == "base":
        return limits["base_velocity_max_m_s"]
    return None


def can_start(plan: dict[str, Any], limits: dict[str, Any]) -> dict[str, Any]:
    """Whether a transport plan and a limit set are safe enough to start.

    The kernel refuses two things and only two: a plan that could not be resolved, and enforcement
    that was promised and cannot be delivered. Everything else — a missing guard, an unsourced
    limit, a degraded observation — is a notice an operator reads and accepts, because a system
    that refuses to start on every imperfection is a system that gets bypassed.
    """
    if plan["verdict"] == "refused":
        return {"ok": False, "reason": "The policy cannot be transported onto this body."}
    if limits["tier"] != "guest" and limits["enforcement"] == "advisory":
        return {
            "ok": False,
            "reason": "This tier promises enforcement and the kernel resolved to advisory.",
        }
    return {"ok": True}


# ── The wire shapes ──────────────────────────────────────────────────────────


def _nullable(value: Any) -> Any:
    """``value ?? null``. Present and null rather than absent: "there is no value" is a fact."""
    return None if value is _ABSENT or value is None else value


def _shape_limits(limits: dict[str, Any]) -> Any:
    """``shapeLimits``. A projection, not the whole resolved set.

    ``frames``, ``workspace``, ``proximity`` and ``grants`` are inputs the kernel carries forward,
    not findings, and the corpus pins what an operator is shown.
    """
    return {
        "tier": limits["tier"],
        "enforcement": limits["enforcement"],
        "joint_velocity_max_rad_s": _nullable(limits["joint_velocity_max_rad_s"]),
        "joint_torque_max_nm": _nullable(limits["joint_torque_max_nm"]),
        "base_velocity_max_m_s": _nullable(limits["base_velocity_max_m_s"]),
        "min_control_hz": _nullable(limits["min_control_hz"]),
        "max_delta_per_cycle": _nullable(limits["max_delta_per_cycle"]),
        # §7.1's fifth guard holds at this distance. Reported because an operator who cannot see
        # the margin in force cannot tell a robot holding 5 cm from one holding at contact.
        "self_collision_margin_m": _nullable(limits["self_collision_margin_m"]),
        "per_channel": limits["per_channel"],
        "active_guards": limits["active_guards"],
        "unavailable_guards": [
            {"guard": entry["guard"], "missing": entry["missing"]}
            for entry in limits["unavailable_guards"]
        ],
        "inert_guards": [
            {"guard": entry["guard"], "reading": entry["reading"]}
            for entry in limits["inert_guards"]
        ],
        "source": _nullable(limits["source"]),
        "notices": [
            {
                "code": notice["code"],
                "message": notice["message"],
                "subjects": notice["subjects"] if notice.get("subjects") is not None else OMIT,
            }
            for notice in limits["notices"]
        ],
    }


def _shape_pose(pose: dict[str, Any]) -> Any:
    return {
        "evaluated": pose["evaluated"],
        "lookahead": pose["lookahead"],
        "truncated": pose["truncated"],
        "unresolved": pose["unresolved"],
        "trips": [
            {
                "guard": trip["guard"],
                "frame": trip["frame"],
                # Present and null on a workspace excursion rather than absent: "this finding is
                # about one frame" is a fact, and a reader of an absent key cannot tell it from a
                # producer that forgot.
                "other": _nullable(trip["other"]),
                "margin_m": _nullable(trip["margin_m"]),
            }
            for trip in pose["trips"]
        ],
    }


def _shape_verdict(verdict: dict[str, Any]) -> Any:
    return {
        "command": verdict["command"],
        "response": _nullable(verdict["response"]),
        "tripped": verdict["tripped"],
        "clamped": [
            {"channel": entry["channel"], "from": entry["from"], "to": entry["to"]}
            for entry in verdict["clamped"]
        ],
        "advisory": verdict["advisory"],
        "pose": _shape_pose(verdict["pose"]),
    }


def _flatten_or_throw(graph: dict[str, Any]) -> list[dict[str, Any]]:
    flattened = flatten(graph)
    if "fault" in flattened:
        raise ValueError(f"the graph does not flatten: {flattened['fault']['code']}")
    return flattened["frames"]


def _params_from(case_limits: dict[str, Any]) -> dict[str, Any]:
    """Read `ResolveLimitsParams` off a corpus case, flattening the frame graph if it carries one.

    The case carries the descriptor's **graph**; `resolveLimits` takes the **flattened** frame list
    (§44.3), and the generator flattens it on the way in. An edition that passed the graph straight
    through would report both pose guards unavailable on a body that declares frames.
    """
    params = {key: value for key, value in case_limits.items() if key != "grants"}
    if "frames" in params and params["frames"] is not None:
        params["frames"] = _flatten_or_throw(params["frames"])
    return params


def limits_to_json(case_input: dict[str, Any]) -> Any:
    """``limits/resolution`` — the resolved set an operator is shown before anything runs."""
    return _shape_limits(resolve_limits(_params_from(case_input)))


def verdict_to_json(case_input: dict[str, Any]) -> Any:
    """``cycle/*`` and ``fuzz/cycle`` — one control cycle's verdict."""
    case_limits = case_input["limits"]
    params = _params_from(case_limits)
    limits = with_grants(resolve_limits(params), case_limits.get("grants") or [])
    last_sequence = case_input.get("last_sequence")
    return _shape_verdict(
        evaluate_cycle(
            case_input["cycle"],
            limits,
            params.get("envelope"),
            # The corpus writes an absent `lastSequence` as null, because JSON has no `undefined`.
            _ABSENT if last_sequence is None else last_sequence,
            case_input.get("joints") or [],
        )
    )
