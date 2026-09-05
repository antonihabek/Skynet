"""Observation assembly (§25.1), Python edition.

Transcribed from ``reference/typescript/observation.ts``. Every rule and every message string is
read from that file, never from ``corpus/``.

§25.1 calls this "one of the harder parts of the whole system", and the reason is one sentence:

    A policy is trained on synchronised frames: image and joint state as of one instant. A real
    robot has three cameras at 30 Hz with 20-60 ms of pipeline latency, joint state at 1 kHz with
    1 ms, and an IMU at 200 Hz — none of them aligned. Feed a policy the most recent value from
    each and it receives **a frame that never existed**, in which the arm is where it is *now* and
    the image shows where it was two frames ago.

The last clause is the expensive part: a skewed frame does not announce itself. It presents as a
policy that grasps slightly late, or oscillates on contact, or works on the bench and not on the
robot — every one of which sends somebody to retrain a model that was fine. This module does not
synchronise anything, and §25.1 is explicit that it should not try. It makes the skew **measured**.

── Where Python is quietly not JavaScript, here ──────────────────────────────

``round``
    Python's built-in rounds **half to even** and the reference rounds **half away from zero**;
    neither is ECMAScript's ``Math.round``, which rounds half toward +∞. Three different answers for
    ``-0.5``. :func:`_round_half_away` is transcribed rather than reached for.

``math.trunc``
    Returns an arbitrary-precision ``int`` where ``Math.trunc`` returns a double. Converted back
    immediately, so nothing downstream is handed an integer Python keeps exactly and a double could
    not — the same trap ``kinematics.py`` documents in its range reduction.

``str(number)``
    A hold's detail interpolates the age and the budget with a template literal, which is
    ``String(n)``: ``String(13)`` is ``"13"`` and ``str(13.0)`` is ``"13.0"``. Those are different
    bytes in a message the corpus compares byte for byte. :func:`_js_number_string` reproduces
    ECMAScript's ``Number::toString``, including its switch to exponential form outside
    ``1e-7 … 1e21`` — which the canonical JSON writer deliberately does *not* do, so
    ``canonical.format_number`` is the wrong tool for this one job.

── A divergence in the reference itself ──────────────────────────────────────

``roundHalfAway`` is defined **twice** in the reference — here and in ``kinematics.ts`` — with one
name and one stated contract, and the two disagree:

======================  ==================  ====================
input                   ``kinematics.ts``   ``observation.ts``
======================  ==================  ====================
``0.49999999999999994``  1                  0
``-0.49999999999999994`` -1                 0
``NaN``, ``±Infinity``   ``NaN``, ``±Inf``  0
======================  ==================  ====================

``kinematics.ts`` computes ``Math.trunc(v ± 0.5)``, and at the largest double below a half that sum
rounds up to exactly ``1.0``. This edition transcribes **this file's** version, because a
transcription that quietly unified them would be an edition disagreeing with the reference in order
to be right. Neither input is reachable from a real latency, and the two rounders are still one
function with two behaviours, which is the shape of defect §7.4's redundant channel exists to catch.
"""

from __future__ import annotations

import math
from decimal import Decimal
from typing import Any, Final

__all__ = [
    "assemble_observation",
    "frame_to_json",
    "latest_common_instant",
    "target_to_json",
    "to_record",
]

NS_PER_MS: Final = 1_000_000

#: Seventy-one minutes of skew. Not a real case; it is here because the Rust edition's alternative
#: in a release build is a wrap, and a frame recorded as four microseconds skewed when it was four
#: thousand seconds skewed is worse than one recorded at the ceiling.
_U32_MAX: Final = 4_294_967_295


# ── ECMAScript arithmetic and rendering ──────────────────────────────────────


def _round_half_away(value: float) -> float:
    """Round half away from zero — **not** ``Math.round``, which sends ``-0.5`` to ``-0``.

    The Rust edition cannot use ``f64::round`` either: it is unavailable under ``no_std`` without a
    maths library, and §31.2's rule about platform maths applies with full force to a rounding
    inside a latency correction. Two editions rounding a half-nanosecond differently is a
    one-nanosecond disagreement today and a *different chosen sample* the day two candidates are
    equidistant, so every edition writes the same four lines out.
    """
    if not _is_finite(value):
        return 0
    # `math.trunc` hands back an arbitrary-precision int; the reference's returns a double. Converted
    # back before the subtraction, so the fraction is computed the way the reference computes it.
    truncated = float(math.trunc(value))
    fraction = value - truncated
    if fraction >= 0.5:
        return truncated + 1
    if fraction <= -0.5:
        return truncated - 1
    return truncated


