"""§11 and §17's check-in — the one thing a robot says about itself, Python edition.

Transcribed from ``reference/typescript/checkin.ts`` and ``reference/typescript/shape.ts``. Every
rule, message string and serialised key is read from those files, never from ``corpus/``.

    ``POST /devices/{id}/checkin { generation, applied, health, attestation }``

Four fields in the specification and one rule behind all of them, from §17:

    **The platform never claims a robot is running something it has not heard from.** A deployment
    is ``pending`` until a check-in.

Everything here exists to make that rule survive contact with a robot that is halfway through
converging, or waiting for a safe state, or running software nobody asked for. The check-in is the
*only* evidence the Hub has, so a check-in that overstates is a fleet console that lies, and there is
no second source to catch it.

── A check-in carries no history, and that is what makes it collapsible ─────

§34.4 puts check-ins in a class of their own — *"superseded — only the newest matters, so older ones
are collapsed rather than queued"* — because *"a week offline should produce one current check-in on
reconnect, not ten thousand historical ones."*

That collapsing is only safe because **every field here is a statement about now**. No deltas, no
counters since last time, no event list. A check-in carrying "twelve guard trips since the last one"
would make collapsing lossy, and the queue would be discarding data while believing it was
discarding duplicates.

── ``applied`` is not a field the daemon may set ────────────────────────────

§33.3: *"Convergence is one plan being empty, not one pass succeeding."* So ``applied`` is derived
from what is left to do — and there are **four** answers rather than two, because "still working"
and "waiting for a safe state that may not come for hours" are different sentences to put in front
of an operator (§33.4).

The generation on the wire is the one the robot **applied**, never the one it was asked for. That is
the single field §17's rule lives or dies on: a check-in echoing the desired generation would mark
every deployment applied the moment the robot heard about it.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``entry.deferred === true``
    Strict, and it decides between ``deferred`` and ``converging`` — between a robot that needs a
    person and a robot that needs a minute. ``if entry["deferred"]`` would make a truthy ``1`` or a
    non-empty reason string defer a plan that nothing is waiting on, and dispatch an engineer to a
    robot that was going to converge on its own. :func:`_is_true` is that comparison, spelled out.

``value ?? null`` then ``Number.isFinite``
    Two gates, not one, and Python collapses them by accident. ``value ?? null`` keeps a declared
    ``0`` — §33.6's generation lag of zero is a **measurement** — so the absence test is against
    ``None`` and never against falsiness. ``Number.isFinite`` then rejects ``NaN`` and the
    infinities, which ``isinstance(x, float)`` does not, and rejects booleans, which
    ``isinstance(True, int)`` lets through.

``.sort()``
    JavaScript's default sort is by UTF-16 code unit, not by code point. :func:`_utf16_key` is that
    order; ``sorted()`` alone would disagree with the reference on the first stage name or package
    id above U+FFFF.

``new Set(pending.map(…))`` then ``.sort()``
    Insertion order, then a sort. A Python ``set`` is neither, so the deduplication is done with a
    dict and the sort applied afterwards — which happens to agree here, and would not if the sort
    were ever dropped.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .identity import attest, attestation_block
from .js import js_number as _js_number, js_trim as _js_trim, utf16_key as _utf16_key

__all__ = [
    "CLAIMS",
    "HEALTH_FIELDS",
    "READINESS",
    "STAGES",
    "check_in",
    "claim_for",
    "claim_to_json",
    "document_to_json",
    "health_to_json",
    "summarize",
    "summary",
]

#: What a robot may claim about the generation it was given.
CLAIMS: Final = ("applied", "converging", "deferred", "inconsistent")

#: §33.4's six ordered stages, as a check-in names them.
#:
#: Here rather than left as free strings because a fleet console groups by them: *"forty robots are
#: waiting on stage 1"* is a fact somebody acts on, and *"forty robots reported forty spellings of
#: the OS image stage"* is not.
STAGES: Final = ("os_image", "rhal_driver", "package", "profile", "policy", "lease")

READINESS: Final = ("ready", "stalled", "not_running", "unknown")

#: The named aggregate fields, in the order §33.6 lists them.
HEALTH_FIELDS: Final = (
    "generation_lag",
    "guard_trip_rate",
    "storage_headroom",
    "battery",
    "temperature_c",
)

def _text(value: Any) -> str:
    """``(value ?? '').trim()`` over a field a decoder may find is not a string at all."""
    return _js_trim(value) if isinstance(value, str) else ""


def _utf16_key(value: str) -> tuple[int, ...]:
    """A sort key ordering by UTF-16 code unit, which is what JavaScript's default sort does."""
    encoded = value.encode("utf-16-be", errors="surrogatepass")
    return tuple((encoded[i] << 8) | encoded[i + 1] for i in range(0, len(encoded), 2))


