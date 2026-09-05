"""§10, §19 and §50's identity, Python edition.

Transcribed from ``reference/typescript/identity.ts``. Every rule, code and message string is read
from that file, never from ``corpus/``.

Three questions in one module: what somebody typed when they meant a pairing code, what state this
robot's credential is in, and what it is actually running.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``String.prototype.length``
    Counts **UTF-16 code units**; ``len`` counts code points. They disagree for every character
    above U+FFFF, which arrives as a surrogate *pair*. Eight astral characters are a code of length
    **16** to the reference — refused as ``too_long`` — and of length 8 to a naive edition, which
    then refuses them one rule later as ``not_in_alphabet`` at position 1. Two different faults, two
    different sentences, and both look correct in isolation. :func:`_utf16_length` and
    :func:`_utf16_at` keep the reference's arithmetic.

``Array.prototype.sort``
    Orders by UTF-16 code unit, and ``sorted`` orders by code point — the same disagreement,
    reached from the other end. ``digest.py``'s header works the ordering through in full; the
    unrecognised-scope list is sorted the same way for the same reason.

``value === true``
    ``credential``, ``birth_certificate`` and ``binding`` are all tested strictly, and the last one
    is the important one: *absent is "nobody looked"*. A robot that has not been asked whether it
    has a secure element has not established that it lacks one, and §10's
    ``security.identity.attested`` is a measurement rather than an assumption.

``String.prototype.trim``
    Strips a specific set the ECMAScript specification names. Python's ``strip()`` also removes the
    C1 separators ``\\x1c``–``\\x1f``, which the reference leaves in place — so a namespace of
    ``"\\x1c"`` is empty to Python and non-empty to the reference. :func:`_js_trim` is the
    reference's set.
"""

from __future__ import annotations

from typing import Any, Final

__all__ = [
    "CODE_FAULTS",
    "DEVICE_SCOPES",
    "DEVICE_TOKEN_PREFIX",
    "IDENTITY_FINDINGS",
    "KEY_BINDINGS",
    "MEASURE_STATES",
    "PAIRING_ALPHABET",
    "PAIRING_CODE_GROUP",
    "PAIRING_CODE_LENGTH",
    "attest",
    "attestation_block",
    "attestation_to_json",
    "check_identity",
    "code_to_json",
    "format_pairing_code",
    "identity_to_json",
    "normalize_pairing_code",
]

#: Crockford's alphabet: no I, L, O or U. The excluded letters are not rejected on input — they are
#: **folded**, because an operator who types the letter they saw should get the code they saw.
PAIRING_ALPHABET: Final = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"

#: Eight. Forty bits, which is a large number of guesses against a code that lives for minutes
#: behind a Hub that rate-limits, and a small number of characters for somebody reading them off a
#: screen — which is the trade the length is actually making.
PAIRING_CODE_LENGTH: Final = 8

#: How a pairing code is written for a person to read: two groups of four.
PAIRING_CODE_GROUP: Final = 4

#: The prefix on a Hub device credential (§10), here so a credential pasted into `skynet pair` is
#: recognised as one and refused by name.
DEVICE_TOKEN_PREFIX: Final = "skd_"

CODE_FAULTS: Final = (
    "empty", "looks_like_a_credential", "too_short", "too_long", "not_in_alphabet",
)

#: §17's and §57's device-facing routes, one scope each. **The direction is §19's and the vocabulary
#: is not**: this is an allow-list because a deny-list of §19's three phrases would be got past by a
#: credential that spelled repository read any other way, and the one thing §19 says about this
#: credential is that it is *assumed lost*.
DEVICE_SCOPES: Final = (
    "device:calibration",
    "device:checkin",
    "device:conformance",
    "device:desired",
    "device:episodes",
    "device:evaluations",
    "device:incidents",
    "device:releases",
)

KEY_BINDINGS: Final = ("hardware", "software")

IDENTITY_FINDINGS: Final = (
    "not_paired",
    "namespace_missing",
    "not_a_device_credential",
    "scope_unrecognised",
    "scope_undeclared",
    "key_binding_unknown",
    "key_not_hardware_bound",
    "key_not_attested",
    "no_birth_certificate",
)