def _is_finite(value: Any) -> bool:
    """``Number.isFinite`` — false for a boolean, which ``math.isfinite`` accepts as 0 or 1."""
    if isinstance(value, bool):
        return False
    return isinstance(value, (int, float)) and math.isfinite(value)


def _js_number_string(value: float) -> str:
    """``String(n)`` — ECMAScript's ``Number::toString``, for a template literal.

    Positional between ``1e-7`` and ``1e21`` and exponential outside it, which is where this differs
    from the canonical JSON writer: canonical form spells ``1e21`` out in full, and a template
    literal does not. Only the message strings need this; every number that reaches the wire goes
    through ``canonical``.
    """
    if isinstance(value, bool):  # `String(true)` is not a number's rendering
        return "true" if value else "false"
    if isinstance(value, float) and value != value:
        return "NaN"
    if value == math.inf:
        return "Infinity"
    if value == -math.inf:
        return "-Infinity"
    if value == 0:
        return "0"  # covers -0.0, which ECMAScript renders as "0"

    # `repr` of a float is the shortest string that round-trips, which is the digit sequence the
    # specification asks for. An int is exact already.
    decimal = Decimal(repr(float(value)) if isinstance(value, float) else str(value))
    sign, digits, exponent = decimal.as_tuple()
    digit_list = list(digits)
    while len(digit_list) > 1 and digit_list[-1] == 0:
        digit_list.pop()
        exponent += 1  # type: ignore[operator]

    k = len(digit_list)
    n = k + int(exponent)
    body = "".join(str(d) for d in digit_list)
    minus = "-" if sign else ""

    if k <= n <= 21:
        return minus + body + "0" * (n - k)
    if 0 < n <= 21:
        return minus + body[:n] + "." + body[n:]
    if -6 < n <= 0:
        return minus + "0." + "0" * (-n) + body
    # Exponential. The exponent the specification prints is `n - 1`, and a positive one carries a
    # `+` while a negative one carries only its own sign.
    power = n - 1
    mantissa = body if k == 1 else body[0] + "." + body[1:]
    return f"{minus}{mantissa}e{'+' if power > 0 else '-'}{abs(power)}"


def _ms(nanos: float) -> float:
    """Nanoseconds as milliseconds. One division of an exact integer, so every edition performing
    it gets the same bits: IEEE-754 division is correctly rounded everywhere.
    """
    return nanos / NS_PER_MS


def _micros(milliseconds: float) -> int:
    """Milliseconds as whole microseconds, saturating at zero and at the top of a ``u32``."""
    value = _round_half_away(milliseconds * 1000)
    if value <= 0:
        return 0
    if value >= _U32_MAX:
        return _U32_MAX
    return int(value)


def _effective_ns(stamp_ns: float, latency_ms: float | None) -> float:
    """The instant a sample describes: its stamp, less the declared latency, in nanoseconds."""
    latency_ns = 0 if latency_ms is None else _round_half_away(latency_ms * NS_PER_MS)
    return stamp_ns - latency_ns


def _nullish(value: Any) -> Any:
    """``value ?? null``. Absent and null are the same thing to every rule in this module."""
    return None if value is None else value


# ── Assembly ─────────────────────────────────────────────────────────────────


