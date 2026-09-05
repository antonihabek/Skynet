"""§12, §34.3 and §79's transfers, Python edition.

Transcribed from ``reference/typescript/bundle.ts`` (fetching an OS image, and checking what
arrived) and ``reference/typescript/upload.ts`` (sending a sealed black box window, and checking
what the receiver ended up with). Every rule and message string is read from those files, never from
``corpus/``.

The two are mirror images and the whole of the difference is one sentence from §79.2:

    §75's fetch has a rule that makes progress free: *"the partial's length is the progress"*. An
    upload cannot have that rule, because **the partial is on the other machine.**

Three consequences follow, and they are why an upload is a separate function rather than a flag:

1. **There is a probe.** A decision this side cannot reach without asking. An edition that folded
   it into "start" would re-send whole windows.
2. **The offset is a claim, not evidence.** It places the next byte and never concludes a transfer;
   that is the digest's job.
3. **A failure cannot discard anything.** ``check_bundle`` deletes a file the robot owns;
   ``check_upload`` can only stop believing an offset and start again at zero. The robot has no
   authority over the receiver's disk, and a contract that pretended otherwise would be one whose
   remedy nothing could carry out.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``Math.trunc``
    Truncates **toward zero**; Python's ``//`` floors, so ``-1.5`` becomes ``-1`` in the reference
    and ``-2`` under a naive edition. Both are then clamped at zero here, so the difference is
    invisible on every input that reaches this file — which is exactly the kind of agreement that
    stops being true when somebody changes the clamp.

``Number.isFinite``
    False for a boolean, and the size guards test it before comparing. A declared size of ``true``
    is not a size, and ``isinstance(True, int)`` would make it one.

template literals
    ``String(1024)`` is ``"1024"`` and ``str(1024.0)`` is ``"1024.0"``. Every byte count in every
    sentence here goes through :func:`_js_number` for that reason; the corpus compares the sentences
    byte for byte.
"""

from __future__ import annotations

import math
from typing import Any, Final

__all__ = [
    "BUNDLE_CHECKS",
    "FETCH_DECISIONS",
    "UPLOAD_CHECKS",
    "UPLOAD_DECISIONS",
    "arrival_to_json",
    "check_bundle",
    "check_upload",
    "fetch_to_json",
    "plan_fetch",
    "plan_upload",
    "receipt_to_json",
    "upload_to_json",
]

FETCH_DECISIONS: Final = (
    "have", "fetch", "resume", "discard_and_fetch", "wait_for_unmetered", "no_bearer", "refuse",
)

BUNDLE_CHECKS: Final = ("match", "short", "long", "digest_mismatch")

UPLOAD_DECISIONS: Final = (
    "done", "probe", "start", "resume", "restart", "wait_for_unmetered", "no_bearer", "refuse",
)

UPLOAD_CHECKS: Final = ("complete", "short", "long", "digest_mismatch")


def _js_number(value: Any) -> str:
    """A number inside a template literal."""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float):
        if value != value:
            return "NaN"
        if value in (math.inf, -math.inf):
            return "Infinity" if value > 0 else "-Infinity"
        if value.is_integer() and abs(value) < 1e21:
            return str(int(value))
    return str(value)


def _is_finite(value: Any) -> bool:
    """``Number.isFinite`` — false for a boolean."""
    if isinstance(value, bool):
        return False
    return isinstance(value, (int, float)) and math.isfinite(value)


def _trunc(value: Any) -> int:
    """``Math.trunc`` — **toward zero**, which is not Python's floor division."""
    if not _is_finite(value):
        return 0
    return math.trunc(value)


def _at_least_zero(value: Any) -> int:
    """``Math.max(0, Math.trunc(value ?? 0))``."""
    return max(0, _trunc(0 if value is None else value))


def _text(value: Any) -> str:
    """``(value ?? '').trim()``."""
    return "" if value is None else str(value).strip()


def _bearer_name(bearer: dict[str, Any]) -> str:
    """``bearer.name ?? '?'`` — a bearer that will not name itself still gets a sentence."""
    name = bearer.get("name")
    return "?" if name is None else str(name)


# ── §12, §34.3: fetching an OS image ─────────────────────────────────────────


