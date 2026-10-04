"""This driver's bytes against SCHUNK's own client, which is run (DRY-COVERAGE-PLAN.md item 12.E.9).

    SCHUNK_REPO=/path/to/schunk_mechatronic_gripper PYTHONPATH=../../bindings/python:. <python with schunk_gripper_library> layout_check.py

`schunk_gripper_library` is the maker's own client (GPL-3.0), installed here by `pip install ./schunk_gripper_library` from a checkout of the maker's repository, and **imported unmodified**; nothing of it is in this tree.
This check refuses to run without it (a layout check that compared nothing would pass for ever). Four things are held to it:

1. **Constants:** the valid and reserved control and status bits, the module and fieldbus enumerations, the parameters this driver reads (name, registers, type), the two instances and the three offsets in the input frame.
2. **The output frame:** a thousand random commands built by the maker's own setters (`clear_plc_output`, `set_control_bit`, `set_target_position`, `set_target_speed`, `set_gripping_force`) and by `frames.command_output`, on EtherNet/IP and on PROFINET.
3. **The input frame:** a thousand random frames read by the maker's getters (`get_status_bit`, `get_actual_position`, `get_error_code`) and by `frames.parse_input`. The error byte is compared on the fieldbuses this driver drives, and the PROFINET one is *not*: the maker's client reads byte 12 there and its own PROFINET dump has the code in byte 15.
4. **The handshake:** the maker's client **connected to a module** (the simulated one written here) and made to acknowledge, move, grip, release, fast stop and stop; this driver doing the same; the control words each wrote to the module compared, intent by intent.

What it cannot show: that the maker's client is right (it is the maker's), or that a real module answers it as the maker's simulator does.
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import random
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(ROOT, "..", "..", "bindings", "python"))
sys.path.insert(0, ROOT)

try:
    from schunk_gripper_library.driver import Driver
    from importlib.resources import files
except ImportError as error:  # pragma: no cover
    print(f"::error::SCHUNK's own client is not importable here ({error}). Nothing was compared: install it with `pip install ./schunk_gripper_library pytest` inside a checkout of SCHUNK-SE-Co-KG/schunk_mechatronic_gripper.")
    raise SystemExit(1)

import skynet_rhal as rhal

from skynet_schunk import frames
from skynet_schunk.bridge import SchunkBridge, TOKEN
from skynet_schunk.frames import Bit, Status
from skynet_schunk.gripper_sim import SimulatedGripper

CHECKS = 0
FAILURES: list[str] = []


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    print(("  ok    " if held else "  FAIL  ") + what, flush=True)
    if not held:
        FAILURES.append(what)
        if detail:
            print(f"        {detail}", flush=True)


def section(title: str) -> None:
    print(f"\n{title}", flush=True)


def maker(fieldbus: str) -> Driver:
    d = Driver()
    d.fieldbus = fieldbus
    return d


def main() -> int:
    repo = os.environ.get("SCHUNK_REPO", "")
    if repo and os.path.isdir(os.path.join(repo, ".git")):
        commit = subprocess.run(["git", "-C", repo, "log", "-1", "--format=%H %cs"], capture_output=True, text=True).stdout.strip()
        print(f"the maker's repository: {commit}")
    config = files("schunk_gripper_library").joinpath("config")

    section("constants — the maker's client and this driver agree")
    d = maker("EI")
    check(list(frames.Status.VALID) == d.valid_status_bits and list(frames.Status.VALID_CONTROL) == d.valid_control_bits, "the valid status and control bits are the maker's")
    check(sorted(set(range(32)) - set(frames.Status.VALID)) == sorted(d.reserved_status_bits) and sorted(set(range(32)) - set(frames.Status.VALID_CONTROL)) == sorted(d.reserved_control_bits),
          "and so are the reserved ones")
    check((d.plc_input, d.plc_output, d.error_byte, d.warning_byte, d.additional_byte) == (frames.Instance.INPUT, frames.Instance.OUTPUT, 12, 14, 15), "the two instances and the offsets of the error, warning and additional bytes are the maker's")
    modules = {int(k): v for k, v in json.load(open(str(config.joinpath("module_types.json")))).items()}
    fieldbuses = {int(k): v for k, v in json.load(open(str(config.joinpath("fieldbus_types.json")))).items()}
    check(modules == frames.MODULE_TYPES and fieldbuses == frames.FIELDBUS, "the module and fieldbus enumerations are the maker's, entry for entry")
    readable = json.load(open(str(config.joinpath("readable_parameters.json"))))
    mismatched = [p for p, spec in frames.PARAMETERS.items() if p not in readable or (readable[p]["name"], readable[p]["registers"], readable[p]["type"]) != (spec.name, spec.registers, spec.kind)]
    check(not mismatched, f"every parameter this driver reads is one the maker's client reads, with the same name, size and type ({len(frames.PARAMETERS)} of them)", str(mismatched))
    bits_named = {bit: name for bit, name in Status.NAMES.items()}
    check(all(bit in d.valid_status_bits for bit in bits_named), "and every status bit this driver names is one the maker's client lists as valid")
    check(frames.Bit.FAST_STOP == 0 and frames.Bit.ACKNOWLEDGE == 2 and frames.Bit.MOVE_ABSOLUTE == 13 and frames.Bit.GRIP_WORKPIECE == 12 and frames.Bit.GRIP_AT_POSITION == 16 and frames.Bit.RELEASE_WORKPIECE == 11
          and frames.Bit.STOP == 1 and frames.Bit.GRIP_DIRECTION == 7, "the control bits it writes are the numbers the maker's client writes (read from `fast_stop`, `acknowledge`, `stop`, `move_to_position`, `grip` and `release`)")

    section("the output frame — a thousand random commands, built both ways")
    rng = random.Random(1210)
    writable = [b for b in frames.Status.VALID_CONTROL if b != Bit.FAST_STOP]
    differing = 0
    first = ""
    total = 0
    for fieldbus in ("EI", "PN"):
        d = maker(fieldbus)
        for _ in range(500):
            bits = {b: True for b in writable if rng.random() < 0.15}
            fast = rng.random() < 0.1
            position, speed, force = rng.randint(0, 120000), rng.randint(10000, 110000), rng.randint(0, 650)
            d.clear_plc_output()
            for b in bits:
                d.set_control_bit(b, True)
            if fast:
                d.set_control_bit(0, False)
            d.set_target_position(position)
            d.set_target_speed(speed)
            d.set_gripping_force(force)
            ours = frames.command_output({**bits, Bit.FAST_STOP: not fast}, fieldbus, position=position, speed=speed, force=force)
            total += 1
            if bytes(d.plc_output_buffer) != ours:
                differing += 1
                first = first or f"{fieldbus} bits={sorted(bits)} fast={fast}: maker {bytes(d.plc_output_buffer).hex()} ours {ours.hex()}"
    check(differing == 0, f"all {total} frames are byte for byte the maker's, on EtherNet/IP and on PROFINET", first)
    d = maker("EI")
    d.clear_plc_output()
    check(bytes(d.plc_output_buffer) == frames.clear_output(), "and the cleared frame is the maker's")

    section("the input frame — a thousand random frames, read both ways")
    mismatches = []
    for fieldbus in ("EI", "EC", "PN"):
        d = maker(fieldbus)
        for _ in range(350):
            raw = bytes(rng.getrandbits(8) for _ in range(16))
            d.set_plc_input(raw.hex().upper())
            ours = frames.parse_input(raw.hex().upper(), fieldbus)
            for bit in frames.Status.VALID:
                if d.get_status_bit(bit) != ours.bit(bit):
                    mismatches.append(f"{fieldbus} bit {bit} of {raw.hex()}")
            if d.get_actual_position() != ours.position_um:
                mismatches.append(f"{fieldbus} position of {raw.hex()}: maker {d.get_actual_position()} ours {ours.position_um}")
            if fieldbus != "PN":
                # The maker's getters answer `hex(byte)` ("0x3", not "0x03"): compared as numbers.
                if (int(d.get_error_code(), 16), int(d.get_warning_code(), 16), int(d.get_additional_code(), 16)) != (ours.error, ours.warning, ours.additional):
                    mismatches.append(f"{fieldbus} codes of {raw.hex()}")
    check(not mismatches, "every status bit and the position are the maker's on all three fieldbuses, and the error, warning and additional bytes on the two this driver drives (1050 frames)", "; ".join(mismatches[:3]))
    d = maker("PN")
    d.set_plc_input("80000080FFFFFE8700000000000000D9")
    check(d.get_error_code() == "0x0" and d.plc_input_buffer[15] == 0xD9, "and the maker's client, given the maker's own PROFINET dump, reports error 0x0 while byte 15 holds 0xD9: the disagreement this driver refuses PROFINET on")

    section("the handshake — the maker's client connected to a module, and this driver on another")
    with SimulatedGripper(start_ready=False, time_scale=20.0, workpiece_um=40000) as sim:
        sim._data["0x0380"] = "0000"
        client = Driver()
        check(client.connect(host="127.0.0.1", port=sim.port, update_cycle=0.05), "the maker's client connects to the simulated module and reads its parameters")
        check(client.gripper_type == "EGU_60_EI_M_B" and client.module_parameters["min_pos"] == 0 and client.module_parameters["max_pos"] == 120000 and client.module_parameters["max_vel"] == 110000,
              "and decodes them as this driver does (min 0, max 120000 um, max speed 110000 um/s)", str(client.module_parameters))
        mark = [len(sim.requests)]

        def writes() -> list[str]:
            out = [f["value"] for m, p, f in sim.requests[mark[0]:] if m == "POST" and isinstance(f, dict)]
            mark[0] = len(sim.requests)
            return out

        theirs: dict[str, list[str]] = {}
        client.acknowledge()
        theirs["acknowledge"] = writes()
        client.move_to_position(position=60000, velocity=20000)
        theirs["move"] = writes()
        result = client.grip(force=50)
        grip_position = sim.position_um
        theirs["grip"] = writes()
        client.release()
        theirs["release"] = writes()
        client.fast_stop()
        theirs["fast_stop"] = writes()
        client.acknowledge()
        writes()
        client.stop()
        theirs["stop"] = writes()
        client.disconnect()
        check(result == Driver.GripResult.WORKPIECE_GRIPPED and abs(grip_position - 40000) <= 1, "the maker's client sees the simulated module do what it expects: a grip on the workpiece reads gripped, with the jaw at the workpiece", f"{result} at {grip_position} um")

    with SimulatedGripper(start_ready=False, time_scale=20.0, workpiece_um=40000) as sim:
        bridge = SchunkBridge()
        with contextlib.redirect_stderr(io.StringIO()):
            code = bridge.open(json.dumps({"host": "127.0.0.1", "port": sim.port, "stop_mode": "fast", "speed": 0.02, "simulator": True}))
        check(code == rhal.SKR_OK, "this driver opens the same module, in its fast stop")
        mark = [len(sim.requests)]

        def writes_ours() -> list[str]:
            out = [f["value"] for m, p, f in sim.requests[mark[0]:] if m == "POST" and isinstance(f, dict)]
            mark[0] = len(sim.requests)
            return out

        ours: dict[str, list[str]] = {}
        bridge.release_stop()
        ours["acknowledge"] = writes_ours()
        time.sleep(0.15)
        bridge.extension(TOKEN, b"move position=0.06 speed=0.02")
        ours["move"] = writes_ours()
        bridge.extension(TOKEN, b"grip force=50")
        ours["grip"] = writes_ours()
        bridge.extension(TOKEN, b"release")
        ours["release"] = writes_ours()
        bridge.emergency_stop()
        time.sleep(0.5)
        ours["fast_stop"] = writes_ours()
        bridge.release_stop()
        writes_ours()
        bridge.close()
    for intent in ("acknowledge", "move", "grip", "release", "fast_stop"):
        check(theirs[intent] == ours[intent] and len(ours[intent]) == 2, f"{intent}: the two frames the maker's client writes (the cleared word, then the command) are the two this driver writes", f"maker {theirs[intent]} ours {ours[intent]}")
    with SimulatedGripper(time_scale=20.0) as sim:
        bridge = SchunkBridge()
        with contextlib.redirect_stderr(io.StringIO()):
            bridge.open(json.dumps({"host": "127.0.0.1", "port": sim.port, "stop_mode": "controlled", "speed": 0.02, "simulator": True}))
        mark = [len(sim.requests)]
        bridge.emergency_stop()
        time.sleep(0.5)
        stop = [f["value"] for m, p, f in sim.requests[mark[0]:] if m == "POST" and isinstance(f, dict)]
        bridge.release_stop()
        bridge.close()
    check(theirs["stop"] == stop and len(stop) == 2, "stop: and the controlled stop is the maker's `stop()`, the stop bit with bit 0 high", f"maker {theirs['stop']} ours {stop}")

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
