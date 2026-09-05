"""§74 — telemetry, as windows rather than samples. Python edition.

Transcribed from ``reference/typescript/telemetry.ts``. Every rule and message string is read from
that file, never from ``corpus/``.

§34.4 gave telemetry a priority, a drop policy, a counter and a paragraph of its own, and neither
§17's eight endpoints nor §57's nine gave it a route. §74 adds the tenth, and this module is what
crosses it.

── A window, not a sample and not a total ───────────────────────────────────

An item covers a **half-open interval** and carries counts over it. The three obvious alternatives
each break a rule that already exists:

* **A series of samples** — a week offline produces ten thousand items, which is the state §34.4
  collapses check-ins to avoid.
* **A running total since boot** — a drop becomes invisible, because the next total absorbs it and
  the series stays tidy. That is precisely what §47.4 forbids.
* **Counts since the last successful send** — makes delivery load-bearing for correctness, and a
  retry that succeeded twice would double-count with no way for the robot to know.

Counts over a self-describing window have none of those properties, and they buy §47.4's rule from
the *shape of the data*: dropping one loses that window and nothing else, and the loss is visible
because the windows no longer tile.

── Two clocks, and both of them ─────────────────────────────────────────────

§35.1 assigns them: ``CLOCK_REALTIME`` is for *"correlation with the Hub"* and
``CLOCK_MONOTONIC_RAW`` is for anything measured. A window does both jobs, so it carries both.
Their disagreement is **reported rather than reconciled**: the skew is a fact about the robot and
not a reason to discard the counts.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``Math.trunc(skew / 1000)``
    Truncation toward zero, and the reference's own comment says why it is not ``Math.round``: that
    is half toward positive infinity, the Rust helper is half away from zero, and the two disagree
    on exactly the negative halves a backwards clock produces. Python's ``//`` **floors**, so
    ``-90500 // 1000`` is ``-91`` where the rule wants ``-90``. :func:`_trunc` is the rule, and it
    returns an ``int`` so the sentence carries no ``.0``.

``Number.isFinite``
    A count of ``true`` is not a count of one. :func:`_finite` rejects booleans, which
    ``isinstance(True, int)`` lets through, and the non-finite floats, which ``isinstance`` does
    not.

``count === null || !Number.isFinite(count) || count < 0``
    Three ways to be unmeasured and one of them is a **negative** count, which is a counter that
    wrapped or a field somebody defaulted to ``-1``. It lands in ``unmeasured`` rather than in a
    rate, because a negative rate on a fleet page is a number nobody can read.

``TELEMETRY_COUNTERS.filter(…)`` then ``.sort()``
    UTF-16 order over a list that is already in declaration order, which is not the same list. The
    sort is what a console iterates.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .certification import field_conformance
from .js import utf16_key as _utf16_key

__all__ = [
    "TELEMETRY_COUNTERS",
    "WINDOW_FAULTS",
    "assess_window",
    "conformance_of",
    "coverage",
    "coverage_to_json",
    "window_to_json",
]

#: What a window may count.
#:
#: A closed set for ``reconcile_stages``' reason: a fleet console divides by these, and six
#: spellings of one counter is six columns that each look empty.
#:
#: ``cycles`` and ``interventions`` are the two §15's field conformance is computed from, and
#: ``guard_trips`` is §33.6's aggregate field that a check-in structurally cannot carry — a rate is a
#: count over a window, and §34.4 collapses check-ins because every field in one is about *now*.
TELEMETRY_COUNTERS: Final = (
    "cycles",
    "guard_trips",
    "clamps",
    "interventions",
    "estops",
    "package_restarts",
    "queue_dropped",
    "reconcile_passes",
)

WINDOW_FAULTS: Final = (
    "unbounded",
    "inverted",
    "unmeasured_duration",
    "implausible_duration",
)


def _fault(code: str, severity: str, message: str) -> dict[str, Any]:
    return {"code": code, "severity": severity, "message": message}


def _finite(value: Any) -> bool:
    """``typeof value === 'number' && Number.isFinite(value)``."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return False
    number = float(value)
    return not (math.isnan(number) or math.isinf(number))


def _trunc(value: float) -> int:
    """``Math.trunc`` — toward zero, where Python's ``//`` floors.

    The reference chose truncation deliberately over rounding, because ``Math.round`` is half toward
    positive infinity and the Rust edition's helper is half away from zero, and the two disagree on
    exactly the negative halves a backwards clock produces. Truncation is the one reading both
    editions spell identically without a shared helper — and a third edition that reached for ``//``
    would put the disagreement back, one second off, in a sentence on a fleet page.
    """
    return int(value)


