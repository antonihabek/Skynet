"""Provenance classes for on-robot evaluation results — §56, Python edition.

Transcribed from ``reference/typescript/provenance.ts``. Every rule and message string is read from
that file, never from ``corpus/``.

§56.1 states the problem in one sentence and the whole design follows from it:

    **A leaderboard fed by self-reported on-robot numbers is trivially gameable** — a publisher
    evaluating their own policy on their own robot, choosing their own seeds, and reporting their
    own score is not producing evidence.

    So an on-robot evaluation result is only worth as much as its attestation.

This module is *"only worth as much as its attestation"* made mechanical.

── The three classes, and why the worst one is accepted ─────────────────────

===============  ==========================================================  ==========================
Class            Means                                                       Leaderboard
===============  ==========================================================  ==========================
``field``        A real robot, valid birth certificate, attested image       Eligible
``simulated``    A simulator RHAL driver, which must declare itself           Eligible, separately ranked
``unattested``   No device signature, or a device the Hub does not know       **Accepted and never ranked**
===============  ==========================================================  ==========================

§56.2: *"``unattested`` is accepted rather than refused because a researcher on a bench robot has a
legitimate result worth recording — and it is never ranked, because it is not evidence."* That
distinction — **recorded** and **not ranked** — is the whole posture.

── The order of the checks is the anti-gaming argument ──────────────────────

Only one of the three classes is worth lying to obtain, and it is ``field``. So an untrustworthy
device signature is ``unattested`` **before anything else is looked at**, because a bundle nobody
can attribute makes claims about its own simulator flag, its own seeds and its own birth
certificate, and none of those claims is evidence either. A *declared* simulator is ``simulated``
and can never be ``field``, however perfect the rest of it is.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``bundle.blackbox_segment === null || === undefined``
    Both, and only those two: an **empty string** is a segment somebody recorded as empty rather
    than a missing one, and it does not raise ``not_replayable``. ``if not segment`` would collapse
    the three.

``a.every((seed, index) => seed === b[index])``
    Strict equality over a pair of lists after a length check. A Python ``==`` on two lists is the
    same answer here and gets there differently — it compares element by element with ``==``, which
    for ``"1"`` and ``1`` is ``False`` in both languages but for ``True`` and ``1`` is ``True`` in
    Python and ``false`` in JavaScript. Seeds are strings, so this is written out rather than left
    to a coincidence.

``String(expectedSeeds.length)``
    A count in a template literal. Both are integers, so this is ``str(int)`` and not
    :func:`skynet_os.js.js_number` — but the two disagree the moment a length is ever a float, which
    is why the conversion is explicit.

``${onRobot}`` in the disagreement sentence
    ECMAScript ``ToString(Number)``: ``0.875`` renders as ``0.875`` and ``50`` as ``50`` where
    Python's ``str`` gives ``50.0``. :func:`skynet_os.js.js_number` is that spelling.

── A ``null`` score is a ``NaN`` score ──────────────────────────────────────

``scores.on_robot`` is typed ``number`` in the reference — not ``number | null`` — so a ``null`` in
the case file did not come from a caller. It came from the generator, which writes every input
through ``JSON.stringify``, and ``JSON.stringify(NaN)`` is ``null``.

That makes the reading forced rather than a choice: in a field the reference declares as a bare
``number``, a ``null`` can only be a non-finite value the format cannot spell. Reading it as an
absence instead would have this edition answer a different question from the one the case's
expectation was computed against — which is the hazard ``tools/corpus-gen/generate.ts`` names at
the frame-bindings rule and refuses to create there.

The rule does **not** generalise to every null. Where the reference's own type admits one —
``Frame.worstSkewUs: number | null`` is the clearest — a ``null`` means null, and reading it as NaN
would be the same mistake pointing the other way. The type is what decides, and
``corpus/README.md`` now says so.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .js import js_number as _js_number

__all__ = [
    "PROVENANCE_CLASSES",
    "PROVENANCE_REASON_CODES",
    "blocks_field",
    "classify",
    "classify_to_json",
    "explain",
    "explain_disagreement",
    "rankable",
    "ranked_separately",
    "replayable",
    "scores_agree",
    "scores_to_json",
]

#: §56.2's classes, ordered strongest to weakest.
#:
#: Not sorted: the order **is** the ranking, and an edition that sorted these alphabetically would
#: put ``field`` after ``unattested``.
PROVENANCE_CLASSES: Final = ("field", "simulated", "unattested")

#: Every reason code a classification can carry, in the order the checks run.
PROVENANCE_REASON_CODES: Final = (
    "device_signature",
    "device_unknown",
    "simulator_declared",
    "birth_unattested",
    "image_unattested",
    "seeds_not_supplied",
    "benchmark_mismatch",
    "not_replayable",
)


def rankable(provenance: str) -> bool:
    """Whether a result of this class appears on a leaderboard at all."""
    return provenance in ("field", "simulated")


def ranked_separately(provenance: str) -> bool:
    """Whether it is ranked in its own table rather than against field results.

    §56.2 says *"eligible, separately ranked"* and the separation is not a courtesy. A simulated run
    and a field run are not comparable numbers, and putting them in one table would make the
    leaderboard a worse instrument than no leaderboard.
    """
    return provenance == "simulated"


def blocks_field(reason: dict[str, Any]) -> bool:
    """Whether this reason is why the bundle cannot be ``field``.

    Everything except ``not_replayable``: a result can be perfectly attested and unreplayable.
    """
    return reason["code"] != "not_replayable"


def _subject(reason: dict[str, Any], index: int) -> str:
    """``reason.subjects[n] ?? ''`` — an out-of-range index is ``undefined``, not an exception."""
    subjects = reason.get("subjects") or []
    return subjects[index] if index < len(subjects) else ""


def explain(reason: dict[str, Any]) -> str:
    """The sentence a publisher gets."""
    code = reason["code"]
    if code == "device_signature":
        return (
            f"The device's signature over this bundle is {_subject(reason, 0)}, so nothing in it "
            "is attributable to a machine. The result is recorded and is not ranked: a number "
            "nobody can attribute is not evidence, which is the whole of §56.1."
        )
    if code == "device_unknown":
        return (
            "The signature verifies and this Hub does not know the device that made it. Nothing "
            "is wrong with the bytes; what is missing is a registered machine to attribute them "
            "to. Register the device, then re-submit."
        )
    if code == "simulator_declared":
        return (
            "The RHAL driver declared itself a simulator (§35.3), so this is a simulated result "
            "and is ranked on the simulated table. This is the driver meeting its obligation "
            "rather than a fault — and a simulated run and a field run are not comparable numbers, "
            "which is why the tables are separate."
        )
    if code == "birth_unattested":
        return (
            "This robot's birth certificate does not attest it (§50), so \"a real robot of this "
            'model with these joint ratings" is a claim the machine makes about itself. The result '
            "is recorded and not ranked as a field result."
        )
    if code == "image_unattested":
        return (
            "The OS image running during this evaluation was not attested, so what produced these "
            "numbers is not established. Recorded, not ranked as a field result."
        )
    if code == "seeds_not_supplied":
        # The counts rather than the seeds: a benchmark can carry hundreds, and a sentence that
        # pasted both lists would be one nobody reads.
        return (
            f"This run used seeds the benchmark did not supply ({_subject(reason, 0)} given, "
            f"{_subject(reason, 1)} used). §56.3: seeds come from the benchmark and are never "
            "generated by the robot, because a robot that picks its own seeds picks easy ones — "
            "and it need not do so dishonestly for the number to be worthless. This is not a run "
            "of that benchmark."
        )
    if code == "benchmark_mismatch":
        return (
            f"This bundle is a run of benchmark {_subject(reason, 1)} and is being ranked against "
            f"{_subject(reason, 0)}. Two benchmarks are two questions, and a score against one "
            "says nothing about the other."
        )
    return (
        "No black box segment accompanies this run, so a disputed result cannot be replayed "
        "(§56.4). The class is unaffected — a result can be perfectly attested and unreplayable — "
        "but a number with a recording behind it is a different class of claim from a number in a "
        "table."
    )


def replayable(classified: dict[str, Any]) -> bool:
    """Whether the run can be replayed from its recording (§56.4)."""
    return not any(reason["code"] == "not_replayable" for reason in classified["reasons"])


def _same_seeds(a: list[Any], b: list[Any]) -> bool:
    return len(a) == len(b) and all(seed is b[index] or seed == b[index] for index, seed in enumerate(a))


def classify(
    bundle: dict[str, Any], expected_benchmark: Any, expected_seeds: list[Any]
) -> dict[str, Any]:
    """Classify a run bundle against the benchmark it claims to be a run of (§56.2).

    ``expected_benchmark`` and ``expected_seeds`` come from the benchmark manifest, **not** from the
    bundle. Passing the bundle's own values would make §56.3 a check a robot performs on itself,
    which is the shape of every problem in §56.1.
    """
    reasons: list[dict[str, Any]] = []

    # §56.4, first because it is the one observation that never changes the class. Collected up
    # front so the reason list reads in the order a person would ask the questions.
    if bundle.get("blackbox_segment") is None:
        reasons.append({"code": "not_replayable", "subjects": []})

    signature = bundle.get("device_signature")
    if signature != "verified":
        reasons.append({"code": "device_signature", "subjects": [signature]})
    elif bundle.get("device_known") is not True:
        reasons.append({"code": "device_unknown", "subjects": []})

    if bundle.get("benchmark_digest") != expected_benchmark:
        reasons.append(
            {
                "code": "benchmark_mismatch",
                "subjects": [expected_benchmark, bundle.get("benchmark_digest")],
            }
        )

    # Order and membership both. Two runs over the same seeds in a different order are two different
    # sequences of episodes, and a benchmark that fixes its seeds is fixing the sequence.
    seeds = list(bundle.get("seeds") or [])
    if not _same_seeds(seeds, list(expected_seeds)):
        reasons.append(
            {
                "code": "seeds_not_supplied",
                "subjects": [str(len(expected_seeds)), str(len(seeds))],
            }
        )

    if bundle.get("simulator"):
        reasons.append({"code": "simulator_declared", "subjects": []})
    else:
        if not bundle.get("birth_attested"):
            reasons.append({"code": "birth_unattested", "subjects": []})
        if not bundle.get("image_attested"):
            reasons.append({"code": "image_unattested", "subjects": []})

    # The order of these two branches is the anti-gaming argument, and it is the one thing in this
    # file that must not be rearranged for tidiness.
    #
    # A bundle nobody can attribute is `unattested` **before** its simulator flag is consulted,
    # because that flag is one of its own claims. And a *declared* simulator is `simulated` even
    # when everything else about it is perfect, because the one result this scheme exists to
    # prevent is a simulator on the field leaderboard.
    attributable = signature == "verified" and bundle.get("device_known") is True

    if not attributable:
        provenance = "unattested"
    elif bundle.get("simulator"):
        provenance = "simulated"
    elif any(blocks_field(reason) for reason in reasons):
        provenance = "unattested"
    else:
        provenance = "field"

    return {"class": provenance, "reasons": reasons}


def scores_agree(on_robot: Any, on_hub: Any) -> bool:
    """Whether two roll-ups of the same run agree (§56.5).

    An equality test with a tolerance, and the tolerance exists only for the floating-point
    **representation** rather than for the arithmetic: ``1e-9``, which no genuine scoring difference
    can hide under. A caller tempted to widen it is a caller papering over the divergence §31.4
    exists to catch.
    """
    # A non-finite score is not a score, and two of them are not an agreement. Returning true for
    # `NaN === NaN` would report a robot and a Hub that both failed to compute anything as being in
    # perfect accord.
    if not _finite(on_robot) or not _finite(on_hub):
        return False
    return abs(on_robot - on_hub) <= 1e-9


def _finite(value: Any) -> bool:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return False
    number = float(value)
    return not (math.isnan(number) or math.isinf(number))


def _score(value: Any) -> float:
    """A score off the wire. A ``null`` in a field typed ``number`` is a NaN — see the header."""
    return float("nan") if not _finite(value) else float(value)


def explain_disagreement(on_robot: Any, on_hub: Any) -> str:
    """The sentence for a disagreement (§56.5)."""
    return (
        f"The robot rolled this run up to {_js_number(on_robot)} and the Hub to "
        f"{_js_number(on_hub)}. The Hub's number is authoritative, and the difference is a defect "
        "rather than a rounding artefact: both sides run the same scoring contract over the same "
        "per-episode outcomes, so they disagreeing means the implementations diverged — which is "
        "precisely what the conformance corpus (§31.4) exists to catch."
    )


# ── The wire shapes ──────────────────────────────────────────────────────────


def classify_to_json(case_input: dict[str, Any]) -> Any:
    """``provenance/classify`` — §56.2's class, and every reason it is not a better one."""
    classified = classify(
        case_input["bundle"],
        case_input["expected_benchmark"],
        list(case_input.get("expected_seeds") or []),
    )
    return {
        "class": classified["class"],
        "rankable": rankable(classified["class"]),
        "ranked_separately": ranked_separately(classified["class"]),
        "reasons": [
            {
                "blocks_field": blocks_field(reason),
                "code": reason["code"],
                "message": explain(reason),
                "subjects": list(reason["subjects"]),
            }
            for reason in classified["reasons"]
        ],
        "replayable": replayable(classified),
    }


def scores_to_json(case_input: dict[str, Any]) -> Any:
    """``provenance/scores`` — §56.5's roll-up comparison.

    The sentence travels beside the boolean rather than being left to the caller, because §56.5's
    whole point is that a disagreement is a defect to investigate — and a bare ``false`` is the
    shape of a thing somebody quietly widens a tolerance around.
    """
    scores = case_input["scores"]
    # `null` is how this file spells a non-finite score: the field is typed `number` in the
    # reference, so a null in it came from `JSON.stringify(NaN)` rather than from a caller. See the
    # module header, and `corpus/README.md` for the rule this is an instance of.
    on_robot = _score(scores.get("on_robot"))
    on_hub = _score(scores.get("on_hub"))
    agree = scores_agree(on_robot, on_hub)
    return {"agree": agree, "detail": None if agree else explain_disagreement(on_robot, on_hub)}
