"""§15's certification award, Python edition.

Transcribed from ``awardCertification`` and its table in ``reference/typescript/conformance.ts``.
Every id, threshold and message string is read from that file, never from ``corpus/``.

Two rules make this worth having.

**Absence is not a pass.** A required test with no result is a ``missing``, never a silent success —
the loophole every self-certification scheme eventually dies of.

**The award is capped by the evidence, not by the claim.** A vendor claiming the certified tier
whose independent stop path was never measured is awarded ``host``, with the reason recorded, rather
than refused outright. Downgrading tells the truth and keeps the vendor in the programme; refusing
sends them to go write "ROS 2 compatible" on a web page instead, which helps nobody.

── Where Python is quietly not JavaScript, here ──────────────────────────────

``toFixed(n)``
    ECMAScript negates first and then picks the integer nearest ``x × 10ⁿ``, **the larger on a
    tie** — which is ties-away-from-zero. Python's ``format`` rounds half to even, and it also
    rounds the *decimal* rendering rather than the exact binary value. Both are reproduced with
    ``Decimal`` over the exact double, because the figure lands in a suspension notice an operator
    reads.

``indexOf`` returning ``-1``
    An unrecognised tier makes ``claimedIndex`` ``-1``, so the evidence walk runs **zero** times and
    the verdict is ``none`` with no reasons at all. ``list.index`` raises instead, which would turn
    a bad input into a crash rather than a refusal.

``new Map(entries)``
    Later entries overwrite earlier ones, so two results for one test id keep the **last**.

strict ``undefined``
    ``field_conformance``, ``bench_age_days`` and ``simulator`` are all tested strictly. A field
    present and null takes the other branch, and ``dict.get`` collapses the two.
"""

from __future__ import annotations

import math
from decimal import Decimal, ROUND_HALF_UP
from typing import Any, Final

__all__ = [
    "BENCH_VALIDITY_DAYS",
    "FIELD_CONFORMANCE_FLOOR",
    "SCTS_AREAS",
    "SCTS_SUITES",
    "TIER_ORDER",
    "award_certification",
    "certification_to_json",
    "field_conformance",
    "tests_required_for",
]

SCTS_SUITES: Final = ("sim", "bench", "field")

SCTS_AREAS: Final = (
    "rhal", "embodiment", "safety", "package", "ota", "telemetry", "offline", "security", "power",
)

TIER_ORDER: Final = ("guest", "host", "certified")

#: A year, because bench testing a humanoid is expensive and a shorter window would make
#: certification something vendors avoid rather than seek. Field conformance is what carries the
#: claim in between — measured continuously, and able to revoke the award long before the year is up.
BENCH_VALIDITY_DAYS: Final = 365

#: 0.999 rather than 1.0: a fleet of a hundred robots at 100 Hz produces tens of millions of cycles
#: a day, and demanding literal perfection would suspend every honest vendor on sensor noise while
#: telling operators nothing. What this catches is a *systematic drift* — a robot whose real
#: envelope has moved away from its declared one.
FIELD_CONFORMANCE_FLOOR: Final = 0.999