def assess_window(window: dict[str, Any]) -> dict[str, Any]:
    """What one window says, and whether it says anything (§74.2, §74.3).

    Every check runs and the result carries all of them, for ``admit_package``'s reason: a fleet
    operator looking at a robot whose telemetry is being refused should get one list rather than one
    problem per window.

    **An unusable window is refused rather than partly counted.** A window with no duration cannot
    produce a rate, and a window whose bounds are inverted is a robot whose clock moved under it.
    Counting either would put a number on a fleet page that is not a rate of anything — and the
    counts are still on the record, because the *item* is kept whether or not the arithmetic is.
    """
    faults: list[dict[str, Any]] = []
    from_ms = window.get("from_ms")
    to_ms = window.get("to_ms")
    elapsed = window.get("elapsed_ms")

    bounded = _finite(from_ms) and _finite(to_ms)
    if not bounded:
        faults.append(
            _fault(
                "unbounded",
                "error",
                "This window does not say when it was. §35.1 makes wall time the clock a Hub "
                "correlates on, so a window without both bounds cannot be placed beside another "
                "robot's — and the gap between windows is how a dropped one is noticed at all "
                "(§34.4, §47.4).",
            )
        )
    elif to_ms < from_ms:
        faults.append(
            _fault(
                "inverted",
                "error",
                "This window ends before it starts. That is a clock that stepped backwards "
                "mid-window rather than a counting error, and the counts in it are over an "
                "interval nobody can name.",
            )
        )

    measured = _finite(elapsed) and elapsed > 0
    if not measured:
        faults.append(
            _fault(
                "unmeasured_duration",
                "error",
                "This window carries no monotonic duration, so there is nothing to divide the "
                "counts by. §35.1 keeps rates off wall time on purpose: an NTP step would "
                "otherwise turn four hundred guard trips into a plausible number and nothing would "
                "look wrong.",
            )
        )

    span = (to_ms - from_ms) if bounded else 0
    skew = (span - elapsed) if (bounded and measured) else 0

    # A minute of disagreement over a five-minute window is a machine that was suspended, an NTP
    # correction, or a clock nobody set. It is a fact about the robot and not a reason to throw the
    # counts away, so it is a warning with the number in it.
    if bounded and measured and (skew > 60_000 or skew < -60_000):
        faults.append(
            _fault(
                "implausible_duration",
                "warning",
                f"Wall time and the monotonic clock disagree by {_trunc(skew / 1000)} s over this "
                "window. The counts are kept and the rates are computed from the monotonic "
                "duration (§35.1); the disagreement is worth one line on a fleet page, because it "
                "is what a suspended machine and a corrected clock both look like.",
            )
        )

    rates: dict[str, float] = {}
    unmeasured: list[str] = []
    counters = window.get("counters") or {}
    for name in TELEMETRY_COUNTERS:
        count = counters.get(name)
        if count is None or not _finite(count) or count < 0:
            unmeasured.append(name)
            continue
        if measured:
            rates[name] = count / (elapsed / 1000)

    return {
        "usable": all(entry["severity"] != "error" for entry in faults),
        "elapsed_ms": elapsed if measured else 0,
        "skew_ms": skew,
        "rates": rates,
        "unmeasured": sorted(unmeasured, key=_utf16_key),
        "faults": faults,
    }


# ── What a set of windows covers ─────────────────────────────────────────────


