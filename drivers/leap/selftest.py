"""The LEAP Hand bridge against a simulated hand written here (DRY-COVERAGE-PLAN.md item 12.E): the frames, what it refuses to open on, how the hand is brought up, the state, the refusals, the faults, the stop,
the motions and the line.

    PYTHONPATH=../../bindings/python:. python3 selftest.py

Needs python3 on Linux (the simulated hand is a pseudo-terminal) and nothing of the makers'. The simulated hand agrees with whoever wrote it (F15); `layout_check.py` holds the driver's bytes and registers to
ROBOTIS's own `dynamixel_sdk`, which is run, and what this shows is the driver's own logic.

Every wait is for a state (`until`), never for a count of polls or a sleep that something else is supposed to have finished inside: a loaded machine stretches every sleep (F210, `tools/flake-hunt.sh`).
"""

from __future__ import annotations

import contextlib
import dataclasses
import io
import json
import math
import os
import sys
import termios
import threading
import time
from unittest import mock

import skynet_rhal as rhal

from skynet_leap import bridge as bridge_module, frames, link
from skynet_leap.bridge import LeapBridge, TOKEN
from skynet_leap.hand_sim import SimulatedHand

CHECKS = 0
FAILURES: list[str] = []
SCALE = 20.0           # the simulated servos move this much faster than their speed says
JOINTS = frames.JOINTS
KP, KI, KD, SIDE = 600, 0, 200, 0.75
CURRENT = 350


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


def raises(kind: type, function, *args) -> str | None:
    """The message of the `kind` the call raises, or None when it raises nothing (or something else)."""
    try:
        function(*args)
    except kind as error:
        return str(error)
    except Exception:  # noqa: BLE001
        return None
    return None


def new_sim(variant: str = "full", **kw) -> SimulatedHand:
    kw.setdefault("time_scale", SCALE)
    return SimulatedHand(variant, **kw).start()


def settings(sim: SimulatedHand, **extra) -> dict:
    out = {"variant": "full", "stop_mode": "hold", "current_ma": CURRENT, "gains": {"kp": KP, "ki": KI, "kd": KD}, "side_scale": SIDE, "simulator": True, "rate_hz": 50, "timeout_ms": 100,
           "roles": {"index_pip": "right_hand.index_pip.flex", "thumb_mcp_side": "right_hand.thumb_mcp.abduct"}}
    out.update({"host": "127.0.0.1", "port": sim.port} if sim.port else {"device": sim.path})
    out.update(extra)
    return {k: v for k, v in out.items() if v is not None}


def config(sim: SimulatedHand, **extra) -> str:
    return json.dumps(settings(sim, **extra))


def opened(sim: SimulatedHand, **extra) -> LeapBridge:
    bridge = LeapBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    assert code == rhal.SKR_OK, said.getvalue()
    return bridge


def opens_with(sim: SimulatedHand, **extra) -> tuple[int, str, LeapBridge]:
    bridge = LeapBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    return code, said.getvalue(), bridge


def refused(text: str) -> tuple[int, str]:
    bridge = LeapBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(text)
    bridge.close()
    return code, said.getvalue()


def state(bridge: LeapBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def cmd(values: list[float], mask: list[int] | None = None, mode: int = rhal.MODE_POSITION, count: int | None = None) -> rhal.Command:
    n = count if count is not None else len(values)
    return rhal.Command(stamp_ns=1, seq=1, mode=mode, values=list(values), mask=mask if mask is not None else [1] * n)


def pose(value: float | list[float]) -> list[float]:
    return [value] * 16 if isinstance(value, (int, float)) else list(value)


def write(bridge: LeapBridge, command: rhal.Command) -> tuple[int, str]:
    bridge._last_refusal = None  # noqa: SLF001 - a refusal is said once for a run of the same cause; each check asks to hear it
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def ext(bridge: LeapBridge, text: str) -> tuple[int, object]:
    code, body = bridge.extension(TOKEN, text.encode())
    try:
        return code, json.loads(body)
    except json.JSONDecodeError:
        return code, body.decode()


def ready(bridge: LeapBridge, timeout: float = 5.0) -> bool:
    return until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, timeout)


def reg(angle: float) -> int:
    return frames.position_to_register(angle)