def _is_true(value: Any) -> bool:
    """``value === true``. See the header: this one decides who gets dispatched."""
    return value is True


def _number(value: Any) -> float | None:
    """``value ?? null``, then ``Number.isFinite``.

    A ``bool`` is not a number here even though Python says ``isinstance(True, int)``, and a
    ``NaN`` is not a reading even though it is a ``float``.
    """
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    number = float(value)
    return None if math.isnan(number) or math.isinf(number) else number


def _generation(value: Any) -> float:
    """``progress.observed_generation ?? 0``, read off raw JSON."""
    number = _number(value)
    return 0.0 if number is None else number


def _stages_of(pending: list[dict[str, Any]]) -> list[str]:
    """``[...new Set(pending.map(e => e.stage))].sort()``."""
    seen: dict[str, None] = {}
    for entry in pending:
        seen.setdefault(_stage_of(entry), None)
    return sorted(seen, key=_utf16_key)


def _stage_of(entry: dict[str, Any]) -> str:
    stage = entry.get("stage")
    return stage if isinstance(stage, str) else ""


# ── §17's claim ──────────────────────────────────────────────────────────────


def claim_for(progress: dict[str, Any]) -> dict[str, Any]:
    """What this robot is entitled to claim (§17, §33.3, §33.4).

    The order of the tests is the order of severity, and the last one is the interesting one. An
    **empty plan with disagreeing generations** is a reconciler that computed a plan closing
    nothing: the robot is not converged and nothing is going to converge it. Reporting that as
    ``applied`` would hide a supervisor bug behind a green tick on a fleet console, which is the one
    place nobody would go looking for it. It gets a name.
    """
    observed = _generation(progress.get("observed_generation"))
    desired = _generation(progress.get("desired_generation"))
    pending = progress.get("pending") or []
    blocking = _stages_of(pending)

    if pending:
        # `every` over a non-empty list. The strict comparison is the whole of the branch: see the
        # header, and `self_check.py`'s trap for it.
        if all(_is_true(entry.get("deferred")) for entry in pending):
            return {
                "claim": "deferred",
                "applied": False,
                "generation": observed,
                "detail": "Everything left to do is waiting for a safe state (§33.4): "
                f"{', '.join(blocking)}. Nothing here converges until the robot is put in one, "
                "which is an operator action rather than a matter of time.",
                "blocking": blocking,
            }
        return {
            "claim": "converging",
            "applied": False,
            "generation": observed,
            "detail": f"Still reconciling generation {_js_number(desired)}, with work left at "
            f"{', '.join(blocking)}. This robot is running generation {_js_number(observed)}.",
            "blocking": blocking,
        }

    if observed != desired:
        return {
            "claim": "inconsistent",
            "applied": False,
            "generation": observed,
            "detail": f"The plan is empty and this robot has applied generation "
            f"{_js_number(observed)} against a desired {_js_number(desired)}. An empty plan that "
            "closes nothing is a supervisor that cannot see the difference it is meant to close, "
            "and reporting it as applied would put a green tick over it.",
            "blocking": blocking,
        }

    return {
        "claim": "applied",
        "applied": True,
        "generation": observed,
        "detail": f"Generation {_js_number(observed)} is applied and the plan is empty.",
        "blocking": blocking,
    }


