"""§8's package admission, Python edition.

Transcribed from ``admitPackage`` and its helpers in ``reference/typescript/package.ts``. Every
rule, code and message string is read from that file, never from ``corpus/``.

Pure, and shared: the Hub runs it to tell an operator what will happen before they deploy, and the
OS runs it at install time on the robot, offline. Neither is authoritative over the other, because
both compute the same function over the same declared facts.

Every check returns a **finding** rather than throwing, so an operator sees all the reasons an
install failed instead of the first one. Fixing four problems one deployment at a time is how people
end up granting everything.

── Where Python is quietly not JavaScript, here ──────────────────────────────

``Number(segment)``, and why this note is now shorter than it was
    ``compareVersions`` used to map ``Number`` over the dotted parts, and the two paragraphs that
    stood here described the contortions needed to reproduce it: ``Number('')`` is **0** where
    ``float('')`` raises, ``Number('abc')`` is **NaN**, and a NaN segment makes ``diff !== 0`` true
    and ``diff < 0`` false so the reference reported ``a > b`` — a robot *newer* than any minimum.
    This edition carried a ``_js_number`` and reproduced all of it faithfully, including the part
    that was a hole.

    The reference now reads the **leading ASCII digits** of a segment and nothing else (§80,
    decided 2026-09-05), which is the same function in Python as it is in JavaScript. There is no
    JavaScript-shaped edge left to reproduce here, so the coercion is gone rather than kept for a
    caller that no longer needs it.

``capacity.simulator !== true``
    Strict. Absent means false — a real robot should not have to declare what it is not, and the
    obligation to declare sits on the simulator driver.

``budget.vram_mb !== undefined``
    Strict, and paired with ``capacity.vram_mb ?? 0`` on the other side. A package that declares no
    VRAM need is not checked; a robot that declares no VRAM has zero of it. The asymmetry is the
    rule, and ``dict.get`` collapses both halves of it.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .canonical import OMIT
from .vocabulary import members as _members

__all__ = [
    "CAPABILITY_DOMAINS",
    "CAPABILITY_TOKENS",
    "RESERVED_CAPABILITIES",
    "TOKEN_CLASSES",
    "SIGNATURE_STATES",
    "WARNING_CHANNEL",
    "admission_to_json",
    "admit_package",
    "classify_token",
    "is_reserved",
    "token_to_json",
]

#: §49.1's reserved token. One entry today, and a list rather than a comparison because the next
#: reserved token will be added by somebody who found this constant, not somebody who found an `if`.
WARNING_CHANNEL: Final = "hri.warning"
RESERVED_CAPABILITIES: Final[tuple[str, ...]] = (WARNING_CHANNEL,)

#: Five states, because a boolean held two and the other three mattered. They have different owners
#: and different remedies, and collapsing them left an operator unable to tell whether to chase a
#: publisher, add a key, upgrade the OS, or stop and investigate.
SIGNATURE_STATES: Final = ("verified", "failed", "untrusted", "absent", "unverifiable")


def is_reserved(token: str) -> bool:
    """Whether a capability token is reserved to the OS (§49.1)."""
    return token in RESERVED_CAPABILITIES


# ── §85, the capability token vocabulary ─────────────────────────────────────

#: The domains a canonical token may belong to, in the order §85.1 lists them.
#:
#: The Standard Robot Interface's, which the Hub has validated robot registrations against since
#: before §8 was written, plus ``hri`` for §49.1's two tokens (DRY-COVERAGE-PLAN.md F34).
CAPABILITY_DOMAINS: Final = (
    "locomotion", "manipulation", "sensing", "control", "safety", "platform", "hri",
)

#: Every canonical token (§85.1). Membership is the whole rule, and the list is read from the
#: vocabulary module rather than retyped, because the closed set and the classifier disagreeing
#: about one token is exactly the divergence the corpus exists to catch.
#:
#: Not all of them are two segments — ``sensing.camera.rgb`` is three — so nothing may be derived
#: from a segment count.
CAPABILITY_TOKENS: Final = _members("capability_tokens")

TOKEN_CLASSES: Final = ("canonical", "unknown", "vendor", "malformed")


def _well_formed(token: str) -> bool:
    """Dot-separated segments, each an ASCII lowercase letter then ASCII lowercase, digits or ``_``.

    Compared by code point, never with ``str.islower`` or ``str.isdigit``: both are Unicode-aware in
    Python, so ``é`` is lowercase and ``٢`` is a digit there and in no other edition.
    """
    segments = token.split(".")
    if len(segments) < 2:
        return False
    for segment in segments:
        if not segment or not ("a" <= segment[0] <= "z"):
            return False
        for character in segment[1:]:
            if not ("a" <= character <= "z" or "0" <= character <= "9" or character == "_"):
                return False
    return True


def classify_token(token: str) -> str:
    """§85.1: ``canonical``, ``unknown``, ``vendor`` or ``malformed``. Exact — nothing is stripped."""
    if not _well_formed(token):
        return "malformed"
    if token in CAPABILITY_TOKENS:
        return "canonical"
    domain = token[: token.index(".")]
    return "unknown" if domain in CAPABILITY_DOMAINS else "vendor"


def _distinct(tokens: list[str]) -> list[str]:
    """First-appearance order, each once — one problem named twice reads as two."""
    out: list[str] = []
    for token in tokens:
        if token not in out:
            out.append(token)
    return out


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


def _or_default(value: Any, default: Any) -> Any:
    """``value ?? default``."""
    return default if value is _ABSENT or value is None else value


def _js_int_string(value: Any) -> str:
    """A number inside a template literal. ``String(512)`` is ``"512"``, not ``"512.0"``."""
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


def _segment_value(segment: str) -> int:
    """The leading run of ASCII digits in one dotted segment, or 0.

    Transcribed from ``segmentValue``. ``str.isdigit`` is deliberately not used: it is true for
    Arabic-Indic digits, superscripts and a few hundred other code points, and the reference tests
    ``>= '0' and <= '9'`` on UTF-16 code units, which is ASCII and only ASCII. A version segment of
    ``'٣'`` is therefore 0 — unreadable, and so the oldest version there is — in both editions.
    """
    digits = 0
    while digits < len(segment) and "0" <= segment[digits] <= "9":
        digits += 1
    return int(segment[:digits]) if digits else 0


def _compare_versions(a: str, b: str) -> int:
    """Numeric-segment comparison, sufficient for ``major.minor.patch`` OS versions."""
    left = [_segment_value(part) for part in a.split(".")]
    right = [_segment_value(part) for part in b.split(".")]
    for i in range(max(len(left), len(right))):
        lhs = left[i] if i < len(left) else 0
        rhs = right[i] if i < len(right) else 0
        if lhs != rhs:
            return -1 if lhs < rhs else 1
    return 0


def admit_package(params: dict[str, Any]) -> dict[str, Any]:
    """Decide whether a package may be installed."""
    manifest = params["manifest"]
    capacity = params["capacity"]
    findings: list[dict[str, Any]] = []

    # Order matters here in one direction only: `failed` is reported as its own code because it is
    # the one state that says something *went wrong* rather than something is missing. An operator
    # who reads `unsigned` shrugs; one who reads `signature_failed` stops; and one who reads
    # `signature_untrusted` goes and looks at the trust store, which is a third place entirely.
    signature = params["signature"]
    if signature == "failed":
        findings.append(
            {
                "code": "signature_failed",
                "severity": "error",
                "message": (
                    "A release signature was present and did not verify. The bytes are not the "
                    "ones the signer attested to — treat this as tampering until somebody proves "
                    "otherwise."
                ),
            }
        )
    elif signature == "untrusted":
        findings.append(
            {
                "code": "signature_untrusted",
                "severity": "error",
                "message": (
                    "A release signature verified, and the key that made it is not one this robot "
                    "trusts. Nothing is wrong with the bytes; what is missing is the operator's "
                    "decision that this signer may publish here."
                ),
            }
        )
    elif signature == "absent":
        findings.append(
            {
                "code": "unsigned",
                "severity": "error",
                "message": (
                    "No publisher signature accompanied this package. The bytes may be intact and "
                    "nobody has attested to who produced them."
                ),
            }
        )
    elif signature == "unverifiable":
        findings.append(
            {
                "code": "signature_unverifiable",
                "severity": "error",
                "message": (
                    "This platform cannot verify a release signature, so nothing has established "
                    "who produced these bytes. That is a gap in the operating system rather than "
                    "in the package."
                ),
            }
        )

    if params["observed_digest"] != manifest["digest"]:
        findings.append(
            {
                "code": "digest_mismatch",
                "severity": "error",
                "message": "The package contents do not hash to the digest in its manifest.",
                "subjects": [manifest["digest"], params["observed_digest"]],
            }
        )

    if manifest["kind"] != "driver" and manifest["isolation"] == "native":
        findings.append(
            {
                "code": "isolation_violation",
                "severity": "error",
                "message": (
                    f"A {manifest['kind']} package may not run unsandboxed. Only drivers touch "
                    "hardware directly."
                ),
            }
        )

    # §55.1. A warning rather than an error, because installing it is not dangerous and refusing it
    # would stop an operator staging a package onto a robot they are about to point at a simulator.
    # What is not acceptable is silence — a package that will never do anything, sitting installed,
    # looking exactly like one that works.
    if manifest["kind"] == "environment" and capacity.get("simulator") is not True:
        findings.append(
            {
                "code": "environment_inert",
                "severity": "warning",
                "message": (
                    "An environment describes a simulated world, and no simulator driver is loaded "
                    "on this robot. It will install, and nothing will ever load it."
                ),
            }
        )

    findings.extend(_check_budget(manifest["budget"], capacity))

    # A package must not request more than it declared it needs.
    over_requested = [
        token
        for token in manifest["requests"]
        if token not in manifest["embodiment"]["capabilities"]
    ]
    if over_requested:
        findings.append(
            {
                "code": "over_request",
                "severity": "error",
                "message": "The package requests permissions it never declared a need for.",
                "subjects": over_requested,
            }
        )

    # §85. A token that is not a capability can never match one; refused by name instead of read as
    # a robot that lacks it.
    unknown = _distinct(
        [
            token
            for token in [*manifest["embodiment"]["capabilities"], *manifest["requests"]]
            if classify_token(token) != "canonical"
        ]
    )
    if unknown:
        findings.append(
            {
                "code": "capability_unknown",
                "severity": "error",
                "message": (
                    "These are not capabilities this platform defines (§85). A token in a canonical "
                    "domain must be one §85 lists, so check the spelling; a vendor's own token "
                    "belongs under `embodiment.extensions`."
                ),
                "subjects": unknown,
            }
        )

    misfiled = _distinct(
        [
            token
            for token in _or_default(_field(manifest["embodiment"], "extensions"), [])
            if classify_token(token) != "vendor"
        ]
    )
    if misfiled:
        findings.append(
            {
                "code": "capability_unknown",
                "severity": "error",
                "message": (
                    "These extensions are not vendor tokens (§85). A token in a canonical domain "
                    "belongs under `embodiment.capabilities`, where it is checked; an extension is "
                    "dot-separated lowercase segments whose first is not a canonical domain."
                ),
                "subjects": misfiled,
            }
        )

    # Two spellings of one fact, and admission enforces one of them (§8.2). A subset test in one
    # direction only: a publisher who declares *more* in `embodiment` than on the card has a package
    # a robot enforces correctly and a Hub that under-reports its needs — the Hub's to catch at
    # publish, and refusing here would refuse the package whose enforced declaration is stricter.
    embodiment_extensions = _or_default(_field(manifest["embodiment"], "extensions"), [])
    unenforced = [
        token
        for token in _or_default(_field(manifest, "required_capabilities"), [])
        if token not in manifest["embodiment"]["capabilities"]
    ] + [
        token
        for token in _or_default(_field(manifest, "required_extensions"), [])
        if token not in embodiment_extensions
    ]
    if unenforced:
        findings.append(
            {
                "code": "embodiment_undeclared",
                "severity": "error",
                "message": (
                    "This package declares requirements its embodiment block does not, so "
                    "admission checked nothing for them. §8.2 enforces `embodiment.capabilities` "
                    "and `embodiment.extensions`."
                ),
                "subjects": unenforced,
            }
        )

    # §49.1, checked before the operator's denials on purpose: a reserved token is not something an
    # operator declined, it is something nobody can grant, and a package that asked for both should
    # be told the one it can never have.
    reserved: list[str] = []
    for token in list(manifest["requests"]) + list(manifest["embodiment"]["capabilities"]):
        # Deduplicated: a package that both declares and requests the channel has one problem, and
        # a finding naming the same token twice reads as two.
        if is_reserved(token) and token not in reserved:
            reserved.append(token)
    if reserved:
        findings.append(
            {
                "code": "capability_reserved",
                "severity": "error",
                "message": (
                    "The audible warning before autonomous motion is a system function and no "
                    "grant confers it. A package that could claim this channel could silence its "
                    "own warning, which removes a control the operator's risk assessment relied "
                    "on (§49.1)."
                ),
                "subjects": reserved,
            }
        )

    denied = _or_default(_field(capacity, "denied"), [])
    explicitly_denied = [token for token in manifest["requests"] if token in denied]
    if explicitly_denied:
        findings.append(
            {
                "code": "capability_denied",
                "severity": "error",
                "message": "The operator profile withholds capabilities this package requires.",
                "subjects": explicitly_denied,
            }
        )

    provided = set(capacity["capabilities"]) | set(
        _or_default(_field(capacity, "extensions"), [])
    )
    required = list(manifest["embodiment"]["capabilities"]) + list(embodiment_extensions)
    unmet = [token for token in required if token not in provided]
    if unmet:
        findings.append(
            {
                "code": "capability_ungranted",
                "severity": "error",
                "message": "This robot does not provide capabilities the package requires.",
                "subjects": unmet,
            }
        )

    ungranted = [
        token
        for token in manifest["requests"]
        if token in provided and token not in capacity["granted"] and token not in denied
    ]
    if ungranted:
        # A warning, not an error: this is the Android runtime-permission moment. The install
        # proceeds and the operator is prompted; the package simply cannot use the capability until
        # they say yes, and the mode gate enforces that at every cycle.
        findings.append(
            {
                "code": "capability_ungranted",
                "severity": "warning",
                "message": (
                    "These capabilities are awaiting an operator grant. The package installs and "
                    "cannot use them until it is given."
                ),
                "subjects": ungranted,
            }
        )

    for dependency in _or_default(_field(manifest, "depends_on"), []):
        if not any(
            installed["id"] == dependency["id"] for installed in capacity["installed"]
        ):
            findings.append(
                {
                    "code": "missing_dependency",
                    "severity": "error",
                    "message": (
                        f"Requires {dependency['id']} {dependency['range']}, which is not "
                        "installed."
                    ),
                    "subjects": [dependency["id"]],
                }
            )

    min_os = _or_default(_field(manifest, "min_os_version"), None)
    # Truthy, not present: an empty `min_os_version` is no requirement at all.
    if min_os and _compare_versions(capacity["os_version"], min_os) < 0:
        findings.append(
            {
                "code": "os_too_old",
                "severity": "error",
                "message": (
                    f"Requires Skynet OS {min_os} or later; this robot runs "
                    f"{capacity['os_version']}."
                ),
            }
        )

    if params.get("transport_verdict") == "refused":
        findings.append(
            {
                "code": "transport_refused",
                "severity": "error",
                "message": "The policy's action space cannot be transported onto this body.",
            }
        )

    if params.get("executes_on_load") and not params.get("trust_remote_code"):
        findings.append(
            {
                "code": "executes_on_load_untrusted",
                "severity": "error",
                "message": (
                    "This package contains files that execute code when read, and the operator "
                    "has not accepted that. Install with trust_remote_code once you have decided "
                    "the publisher is trustworthy."
                ),
            }
        )

    admitted = not any(finding["severity"] == "error" for finding in findings)
    effective_grants = (
        [
            token
            for token in manifest["requests"]
            if token in provided and token in capacity["granted"]
        ]
        if admitted
        else []
    )

    return {"admitted": admitted, "findings": findings, "effective_grants": effective_grants}


def _check_budget(budget: dict[str, Any], capacity: dict[str, Any]) -> list[dict[str, Any]]:
    shortfalls: list[str] = []
    if budget["ram_mb"] > capacity["ram_mb"]:
        shortfalls.append(
            f"RAM: needs {_js_int_string(budget['ram_mb'])} MB, has "
            f"{_js_int_string(capacity['ram_mb'])} MB"
        )
    vram = _field(budget, "vram_mb")
    if vram is not _ABSENT and vram > _or_default(_field(capacity, "vram_mb"), 0):
        shortfalls.append(
            f"VRAM: needs {_js_int_string(vram)} MB, has "
            f"{_js_int_string(_or_default(_field(capacity, 'vram_mb'), 0))} MB"
        )
    disk = _field(budget, "disk_mb")
    if disk is not _ABSENT and disk > _or_default(_field(capacity, "disk_mb"), 0):
        shortfalls.append(
            f"Disk: needs {_js_int_string(disk)} MB, has "
            f"{_js_int_string(_or_default(_field(capacity, 'disk_mb'), 0))} MB"
        )
    cores = _field(budget, "cpu_cores")
    if cores is not _ABSENT and cores > _or_default(_field(capacity, "cpu_cores"), 0):
        shortfalls.append(
            f"CPU: needs {_js_int_string(cores)} cores, has "
            f"{_js_int_string(_or_default(_field(capacity, 'cpu_cores'), 0))}"
        )

    if not shortfalls:
        return []
    return [
        {
            "code": "budget_exceeded",
            "severity": "error",
            "message": "This robot cannot supply the resources the package declares it needs.",
            "subjects": shortfalls,
        }
    ]


def token_to_json(case_input: dict[str, Any]) -> Any:
    """``admission/tokens`` — what one string in a capability list is (§85.1)."""
    return {"class": classify_token(case_input["token"])}


def admission_to_json(case_input: dict[str, Any]) -> Any:
    """``admission/install`` — the verdict, every reason for it, and what will actually be active."""
    result = admit_package(case_input)
    return {
        "admitted": result["admitted"],
        "findings": [
            {
                "code": f["code"],
                "severity": f["severity"],
                "message": f["message"],
                "subjects": f["subjects"] if f.get("subjects") is not None else OMIT,
            }
            for f in result["findings"]
        ],
        "effective_grants": result["effective_grants"],
    }