def said_by(function, *args) -> tuple[object, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        result = function(*args)
    return result, said.getvalue()


def fds() -> int:
    return len(os.listdir("/proc/self/fd"))


def settled_fds() -> int:
    """The number of open descriptors once nothing is closing them: the simulated hand's own threads close their side of a connection a moment after the driver closes its side."""
    last, since, deadline = fds(), time.monotonic(), time.monotonic() + 3.0
    while time.monotonic() < deadline:
        time.sleep(0.02)
        now = fds()
        if now != last:
            last, since = now, time.monotonic()
        elif time.monotonic() - since >= 0.25:
            break
    return last


def holding_the_line(bridge: LeapBridge, seconds: float) -> threading.Thread:
    """A thread that owns the line's lock for `seconds`, as a transaction in flight does."""
    taken = threading.Event()

    def hold() -> None:
        with bridge._link_lock:  # noqa: SLF001
            taken.set()
            time.sleep(seconds)

    thread = threading.Thread(target=hold, daemon=True)
    thread.start()
    taken.wait(1.0)
    return thread


def main_frames() -> None:
    # ── the protocol ──
    section("frames — the e-manual's own ping, CRC and byte stuffing")
    check(frames.ping(1) == bytes.fromhex("FFFFFD0001030001194E"), "a ping of servo 1 is the e-manual's example packet (FF FF FD 00 01 03 00 01 19 4E)")
    check(frames.crc16(bytes.fromhex("FFFFFD00010300 01")) == 0x4E19, "and its CRC-16 (polynomial 0x8005, no reflection, initial 0) is 0x4E19")
    check(frames.crc16(b"") == 0 and frames.crc16(b"\x00") == 0, "the CRC of nothing is 0")
    packet = frames.read(3, 132, 4)
    check(packet[:4] == frames.HEADER and packet[4] == 3 and packet[5:7] == (7).to_bytes(2, "little") and packet[7] == frames.INST_READ and packet[8:12] == bytes([132, 0, 4, 0]), "a read is the header, the id, a length of 7, the instruction and an address and a length")
    check(int.from_bytes(packet[-2:], "little") == frames.crc16(packet[:-2]), "and ends in the CRC of everything before it")
    check(frames.stuff(b"\x01\xff\xff\xfd\x02") == b"\x01\xff\xff\xfd\xfd\x02", "a FF FF FD in the parameters is followed by one more FD")
    check(frames.stuff(b"\xff\xff\xfd\xff\xff\xfd") == b"\xff\xff\xfd\xfd\xff\xff\xfd\xfd", "and so is each of two in a row")
    check(frames.stuff(b"\xff\xff\xff\xfd") == b"\xff\xff\xff\xfd\xfd", "a run of three FF before the FD is one FF FF FD, not two")
    check(frames.stuff(b"\xff\xfd\xff\xff") == b"\xff\xfd\xff\xff", "and an FF FD that is not preceded by FF FF is left as it is")
    for sample in (b"", b"\xff\xff\xfd", b"\x00\xff\xff\xfd\xfd\xff", bytes(range(256))):
        check(frames.unstuff(frames.stuff(sample)) == sample, f"unstuff undoes stuff ({len(sample)} bytes)")
    stuffed = frames.packet(1, frames.INST_WRITE, b"\x74\x00\xff\xff\xfd\x00")
    check(stuffed[5] | stuffed[6] << 8 == len(frames.stuff(b"\x74\x00\xff\xff\xfd\x00")) + 3, "a stuffed packet's length counts the stuffed bytes")
    check(frames.sync_read(126, 10, [0, 1])[7] == frames.INST_SYNC_READ and frames.sync_read(126, 10, [0, 1])[4] == frames.BROADCAST, "a synchronised read is sent to the broadcast id")
    check(frames.sync_read(126, 10, [0, 1])[8:14] == bytes([126, 0, 10, 0, 0, 1]), "with an address, a length and the ids")
    check(frames.sync_write(116, 4, {0: b"", 5: b""})[8:22] == bytes([116, 0, 4, 0, 0, 1, 2, 3, 4, 5, 5, 6, 7, 8]), "a synchronised write is an address, a length and an id with its bytes for each servo")
    check(raises(frames.ProtocolError, frames.sync_write, 116, 4, {0: b"\x01"}) is not None, "a synchronised write of the wrong number of bytes for a servo is refused")
    check(frames.reboot(4)[7] == frames.INST_REBOOT, "a reboot is instruction 8")

    section("frames — status packets")
    good = frames.packet(2, frames.INST_STATUS, bytes([0, 0x34, 0x12]))
    parsed = frames.parse_status(good)
    check(parsed is not None and parsed[0].servo == 2 and parsed[0].error == 0 and parsed[0].params == b"\x34\x12" and parsed[1] == len(good), "a status packet is read: the servo, the error byte and the parameters")
    check(frames.parse_status(good[:-1]) is None and frames.parse_status(good[:5]) is None, "and one that has not all arrived is waited for")
    check(frames.parse_status(b"noise" + good) is not None and frames.parse_status(b"noise" + good)[0].servo == 2, "and noise before its header is skipped")
    alert = frames.parse_status(frames.packet(3, frames.INST_STATUS, bytes([0x80, 1])))
    check(alert is not None and alert[0].alert and alert[0].error_number == 0, "an error byte of 0x80 is the alert flag with no error number")
    result = frames.parse_status(frames.packet(3, frames.INST_STATUS, bytes([0x84])))
    check(result is not None and result[0].alert and result[0].error_number == 4, "and 0x84 is the alert with data range error")
    bad = bytearray(good)
    bad[-1] ^= 0xFF
    check(raises(frames.ProtocolError, frames.parse_status, bytes(bad)) is not None and "CRC" in (raises(frames.ProtocolError, frames.parse_status, bytes(bad)) or ""), "a status packet with a wrong CRC is refused, and says so")
    request = frames.packet(2, frames.INST_READ, b"\x00\x00\x00")
    check(frames.parse_status(request) is None, "a request heard back (a half-duplex adapter's echo) is not a status packet, and is skipped, not refused")
    check(frames.parse_status(request + good) is not None and frames.parse_status(request + good)[0].params == b"4", "and the status packet after it is found")
    huge = frames.HEADER + bytes([2, 0xFF, 0xFF, 0x55])
    check(frames.parse_status(huge + b"\x00" * 8) is None and frames.parse_status(huge + b"\x00" * 8 + good) is not None, "a header that claims 65 kilobytes is noise, and a packet after it is found")
    check(frames.parse_status(frames.HEADER + bytes([2, 2, 0, 0x55]) + good) is not None, "and so is one that claims fewer bytes than an instruction, an error byte and a CRC")
    check(frames.parse_status(frames.HEADER + bytes([253, 5, 0, 0x55, 0, 0, 0]) + good) is not None, "and one from an id no servo has (above 252)")
    both = frames.parse_status(good + good)
    check(both is not None and both[1] == len(good), "two packets in a row are read one at a time")

    section("frames — registers and units")
    check(frames.position_to_register(0.0) == 2048 and frames.register_to_position(2048) == 2048 * frames.RAD_PER_TICK - frames.OFFSET_RAD, "the hand's zero is the servo's 3.14159 rad, register 2048 (4096 ticks to a turn)")
    check(abs(frames.register_to_position(2048)) < 1e-5, "and it reads back as zero to within the maker's rounding of π")
    check(frames.position_to_register(1.0) == int(round((1.0 + 3.14159) / frames.RAD_PER_TICK)), "an angle is rounded to the nearest tick")
    check(frames.position_to_register(frames.RAD_PER_TICK * 0.49) == 2048 and frames.position_to_register(frames.RAD_PER_TICK * 0.51) == 2049, "and half a tick is where it rounds")
    for angle in (-1.34, -0.5, 0.0, 1.0, 2.443):
        check(abs(frames.register_to_position(frames.position_to_register(angle)) - angle) <= frames.RAD_PER_TICK / 2 + 1e-9, f"{angle} rad reads back within half a tick")
    check(raises(frames.ProtocolError, frames.position_to_register, math.nan) is not None and raises(frames.ProtocolError, frames.position_to_register, math.inf) is not None, "NaN and infinity are not angles")
    check(frames.pack_u32(-1) == b"\xff\xff\xff\xff" and frames.i32(frames.pack_u32(-5)) == -5 and frames.pack_u32(2048) == (2048).to_bytes(4, "little"), "a goal is four bytes, signed or not")
    check(raises(frames.ProtocolError, frames.pack_u16, 70000) is not None and raises(frames.ProtocolError, frames.pack_u32, 1 << 33) is not None, "a value that does not fit its register is refused")
    check(len(frames.JOINTS) == 16 and frames.JOINTS[0] == "index_mcp_side" and frames.JOINTS[3] == "index_dip" and frames.JOINTS[12] == "thumb_mcp_side" and frames.JOINTS[15] == "thumb_dip", "the joints are the maker's: index, middle, ring and thumb, each from its side-to-side MCP to its DIP")
    check(all(lo < 0 < hi for lo, hi in zip(frames.LIMITS_MIN, frames.LIMITS_MAX)) and len(frames.LIMITS_MIN) == len(frames.LIMITS_MAX) == 16, "every joint's range spans zero, and there are sixteen")
    check(all(0 <= reg(lo) < reg(hi) <= 4095 for lo, hi in zip(frames.LIMITS_MIN, frames.LIMITS_MAX)), "and fits one turn of the servo, so a goal never needs the multi-turn range")
    check(frames.SPAN_STATE == (64, 7) and frames.SPAN_GAINS == (80, 24) and frames.SPAN_MOTION == (116, 20), "a cycle reads 64..70, 80..103 and 116..135")
    check(frames.joint_fault(0) is None and frames.joint_fault(0x04) == 0x04 and frames.joint_fault(0x02) == 0x02, "any hardware error bit is a fault, named or not")
    check("overload" in frames.describe_hw_error(0x20) and "input voltage" in frames.describe_hw_error(0x01) and "not name" in frames.describe_hw_error(0x02), "and the bits the e-manual names are named, and the others are said not to be")
    check(frames.describe_hw_error(0) == "no error", "no bits is no error")
    check(frames.VARIANTS["lite"].model_number == 1200 and frames.VARIANTS["full"].model_number == 1240, "the Lite's servos are model 1200 and the Full's 1240")

    section("frames — the e-manual's numbers and the maker's ranges, retyped here as literals so that a changed constant is a red check")
    check((frames.INST_PING, frames.INST_READ, frames.INST_WRITE, frames.INST_REBOOT) == (0x01, 0x02, 0x03, 0x08), "the instructions: ping 1, read 2, write 3, reboot 8")
    check((frames.INST_SYNC_READ, frames.INST_SYNC_WRITE, frames.INST_FAST_SYNC_READ, frames.INST_STATUS) == (0x82, 0x83, 0x8A, 0x55), "synchronised read 0x82, synchronised write 0x83, fast synchronised read 0x8A and a status 0x55")
    check(frames.HEADER == b"\xff\xff\xfd\x00" and frames.BROADCAST == 0xFE and frames.ALERT == 0x80, "the header, the broadcast id and the alert flag")
    check(frames.ERROR_NUMBERS == {1: "result fail", 2: "instruction error", 3: "CRC error", 4: "data range error", 5: "data length error", 6: "data limit error", 7: "access error"}, "and the seven error numbers")
    check(frames.MAX_ID == 0xFC and frames.MAX_PACKET == 1024, "a servo id is at most 252 and a packet claims at most 1,024 bytes")
    check(frames.crc16(b"123456789") == 0xFEE8, "the CRC of \"123456789\" is 0xFEE8, the published check value of CRC-16/BUYPASS (polynomial 0x8005, initial value 0, no reflection)")
    check(frames.crc16(bytes.fromhex("FFFFFD0001030001")) == 0x4E19, "and of the e-manual's ping, 0x4E19")
    registers = {"model": (frames.ADDR_MODEL_NUMBER, 0), "mode": (frames.ADDR_OPERATING_MODE, 11), "limit": (frames.ADDR_CURRENT_LIMIT, 38), "shutdown": (frames.ADDR_SHUTDOWN, 63), "torque": (frames.ADDR_TORQUE_ENABLE, 64),
                 "error": (frames.ADDR_HARDWARE_ERROR, 70), "D": (frames.ADDR_POSITION_D, 80), "I": (frames.ADDR_POSITION_I, 82), "P": (frames.ADDR_POSITION_P, 84), "goal current": (frames.ADDR_GOAL_CURRENT, 102),
                 "goal": (frames.ADDR_GOAL_POSITION, 116), "moving": (frames.ADDR_MOVING, 122), "current": (frames.ADDR_PRESENT_CURRENT, 126), "velocity": (frames.ADDR_PRESENT_VELOCITY, 128),
                 "position": (frames.ADDR_PRESENT_POSITION, 132), "voltage": (frames.ADDR_PRESENT_VOLTAGE, 144), "temperature": (frames.ADDR_PRESENT_TEMPERATURE, 146)}
    for name, (have, want) in registers.items():
        check(have == want, f"the {name} register is at {want} (the e-manual's control table)")
    check(frames.OPERATING_MODE_CURRENT_POSITION == 5, "current-based position control is operating mode 5")
    check(abs(frames.RAD_PER_TICK - 0.0015339807878856412) < 1e-15 and abs(frames.VELOCITY_RAD_S_PER_UNIT - 0.229 * 2 * math.pi / 60) < 1e-12 and frames.CURRENT_MA_PER_UNIT == 1.0 and frames.OFFSET_RAD == 3.14159,
          "a tick is 2π/4096 rad, a velocity unit 0.229 rev/min, a current unit 1 mA, and the hand's zero is the servo's 3.14159 rad")
    check(frames.HW_ERROR_BITS == {0x01: "input voltage error", 0x04: "overheating error", 0x10: "electrical shock error", 0x20: "overload error"} and frames.KNOWN_HW_ERROR_MASK == 0x35, "the four hardware error bits the e-manual names")
    check([(v.model_name, v.model_number, v.max_current_ma) for v in frames.VARIANTS.values()] == [("XL330-M288-T", 1200, 350), ("XC330-M288-T", 1240, 550)], "and the two builds: their servos, model numbers and the most current the maker's readme suggests for each")
    maker_min = (-1.047, -0.314, -0.506, -0.366, -1.047, -0.314, -0.506, -0.366, -1.047, -0.314, -0.506, -0.366, -0.349, -0.47, -1.20, -1.34)
    maker_max = (1.047, 2.23, 1.885, 2.042, 1.047, 2.23, 1.885, 2.042, 1.047, 2.23, 1.885, 2.042, 2.094, 2.443, 1.90, 1.88)
    for joint, name in enumerate(frames.JOINTS):
        check(frames.LIMITS_MIN[joint] == maker_min[joint] and frames.LIMITS_MAX[joint] == maker_max[joint], f"{name} ranges from {maker_min[joint]} to {maker_max[joint]} rad (the maker's `LEAPsim_limits`)")
    check(frames.i16(b"\xff\xff") == -1 and frames.u16(b"\xff\xff") == 65535 and frames.i32(b"\xff\xff\xff\xff") == -1 and frames.u32(b"\xff\xff\xff\xff") == 4294967295, "the readers: signed where the register is signed, and not where it is not")
    check(frames.u16(b"\x00\x01\x02\x03", 1) == 0x0201 and frames.u32(b"\x00\x01\x02\x03\x04\x05", 1) == 0x04030201 and frames.i16(b"\x00\xfe\xff", 1) == -2 and frames.i32(b"\x00\xfc\xff\xff\xff", 1) == -4, "at an offset, little-endian")
    check(frames.pack_u16(0) == b"\x00\x00" and frames.pack_u16(65535) == b"\xff\xff" and frames.pack_u16(0x0102) == b"\x02\x01", "a sixteen-bit register is packed little-endian, over its whole range")
    check(raises(frames.ProtocolError, frames.pack_u16, -1) is not None and raises(frames.ProtocolError, frames.pack_u16, 65536) is not None, "and refused outside it")
    check(frames.pack_u32((1 << 32) - 1) == b"\xff\xff\xff\xff" and frames.pack_u32(-(1 << 31)) == b"\x00\x00\x00\x80" and raises(frames.ProtocolError, frames.pack_u32, -(1 << 31) - 1) is not None, "a thirty-two-bit register takes a signed or an unsigned value, over its whole range")
    check(frames.joint_fault(0x35) == 0x35 and frames.joint_fault(0xCA) == 0xCA and frames.joint_fault(0x80) == 0x80, "every bit of the error byte is a fault, the ones the e-manual names and the ones it does not")
    check(frames.describe_hw_error(0x35).count(",") == 3 and "bits 0xca" in frames.describe_hw_error(0xCA), "and the description names the bits the e-manual names and says how many it does not")




def main_open() -> None:
    section("open — what a configuration may not say")
    sim = new_sim()
    try:
        code, said = refused("not json")
        check(code == rhal.SKR_EINVAL and "not JSON" in said, "a configuration that is not JSON is EINVAL", said)
        code, said = refused("[1]")
        check(code == rhal.SKR_EINVAL and "not a JSON object" in said, "and so is one that is not an object", said)
        absent = object()
        for key, bad, word in (
                ("variant", absent, "`variant`"), ("variant", "medium", "`variant`"), ("variant", 3, "`variant`"),
                ("stop_mode", absent, "stop_mode"), ("stop_mode", "torque_off", "stop_mode"), ("stop_mode", "release", "stop_mode"),
                ("simulator", absent, "simulator"), ("simulator", "yes", "simulator"), ("simulator", 1, "simulator"),
                ("current_ma", absent, "current_ma"), ("current_ma", 0, "current_ma"), ("current_ma", -5, "current_ma"), ("current_ma", 551, "current_ma"), ("current_ma", True, "current_ma"), ("current_ma", 350.5, "current_ma"),
                ("gains", absent, "gains"), ("gains", {"kp": 600}, "gains"), ("gains", {"kp": 600, "ki": 0, "kd": 200, "kx": 1}, "gains"), ("gains", {"kp": -1, "ki": 0, "kd": 200}, "gains"),
                ("gains", {"kp": 20000, "ki": 0, "kd": 200}, "gains"), ("gains", {"kp": 600.5, "ki": 0, "kd": 200}, "gains"), ("gains", {"kp": True, "ki": 0, "kd": 200}, "gains"), ("gains", [600, 0, 200], "gains"),
                ("side_scale", 0, "side_scale"), ("side_scale", 1.5, "side_scale"), ("side_scale", "x", "side_scale"), ("side_scale", True, "side_scale"),
                ("rate_hz", 0.5, "rate_hz"), ("rate_hz", 500, "rate_hz"), ("rate_hz", True, "rate_hz"), ("timeout_ms", 1, "timeout_ms"), ("stale_ms", 5, "stale_ms"), ("open_timeout_ms", 10, "open_timeout_ms"),
                ("baud", 12345, "baud"), ("baud", True, "baud"), ("baud", "4000000", "baud")):
            if bad is absent:
                code, said = refused(json.dumps({k: v for k, v in settings(sim).items() if k != key}))
            else:
                code, said, bridge = opens_with(sim, **{key: bad})
                bridge.close()
            check(code == rhal.SKR_ENODEV and word in said, f"{key} of {'absent' if bad is absent else repr(bad)} is refused and says so", said)
        code, said = refused(json.dumps({**settings(sim), "frobnicate": 1}))
        check(code == rhal.SKR_ENODEV and "'frobnicate'" in said and "variant" in said, "a key the driver does not read is refused, naming it and the keys it does read", said)
        both = {**settings(sim), "host": "127.0.0.1", "port": 5}
        code, said = refused(json.dumps(both))
        check(code == rhal.SKR_ENODEV and "neither" in said, "a configuration that names a device and a gateway is refused", said)
        none = {k: v for k, v in settings(sim).items() if k not in ("device", "host", "port")}
        code, said = refused(json.dumps(none))
        check(code == rhal.SKR_ENODEV and "neither" in said, "and so is one that names neither", said)
        code, said = refused(json.dumps({**none, "host": "127.0.0.1"}))
        check(code == rhal.SKR_ENODEV and "port" in said, "a gateway with no port is refused", said)
        code, said = refused(json.dumps({**none, "host": "", "port": 5}))
        check(code == rhal.SKR_ENODEV and "do not name a gateway" in said, "and an empty host", said)
        code, said = refused(json.dumps({**none, "host": "127.0.0.1", "port": 70000}))
        check(code == rhal.SKR_ENODEV and "do not name a gateway" in said, "and a port past 65535", said)
        code, said = refused(json.dumps({**settings(sim), "device": ""}))
        check(code == rhal.SKR_ENODEV and "not a path" in said, "an empty device is refused", said)
        code, said, bridge = opens_with(sim, variant="lite", current_ma=300)
        check(code == rhal.SKR_ENODEV and "1240" in said and "1200" in said, "a Lite configured over a Full hand is refused, naming the model numbers of both", said)
        bridge.close()
        code, said, bridge = opens_with(sim, variant="lite", current_ma=351)
        check(code == rhal.SKR_ENODEV and "350" in said, "a Lite takes at most 350 mA here, and a Full 550", said)
        bridge.close()

        section("open — limits and roles")
        code, said, bridge = opens_with(sim, limits=[1, 2])
        check(code == rhal.SKR_ENODEV and "mapping" in said, "limits that are not a mapping from joints are refused", said)
        bridge.close()
        for limits, word in (({"wrist": [0, 1]}, "mapping"), ({"index_pip": [0.5]}, "two numbers"), ({"index_pip": [0.5, 0.1]}, "not a range"), ({"index_pip": [0.0, 2.5]}, "wider"), ({"index_pip": [-0.6, 1.0]}, "wider"),
                             ({"index_pip": ["a", 1]}, "two numbers"), ({"index_pip": [0, math.nan]}, "two numbers"), ({"index_pip": [True, 1]}, "two numbers")):
            code, said, bridge = opens_with(sim, limits=limits)
            check(code == rhal.SKR_ENODEV and word in said, f"limits of {limits} are refused: {word}", said)
            bridge.close()
        code, said, bridge = opens_with(sim, roles={"wrist": "x"})
        check(code == rhal.SKR_ENODEV and "roles" in said, "a role for a joint the hand does not have is refused", said)
        bridge.close()
        code, said, bridge = opens_with(sim, roles=[1])
        check(code == rhal.SKR_ENODEV and "roles" in said, "and roles that are not a mapping", said)
        bridge.close()
        code, said, bridge = opens_with(sim, limits={"index_pip": [-0.2, 1.0], "thumb_dip": [-1.34, 1.88]})
        check(code == rhal.SKR_OK, "a configuration may narrow a joint's range, and name its full range", said)
        descriptor = json.loads(bridge.describe())
        pip = descriptor["joints"][2]
        check(pip["limits"] == {"position_min_rad": -0.2, "position_max_rad": 1.0} and descriptor["joints"][15]["limits"] == {"position_min_rad": -1.34, "position_max_rad": 1.88}, "and the descriptor says the narrowed range")
        bridge.close()
    finally:
        sim.stop()

    section("open — the servos must answer, and be what the operator said")
    sim = new_sim()
    try:
        sim.silence(7)
        code, said, bridge = opens_with(sim, open_timeout_ms=600)
        check(code == rhal.SKR_EIO and "did not answer" in said and "7" in said, "a hand with one servo that does not answer is EIO and names it", said)
        bridge.close()
        sim.silence(7, False)
        sim.silent = True
        code, said, bridge = opens_with(sim, open_timeout_ms=600)
        check(code == rhal.SKR_EIO and "did not answer" in said, "a bus that carries nothing is EIO", said)
        bridge.close()
        sim.silent = False
        sim.noise = 50
        code, said, bridge = opens_with(sim, open_timeout_ms=600)
        check(code == rhal.SKR_EIO and ("none of them is the start of a packet" in said or "did not answer" in said), "a bus that answers with noise (the wrong baud rate) is EIO", said)
        bridge.close()
        sim.noise = 0
        sim.set_register(4, 0, (1030).to_bytes(2, "little"))
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_ENODEV and "[4]" in said and "1030" in said and "1240" in said, "a servo that is another model is refused by name, with its model number", said)
        bridge.close()
        sim.set_register(4, 0, (1240).to_bytes(2, "little"))
        sim.set_register(9, 38, (200).to_bytes(2, "little"))
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_ENODEV and "Current Limit" in said and "[9]" in said, "a `current_ma` above a servo's own Current Limit is refused, naming the servo", said)
        bridge.close()
        sim.set_register(9, 38, (2352).to_bytes(2, "little"))
        sim.requests.clear()
        sim.fault(3, 0x20)
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_ENODEV and "overload" in said and "clear_errors" in said and "servo 3" in said, "a hand with a hardware error is refused, naming the servo, the error and the way out", said)
        bridge.close()
        check(not any(i in (frames.INST_SYNC_WRITE, frames.INST_WRITE) for i, *_ in sim.requests), "and nothing was written to it")
        sim.clear_faults()
    finally:
        sim.stop()

    section("open — bringing the hand up without moving it")
    start = tuple(reg(a) for a in (0.1, 0.2, 0.3, 0.4, 0.0, 0.1, 0.2, 0.3, -0.2, 0.0, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0))
    sim = new_sim(start_registers=start)
    try:
        before = sim.positions
        bridge = opened(sim)
        check(sim.torque == [1] * 16, "a limp hand has its torque switched on")
        check(sim.positions == before and sim.goals == before, "and its goals are where its servos are, so that nothing moved")
        check(all(address >= 64 for _, _, address, _ in sim.writes), "nothing in the EEPROM was written to a hand that was already in mode 5")
        expected = [(round(KD * (SIDE if j in (0, 4, 8) else 1)), KI, round(KP * (SIDE if j in (0, 4, 8) else 1))) for j in range(16)]
        check(sim.gains == expected, f"every servo has the operator's gains (D, I, P), the three side-to-side MCPs at {SIDE} of P and D ({sim.gains[0]}, {sim.gains[1]})")
        check(sim.goal_currents == [CURRENT] * 16, "and the operator's goal current")
        check(ready(bridge), "the hand is read")
        rc, out = state(bridge)
        check(all(abs(out.position_rad[j] - frames.register_to_position(start[j])) < 1e-9 for j in range(16)), "and its positions are the servos' own, in the hand's convention")
        check(all(math.isnan(v) for v in out.effort_nm), "no effort is reported: the servos report a current, and no torque constant is known here")
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(mode=3, torque=False)
    try:
        bridge = opened(sim)
        check(sim.operating_modes == [5] * 16, "a limp hand in another operating mode is put in current-based position control")
        check(any(address == 11 for _, _, address, _ in sim.writes), "which is the one EEPROM write this driver makes, and only while torque is off")
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(mode=3, torque=True)
    try:
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_ENODEV and "operating mode" in said and "release" in said, "a hand that holds torque in another mode is refused: the EEPROM is locked, and letting go is a person's", said)
        bridge.close()
        check(not sim.writes, "and nothing was written to it")
    finally:
        sim.stop()
    sim = new_sim(torque=True, start_registers=start)
    try:
        sim.set_register(2, 116, (reg(0.9)).to_bytes(4, "little", signed=True))
        goals = sim.goals
        bridge = opened(sim)
        check(sim.goals == goals and not sim.goal_writes, "a hand that holds torque keeps its goals: opening it does not move it or take over what it was told")
        check(list(bridge._image) == goals, "and the driver's image of what it was told is the hand's own goals")  # noqa: SLF001
        check(sim.goal_currents == [CURRENT] * 16 and sim.gains[1] != (150, 0, 450) or sim.gains[1] == (round(KD), KI, round(KP)), "but it is told the operator's gains and goal current")
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(len(bridge._firmware) == 16 and len(bridge._current_limit) == 16 and bridge._current_limit[0] == 2352, "the firmware and the Current Limit of every servo are read once, for the record")  # noqa: SLF001
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim("lite")
    try:
        code, said, bridge = opens_with(sim, variant="lite", current_ma=300)
        check(code == rhal.SKR_OK, "a Lite hand is opened as a Lite", said)
        check(json.loads(bridge.describe())["robot_slug"] == "leap-hand-lite", "and its descriptor says so")
        bridge.close()
    finally:
        sim.stop()