# ── §33.6's health, and the age rule ─────────────────────────────────────────


def summarize(health: dict[str, Any]) -> dict[str, Any]:
    """Roll health up for a check-in (§33.6).

    Three rules, and the second is the one this function was written for:

    * **A stalled package is degraded and an unmeasured field is not.** *"We could not measure the
      battery"* is not *"the battery is fine"*, and it is also not *"the robot is unhealthy"* — it
      is a gap, and reporting a gap as a fault is how an operator learns to ignore the health
      screen.
    * **A value with no age is dropped and named.** §33.6 ends with four words that decide the whole
      shape: *"Every field carries its age."* A number without one is not a measurement, it is a
      value a fleet console will render as current — and the battery reading from four hours ago is
      exactly the one somebody dispatches an engineer on. So it is reported in ``undated`` rather
      than in ``unmeasured``, because *"nobody measured this"* and *"somebody measured this and lost
      track of when"* send different people to different places.
    * **``unknown`` readiness is not a stall.** A policy in its first cycle has legitimately
      produced nothing, and reporting that as a fault would make every start look like one.
    """
    packages = []
    for entry in health.get("packages") or []:
        readiness = entry.get("readiness")
        packages.append(
            {
                "id": entry["id"] if isinstance(entry.get("id"), str) else "",
                "live": _is_true(entry.get("live")),
                # An unrecognised word decodes to `unknown` rather than to `ready`, for the reason
                # every default here points the same way: a case that did not say has not
                # established that a policy is producing, and `unknown` is the one of the four that
                # claims nothing.
                "readiness": readiness if readiness in READINESS else "unknown",
                "age_ms": _generation(entry.get("age_ms")),
            }
        )

    stalled = sorted(
        (entry["id"] for entry in packages if entry["readiness"] == "stalled"), key=_utf16_key
    )

    unmeasured: list[str] = []
    undated: list[str] = []
    fields: dict[str, Any] = {}

    for name in HEALTH_FIELDS:
        measured = health.get(name)
        measured = measured if isinstance(measured, dict) else {}
        value = _number(measured.get("value"))
        age = _number(measured.get("age_ms"))
        if value is None:
            unmeasured.append(name)
            fields[name] = None
            continue
        # A clock that went backwards is a fact worth reporting rather than an absence: a negative
        # age is `undated`, beside the reading that has no age at all.
        if age is None or age < 0:
            undated.append(name)
            fields[name] = None
            continue
        fields[name] = {"value": value, "age_ms": age}

    return {
        "degraded": len(stalled) > 0,
        "stalled": stalled,
        "unmeasured": sorted(unmeasured, key=_utf16_key),
        "undated": sorted(undated, key=_utf16_key),
        "packages": packages,
        "fields": fields,
    }


# ── The document ─────────────────────────────────────────────────────────────