def coverage(windows: list[dict[str, Any]]) -> dict[str, Any]:
    """What a set of windows actually covers (§74.1, §74.5).

    This is the function §15's field conformance has to be computed through, and the reason is one
    line: **the denominator is the time that was observed, not the time that elapsed.**

    A robot that sent windows for one hour out of a day has not demonstrated anything about the
    other twenty-three, and a Hub that divided by twenty-four hours would report a conformance
    figure that is arithmetic over a period it did not observe. §15 puts a floor of 0.999 on that
    figure and suspends an award below it, so the difference between the two denominators is the
    difference between suspending a robot and not.

    Overlapping windows are merged rather than summed for the purpose of the span, and their counts
    are summed anyway — an overlap is a robot that restarted and re-reported, and the honest reading
    of two counts over overlapping intervals is that both events happened.
    """
    usable: list[dict[str, Any]] = []
    unusable = 0
    observed = 0.0

    for window in windows:
        report = assess_window(window)
        if not report["usable"]:
            unusable += 1
            continue
        observed += report["elapsed_ms"]
        usable.append({"from": window["from_ms"], "to": window["to_ms"], "window": window})

    totals: dict[str, float] = {}
    measured: set[str] = set()
    for entry in usable:
        counters = entry["window"].get("counters") or {}
        for name in TELEMETRY_COUNTERS:
            count = counters.get(name)
            if count is None or not _finite(count) or count < 0:
                continue
            totals[name] = totals.get(name, 0) + count
            measured.add(name)

    # A stable sort on `from`, as `Array.prototype.sort` is: two windows starting at the same
    # instant keep the order they were reported in, which is the order a robot wrote them.
    ordered = sorted(usable, key=lambda entry: entry["from"])
    gaps: list[dict[str, Any]] = []
    spanned: float = 0

    if ordered:
        reach = ordered[0]["to"]
        for entry in ordered[1:]:
            if entry["from"] > reach:
                gaps.append({"from_ms": reach, "to_ms": entry["from"]})
            reach = max(reach, entry["to"])
        # The last window may end before the latest end seen, when windows overlap out of order.
        spanned = max(0, reach - ordered[0]["from"])

    return {
        "observed_ms": observed,
        "spanned_ms": spanned,
        "gaps": gaps,
        "unusable": unusable,
        "totals": totals,
        "unmeasured": sorted(
            (name for name in TELEMETRY_COUNTERS if name not in measured), key=_utf16_key
        ),
    }


def conformance_of(value: dict[str, Any]) -> float | None:
    """§15's field conformance, over what was actually observed.

    ``None`` when nothing counted cycles, which is not the same as 1 and is what
    ``award_certification`` takes for *"no fleet data exists yet"*. A Hub that substituted 1 here
    would be awarding a certified tier on the strength of a robot that has never reported.
    """
    totals = value["totals"]
    cycles = totals.get("cycles")
    if cycles is None or not _finite(cycles):
        return None
    # Absent interventions read as zero *only once there are cycles*, and that is not the usual
    # rule. §74.3 keeps unmeasured and zero apart everywhere else — but a window that counted cycles
    # and did not count interventions is a robot whose kernel intervened in none of them, because
    # the counter that would have recorded one is the counter that recorded the cycles.
    interventions = totals.get("interventions", 0)
    # **Through `field_conformance`, not beside it.** These two functions computed the same ratio,
    # with the same clamp, in two modules, and neither had a caller — so the day one of them gained
    # a rule about a robot whose counters disagree, the other would have kept the old answer and
    # nothing would have compared them.
    return field_conformance({"cycles": cycles, "interventions": interventions})


# ── The wire shapes ──────────────────────────────────────────────────────────


def window_to_json(case_input: dict[str, Any]) -> Any:
    """``telemetry/window`` — what one window says, and whether it says anything.

    ``rates`` is an object rather than a list, and the counters that were *not* measured are absent
    from it rather than present as zero — which is the whole of §74.3 in one shape. A consumer
    iterating :data:`TELEMETRY_COUNTERS` and reading ``rates[name] or 0`` would be committing the
    mistake the section exists to prevent; one that reads ``unmeasured`` first cannot.
    """
    report = assess_window(case_input)
    return {
        "elapsed_ms": report["elapsed_ms"],
        "faults": [
            {"code": f["code"], "message": f["message"], "severity": f["severity"]}
            for f in report["faults"]
        ],
        "rates": dict(report["rates"]),
        "skew_ms": report["skew_ms"],
        "unmeasured": list(report["unmeasured"]),
        "usable": report["usable"],
    }


def coverage_to_json(case_input: dict[str, Any]) -> Any:
    """``telemetry/coverage`` — what a set of windows covers.

    ``gaps`` is the field this shape exists for. §47.4's rule — a gap annotated as a gap is honest,
    a gap that looks like a quiet period is not — is obtained here from the data rather than from an
    annotation, and a shape that dropped the gaps would leave a fleet page unable to tell a robot
    that reported nothing from one whose telemetry was discarded under §34.4's drop policy.
    """
    value = coverage(list(case_input.get("windows") or []))
    return {
        # §15's third suite, over what was actually observed. Nullable rather than optional, and
        # frozen here rather than left to each consumer, because this is the number that decides
        # whether a certificate is suspended.
        "conformance": conformance_of(value),
        "gaps": [{"from_ms": gap["from_ms"], "to_ms": gap["to_ms"]} for gap in value["gaps"]],
        "observed_ms": value["observed_ms"],
        "spanned_ms": value["spanned_ms"],
        "totals": dict(value["totals"]),
        "unmeasured": list(value["unmeasured"]),
        "unusable": value["unusable"],
    }
