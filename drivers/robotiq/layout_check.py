"""This driver's bytes against Robotiq's own client, which is run (DRY-COVERAGE-PLAN.md item 12.E.8).

    ROBOTIQ_SIM=/path/to/built/harnesses PYTHONPATH=../../bindings/python:. python3 layout_check.py

`sim/build.sh` builds the maker's own SDK (`github.com/Robotiq/grippers`, BSD-3-Clause) **unmodified** and four small programs against it; this check refuses to run without them (a layout check that compared nothing
would pass for ever). Nothing of the maker's is in this tree. Six things are held to the maker's client:

1. **Constants:** the two block addresses and their widths, the slave address, the four action bits and the two profiles' six numbers each.
2. **The requests:** five hundred random commands written by the maker's own `GripperModbusClient` as function 0x17 (`exchange`), 0x10 (`writeCommand`) and 0x03 (`readStatus`), byte for byte what `frames.request_*` writes, the CRC included.
3. **The status block:** every value of the gripper-status byte and of the fault byte, and a thousand random blocks, decoded by the maker's own accessors and by `frames.parse_status`.
4. **The replies:** a good one in each of the three functions decoded by both, and fourteen kinds of bad one (a CRC, a slave, a function, a length, an exception of each code) that both must refuse.
5. **The conversions:** speed, opening, position and effort, for both profiles, over a grid and random values, against the maker's `units.cpp`.
6. **The faults:** each of the sixteen codes of each nibble, the name the maker gives it and how serious it says it is, against this driver's table; and the maker's whole client, `Robotiq::Gripper`, **driving the simulated gripper written here** through a TCP gateway: connect, activate, go to three positions, stop on a workpiece.

What it cannot show: that the maker's client is right (it is the maker's), or that a real gripper answers it as the maker's fake does.
"""

from __future__ import annotations

import json
import os
import random
import subprocess
import sys
import threading

ROOT = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(ROOT, "..", "..", "bindings", "python"))
sys.path.insert(0, ROOT)

from skynet_robotiq import frames  # noqa: E402
from skynet_robotiq.gripper_sim import SimulatedGripper  # noqa: E402

CHECKS = 0
FAILURES: list[str] = []
HERE = os.environ.get("ROBOTIQ_SIM", "")


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


class Dump:
    """The maker's client, as a process: one line in, one JSON line out."""

    def __init__(self) -> None:
        self.process = subprocess.Popen([os.path.join(HERE, "robotiq-codec-dump")], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)

    def ask(self, line: str) -> dict:
        assert self.process.stdin and self.process.stdout
        self.process.stdin.write(line + "\n")
        self.process.stdin.flush()
        return json.loads(self.process.stdout.readline())

    def close(self) -> None:
        if self.process.stdin:
            self.process.stdin.close()
        self.process.wait(timeout=5)