def _guard_report(report: Any) -> dict[str, Any] | None:
    """§7.6's ceilings and guard partition, carried through unchanged.

    Nothing here is recomputed, and that is deliberate: what the guards are and what they lack is
    the kernel's answer for this body, and a builder that re-derived any of it would be a second
    implementation of §7.3 living in the reporting path.

    ``enforced`` is the one field with a default, and it defaults to ``false`` — the claim-least
    direction. A document that forgot to say whether its limits are enforced is not a document
    asserting that they are, and ``enforcement`` beside it is a different fact: what the *tier*
    permits, which on an edition with no policy runtime is a permission nothing exercises.
    """
    if not isinstance(report, dict):
        return None
    return {
        "resolved_at": _text(report.get("resolved_at")),
        "enforcement": _text(report.get("enforcement")),
        "enforced": _is_true(report.get("enforced")),
        "joint_velocity_max_rad_s": _number(report.get("joint_velocity_max_rad_s")),
        "joint_torque_max_nm": _number(report.get("joint_torque_max_nm")),
        "base_velocity_max_m_s": _number(report.get("base_velocity_max_m_s")),
        "min_control_hz": _number(report.get("min_control_hz")),
        "max_delta_per_cycle": _number(report.get("max_delta_per_cycle")),
        "self_collision_margin_m": _number(report.get("self_collision_margin_m")),
        "source": report["source"] if isinstance(report.get("source"), str) else None,
        "active": [item for item in (report.get("active") or []) if isinstance(item, str)],
        "unavailable": [
            {
                "guard": _text(entry.get("guard")),
                "missing": [
                    item for item in (entry.get("missing") or []) if isinstance(item, str)
                ],
            }
            for entry in (report.get("unavailable") or [])
        ],
        # §80's third state. Absent decodes to an empty list, on this decoder's own rule that a
        # field that is not there is absent rather than a zero — and an older robot that predates
        # the state genuinely has none to report.
        "inert": [
            {"guard": _text(entry.get("guard")), "reading": _text(entry.get("reading"))}
            for entry in (report.get("inert") or [])
        ],
        "notices": [
            {
                "code": _text(entry.get("code")),
                "subjects": [
                    item for item in (entry.get("subjects") or []) if isinstance(item, str)
                ],
            }
            for entry in (report.get("notices") or [])
        ],
    }


def check_in(case_input: dict[str, Any]) -> dict[str, Any]:
    """Build the document a robot sends (§11, §17).

    This function is the whole of what crosses to the Hub, which is why it lives beside the
    contracts rather than in the daemon: two editions must produce the **same bytes** for the same
    robot, or a fleet console shows two robots that differ only in who compiled their supervisor.

    It cannot fail, for :func:`skynet_os.identity.attest`'s reason and it is the same reason: the
    robot most in need of a check-in is the one that cannot measure everything. A report withheld
    for want of a field is a robot the Hub believes is offline.
    """
    pending = [
        {
            "stage": _stage_of(entry),
            "deferred": _is_true(entry.get("deferred")),
            "reason": _text(entry.get("reason")),
        }
        for entry in ((case_input.get("progress") or {}).get("pending") or [])
    ]
    pending.sort(key=lambda entry: _utf16_key(entry["stage"]))

    return {
        "claim": claim_for(case_input.get("progress") or {}),
        "health": summarize(case_input.get("health") or {}),
        "attestation": case_input.get("attestation") or {},
        "device_id": _text(case_input.get("device_id")),
        "agent": _text(case_input.get("agent")),
        "contracts": _text(case_input.get("contracts")),
        "state": _text(case_input.get("state")),
        "pending": pending,
        "desired_digest": _text(case_input.get("desired_digest")),
        # `get_present`, not `get`: a `limits` key that is there and null is an edition reporting no
        # report, which lands in the same place as one that does not report them at all.
        "limits": _guard_report(case_input.get("limits")),
    }


def summary(built: dict[str, Any]) -> str:
    """The one-line summary a console and a terminal both render."""
    report = attest(built["attestation"])
    who = "This robot" if built["device_id"] == "" else built["device_id"]
    health = built["health"]
    state = (
        f"degraded ({', '.join(health['stalled'])} alive and producing nothing)"
        if health["degraded"]
        else "no package is alive and producing nothing"
    )
    return (
        f"{who} reports generation {_js_number(built['claim']['generation'])} as "
        f"{built['claim']['claim']}, {state}. {report['sentence']}"
    )


# ── The wire shapes ──────────────────────────────────────────────────────────


def claim_to_json(case_input: dict[str, Any]) -> Any:
    """``checkin/claim`` — the field §17's rule lives or dies on."""
    verdict = claim_for(case_input)
    return {
        "applied": verdict["applied"],
        "blocking": list(verdict["blocking"]),
        "claim": verdict["claim"],
        "detail": verdict["detail"],
        "generation": verdict["generation"],
    }


