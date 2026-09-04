"""The watchdog chain (§46) — the Python edition of ``reference/typescript/watchdog.ts``.

Four layers, each catching a failure the one above it cannot see:

===  ==========================  =========================================  ===================
  #  Watchdog                    Catches                                    Response
===  ==========================  =========================================  ===================
  1  Kernel ``watchdog`` guard   the policy stopped producing commands      ``stop_cat2``
  2  systemd ``WatchdogSec``     the supervisor's reconcile loop hung       restart ``skynetd``
  3  SoC hardware WDT            the application processor wedged entirely  **reset the SoC**
  4  Safety MCU timer            the AP wedged *and* the reset failed       **cut drive enable**
===  ==========================  =========================================  ===================

Layer 3 is petted by the **Safety Kernel**, not by ``skynetd``, and that is what makes it mean
anything: the WDT is satisfied only while the thing that actually enforces the envelope is running
its cycle. A watchdog petted by a supervisor tells you the supervisor is alive, which is not the
question anybody is asking.

# A reset is not a stop

When the SoC resets the drives lose their command source, and what they then do is a property of
the **hardware**. Some hold, some brake, some fault — and **some coast**, and on such a body layer
3's own recovery path is a fall.

So `host_blocked_by_command_loss` refuses the host tier for a coasting body, and the shape of that
refusal is unusual and correct: **the middle rung is the unsafe one.** Guest is fine, because
enforcement is advisory and nothing relies on a hardware watchdog. Certified is fine, because layer
4 cuts drive enable independently of the processor being reset. Only host, where the OS owns the
control loop and layer 3 is the last line, is the configuration where a reset means a fall.

# Where Python is not JavaScript

`_present` refuses a boolean before it refuses anything else. A window declared as ``True`` is a
finite ``int`` in Python and would be compared as 1 ms, which nests inside every real window and
would report a chain as sound on the strength of a value that is not a duration at all. The
reference gets this free from ``typeof``.
"""

from __future__ import annotations

import math
from typing import Any, Final

__all__ = [
    "CHAIN",
    "COMMAND_LOSS_BEHAVIOURS",
    "WINDOWS",
    "check_nesting",
    "host_blocked_by_command_loss",
    "watchdog_to_json",
]

#: §46.2's vocabulary, in the order the specification lists it.
COMMAND_LOSS_BEHAVIOURS: Final = ["hold", "brake", "fault", "coast"]

#: The chain, as data (§46.1).
CHAIN: Final = [
    {
        "layer": 1, "name": "kernel_guard",
        "catches": "the policy stopped producing commands",
        "response": "stop_cat2", "required_from": "guest",
        "petted_by": "the policy runtime, every cycle",
    },
    {
        "layer": 2, "name": "supervisor",
        "catches": "the supervisor's reconcile loop hung",
        "response": "restart skynetd", "required_from": "guest",
        "petted_by": "skynetd, from inside the reconcile loop",
    },
    {
        "layer": 3, "name": "soc_wdt",
        "catches": "the application processor wedged entirely",
        "response": "reset the SoC", "required_from": "host",
        # §46.1's sentence, and the reason layer 3 means anything.
        "petted_by": "the Safety Kernel, not skynetd",
    },
    {
        "layer": 4, "name": "safety_mcu",
        "catches": "the AP wedged and the reset did not recover it",
        "response": "cut drive enable", "required_from": "certified",
        "petted_by": "an independent timer on the safety MCU",
    },
]

#: The four windows §46.3 nests, in order, innermost first.
WINDOWS: Final = [
    "kernel_cycle_worst_case_ms", "envelope_watchdog_ms", "soc_wdt_ms", "drive_command_timeout_ms",
]


def host_blocked_by_command_loss(on_command_loss: Any) -> str | None:
    """Whether a body may attempt the host tier, given what it says its drives do (§46.2).

    Returns the reason it may not, or ``None`` when it may.

    Two refusals, and they are the same refusal: **the recovery path has not been established to be
    survivable.** A body that coasts has one that demonstrably is not; a body that has declared
    nothing has one nobody has characterised, and treating silence as ``hold`` would be inventing
    the most convenient answer to the only question layer 3 turns on.
    """
    if on_command_loss == "coast":
        return (
            "this body's drives coast when commands stop, and layer 3 of the watchdog chain "
            "recovers by resetting the processor — which on this machine is a fall. The sound "
            "configuration is the certified tier, where layer 4 cuts drive enable independently of "
            "the processor (§46.2)"
        )
    if on_command_loss is None:
        return (
            "this body does not declare what its drives do when commands stop, and layer 3 of the "
            "watchdog chain recovers by resetting the processor. Silence is not `hold`: it is a "
            "recovery path nobody has characterised (§46.2)"
        )
    if on_command_loss not in COMMAND_LOSS_BEHAVIOURS:
        return (
            f"this body declares `on_command_loss: {on_command_loss}`, which is not one of hold, "
            "brake, fault or coast — so what it does while its processor resets is not something "
            "anybody can read off the descriptor (§46.2)"
        )
    return None