def main_state() -> None:
    section("read_state — positions, flags and faults")
    start = tuple(reg(a) for a in (0.1, 0.2, 0.3, 0.4, 0.0, 0.1, 0.2, 0.3, -0.2, 0.0, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0))
    sim = new_sim(start_registers=start)
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        rc, out = state(bridge)
        check(rc == rhal.SKR_OK and out.flags & rhal.HOMING_MASK == rhal.HOMING_NOT_REQUIRED and not out.flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP | rhal.FLAG_DEGRADED) and out.fault_code == 0, "a healthy hand: OK, no homing, no fault, no stop, not degraded")
        check(out.seq >= 1 and out.stamp_ns > 0 and len(out.position_rad) == 16 and len(out.velocity_rad_s) == 16, "with a sequence number, a stamp and sixteen of each")
        sim.place([reg(0.5)] * 16)
        check(until(lambda: all(abs(p - 0.5) < 2e-3 for p in state(bridge)[1].position_rad), 3), "positions follow the servos")
        check(until(lambda: all(v == 0.0 for v in state(bridge)[1].velocity_rad_s), 3), "and a servo at rest has no velocity")
        bridge.close()
        rc, out = state(bridge)
        check(rc == rhal.SKR_EPERM, "a state read after a close is EPERM")
        check(state(LeapBridge())[0] == rhal.SKR_EPERM, "and from a driver that was never opened")

        slow = new_sim(time_scale=0.2)
        try:
            bridge = opened(slow)
            check(ready(bridge), "ready")
            write(bridge, cmd(pose(1.0)))
            check(until(lambda: any(v > 0 for v in state(bridge)[1].velocity_rad_s), 3), "a moving servo reports a positive velocity (closing is positive)")
            check(until(lambda: any(i > 0 for i in bridge._latest.current), 3), "and a current")  # noqa: SLF001
            bridge.close()
        finally:
            slow.stop()

        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.fault(2, 0x04)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "a hardware error is a fault")
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_DEGRADED and out.fault_code == (2 + 1) << 16 | 0x04, "and degraded, with the servo's number and its error bits in the fault word")
        check(sim.torque[2] == 0, "(the servo's own shutdown switched its torque off)")
        sim.fault(9, 0x20)
        sim.fault(1, 0x01)
        check(until(lambda: state(bridge)[1].fault_code == (1 + 1) << 16 | 0x01, 3), "with several, the lowest-numbered servo's is the fault word")
        sim.clear_faults()
        for j in (2, 9, 1):
            sim.set_register(j, 64, b"\x01")
        check(until(lambda: not state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "and it clears when the servos' errors are gone")
        sim.fault(5, 0x02)
        check(until(lambda: state(bridge)[1].fault_code == (5 + 1) << 16 | 0x02, 3), "a bit the e-manual does not name is a fault too")
        check(sim.torque[5] == 1, "(the shutdown register does not act on it, so the servo holds)")
        sim.clear_faults()
        check(until(lambda: not state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "and clears")
        sim.set_register(6, 64, b"\x00")
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3) and not state(bridge)[1].flags & rhal.FLAG_FAULT, "a servo with torque off and no error is degraded and not a fault")
        sim.set_register(6, 64, b"\x01")
        check(ready(bridge), "and clears when torque is back")
        bridge.close()
    finally:
        sim.stop()

    section("read_state — a driver with no reading yet, a reading that is old, a line that is dead")
    sim = new_sim()
    try:
        bridge = opened(sim, rate_hz=1)
        check(ready(bridge), "ready")
        with bridge._lock:  # noqa: SLF001
            bridge._latest = None  # noqa: SLF001
        rc, out = state(bridge)
        check(rc == rhal.SKR_ETIMEDOUT and out.flags & rhal.FLAG_DEGRADED, "a state with no reading is ETIMEDOUT and DEGRADED")
        code, said = write(bridge, cmd(pose(0.0)))
        check(code == rhal.SKR_ETIMEDOUT and "not been read" in said, "a command is refused until the hand has been read", said)
        code, text = ext(bridge, "status")
        check(code == rhal.SKR_ETIMEDOUT and "not been read" in str(text), "and so is a status")
        bridge.close()
        bridge = opened(sim, stale_ms=300)
        check(ready(bridge), "ready")
        sim.silent = True
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 3), "a hand that goes quiet is ETIMEDOUT")
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_FAULT, "and degraded, and not a fault")
        check(until(lambda: bridge._dead is not None, 3), "after three failed cycles the line is called dead")  # noqa: SLF001
        code, text = ext(bridge, "status")
        check(code == rhal.SKR_OK and text["link_dead"] and "did not answer" in text["link_dead"], "and status says why")
        sim.silent = False
        check(ready(bridge, 6), "and it is read again when the line is back")
        bridge.close()
    finally:
        sim.stop()