def plan_fetch(subject: dict[str, Any]) -> dict[str, Any]:
    """What to do about one OS image.

    The order of the tests is **the order of cost**. Having it already is free; refusing costs
    nothing; waiting for a link costs nothing; and only then is a transfer considered. A version
    that checked metering first would report a robot as waiting for an unmetered bearer to fetch an
    image it already has.
    """
    digest = _text(subject.get("digest"))
    partial = _at_least_zero(subject.get("partial"))
    size = subject.get("size")
    size = None if size is None else size

    if subject.get("have") is True:
        return {
            "decision": "have",
            "from": 0,
            "detail": (
                "This bundle is already on the disk and has been checked against the manifest. "
                "Nothing to fetch."
            ),
        }

    if digest == "":
        return {
            "decision": "refuse",
            "from": 0,
            "detail": (
                "This release names no bundle digest, so there is nothing to check the bytes "
                "against. §12 binds a bench pass to the exact bytes it was taken against, and "
                "bytes with no name cannot be bound to — so they are not fetched either."
            ),
        }

    bearer = subject.get("bearer")
    bearer = None if bearer is None else bearer
    if bearer is None:
        return {
            "decision": "no_bearer",
            "from": 0,
            "detail": (
                "No bearer is up, so there is nothing to fetch over. That is the ordinary state "
                "for most of a robot's life (§34.1) and nothing about the update is lost by it — "
                "the release manifest is already here and the transfer resumes when a link returns."
            ),
        }

    if bearer.get("metered") is True:
        declared = "declared" if bearer.get("metering_declared") is True else "assumed"
        return {
            "decision": "wait_for_unmetered",
            "from": 0,
            "detail": (
                f"An OS image is the largest bulk transfer this system has, and "
                f"`{_bearer_name(bearer)}` is {declared} metered. It waits for an unmetered link "
                "(§34.3). Nothing is lost by waiting, and a fleet that downloaded hundreds of "
                "megabytes per robot over cellular would find out at the end of the month."
            ),
        }

    # A partial at least as large as the whole thing is not a partial of this bundle. Resuming from
    # it would append this image's tail to something else's, and the mismatch would only surface
    # after the entire transfer — which on a poor link is hours.
    if size is not None and _is_finite(size) and size > 0 and partial >= size:
        return {
            "decision": "discard_and_fetch",
            "from": 0,
            "detail": (
                f"There are {_js_number(partial)} bytes here for a bundle the manifest says is "
                f"{_js_number(size)}. A partial cannot be larger than what it is part of, so this "
                "is left over from something else. It is discarded and the transfer starts again."
            ),
        }

    if partial > 0:
        of_size = "" if size is None else f" of {_js_number(size)}"
        return {
            "decision": "resume",
            "from": partial,
            "detail": (
                f"Resuming at {_js_number(partial)} bytes{of_size}. The partial is keyed to this "
                "bundle's digest, so it cannot be a different image's tail."
            ),
        }

    what = "the bundle" if size is None else f"{_js_number(size)} bytes"
    return {
        "decision": "fetch",
        "from": 0,
        "detail": f"Fetching {what} over `{_bearer_name(bearer)}`.",
    }


def check_bundle(arrived: dict[str, Any], expected: dict[str, Any]) -> dict[str, Any]:
    """Are these the bytes the signed manifest describes (§12)?

    Size is checked before the digest, and the reason is **the sentence rather than the verdict** —
    both refuse. A file short of its declared size is a transfer that stopped, which is ordinary and
    resumable; a file of exactly the right size whose digest is wrong is a different set of bytes.
    Reporting the first as a digest mismatch would send an engineer looking for tampering every time
    a link dropped.

    ``short`` keeps the file, because that is exactly what a resume is for. Everything else deletes
    it: bytes that will never hash correctly are bytes a later resume would append to for ever.
    """
    want = _text(expected.get("digest"))
    got = _text(arrived.get("digest"))
    size = expected.get("size")
    size = None if size is None else size
    actual = arrived.get("size")
    actual = 0 if actual is None else actual

    if size is not None and _is_finite(size) and actual < size:
        return {
            "code": "short",
            "ok": False,
            "discard": False,
            "detail": (
                f"{_js_number(actual)} of {_js_number(size)} bytes arrived. The transfer stopped "
                "early, which is what a link dropping looks like. The partial is kept and the next "
                "attempt resumes from it."
            ),
        }

    if size is not None and _is_finite(size) and actual > size:
        return {
            "code": "long",
            "ok": False,
            "discard": True,
            "detail": (
                f"{_js_number(actual)} bytes arrived for a bundle the manifest says is "
                f"{_js_number(size)}. More than was declared is not a transfer that stopped — it "
                "is a different artifact, or a proxy that served something else. Discarded."
            ),
        }

    if want == "" or got == "" or want != got:
        return {
            "code": "digest_mismatch",
            "ok": False,
            "discard": True,
            "detail": (
                "These bytes do not hash to what the signed release manifest says this bundle is. "
                "§12 binds every gate it applies to that digest, so bytes that are not it are not "
                "this release — do not install them, and find out where they came from. Discarded, "
                "because bytes that will never verify are bytes a resume would append to for ever."
            ),
        }

    return {
        "code": "match",
        "ok": True,
        "discard": False,
        "detail": "These bytes are the release the manifest describes.",
    }


