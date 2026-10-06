"""Generate the moteus entry of `drivers/fault-vocabulary.json` from mjbots's own firmware, and splice it in.

    PYTHONPATH=bindings/python:drivers/canopen:drivers/moteus python3 drivers/moteus/vocab_gen.py <moteus checkout> [--write]

The tables are *generated* from the maker's own source and not retyped, the way ODrive's was from its DBC and Piper's from its SDK: the numbers and names of the
fault, limiting and mode codes are `fw/error.h`'s `errc` and `fw/bldc_servo_structs.h`'s `BldcServoMode`, read at the pin `layout_check.py` holds, and every number is
asserted equal to what `skynet_moteus.protocol` carries before anything is written (a table that disagreed with the driver would hold the contract to a driver nobody
shipped). What the *class* of each code is (`ok`, `degraded`, `fault`) is this repository's statement about what the driver must do with it, and is written here.

Run without `--write` it prints the entry; with it, it adds or replaces `"moteus"` in the file as text, leaving every other byte alone.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

from skynet_moteus import protocol as p

PIN = "8f747f4ac448d117c1390b77ca2d80c27e751a56"


def firmware_enum(text: str, header: str) -> dict[str, tuple[int, str]]:
    """The entries of the enum that opens with ``header``: name to (value, trailing comment). The same reader `layout_check.py` uses."""
    body = text.split(header, 1)[1].split("};", 1)[0]
    entries: dict[str, tuple[int, str]] = {}
    value = 0
    for raw in body.splitlines():
        code, _, comment = raw.partition("//")
        code = code.strip()
        if not code or code.startswith("*") or code.startswith("/*"):
            continue
        for item in code.split(","):
            item = item.strip()
            if not item:
                continue
            if "=" in item:
                name, number = (part.strip() for part in item.split("=", 1))
                value = int(number, 0)
            else:
                name = item
            entries[name] = (value, comment.strip())
            value += 1
    return entries


def entry(checkout: str) -> dict:
    head = subprocess.run(["git", "-C", checkout, "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    assert head == PIN, f"{checkout} is at {head}, not the pin {PIN}"
    codes = firmware_enum(open(os.path.join(checkout, "fw", "error.h"), encoding="utf-8").read(), "enum class errc {")
    modes = firmware_enum(open(os.path.join(checkout, "fw", "bldc_servo_structs.h"), encoding="utf-8").read(), "enum BldcServoMode {")
    modes.pop("kNumModes", None)
    faults = {v: n for n, (v, _) in codes.items() if 32 <= v <= 50}
    limits = {v: (n, c) for n, (v, c) in codes.items() if v >= 96}
    assert sorted(faults) == sorted(p.FAULTS) and sorted(limits) == sorted(p.LIMITING), "the firmware's codes are not the driver's"
    by_mode = {v: n for n, (v, _) in modes.items()}
    assert sorted(by_mode) == sorted(p.MODES), "the firmware's modes are not the driver's"

    source = f"mjbots's own firmware, `mjbots/moteus` at {PIN[:8]} (2026-09-27, the pin `drivers/moteus/layout_check.py` holds)"
    read_only = "Read, not measured: the maker's firmware raises these on events this repository cannot cause, and mjbots publishes no simulator of the controller."

    fault_codes: dict[str, dict] = {"0": {"name": "0 kSuccess (no fault)", "class": "ok"}}
    for value in sorted(faults):
        fault_codes[str(value)] = {"name": f"{value} {faults[value]}", "class": "fault"}
    limit_codes: dict[str, dict] = {"0": {"name": "0 kSuccess (nothing is limiting the output)", "class": "ok"}}
    for value in sorted(limits):
        name, comment = limits[value]
        limit_codes[str(value)] = {"name": f"{value} {name}" + (f" ({comment})" if comment else ""), "class": "degraded"}
    mode_codes: dict[str, dict] = {}
    for value in sorted(by_mode):
        mode_codes[str(value)] = {"name": f"{value} {by_mode[value]}", "class": "ok" if value == p.MODE_POSITION else "fault"}

    return {
        "subject": "drivers/moteus/faultvocab.py",
        "vocabularies": {
            "fault": {
                "channel": "the fault register (0x00f) while the mode register (0x000) reads 1, fault; both are in the query every cycle",
                "source": f"{source}: `fw/error.h`, `errc` 32 to 50, generated and not retyped, with `docs/protocol/registers.md`'s table of the same codes held name for name by the layout check. {read_only}",
                "kind": "alarm",
                "why": "mode 1 is a controller that has taken its output stage off and ignores every command but a stop (`ISR_MaybeChangeMode`: from fault a command returns). The driver faults, refuses commands whole, and never writes the stop that clears it: the clear is a person's act at the controller, and `open` is the only place an axis is brought up. The code is carried in the fault word, with the joint and the mode beside it.",
                "codes": fault_codes,
                "unmapped": "fault",
                "unmapped_because": "in mode 1 the register holds a code from a firmware that may be newer than this table, and every state in which a controller has faulted is a fault whatever its number: reading an unknown one as healthy would clear a fault nobody read. The probes are values the table does not define, including the firmware's own transport codes (1 to 7), which are not servo states and which a faulted controller could only carry by mistake.",
                "probes": [1, 5, 8, 31, 51, 64, 95, 127],
                "fault_code": "derived",
                "fault_code_is": "joint number (from 1) in bits 16 and up, the mode (1) in bits 8 to 15, and the injected code in the low byte",
            },
            "limiting": {
                "channel": "the fault register (0x00f) while the mode register reads 10, position; both are in the query every cycle",
                "source": f"{source}: `fw/error.h`, `errc` 96 to 105. The register reference (`registers.md`) lists 96 to 103; the firmware defines 104 (flux braking) and 105 (field weakening) as well, which it does not (F285), and the table is the firmware's because it is the firmware that sends them. {read_only}",
                "kind": "warning",
                "why": "in a valid control mode a non-zero code says which function is limiting the output (`registers.md`: *not faults*). The controller keeps running and takes commands, so the driver reads DEGRADED alone and carries the code in the fault word, joint and mode beside it, so the black box says which function; an unlisted non-zero code is never read as healthy.",
                "codes": limit_codes,
                "unmapped": "degraded",
                "unmapped_because": "a warning channel's floor is `degraded`: a code the table does not list is a limit from a firmware newer than this table, which the controller runs on through, and which is not read as healthy and not as a stop.",
                "probes": [1, 31, 77, 95, 106, 127],
                "fault_code": "derived",
                "fault_code_is": "joint number (from 1) in bits 16 and up, the mode (10) in bits 8 to 15, and the injected code in the low byte",
            },
            "mode": {
                "channel": "the mode register (0x000), read every cycle for each controller",
                "source": f"{source}: `fw/bldc_servo_structs.h`, `BldcServoMode`, sixteen values, generated and not retyped, with the maker's Python client's `Mode` held against it by the layout check. {read_only}",
                "kind": "alarm",
                "why": "position is the one mode this driver put an axis in and the one in which it takes commands. Any other mode in a session is something done to the axis by someone else or by the controller itself — stopped, a preparing phase, another master's pwm, voltage, current, stay-within or measure-inductance, a watchdog timeout, or a zero-velocity or brake stop this driver did not write — and the driver faults, refuses commands whole, and sends nothing to bring the axis back: a driver that re-enabled an axis a person had stopped would be starting a machine whose reason for stopping nobody had looked at. The stop it writes itself is read against its own latch (the suite's, `emergency_stop`).",
                "codes": mode_codes,
                "unmapped": "fault",
                "unmapped_because": "the mode is a value the driver compares with position: a value the table does not define is not position. The probes are values the register can carry as an int8 above the firmware's last mode.",
                "probes": [16, 17, 32, 64, 127],
                "fault_code": "derived",
                "fault_code_is": "joint number (from 1) in bits 16 and up and the mode in bits 8 to 15, with the fault register in the low byte (zero outside mode 1)",
            },
        },
        "not_covered": [
            {
                "what": "the firmware's codes 1 to 7 (DMA stream and UART errors)",
                "why": "they are the diagnostic port's transport errors, raised on the controller's serial interface and not states of the servo; no register this driver reads carries them, and the contract's probes include some of them to show a faulted controller holding one is read as a fault.",
            },
            {
                "what": "the motor driver's own fault registers (0x140 and 0x141)",
                "why": "they carry the gate driver's bit fields only when the fault code is 33 (motor driver fault), which this driver already reads as a fault with the code in its word; the bits say *why* the gate driver faulted (under-voltage, over-current, over-temperature), which a person reads from the controller, and this driver would have no different thing to do with any of them.",
            },
            {
                "what": "the driver's own stop (modes 0, 12 and 15 after `emergency_stop`)",
                "why": "the contract's `estop` class is the maker's stop chain being open, and a moteus has no such signal on the wire: the stop here is the driver's own latch and the mode the operator named. That is held by the suite (the latch against an arm that is deaf to the stop, the mode written, the release that is honest only where the driver stays on), not by a code a maker defined.",
            },
            {
                "what": "the home state (0x00c), the voltage and the temperature",
                "why": "none is a fault. The home state is reported as the homing bits of the state word (output-referenced reads as homed, anything else as pending) and held by the suite; the voltage and the temperature are read and not compared with any limit of this driver's own, because the controller's own thresholds raise fault codes 34, 38 and 40 and a number this driver compared with a limit it chose would be a second, unsourced threshold beside the maker's.",
            },
        ],
    }


def _end_of_object(text: str, start: int) -> int:
    """The index just past the JSON object that begins at ``text[start]`` (a ``{``), reading strings so a brace inside one does not count."""
    assert text[start] == "{"
    depth = 0
    in_string = False
    index = start
    while index < len(text):
        char = text[index]
        if in_string:
            if char == chr(92):
                index += 1
            elif char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index + 1
        index += 1
    raise ValueError("the object does not end")


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2
    result = entry(argv[0])
    body = json.dumps(result, indent=2, ensure_ascii=False)
    body = "\n".join(("    " + line) if index else line for index, line in enumerate(body.split("\n")))
    if "--write" not in argv[1:]:
        print(body)
        return 0
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "fault-vocabulary.json")
    text = open(path, encoding="utf-8", newline="").read()
    assert "\r\n" not in text
    tail = "\n  }\n}\n"
    assert text.endswith(tail), repr(text[-20:])
    marker = '\n    "moteus": '
    if marker in text:
        start = text.index(marker) + len(marker)
        end = _end_of_object(text, start)
        text = text[:start] + body + text[end:]
    else:
        text = text[: -len(tail)] + ",\n    \"moteus\": " + body + tail
    json.loads(text)
    with open(path, "w", encoding="utf-8", newline="") as handle:
        handle.write(text)
    print(f"wrote the moteus entry to {os.path.normpath(path)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