def main_commands() -> None:
    section("write_command — a command is checked whole, and only what it applies is written")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.goal_writes.clear()
        target = [lo + (hi - lo) * (j + 1) / 18 for j, (lo, hi) in enumerate(zip(frames.LIMITS_MIN, frames.LIMITS_MAX))]
        code, said = write(bridge, cmd(target))
        check(code == rhal.SKR_OK, "a command in range is accepted", said)
        check(until(lambda: sim.goals == [reg(v) for v in target], 3), "and every servo's goal is the angle in ticks, rounded to the nearest")
        check(len(sim.goal_writes) == 1 and set(sim.goal_writes[0][1]) == set(range(16)), "in one synchronised write")
        check(list(bridge._image) == [reg(v) for v in target], "and the driver's image of what it told the hand is those goals")  # noqa: SLF001
        sim.goal_writes.clear()
        mask = [1 if j in (2, 5) else 0 for j in range(16)]
        code, said = write(bridge, cmd(pose(0.3), mask))
        check(code == rhal.SKR_OK and until(lambda: sim.goals[2] == reg(0.3) and sim.goals[5] == reg(0.3), 3), "a masked command moves the joints it applies")
        check(len(sim.goal_writes) == 1 and set(sim.goal_writes[0][1]) == {2, 5}, "and writes those two servos' goals and no others")
        check(all(sim.goals[j] == reg(target[j]) for j in range(16) if j not in (2, 5)), "so the rest keep the goals they had")
        sim.goal_writes.clear()
        code, said = write(bridge, cmd([math.nan] * 16, [0] * 16))
        check(code == rhal.SKR_OK and not sim.goal_writes, "a command that applies nothing is OK and writes nothing, whatever it holds")
        masked_nan = pose(0.2)
        masked_nan[7] = math.nan
        code, said = write(bridge, cmd(masked_nan, [1 if j != 7 else 0 for j in range(16)]))
        check(code == rhal.SKR_OK and until(lambda: bool(sim.goal_writes), 3) and 7 not in sim.goal_writes[0][1], "a NaN in a joint the command does not apply is not looked at")
        sim.goal_writes.clear()
        for what, command, word in (
                ("a command of fifteen joints", cmd(pose(0.0)[:15]), "sixteen"),
                ("a command of seventeen", cmd(pose(0.0) + [0.0]), "sixteen"),
                ("a velocity command", cmd(pose(0.0), mode=rhal.MODE_VELOCITY), "positions"),
                ("a NaN", cmd([0.0] * 5 + [math.nan] + [0.0] * 10), "not finite"),
                ("an infinity", cmd([0.0] * 5 + [math.inf] + [0.0] * 10), "not finite"),
                ("a joint above its range", cmd([0.0] * 2 + [1.9] + [0.0] * 13), "outside its range"),
                ("a joint below its range", cmd([0.0] * 14 + [-1.21, 0.0]), "outside its range"),
                ("the side-to-side MCP past 1.047", cmd([1.06] + [0.0] * 15), "outside its range")):
            code, said = write(bridge, command)
            check(code == rhal.SKR_EINVAL and word in said, f"{what} is EINVAL and says why", said)
        check(not sim.goal_writes, "and none of them wrote anything, a command refused for one joint being refused whole")
        nan_command = cmd([0.0] * 5 + [math.nan] + [0.0] * 10)
        first = bridge.write_command(nan_command, 0)
        again, said = write(bridge, nan_command)
        bridge._last_refusal = None  # noqa: SLF001
        buffer = io.StringIO()
        with contextlib.redirect_stderr(buffer):
            bridge.write_command(nan_command, 0)
            bridge.write_command(nan_command, 0)
        check(first == rhal.SKR_EINVAL and buffer.getvalue().count("not finite") == 1, "a refusal is said once for a run of the same cause")
        edge = list(frames.LIMITS_MIN)
        code, said = write(bridge, cmd(edge))
        check(code == rhal.SKR_OK and until(lambda: sim.goals == [reg(v) for v in edge], 3), "every joint at the bottom of its range is accepted")
        edge = list(frames.LIMITS_MAX)
        code, said = write(bridge, cmd(edge))
        check(code == rhal.SKR_OK and until(lambda: sim.goals == [reg(v) for v in edge], 3), "and at the top")
        code, said = write(bridge, cmd([frames.LIMITS_MAX[0] + 5e-10] + [0.0] * 15))
        check(code == rhal.SKR_OK and until(lambda: sim.goals[0] == reg(frames.LIMITS_MAX[0]), 3), "a hair past the end is the end")
        bridge.close()
        check(write(bridge, cmd(pose(0.0)))[0] == rhal.SKR_EPERM, "a command to a driver that is closed is EPERM")
    finally:
        sim.stop()

    section("write_command — narrowed limits, a hand that holds a fault, a limp servo, a hand that acknowledges and does not take")
    sim = new_sim()
    try:
        bridge = opened(sim, limits={"index_pip": [-0.1, 0.6]})
        check(ready(bridge), "ready")
        code, said = write(bridge, cmd([0.0, 0.0, 0.7] + [0.0] * 13))
        check(code == rhal.SKR_EINVAL and "[-0.100000, 0.600000]" in said, "a joint past the operator's narrower range is refused, naming that range", said)
        check(write(bridge, cmd([0.0, 0.0, 0.55] + [0.0] * 13))[0] == rhal.SKR_OK, "and one inside it is not")
        bridge.close()

        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.fault(4, 0x04)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "a fault")
        sim.goal_writes.clear()
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EPERM and "servo 4" in said and "overheating" in said and "clear_errors" in said, "a command to a hand that holds a fault is EPERM, naming the servo, the error and the way out", said)
        check(not sim.goal_writes, "and writes nothing")
        sim.clear_faults()
        sim.set_register(4, 64, b"\x01")
        check(until(lambda: write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, 3), "and commands again when it is cleared")
        sim.set_register(11, 64, b"\x00")
        check(until(lambda: bridge._latest is not None and not bridge._latest.torque[11], 3), "a servo is limp")  # noqa: SLF001
        sim.goal_writes.clear()
        code, said = write(bridge, cmd(pose(0.2)))
        check(code == rhal.SKR_EPERM and "ring_dip" in said and "enable_torque" in said, "a command that applies a limp servo's joint is EPERM, naming the joint and the way out", said)
        code, said = write(bridge, cmd(pose(0.2), [1 if j != 11 else 0 for j in range(16)]))
        check(code == rhal.SKR_OK, "but one that does not apply it is taken")
        sim.set_register(11, 64, b"\x01")
        bridge.close()

        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.deaf = True
        write(bridge, cmd(pose(0.3)))
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 4), "a goal the servos acknowledged and did not take, longer than the grace, is DEGRADED (F175)")
        check(not state(bridge)[1].flags & rhal.FLAG_FAULT, "and not a fault")
        sim.deaf = False
        write(bridge, cmd(pose(0.3)))
        check(until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 4), "and it clears when the servos take the goal")
        bridge.close()
    finally:
        sim.stop()


