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

``??``
    ``policy_source`` is read with nullish coalescing, so absent **and** null are both ``local``;
    that one field is not strict.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .js import to_fixed as _to_fixed

__all__ = [
    "BENCH_VALIDITY_DAYS",
    "FIELD_CONFORMANCE_FLOOR",
    "POLICY_SOURCES",
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

#: Where the robot's policy infers (§90.2): on the robot, or on a GPU the Hub allocates.
POLICY_SOURCES: Final = ("local", "hosted")

#: A year, because bench testing a humanoid is expensive and a shorter window would make
#: certification something vendors avoid rather than seek. Field conformance is what carries the
#: claim in between — measured continuously, and able to revoke the award long before the year is up.
BENCH_VALIDITY_DAYS: Final = 365

#: 0.999 rather than 1.0: a fleet of a hundred robots at 100 Hz produces tens of millions of cycles
#: a day, and demanding literal perfection would suspend every honest vendor on sensor noise while
#: telling operators nothing. What this catches is a *systematic drift* — a robot whose real
#: envelope has moved away from its declared one.
FIELD_CONFORMANCE_FLOOR: Final = 0.999

#: Every SCTS test, in declaration order, as ``(id, required_from, policy)``.
#:
#: Only the three fields the award reads. ``policy`` is the one policy source the test applies to,
#: or ``None`` for every robot (§90.3). Declaration order is load-bearing: ``missing`` and
#: ``failures`` are reported in it, and the corpus compares those arrays element by element.
SCTS_TESTS: Final[tuple[tuple[str, str, str | None], ...]] = (
    ("rhal.abi.version", "guest", None),
    ("rhal.state.completeness", "guest", None),
    ("rhal.state.rate", "host", None),
    ("rhal.command.latency", "host", None),
    ("rhal.command.rejection", "guest", None),
    ("rhal.sensor.latency", "host", None),
    ("embodiment.descriptor.valid", "guest", None),
    ("embodiment.descriptor.signed", "host", None),
    ("embodiment.limits.sourced", "host", None),
    ("embodiment.limits.honest", "certified", None),
    ("embodiment.roles.coverage", "host", None),
    ("embodiment.capabilities.declared", "host", None),
    ("embodiment.frames.complete", "host", None),
    ("safety.kernel.parity", "guest", None),
    ("safety.watchdog.fires", "host", None),
    ("safety.envelope.clamps", "host", None),
    ("safety.estop.latency", "host", None),
    ("safety.estop.independent", "certified", None),
    ("safety.ratelimit.enforced", "host", None),
    ("safety.command_loss.behaviour", "host", None),
    ("safety.homing.envelope", "host", None),
    ("safety.policy.isolation", "guest", None),
    ("package.signature.required", "guest", None),
    ("package.budget.enforced", "host", None),
    ("package.grants.gate", "guest", None),
    ("ota.rollback.works", "host", None),
    ("ota.safe_state.respected", "host", None),
    ("ota.safety_gate", "host", None),
    ("telemetry.blackbox.complete", "host", None),
    ("telemetry.replay.deterministic", "host", None),
    ("telemetry.blackbox.survives", "certified", None),
    ("offline.control.independent", "guest", "local"),
    ("offline.control.remote_holds", "guest", "hosted"),
    ("offline.reconcile.resumes", "host", None),
    ("security.identity.attested", "host", None),
    ("security.credential.scope", "guest", None),
    ("power.safe_state.reachable", "host", None),
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


def tests_required_for(tier: str, policy: str = "local") -> list[str]:
    """Tests required at a tier — its own, plus every tier below it — for a robot whose policy
    infers where ``policy`` says (§90.3)."""
    ceiling = TIER_ORDER.index(tier)
    return [
        test_id
        for test_id, required_from, only in SCTS_TESTS
        if TIER_ORDER.index(required_from) <= ceiling and (only is None or only == policy)
    ]


def award_certification(inp: dict[str, Any]) -> dict[str, Any]:
    """Award a tier from the evidence."""
    reasons: list[str] = []
    best = "none"
    failures: list[str] = []
    missing: list[str] = []

    policy = inp.get("policy_source") if isinstance(inp, dict) else None
    if policy is None:
        policy = "local"
    if policy not in POLICY_SOURCES:
        raise ValueError(f"policy_source must be one of {', '.join(POLICY_SOURCES)}, not `{policy}`")

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

        for test_id in tests_required_for(tier, policy):
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

    # §90.4: a robot whose policy runs remotely may hold host and never certified. After the
    # simulator cap, which is the stricter and would otherwise be reported against a tier this one
    # had already lowered.
    if policy == "hosted" and best == "certified":
        reasons.append(
            "The policy runs remotely (§90.4), so the award is capped at host; the evidence "
            "supports certified."
        )
        best = "host"

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
