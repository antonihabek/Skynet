"""§50's birth certificate, Python edition.

Transcribed from ``reference/typescript/birth.ts``. Every finding, sentence and ordering decision is
read from that file, never from ``corpus/``.

§50.2 keeps two facts apart, and this module exists to hold the line between them: **the certificate
proves what this robot is, and pairing proves whose it is.** A vendor who put a namespace in a birth
certificate would be asserting a fact they cannot know — so the field is read rather than discarded
at parse, and reported, because silently ignoring it means the next vendor adds it too.

Every field of ``Observed`` is optional because a robot early in boot has seen some and not others,
and §7.2's rule applies here as everywhere: **an absent input makes a check unavailable, never
passed.** An absent value never satisfies a comparison.

── The two findings that void a certificate, and the many that do not ───────

``attests`` is false only for ``unattested`` and a mismatched device key. The others *narrow* what
is attested; those two remove it. And note what ``attests`` is **not**: it is not "may the robot
run". §50 establishes identity and does not gate motion, and a robot whose certificate is void is
one with an unattested identity — the state every robot was in before §50 existed.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``const [first = '', second = ''] = finding.subjects``
    Array destructuring with defaults: a missing element becomes ``''``, and so does an element that
    is explicitly ``undefined``. Indexing a short list in Python raises instead, which would turn a
    malformed finding into a crash in the function whose whole job is to explain one.

``observed === null || observed === undefined``
    Both, and only those. An observed value of ``''`` is a reading, not an absence — so an empty
    string is *compared* and mismatches, rather than being reported unchecked.

``subjects[0] ?? ''``
    Same idea, in ``unchecked``.
"""

from __future__ import annotations

from typing import Any, Final

__all__ = [
    "BIRTH_FINDINGS",
    "OWNERSHIP_FIELDS",
    "REQUIRED_FIELDS",
    "attested_to_json",
    "attests",
    "check",
    "explain",
    "fully_confirmed",
    "unchecked",
    "voids_the_certificate",
]

#: Ordered by how much they cost, and each names its own remedy in :func:`explain` because the
#: remedies are genuinely different: a re-issue, a resync, a trust-store change, or a call.
BIRTH_FINDINGS: Final = (
    "unattested",
    "device_key_mismatch",
    "serial_mismatch",
    "descriptor_mismatch",
    "hardware_revision_mismatch",
    "ownership_asserted",
    "unobservable",
    "incomplete",
)

#: The fields §50.2 says a certificate binds, in the order it lists them.
REQUIRED_FIELDS: Final = (
    "device_key", "model", "serial", "descriptor_digest", "hardware_revision", "manufactured",
)

#: The fields a certificate must never carry (§50.2).
OWNERSHIP_FIELDS: Final = ("namespace", "owner", "operator", "customer")


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


def _at(items: list[Any], index: int) -> str:
    """``const [a = '', b = ''] = subjects`` — a missing element is the empty string, not an error."""
    if index >= len(items):
        return ""
    value = items[index]
    return "" if value is None else str(value)


def voids_the_certificate(finding: dict[str, Any]) -> bool:
    """Whether this finding means the certificate establishes nothing at all.

    True only for ``unattested`` and a mismatched device key.
    """
    return finding["code"] in ("unattested", "device_key_mismatch")