def expected_gains(servo: int) -> tuple[int, int, int]:
    """(D, I, P) of a servo as the sim shows them."""
    scale = SIDE if servo in (0, 4, 8) else 1.0
    return round(KD * scale), KI, round(KP * scale)


def main_gains() -> None:
    section("gains and goal current — a hand that was told something and forgot it")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.set_register(0, 84, (1000).to_bytes(2, "little"))
        check(until(lambda: sim.gains[0] == expected_gains(0), 3), "a servo whose P gain was changed behind the driver's back is told the operator's again")
        sim.set_register(5, 102, (200).to_bytes(2, "little", signed=True))
        check(until(lambda: sim.goal_currents[5] == CURRENT, 3), "and so is one whose goal current was")
        check(until(lambda: ext(bridge, "status")[1]["parameters_reasserted"] >= 2, 3), "and the driver counts how often it had to")
        check(ready(bridge), "and the hand is not degraded once it has been told")

        sim.power_cycle()
        check(until(lambda: sim.torque == [0] * 16 and state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3), "a hand that lost its power is limp, and degraded")
        check(until(lambda: sim.gains == [expected_gains(j) for j in range(16)] and sim.goal_currents == [CURRENT] * 16, 4), "the operator's gains and goal current are told again to servos that came back with the defaults")
        check(sim.torque == [0] * 16, "but torque is not switched on by the driver: a limp hand is a person's to decide about")
        code, said = write(bridge, cmd(pose(0.2)))
        check(code == rhal.SKR_EPERM and "enable_torque" in said, "and a command to it is EPERM, naming the way out", said)
        sim.goal_writes.clear()
        code, body = ext(bridge, "enable_torque")
        check(code == rhal.SKR_OK and body["result"] == "enabled" and len(body["enabled"]) == 16, "`enable_torque` switches it on", str(body))
        check(sim.torque == [1] * 16 and sim.goals == sim.positions, "with every goal where its servo is, so that nothing moved")
        check(ready(bridge, 4), "and the hand is read as healthy again")
        check(write(bridge, cmd(pose(0.2)))[0] == rhal.SKR_OK, "and commanded")

        sim.silent = True
        check(until(lambda: bridge._dead is not None, 4), "a line that goes dead")  # noqa: SLF001
        sim.silent = False
        sim.power_cycle()
        check(until(lambda: sim.gains == [expected_gains(j) for j in range(16)], 5), "is followed by the gains told again, the hand having been power-cycled behind it")
        bridge.close()

        # A command that arrives before the reading thread has told a forgetful hand its gains tells it first.
        with mock.patch.object(LeapBridge, "_maintain", lambda self, reading: None):
            untold = new_sim()
            try:
                bridge = opened(untold)
                check(ready(bridge), "ready")
                untold.set_register(3, 84, (77).to_bytes(2, "little"))
                with bridge._lock:  # noqa: SLF001
                    bridge._params_dirty = True  # noqa: SLF001
                untold.writes.clear()
                code, said = write(bridge, cmd(pose(0.3)))
                check(code == rhal.SKR_OK, "a command to a hand whose parameters are owed is taken", said)
                check(until(lambda: untold.goals[3] == reg(0.3), 3), "and reaches the servos")
                written = [address for _, servo, address, _ in untold.writes if servo == 3]
                check(untold.gains[3] == expected_gains(3) and written and written[-1] == frames.ADDR_GOAL_POSITION and frames.ADDR_POSITION_P in written, "and the gains are told before the goal, whoever else is about to (the command tells them itself)")
                bridge.close()
            finally:
                untold.stop()
    finally:
        sim.stop()