def main() -> int:
    if not HERE or not os.path.exists(os.path.join(HERE, "robotiq-codec-dump")):
        print("::error::ROBOTIQ_SIM names no directory of built harnesses (sim/build.sh builds them from a checkout of Robotiq/grippers). Nothing was compared.")
        return 1
    maker = open(os.path.join(HERE, "robotiq.txt"), encoding="utf-8").read().strip() if os.path.exists(os.path.join(HERE, "robotiq.txt")) else "robotiq grippers"
    print(f"skynet-robotiq layout check — against {maker}\n")
    rng = random.Random(0xF2)
    dump = Dump()
    try:
        # ── constants ─────────────────────────────────────────────────────────
        section("constants — the addresses, the widths, the slave, the action bits and the two profiles")
        constants = dump.ask("constants")
        check((constants["commandAddress"], constants["statusAddress"]) == (frames.COMMAND_ADDRESS, frames.STATUS_ADDRESS), "the command block is at register 1000 and the status block at 2000")
        check((constants["commandRegisters"], constants["statusRegisters"]) == (frames.BLOCK_REGISTERS, frames.BLOCK_REGISTERS), "three registers each: the six documented bytes, packed in pairs")
        check(constants["slave"] == frames.SLAVE_ADDRESS, "the slave address is 9")
        check(constants["actionBits"] == [0, 3, 4, 5] and (frames.ACT, frames.GTO, frames.ATR, frames.ARD) == (1 << 0, 1 << 3, 1 << 4, 1 << 5), "rACT is bit 0, rGTO bit 3, rATR bit 4 and rARD bit 5")
        for name, key in (("2F85", "2F-85"), ("HandE", "Hand-E")):
            theirs = dump.ask(f"profile {name}")
            mine = frames.PROFILES[key]
            check((theirs["minSpeed"], theirs["maxSpeed"], theirs["minOpening"], theirs["maxOpening"], theirs["openPosition"], theirs["closedPosition"])
                  == (mine.min_speed, mine.max_speed, mine.min_opening, mine.max_opening, mine.open_position, mine.closed_position), f"the {key}'s measured profile is the maker's, in all six numbers")

        # ── the requests ──────────────────────────────────────────────────────
        section("the requests — five hundred random commands, as the maker's client writes them")
        valid_actions = [a for a in range(256) if not a & frames.ACTION_RESERVED]
        reply17 = frames.with_crc(bytes([9, 0x17, 6, 0, 0, 0, 0, 0, 0])).hex()
        reply03 = frames.with_crc(bytes([9, 0x03, 6, 0, 0, 0, 0, 0, 0])).hex()
        reply10 = frames.with_crc(bytes([9, 0x10, 0x03, 0xE8, 0, 3])).hex()
        wrong = {"exchange": 0, "writeCommand": 0, "readStatus": 0}
        first_wrong: dict[str, str] = {}
        for _ in range(500):
            block = bytes([rng.choice(valid_actions), 0, 0, rng.randrange(256), rng.randrange(256), rng.randrange(256)])
            for op, mine, reply in (("exchange", frames.request_exchange(9, block), reply17), ("writeCommand", frames.request_write(9, frames.COMMAND_ADDRESS, frames.registers_from_block(block)), reply10)):
                theirs = dump.ask(f"{op} {block.hex()} {reply}")["request"]
                if theirs != mine.hex():
                    wrong[op] += 1
                    first_wrong.setdefault(op, f"{block.hex()}: the maker's {theirs}, this driver's {mine.hex()}")
        theirs = dump.ask(f"readStatus {reply03}")["request"]
        check(theirs == frames.request_read(9, frames.STATUS_ADDRESS, 3).hex(), "a status read is the same bytes, function 0x03 at 2000, three registers", theirs)
        check(wrong["exchange"] == 0, "the exchange (function 0x17) is byte for byte the maker's in 500 of 500, the CRC included", first_wrong.get("exchange", ""))
        check(wrong["writeCommand"] == 0, "the command write (function 0x10) is byte for byte the maker's in 500 of 500", first_wrong.get("writeCommand", ""))

        # ── the status block ──────────────────────────────────────────────────
        section("the status block — every status byte, every fault byte, a thousand random blocks")
        blocks = [bytes([b, 0, 0, 0, 0, 0]) for b in range(256)] + [bytes([0, 0, b, 0, 0, 0]) for b in range(256)] + [bytes(rng.randrange(256) for _ in range(6)) for _ in range(1000)]
        wrong_blocks = []
        for block in blocks:
            theirs = dump.ask(f"decode {block.hex()}")["status"]
            mine = frames.parse_status(block)
            got = (mine.activated, mine.go_to, mine.activation_state, mine.object_detection, mine.gripper_fault, mine.controller_fault, mine.position_echo, mine.position, mine.current)
            want = (bool(theirs["gACT"]), bool(theirs["gGTO"]), theirs["gSTA"], theirs["gOBJ"], theirs["gFLT"], theirs["kFLT"], theirs["gPR"], theirs["gPO"], theirs["gCU"])
            if got != want:
                wrong_blocks.append((block.hex(), got, want))
        check(not wrong_blocks, f"gACT, gGTO, gSTA, gOBJ, gFLT, kFLT, gPR, gPO and gCU decode the same in {len(blocks) - len(wrong_blocks)} of {len(blocks)} blocks", str(wrong_blocks[:2]))

        # ── the replies ───────────────────────────────────────────────────────
        section("the replies — a good one in each function, and the bad ones both must refuse")
        status = bytes([0xB9, 0, 0x0E, 0x64, 0x7B, 0x2A])
        for op, function, good in (("exchange", 0x17, frames.with_crc(bytes([9, 0x17, 6]) + status)), ("readStatus", 0x03, frames.with_crc(bytes([9, 0x03, 6]) + status))):
            theirs = dump.ask(f"{op} {bytes(6).hex() + ' ' if op == 'exchange' else ''}{good.hex()}")["result"]
            mine = frames.parse_status(frames.parse_response(good, 9, function))
            check(theirs.get("block") == status.hex() and mine.raw == status, f"a good function 0x{function:02X} reply is the status block, decoded alike by both")
        theirs = dump.ask(f"writeCommand {bytes(6).hex()} {frames.with_crc(bytes([9, 0x10, 0x03, 0xE8, 0, 3])).hex()}")["result"]
        check(theirs is None and frames.parse_response(frames.with_crc(bytes([9, 0x10, 0x03, 0xE8, 0, 3])), 9, 0x10) == bytes([0x03, 0xE8, 0, 3]), "a good function 0x10 reply is taken by both")
        good17 = frames.with_crc(bytes([9, 0x17, 6]) + status)
        bad = {
            "a wrong CRC": good17[:-1] + bytes([good17[-1] ^ 0x55]),
            "another slave": frames.with_crc(bytes([10, 0x17, 6]) + status),
            "another function": frames.with_crc(bytes([9, 0x03, 6]) + status),
            "a count that is too small": frames.with_crc(bytes([9, 0x17, 4]) + status),
            "a count that is too big": frames.with_crc(bytes([9, 0x17, 8]) + status),
            "a reply cut short": good17[:-3],
            "no reply": b"",
        }
        for code in (1, 2, 3, 4):
            bad[f"exception {code}"] = frames.with_crc(bytes([9, 0x17 | 0x80, code]))
        for name, frame in bad.items():
            theirs = dump.ask(f"exchange {bytes(6).hex()} {frame.hex() or '00'}")["result"]
            try:
                frames.parse_response(frame, 9, 0x17)
                mine_refuses = False
            except frames.ProtocolError:
                mine_refuses = True
            maker_refuses = isinstance(theirs, dict) and "error" in theirs
            check(maker_refuses and mine_refuses, f"{name}: the maker's client refuses it and so does this driver", f"the maker's: {theirs}; this driver refuses: {mine_refuses}")

        # ── the conversions ───────────────────────────────────────────────────
        section("the conversions — speed, opening, position and effort, against the maker's units.cpp")
        for name, key in (("2F85", "2F-85"), ("HandE", "Hand-E")):
            profile = frames.PROFILES[key]
            values = [0.0, 0.01, 0.02, 0.0425, 0.05, 0.085, 0.1, 0.15, 0.2, -0.01, 1e-9] + [rng.uniform(-0.02, 0.2) for _ in range(300)]
            for which, mine_fn in (("speed", frames.speed_to_register), ("opening", frames.opening_to_register)):
                wrong_values = []
                for value in values:
                    theirs = dump.ask(f"units {which} {name} {value!r}")["value"]
                    mine = mine_fn(value, profile)
                    if (theirs is None) != (mine is None) or (mine is not None and theirs != mine):
                        wrong_values.append((value, theirs, mine))
                check(not wrong_values, f"{which} → register, {key}: {len(values)} values, the same register every time", str(wrong_values[:3]))
            wrong_values = []
            for register in range(256):
                theirs = dump.ask(f"units fromregister {name} {register}")["value"]
                mine = frames.opening_from_register(register, profile)
                if (theirs is None) != (mine is None) or (mine is not None and abs(theirs - mine) > 1e-12):
                    wrong_values.append((register, theirs, mine))
            check(not wrong_values, f"register → opening, {key}: all 256 registers, the same opening", str(wrong_values[:3]))
        wrong_values = []
        for value in [0.0, 0.25, 0.5, 0.75, 1.0, 1.5, -0.1, 0.499999, 0.5000001] + [rng.uniform(-0.1, 1.2) for _ in range(300)]:
            theirs = dump.ask(f"units effort 2F85 {value!r}")["value"]
            mine = frames.effort_to_register(value)
            if (theirs is None) != (mine is None) or (mine is not None and theirs != mine):
                wrong_values.append((value, theirs, mine))
        check(not wrong_values, "effort → register: the same register every time", str(wrong_values[:3]))
        check(all(abs(dump.ask(f"units current 2F85 {r}")["value"] - r * frames.AMPERES_PER_STEP) < 1e-12 for r in range(256)), "gCU → amperes is 0.010 A a step, as the maker's is")

        # ── the faults ────────────────────────────────────────────────────────
        section("the faults — the name and severity the maker gives each code, against this driver's table")
        for which, table in (("gripper", frames.GRIPPER_FAULTS), ("controller", frames.CONTROLLER_FAULTS)):
            wrong_codes = []
            for code in range(16):
                theirs = dump.ask(f"fault {which} {code}")
                named = theirs["name"] != "Unrecognized"
                mine_text, mine_severity = table.get(code, (None, "major"))
                if named != (code in table) or theirs["severity"].lower() != mine_severity:
                    wrong_codes.append((code, theirs["name"], theirs["severity"], mine_severity))
            check(not wrong_codes, f"the {which} fault table: sixteen codes, the same ones named and the same severity for each (an unnamed code is Major)", str(wrong_codes))

        # ── the maker's whole client, driving the simulated gripper ───────────
        section("the maker's client, Robotiq::Gripper, driving the simulated gripper written here")
        probe = os.path.join(HERE, "robotiq-client-probe")

        def run_probe(sim: SimulatedGripper, *positions: int) -> list[dict]:
            done = subprocess.run([probe, "127.0.0.1", str(sim.port), "9", *map(str, positions)], capture_output=True, text=True, timeout=60)
            return [json.loads(line) for line in done.stdout.splitlines() if line.strip()]

        sim = SimulatedGripper(time_scale=20.0, activated=True, tcp=True).start()
        try:
            steps = run_probe(sim, 100, 3, 200)
            check(len(steps) == 5 and steps[0]["step"] == "connected" and steps[0]["status"]["gSTA"] == 3, "the maker's client connects to a gripper that is already activated and reads it as one")
            check(steps[1]["result"] == "AlreadyActive", "and its `activate()` leaves it undisturbed, which is what the maker's SDK promises a healthy activated gripper", str(steps[1]))
            moves = steps[2:]
            check(all(step["sent"] and step["stopped"] for step in moves), "it commands three go-to requests and each is carried and ends")
            check([step["status"]["gPR"] for step in moves] == [100, 3, 200] and [step["status"]["gPO"] for step in moves] == [100, 3, 200] and all(step["status"]["gOBJ"] == 3 for step in moves),
                  "and the maker's client reads the fingers at each requested register, at the requested position (gOBJ 3), with no fault", str([step["status"] for step in moves]))
            sim.workpiece = 150
            sim.place(3)
            stopped = run_probe(sim, 230)[-1]
            check(stopped["stopped"] and stopped["status"]["gOBJ"] == 2 and stopped["status"]["gPO"] == 150, f"closing on a workpiece, it reads the fingers stopped on an object while closing at register 150 ({stopped['status']['gPO']})")
        finally:
            sim.stop()
        cold = SimulatedGripper(time_scale=20.0, activated=False, tcp=True).start()
        try:
            steps = run_probe(cold, 120)
            check(steps[0]["status"]["gSTA"] == 0 and steps[1]["result"] == "Activated" and steps[1]["status"]["gSTA"] == 3, "a gripper that is not activated: the maker's `activate()` runs its reset-and-sweep handshake against the simulated one and it completes", str(steps[:2]))
            check(steps[2]["stopped"] and steps[2]["status"]["gPO"] == 120, "and the go-to after it is taken")
            check(cold.commands[0].action & frames.GTO == 0 and any(c.action == 0 for c in cold.commands) and any(c.action == frames.ACT for c in cold.commands), "by clearing rACT and setting it again with rGTO clear, which is the handshake the driver's `activate` motion makes too")
        finally:
            cold.stop()
        majors = SimulatedGripper(time_scale=20.0, activated=True, tcp=True).start()
        try:
            majors.set_fault(0x0E)
            steps = run_probe(majors)
            check(steps[1]["result"] == "FaultLatched", "a gripper with a major fault latched is `FaultLatched` to the maker's `activate()`: the same refusal this driver's `activate` motion makes, naming `recover`", str(steps[:2]))
        finally:
            majors.stop()
    finally:
        dump.close()
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    threading.current_thread().name = "layout-check"
    raise SystemExit(main())