# ── §79: sending a sealed window ─────────────────────────────────────────────


def plan_upload(subject: dict[str, Any]) -> dict[str, Any]:
    """What to do about one sealed window (§79, §34.3).

    The order of the tests is the order of cost, as in :func:`plan_fetch`. Having a receipt is free;
    refusing costs nothing; waiting for a link costs nothing; asking costs one small request; and
    only then are megabytes considered.
    """
    digest = _text(subject.get("digest"))
    size = _at_least_zero(subject.get("size"))

    if subject.get("receipt") is True:
        return {
            "decision": "done",
            "from": 0,
            "detail": (
                "The receiver has acknowledged this window by digest and the receipt is on the "
                "disk. Nothing to send."
            ),
        }

    if digest == "":
        return {
            "decision": "refuse",
            "from": 0,
            "detail": (
                "This window carries no digest, so there is no address to send it to and nothing "
                "to check the far side's answer against. §79.1 makes the digest the route, and "
                "bytes with no name have nowhere to go."
            ),
        }

    if size == 0:
        return {
            "decision": "refuse",
            "from": 0,
            "detail": (
                "This window is empty. A seal that produced no bytes is a fact about the recorder "
                "worth reporting (§77.2) and is not something to upload — a zero-length object "
                "under a digest would be an artifact an investigator could open and mistake for "
                "the window."
            ),
        }

    bearer = subject.get("bearer")
    bearer = None if bearer is None else bearer
    if bearer is None:
        return {
            "decision": "no_bearer",
            "from": 0,
            "detail": (
                "No bearer is up, so there is nothing to send over. That is the ordinary state for "
                "most of a robot's life (§34.1) and nothing is lost by it — the window is sealed "
                "on the disk and the incident naming it is already queued."
            ),
        }

    if bearer.get("metered") is True:
        declared = "declared" if bearer.get("metering_declared") is True else "assumed"
        return {
            "decision": "wait_for_unmetered",
            "from": 0,
            "detail": (
                f"A sealed window is tens of megabytes and `{_bearer_name(bearer)}` is {declared} "
                "metered, so it waits for an unmetered link (§34.3). The alarm does not wait: "
                "§79.3 puts the incident on any bearer that is up, so an operator already knows "
                "this recording exists and what it hashes to."
            ),
        }

    held = subject.get("held")
    held = None if held is None else held
    if held is None:
        return {
            "decision": "probe",
            "from": 0,
            "detail": (
                "The receiver has not been asked how much of this window it holds. §79.2: the "
                "partial is on the other machine, so an upload has a step a download does not — "
                "and a robot that skipped it would re-send the whole window every pass."
            ),
        }

    bytes_held = _at_least_zero(held.get("bytes"))
    held_digest = _text(held.get("digest"))

    if bytes_held >= size:
        if bytes_held == size and held_digest != "" and held_digest == digest:
            return {
                "decision": "done",
                "from": 0,
                "detail": (
                    f"The receiver holds all {_js_number(size)} bytes and says they hash to this "
                    "window's digest. Nothing is sent and a receipt is written — which is the "
                    "state a robot reaches after an upload whose acknowledgement was lost."
                ),
            }
        because = (
            ", and does not say what they hash to. A complete object nobody will name is not "
            "evidence this window arrived, and writing a receipt for it would record an upload "
            "that may never have happened"
            if bytes_held == size
            else ". A partial cannot be larger than what it is part of, so what is over there is "
            "not part of this window"
        )
        return {
            "decision": "restart",
            "from": 0,
            "detail": (
                f"The receiver claims {_js_number(bytes_held)} bytes for a window of "
                f"{_js_number(size)}{because}. It is sent again from the beginning."
            ),
        }

    if bytes_held > 0:
        return {
            "decision": "resume",
            "from": bytes_held,
            "detail": (
                f"Resuming at {_js_number(bytes_held)} of {_js_number(size)} bytes. The offset is "
                "the receiver's claim and is used to place the next byte, never to conclude the "
                "window arrived — that is the digest's job (§79.2)."
            ),
        }

    return {
        "decision": "start",
        "from": 0,
        "detail": f"Sending {_js_number(size)} bytes over `{_bearer_name(bearer)}`.",
    }