#: ``nothing`` and ``unmeasured`` are kept apart: "there is no policy running" and "a policy is
#: running and its digest could not be read" are two facts with two different people at the end of
#: them, and collapsing them would report a robot running unknown software as one running none.
MEASURE_STATES: Final = ("known", "nothing", "unmeasured")

#: ECMAScript's ``WhiteSpace`` plus ``LineTerminator``. Python's ``strip()`` removes more.
_JS_WHITESPACE: Final = (
    "\t\n\v\f\r             "
    "     　﻿"
)


def _js_trim(value: str) -> str:
    """``String.prototype.trim`` — the specification's set, not Python's larger one."""
    return value.strip(_JS_WHITESPACE)


def _utf16_units(value: str) -> list[int]:
    """The string as UTF-16 code units, which is what JavaScript indexes and measures."""
    encoded = value.encode("utf-16-be", errors="surrogatepass")
    return [(encoded[i] << 8) | encoded[i + 1] for i in range(0, len(encoded), 2)]


def _utf16_length(value: str) -> int:
    """``value.length``. Eight emoji are sixteen of these."""
    return len(_utf16_units(value))


def _utf16_at(value: str, index: int) -> str:
    """``value[index]`` — one code unit, which for an astral character is half of one."""
    unit = _utf16_units(value)[index]
    return chr(unit) if not 0xD800 <= unit <= 0xDFFF else "�"


def _utf16_key(value: str) -> tuple[int, ...]:
    """A sort key ordering by UTF-16 code unit, which is what JavaScript's ``<`` does."""
    return tuple(_utf16_units(value))


def _fold(character: str) -> str:
    """Crockford's folding: the letters excluded from the alphabet map to the digits they resemble."""
    return {"O": "0", "I": "1", "L": "1"}.get(character, character)


def normalize_pairing_code(value: str) -> dict[str, Any]:
    """Turn what somebody typed into a pairing code, or say why it is not one.

    The order of the checks is the order of usefulness, and it is not the order they would fall out
    of a parser. A credential pasted here is caught **before** anything is stripped, because ``skd_``
    contains an underscore and would otherwise be reported as an unusable character at position four
    — true, useless, and it would have the operator hunting for a typo in a string that has none.
    Length is checked before the alphabet for the same reason: somebody who typed seven characters
    needs to be told they typed seven.
    """
    if _js_trim(value).lower().startswith(DEVICE_TOKEN_PREFIX):
        return {
            "code": None,
            "fault": {
                "code": "looks_like_a_credential",
                "detail": (
                    f"This is a device credential — it begins `{DEVICE_TOKEN_PREFIX}` — and "
                    "pairing takes the short code the Hub showed you, not the credential it hands "
                    "back afterwards. Nothing has been sent. Treat the credential as exposed if it "
                    "has been on a screen somebody else could see."
                ),
                "position": None,
            },
        }

    normalized = ""
    for character in value:
        if character in ("-", " ", "\t", "\n", "\r"):
            continue
        normalized += _fold(character.upper())

    if normalized == "":
        return {
            "code": None,
            "fault": {
                "code": "empty",
                "detail": (
                    "No code was given. `skynet pair` takes the eight-character code the Hub "
                    f"showed you, written as {'X' * PAIRING_CODE_GROUP}-{'X' * PAIRING_CODE_GROUP}."
                ),
                "position": None,
            },
        }

    length = _utf16_length(normalized)
    if length != PAIRING_CODE_LENGTH:
        return {
            "code": None,
            "fault": {
                "code": "too_short" if length < PAIRING_CODE_LENGTH else "too_long",
                "detail": (
                    f"A pairing code is {PAIRING_CODE_LENGTH} characters and this is {length}. "
                    "Hyphens and spaces do not count towards it."
                ),
                "position": None,
            },
        }

    for index in range(length):
        if _utf16_at(normalized, index) not in PAIRING_ALPHABET:
            return {
                "code": None,
                "fault": {
                    "code": "not_in_alphabet",
                    "detail": (
                        f"Character {index + 1} of the {PAIRING_CODE_LENGTH} is not one a pairing "
                        "code uses. Codes are digits and upper-case letters, without I, L, O or U "
                        "— if you read one of those off the screen it was a 1 or a 0, and typing "
                        "it that way works too."
                    ),
                    "position": index + 1,
                },
            }

    return {"code": normalized, "fault": None}