def assemble_observation(target_ns: float, streams: list[dict[str, Any]]) -> dict[str, Any]:
    """Assemble one observation frame for the given instant (§25.1).

    For each stream: correct every sample for the declared latency, pick the one nearest the target,
    and measure how far behind the target it is. Then measure the spread across the streams, which
    is the frame's own internal inconsistency.

    Nearest is by absolute distance, so a sample *after* the target can be chosen when it is
    genuinely closer — impossible on a live robot, correct in replay. **A tie takes the earlier
    sample**: two samples equidistant either side of the target are equally good by the distance
    rule and are not equally good in any other sense, since the earlier describes something that had
    already happened at the instant the frame claims to describe. The tie-break also has to exist
    for a duller reason — without it, two editions iterating in a different order would pick
    different samples and the corpus would diverge on an input where nothing is wrong.
    """
    frames: list[dict[str, Any]] = []
    holds: list[dict[str, Any]] = []
    undeclared_latency: list[str] = []
    undeclared_budget: list[str] = []

    for stream in streams:
        latency = _nullish(stream.get("latency_ms"))
        uncertainty = _nullish(stream.get("uncertainty_ms"))
        budget = _nullish(stream.get("budget_ms"))
        if latency is None:
            undeclared_latency.append(stream["name"])
        if budget is None:
            undeclared_budget.append(stream["name"])

        best: dict[str, Any] | None = None
        for sample in stream["samples"]:
            effective = _effective_ns(sample["stamp_ns"], latency)
            gap = abs(effective - target_ns)
            if best is None:
                better = True
            else:
                best_gap = abs(best["effective"] - target_ns)
                # Strictly nearer wins; on a tie the earlier instant wins. Never "the one seen
                # first", which would make the answer depend on the order the capture ring happened
                # to be walked.
                better = gap < best_gap or (gap == best_gap and effective < best["effective"])
            if better:
                best = {"effective": effective, "sample": sample}

        chosen: dict[str, Any] | None = None
        if best is None:
            holds.append(
                {
                    "stream": stream["name"],
                    "reason": "no_sample",
                    "detail": (
                        f"{stream['name']} ({stream['modality']}) offered no sample for this "
                        "instant, so there is no frame to assemble. A stream the policy declared "
                        "and the pipeline is not producing is a capture path that is not running."
                    ),
                }
            )
        else:
            age = _ms(target_ns - best["effective"])
            # One-sided, and deliberately so. The budget bounds how far *behind* the target a sample
            # may be; a sample from ahead of it is a clock fault rather than a stale one, it is
            # visible in the signed age, and reporting it as staleness would send somebody to look
            # at a capture pipeline that is working.
            over_budget = budget is not None and age > budget
            if over_budget:
                holds.append(
                    {
                        "stream": stream["name"],
                        "reason": "stale",
                        "detail": (
                            f"{stream['name']} ({stream['modality']}) is "
                            f"{_js_number_string(age)} ms behind the frame's instant and the "
                            f"policy's budget for it is "
                            f"{_js_number_string(0 if budget is None else budget)} ms. §25.1: "
                            "exceeding it is a hold, not a silently older frame."
                        ),
                    }
                )
            chosen = {
                "stamp_ns": best["sample"]["stamp_ns"],
                "seq": best["sample"]["seq"],
                "effective_ns": best["effective"],
                "age_ms": age,
                "over_budget": over_budget,
                "considered": len(stream["samples"]),
            }

        frames.append(
            {
                "name": stream["name"],
                "modality": stream["modality"],
                "latency_ms": latency,
                "uncertainty_ms": uncertainty,
                "budget_ms": budget,
                "chosen": chosen,
            }
        )

    skew, upper = _measure_skew(frames, undeclared_latency)

    return {
        "target_ns": target_ns,
        "streams": frames,
        "skew_ms": skew,
        "skew_upper_ms": upper,
        "undeclared_latency": undeclared_latency,
        "undeclared_budget": undeclared_budget,
        "holds": holds,
        # §25.1: exceeding a staleness budget "is a hold, not a silently older frame". So is an
        # empty stream. Everything else — an undeclared latency, an undeclared budget, an unbounded
        # skew — is reported and does not block, because none of them is evidence that the frame is
        # wrong. They are evidence that nobody can say how wrong it is, a different sentence.
        "verdict": "ok" if not holds else "hold",
        "deliverable": not holds,
    }


def _measure_skew(
    frames: list[dict[str, Any]], undeclared_latency: list[str]
) -> tuple[float | None, float | None]:
    lowest: dict[str, Any] | None = None
    highest: dict[str, Any] | None = None
    contributors = 0

    for frame in frames:
        if frame["chosen"] is None:
            continue
        contributors += 1
        uncertainty = 0 if frame["uncertainty_ms"] is None else frame["uncertainty_ms"]
        instant = frame["chosen"]["effective_ns"]
        if lowest is None or instant < lowest["instant"]:
            lowest = {"instant": instant, "uncertainty": uncertainty}
        if highest is None or instant > highest["instant"]:
            highest = {"instant": instant, "uncertainty": uncertainty}

    # One instant has no spread, and reporting zero would let a frame with a single working stream
    # look perfectly synchronised — the presentation §25.1 is written against.
    if contributors < 2 or lowest is None or highest is None:
        return None, None

    measured = _ms(highest["instant"] - lowest["instant"])
    if undeclared_latency:
        # The widening is unbounded once a latency is undeclared, and §47.2's "larger uncertainty"
        # has no largest.
        return measured, None
    return measured, measured + lowest["uncertainty"] + highest["uncertainty"]


def latest_common_instant(now_ns: float, streams: list[dict[str, Any]]) -> float | None:
    """The most recent instant at which every stream has a sample (§25.1's "targets an instant").

    A live control loop has to choose a target, and choosing badly defeats the whole section: aim at
    *now* and the slowest camera is always the stalest stream in every frame, so every frame is
    skewed by that camera's latency and the assembler reports a problem the loop created.

    Offered rather than imposed: a runtime may target an older instant deliberately. What this
    exists to prevent is two editions inventing two different defaults and disagreeing about frames
    neither of them assembled wrongly.
    """
    earliest: float | None = None

    for stream in streams:
        latest: float | None = None
        for sample in stream["samples"]:
            effective = _effective_ns(sample["stamp_ns"], _nullish(stream.get("latency_ms")))
            if effective > now_ns:
                continue
            if latest is None or effective > latest:
                latest = effective
        # A stream with nothing at or before `now` cannot contribute to any frame at or before
        # `now`, so there is no common instant at all — not a later one, and not one that quietly
        # drops it.
        if latest is None:
            return None
        if earliest is None or latest < earliest:
            earliest = latest

    # No streams at all: every instant is common to all of them.
    answer = now_ns if earliest is None else earliest
    # A declared latency larger than the stamp itself. The correction is still right and the instant
    # it names is before the monotonic clock started, which no `u64` can express.
    return None if answer < 0 else answer