def check_upload(reported: dict[str, Any], window: dict[str, Any]) -> dict[str, Any]:
    """Did the window arrive (§79.2, §79.4)?

    The same four outcomes as :func:`check_bundle` and one different remedy, which is the asymmetry
    worth naming: a bundle that fails its check is **deleted**, because the robot owns those bytes.
    A window that fails this check cannot be deleted from anywhere — the robot can only decline to
    trust the offset it was given and begin again. ``restart`` is therefore about what *this* robot
    will do next, never about what the receiver should do with what it has.
    """
    want = _text(window.get("digest"))
    got = _text(reported.get("digest"))
    size = _at_least_zero(window.get("size"))
    bytes_held = _at_least_zero(reported.get("bytes"))

    if bytes_held < size:
        return {
            "code": "short",
            "ok": False,
            "restart": False,
            "detail": (
                f"The receiver holds {_js_number(bytes_held)} of {_js_number(size)} bytes. The "
                "transfer stopped early, which is what a link dropping looks like, and the next "
                "pass resumes from what is there."
            ),
        }

    if bytes_held > size:
        return {
            "code": "long",
            "ok": False,
            "restart": True,
            "detail": (
                f"The receiver holds {_js_number(bytes_held)} bytes for a window of "
                f"{_js_number(size)}. More than was sent is not a transfer that stopped — it is "
                "another object under this digest, or a proxy answering for one. The next attempt "
                "begins at zero; this robot cannot delete what is over there and does not pretend "
                "to."
            ),
        }

    if want == "" or got == "" or want != got:
        return {
            "code": "digest_mismatch",
            "ok": False,
            "restart": True,
            "detail": (
                "The receiver holds the right number of bytes and they are not this window's. The "
                "digest was recorded at the seal, before any transfer existed (§77.1), so it is "
                "the half of this comparison that is evidence. No receipt is written and the next "
                "attempt begins at zero."
            ),
        }

    return {
        "code": "complete",
        "ok": True,
        "restart": False,
        "detail": (
            "The receiver holds the whole window and it hashes to what was sealed. A receipt is "
            "written beside the recording; the recording itself is kept, because §39.1's local "
            "route is for the operator standing next to the robot (§79.4)."
        ),
    }


# ── The wire shapes ──────────────────────────────────────────────────────────
#
# These four are emitted directly rather than through a `shape*` projection: the plans and checks
# are already flat records of exactly the fields the corpus pins.


def fetch_to_json(case_input: dict[str, Any]) -> Any:
    """``transfer/fetch`` — what to do about one OS image."""
    plan = plan_fetch(case_input["subject"])
    return {"decision": plan["decision"], "from": plan["from"], "detail": plan["detail"]}


def arrival_to_json(case_input: dict[str, Any]) -> Any:
    """``transfer/arrival`` — are these the bytes the signed manifest describes?"""
    check = check_bundle(case_input["arrived"], case_input["expected"])
    return {
        "code": check["code"],
        "ok": check["ok"],
        "discard": check["discard"],
        "detail": check["detail"],
    }


def upload_to_json(case_input: dict[str, Any]) -> Any:
    """``transfer/upload`` — what to do about one sealed window."""
    plan = plan_upload(case_input["subject"])
    return {"decision": plan["decision"], "from": plan["from"], "detail": plan["detail"]}


def receipt_to_json(case_input: dict[str, Any]) -> Any:
    """``transfer/receipt`` — did the window arrive?"""
    check = check_upload(case_input["reported"], case_input["window"])
    return {
        "code": check["code"],
        "ok": check["ok"],
        "restart": check["restart"],
        "detail": check["detail"],
    }
