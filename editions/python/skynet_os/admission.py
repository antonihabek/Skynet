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

``Number(segment)``
    ``compareVersions`` splits on ``.`` and maps ``Number`` over the parts. ``Number('')`` is **0**
    and ``Number('abc')`` is **NaN**, where ``float('')`` and ``float('abc')`` both raise — so a
    malformed version string is an *answer* in the reference and a crash in a naive edition.
    :func:`_js_number` is that coercion, and it is deliberately **not** ``predicate._js_number``,
    which implements the reference's stricter ``toNumber``: that one refuses a blank string on
    purpose, and reusing it here would make ``1..0`` compare differently from the way it does.

``NaN`` in a comparison
    A NaN segment makes ``diff !== 0`` true and ``diff < 0`` false, so the reference reports
    ``a > b``. Reproduced rather than rejected: it is the specification's answer.

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
import re
from typing import Any, Final

from .canonical import OMIT

__all__ = [
    "RESERVED_CAPABILITIES",
    "SIGNATURE_STATES",
    "WARNING_CHANNEL",
    "admission_to_json",
    "admit_package",
    "is_reserved",
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


_RADIX = re.compile(r"^[+-]?0[xXoObB]")
_DECIMAL = re.compile(r"^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$")


def _js_number(value: Any) -> float:
    """``Number(value)`` — the raw coercion, blanks and all.

    Not :func:`predicate._js_number`, which is the reference's stricter ``toNumber``: that one
    refuses a blank string so an empty field cannot compare as zero. Here a blank *is* zero, because
    that is what ``'1..0'.split('.').map(Number)`` produces, and the difference decides how a
    malformed version orders.
    """
    if isinstance(value, bool):
        return 1.0 if value else 0.0
    if value is None:
        return 0.0
    if isinstance(value, (int, float)):
        return float(value)
    if not isinstance(value, str):
        return math.nan

    text = value.strip()
    if text == "":
        return 0.0
    if text in ("Infinity", "+Infinity"):
        return math.inf
    if text == "-Infinity":
        return -math.inf
    if _RADIX.match(text):
        sign = -1.0 if text[0] == "-" else 1.0
        body = text.lstrip("+-")
        base = {"x": 16, "o": 8, "b": 2}[body[1].lower()]
        try:
            return sign * float(int(body[2:], base))
        except ValueError:
            return math.nan
    if not _DECIMAL.match(text):
        return math.nan
    try:
        return float(text)
    except ValueError:
        return math.nan


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


def _compare_versions(a: str, b: str) -> int:
    """Numeric-segment comparison, sufficient for ``major.minor.patch`` OS versions."""
    left = [_js_number(part) for part in a.split(".")]
    right = [_js_number(part) for part in b.split(".")]
    for i in range(max(len(left), len(right))):
        lhs = left[i] if i < len(left) else 0.0
        rhs = right[i] if i < len(right) else 0.0
        diff = lhs - rhs
        # `NaN !== 0` is true and `NaN < 0` is false, so a segment that is not a number reports
        # `a > b`. The reference's answer, reproduced rather than corrected.
        if not (diff == 0):
            return -1 if diff < 0 else 1
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
