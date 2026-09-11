"""§12's OS update, Python edition.

Transcribed from ``reference/typescript/ota.ts``. Every rule, code and message string is read from
that file, never from ``corpus/``.

Two of §12's rules live here. The **third** decides whether an image may be installed at all, and
the **second** decides whether a slot the robot has just booted into is good.

Both are written down rather than left to a script for the same reason, stated once in each:

    Absence of evidence is not health. Every check is optional and absent never confirms. A
    supervisor that marked a slot good because it had not yet been told otherwise would replace
    §12's unattended recovery with a race between the health check and the confirmation.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``new Set(...)`` then ``.sort()``
    A JavaScript ``Set`` keeps insertion order and ``Array.sort`` then orders by UTF-16 code unit;
    a Python ``set`` has no order at all, and ``sorted`` orders by code point. ``gated_components``
    is reported to an operator *and* used as a fault's subjects, so both halves matter.

``evidence.pending !== true``
    Strict, and it is the guard on everything else: false makes every other field irrelevant,
    because a supervisor that could mark a slot bad on an ordinary boot would be a supervisor that
    can roll back a robot nobody updated.

``compatible.includes(board)``
    ``SameValueZero``, so a board named ``true`` does not match a ``1`` in the list. Python's ``in``
    would match them.

template literals
    ``String(600000)`` is ``"600000"``, not ``"600000.0"``. Every millisecond figure in a boot
    verdict's sentence goes through :func:`_js_number`.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .js import js_trim as _js_trim

__all__ = [
    "BOOT_DECISIONS",
    "BUNDLE_FAULTS",
    "CERTIFICATION_GATED",
    "DEFAULT_BOOT_BUDGET_MS",
    "OS_COMPONENTS",
    "admit_bundle",
    "boot_to_json",
    "bundle_to_json",
    "confirm_boot",
    "gated",
]

OS_COMPONENTS: Final = (
    "bootloader",
    "kernel_image",
    "rhal_abi",
    "rhal_driver",
    "safety_kernel",
    "envelope",
    "skynetd",
    "policy_runtime",
    "userspace",
)

#: The components §12's third rule gates, by name. **Not the whole rule** — :func:`gated` is,
#: because an unrecognised name is gated too.
CERTIFICATION_GATED: Final = ("safety_kernel", "envelope")

BUNDLE_FAULTS: Final = (
    "unversioned",
    "undigested",
    "signature_failed",
    "signature_untrusted",
    "signature_absent",
    "signature_unverifiable",
    "incompatible",
    "compatibility_undeclared",
    "channel_mismatch",
    "certification_required",
    "bench_pass_stale",
    "bench_pass_failed",
    "already_running",
)

BOOT_DECISIONS: Final = ("not_pending", "wait", "confirm", "roll_back")

#: Ten minutes. Long enough for a robot that boots slowly, fetches a desired state over a poor link
#: and starts a driver that probes a bus; short enough that a warehouse at 3 a.m. is not a warehouse
#: with a brick in it until morning. The document gives no number, so this is a default an operator
#: may narrow rather than a constant anybody derived.
DEFAULT_BOOT_BUDGET_MS: Final = 600_000


def _js_number(value: Any) -> str:
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


def _is_finite(value: Any) -> bool:
    if isinstance(value, bool):
        return False
    return isinstance(value, (int, float)) and math.isfinite(value)


def _utf16_key(value: str) -> tuple[int, ...]:
    """Order by UTF-16 code unit, which is what ``Array.prototype.sort`` does."""
    encoded = value.encode("utf-16-be", errors="surrogatepass")
    return tuple((encoded[i] << 8) | encoded[i + 1] for i in range(0, len(encoded), 2))


def _strict_includes(haystack: Any, needle: Any) -> bool:
    """``Array.prototype.includes`` — ``SameValueZero``, so ``true`` is not ``1``."""
    if not isinstance(haystack, (list, tuple)):
        return False
    for item in haystack:
        if isinstance(item, bool) != isinstance(needle, bool):
            continue
        if item == needle:
            return True
    return False


def gated(component: Any) -> bool:
    """Whether changing this component requires a fresh bench pass.

    True for §12's two names, **and true for anything this edition does not recognise**: a component
    nobody can classify is one nobody has established is safe to change, and the two readings fail
    in opposite directions.
    """
    if _strict_includes(CERTIFICATION_GATED, component):
        return True
    return not _strict_includes(OS_COMPONENTS, component)


def _fault(
    code: str, severity: str, message: str, subjects: list[str] | None = None
) -> dict[str, Any]:
    return {
        "code": code,
        "severity": severity,
        "message": message,
        "subjects": [] if subjects is None else subjects,
    }


def _display_digest(digest: str) -> str:
    return "nothing" if digest == "" else digest


def _certification_message(gated_components: list[str], unrecognised: list[str]) -> str:
    message = (
        f"This update changes {', '.join(gated_components)}, and §12 refuses one that changes the "
        "safety kernel or the envelope without a fresh bench pass for these exact bytes."
    )
    if unrecognised:
        message += (
            f" {', '.join(unrecognised)} is not a component this edition recognises, and an "
            "unrecognised component is gated for the same reason the two named ones are: nobody "
            "has established what it changes."
        )
    message += " This robot holds no bench pass at all."
    return message


def admit_bundle(bundle: dict[str, Any], subject: dict[str, Any]) -> dict[str, Any]:
    """§12's third rule, and everything else that decides whether an image may be installed.

    Every check runs, and the result carries all of them. An operator fixing four problems one
    attempt at a time is the failure ``admit_package`` already refuses to cause, and an OS update is
    the case where each attempt costs a reboot.
    """
    faults: list[dict[str, Any]] = []
    version = bundle.get("version")
    version = "" if version is None else version
    digest = bundle.get("digest")
    digest = "" if digest is None else digest
    channel = bundle.get("channel")
    channel = "" if channel is None else channel
    compatible = bundle.get("compatible")
    compatible = [] if compatible is None else compatible
    changes = bundle.get("changes")
    changes = [] if changes is None else changes

    if _js_trim(version) == "":
        faults.append(
            _fault(
                "unversioned",
                "error",
                "This bundle names no version, so nothing can say what the robot would be running "
                "afterwards. Rebuild it with a version.",
            )
        )

    # Before the certification gate, because that gate binds a bench pass to this digest and a
    # bundle with no digest cannot be bound to anything. Reported separately so the two refusals do
    # not read as one problem.
    if _js_trim(digest) == "":
        faults.append(
            _fault(
                "undigested",
                "error",
                "This bundle names no digest. §12's certification gate binds a bench pass to the "
                "exact bytes it was taken against, and bytes with no name cannot be bound to.",
            )
        )

    # §37.1's five outcomes, kept apart here for the reason they are kept apart there: four
    # different facts with four different people at the end of them.
    signature = subject["signature"]
    if signature == "verified":
        pass
    elif signature == "failed":
        faults.append(
            _fault(
                "signature_failed",
                "error",
                "A signature was present and did not verify. The bytes are attested to somebody "
                "other than the manifest claims, which is what tampering looks like — do not "
                "install this, and find out where it came from.",
            )
        )
    elif signature == "untrusted":
        faults.append(
            _fault(
                "signature_untrusted",
                "error",
                "The signature verifies against a key this robot has not been told to believe. Add "
                "the key with `skynetd trust add`, or find out why a stranger signed an OS image "
                "for this fleet.",
            )
        )
    elif signature == "absent":
        faults.append(
            _fault(
                "signature_absent",
                "error",
                "Nobody signed this bundle. §12 signs images with the same Ed25519 machinery as "
                "releases, and an unsigned root filesystem is a root filesystem anybody on the "
                "path could have written.",
            )
        )
    else:
        faults.append(
            _fault(
                "signature_unverifiable",
                "error",
                "This platform cannot check a signature at all, so it cannot establish that this "
                "image is the one the fleet published. That is the operating system's problem "
                "rather than the publisher's.",
            )
        )

    # A bundle that declares nothing is a **warning**, because §12 does not require the field and
    # refusing on it would stop every image built before it existed — but it is said out loud.
    board = subject.get("board")
    board = None if board is None else board
    if len(compatible) == 0:
        faults.append(
            _fault(
                "compatibility_undeclared",
                "warning",
                "This bundle declares no compatible machines, so nothing here can tell whether it "
                "is built for this one.",
            )
        )
    elif board is not None and not _strict_includes(compatible, board):
        faults.append(
            _fault(
                "incompatible",
                "error",
                f"This bundle is built for {', '.join(compatible)}, and this machine is `{board}`.",
                list(compatible),
            )
        )
    elif board is None:
        # A robot that cannot say what board it is has not established a mismatch, and refusing
        # would make an unknown machine unupdatable — which is the state a robot is in exactly when
        # it most needs an update.
        faults.append(
            _fault(
                "compatibility_undeclared",
                "warning",
                "This machine does not report what it is, so the bundle's compatibility with "
                f"{', '.join(compatible)} could not be checked.",
                list(compatible),
            )
        )

    wanted = subject.get("channel")
    wanted = "" if wanted is None else wanted
    if wanted != "" and channel != "" and wanted != channel:
        faults.append(
            _fault(
                "channel_mismatch",
                "error",
                f"The desired state asks for the `{wanted}` channel and this bundle is "
                f"`{channel}`. A robot pulled onto another channel by an image rather than by an "
                "operator is a robot nobody moved.",
            )
        )

    # ── §12's third rule ──────────────────────────────────────────────────────
    seen: dict[Any, None] = {}
    for component in changes:
        if gated(component):
            seen[component] = None
    gated_components = sorted(seen.keys(), key=_utf16_key)

    if gated_components:
        unrecognised = [c for c in gated_components if not _strict_includes(OS_COMPONENTS, c)]
        bench = subject.get("bench")
        bench = None if bench is None else bench
        if bench is None:
            faults.append(
                _fault(
                    "certification_required",
                    "error",
                    _certification_message(gated_components, unrecognised),
                    list(gated_components),
                )
            )
        elif bench["image_digest"] != digest or digest == "":
            faults.append(
                _fault(
                    "bench_pass_stale",
                    "error",
                    f"The bench pass this robot holds was taken against "
                    f"`{_display_digest(bench['image_digest'])}` and this bundle is "
                    f"`{_display_digest(digest)}`. Evidence that is not bound to what it is "
                    "evidence of is not evidence — book a fresh bench pass for these bytes.",
                    list(gated_components),
                )
            )
        elif not bench["passed"]:
            faults.append(
                _fault(
                    "bench_pass_failed",
                    "error",
                    "A bench pass exists for these bytes and it records a failure. This update "
                    f"changes {', '.join(gated_components)}, and §12 refuses one that has not "
                    "passed rather than one that has not been tried.",
                    list(gated_components),
                )
            )

    running = subject.get("running_version")
    running = "" if running is None else running
    if running != "" and running == version:
        faults.append(
            _fault(
                "already_running",
                "warning",
                f"This robot is already running `{version}`. Staging it again is harmless and "
                "applying it costs a reboot for nothing.",
            )
        )

    return {
        "admitted": not any(f["severity"] == "error" for f in faults),
        "faults": faults,
        "gated_components": gated_components,
    }


def confirm_boot(evidence: dict[str, Any]) -> dict[str, Any]:
    """§12's second rule — health-gated rollback.

        The new slot must pass a health check within a boot budget or the bootloader returns to the
        previous slot unattended.

    Three properties: absence of evidence is not health; the budget is what makes it unattended; and
    a robot that is not pending decides nothing, because marking a slot bad on an ordinary boot is a
    rollback of an update nobody made.
    """
    if evidence.get("pending") is not True:
        return {
            "decision": "not_pending",
            "detail": (
                "This boot is not the first into a new slot, so there is nothing to confirm and "
                "nothing to roll back."
            ),
            "missing": [],
        }

    elapsed = evidence.get("elapsed_ms")
    elapsed = 0 if elapsed is None else elapsed
    budget = evidence.get("budget_ms")
    budget = DEFAULT_BOOT_BUDGET_MS if budget is None else budget

    missing: list[str] = []
    if evidence.get("post_passed") is not True:
        missing.append("post")
    if evidence.get("packages_healthy") is not True:
        missing.append("packages")
    if evidence.get("converged") is not True:
        missing.append("converged")
    missing.sort(key=_utf16_key)

    if not missing:
        return {
            "decision": "confirm",
            "detail": (
                "The self-test passed, every wanted package is running and the reconciler "
                f"converged, {_js_number(elapsed)} ms into a {_js_number(budget)} ms budget. The "
                "slot is good."
            ),
            "missing": missing,
        }

    # A budget that is not a positive number is no budget, and no budget means waiting for ever
    # rather than rolling back on the first pass. The safe direction here is the patient one: an
    # unattended rollback triggered by a misconfigured budget is a working robot rebooted into an
    # older image for no reason.
    expired = (
        _is_finite(budget) and budget > 0 and _is_finite(elapsed) and elapsed >= budget
    )

    if expired:
        have = "has" if len(missing) == 1 else "have"
        return {
            "decision": "roll_back",
            "detail": (
                f"{_js_number(elapsed)} ms have passed and the budget is {_js_number(budget)} ms, "
                f"and {', '.join(missing)} still {have} not passed. Marking the slot bad; the "
                "bootloader returns to the previous one."
            ),
            "missing": missing,
        }

    return {
        "decision": "wait",
        "detail": (
            f"{_js_number(elapsed)} ms into a {_js_number(budget)} ms budget, with "
            f"{', '.join(missing)} still to pass. Saying nothing to the bootloader until they do "
            "or the budget expires."
        ),
        "missing": missing,
    }


# ── The wire shapes ──────────────────────────────────────────────────────────


def bundle_to_json(case_input: dict[str, Any]) -> Any:
    """``ota/bundle`` — may this image be installed?"""
    verdict = admit_bundle(case_input["bundle"], case_input["subject"])
    return {
        "admitted": verdict["admitted"],
        "faults": [
            {
                "code": f["code"],
                "severity": f["severity"],
                "message": f["message"],
                "subjects": list(f["subjects"]),
            }
            for f in verdict["faults"]
        ],
        "gated_components": list(verdict["gated_components"]),
    }


def boot_to_json(case_input: dict[str, Any]) -> Any:
    """``ota/boot`` — is the slot this robot just booted into good?"""
    verdict = confirm_boot(case_input)
    return {
        "decision": verdict["decision"],
        "detail": verdict["detail"],
        "missing": list(verdict["missing"]),
    }