def format_pairing_code(code: str) -> str:
    """Render a normalized code the way it is printed for a person."""
    return "-".join(
        code[index : index + PAIRING_CODE_GROUP]
        for index in range(0, len(code), PAIRING_CODE_GROUP)
    )


# ── §19's device credential ──────────────────────────────────────────────────


def _finding(
    code: str, severity: str, message: str, subjects: list[str] | None = None
) -> dict[str, Any]:
    return {
        "code": code,
        "severity": severity,
        "message": message,
        "subjects": [] if subjects is None else subjects,
    }


def check_identity(identity: dict[str, Any], observed: dict[str, Any]) -> dict[str, Any]:
    """What state this robot's identity is in, and everything wrong with it.

    Every check runs and the result carries all of them: an operator standing in front of a robot
    fixing one problem per attempt is the failure a finding list exists to prevent, and pairing is a
    job somebody drove to a warehouse to do.

    **Not paired is not a fault.** §50.2 ships a robot with a birth certificate and no namespace, so
    an unpaired robot is at the correct point in its lifecycle and is reported as a warning naming
    the command rather than as something broken.
    """
    findings: list[dict[str, Any]] = []
    credential = identity.get("credential") is True
    namespace = _js_trim(identity.get("namespace") or "")

    if not credential:
        findings.append(
            _finding(
                "not_paired",
                "warning",
                "This robot holds no device credential, so it belongs to nobody and reports to "
                "nothing. That is the state a robot ships in (§50.2). Pair it with `skynet pair` "
                "and the code the Hub showed you.",
            )
        )
    else:
        if namespace == "":
            findings.append(
                _finding(
                    "namespace_missing",
                    "error",
                    "This robot holds a credential and names no namespace. §10 pairs a robot to a "
                    "namespace, so a credential without one has bound it to nothing — and every "
                    "report it sends is addressed to a fleet nobody can name.",
                )
            )

        prefix = identity.get("credential_prefix")
        prefix = "" if prefix is None else prefix
        if prefix != DEVICE_TOKEN_PREFIX:
            shown = "with nothing recognisable" if prefix == "" else f"`{prefix}`"
            findings.append(
                _finding(
                    "not_a_device_credential",
                    "error",
                    f"A device credential begins `{DEVICE_TOKEN_PREFIX}` and this one begins "
                    f"{shown}. §10 puts the narrowest credential class in the system on the robot "
                    "on purpose, because a credential physically present in the field is the "
                    "easiest one to lose. A wider one here is a wider one lost.",
                )
            )

        scopes = identity.get("scopes")
        scopes = None if scopes is None else scopes
        if scopes is None:
            findings.append(
                _finding(
                    "scope_undeclared",
                    "warning",
                    "This credential declares no scopes, so nothing on the robot can check that it "
                    "is as narrow as §19 requires. That is the ordinary shape of an opaque bearer "
                    "token and is not a fault — it means `security.credential.scope` is answered on "
                    "the Hub and not here.",
                )
            )
        else:
            seen: dict[str, None] = {}
            for scope in scopes:
                if scope not in DEVICE_SCOPES:
                    seen[scope] = None
            unrecognised = sorted(seen.keys(), key=_utf16_key)
            if unrecognised:
                findings.append(
                    _finding(
                        "scope_unrecognised",
                        "error",
                        f"This credential carries {', '.join(unrecognised)}, which is not a "
                        "capability a device credential is recognised to need. §19 assumes this "
                        "credential is lost, and a capability nobody has established is narrow, on "
                        "a credential everybody assumes is stolen, is refused rather than allowed.",
                        unrecognised,
                    )
                )

    # ── §10's second addition: what the key is bound to ───────────────────────
    key = _js_trim(identity.get("device_key") or "")
    binding = identity.get("binding")
    binding = None if binding is None else binding
    birth_key = _js_trim(observed.get("birth_key") or "")

    if binding is None:
        findings.append(
            _finding(
                "key_binding_unknown",
                "warning",
                "Nothing has established whether this key lives in a secure element. "
                "`security.identity.attested` is a measurement, so an unmeasured robot is reported "
                "unmeasured rather than counted as either answer.",
            )
        )
    elif binding != "hardware":
        findings.append(
            _finding(
                "key_not_hardware_bound",
                "warning",
                "This device key is a file rather than a secret inside a secure element, so it "
                "leaves with a copy of the filesystem and identifies whatever machine that copy is "
                "put on. That is the ordinary state at guest tier and it is the ceiling §10 puts "
                "on `security.identity.attested`.",
            )
        )

    if observed.get("birth_certificate") is not True:
        findings.append(
            _finding(
                "no_birth_certificate",
                "warning",
                "This robot holds no trusted birth certificate, so there is nothing to check the "
                "device key against. §50.3: without one, what this machine is remains a claim it "
                "makes about itself.",
            )
        )
    elif birth_key != "" and key != "" and birth_key != key:
        findings.append(
            _finding(
                "key_not_attested",
                "error",
                "The key this robot was paired on is not the key its vendor attested at "
                "manufacture. §50.2 separates what it is from whose it is, and this is the seam "
                "between them coming apart: either the mainboard was replaced — §50.4, expected, "
                "and the remedy is a vendor-signed re-issue — or this robot was paired on a key "
                "nobody signed.",
                [birth_key],
            )
        )

    # Both halves. A hardware key nobody attested is a key the robot generated for itself, and an
    # attested key sitting in a file is an attestation that copies to any machine somebody puts the
    # file on.
    attested = (
        binding == "hardware"
        and observed.get("birth_certificate") is True
        and birth_key != ""
        and key != ""
        and birth_key == key
    )

    return {"paired": credential and namespace != "", "attested": attested, "findings": findings}


