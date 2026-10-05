"""The Robotiq gripper bridge against a simulated gripper written here (DRY-COVERAGE-PLAN.md item 12.E.8): the frames, what it refuses to open on, the process image and the exchange cycle, the state, the refusals,
the faults, the stop, the motions and the line.

    PYTHONPATH=../../bindings/python:. python3 selftest.py

Needs python3 on Linux (the simulated gripper is a pseudo-terminal) and nothing of the maker's. The simulated gripper agrees with whoever wrote it (F15); `layout_check.py` holds the driver's bytes to the maker's own
client and `live.py` runs it against the maker's own fake gripper, and what this shows is the driver's own logic.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import struct
import sys
import threading
import time

import skynet_rhal as rhal

from skynet_robotiq import frames, link
from skynet_robotiq.bridge import RobotiqBridge, TOKEN
from skynet_robotiq.frames import ACT, GTO, Command
from skynet_robotiq.gripper_sim import SimulatedGripper

CHECKS = 0
FAILURES: list[str] = []
SPEED = 0.1           # m/s
EFFORT = 0.5
SCALE = 20.0          # the simulated fingers move this much faster than their speed says
SPEED_REGISTER = frames.speed_to_register(SPEED, frames.PROFILES["2F-85"])
EFFORT_REGISTER = frames.effort_to_register(EFFORT)
OPEN_M, CLOSED_M = 0.085, 0.0


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")
        if detail:
            print(f"        what the driver said: {detail!r}")


def section(title: str) -> None:
    print(f"\n{title}")


def until(predicate, timeout: float = 5.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return True
        except Exception:  # noqa: BLE001 - a predicate that cannot be evaluated yet is not true yet
            pass
        time.sleep(0.005)
    return bool(predicate())


def new_sim(**kw) -> SimulatedGripper:
    kw.setdefault("time_scale", SCALE)
    kw.setdefault("activated", True)
    return SimulatedGripper(**kw).start()


def settings(sim: SimulatedGripper, **extra) -> dict:
    out = {"model": "2F-85", "stop_mode": "halt", "speed": SPEED, "effort": EFFORT, "simulator": True, "roles": {"jaw": "right_hand.gripper.grip"}, "rate_hz": 50, "timeout_ms": 80}
    out.update({"host": "127.0.0.1", "port": sim.port} if sim.port else {"device": sim.path})
    out.update(extra)
    return {k: v for k, v in out.items() if v is not None}


def config(sim: SimulatedGripper, **extra) -> str:
    return json.dumps(settings(sim, **extra))


def opened(sim: SimulatedGripper, **extra) -> RobotiqBridge:
    bridge = RobotiqBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    assert code == rhal.SKR_OK, said.getvalue()
    return bridge


def opens_with(sim: SimulatedGripper, **extra) -> tuple[int, str, RobotiqBridge]:
    bridge = RobotiqBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    return code, said.getvalue(), bridge


def refused(text: str) -> tuple[int, str]:
    bridge = RobotiqBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(text)
    bridge.close()
    return code, said.getvalue()


def state(bridge: RobotiqBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def cmd(value: float, mask: list[int] | None = None, mode: int = rhal.MODE_POSITION, count: int = 1) -> rhal.Command:
    return rhal.Command(stamp_ns=1, seq=1, mode=mode, values=[value] * count, mask=mask or [1] * count)


def write(bridge: RobotiqBridge, command: rhal.Command) -> tuple[int, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def ext(bridge: RobotiqBridge, text: str) -> tuple[int, object]:
    code, body = bridge.extension(TOKEN, text.encode())
    try:
        return code, json.loads(body)
    except json.JSONDecodeError:
        return code, body.decode()


def ready(bridge: RobotiqBridge) -> bool:
    return until(lambda: state(bridge)[0] == rhal.SKR_OK)


def register(opening: float, model: str = "2F-85") -> int:
    value = frames.opening_to_register(opening, frames.PROFILES[model])
    assert value is not None
    return value


def settled(sim: SimulatedGripper, seconds: float = 3.0) -> bool:
    return until(lambda: sim._moving_to is None and not sim._stopping, seconds)


def exchanges_after(sim: SimulatedGripper, count: int, timeout: float = 3.0) -> bool:
    target = sim.exchanges + count
    return until(lambda: sim.exchanges >= target, timeout)


def main() -> int:
    print("skynet-robotiq selftest\n")

    # ── the frames ───────────────────────────────────────────────────────────
    section("frames — the maker's blocks, byte for byte")
    check(Command(ACT | GTO, 100, 150, 200).block() == bytes([0x09, 0, 0, 100, 150, 200]), "a command block is ACTION, two reserved bytes, rPR, rSP and rFR")
    check(frames.registers_from_block(bytes([0x09, 0, 0, 100, 150, 200])) == [0x0900, 0x0064, 0x96C8], "three registers, the first byte of each pair the high one (the maker's modbus_constants.hpp)")
    check(frames.block_from_registers([0x0900, 0x0064, 0x96C8]) == bytes([0x09, 0, 0, 100, 150, 200]), "and back")
    for name, bad in (("a reserved action bit", Command(0x02, 0, 0, 0)), ("a position of 256", Command(ACT, 256, 0, 0)), ("a negative speed", Command(ACT, 0, -1, 0)), ("a force that is a bool", Command(ACT, 0, 0, True)),
                      ("a position that is a float", Command(ACT, 1.5, 0, 0))):  # type: ignore[arg-type]
        try:
            bad.block()
            check(False, f"{name} is refused by the block")
        except frames.ProtocolError as error:
            check(True, f"{name} is refused by the block ({str(error)[:60]}...)")
    check(frames.crc16(bytes.fromhex("01030000000A")) == 0xCDC5, "the Modbus CRC-16 is the standard one (01 03 00 00 00 0A → C5 CD on the wire)")
    request = frames.request_exchange(9, bytes([0x09, 0, 0, 100, 150, 200]))
    check(request[:11].hex() == "091707d0000303e8000306", "a function 0x17 request reads three registers at 2000 and writes three at 1000, and says it is writing six bytes", request.hex())
    check(request[11:17] == bytes([0x09, 0x00, 0x00, 0x64, 0x96, 0xC8]) and len(request) == 19, "with the six bytes of the command block after the byte count", request.hex())
    check(frames.crc16(request[:-2]) == struct.unpack("<H", request[-2:])[0], "and the CRC after them, low byte first")
    check(frames.request_read(9, frames.STATUS_ADDRESS, 3)[:6].hex() == "090307d00003", "a status read is function 0x03, address 2000, three registers")
    check(frames.request_write(9, frames.COMMAND_ADDRESS, [0x0900, 0x0064, 0x96C8])[:11].hex() == "091003e80003060900" + "0064" and len(frames.request_write(9, frames.COMMAND_ADDRESS, [1, 2, 3])) == 15,
          "a command write is function 0x10, address 1000, three registers and six bytes")
    ok_answer = frames.with_crc(bytes([9, 3, 6, 0xB9, 0, 0x0E, 0x64, 0x7B, 0x2A]))
    check(frames.parse_response(ok_answer, 9, 3) == bytes([0xB9, 0, 0x0E, 0x64, 0x7B, 0x2A]), "an answer's data is the bytes after its count")
    for name, frame, function in (
            ("a wrong CRC", ok_answer[:-1] + bytes([ok_answer[-1] ^ 1]), 3),
            ("another slave", frames.with_crc(bytes([10, 3, 6, 0, 0, 0, 0, 0, 0])), 3),
            ("an exception", frames.with_crc(bytes([9, 0x83, 2])), 3),
            ("another function", frames.with_crc(bytes([9, 0x10, 0x03, 0xE8, 0, 3])), 3),
            ("a count that is not the length", frames.with_crc(bytes([9, 3, 4, 0, 0, 0, 0, 0, 0])), 3),
            ("a frame too short to be one", bytes([9, 3, 0]), 3),
            ("a reply to a write that is longer than the eight bytes the protocol gives it", frames.with_crc(bytes([9, 0x10, 0x03, 0xE8, 0, 3, 0])), 0x10)):
        try:
            frames.parse_response(frame, 9, function)
            check(False, f"{name} is refused")
        except frames.ProtocolError as error:
            check(True, f"{name} is refused: {str(error)[:70]}")
    try:
        frames.parse_response(frames.with_crc(bytes([9, 0x83, 2])), 9, 3)
    except frames.ProtocolError as error:
        check("exception 2" in str(error) and "illegal data address" in str(error), "and an exception says which one and what it means")
    for short in (bytes([9, 3, 0]), bytes([9, 3, 0, 0])):
        try:
            frames.parse_response(short, 9, 3)
            check(False, f"a frame of {len(short)} bytes is refused")
        except frames.ProtocolError as error:
            check("shorter than any Modbus RTU frame" in str(error), f"a frame of {len(short)} bytes is refused as too short to be a Modbus frame, and not as one with a wrong CRC", str(error))
    check(frames.response_length(bytes([9, 3])) is None and frames.response_length(bytes([9, 3, 6])) == 11 and frames.response_length(bytes([9, 0x83, 2])) == 5 and frames.response_length(bytes([9, 0x10, 3])) == 8,
          "the length of an answer is known from its first three bytes: a read's count, five for an exception, eight for a write")
    try:
        frames.response_length(bytes([9, 0x2B, 0]))
        check(False, "a function this gripper does not use cannot be framed")
    except frames.ProtocolError:
        check(True, "a function this gripper does not use cannot be framed")

    section("frames — the status block and the faults")
    status = frames.parse_status(bytes([0xB9, 0, 0x1E, 0x64, 0x7B, 0x2A]))
    check(status.activated and status.go_to and status.activation_state == 3 and status.object_detection == 2, "gACT is bit 0, gGTO bit 3, gSTA bits 4 and 5 and gOBJ bits 6 and 7")
    check(status.gripper_fault == 0x0E and status.controller_fault == 0x01 and status.fault_byte == 0x1E, "gFLT is the low nibble of the fault byte and kFLT the high one")
    check(status.position_echo == 100 and status.position == 123 and status.current == 42, "gPR, gPO and gCU are bytes 3, 4 and 5")
    check(not frames.parse_status(bytes(6)).activated and frames.parse_status(bytes(6)).activation_state == frames.STA_RESET, "a block of zeros is a gripper that is not activated")
    for table, name in ((frames.GRIPPER_FAULTS, "gripper"), (frames.CONTROLLER_FAULTS, "controller")):
        check(all(severity in ("none", "warning", "minor", "major") for _, severity in table.values()), f"every {name} fault the table names has a severity the maker's enumeration has")
    expected_gripper = {0x00: "ok", 0x05: "degraded", 0x07: "degraded", 0x08: "degraded", 0x09: "degraded", 0x0A: "fault", 0x0B: "fault", 0x0C: "fault", 0x0D: "fault", 0x0E: "fault", 0x0F: "fault"}
    for code in range(16):
        got = frames.fault_class(frames.parse_status(bytes([0, 0, code, 0, 0, 0])))
        want = expected_gripper.get(code, "fault")
        if code not in expected_gripper or code in (0x05, 0x0A, 0x0E):
            check(got == want, f"gripper fault 0x{code:02X} is {want}" + (" (a code the maker does not name is major, so a fault: never read as healthy)" if code not in expected_gripper else ""))
    expected_controller = {0x00: "ok", 0x04: "degraded", 0x05: "degraded", 0x09: "degraded", 0x0C: "estop", 0x0E: "fault"}
    for code in range(16):
        got = frames.fault_class(frames.parse_status(bytes([0, 0, code << 4, 0, 0, 0])))
        want = expected_controller.get(code, "fault")
        check(got == want, f"controller fault 0x{code:X} is {want}" + (" (unnamed: major)" if code not in expected_controller else ""))
    check(frames.fault_class(frames.parse_status(bytes([0, 0, 0xC8, 0, 0, 0]))) == "estop", "an emergency stop beside a minor gripper fault is the stop: the worse class wins")
    check(frames.fault_class(frames.parse_status(bytes([0, 0, 0x0E | 0x40, 0, 0, 0]))) == "fault", "and a major gripper fault beside a controller warning is a fault")

    section("frames — the maker's profiles and unit conversions")
    p85, phe = frames.PROFILES["2F-85"], frames.PROFILES["Hand-E"]
    check((p85.open_position, p85.closed_position, p85.max_opening) == (3, 230, 0.085) and (phe.open_position, phe.closed_position, phe.max_opening) == (3, 250, 0.050), "the two measured profiles are the maker's")
    check(frames.opening_to_register(0.085, p85) == 3 and frames.opening_to_register(0.0, p85) == 230 and frames.opening_to_register(0.0425, p85) == 117, "an opening is a register on the measured band: open 3, closed 230, half 117")
    check(frames.opening_to_register(0.2, p85) == 3 and frames.opening_to_register(-0.1, p85) == 230, "and an opening beyond either end is clamped to it, as the maker's is")
    check(frames.opening_to_register(math.nan, p85) is None and frames.opening_to_register(math.inf, p85) is None, "NaN and infinity have no register: the maker's conversion refuses what is not finite")
    check(frames.opening_from_register(3, p85) == 0.085 and frames.opening_from_register(230, p85) == 0.0 and frames.opening_from_register(255, p85) == 0.0 and frames.opening_from_register(0, p85) == 0.085,
          "a position register is an opening, clamped to the band at both ends")
    check(abs(frames.opening_from_register(117, p85) - 0.0425) < 1e-3, "and the middle of the band is the middle of the stroke")
    check(frames.speed_to_register(0.02, p85) == 0 and frames.speed_to_register(0.15, p85) == 255 and frames.speed_to_register(0.5, p85) == 255 and frames.speed_to_register(0.01, p85) == 0,
          "a speed is a register on the profile's range, floored and saturated")
    check(frames.speed_to_register(-0.01, p85) is None and frames.speed_to_register(math.nan, p85) is None, "a negative or non-finite speed has no register")
    check(frames.effort_to_register(0.0) == 0 and frames.effort_to_register(1.0) == 255 and frames.effort_to_register(0.5) == 128 and frames.effort_to_register(1.5) == 255 and frames.effort_to_register(-0.1) is None,
          "an effort of 0 to 1 is a register of 0 to 255, rounding half up, and saturates")

    # ── opening ──────────────────────────────────────────────────────────────
    sim = new_sim()
    try:
        section("open — what it refuses to bring up, and why")
        base = settings(sim)

        def refuses(what: str, expect: tuple[str, ...], **change) -> None:
            body = {**base, **change}
            for key in [k for k, v in change.items() if v is None]:
                body.pop(key, None)
            code, said = refused(json.dumps(body))
            check(code != rhal.SKR_OK and all(word in said for word in expect), what, said)

        code, said = refused("not json")
        check(code == rhal.SKR_EINVAL and "not JSON" in said, "a configuration that is not JSON is EINVAL", said)
        code, said = refused("[1, 2]")
        check(code == rhal.SKR_EINVAL and "not a JSON object" in said, "and one that is not an object", said)
        refuses("a key it does not read is refused, naming the keys it does", ("'frobnicate'", "keys it reads"), frobnicate=1)
        refuses("neither a device nor a host is refused: it does not scan for a gripper", ("neither", "does not scan"), device=None)
        refuses("both a device and a host is refused", ("both",), host="127.0.0.1", port=502)
        refuses("a baud rate the line cannot be set to is refused, saying it must match the gripper's", ("baud", "115200"), baud=12345)
        refuses("a slave address outside 1 to 247 is refused", ("slave", "1 to 247"), slave=0)
        refuses("True is not the slave address 1: refused as an address, not by the open failing on it", ("slave", "True", "is not a Modbus address"), slave=True)
        refuses("no model is refused: nothing the gripper says identifies it", ("model", "identifies"), model=None)
        refuses("a model that is not one of the maker's is refused, naming custom", ("2F-140", "custom"), model="2F-140")
        refuses("a profile with a model that has the maker's own is refused", ("profile", "custom"), profile={"min_speed": 0.02})
        refuses("custom without a profile is refused, naming the six keys", ("custom", "min_speed", "closed_position"), model="custom")
        good_profile = {"min_speed": 0.03, "max_speed": 0.25, "min_opening": 0.0, "max_opening": 0.14, "open_position": 5, "closed_position": 220}
        refuses("a custom profile with a missing key is refused", ("exactly",), model="custom", profile={k: v for k, v in good_profile.items() if k != "max_speed"})
        refuses("a custom profile whose open register is not below its closed one is refused", ("open_position",), model="custom", profile={**good_profile, "open_position": 230, "closed_position": 3})
        refuses("a custom profile with a speed range that is not one is refused", ("min_speed",), model="custom", profile={**good_profile, "min_speed": 0.3})
        refuses("a custom profile with an opening that is not a number is refused", ("not a number",), model="custom", profile={**good_profile, "max_opening": "wide"})
        refuses("no stop_mode is refused, saying what a stop is and that release is not offered", ("stop_mode", "rATR", "risk assessment"), stop_mode=None)
        refuses("a stop_mode that asks for release is refused with the same words", ("stop_mode", "rATR"), stop_mode="release")
        refuses("no simulator claim is refused: nothing says a real gripper from a simulation", ("simulator", "false claim"), simulator=None)
        refuses("a simulator that is not a bool is refused", ("simulator",), simulator="yes")
        refuses("no speed is refused, naming the range", ("speed", "0.02", "0.15"), speed=None)
        refuses("a speed below the profile's minimum is refused", ("speed",), speed=0.001)
        refuses("a speed above the profile's maximum is refused", ("speed",), speed=0.9)
        refuses("no effort is refused, saying how hard to hold is the operator's", ("effort", "how hard"), effort=None)
        refuses("an effort above one is refused", ("effort",), effort=1.5)
        refuses("limits wider than the profile are refused: a configuration narrows and never widens", ("limits", "never widen"), limits=[0.0, 0.2])
        refuses("limits that are not a range are refused", ("not a range",), limits=[0.05, 0.05])
        refuses("roles for a joint the body does not have are refused", ("roles", "jaw"), roles={"finger": "x"})
        refuses("a rate above the line's 250 Hz is refused", ("rate_hz",), rate_hz=1000)
        refuses("a transaction timeout below ten milliseconds is refused", ("timeout_ms",), timeout_ms=1)
        code, said = refused(json.dumps({**{k: v for k, v in base.items() if k != "device"}, "host": "127.0.0.1", "port": 0}))
        check(code != rhal.SKR_OK and "gateway" in said, "a gateway with no usable port is refused", said)

        # ── the line has nothing on it
        master, slave_fd = os.openpty()
        try:
            began = time.monotonic()
            dead_path = os.ttyname(slave_fd)
            code, said = refused(json.dumps({**{k: v for k, v in base.items() if k != "device"}, "device": dead_path, "open_timeout_ms": 300}))
            took = time.monotonic() - began
            check(code == rhal.SKR_EIO and "slave 9" in said and "115200" in said and took < 2.0, f"a line with nothing answering is EIO, naming the slave and the baud rate ({took:.2f} s)", said)
        finally:
            os.close(master)
            os.close(slave_fd)
        code, said = refused(json.dumps({**base, "device": "/dev/does-not-exist"}))
        check(code == rhal.SKR_EIO and "could not be" in said, "a device that does not exist is EIO", said)
        wrong = new_sim(slave=9)
        try:
            code, said = refused(json.dumps({**settings(wrong), "slave": 10, "open_timeout_ms": 300}))
            check(code == rhal.SKR_EIO and "slave 10" in said, "a gripper at another slave address does not answer, and the refusal says which address was asked", said)
        finally:
            wrong.stop()

        # ── a good open
        section("open and state — a gripper that is activated")
        bridge = opened(sim)
        check(ready(bridge), "state reads once the first exchange has")
        rc, out = state(bridge)
        check(rc == rhal.SKR_OK and abs(out.position_rad[0] - OPEN_M) < 1e-9, f"the fingers are at the open end, {OPEN_M} m, from the register the gripper reports", str(out.position_rad))
        check((out.flags & rhal.HOMING_MASK) == rhal.HOMING_COMPLETE and not out.flags & (rhal.FLAG_ESTOP | rhal.FLAG_FAULT | rhal.FLAG_DEGRADED), "an activated gripper is HOMING_COMPLETE with no flag", hex(out.flags))
        check(out.fault_code == 0 and math.isnan(out.velocity_rad_s[0]) and math.isnan(out.effort_nm[0]), "no fault word, and no velocity or effort: the gripper reports neither")
        before = out.seq
        state(bridge)
        check(state(bridge)[1].seq == before + 2, "the sequence number rises on each read")
        descriptor = json.loads(bridge.describe())
        joint = descriptor["joints"][0]
        check(descriptor["robot_slug"] == "robotiq-2f-85" and descriptor["dof"] == 1 and joint["name"] == "jaw" and joint["type"] == "prismatic", "the descriptor is one prismatic joint of the 2F-85")
        check(joint["limits"]["position_min_rad"] == 0.0 and joint["limits"]["position_max_rad"] == 0.085 and joint["limits"]["velocity_max_rad_s"] == SPEED and joint["control_modes"] == ["position"],
              "its limits are the profile's opening range, its speed the operator's, and it takes a position")
        check(descriptor["simulator"] is True and descriptor["verified"] is False and descriptor["envelope"] is None and descriptor["capabilities"] == ["safety.estop"] and descriptor["extensions"] == [TOKEN],
              "it says it is a simulation, that it is unverified, that it has no envelope of its own and a software stop")
        check([m["name"] for m in descriptor["motions"]] == ["status", "activate", "recover", "move", "grip", "release"], "and six motions")
        check(descriptor["end_effectors"][0]["kind"] == "gripper" and descriptor["end_effectors"][0]["max_grip_force_n"] is None, "a gripper end effector with no force in newtons: it takes a fraction of a range it cannot see")
        check(bridge.read_sensor("x", rhal.Frame()) == rhal.SKR_ENOSYS, "there is no sensor to read")
        bridge.close()
        check(state(bridge)[0] == rhal.SKR_EPERM and write(bridge, cmd(0.05))[0] == rhal.SKR_EPERM, "a closed bridge reads and writes EPERM")
        check(bridge.extension(TOKEN, b"status")[0] == rhal.SKR_EPERM, "and answers no motion")

        section("the exchange cycle — one image, sent every period, whatever has changed")
        count = sim.exchanges
        bridge = opened(sim, rate_hz=100)
        time.sleep(0.5)
        made = sim.exchanges - count
        check(made >= 35, f"with nothing to say, the gripper is exchanged with every period: {made} in half a second at 100 Hz")
        check(all(request[0] == frames.FC_READ_WRITE for request in sim.requests[-20:]), "each of them function 0x17: the command written and the status read in one transaction")
        check(all(c == sim.commands[-1] for c in sim.commands[-20:]), "and each carries the same image: nothing is sent only when it changes")
        check(sim.commands[-1] == Command(ACT, sim.commands[-1].position, SPEED_REGISTER, EFFORT_REGISTER), "the image has the operator's speed and force, rACT set from the gripper's own echo, and no go-to")
        bridge.close()
        quiet = sim.exchanges
        time.sleep(0.3)
        check(sim.exchanges == quiet, "after a close nothing more is written: the gripper keeps what it was last given, as the maker's own SDK leaves it")

        section("the image is seeded from the gripper's own echoes: connecting does not disturb one that is running")
        bridge = opened(sim)
        ready(bridge)
        check(write(bridge, cmd(0.03))[0] == rhal.SKR_OK, "a first session sends a go-to")
        target = register(0.03)
        bridge.close()
        mark = len(sim.commands)
        bridge = opened(sim)
        time.sleep(0.15)
        first = sim.commands[mark]
        check(first.action == ACT | GTO and first.position == target and first.speed == SPEED_REGISTER and first.force == EFFORT_REGISTER,
              "a second session's first exchange carries the go-to and the target the gripper echoed, with the operator's speed and force", repr(first))
        check(all(c.position == target and c.action == ACT | GTO for c in sim.commands[mark:]), "and nothing else is sent until it is asked")
        bridge.close()
        bridge = opened(sim)
        ready(bridge)
        bridge.emergency_stop()
        until(lambda: not sim.go_to)
        bridge.close()
        mark = len(sim.commands)
        bridge = opened(sim)
        time.sleep(0.1)
        check(sim.commands[mark].action == ACT and all(c.action == ACT for c in sim.commands[mark:]), "a gripper that was halted is not told to go anywhere on connecting")
        bridge.close()

        # ── commands
        section("write_command — the image changes, and the exchange that carries it is waited for")
        bridge = opened(sim)
        ready(bridge)
        began = time.monotonic()
        rc, said = write(bridge, cmd(0.04))
        took = time.monotonic() - began
        check(rc == rhal.SKR_OK and took < 0.2, f"a position command is carried and returns ({took * 1000:.1f} ms)", said)
        check(sim.commands[-1].action == ACT | GTO and sim.commands[-1].position == register(0.04) and sim.commands[-1].speed == SPEED_REGISTER and sim.commands[-1].force == EFFORT_REGISTER,
              "carried with rACT and rGTO, the target's register, the operator's speed and force", repr(sim.commands[-1]))
        check(until(lambda: abs(state(bridge)[1].position_rad[0] - 0.04) < 0.002), "and the fingers go there")
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED == 0), "with no flag once they have")
        mark = len(sim.commands)
        again = [write(bridge, cmd(0.04))[0] for _ in range(5)]
        check(again == [rhal.SKR_OK] * 5, "the same target again is accepted")
        check(all(c.position == register(0.04) for c in sim.commands[mark:]), "and changes nothing: the image already has it")
        check(write(bridge, cmd(OPEN_M))[0] == rhal.SKR_OK and sim.commands[-1].position == register(OPEN_M), "the open end is the open register")
        check(write(bridge, cmd(0.0))[0] == rhal.SKR_OK and sim.commands[-1].position == register(0.0), "the closed end is the closed register")
        sent = len(sim.commands)
        rc, said = write(bridge, cmd(0.2))
        check(rc == rhal.SKR_EINVAL and "outside its opening" in said and "nothing was sent" in said, "an opening outside the range is refused whole", said)
        rc, said = write(bridge, cmd(-0.001))
        check(rc == rhal.SKR_EINVAL, "below it too")
        rc, said = write(bridge, cmd(math.nan))
        check(rc == rhal.SKR_EINVAL and "not finite" in said, "NaN is refused", said)
        rc, said = write(bridge, cmd(math.inf))
        check(rc == rhal.SKR_EINVAL, "and infinity")
        rc, said = write(bridge, cmd(0.05, count=2))
        check(rc == rhal.SKR_EINVAL and "has 2 joints" in said, "two joints are refused: the body has one", said)
        rc, said = write(bridge, cmd(0.05, mode=rhal.MODE_VELOCITY))
        check(rc == rhal.SKR_EINVAL and "takes a position" in said, "a velocity command is refused, saying what it takes", said)
        rc, said = write(bridge, cmd(0.05, mode=rhal.MODE_TORQUE))
        check(rc == rhal.SKR_EINVAL, "and a torque command")
        rc, said = write(bridge, cmd(0.05, mask=[0]))
        check(rc == rhal.SKR_OK and said == "", "a masked joint is not commanded, and that is not an error")
        time.sleep(0.1)
        check(all(c.position == register(0.0) for c in sim.commands[sent:]), "and none of those reached the gripper: the image still says closed")
        said_by = [write(bridge, cmd(value))[1] for value in (0.2, 0.3, 0.4)]
        check(said_by[0] != "" and said_by[1] == "" and said_by[2] == "", "a refusal is said once for a run of the same cause, not at every period")
        bridge.close()

        section("write_command — narrowed limits, the Hand-E, and a profile of the operator's own")
        bridge = opened(sim, limits=[0.01, 0.07])
        ready(bridge)
        check(write(bridge, cmd(0.005))[0] == rhal.SKR_EINVAL and write(bridge, cmd(0.075))[0] == rhal.SKR_EINVAL and write(bridge, cmd(0.05))[0] == rhal.SKR_OK, "an opening outside the narrowed limits is refused and one inside is carried")
        check(json.loads(bridge.describe())["joints"][0]["limits"]["position_min_rad"] == 0.01, "and the descriptor says so")
        bridge.close()
        he = new_sim(profile="Hand-E")
        try:
            bridge = opened(he, model="Hand-E")
            ready(bridge)
            check(abs(state(bridge)[1].position_rad[0] - 0.05) < 1e-9, "a Hand-E opens at 0.05 m, the end of its own stroke")
            check(write(bridge, cmd(0.0))[0] == rhal.SKR_OK and he.commands[-1].position == 250 and json.loads(bridge.describe())["robot_slug"] == "robotiq-hand-e", "and closes to register 250")
            bridge.close()
            code, said, bridge = opens_with(he, model="custom", profile={"min_speed": 0.02, "max_speed": 0.15, "min_opening": 0.0, "max_opening": 0.05, "open_position": 3, "closed_position": 250}, speed=0.1)
            check(code == rhal.SKR_OK and json.loads(bridge.describe())["robot_slug"] == "robotiq-custom", "a custom profile opens, and the slug says it is one")
            ready(bridge)
            check(write(bridge, cmd(0.0))[0] == rhal.SKR_OK and he.commands[-1].position == 250, "and is converted by its own registers")
            bridge.close()
        finally:
            he.stop()

        # ── a gripper that is not activated
        section("a gripper that is not activated: it opens, and refuses to move until a person has said so")
        cold = new_sim(activated=False, time_scale=1.0)
        try:
            code, said, bridge = opens_with(cold)
            check(code == rhal.SKR_OK and "not activated" in said and "activate" in said and "calibration sweep" in said, "it opens, saying it is not activated and that activation is a sweep of the full range", said)
            ready(bridge)
            rc, out = state(bridge)
            check((out.flags & rhal.HOMING_MASK) == rhal.HOMING_PENDING and out.flags & rhal.FLAG_DEGRADED, "its position is not trustworthy: HOMING_PENDING and DEGRADED")
            time.sleep(0.1)
            check(len(cold.commands) > 2 and all(c.action & ACT == 0 for c in cold.commands), "and the driver sends no rACT to a gripper that says it is not activated: the exchange cycle would otherwise activate it, with a sweep nobody asked for")
            rc, said = write(bridge, cmd(0.04))
            check(rc == rhal.SKR_EPERM and "not activated" in said and "`activate`" in said, "a command is refused EPERM, naming the motion that activates", said)
            check(all(c.action & GTO == 0 for c in cold.commands), "and no go-to reached the gripper")
            rc, body = ext(bridge, "move position=0.04")
            check(rc == rhal.SKR_EPERM and "activate" in str(body), "a move is refused the same way", str(body))
            samples: list[float] = []
            homing: list[tuple[float, int]] = []
            holder: list[tuple[int, object]] = []
            runner = threading.Thread(target=lambda: holder.append(ext(bridge, "activate")))
            runner.start()
            while runner.is_alive():
                rc, out = state(bridge)
                samples.append(out.position_rad[0])
                homing.append((out.position_rad[0], out.flags & rhal.HOMING_MASK))
                time.sleep(0.01)
            runner.join()
            rc, body = holder[0]
            check(rc == rhal.SKR_OK and body["result"] == "activated", "the activate motion runs the handshake and says activated", str(body))
            check(min(samples) < 0.01 and any(a > b for a, b in zip(samples, samples[1:])) and any(a < b for a, b in zip(samples, samples[1:])),
                  f"the fingers closed through their range and opened again (closest {min(samples):.3f} m)")
            check(abs(state(bridge)[1].position_rad[0] - OPEN_M) < 0.005, "and finished at the open end")
            sweeping = [h for position, h in homing if 0.002 < position < 0.083]
            check(len(sweeping) > 5 and all(h == rhal.HOMING_PENDING for h in sweeping), f"and the position was HOMING_PENDING all the while the fingers were between their ends ({len(sweeping)} samples): gACT is up from the first moment and gSTA says when it is done")
            check(cold.commands[0].action == 0 or any(c.action == 0 for c in cold.commands), "by clearing rACT first, as the maker's handshake does (a reset)")
            check(any(c.action == ACT for c in cold.commands) and all(c.action & GTO == 0 for c in cold.commands), "and then setting it, with rGTO clear throughout")
            rc, out = state(bridge)
            check((out.flags & rhal.HOMING_MASK) == rhal.HOMING_COMPLETE and not out.flags & rhal.FLAG_DEGRADED, "afterwards it is HOMING_COMPLETE with no flag")
            rc, body = ext(bridge, "activate")
            check(rc == rhal.SKR_OK and body["result"] == "already_active", "activating an activated gripper leaves it undisturbed")
            check(write(bridge, cmd(0.04))[0] == rhal.SKR_OK, "and a command is carried now")
            bridge.close()
        finally:
            cold.stop()

        section("faults — what a fault is for the robot, and what the driver does with a command")
        bridge = opened(sim)
        ready(bridge)

        def with_fault(gripper: int = 0, controller: int = 0) -> tuple[int, rhal.State]:
            sim.set_fault(gripper, controller)
            exchanges_after(sim, 3)
            return state(bridge)

        rc, out = with_fault(0x05)
        check(rc == rhal.SKR_OK and out.flags & rhal.FLAG_DEGRADED and not out.flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP) and out.fault_code == 0x05, "a warning (action delayed) is DEGRADED and nothing worse, and the word is the fault byte")
        check(write(bridge, cmd(0.06))[0] == rhal.SKR_OK, "and commands are still accepted")
        rc, out = with_fault(0x08)
        check(out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_FAULT and out.fault_code == 0x08, "a minor fault (over-temperature) is DEGRADED: the maker says it clears on its own")
        check(write(bridge, cmd(0.05))[0] == rhal.SKR_OK, "and commands are still accepted")
        for code, name in ((0x0A, "under-voltage"), (0x0E, "overcurrent"), (0x0C, "an internal fault"), (0x03, "a code the maker does not name")):
            rc, out = with_fault(code)
            check(out.flags & rhal.FLAG_FAULT and out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_ESTOP and out.fault_code == code, f"a major fault ({name}, 0x{code:02X}) is FAULT with DEGRADED, and the word is its code")
            rc, said = write(bridge, cmd(0.04))
            check(rc == rhal.SKR_EPERM and f"0x{code:02X}" in said and "recover" in said, "and a command is refused EPERM, naming the code and the motion that resets it", said)
        rc, out = with_fault(0, 0x04)
        check(out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_FAULT and out.fault_code == 0x40, "a controller warning (24 V not detected) is DEGRADED, and the word carries the controller's nibble")
        rc, out = with_fault(0, 0x0C)
        check(out.flags & rhal.FLAG_ESTOP and out.flags & rhal.FLAG_DEGRADED and out.fault_code == 0xC0, "the controller's emergency stop is ESTOP with DEGRADED")
        check(write(bridge, cmd(0.04))[0] == rhal.SKR_EPERM, "and a command is refused")
        rc, out = with_fault(0, 0x0E)
        check(out.flags & rhal.FLAG_FAULT and not out.flags & rhal.FLAG_ESTOP, "a controller overcurrent is a fault, not a stop")
        rc, out = with_fault(0, 0x01)
        check(out.flags & rhal.FLAG_FAULT, "and a controller code the maker does not name is a fault: an unknown state of a safety system is not healthy")
        rc, out = with_fault(0x0E, 0x0C)
        check(out.flags & rhal.FLAG_ESTOP and out.flags & rhal.FLAG_DEGRADED and out.fault_code == 0xCE, "an emergency stop beside an overcurrent is the stop in the flags (the worse class wins), and the word still names both")
        rc, out = with_fault(0, 0)
        check(rc == rhal.SKR_OK and not out.flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP | rhal.FLAG_DEGRADED) and out.fault_code == 0, "and a gripper that no longer reports a fault reads healthy again")
        check(write(bridge, cmd(0.04))[0] == rhal.SKR_OK, "and takes a command")

        section("faults — a major fault is reset by a handshake that moves the fingers, and activate does not pretend otherwise")
        sim.set_fault(0x0E)
        exchanges_after(sim, 3)
        rc, body = ext(bridge, "activate")
        check(rc == rhal.SKR_EPERM and "recover" in str(body) and "0x0E" in str(body), "activate on a major fault is refused, naming recover (the maker's `activate` returns FaultLatched)", str(body))
        mark = len(sim.commands)
        rc, body = ext(bridge, "recover timeout=10")
        check(rc == rhal.SKR_OK and body["result"] == "activated", "recover runs the handshake: clear rACT, wait for the reset, set it, wait for gSTA", str(body))
        check(any(c.action == 0 for c in sim.commands[mark:]) and any(c.action == ACT for c in sim.commands[mark:]) and all(c.action & GTO == 0 for c in sim.commands[mark:]), "a falling edge on rACT and then a rising one, with rGTO clear")
        rc, out = state(bridge)
        check(rc == rhal.SKR_OK and out.fault_code == 0 and not out.flags & (rhal.FLAG_FAULT | rhal.FLAG_DEGRADED), "and the fault is gone")
        check(write(bridge, cmd(0.04))[0] == rhal.SKR_OK, "and the gripper takes a command")
        bridge.close()
        bridge = opened(sim)
        ready(bridge)
        sim.set_fault(0x0A)
        exchanges_after(sim, 3)
        bridge.close()
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_OK and "under the minimum operating voltage" in said and "recover" in said, "a gripper that opens already in a major fault says so, and what clears it", said)
        check(ext(bridge, "recover timeout=10")[0] == rhal.SKR_OK and ready(bridge) and state(bridge)[1].fault_code == 0, "and recovers")
        bridge.close()

        section("the stop — a latch and the go-to taken away, in one step")
        bridge = opened(sim)
        ready(bridge)
        write(bridge, cmd(0.02))
        began = time.monotonic()
        rc = bridge.emergency_stop()
        took = time.monotonic() - began
        check(rc == rhal.SKR_OK and took < 0.01, f"emergency_stop returns at once ({took * 1000:.2f} ms)")
        check(state(bridge)[1].flags & rhal.FLAG_ESTOP, "and the latch is in the very next state")
        check(until(lambda: sim.commands[-1].action & GTO == 0 and sim.commands[-1].action & ACT), "the image has rGTO clear and rACT still set: the go-to is taken away and the gripper stays activated")
        check(until(lambda: not sim.go_to), "and the gripper shows gGTO clear: it took the stop")
        rc, said = write(bridge, cmd(0.05))
        check(rc == rhal.SKR_EPERM, "a command after a stop is refused EPERM")
        exchanges_after(sim, 6)
        check(all(c.action & GTO == 0 for c in sim.commands[-5:]), "and the image goes on saying halt in every exchange: a lost frame is sent again a period later")
        rc, body = ext(bridge, "move position=0.04")
        check(rc == rhal.SKR_EPERM and "latched" in str(body), "a motion is refused while latched", str(body))
        check(ext(bridge, "status")[0] == rhal.SKR_OK and ext(bridge, "status")[1]["stop_latched"] is True, "and status is still answered, and says it is latched")
        rc, body = ext(bridge, "activate")
        check(rc == rhal.SKR_EPERM, "activate is refused while latched: a stop stops a calibration sweep too")
        check(bridge.release_stop() == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_ESTOP, "release_stop clears the latch once the gripper has shown it took the halt")
        check(sim.commands[-1].action & GTO == 0, "and the image still has no go-to: nothing moves until a command asks")
        check(write(bridge, cmd(0.05))[0] == rhal.SKR_OK and until(lambda: sim.go_to), "a command is carried after a release")
        bridge.close()

        section("the stop — a command cannot put the go-to back after it")
        bridge = opened(sim, rate_hz=100)
        ready(bridge)
        stop_ns = [0.0]
        quit_event = threading.Event()
        rcs: list[int] = []

        def hammer(value: float) -> None:
            while not quit_event.is_set():
                rcs.append(write(bridge, cmd(value))[0])
                value = 0.08 - value if value < 0.04 else 0.01
                time.sleep(0.0005)

        threads = [threading.Thread(target=hammer, args=(0.01 + 0.01 * i,)) for i in range(4)]
        for thread in threads:
            thread.start()
        time.sleep(0.15)
        bridge.emergency_stop()
        stop_ns[0] = time.monotonic()
        time.sleep(0.4)
        quit_event.set()
        for thread in threads:
            thread.join()
        late = [c for c, when in zip(sim.commands, sim.command_times) if when > stop_ns[0] + 0.05 and c.action & GTO]
        check(not late, f"four threads hammering write_command while the stop lands: after it, not one exchange carries a go-to ({len(late)} did)")
        check(rhal.SKR_OK in rcs and rhal.SKR_EPERM in rcs and set(rcs) <= {rhal.SKR_OK, rhal.SKR_EPERM, rhal.SKR_EIO}, "every write answered OK before the stop and EPERM after it (or EIO for one caught between)")
        check(bridge.release_stop() == rhal.SKR_OK, "and the stop releases")
        bridge.close()

        section("the stop — release_stop wants the gripper seen to take it")
        bridge = opened(sim, timeout_ms=30)
        ready(bridge)
        write(bridge, cmd(0.02))
        sim.drop = True
        bridge.emergency_stop()
        time.sleep(0.2)
        began = time.monotonic()
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            rc = bridge.release_stop()
        took = time.monotonic() - began
        check(rc == rhal.SKR_EIO and "stays latched" in said.getvalue() and state(bridge)[1].flags & rhal.FLAG_ESTOP and took < 3.0,
              f"with the line dead, release_stop is EIO and the latch stays ({took:.1f} s)", said.getvalue())
        sim.drop = False
        check(until(lambda: not sim.go_to, 2.0), "and when the line returns the stop that was waiting is delivered at once: the image said halt all along")
        check(bridge.release_stop() == rhal.SKR_OK, "after which it releases")
        bridge.close()

        section("a stop and a close")
        bridge = opened(sim)
        ready(bridge)
        bridge.emergency_stop()
        bridge.close()
        bridge = opened(sim)
        ready(bridge)
        check(not state(bridge)[1].flags & rhal.FLAG_ESTOP and write(bridge, cmd(0.05))[0] == rhal.SKR_OK, "a new session starts with no latch: the first session's stop does not outlive it")
        bridge.close()

        # ── motions
        section("motions — move, grip and release, judged on the exchange that shows them")
        bridge = opened(sim)
        ready(bridge)
        rc, body = ext(bridge, "status")
        check(rc == rhal.SKR_OK and body["model"] == "2F-85" and body["activated"] is True and body["fault_class"] == "ok" and body["link"] == sim.path and body["limits_m"] == [0.0, 0.085], "status says what the gripper and the driver know", str(body))
        check(body["image"]["speed"] == f"{SPEED_REGISTER}" or body["image"]["speed"] == SPEED_REGISTER, "including the image it is sending", str(body["image"]))
        rc, body = ext(bridge, "move position=0.03")
        check(rc == rhal.SKR_OK and body["object"] == "at the requested position" and abs(body["opening_m"] - 0.03) < 0.002, "a move waits for the fingers to be at the requested position", str(body))
        sim.workpiece = register(0.02)
        rc, body = ext(bridge, "grip effort=0.4")
        check(rc == rhal.SKR_OK and body["result"] == "gripped" and abs(body["opening_m"] - 0.02) < 0.002, "a grip on a workpiece says gripped, at the workpiece's opening", str(body))
        check(sim.commands[-1].force == frames.effort_to_register(0.4) and sim.commands[-1].position == 230, "closing at the effort asked, to the closed register")
        rc, out = state(bridge)
        check(rc == rhal.SKR_OK and abs(out.position_rad[0] - 0.02) < 0.002 and not out.flags & rhal.FLAG_DEGRADED, "and the state says the fingers are there")
        rc, body = ext(bridge, "release")
        check(rc == rhal.SKR_OK and body["result"] == "released" and body["opening_m"] > 0.08, "a release opens the fingers: they open from the workpiece, which does not stop them", str(body))
        sim.workpiece = None
        rc, body = ext(bridge, "release")
        check(rc == rhal.SKR_OK and body["result"] == "released" and abs(body["opening_m"] - OPEN_M) < 0.002, "with nothing between them, all the way", str(body))
        rc, body = ext(bridge, "grip effort=0.3")
        check(rc == rhal.SKR_OK and body["result"] == "closed_on_nothing" and body["opening_m"] < 0.002, "a grip on nothing closes to the end and says so: it is not the same as gripped", str(body))
        ext(bridge, "release")
        sim.workpiece = register(0.04)
        rc, body = ext(bridge, "move position=0.0 speed=0.15 effort=1")
        check(rc == rhal.SKR_OK and body["object"] == "an object detected while closing", "a move that meets something says an object was detected", str(body))
        check(sim.commands[-1].speed == 255 and sim.commands[-1].force == 255, "at the speed and effort asked")
        sim.workpiece = None
        for text, word in (("move", "required"), ("move position=abc", "not a number"), ("move position=0.5", "outside"), ("move position=0.04 speed=1", "outside"), ("move position=0.04 effort=2", "outside"), ("move position=0.04 colour=red", "does not take"),
                           ("grip", "required"), ("grip effort=1.5", "outside"), ("release speed=9", "outside"), ("status extra=1", "does not take"), ("activate timeout=0", "outside"), ("frobnicate", "unknown motion"), ("move position", "malformed"),
                           ("", "empty")):
            rc, body = ext(bridge, text)
            check(rc == rhal.SKR_EINVAL and word in str(body), f"`{text}` is EINVAL, saying why", str(body))
        check(bridge.extension("vendor.other.task", b"status")[0] == rhal.SKR_ENOSYS, "another vendor's token is ENOSYS")
        check(bridge.extension(TOKEN, b"\xff\xfe")[0] == rhal.SKR_EINVAL, "and a request that is not text is EINVAL")
        bridge.close()

        section("motions — a stop ends the motion that is running")
        slow = new_sim(time_scale=1.0)
        try:
            bridge = opened(slow, speed=0.02)
            ready(bridge)
            holder = []
            runner = threading.Thread(target=lambda: holder.append(ext(bridge, "move position=0.0")))
            runner.start()
            time.sleep(0.5)
            at_stop = slow.position
            bridge.emergency_stop()
            runner.join(timeout=3.0)
            check(not runner.is_alive() and holder and holder[0][0] == rhal.SKR_EPERM and "latched" in str(holder[0][1]), "a move in progress ends EPERM the moment the stop is latched", str(holder))
            time.sleep(0.4)
            check(abs(slow.position - at_stop) <= 12 and 3 < slow.position < 230, f"and the fingers do not go on to the target (register {at_stop} at the stop, {slow.position} later; closed is 230)")
            bridge.close()
        finally:
            slow.stop()

        # ── the line
        section("the line — a gripper that stops answering, answers late, answers badly or answers as someone else")
        bridge = opened(sim, timeout_ms=30, stale_ms=150)
        ready(bridge)
        write(bridge, cmd(0.05))
        sim.drop = True
        began = time.monotonic()
        rc, said = write(bridge, cmd(0.02))
        took = time.monotonic() - began
        check(rc == rhal.SKR_EIO and 0.15 < took < 0.35 and "did not carry" in said and "put back" in said, f"a command the line did not carry is EIO within the quarter of a second a command is given ({took * 1000:.0f} ms), and the image is put back", said)
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT and state(bridge)[1].flags & rhal.FLAG_DEGRADED, 1.0), "state is ETIMEDOUT and DEGRADED once the status is stale and the line has failed three exchanges")
        check(not math.isnan(state(bridge)[1].position_rad[0]), "with the last position it had, not NaN")
        mark = len(sim.commands)
        sim.drop = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 2.0), "the line coming back is read at once")
        time.sleep(0.15)
        check(all(c.position == register(0.05) for c in sim.commands[mark:]), "and the command that failed was not written late: the gripper is sent the image that was put back")
        for lever, value, what in (("garble", True, "a CRC that is wrong"), ("exception_code", 4, "an exception answer"), ("reply_slave", 7, "an answer from another slave"), ("truncate", 2, "an answer that is short")):
            setattr(sim, lever, value)
            check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 2.0), f"{what} is a failed exchange, and three of them make the state ETIMEDOUT")
            rc, said = write(bridge, cmd(0.03))
            check(rc == rhal.SKR_EIO, "and a command is EIO while it lasts")
            setattr(sim, lever, type(value)())
            if lever == "reply_slave":
                sim.reply_slave = None
            check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 2.0), "and the state recovers when it stops")
        sim.split_gap_s = 0.02
        check(exchanges_after(sim, 5) and state(bridge)[0] == rhal.SKR_OK, "an answer that comes in two pieces with a short gap (a slow USB adapter) is assembled")
        sim.split_gap_s = 0.12
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 2.0), "and one whose gap is longer than the byte timeout is a failed exchange: it stopped sending")
        sim.split_gap_s = 0.0
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 2.0), "and recovers")
        sim.delay_s = 0.2
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 2.0), "an answer later than the transaction's budget is a failed exchange")
        sim.delay_s = 0.0
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3.0), "and recovers")
        bridge.close()

        section("the line — the same frames over a gateway on TCP")
        gate = new_sim(tcp=True)
        try:
            bridge = opened(gate)
            check(ready(bridge) and abs(state(bridge)[1].position_rad[0] - OPEN_M) < 1e-9, "a gripper behind an RS-485 gateway is read the same way")
            check(write(bridge, cmd(0.04))[0] == rhal.SKR_OK and until(lambda: abs(state(bridge)[1].position_rad[0] - 0.04) < 0.002), "and commanded")
            bridge.emergency_stop()
            check(until(lambda: not gate.go_to) and bridge.release_stop() == rhal.SKR_OK, "and stopped, and released")
            before = gate.connections
            gate.hang_up()
            check(until(lambda: gate.connections > before, 3.0) and until(lambda: state(bridge)[0] == rhal.SKR_OK, 3.0), "a gateway that drops the connection is connected to again, and the state is read again")
            check(write(bridge, cmd(0.03))[0] == rhal.SKR_OK and until(lambda: abs(state(bridge)[1].position_rad[0] - 0.03) < 0.002), "and the gripper is commanded as before")
            gate.refuse = True
            gate.hang_up()
            check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT and state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0), "a gateway that is up and takes no connection leaves the state ETIMEDOUT and DEGRADED")
            rc, said = write(bridge, cmd(0.05))
            check(rc == rhal.SKR_EIO, "and a command EIO")
            gate.refuse = False
            check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3.0), "and when it takes connections again the driver is back")

            section("the line comes back — the image is made again from what the gripper says it is, and a lost activation is not given back")
            write(bridge, cmd(0.03))
            until(lambda: abs(state(bridge)[1].position_rad[0] - 0.03) < 0.002)
            gate.refuse = True
            gate.hang_up()
            check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 3.0), "a gateway that is down")
            with gate._lock:        # the gripper loses its power while nobody is talking to it: it is not activated when it is back, and the fingers are wherever they were
                gate._sta, gate._action, gate._moving_to = frames.STA_RESET, 0, None
                gate._publish()
            mark = len(gate.commands)
            gate.refuse = False
            check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3.0), "and up again")
            time.sleep(0.2)
            check(len(gate.commands) > mark and all(c.action & ACT == 0 for c in gate.commands[mark:]), "the driver does not set rACT on a gripper that says it is not activated: it would run a calibration sweep nobody asked for", str(gate.commands[mark:mark + 3]))
            rc, out = state(bridge)
            check((out.flags & rhal.HOMING_MASK) == rhal.HOMING_PENDING and out.flags & rhal.FLAG_DEGRADED, "it reads HOMING_PENDING and DEGRADED: the activation is gone, and the driver says so")
            rc, said = write(bridge, cmd(0.05))
            check(rc == rhal.SKR_EPERM and "activate" in said, "and a command is refused, naming the motion that activates", said)
            check(ext(bridge, "activate timeout=10")[0] == rhal.SKR_OK and write(bridge, cmd(0.05))[0] == rhal.SKR_OK, "which a person runs, and then the gripper is commanded")
            write(bridge, cmd(0.02))
            until(lambda: gate.go_to and gate.commands[-1].position == register(0.02))
            gate.refuse = True
            gate.hang_up()
            until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 3.0)
            mark = len(gate.commands)
            gate.refuse = False
            until(lambda: state(bridge)[0] == rhal.SKR_OK, 3.0)
            time.sleep(0.15)
            check(len(gate.commands) > mark and all(c.action == ACT | GTO and c.position == register(0.02) for c in gate.commands[mark:]), "and a gripper that was on its way is told to go on to where it was going: the image is what it echoed")
            bridge.close()
            bridge = opened(sim, timeout_ms=30)
            ready(bridge)
            holder = []
            sim.drop = True
            runner = threading.Thread(target=lambda: holder.append(write(bridge, cmd(0.01))))
            began = time.monotonic()
            runner.start()
            time.sleep(0.05)
            with sim._lock:
                echoed = frames.parse_status(sim._status_block())
            bridge._reseed(echoed)
            runner.join(timeout=1.0)
            took = time.monotonic() - began
            sim.drop = False
            check(holder and holder[0][0] == rhal.SKR_EIO and "found again" in holder[0][1] and took < 0.15, f"a command that is waiting when the image is made again is EIO at once, and says why ({took * 1000:.0f} ms)", str(holder))
            bridge.close()
            code, said = refused(json.dumps({**{k: v for k, v in settings(gate).items() if k not in ("host", "port")}, "host": "127.0.0.1", "port": 1, "open_timeout_ms": 300}))
            check(code == rhal.SKR_EIO and "could not be opened" in said, "a gateway that is not listening is EIO, naming it", said)
        finally:
            gate.stop()

        section("a gripper whose position echo agrees and whose go-to echo does not")
        bridge = opened(sim, stale_ms=5000)
        ready(bridge)
        write(bridge, cmd(0.04))
        until(lambda: sim.go_to)
        bridge.emergency_stop()
        until(lambda: not sim.go_to)
        bridge.release_stop()
        sim.deaf = True
        write(bridge, cmd(0.04))
        time.sleep(0.7)
        check(state(bridge)[1].flags & rhal.FLAG_DEGRADED, "a gripper that echoes the position it was asked for and does not show the go-to is DEGRADED: the position alone does not say it took the command")
        sim.deaf = False
        sim._action = ACT
        bridge.close()

        section("a stop that lands while a command waits for a line that has gone")
        bridge = opened(sim, timeout_ms=30)
        ready(bridge)
        sim.drop = True
        outcome = []

        def waiting_write() -> None:
            result = write(bridge, cmd(0.01))
            outcome.append((time.monotonic(), result))

        worker = threading.Thread(target=waiting_write)
        worker.start()
        time.sleep(0.05)
        stopped_at = time.monotonic()
        bridge.emergency_stop()
        worker.join(2.0)
        sim.drop = False
        check(not worker.is_alive() and outcome and outcome[0][1][0] == rhal.SKR_EPERM and outcome[0][0] - stopped_at < 0.12,
              f"a command waiting for the line is ended by a stop at once, as latched ({(outcome[0][0] - stopped_at) * 1000 if outcome else float('nan'):.0f} ms after it, where the wait runs to 200)", str(outcome))
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        bridge.release_stop()
        bridge.close()

        section("a command the gripper acknowledges and does not take")
        sim.deaf = True
        bridge = opened(sim, stale_ms=5000)
        ready(bridge)
        write(bridge, cmd(0.02))
        check(state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, "straight after the command nothing is said: the echo is allowed a moment")
        time.sleep(0.7)
        check(state(bridge)[1].flags & rhal.FLAG_DEGRADED, "but a gripper whose gGTO and gPR have not become what was asked for half a second later is DEGRADED: the acknowledgement was on the wire and the command was not taken (F175)")
        sim.deaf = False
        bridge.close()
        sim.deaf = False
        sim._action = ACT

        section("a stop latched in one session does not outlive it, on the same driver")
        bridge = opened(sim)
        ready(bridge)
        bridge.emergency_stop()
        bridge.close()
        code = bridge.open(config(sim))
        check(code == rhal.SKR_OK and ready(bridge) and not state(bridge)[1].flags & rhal.FLAG_ESTOP and write(bridge, cmd(0.05))[0] == rhal.SKR_OK,
              "a driver opened again after a stop has no latch: the stop belonged to the session it was made in")
        bridge.close()

        section("sessions — closed, opened again, and two at once")
        bridge = opened(sim)
        ready(bridge)
        check(bridge.close() == rhal.SKR_OK and bridge.close() == rhal.SKR_OK, "close twice is not an error")
        code, said, again = opens_with(sim)
        check(code == rhal.SKR_OK and ready(again) and write(again, cmd(0.05))[0] == rhal.SKR_OK, "the same gripper opens again")
        code = again.open(config(sim))
        check(code == rhal.SKR_OK and ready(again), "and an open on an open driver is a new session, with the old one torn down")
        again.close()

        # ── what the seeded-defect run is going to ask, written ahead of it from reading the driver ─────────────
        section("a stop and a repeated command do not wait for the next period")
        bridge = opened(sim, rate_hz=1.0)
        ready(bridge)
        began = time.monotonic()
        rc, said = write(bridge, cmd(0.03))
        took = time.monotonic() - began
        check(rc == rhal.SKR_OK and took < 0.19, f"at one exchange a second a command is carried at once, and not at the next period ({took * 1000:.0f} ms)", said)
        began = time.monotonic()
        rc, said = write(bridge, cmd(0.03))
        check(rc == rhal.SKR_OK and time.monotonic() - began < 0.02, "and the same command again returns at once: the image has it already, and there is nothing to wait for")
        exchanges_after(sim, 1, 2.0)
        bridge.emergency_stop()
        check(until(lambda: not sim.go_to, 0.4), "a stop made just after an exchange reaches the gripper in a fraction of the second that is left, and does not wait for the next cycle")
        check(bridge.release_stop() == rhal.SKR_OK, "and releases")
        bridge.close()

        section("the first read of the gripper is tried three times")
        sim.skip_requests = 2
        code, said, bridge = opens_with(sim, open_timeout_ms=2000, timeout_ms=100)
        check(code == rhal.SKR_OK, "a gripper that misses the first two status reads is found by the third (the maker's `initializeImage` tries three times)", said)
        bridge.close()
        sim.skip_requests = 3
        code, said, bridge = opens_with(sim, open_timeout_ms=2000, timeout_ms=100)
        check(code == rhal.SKR_EIO and "no gripper answered" in said, "and one that misses three is not there", said)
        bridge.close()
        sim.skip_requests = 0

        section("staleness and a dead line are two things")
        bridge = opened(sim, rate_hz=5.0, stale_ms=100.0)
        ready(bridge)
        reads = []
        for _ in range(60):
            reads.append(state(bridge)[0])
            time.sleep(0.02)
        check(rhal.SKR_ETIMEDOUT in reads and rhal.SKR_OK in reads, "a status older than the staleness bound is ETIMEDOUT with no failed exchange anywhere: five exchanges a second and a bound of a tenth leave the status stale half the time")
        bridge.close()
        bridge = opened(sim, timeout_ms=30, stale_ms=20000.0)
        ready(bridge)
        sim.drop = True
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 1.5), "three failed exchanges are ETIMEDOUT though the last sample is younger than a bound of twenty seconds")
        sim.drop = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 2.0), "and the state is read again once the line is")
        sim.skip_requests = 1
        seen = set()
        end = time.monotonic() + 0.3
        while time.monotonic() < end:
            seen.add(state(bridge)[0])
            time.sleep(0.005)
        check(seen == {rhal.SKR_OK}, "one lost frame is not an outage: it takes three in a row (the maker's own threshold) to call the line dead")
        sim.skip_requests = 0
        bridge.close()

        section("the exchange cycle keeps its period, and a stream of commands cannot flood the line")
        bridge = opened(sim, rate_hz=100)
        ready(bridge)
        count = sim.exchanges
        time.sleep(0.5)
        made = sim.exchanges - count
        check(35 <= made <= 70, f"at 100 Hz the gripper is exchanged with about fifty times in half a second, not more ({made})")
        bridge.close()
        bridge = opened(sim, rate_hz=10)
        ready(bridge)
        count = sim.exchanges
        end = time.monotonic() + 0.5
        sent = 0
        while time.monotonic() < end:
            write(bridge, cmd(0.02 if sent % 2 else 0.06))
            sent += 1
        made = sim.exchanges - count
        check(sent > 20 and made <= 160, f"{sent} commands one after another in half a second make {made} exchanges: never more than the 250 a second a line at 115200 baud can carry")
        bridge.close()

        section("the stop and a command that has already passed its first look at the latch")
        bridge = opened(sim)
        ready(bridge)
        original = bridge._gate

        def gate_then_stop() -> None:
            bridge.emergency_stop()       # the stop lands after `write_command` looked at the latch, and before it takes the lock that changes the image
            original()

        bridge._gate = gate_then_stop
        rc, said = write(bridge, cmd(0.04))
        bridge._gate = original
        check(rc == rhal.SKR_EPERM, "a command that passed the latch just before a stop landed is refused when it takes the lock")
        time.sleep(0.15)
        check(not sim.go_to and sim.commands[-1].action & GTO == 0, "and no go-to reached the gripper after the stop: the check is made again under the lock the stop takes")
        check(bridge.release_stop() == rhal.SKR_OK, "and the stop releases")
        bridge.close()

        section("a stop that nothing has confirmed stays latched")
        bridge = opened(sim, timeout_ms=30)
        ready(bridge)
        bridge.emergency_stop()
        until(lambda: not sim.go_to)
        bridge.release_stop()
        sim.drop = True
        time.sleep(0.2)
        bridge.emergency_stop()
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            rc = bridge.release_stop()
        check(rc == rhal.SKR_EIO, "a gripper already halted shows gGTO clear in the last sample it gave, and that sample is older than the stop: release_stop wants an exchange after it, and with the line dead there is none")
        sim.drop = False
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        check(bridge.release_stop() == rhal.SKR_OK, "and when the line is back it releases")
        bridge.close()

        section("a release waits for the gripper to show the halt, not for any exchange")
        lagged = new_sim(activated=True)
        try:
            bridge = opened(lagged)
            ready(bridge)
            write(bridge, cmd(0.02))
            until(lambda: lagged.go_to)
            lagged.echo_lag_s = 0.4
            time.sleep(0.5)
            bridge.emergency_stop()
            began = time.monotonic()
            rc = bridge.release_stop()
            took = time.monotonic() - began
            check(rc == rhal.SKR_OK and took >= 0.25, f"with a status that shows the halt 0.4 s after it was made, release_stop waits for it ({took * 1000:.0f} ms) and does not take the first exchange that comes", str(rc))
            bridge.close()
        finally:
            lagged.stop()

        section("a command that is lost is put back, with no dead line to put everything back for it")
        bridge = opened(sim, rate_hz=5.0, timeout_ms=30)
        ready(bridge)
        check(write(bridge, cmd(0.05))[0] == rhal.SKR_OK, "a command at five exchanges a second is carried")
        sim.skip_requests = 1
        rc, said = write(bridge, cmd(0.02))
        check(rc == rhal.SKR_EIO and "put back" in said, "and the next one, whose one exchange is lost, is EIO inside its budget", said)
        sim.skip_requests = 0
        time.sleep(0.7)
        check(sim.commands[-1].position == register(0.05) and all(c.position == register(0.05) for c in sim.commands[-3:]), "and is not written when the next period comes round: one lost frame is not a dead line, and the image went back by itself")
        bridge.close()

        section("refusals are said when they are new")
        bridge = opened(sim)
        ready(bridge)
        first = write(bridge, cmd(0.2))
        again_ok = write(bridge, cmd(0.04))
        second = write(bridge, cmd(0.2))
        check(first[1] != "" and again_ok[0] == rhal.SKR_OK and second[1] != "", "a refusal that comes back after a command that was accepted is said again: it is a new run")
        sim.set_fault(0x0E)
        exchanges_after(sim, 3)
        said_a = write(bridge, cmd(0.04))[1]
        sim.set_fault(0x0A)
        exchanges_after(sim, 3)
        said_b = write(bridge, cmd(0.04))[1]
        check("0x0E" in said_a and "0x0A" in said_b, "and two refusals with different causes are each said, naming their own code")
        sim.set_fault(0, 0)
        bridge.close()

        section("the checks of a custom profile that the first pass left")
        good = {"min_speed": 0.03, "max_speed": 0.25, "min_opening": 0.0, "max_opening": 0.14, "open_position": 5, "closed_position": 220}
        code, said = refused(json.dumps({**settings(sim), "model": "custom", "profile": {**good, "colour": "red"}}))
        check(code != rhal.SKR_OK and "exactly" in said, "a custom profile with a key it does not know is refused: exactly the six", said)
        code, said = refused(json.dumps({**settings(sim), "model": "custom", "profile": {**good, "closed_position": 5}}))
        check(code != rhal.SKR_OK and "open_position" in said, "and one whose open and closed registers are the same register: it has no band to convert on", said)
        code, said = refused(json.dumps({**settings(sim), "model": "custom", "profile": {**good, "max_opening": 0.0}}))
        check(code != rhal.SKR_OK and "min_opening" in said, "and one with no stroke", said)

        section("the descriptor, in the parts the first pass did not read")
        bridge = opened(sim, simulator=False, limits=[0.01, 0.07], rate_hz=40)
        descriptor = json.loads(bridge.describe())
        check(descriptor["simulator"] is False, "a gripper the operator says is not a simulation is described as one that is not")
        check(descriptor["joints"][0]["role"] == "right_hand.gripper.grip" and descriptor["end_effectors"][0]["role"] == "right_hand.gripper.grip", "the role the operator asserted is on the joint and on the end effector")
        check(descriptor["joints"][0]["limits"]["position_max_rad"] == 0.07 and abs(descriptor["end_effectors"][0]["max_opening_m"] - 0.06) < 1e-9, "the narrowed limits are in both, the opening of the end effector the width between them")
        check(descriptor["control"]["rate_hz_max"] == 40 and descriptor["control"]["modes"] == ["position"], "and the exchange rate and the one mode are said")
        bridge.close()

        section("a gap inside an answer that is longer than the byte timeout, with a transaction budget that could wait for it")
        bridge = opened(sim, timeout_ms=1000, stale_ms=20000.0)
        ready(bridge)
        sim.split_gap_s = 0.2
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 4.0), "an answer that stops for 0.2 s in the middle is a failed exchange though the whole transaction has a second: the gap between bytes is 50 ms, and an answer that waits longer is a gripper that stopped talking")
        sim.split_gap_s = 0.0
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 4.0), "and the state is read again when it stops")
        bridge.close()

        section("the line, below the bridge: what is left on it, and what is not coming back")
        master, slave_fd = os.openpty()
        import tty
        tty.setraw(slave_fd)
        try:
            serial = link.Link(device=os.ttyname(slave_fd))
            serial.open()
            os.write(master, frames.with_crc(bytes([9, 3, 6, 0xB9, 0, 0, 0x11, 0x22, 0x33])))       # a late answer to an earlier request, already on the line when the next one is written

            def answer_the_request() -> None:
                os.read(master, 64)
                time.sleep(0.01)
                os.write(master, frames.with_crc(bytes([9, 3, 6, 0xB9, 0, 0, 0x44, 0x55, 0x66])))

            responder = threading.Thread(target=answer_the_request, daemon=True)
            responder.start()
            time.sleep(0.02)
            data = serial.transact(frames.request_read(9, frames.STATUS_ADDRESS, 3), frames.FC_READ_HOLDING, 9, 1.0)
            responder.join(1.0)
            check(data[3:] == bytes([0x44, 0x55, 0x66]), "an answer that was left on the line by an earlier request is thrown away before the next is sent, and not read as its answer", data.hex())
            serial.close()
        finally:
            os.close(master)
            os.close(slave_fd)
        import socket
        for how in ("closes", "resets"):
            server_socket = socket.socket()
            server_socket.bind(("127.0.0.1", 0))
            server_socket.listen(1)

            def hang_up_on_it(server_socket=server_socket, how=how) -> None:
                connection, _ = server_socket.accept()
                if how == "resets":
                    connection.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
                time.sleep(0.05)
                connection.close()

            hanger = threading.Thread(target=hang_up_on_it, daemon=True)
            hanger.start()
            gateway = link.Link(host="127.0.0.1", port=server_socket.getsockname()[1])
            gateway.open()
            time.sleep(0.15)
            try:
                gateway.transact(frames.request_read(9, frames.STATUS_ADDRESS, 3), frames.FC_READ_HOLDING, 9, 0.5)
                gone = False
            except link.LinkError:
                gone = True
            check(gone and not gateway.is_open, f"a gateway that {how} the connection is a failed transaction, and leaves the line closed, so that whoever drives it knows to open it again")
            server_socket.close()

        section("a close in the middle of a command, and a session that leaves nothing behind")
        bridge = opened(sim, timeout_ms=30)
        ready(bridge)
        sim.drop = True
        holder = []
        began = time.monotonic()
        runner = threading.Thread(target=lambda: holder.append(write(bridge, cmd(0.01))))
        runner.start()
        time.sleep(0.05)
        bridge.close()
        runner.join(1.0)
        took = time.monotonic() - began
        sim.drop = False
        check(not runner.is_alive() and holder and holder[0][0] == rhal.SKR_EIO and took < 0.15, f"a command waiting for the line is ended by the close at once, and not left to its own budget ({took * 1000:.0f} ms of the 200)", str(holder))
        fds, threads = len(os.listdir("/proc/self/fd")), threading.active_count()
        for _ in range(15):
            session = opened(sim)
            ready(session)
            session.close()
        check(len(os.listdir("/proc/self/fd")) <= fds + 1 and threading.active_count() <= threads, "fifteen sessions leave no descriptor and no thread behind")
        bridge = opened(sim, rate_hz=50)
        ready(bridge)
        count = sim.exchanges
        time.sleep(0.5)
        one = sim.exchanges - count
        bridge.open(config(sim, rate_hz=50))
        ready(bridge)
        count = sim.exchanges
        time.sleep(0.5)
        two = sim.exchanges - count
        check(one > 15 and two <= one * 1.4, f"an open on an open driver leaves one exchange loop, and not two ({one} exchanges in half a second before, {two} after)")
        # The rate above is a measurement and a loaded machine moves it (the full campaign's `robotiq-open-no-teardown` survived one run on that alone); the threads are a count.
        loops = [thread.name for thread in threading.enumerate() if thread.name == "robotiq-exchange"]
        check(len(loops) == 1, f"and the first session's exchange thread is gone, not still running beside the second's ({len(loops)} alive)")
        bridge.close()

        section("the gateway down, and the reasons it gives")
        gate = new_sim(tcp=True)
        try:
            bridge = opened(gate)
            ready(bridge)
            base = gate.connections
            gate.refuse = True
            gate.hang_up()
            time.sleep(1.7)
            made = gate.connections - base
            check(2 <= made <= 5, f"a gateway that takes a connection and drops it is tried again about twice a second, and not every cycle ({made} attempts in 1.7 s)")
            gate.refuse = False
            ready(bridge)
            gate.go_down()
            check(until(lambda: "could not be opened" in str(ext(bridge, "status")[1].get("link_dead")), 4.0), "a gateway that is not listening says so: the status gives the refusal of the connection as the reason, and not that the line is not open")
            gate.come_up()
            check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 4.0), "and it is connected to again when it listens")
            bridge.close()
        finally:
            gate.stop()

        section("the handshake, with a gripper that is slow to show what it was told")
        lagging = new_sim(activated=True)
        try:
            lagging.reset_lag_s = 0.3
            bridge = opened(lagging)
            ready(bridge)
            check(lagging.activation_state == frames.STA_COMPLETE, "a gripper that is activated, to begin with")
            mark = len(lagging.commands)
            rc, body = ext(bridge, "recover timeout=10")
            edges = [i for i, c in enumerate(lagging.commands[mark:], mark) if c.action == 0]
            sets = [i for i, c in enumerate(lagging.commands[mark:], mark) if c.action == ACT and edges and i > edges[0]]
            check(rc == rhal.SKR_OK and edges and sets and lagging.command_times[sets[0]] - lagging.command_times[edges[0]] >= 0.25,
                  "rACT is set again only once the gripper has been seen to be reset: with a status that goes on saying activated for 0.3 s the driver waits that long between the two edges", str(body))
            bridge.close()
        finally:
            lagging.stop()
        slowest = new_sim(activated=False, time_scale=1.0)
        try:
            bridge = opened(slowest)
            ready(bridge)
            runner = threading.Thread(target=lambda: ext(bridge, "activate"))
            runner.start()
            until(lambda: slowest.activation_state == frames.STA_IN_PROGRESS)
            time.sleep(0.05)
            rc, body = ext(bridge, "activate")
            runner.join(5.0)
            resets = [c for c in slowest.commands if c.action == 0]
            check(rc == rhal.SKR_OK and body["result"] == "activated" and len(resets) == 1, "an activate that finds the activation already running waits for it and does not begin it again: one reset, not two", str(body))
            bridge.close()
        finally:
            slowest.stop()

        section("the motions, in the parts the first pass did not read")
        bridge = opened(sim)
        ready(bridge)
        ext(bridge, "move position=0.03")
        check(sim.commands[-1].speed == SPEED_REGISTER and sim.commands[-1].force == EFFORT_REGISTER, "a move with no speed and no effort uses the operator's")
        ext(bridge, "move position=0.05 effort=0")
        check(sim.commands[-1].force == 0, "and an effort of nothing is nothing: it is the gripper's minimum force, and not the same as having none to give")
        ext(bridge, "move position=0.04 effort=1")
        check(sim.commands[-1].force == 255, "and one is the maximum")
        image_before = sim.commands[-1]
        rc, body = ext(bridge, "move position=0.04 speed=1")
        time.sleep(0.1)
        check(rc == rhal.SKR_EINVAL and all(c == image_before for c in sim.commands[-3:]), "a refused move changes nothing: the image is what it was", str(body))
        rc, body = ext(bridge, "move position=nan")
        check(rc == rhal.SKR_EINVAL and "outside" in str(body), "a position that is not a number is outside the range", str(body))
        rc, body = ext(bridge, "status")
        check(abs(body["current_a"] - 0.42) < 1e-9, "status gives the motor current, at the maker's 0.010 A a step: a gCU of 42 is 0.42 A", str(body))
        ext(bridge, "grip effort=0.3")
        rc, out = state(bridge)
        check(rc == rhal.SKR_OK and out.position_rad[0] == 0.0, "the fingers closed all the way read as an opening of nothing, and not as no reading")
        bridge.close()
        bridge = opened(sim, limits=[0.01, 0.07])
        ready(bridge)
        ext(bridge, "grip effort=0.3")
        check(sim.commands[-1].position == register(0.01), "a grip closes as far as the narrowed limit and no further")
        ext(bridge, "release")
        check(sim.commands[-1].position == register(0.07), "and a release opens as far as the narrowed limit")
        bridge.close()
        sim.blocks_opening = register(0.06)
        bridge = opened(sim)
        ready(bridge)
        ext(bridge, "grip effort=0.3")
        rc, body = ext(bridge, "release")
        check(rc == rhal.SKR_OK and body["result"] == "an object detected while opening" and abs(body["opening_m"] - 0.06) < 0.003, "a release that meets something while it opens says so, and where the fingers are", str(body))
        sim.blocks_opening = None
        ext(bridge, "release")
        bridge.close()
        sim.echo_lag_s = 0.15
        bridge = opened(sim)
        ready(bridge)
        rc, body = ext(bridge, "move position=0.03")
        check(rc == rhal.SKR_OK and abs(body["opening_m"] - 0.03) < 0.002, "with a status that shows a command a moment after it was written, a move still ends at the place it was told to go to: the old echo and the old position do not end it", str(body))
        sim.echo_lag_s = 0.0
        bridge.close()
        slow = new_sim(time_scale=1.0)
        try:
            bridge = opened(slow, speed=0.02)
            ready(bridge)
            holder = []
            runner = threading.Thread(target=lambda: holder.append(ext(bridge, "move position=0.0")))
            began = time.monotonic()
            runner.start()
            time.sleep(0.4)
            slow.set_fault(0x0E)
            runner.join(3.0)
            check(not runner.is_alive() and holder and holder[0][0] == rhal.SKR_EIO and "0x0E" in str(holder[0][1]) and time.monotonic() - began < 2.5,
                  "a move whose gripper faults ends with the fault named, and is not left to wait out its own timeout", str(holder))
            bridge.close()
        finally:
            slow.stop()

        section("a close does not stop the gripper")
        slow = new_sim(time_scale=1.0)
        try:
            bridge = opened(slow, speed=0.02)
            ready(bridge)
            write(bridge, cmd(0.0))
            time.sleep(0.3)
            bridge.close()
            at_close = slow.position
            time.sleep(0.5)
            check(slow.go_to and slow.position > at_close, f"the fingers go on closing after a close, as the maker's own SDK leaves them (register {at_close}, then {slow.position})")
        finally:
            slow.stop()
    finally:
        sim.stop()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    try:
        code = main()
    except BaseException:
        # Several checks capture what the driver says with `redirect_stderr`, from threads as well as the main one, and a redirect that is left half undone leaves `sys.stderr` pointing at a buffer:
        # the traceback of a failure would then be written to nowhere, and the run would end with an exit status and no word.
        sys.stderr = sys.__stderr__
        raise
    raise SystemExit(code)
