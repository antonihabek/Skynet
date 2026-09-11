"""§11's pull — what a robot does with a desired state it has just been handed, Python edition.

Transcribed from ``reference/typescript/intent.ts``. Every rule and message string is read from that
file, never from ``corpus/``.

    **The Hub stores intent. The robot reconciles.** Never push.

The document arrives over a network, which makes it the least trusted input in the system that is
also the most consequential: it decides what software runs on a machine that moves.

**Refusing is not the same as ignoring.** Two of the four findings refuse the offer, and in both
cases the robot keeps reconciling against what it already has — the *ordinary* state rather than a
failure mode, because §34.1 assumes the robot is offline most of the time. A refused offer costs
nothing that being offline does not.

── The two asymmetric calls, which are the whole module ─────────────────────

**A reused generation is refused.** A Hub serving two different documents under one generation has
broken the property §11 rests idempotence on, and the robot is the only party that can see it.
Adopting would have the robot running new software while reporting the old generation as applied,
and a fleet console would show it converged — an invisible wrong state. Refusing produces a visible,
actionable one.

**A *lower* generation is adopted.** The opposite call, and for a reason worth writing down: a
generation that goes backwards is a Hub restored from a backup, and a robot that refused to follow
one would be a fleet an operator could not recover, permanently, with no remedy short of visiting
every machine.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``offer.decoded !== true``
    Strict, and it is the first gate. A truthy non-boolean does not make a response a desired state.

``(offer.packages ?? 0) === 0``
    Nullish, so a declared ``0`` and an absent field are the same thing here — which is right, and
    is *not* right two lines above where ``digest`` distinguishes absent from empty.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .js import js_trim as _js_trim

__all__ = ["OFFER_FAULTS", "assess_offer", "offer_to_json"]

OFFER_FAULTS: Final = (
    "undecodable",
    "generation_reused",
    "generation_went_backwards",
    "empties_the_robot",
)


def _fault(code: str, severity: str, message: str) -> dict[str, Any]:
    return {"code": code, "severity": severity, "message": message}


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


def _or(value: Any, default: Any) -> Any:
    """``value ?? default``."""
    return default if value is None else value


def assess_offer(offer: dict[str, Any], held: dict[str, Any]) -> dict[str, Any]:
    """May this response become the desired state (§11, §17, §33.8)?

    Every check runs and the result carries all of them: a fleet operator diagnosing why a robot
    will not take an update should get one list rather than one problem per poll.
    """
    faults: list[dict[str, Any]] = []
    held_generation = _or(held.get("generation"), 0)
    held_digest = _js_trim(str(_or(held.get("digest"), "")))
    offered = _or(offer.get("generation"), 0)
    digest = _js_trim(str(_or(offer.get("digest"), "")))

    if offer.get("decoded") is not True:
        why = _js_trim(str(_or(offer.get("decode_error"), "")))
        because = "" if why == "" else f": {why}"
        faults.append(
            _fault(
                "undecodable",
                "error",
                f"This response is not a desired state{because}. It has not been written to disk, "
                "and this robot carries on reconciling against what it already has — which is what "
                "it does whenever it cannot reach the Hub at all (§34.1), so nothing about the "
                "robot is worse for it.",
            )
        )
        return {
            "adopt": False,
            "changed": False,
            "generation": held_generation,
            "faults": faults,
        }

    # The reused generation, and the reason it refuses. See the header.
    if offered == held_generation and digest != "" and held_digest != "" and digest != held_digest:
        faults.append(
            _fault(
                "generation_reused",
                "error",
                f"The Hub is serving a different document under generation {_js_number(offered)}, "
                "which this robot has already applied. §11 rests idempotence on that counter, so "
                "adopting this would have the robot running new software while reporting the old "
                "generation as applied — and a fleet console would show it converged. Bump the "
                "generation on the Hub and this robot will take it on the next poll.",
            )
        )
        return {
            "adopt": False,
            "changed": False,
            "generation": held_generation,
            "faults": faults,
        }

    if offered < held_generation:
        faults.append(
            _fault(
                "generation_went_backwards",
                "warning",
                f"This robot has applied generation {_js_number(held_generation)} and the Hub is "
                f"asking for {_js_number(offered)}. Intent has gone backwards, which is what a Hub "
                "restored from a backup looks like. It is adopted — a robot that refused to follow "
                "one would be a fleet nobody could recover — and it is reported, because it is "
                "worth somebody knowing once.",
            )
        )

    running = _or(held.get("running"), 0)
    if _or(offer.get("packages"), 0) == 0 and running > 0:
        faults.append(
            _fault(
                "empties_the_robot",
                "warning",
                f"This desired state names no packages and this robot is running "
                f"{_js_number(running)}. That is what an operator emptying a robot looks like, and "
                "it is also what a Hub serving a stub looks like. It is adopted, because stopping "
                "is never the dangerous direction (§33.4) — and it is said out loud, because the "
                "two are indistinguishable from here.",
            )
        )

    return {
        "adopt": True,
        "changed": digest != held_digest,
        "generation": offered,
        "faults": faults,
    }


def offer_to_json(case_input: dict[str, Any]) -> Any:
    """``intent/offer`` — may this response become the desired state?"""
    verdict = assess_offer(case_input["offer"], case_input["held"])
    return {
        "adopt": verdict["adopt"],
        "changed": verdict["changed"],
        "faults": [
            {"code": f["code"], "message": f["message"], "severity": f["severity"]}
            for f in verdict["faults"]
        ],
        "generation": verdict["generation"],
    }