# ── §10's third addition: image attestation ──────────────────────────────────


def _render(measure: dict[str, Any], label: str, noun: str) -> str:
    """One clause of §10's sentence, **including its own label**.

    The label belongs to the clause rather than to the template, and that is not a stylistic choice:
    a template reading ``running image ${…}`` produces "running image `sha256:aa`" for a measured
    image and "running image an unmeasured OS image" for one that is not. The first version did
    exactly that, and it was found by opening the console rather than by any assertion — the bytes
    were correct, the shape was correct, and the sentence was unreadable.
    """
    state = measure.get("state")
    if state == "known":
        digest = _js_trim(measure.get("digest") or "")
        # A `known` measure with nothing in it is a caller bug, rendered as the absence it is rather
        # than as an empty pair of backticks somebody reads as a digest of zero length.
        return f"an unnamed {noun}" if digest == "" else f"{label}`{digest}`"
    if state == "nothing":
        return f"no {noun}"
    why = _js_trim(measure.get("why") or "")
    return f"an unmeasured {noun}" if why == "" else f"an unmeasured {noun} ({why})"


def attest(attestation: dict[str, Any]) -> dict[str, Any]:
    """§10's sentence, which **cannot fail**.

    §13's rule for a recording with a hole in it — seal it, and say it is incomplete — is the same
    rule here, because the reason is the same: the report most likely to have a hole in it is the one
    covering whatever made the hole. A check-in withheld because a digest could not be read is a
    robot the fleet believes is *offline*, at the exact moment somebody needs to know what it is
    running, and it converts a named gap into an indistinguishable silence.
    """
    unmeasured: list[str] = []
    if attestation["os_image"].get("state") == "unmeasured":
        unmeasured.append("os_image")
    if attestation["policy"].get("state") == "unmeasured":
        unmeasured.append("policy")
    if attestation["embodiment"].get("state") == "unmeasured":
        unmeasured.append("embodiment")
    unmeasured.sort(key=_utf16_key)

    device = _js_trim(attestation.get("device_id") or "")
    subject = "This robot" if device == "" else device

    sentence = (
        f"{subject} is running {_render(attestation['os_image'], 'image ', 'OS image')}, "
        f"{_render(attestation['policy'], 'policy ', 'policy')}, "
        f"against {_render(attestation['embodiment'], 'body ', 'body')}."
    )

    return {"complete": not unmeasured, "sentence": sentence, "unmeasured": unmeasured}