def main_stop() -> None:
    section("the stop — latched, written by the reading thread, a hold where the hand is")
    sim = new_sim(time_scale=0.1)
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        start = sim.positions
        write(bridge, cmd(pose(1.0)))
        check(until(lambda: all(p > s + 20 for p, s in zip(sim.positions, start)), 4), "the servos are on their way")
        began = time.monotonic()
        check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.04, "emergency_stop returns OK at once")
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_ESTOP, "and the state says the stop is latched")
        code, said = write(bridge, cmd(pose(0.5)))
        check(code == rhal.SKR_EPERM, "a command while latched is EPERM")
        code, said = write(bridge, cmd([math.nan] * 16))
        check(code == rhal.SKR_EPERM, "whatever it holds (a latched stop is the first answer)")
        code, said = write(bridge, cmd(pose(9.0)))
        check(code == rhal.SKR_EPERM, "even one that would be refused for its values")
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3), "the hold is written by the reading thread")
        status = ext(bridge, "status")[1]
        check(status["stop_hold"] == sim.goals and status["image"] == status["stop_hold"], "the status says the hold, and the image is the hold")
        check(all(abs(h - p) <= 12 for h, p in zip(sim.goals, sim.positions)), "which is where the servos are, each its own")
        check(all(g != reg(1.0) for g in sim.goals) and sim.torque == [1] * 16, "and not the goals they were on their way to; torque stays on")
        code, text = ext(bridge, "move index_pip=0.1")
        check(code == rhal.SKR_EPERM and "stop is latched" in str(text), "motions are refused while latched, and say why")
        check(ext(bridge, "status")[0] == rhal.SKR_OK, "but `status` is answered")
        sim.goal_writes.clear()
        bridge.emergency_stop()
        check(until(lambda: len(sim.goal_writes) >= 1, 3), "a second stop writes the hold again")
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3), "and is delivered")
        check(bridge.release_stop() == rhal.SKR_OK, "released")
        rc, out = state(bridge)
        check(not out.flags & rhal.FLAG_ESTOP, "the latch is gone")
        status = ext(bridge, "status")[1]
        check(status["stop_hold"] is None and status["stop_delivered"] is False, "and a released stop leaves no hold and nothing delivered")
        check(list(bridge._image) == sim.goals, "and the image of what the hand was told is the hold")  # noqa: SLF001
        check(until(lambda: write(bridge, cmd(pose(0.4)))[0] == rhal.SKR_OK, 3), "and commands are taken again")
        bridge.close()

        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.silent = True
        began = time.monotonic()
        check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.04, "a stop with the line down still returns OK within its budget: the latch is the stop (F298)")
        status = ext(bridge, "status")[1]
        check(status["stop_delivered"] is False and status["stop_hold"] is None, "and a stop that has not been written does not show a hold")
        check(write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_EPERM, "and commands are refused")
        sim.goal_writes.clear()
        sim.silent = False
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 5), "the hold goes out as soon as the line carries it")
        check(bridge.release_stop() == rhal.SKR_OK, "and then the stop can be released")
        bridge.close()

        slow_hand = new_sim(time_scale=0.1)
        bridge = opened(slow_hand)
        check(ready(bridge), "ready")
        write(bridge, cmd(pose(1.0)))
        check(until(lambda: slow_hand.positions[2] > reg(0.0) + 20, 4), "the servos are on their way")
        slow_hand.deaf = True
        bridge.emergency_stop()
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3), "the hold is written")
        code, said = said_by(bridge.release_stop)
        check(code == rhal.SKR_EIO and "stays latched" in said and bridge._estop, "and a hand that acknowledged it and did not take it (its own record of its goals is not the hold) is not released", said)  # noqa: SLF001
        slow_hand.deaf = False
        bridge.emergency_stop()
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3) and bridge.release_stop() == rhal.SKR_OK, "a stop called again is written, taken and released")
        bridge.close()
        slow_hand.stop()

        # A stop called while an earlier one is being written is owed a hold read after it: the delivery in flight began before the call and must not stand for it. The second call is made from inside the first
        # one's write, so the order does not depend on how late a loaded machine runs the reading thread.
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.goal_writes.clear()
        real, fired = bridge._link.transact, []  # noqa: SLF001

        def stop_again_while_writing(request, ids, budget):
            answer = real(request, ids, budget)
            if request[7] == frames.INST_SYNC_WRITE and not fired:
                fired.append(True)
                bridge.emergency_stop()
            return answer

        bridge._link.transact = stop_again_while_writing  # noqa: SLF001
        bridge.emergency_stop()
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3) and fired, "a stop called while an earlier one is being written is delivered")
        check(len(sim.goal_writes) == 2, f"and it is written again, from positions read after the second call, not taken as the first one's ({len(sim.goal_writes)} writes)")
        bridge.close()

        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.report_positions = [1 << 30] * 16
        sim.goal_writes.clear()
        bridge.emergency_stop()
        time.sleep(0.4)
        check(not sim.goal_writes, "a hold is never written from positions that are not positions")
        sim.report_positions = None
        bridge.close()
    finally:
        sim.stop()

    section("the stop — a command that is waiting while it is called does not follow it")
    sim = new_sim(time_scale=0.05)
    try:
        for stopped in (False, True):
            bridge = opened(sim, timeout_ms=300)
            check(ready(bridge), "ready")
            sim.goal_writes.clear()
            owner = holding_the_line(bridge, 0.7)
            results: list[int] = []
            began = time.monotonic()
            sender = threading.Thread(target=lambda: results.append(write(bridge, cmd(pose(0.9)))[0]))
            sender.start()
            time.sleep(0.05)
            if stopped:
                bridge.emergency_stop()
            sender.join(3.0)
            took = time.monotonic() - began
            owner.join()
            check(results == [rhal.SKR_EPERM if stopped else rhal.SKR_EIO], f"a command that ran out of time waiting for a busy line is {'refused by the stop that was called while it waited' if stopped else 'a slow line'} ({results})")
            if not stopped:
                check(0.2 <= took <= 0.45, f"and it took its quarter of a second, not the time the line was held ({took:.2f} s)")
            if stopped:
                check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3), "the stop is written once the line is free")
                check(all(g != reg(0.9) for g in sim.goals), "and the command's goals were never written")
            bridge.close()

    finally:
        sim.stop()

    section("a close does not stop the hand")
    slow = new_sim(time_scale=0.1)
    try:
        bridge = opened(slow)
        check(ready(bridge), "ready")
        write(bridge, cmd(pose(1.0)))
        check(until(lambda: slow.positions[2] > reg(0.0) + 30, 4), "the servos are moving")
        bridge.close()
        at_close = slow.positions[2]
        check(until(lambda: slow.positions[2] > at_close + 20, 4), "and go on moving after a close, as servos that are told nothing do")
        check(slow.torque == [1] * 16, "with their torque on")
    finally:
        slow.stop()


def main_motions() -> None:
    section("extension — the request itself")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, body = bridge.extension("vendor.other.task", b"status")
        check(code == rhal.SKR_ENOSYS and b"vendor.leap.task" in body, "another vendor's token is ENOSYS and names the one this driver serves")
        code, body = bridge.extension(TOKEN, b"\xff\xfe")
        check(code == rhal.SKR_EINVAL and b"UTF-8" in body, "a request that is not UTF-8 is EINVAL")
        code, body = bridge.extension(TOKEN, b"   ")
        check(code == rhal.SKR_EINVAL and b"empty" in body, "an empty one is EINVAL and lists the motions")
        code, body = bridge.extension(TOKEN, b"dance")
        check(code == rhal.SKR_EINVAL and b"unknown motion" in body, "an unknown motion is EINVAL")
        code, body = bridge.extension(TOKEN, b"status loud")
        check(code == rhal.SKR_EINVAL and b"malformed" in body, "an argument that is not k=v is EINVAL")
        code, body = bridge.extension(TOKEN, b"status loud=1")
        check(code == rhal.SKR_EINVAL and b"does not take" in body, "and `status` takes none")
        code, body = bridge.extension(TOKEN, b"enable_torque loud=1")
        check(code == rhal.SKR_EINVAL and b"does not take" in body, "nor does `enable_torque`")
        code, status = ext(bridge, "status")
        check(code == rhal.SKR_OK and status["variant"] == "full" and status["servo"] == "XC330-M288-T" and status["simulator"] is True and len(status["position"]) == 16, "`status` names the variant, the servos, that it is a simulation, and reports sixteen of everything")
        check(status["torque"] == [1] * 16 and status["hardware_error"] == ["0x00"] * 16 and status["moving"] == [0] * 16 and status["goal"] == status["position"], "the torque, errors, motion and goals")
        check(status["goal_current_ma"] == [CURRENT] * 16 and status["gains_p"][1] == KP and status["gains_p"][0] == round(KP * SIDE) and status["gains_d"][0] == round(KD * SIDE), "the gains and the goal current the servos hold")
        check(status["voltage_v"] == [5.0] * 16 and status["temperature_c"] == [30] * 16, "and the supply and temperature, read for the purpose")
        check(status["gains_configured"] == {"kp": KP, "ki": KI, "kd": KD, "side_scale": SIDE} and status["current_ma_configured"] == CURRENT, "and what the operator asked")
        sim.mute_reads = True
        code, status = ext(bridge, "status")
        check(code == rhal.SKR_OK and "read_error" in status and "voltage_v" not in status, "a status when the servos do not answer the extra reads still answers, and says what failed")
        sim.mute_reads = False
        bridge.close()
        code, body = bridge.extension(TOKEN, b"status")
        check(code == rhal.SKR_EPERM and b"not open" in body, "a request to a driver that is not open is EPERM")
    finally:
        sim.stop()

    section("extension — move")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, body = ext(bridge, "move index_pip=0.5 middle_pip=0.6 timeout=5")
        check(code == rhal.SKR_OK and body["result"] == "arrived" and set(body["joints"]) == {"index_pip", "middle_pip"}, "a move of two joints waits for both and says they arrived", str(body))
        check(abs(body["joints"]["index_pip"]["position"] - reg(0.5)) <= bridge_module._ARRIVAL_TOLERANCE, "at the position the servo reports")  # noqa: SLF001
        check(sim.positions[0] == reg(0.0) and sim.positions[5] == reg(0.0), "and the others were not moved")
        code, body = ext(bridge, "move index_pip=0.5")
        check(code == rhal.SKR_OK and body["result"] == "arrived", "a move to where the finger already is has arrived")
        sim.blocks[7] = reg(0.3)
        code, body = ext(bridge, "move middle_dip=1.0 timeout=5")
        check(code == rhal.SKR_OK and body["result"] == "stopped_on_an_object" and abs(body["joints"]["middle_dip"]["position"] - reg(0.3)) <= 2, "a finger that meets an object stops, and the move says it stopped on one", str(body))
        check(body["joints"]["middle_dip"]["current_ma"] >= CURRENT - 1, "with the motor at its goal current")
        sim.blocks[7] = None
        code, body = ext(bridge, "move middle_dip=0.0 timeout=5")
        check(code == rhal.SKR_OK and body["result"] == "arrived", "and it opens again from the object")
        for text, word in (("move", "at least one joint"), ("move wrist=0.1", "not a joint"), ("move index_pip=abc", "not a number"), ("move index_pip=nan", "outside"), ("move index_pip=3.0", "outside"),
                           ("move index_pip=0.1 timeout=0.1", "outside"), ("move index_pip=0.1 timeout=500", "outside"), ("move index_pip=0.1 timeout=x", "not a number")):
            code, body = ext(bridge, text)
            check(code == rhal.SKR_EINVAL and word in str(body), f"`{text}` is EINVAL: {word}", str(body))
        sim.set_register(6, 64, b"\x00")
        check(until(lambda: not bridge._latest.torque[6], 3), "a servo is limp")  # noqa: SLF001
        code, body = ext(bridge, "move middle_pip=0.1")
        check(code == rhal.SKR_EPERM and "enable_torque" in str(body), "a move of a limp servo's joint is EPERM, naming the way out", str(body))
        sim.set_register(6, 64, b"\x01")
        sim.fault(1, 0x20)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "a fault")
        code, body = ext(bridge, "move index_pip=0.1")
        check(code == rhal.SKR_EPERM and "overload" in str(body), "a move on a hand that holds one is EPERM", str(body))
        sim.clear_faults()
        sim.set_register(1, 64, b"\x01")
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(time_scale=0.05)
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, body = ext(bridge, "move index_pip=1.5 timeout=0.6")
        check(code == rhal.SKR_EIO and "no reading showed" in str(body) and "never started" not in str(body), "a move that does not arrive in its timeout is EIO and says so", str(body))
        sim.deaf = True
        code, body = ext(bridge, "move ring_pip=0.5 timeout=0.6")
        check(code == rhal.SKR_EIO and "never started" in str(body) and "ring_pip" in str(body), "and one the servos acknowledge and do not take is EIO, and names the joint that never started", str(body))
        sim.deaf = False
        write(bridge, cmd(pose(0.0)))
        results: list[tuple[int, object]] = []
        mover = threading.Thread(target=lambda: results.append(ext(bridge, "move index_pip=1.5 timeout=30")))
        mover.start()
        check(until(lambda: sim.positions[2] > reg(0.0) + 20, 4), "a move is under way")
        began = time.monotonic()
        bridge.emergency_stop()
        mover.join(3.0)
        check(not mover.is_alive() and time.monotonic() - began < 1.5 and results and results[0][0] == rhal.SKR_EPERM and "stop was latched" in str(results[0][1]), "a stop ends a move that is waiting, and the move says why")
        bridge.release_stop()
        write(bridge, cmd(pose(0.0)))
        results.clear()
        mover = threading.Thread(target=lambda: results.append(ext(bridge, "move ring_pip=1.5 timeout=30")))
        mover.start()
        check(until(lambda: sim.positions[10] > reg(0.0) + 20, 4), "another is under way")
        sim.fault(10, 0x20)
        mover.join(5.0)
        check(not mover.is_alive() and results and results[0][0] == rhal.SKR_EIO and "overload" in str(results[0][1]), "a fault during a move ends it, EIO, and says what")
        sim.clear_faults()
        bridge.close()
        code, body = ext(bridge, "move index_pip=1.5 timeout=30")
        check(code == rhal.SKR_EPERM, "a move on a driver that has been closed is refused at once")
    finally:
        sim.stop()

    section("extension — enable_torque and clear_errors")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, body = ext(bridge, "enable_torque")
        check(code == rhal.SKR_OK and body == {"result": "already_on", "enabled": []}, "`enable_torque` on a hand that holds torque does nothing")
        for j in (3, 4):
            sim.set_register(j, 64, b"\x00")
        sim.place([reg(0.4)] * 16, retarget=False)
        check(until(lambda: bridge._latest is not None and not bridge._latest.torque[3] and not bridge._latest.torque[4] and bridge._latest.position[3] == reg(0.4), 3), "two servos are limp, and the reading shows where they are")  # noqa: SLF001
        sim.goal_writes.clear()
        code, body = ext(bridge, "enable_torque")
        check(code == rhal.SKR_OK and body["result"] == "enabled" and body["enabled"] == [JOINTS[3], JOINTS[4]], "`enable_torque` switches on the ones that are limp and names them", str(body))
        check(sim.torque == [1] * 16 and sim.goals[3] == sim.positions[3] == reg(0.4) and sim.goals[4] == sim.positions[4], "each with its goal where it is, written before its torque went on")
        code, body = ext(bridge, "clear_errors")
        check(code == rhal.SKR_EINVAL and "confirm=limp" in str(body), "`clear_errors` wants a person's word that a rebooted servo goes limp")
        code, body = ext(bridge, "clear_errors confirm=yes")
        check(code == rhal.SKR_EINVAL and "confirm=limp" in str(body), "and `limp` is the word")
        code, body = ext(bridge, "clear_errors confirm=limp")
        check(code == rhal.SKR_OK and body == {"result": "nothing_to_clear", "rebooted": []}, "with nothing to clear it reboots nothing")
        sim.fault(5, 0x20)
        sim.fault(8, 0x04)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "two faults")
        code, body = ext(bridge, "enable_torque")
        check(code == rhal.SKR_EPERM and "overload" in str(body), "`enable_torque` does not switch torque on over a hardware error")
        sim.requests.clear()
        code, body = ext(bridge, "clear_errors confirm=limp")
        check(code == rhal.SKR_OK and body["result"] == "cleared" and body["rebooted"] == [JOINTS[5], JOINTS[8]], "`clear_errors` reboots the servos that hold an error and names them", str(body))
        check([ids for i, *_, ids in sim.requests if i == frames.INST_REBOOT] == [(5,), (8,)], "and only those")
        check(sim.hw_errors == [0] * 16 and sim.torque[5] == 0 and sim.torque[8] == 0, "their errors are gone and they are limp")
        check(sim.gains == [expected_gains(j) for j in range(16)], "and the operator's gains are told to the servos that came back with the defaults")
        code, body = ext(bridge, "enable_torque")
        check(code == rhal.SKR_OK and body["enabled"] == [JOINTS[5], JOINTS[8]] and sim.torque == [1] * 16, "`enable_torque` then holds them where they are")
        sim.persist(5, 0x04)
        sim.fault(5, 0x04)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "a servo whose cause is still there")
        code, body = ext(bridge, "clear_errors confirm=limp")
        check(code == rhal.SKR_OK and body["result"] == "not_cleared" and "overheating" in body["remaining"][0], "is rebooted and still holds its error: `not_cleared`, saying what", str(body))
        bridge.close()
    finally:
        sim.stop()