# ── §26's record ─────────────────────────────────────────────────────────────


def to_record(frame: dict[str, Any]) -> dict[str, Any]:
    """Project a frame onto what the black box records (§25.1, §26).

    A projection rather than a rename, and it loses two things:

    * **``age_us`` cannot be negative.** The record's field is unsigned, so a chosen sample from
      *after* the target instant saturates at zero. It cannot arise on a live robot — every target a
      runtime picks is at or before now, and ``CLOCK_MONOTONIC_RAW`` does not step — which is why
      the saturation is acceptable rather than merely convenient. The signed figure stays on the
      frame.
    * **``worst_skew_us`` is null when the skew is not measurable**, because zero would make a frame
      with one working stream and three dead capture paths read, months later, as perfectly
      synchronised.
    """
    streams: list[dict[str, Any]] = []
    absent = 0
    for stream in frame["streams"]:
        if stream["chosen"] is None:
            # Distinct from the record's `streams_omitted`, which is "streams the record could not
            # fit" — a limit of the format. This is a limit of the robot, and an investigation
            # reading one as the other would look for a bigger record instead of a dead camera.
            absent += 1
            continue
        streams.append(
            {
                "name": stream["name"],
                "age_us": _micros(stream["chosen"]["age_ms"]),
                "stale": stream["chosen"]["over_budget"],
            }
        )
    return {
        "target_ns": frame["target_ns"],
        "worst_skew_us": None if frame["skew_ms"] is None else _micros(frame["skew_ms"]),
        "streams_absent": absent,
        "streams": streams,
    }


# ── The wire shapes ──────────────────────────────────────────────────────────


def _shape_record(frame: dict[str, Any]) -> Any:
    record = to_record(frame)
    return {
        "target_ns": record["target_ns"],
        "worst_skew_us": record["worst_skew_us"],
        "streams_absent": record["streams_absent"],
        "streams": [
            {"name": s["name"], "age_us": s["age_us"], "stale": s["stale"]}
            for s in record["streams"]
        ],
    }


def _shape_observation(frame: dict[str, Any]) -> Any:
    return {
        "target_ns": frame["target_ns"],
        "streams": [
            {
                "name": stream["name"],
                "modality": stream["modality"],
                "latency_ms": stream["latency_ms"],
                "uncertainty_ms": stream["uncertainty_ms"],
                "budget_ms": stream["budget_ms"],
                "age_ms": None if stream["chosen"] is None else stream["chosen"]["age_ms"],
                "considered": 0 if stream["chosen"] is None else stream["chosen"]["considered"],
                "over_budget": (
                    False if stream["chosen"] is None else stream["chosen"]["over_budget"]
                ),
                "seq": None if stream["chosen"] is None else stream["chosen"]["seq"],
                "stamp_ns": None if stream["chosen"] is None else stream["chosen"]["stamp_ns"],
            }
            for stream in frame["streams"]
        ],
        "skew_ms": frame["skew_ms"],
        "skew_upper_ms": frame["skew_upper_ms"],
        "undeclared_latency": list(frame["undeclared_latency"]),
        "undeclared_budget": list(frame["undeclared_budget"]),
        "holds": [
            {"stream": h["stream"], "reason": h["reason"], "detail": h["detail"]}
            for h in frame["holds"]
        ],
        "verdict": frame["verdict"],
        "deliverable": frame["deliverable"],
        # The projection onto §26's record, inside the frame rather than beside it. It is derived
        # from everything above, and pinning it in a second corpus would let an edition agree about
        # a frame and disagree about the only version of it that survives the day.
        "record": _shape_record(frame),
    }


def frame_to_json(case_input: dict[str, Any]) -> Any:
    """``observation/frames`` and ``fuzz/observation`` — one assembled frame."""
    return _shape_observation(
        assemble_observation(case_input["target_ns"], case_input["streams"])
    )


def target_to_json(case_input: dict[str, Any]) -> Any:
    """``observation/target`` — the instant every stream can answer for."""
    return {"target_ns": latest_common_instant(case_input["now_ns"], case_input["streams"])}