#: Every SCTS test, in declaration order, as ``(id, required_from)``.
#:
#: Only the two fields the award reads. Declaration order is load-bearing: ``missing`` and
#: ``failures`` are reported in it, and the corpus compares those arrays element by element.
SCTS_TESTS: Final[tuple[tuple[str, str], ...]] = (
    ("rhal.abi.version", "guest"),
    ("rhal.state.completeness", "guest"),
    ("rhal.state.rate", "host"),
    ("rhal.command.latency", "host"),
    ("rhal.command.rejection", "guest"),
    ("rhal.sensor.latency", "host"),
    ("embodiment.descriptor.valid", "guest"),
    ("embodiment.descriptor.signed", "host"),
    ("embodiment.limits.sourced", "host"),
    ("embodiment.limits.honest", "certified"),
    ("embodiment.roles.coverage", "host"),
    ("embodiment.capabilities.declared", "host"),
    ("embodiment.frames.complete", "host"),
    ("safety.kernel.parity", "guest"),
    ("safety.watchdog.fires", "host"),
    ("safety.envelope.clamps", "host"),
    ("safety.estop.latency", "host"),
    ("safety.estop.independent", "certified"),
    ("safety.ratelimit.enforced", "host"),
    ("safety.command_loss.behaviour", "host"),
    ("safety.homing.envelope", "host"),
    ("safety.policy.isolation", "guest"),
    ("package.signature.required", "guest"),
    ("package.budget.enforced", "host"),
    ("package.grants.gate", "guest"),
    ("ota.rollback.works", "host"),
    ("ota.safe_state.respected", "host"),
    ("ota.safety_gate", "host"),
    ("telemetry.blackbox.complete", "host"),
    ("telemetry.replay.deterministic", "host"),
    ("telemetry.blackbox.survives", "certified"),
    ("offline.control.independent", "guest"),
    ("offline.reconcile.resumes", "host"),
    ("security.identity.attested", "host"),
    ("security.credential.scope", "guest"),
    ("power.safe_state.reachable", "host"),
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


def _to_fixed(value: float, digits: int) -> str:
    """``Number.prototype.toFixed(digits)``.

    ECMAScript takes the sign off first and then picks the integer nearest ``x × 10ⁿ``, the larger
    on a tie — ties **away from zero** once the sign is restored. It also decides on the *exact*
    binary value of the double, which is why this goes through ``Decimal(value)`` rather than
    through a decimal string: ``Decimal(0.1)`` is the number the machine holds, and ``Decimal('0.1')``
    is not.
    """
    if not math.isfinite(value):
        return "NaN" if value != value else ("Infinity" if value > 0 else "-Infinity")
    sign = "-" if value < 0 else ""
    exact = Decimal(abs(value))
    quantum = Decimal(1).scaleb(-digits)
    return sign + str(exact.quantize(quantum, rounding=ROUND_HALF_UP))


def tests_required_for(tier: str) -> list[str]:
    """Tests required at a tier: its own, plus every tier below it."""
    ceiling = TIER_ORDER.index(tier)
    return [
        test_id
        for test_id, required_from in SCTS_TESTS
        if TIER_ORDER.index(required_from) <= ceiling
    ]


def award_certification(inp: dict[str, Any]) -> dict[str, Any]:
    """Award a tier from the evidence."""
    reasons: list[str] = []
    best = "none"
    failures: list[str] = []
    missing: list[str] = []

    # Later entries overwrite earlier ones, as `new Map(entries)` does: two results for one test id
    # keep the last.
    by_id: dict[str, Any] = {}
    for result in inp["results"]:
        by_id[result["test_id"]] = result

    # `indexOf` on an unrecognised tier is -1, so the walk runs zero times and the verdict is `none`
    # with no reasons — a refusal rather than the exception `list.index` would raise.
    claimed = inp["claimed"]
    claimed_index = TIER_ORDER.index(claimed) if claimed in TIER_ORDER else -1

    # Walk upward and keep the highest tier fully satisfied, so the evidence sets the ceiling.
    for i in range(claimed_index + 1):
        tier = TIER_ORDER[i]
        tier_failures: list[str] = []
        tier_missing: list[str] = []

        for test_id in tests_required_for(tier):
            result = by_id.get(test_id)
            if result is None or result["outcome"] == "skipped":
                tier_missing.append(test_id)
            elif result["outcome"] == "fail":
                tier_failures.append(test_id)
            # `not_applicable` passes: the balance guard on a wheeled base is genuinely
            # inapplicable, and forcing a fail there would push vendors to fake a result rather
            # than declare a shape.

        if not tier_failures and not tier_missing:
            best = tier
            failures = []
            missing = []
        else:
            failures = tier_failures
            missing = tier_missing
            reasons.append(
                f"{tier}: {len(tier_failures)} failing, {len(tier_missing)} unmeasured."
            )
            break

    if best != "none" and claimed != best:
        reasons.append(f"Claimed {claimed}; the evidence supports {best}.")

    # §35.3 and §70: a simulator RHAL driver declares itself one, and the declaration caps the award
    # at guest — not because a simulated run is worthless, but because no simulated run may ever be
    # presented as bench evidence, and a `host` badge on a body that does not exist is exactly that.
    #
    # Applied *after* the evidence walk rather than by refusing to walk, so the verdict still records
    # which tier the tests would have supported. "The evidence supports host; the body is simulated,
    # so guest" is a more useful sentence to a vendor than a silent guest.
    if inp.get("simulator") is True and best in ("host", "certified"):
        reasons.append(
            f"The body is simulated (§35.3), so the award is capped at guest; the evidence "
            f"supports {best}."
        )
        best = "guest"

    # Field conformance suspends an award rather than lowering it. A robot whose commands have been
    # leaving its envelope is not a lower-tier robot — it is one whose certificate no longer
    # describes it, and the honest state is suspended pending re-measurement.
    conformance = _field(inp, "field_conformance")
    if conformance is not _ABSENT and conformance < FIELD_CONFORMANCE_FLOOR and best != "none":
        reasons.append(
            f"Field conformance {_to_fixed(conformance * 100, 3)}% is below the "
            f"{_to_fixed(FIELD_CONFORMANCE_FLOOR * 100, 1)}% floor; the award is suspended "
            "pending re-measurement."
        )
        best = "none"

    expires: int | None = None
    bench_age = _field(inp, "bench_age_days")
    if best != "none" and bench_age is not _ABSENT:
        expires = BENCH_VALIDITY_DAYS - bench_age
        if expires <= 0:
            reasons.append("Bench results are older than a year; the award has lapsed.")
            best = "none"
            expires = 0

    return {
        "awarded": best,
        "failures": failures,
        "missing": missing,
        "reasons": reasons,
        "expires_in_days": expires,
    }


def field_conformance(window: dict[str, Any]) -> float | None:
    """Field conformance from a reporting window.

    The ratio of cycles the kernel did not have to intervene in. Not "violations detected", which
    would reward a robot with a broken detector — a robot reporting zero cycles reports no
    conformance at all, and the caller gets nothing rather than a flattering 1.0.
    """
    cycles = window["cycles"]
    if not (isinstance(cycles, (int, float)) and not isinstance(cycles, bool) and math.isfinite(cycles)):
        return None
    if cycles <= 0:
        return None
    # Clamped rather than trusted. More interventions than cycles is a robot whose counters disagree
    # with each other, and the honest floor for that is zero rather than a negative figure §15 has
    # no reading for. Above 1 is the dangerous direction and is the one this must never produce.
    return max(0, min(1, (cycles - window["interventions"]) / cycles))


def certification_to_json(case_input: dict[str, Any]) -> Any:
    """``certification/award`` — the tier the evidence supports, and every reason it is not higher."""
    verdict = award_certification(case_input)
    return {
        "awarded": verdict["awarded"],
        "failures": verdict["failures"],
        "missing": verdict["missing"],
        "reasons": verdict["reasons"],
        "expires_in_days": verdict["expires_in_days"],
    }