def scripted_link(replies: list[bytes], baud: int = 115200) -> tuple[link.Link, threading.Thread]:
    """A Link on a pseudo-terminal whose far end answers its first request with `replies`, one write each."""
    master, slave = os.openpty()
    path = os.ttyname(slave)
    attributes = termios.tcgetattr(slave)
    attributes[0] = attributes[1] = attributes[3] = 0
    attributes[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
    attributes[6][termios.VMIN] = 0
    attributes[6][termios.VTIME] = 0
    termios.tcsetattr(slave, termios.TCSANOW, attributes)

    def answer() -> None:
        import select as selector
        if selector.select([master], [], [], 3.0)[0]:
            os.read(master, 1024)
            for reply in replies:
                os.write(master, reply)
                time.sleep(0.005)
        time.sleep(0.4)
        os.close(master)
        os.close(slave)

    lk = link.Link(device=path, baud=baud)
    lk.open()
    thread = threading.Thread(target=answer, daemon=True)
    thread.start()
    return lk, thread


def status_of(servo: int, data: bytes = b"", error: int = 0) -> bytes:
    return frames.packet(servo, frames.INST_STATUS, bytes([error]) + data)


def main_line() -> None:
    section("the line — answers that are late, in pieces, wrong, or not there")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        for gap_at in (1, 3, 5, 7, 9, 12):
            sim.split_gap_s, sim.split_at = 0.01, gap_at
            before = bridge._exchanges  # noqa: SLF001
            check(until(lambda: bridge._exchanges >= before + 3, 4) and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, f"an answer that comes in two pieces (cut after {gap_at} bytes) is read")  # noqa: SLF001
        sim.split_gap_s, sim.split_at = 0.0, 3
        sim.trailer = b"\x01\x02\x03"
        before = bridge._exchanges  # noqa: SLF001
        check(until(lambda: bridge._exchanges >= before + 3, 4) and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, "bytes after an answer that belong to no answer are thrown away before the next request")  # noqa: SLF001
        sim.trailer = b""
        sim.garble = True
        check(until(lambda: bridge._dead is not None and "CRC" in bridge._dead, 4), "an answer whose CRC is wrong is refused, and the dead line says so")  # noqa: SLF001
        sim.garble = False
        check(ready(bridge, 6), "and the hand is read again when the answers are right")
        sim.truncate = 3
        check(until(lambda: bridge._dead is not None, 4), "an answer that stops short is given up on")  # noqa: SLF001
        sim.truncate = 0
        check(ready(bridge, 6), "and the hand is read again")
        sim.noise = 3000
        check(until(lambda: bridge._dead is not None and "baud rate" in bridge._dead, 5), "a line that is noise is said to be noise, or the wrong baud rate")  # noqa: SLF001
        sim.noise = 0
        check(ready(bridge, 6), "and the hand is read again")
        sim.drop = True
        check(until(lambda: bridge._dead is not None and "did not answer" in bridge._dead, 4), "requests that are not answered are a line that did not answer")  # noqa: SLF001
        sim.drop = False
        check(ready(bridge, 6), "and the hand is read again")
        sim.silence(7)
        check(until(lambda: bridge._dead is not None and "[7]" in bridge._dead, 4), "one servo that stops answering is named")  # noqa: SLF001
        sim.silence(7, False)
        check(ready(bridge, 6), "and the hand is read again when it is back")
        sim.echo_request = True
        before = bridge._exchanges  # noqa: SLF001
        check(until(lambda: bridge._exchanges >= before + 3, 4) and bridge._dead is None and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, "a request heard back (a half-duplex adapter's echo) is skipped, and the answers behind it are read")  # noqa: SLF001
        sim.echo_request = False
        sim.skip_requests = 1
        before = bridge._exchanges  # noqa: SLF001
        check(until(lambda: bridge._exchanges >= before + 3, 4) and bridge._dead is None, "one lost request is one failed cycle and not a dead line")  # noqa: SLF001
        sim.echo_lag_s = 0.4
        sim.place([reg(0.3)] * 16)
        check(until(lambda: abs(state(bridge)[1].position_rad[0] - 0.3) < 2e-3, 4), "feedback that lags is read late and is read")
        sim.echo_lag_s = 0.0
        bridge.close()
    finally:
        sim.stop()

    section("the line — a servo that answers with an error, and positions that are not positions")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.report_positions = [1 << 30] * 16
        check(until(lambda: bridge._dead is not None and "multi-turn" in bridge._dead, 4), "positions outside the servo's multi-turn range are a frame that is wrong, not a hand somewhere strange")  # noqa: SLF001
        sim.report_positions = None
        check(ready(bridge, 6), "and the hand is read again")
        bridge.close()
    finally:
        sim.stop()

    section("the line — the link on its own")
    lk, thread = scripted_link([status_of(0, b"\x01\x02"), status_of(1, b"\x03\x04")])
    answers = lk.transact(frames.sync_read(0, 2, [0, 1]), (0, 1), 1.0)
    check(answers[0].params == b"\x01\x02" and answers[1].params == b"\x03\x04", "two servos' answers to a synchronised read are collected by id")
    lk.close()
    thread.join()
    lk, thread = scripted_link([status_of(0, b"\x01"), status_of(0, b"\x01")])
    check("twice" in (raises(frames.ProtocolError, lk.transact, frames.sync_read(0, 1, [0, 1]), (0, 1), 1.0) or ""), "a servo that answers twice is refused")
    lk.close()
    thread.join()
    lk, thread = scripted_link([status_of(5, b"\x01")])
    check("asked [0, 1]" in (raises(frames.ProtocolError, lk.transact, frames.sync_read(0, 1, [0, 1]), (0, 1), 1.0) or ""), "and so is one that was not asked")
    lk.close()
    thread.join()
    good = status_of(0, b"\x01\x02")
    lk, thread = scripted_link([good[:6]])
    message = raises(link.LinkError, lk.transact, frames.read(0, 0, 2), (0,), 1.0) or ""
    check("stopped sending" in message and "[0]" in message, "an answer that stops in the middle of a packet is given up on after the byte timeout, naming the servo that is missing", message)
    lk.close()
    thread.join()
    lk, thread = scripted_link([b"\x00" * 100 + good])
    check(lk.transact(frames.read(0, 0, 2), (0,), 1.0)[0].params == b"\x01\x02", "a hundred bytes of noise before a packet are skipped")
    lk.close()
    thread.join()
    lk, thread = scripted_link([status_of(0, b"\x01"), status_of(1, b"\x03")])
    message = raises(link.LinkError, lk.transact, frames.sync_read(0, 1, [0, 1, 2]), (0, 1, 2), 0.6) or ""
    check("[2]" in message and "2 of 3" in message, "a synchronised read with one servo missing says which, and how many answered", message)
    lk.close()
    thread.join()
    message = raises(link.LinkError, lk.transact, frames.ping(0), (0,), 0.1) or ""
    check("not open" in message, "a transaction on a link that is closed is refused")
    lk = link.Link(device="/dev/leap-does-not-exist")
    check("could not be opened" in (raises(link.LinkError, lk.open) or ""), "a device that does not exist is a line that could not be opened")
    sim = new_sim()
    try:
        lk = link.Link(device=sim.path, baud=12345)
        check("not a speed" in (raises(link.LinkError, lk.open) or ""), "and a baud rate the line cannot be set to")
        lk = link.Link(device=sim.path, baud=4000000)
        lk.open()
        check(lk.transact(frames.ping(3), (3,), 1.0)[3].params == (1240).to_bytes(2, "little") + b"\x34", "a ping is answered with the model number and the firmware version")
        sim.pull()
        check(until(lambda: raises(link.LinkError, lk.transact, frames.ping(3), (3,), 0.3) is not None and not lk.is_open, 3), "a line that failed with an OS error (the adapter was pulled) is closed, so that whoever drives it knows to open it again")
    finally:
        sim.stop()

    section("the line — a hand behind a gateway that forwards the line raw")
    gateway = new_sim(tcp=True)
    try:
        bridge = opened(gateway)
        check(ready(bridge), "a hand behind a gateway is read")
        check(write(bridge, cmd(pose(0.3)))[0] == rhal.SKR_OK and until(lambda: gateway.goals == [reg(0.3)] * 16, 3), "and commanded")
        gateway.hang_up()
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 6), "a gateway that drops the connection is reconnected and the hand is read again")
        check(until(lambda: write(bridge, cmd(pose(0.2)))[0] == rhal.SKR_OK and gateway.goals == [reg(0.2)] * 16, 6), "and commanded again (a command in the moment before the line is seen to be down is written to a socket that is not there, so it is repeated)")
        bridge.close()
        gateway.refuse_connections = True
        code, said, bridge = opens_with(gateway, open_timeout_ms=600)
        check(code == rhal.SKR_EIO, "a gateway that accepts and hangs up is EIO at open", said)
        bridge.close()
        gateway.refuse_connections = False
        gateway.stop_listening()
        code, said, bridge = opens_with(gateway, open_timeout_ms=600)
        check(code == rhal.SKR_EIO and "could not be opened" in said, "and one that is not listening is a line that could not be opened", said)
        bridge.close()
    finally:
        gateway.stop()

    section("the line — a serial line the kernel will not configure, and a device that is not there")
    sim = new_sim()
    try:
        with mock.patch("termios.tcsetattr", side_effect=termios.error(22, "Invalid argument")):
            code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_EIO and "could not be opened" in said and "Invalid argument" in said, "a serial line the kernel will not configure (termios.error, which is not an OSError) is EIO and says what it was: it does not escape into the host (F301)", said)
        bridge.close()
        code, said, bridge = opens_with(sim, device="/dev/leap-does-not-exist")
        check(code == rhal.SKR_EIO and "could not be opened" in said, "and a device that is not there is EIO", said)
        bridge.close()
    finally:
        sim.stop()