def _health_json(report: dict[str, Any]) -> dict[str, Any]:
    """§33.6's health, rolled up.

    ``fields`` carries **every** name in :data:`HEALTH_FIELDS`, present and null where there is no
    dated reading. That is load-bearing rather than tidy: a console iterating the five names §33.6
    lists must not have to discover which of them a particular robot happened to measure, and a key
    that came and went would make "unmeasured" a question about key presence.
    """
    fields: dict[str, Any] = {}
    for name in HEALTH_FIELDS:
        held = report["fields"].get(name)
        fields[name] = None if held is None else {"age_ms": held["age_ms"], "value": held["value"]}
    return {
        "degraded": report["degraded"],
        "fields": fields,
        "packages": [
            {
                "age_ms": entry["age_ms"],
                "id": entry["id"],
                "live": entry["live"],
                "readiness": entry["readiness"],
            }
            for entry in report["packages"]
        ],
        "stalled": list(report["stalled"]),
        "undated": list(report["undated"]),
        "unmeasured": list(report["unmeasured"]),
    }


def health_to_json(case_input: dict[str, Any]) -> Any:
    """``checkin/health`` — §33.6's age rule."""
    return _health_json(summarize(case_input))


def _guard_json(report: dict[str, Any] | None) -> Any:
    """§7.6's guard report, as a check-in carries it."""
    if report is None:
        return None
    return {
        "active": list(report["active"]),
        "base_velocity_max_m_s": report["base_velocity_max_m_s"],
        "enforced": report["enforced"],
        "enforcement": report["enforcement"],
        "joint_torque_max_nm": report["joint_torque_max_nm"],
        "joint_velocity_max_rad_s": report["joint_velocity_max_rad_s"],
        "max_delta_per_cycle": report["max_delta_per_cycle"],
        "self_collision_margin_m": report["self_collision_margin_m"],
        "min_control_hz": report["min_control_hz"],
        "notices": [
            {"code": notice["code"], "subjects": list(notice["subjects"])}
            for notice in report["notices"]
        ],
        "resolved_at": report["resolved_at"],
        "source": report["source"],
        "unavailable": [
            {"guard": entry["guard"], "missing": list(entry["missing"])}
            for entry in report["unavailable"]
        ],
        # Always present, empty included: a field omitted where it is empty would let an edition
        # that never emits it pass this frozen document (§31.6).
        "inert": [
            {"guard": entry["guard"], "reading": entry["reading"]}
            for entry in report.get("inert", [])
        ],
    }


def document_to_json(case_input: dict[str, Any]) -> Any:
    """``checkin/document`` — the bytes that actually cross to the Hub.

    Frozen as one document rather than as its parts, because this is the artifact §17 describes and
    the one two editions must not differ over. A corpus that pinned the claim and the health
    separately would let an edition agree about both and still send different bytes.
    """
    built = check_in(case_input)
    claim = built["claim"]
    return {
        "agent": built["agent"],
        # §17's field, first among the ones a reader looks for. It is `false` for three of the four
        # claims, and the claim beside it says which — because "not applied" covers a robot that is
        # working, a robot that is waiting for a person, and a robot whose supervisor disagrees with
        # itself.
        "applied": claim["applied"],
        "attestation": attestation_block(built["attestation"]),
        "blocking": list(claim["blocking"]),
        "claim": claim["claim"],
        "contracts": built["contracts"],
        "desired_digest": built["desired_digest"],
        "detail": claim["detail"],
        "device_id": built["device_id"],
        "generation": claim["generation"],
        "health": _health_json(built["health"]),
        # Present-and-null so a consumer can tell "this edition does not report them" from "this
        # robot resolved none".
        "limits": _guard_json(built["limits"]),
        "pending": [
            {"deferred": entry["deferred"], "reason": entry["reason"], "stage": entry["stage"]}
            for entry in built["pending"]
        ],
        "state": built["state"],
    }