def explain(finding: dict[str, Any]) -> str:
    """The sentence an operator gets, naming the remedy."""
    subjects = finding["subjects"]
    first = _at(subjects, 0)
    second = _at(subjects, 1)
    code = finding["code"]

    if code == "unattested":
        return (
            f"The birth certificate's signature is {first}, so it attests nothing: everything it "
            "says about this machine is currently a claim the machine makes about itself, which is "
            "the situation a birth certificate exists to end. Register the vendor's key with this "
            "robot's trust store."
        )
    if code == "device_key_mismatch":
        return (
            f"This certificate was issued over device key {first} and the secure element holds "
            f"{second}. The document is about a different machine. If the mainboard was replaced "
            "this is the expected result and not a fault — §50.4 — and the remedy is a "
            "vendor-signed re-issue, which also invalidates the calibrations a board swap could "
            "have disturbed."
        )
    if code == "serial_mismatch":
        return (
            f'The certificate names serial "{first}" and this machine reports "{second}". Ask the '
            "vendor to re-issue."
        )
    if code == "descriptor_mismatch":
        return (
            f"The certificate attests descriptor {first} and this robot is running {second}. The "
            "joint ratings in force are ones nobody has signed for this machine. A vendor who has "
            "published an updated descriptor should re-issue the certificate over it; until then "
            "the envelope rests on the descriptor's own signature rather than on the vendor's "
            "attestation that it belongs to this body."
        )
    if code == "hardware_revision_mismatch":
        return (
            f'The certificate names hardware revision "{first}" and this machine reports '
            f'"{second}". Either the machine was modified or the certificate is stale; both are a '
            "vendor re-issue."
        )
    if code == "ownership_asserted":
        return (
            f"This certificate asserts ownership ({', '.join(subjects)}), and ownership is not the "
            "vendor's to assert. §50.2 keeps two facts apart: the certificate proves what this "
            "robot is, and pairing proves whose it is. The assertion is ignored."
        )
    if code == "unobservable":
        return (
            f"The certificate binds {first} and this robot could not read its own. The binding is "
            "neither confirmed nor contradicted — it is unchecked, which is not the same as "
            "matching."
        )
    if code == "incomplete":
        return (
            f"The certificate does not bind {first}, which §50.2 requires it to. It is not a "
            "complete birth certificate and whatever it does bind is all it attests."
        )
    raise AssertionError(f"unknown finding {code!r}")


def attests(result: dict[str, Any]) -> bool:
    """Whether this certificate attests anything at all about this machine."""
    return not any(voids_the_certificate(f) for f in result["findings"])


def fully_confirmed(result: dict[str, Any]) -> bool:
    """Whether every binding the certificate makes was checked and matched."""
    return len(result["findings"]) == 0


def unchecked(result: dict[str, Any]) -> list[str]:
    """The bindings that could not be checked, as their field names."""
    return [
        _at(f["subjects"], 0) for f in result["findings"] if f["code"] == "unobservable"
    ]


def _compare(
    certificate: str,
    observed: Any,
    field: str,
    mismatch: str,
    findings: list[dict[str, Any]],
) -> None:
    if certificate == "":
        findings.append({"code": "incomplete", "subjects": [field]})
        return
    # Null and undefined, and only those: an observed `''` is a reading rather than an absence, so
    # it is compared and mismatches instead of being reported unchecked.
    if observed is None or observed is _ABSENT:
        findings.append({"code": "unobservable", "subjects": [field]})
        return
    if observed == certificate:
        return
    findings.append({"code": mismatch, "subjects": [certificate, observed]})


def check(certificate: dict[str, Any], observed: dict[str, Any]) -> dict[str, Any]:
    """Check a birth certificate against what the robot observes about itself (§50).

    The order of the findings is the order of their cost, and the first is deliberately about the
    *document* rather than the machine: comparing fields against a certificate nobody signed is
    comparing the robot to a story it told itself.
    """
    findings: list[dict[str, Any]] = []

    if certificate["signature"] != "verified":
        findings.append({"code": "unattested", "subjects": [certificate["signature"]]})

    _compare(
        certificate["device_key"], _field(observed, "device_key"),
        "the device key", "device_key_mismatch", findings,
    )
    _compare(
        certificate["serial"], _field(observed, "serial"),
        "a serial number", "serial_mismatch", findings,
    )
    _compare(
        certificate["descriptor_digest"], _field(observed, "descriptor_digest"),
        "an embodiment descriptor digest", "descriptor_mismatch", findings,
    )
    _compare(
        certificate["hardware_revision"], _field(observed, "hardware_revision"),
        "a hardware revision", "hardware_revision_mismatch", findings,
    )

    if certificate["model"] == "":
        # Bound but not observable: a model is what the vendor calls this design, and there is
        # nothing on the machine to compare it against. Its absence is still incomplete.
        findings.append({"code": "incomplete", "subjects": ["a model"]})

    if len(certificate["claims_ownership"]) > 0:
        findings.append(
            {"code": "ownership_asserted", "subjects": list(certificate["claims_ownership"])}
        )

    return {"findings": findings}


def attested_to_json(case_input: dict[str, Any]) -> Any:
    """``birth/certificate`` — what one certificate establishes about one machine."""
    result = check(case_input["certificate"], case_input["observed"])
    return {
        "attests": attests(result),
        "findings": [
            {"code": f["code"], "message": explain(f), "subjects": list(f["subjects"])}
            for f in result["findings"]
        ],
        "fully_confirmed": fully_confirmed(result),
        "unchecked": unchecked(result),
    }