def main_session() -> None:
    section("open and close — a close ends the thread and the line, and a session starts with nothing of the last one's")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.delay_s = 0.1
        thread = bridge._loop  # noqa: SLF001
        began = time.monotonic()
        bridge.close()
        check(thread is not None and not thread.is_alive() and time.monotonic() - began < 1.5, "a close does not return until the reading thread has ended, and does not take its two seconds")
        sim.delay_s = 0.0
        base = settled_fds()
        for _ in range(5):
            opened(sim).close()
        check(until(lambda: fds() <= base, 3), "five opens and closes leave no line open")
        bridge = opened(sim)
        check(ready(bridge), "ready")
        bridge._last_refusal = "an old refusal"  # noqa: SLF001
        bridge._reasserted = 7  # noqa: SLF001
        bridge.emergency_stop()
        check(bridge._estop, "a stop is latched")  # noqa: SLF001
        check(bridge.close() == rhal.SKR_OK and bridge.close() == rhal.SKR_OK, "a close is OK, and so is a second one")
        code = bridge.open(config(sim))
        check(code == rhal.SKR_OK and ready(bridge), "the same driver opened again")
        check(bridge._last_refusal is None and bridge._reasserted == 0 and not state(bridge)[1].flags & rhal.FLAG_ESTOP and ext(bridge, "status")[1]["stop_delivered"] is False, "has no refusal remembered, no count of parameters told, no stop and nothing delivered")  # noqa: SLF001
        first = bridge._loop  # noqa: SLF001
        check(bridge.open(config(sim)) == rhal.SKR_OK, "opened again without a close")
        check(first is not None and not first.is_alive() and bridge._loop is not first, "the first reading thread has ended")  # noqa: SLF001
        bridge.close()
        check(LeapBridge().close() == rhal.SKR_OK, "a driver that was never opened can be closed")
        check(bridge.release_stop() == rhal.SKR_OK and LeapBridge().release_stop() == rhal.SKR_OK, "and a stop released on one that is closed is OK: there is nothing latched")
    finally:
        sim.stop()

    section("describe — the descriptor")
    sim = new_sim()
    try:
        check(LeapBridge().describe() == "{}", "a driver that is not open describes nothing")
        bridge = opened(sim)
        descriptor = json.loads(bridge.describe())
        check(descriptor["robot_slug"] == "leap-hand-full" and descriptor["simulator"] is True and descriptor["base"] == "fixed" and descriptor["dof"] == 16 and descriptor["verified"] is False and descriptor["digest"] is None, "the descriptor names the hand, says it is a simulation, fixed, of sixteen degrees of freedom, and unverified")
        joints = descriptor["joints"]
        check([j["name"] for j in joints] == list(JOINTS) and all(j["type"] == "revolute" and j["control_modes"] == ["position"] for j in joints), "sixteen revolute joints in the maker's order, position-controlled")
        check([j["limits"]["position_min_rad"] for j in joints] == list(frames.LIMITS_MIN) and [j["limits"]["position_max_rad"] for j in joints] == list(frames.LIMITS_MAX), "with the maker's ranges")
        check(all("velocity_max_rad_s" not in j["limits"] for j in joints), "and no speed: nothing here says what a servo under a current cap does")
        check(joints[2]["role"] == "right_hand.index_pip.flex" and joints[12]["role"] == "right_hand.thumb_mcp.abduct" and joints[0]["role"] is None, "the roles the operator asserted, and none that were not")
        check(descriptor["end_effectors"][0]["kind"] == "dexterous_hand" and descriptor["end_effectors"][0]["dof"] == 16 and descriptor["end_effectors"][0]["max_grip_force_n"] is None, "a dexterous hand, with no force in newtons")
        check(descriptor["envelope"] is None and descriptor["capabilities"] == ["safety.estop"] and descriptor["extensions"] == [TOKEN] and descriptor["control"] == {"modes": ["position"], "rate_hz_max": 50.0, "rate_hz_min": 1.0}, "no envelope, an e-stop capability, one extension and a control rate")
        check([m["name"] for m in descriptor["motions"]] == ["status", "move", "enable_torque", "clear_errors"], "four motions")
        move = [m for m in descriptor["motions"] if m["name"] == "move"][0]
        check(len(move["parameters"]) == 17 and move["parameters"][-1]["name"] == "timeout" and all(p["unit"] in ("rad", "s") for p in move["parameters"]), "`move` takes any of the joints in radians and a timeout in seconds")
        check([p["required"] for m in descriptor["motions"] if m["name"] == "clear_errors" for p in m["parameters"]] == [True], "and `clear_errors` needs its confirmation")
        bridge.close()
        bridge = opened(sim, simulator=False)
        check(json.loads(bridge.describe())["simulator"] is False, "a driver told it is not a simulation says so")
        check(bridge.read_sensor("camera", rhal.Frame()) == rhal.SKR_ENOSYS, "there is no sensor to read")
        bridge.close()
    finally:
        sim.stop()


def main() -> int:
    main_frames()
    main_open()
    main_state()
    main_commands()
    main_gains()
    main_stop()
    main_motions()
    main_line()
    main_session()
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAILED: {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
