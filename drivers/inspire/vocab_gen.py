"""Generate the inspire entry of `drivers/fault-vocabulary.json` from the maker's own manuals, and splice it in.

    PYTHONPATH=bindings/python:drivers/inspire python3 drivers/inspire/vocab_gen.py <series manual pdf> <DFTP manual pdf> [--write]

The tables are *generated* from the maker's own documents and not retyped, the way ODrive's was from its DBC and moteus's from its firmware: the error byte's five bits and the status byte's seven codes are read
out of the text of the two manuals (`pdftotext -layout`, poppler), after each PDF's sha256 is checked against the pin below, and every number is asserted equal to what `skynet_inspire.frames` carries before anything
is written (a table that disagreed with the driver would hold the contract to a driver nobody shipped). Two things the reading established are written into the entry's `source` and `not_covered`: the DFTP manual lists
the error bits and **no status code table at all**, so the meaning of a status code on a DFTP rests on the series manual; and the two manuals' error tables agree. What the *class* of each code is (`ok`, `fault`) is this
repository's statement about what the driver must do with it, and is written here.

Run without `--write` it prints the entry; with it, it adds or replaces `"inspire"` in the file as text, leaving every other byte alone.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys

from skynet_inspire import frames

SERIES_SHA256 = "f16c9247a4818f0891aba79aae48b025d3593f57520aa52864e81d2be16156b4"      # RH56 series user manual PRJ-02-TS-U-001 V1.0.9 (January 2024)
DFTP_SHA256 = "4cad22265b489ec8e3e56e71f35f84be29d627e37f987b657f9d6a1b469430c5"        # RH56DFTP user manual PRJ-02-TS-U-010 V1.0.0 (December 2024)


def manual_text(path: str, pin: str) -> str:
    digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
    assert digest == pin, f"{path} is sha256 {digest}, not the pin {pin}"
    return subprocess.run(["pdftotext", "-layout", path, "-"], capture_output=True, text=True, check=True).stdout


def error_bits(text: str) -> dict[int, str]:
    out: dict[int, str] = {}
    for line in text.splitlines():
        match = re.match(r"^\s*Bit([0-9])\s{2,}(\S.*?)\s*$", line)
        if match:
            out[int(match.group(1))] = match.group(2)
    return out


def status_codes(text: str) -> dict[int, str]:
    out: dict[int, str] = {}
    start = text.find("Meaning of status codes")
    if start < 0:
        return out
    end = text.find("TEMP(m)", start)
    for line in text[start:end].splitlines():
        match = re.match(r"^\s*([0-9])\s{3,}(\S.*?)\s*$", line)
        if match:
            out[int(match.group(1))] = match.group(2)
    return out


def entry(series_pdf: str, dftp_pdf: str) -> dict:
    series, dftp = manual_text(series_pdf, SERIES_SHA256), manual_text(dftp_pdf, DFTP_SHA256)
    bits, dftp_bits = error_bits(series), error_bits(dftp)
    squash = lambda table: {bit: re.sub(r"[-\s]+", "", name).lower() for bit, name in table.items()}      # one manual writes over-current, the other overcurrent
    assert squash(bits) == squash(dftp_bits), f"the manuals' error tables disagree: {bits} and {dftp_bits}"
    statuses, dftp_statuses = status_codes(series), status_codes(dftp)
    assert not dftp_statuses, "the DFTP manual has a status table now: this generator's account of it (none) is out of date"
    assert sorted(bits) == sorted(frames.ERROR_BITS) and sorted(statuses) == sorted(frames.STATUS_NAMES), "the manuals' tables are not the driver's"
    assert len(bits) == 5 and len(statuses) == 7, (bits, statuses)

    error_codes = {"0": {"name": "0x00 no error", "class": "ok"}}
    for bit in sorted(bits):
        error_codes[str(1 << bit)] = {"name": f"0x{1 << bit:02X} bit {bit}: {bits[bit]}", "class": "fault"}
    status_table = {}
    for code in sorted(statuses):
        status_table[str(code)] = {"name": f"{code} {statuses[code]}", "class": "ok" if code in (0, 1, 2, 3) else "fault"}
    word = "joint number (from 1) in bits 16 and up, the status byte in bits 8 to 15 and the error byte in the low byte, of the lowest-numbered joint that holds a fault"
    return {
        "subject": "drivers/inspire/faultvocab.py",
        "vocabularies": {
            "error_byte": {
                "channel": "ERROR(m), six bytes at 1606 to 1611, one per actuator, read every cycle (with STATUS, in one twelve-byte read)",
                "source": f"Inspire Robots' own manuals: the RH56 series user manual PRJ-02-TS-U-001 V1.0.9 §2.4.18 (sha256 {SERIES_SHA256[:12]}) and the RH56DFTP user manual PRJ-02-TS-U-010 V1.0.0 §2.6.18 (sha256 {DFTP_SHA256[:12]}): "
                          "the five bits, generated from the text of both (the two tables are identical) and not retyped. Read, not measured: no hand was reached, and the maker's simulator does not exist.",
                "kind": "alarm",
                "why": "an error bit is an actuator that has stopped for cause. Four of the five (locked rotor, over-current, abnormal operation of the motor, a communication error) clear when written to CLEAR_ERROR, which is the "
                       "`clear_errors` motion and never the driver's own; over-temperature is \"not clearable\" and clears when the actuator cools. The manual does not say what a hand does with a command while an error is "
                       "held, so none is sent: a hand holding any error bit is FAULT with DEGRADED and a command is refused, because answering one OK would be success the driver did not have.",
                "codes": error_codes,
                "unmapped": "fault",
                "unmapped_because": "the byte is a bitset the driver reads whole: a bit the table does not define (5, 6, 7) is an error from a firmware newer than this table, and masking it to healthy would clear a fault nobody read. "
                                    "The probes are the three undefined single bits and two combinations of them with a defined one.",
                "probes": [32, 64, 128, 33, 192],
                "fault_code": "derived",
                "fault_code_is": word,
            },
            "status_byte": {
                "channel": "STATUS(m), six bytes at 1612 to 1617, one per actuator, read every cycle (with ERROR)",
                "source": f"the RH56 series user manual PRJ-02-TS-U-001 V1.0.9 §2.4.19 (sha256 {SERIES_SHA256[:12]}): the seven codes, generated from its text. **The RH56DFTP manual lists the STATUS register and gives no table of its codes**, "
                          "so on a DFTP the meaning of a status code rests on the series manual's. Read, not measured.",
                "kind": "alarm",
                "why": "status 0 to 3 are a finger doing its job (unclenching, grasping, stopped at the target, stopped at the force limit) and say nothing is wrong. 5, 6 and 7 are the maker's own words for a stop for cause "
                       "(current protection, locked rotor, actuator fault): FAULT with DEGRADED, and a command is refused. Code 4 and everything above 7 are in no table.",
                "codes": status_table,
                "unmapped": "fault",
                "unmapped_because": "a driver that cannot tell what a finger is doing does not command it: a code the table does not list is a state from a firmware newer than this table, and reading it as healthy would be a guess "
                                    "about the one thing the manual leaves open. The probes are the hole at 4 and values above 7.",
                "probes": [4, 8, 16, 64, 128, 255],
                "fault_code": "derived",
                "fault_code_is": word,
            },
        },
        "not_covered": [
            {"what": "CURRENT (1594), FORCE_ACT (1582) and TEMP (1618)",
             "why": "none is a fault. The current protection, the locked rotor and the over-temperature are the hand's own, and each raises its own error bit and status, which are covered; this driver reads the three on demand (the `status` motion) "
                    "and compares none with a limit of its own, because a threshold this driver chose would be a second, unsourced one beside the maker's."},
            {"what": "the Modbus and CAN faults of the DFTP and the CAN supplement's own",
             "why": "this driver speaks the RS-485 private protocol and refuses Modbus by name (the byte registers' order under Modbus is not in the manuals); a CAN transport is not built, so no error frame of either is read."},
            {"what": "the hand's own emergency stop",
             "why": "the manuals document none: the `estop` class is a maker's stop chain being open, and an Inspire hand has no such signal on the wire. The stop here is the driver's own latch and the hold it writes, which the suite holds."},
        ],
    }


def _end_of_object(text: str, start: int) -> int:
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
    if len(argv) < 2:
        print(__doc__)
        return 2
    result = entry(argv[0], argv[1])
    body = json.dumps(result, indent=2, ensure_ascii=False)
    body = "\n".join(("    " + line) if index else line for index, line in enumerate(body.split("\n")))
    if "--write" not in argv[2:]:
        print(body)
        return 0
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "fault-vocabulary.json")
    text = open(path, encoding="utf-8", newline="").read()
    assert "\r\n" not in text
    tail = "\n  }\n}\n"
    assert text.endswith(tail), repr(text[-20:])
    marker = '\n    "inspire": '
    if marker in text:
        start = text.index(marker) + len(marker)
        end = _end_of_object(text, start)
        text = text[:start] + body + text[end:]
    else:
        text = text[: -len(tail)] + ",\n    \"inspire\": " + body + tail
    json.loads(text)
    with open(path, "w", encoding="utf-8", newline="") as handle:
        handle.write(text)
    print(f"wrote the inspire entry to {os.path.normpath(path)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