# ── The wire shapes ──────────────────────────────────────────────────────────


def code_to_json(case_input: dict[str, Any]) -> Any:
    """``identity/pairing`` — what a robot made of the characters somebody typed."""
    result = normalize_pairing_code(case_input["input"])
    fault = result["fault"]
    return {
        "code": result["code"],
        "fault": None
        if fault is None
        else {"code": fault["code"], "detail": fault["detail"], "position": fault["position"]},
    }


def identity_to_json(case_input: dict[str, Any]) -> Any:
    """``identity/credential`` — what state this robot's credential is in."""
    verdict = check_identity(case_input["identity"], case_input["observed"])
    return {
        "attested": verdict["attested"],
        "findings": [
            {
                "code": f["code"],
                "message": f["message"],
                "severity": f["severity"],
                "subjects": list(f["subjects"]),
            }
            for f in verdict["findings"]
        ],
        "paired": verdict["paired"],
    }


def attestation_to_json(case_input: dict[str, Any]) -> Any:
    """``identity/attestation`` — §10's sentence, holes included."""
    report = attest(case_input)
    return {
        "complete": report["complete"],
        "sentence": report["sentence"],
        "unmeasured": list(report["unmeasured"]),
    }


def _measure_block(measure: dict[str, Any]) -> dict[str, Any]:
    """One measure, with the digest kept rather than rendered away.

    ``digest`` and ``why`` are present-and-null rather than omitted, for the same reason
    :func:`skynet_os.checkin.summarize` keeps all five of §33.6's fields: a consumer that has to
    discover which keys this robot happened to fill in is a consumer that will get it wrong on the
    robot that filled in none.

    **The digest is gated on the state**, which is the line worth reading twice. A measure that says
    ``unmeasured`` and carries a digest is a contradiction — the state says nobody read the slot and
    the field carries a reading of it — and §32.1 puts a fleet column on exactly this question:
    *which robots are running image ``sha256:aa``*. Passing the digest through would answer that
    query with a robot whose image was never measured, which is a false positive on the one query an
    incident is run through. So a digest survives only on a ``known`` measure.
    """
    state = measure.get("state")
    if state not in MEASURE_STATES:
        # An unrecognised word is `unmeasured`, never `known`. Every default in this edition points
        # the same way: a document that did not say has not established that a slot was read.
        state = "unmeasured"
    digest = _js_trim(measure["digest"]) if isinstance(measure.get("digest"), str) else ""
    why = _js_trim(measure["why"]) if isinstance(measure.get("why"), str) else ""
    return {
        "digest": (digest or None) if state == "known" else None,
        "state": state,
        "why": why or None,
    }


def attestation_block(attestation: dict[str, Any]) -> dict[str, Any]:
    """The attestation as §10 asks for it: the sentence **and** the three digests it is made of.

    A sentence is not a record. §10 asks the Hub to record *"robot X is running image ``sha256:…``,
    policy ``sha256:…``, against body ``sha256:…``"*, and :func:`attest` produces exactly that — but
    ask a fleet *"which robots are running image ``sha256:aa``"*, the question §32.1 puts a column
    on, and the only way to answer it from the sentence is to parse English back out of prose that
    is deliberately not machine-readable: ``no policy``, ``an unmeasured OS image (the slot could
    not be read)``, ``an unnamed body``.

    So both go on the wire. The sentence, because it is what somebody reads at 3 a.m.; the measures,
    because they are what somebody queries at 3 a.m.
    """
    return {
        **attestation_to_json(attestation),
        "embodiment": _measure_block(attestation.get("embodiment") or {}),
        "os_image": _measure_block(attestation.get("os_image") or {}),
        "policy": _measure_block(attestation.get("policy") or {}),
    }