def _nesting_code(inner: str, outer: str) -> str:
    if inner == "kernel_cycle_worst_case_ms" and outer == "envelope_watchdog_ms":
        return "cycle_exceeds_watchdog"
    if inner == "envelope_watchdog_ms" and outer == "soc_wdt_ms":
        return "wdt_pre_empts_kernel"
    if inner == "soc_wdt_ms" and outer == "drive_command_timeout_ms":
        return "drive_faults_first"
    return "windows_not_nested"


def _nesting_consequence(inner: str, outer: str) -> str:
    if inner == "kernel_cycle_worst_case_ms" and outer == "envelope_watchdog_ms":
        return (
            "The kernel cannot complete a cycle inside its own command deadline, so the watchdog "
            "guard trips on a robot that is working correctly."
        )
    if inner == "envelope_watchdog_ms" and outer == "soc_wdt_ms":
        return (
            "The processor resets before the kernel executes its controlled stop, so the harsher "
            "recovery pre-empts the gentler one and the robot never performs its best available "
            "behaviour (§46.3)."
        )
    if inner == "soc_wdt_ms" and outer == "drive_command_timeout_ms":
        return (
            "The drive faults on its own before the hardware watchdog fires, so layer 3 is present, "
            "configured, and never reached (§46.3)."
        )
    return "The windows must nest innermost-first for every layer above to be reachable."


def _present(value: Any) -> bool:
    """A declared, finite duration. A boolean is neither, and in Python it is an ``int``."""
    if isinstance(value, bool) or value is None:
        return False
    return isinstance(value, (int, float)) and math.isfinite(value)


def check_nesting(windows: dict[str, Any]) -> dict[str, Any]:
    """Check §46.3's nesting.

    Adjacent pairs only: comparing every pair would report the same inversion up to three times and
    bury the one an operator has to fix.

    **Strictly less than.** Two watchdogs with the same window is a race whose winner is decided by
    interrupt latency, and a chain whose order depends on which timer fires first is not a chain.

    A gap in the middle does not stop the walk: with no ``envelope_watchdog_ms`` declared, the
    kernel cycle and the SoC window are still worth comparing, because that inversion is real
    whatever sits between them.
    """
    pairs = [(name, windows.get(name)) for name in WINDOWS]

    missing = [name for name, value in pairs if not _present(value)]
    declared = [(name, value) for name, value in pairs if _present(value)]

    faults: list[dict[str, Any]] = []
    checked: list[list[str]] = []

    for index in range(len(declared) - 1):
        inner_name, inner = declared[index]
        outer_name, outer = declared[index + 1]
        checked.append([inner_name, outer_name])
        if inner < outer:
            continue
        faults.append({
            "code": _nesting_code(inner_name, outer_name),
            "inner": inner_name,
            "outer": outer_name,
            "detail": (
                f"{inner_name} is {_ms(inner)} ms and {outer_name} is {_ms(outer)} ms, so the outer "
                f"window is not outside the inner one. {_nesting_consequence(inner_name, outer_name)}"
            ),
        })

    return {
        "faults": faults,
        "missing": missing,
        "checked": checked,
        # Every pair that could be compared nests. **Not** the same as "the chain is sound".
        "ordered": len(faults) == 0,
        # Every window is declared *and* every pair nests.
        "complete": len(faults) == 0 and len(missing) == 0,
    }


def _ms(value: float | int) -> str:
    """A duration inside a sentence, rendered the way the reference's template literal renders it.

    JavaScript writes a whole-valued double without a fractional part — ``${20}`` is ``20``, never
    ``20.0`` — so a Python ``float`` that arrived from JSON has to be printed the same way or the
    detail string diverges by two characters on every fault.
    """
    if isinstance(value, float) and value.is_integer():
        return str(int(value))
    return str(value)


def watchdog_to_json(on_command_loss: Any, windows: dict[str, Any]) -> Any:
    """``shapeWatchdog`` — the chain, the tier refusal, and the nesting report."""
    report = check_nesting(windows)
    return {
        "chain": [
            {
                "layer": layer["layer"],
                "name": layer["name"],
                "catches": layer["catches"],
                "response": layer["response"],
                "required_from": layer["required_from"],
                "petted_by": layer["petted_by"],
            }
            for layer in CHAIN
        ],
        "on_command_loss": on_command_loss if on_command_loss is not None else None,
        "host_blocked": host_blocked_by_command_loss(on_command_loss),
        "nesting": {
            # An array, not an object: canonical JSON sorts an object's keys, and these four
            # windows mean nothing except in order. The object form renders them alphabetically,
            # which on a panel about nesting is worse than rendering nothing.
            "declared": [
                {"window": name, "ms": windows.get(name) if windows.get(name) is not None else None}
                for name in WINDOWS
            ],
            "faults": [
                {
                    "code": fault["code"],
                    "inner": fault["inner"],
                    "outer": fault["outer"],
                    "detail": fault["detail"],
                }
                for fault in report["faults"]
            ],
            "missing": list(report["missing"]),
            "ordered": report["ordered"],
            "complete": report["complete"],
        },
    }
