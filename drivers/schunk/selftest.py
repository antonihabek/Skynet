"""The SCHUNK gripper bridge against a simulated module written here (DRY-COVERAGE-PLAN.md item 12.E.9): the frames, what it refuses to open on, the state, the handshake, the refusals, the stop and the motions.

    PYTHONPATH=../../bindings/python:. python3 selftest.py

Needs python3 and nothing of the maker's. The simulated module agrees with whoever wrote it (F15); `layout_check.py` holds the driver's bytes to the maker's own client and `live.py` runs it against the maker's own
simulator, and what this shows is the driver's own logic.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import struct
import sys
import threading
import time

import skynet_rhal as rhal

from skynet_schunk import frames
from skynet_schunk.bridge import BridgeRefusal, SchunkBridge, TOKEN
from skynet_schunk.frames import Bit, Status
from skynet_schunk.gripper_sim import SimulatedGripper

CHECKS = 0
FAILURES: list[str] = []
SPEED = 0.02          # m/s
SCALE = 20.0          # the simulated jaw moves this much faster than its speed says


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
        time.sleep(0.01)
    return bool(predicate())


def settings(gripper: SimulatedGripper, **extra) -> dict:
    out = {"host": "127.0.0.1", "port": gripper.port, "stop_mode": "controlled", "speed": SPEED, "simulator": True, "roles": {"jaw": "right_hand.gripper.grip"}, "rate_hz": 20}
    out.update(extra)
    return {k: v for k, v in out.items() if v is not None}


def config(gripper: SimulatedGripper, **extra) -> str:
    return json.dumps(settings(gripper, **extra))


def opened(gripper: SimulatedGripper, **extra) -> SchunkBridge:
    bridge = SchunkBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(gripper, **extra))
    assert code == rhal.SKR_OK, said.getvalue()
    return bridge


def refused(text: str) -> tuple[int, str]:
    bridge = SchunkBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(text)
    bridge.close()
    return code, said.getvalue()


def opens_with(gripper: SimulatedGripper, **extra) -> tuple[int, str, SchunkBridge]:
    bridge = SchunkBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(gripper, **extra))
    return code, said.getvalue(), bridge


def state(bridge: SchunkBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def cmd(value: float, mask: list[int] | None = None, mode: int = rhal.MODE_POSITION, count: int = 1) -> rhal.Command:
    return rhal.Command(stamp_ns=1, seq=1, mode=mode, values=[value] * count, mask=mask or [1] * count)


def write(bridge: SchunkBridge, command: rhal.Command) -> tuple[int, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def ext(bridge: SchunkBridge, text: str) -> tuple[int, object]:
    code, body = bridge.extension(TOKEN, text.encode())
    try:
        return code, json.loads(body)
    except json.JSONDecodeError:
        return code, body.decode()


def ready(bridge: SchunkBridge) -> bool:
    return until(lambda: state(bridge)[0] == rhal.SKR_OK)


def words(frame_hex: str) -> int:
    return struct.unpack("<I", bytes.fromhex(frame_hex)[:4])[0]


def main() -> int:
    print("skynet-schunk selftest\n")

    # ── the frames ───────────────────────────────────────────────────────────
    section("frames — the maker's two 16-byte frames, byte for byte")
    check(frames.clear_output().hex().upper() == "01" + "00" * 15, "the cleared output has control bit 0 high and nothing else: a fast stop triggers on a LOW bit")
    move = frames.command_output({Bit.MOVE_ABSOLUTE: True}, "EI", position=50000, speed=20000)
    check(move.hex().upper() == "01200000" + "50C30000" + "204E0000" + "00000000", "a move to 50000 um at 20000 um/s is control word 0x2001, then the target and the speed little-endian, then no force")
    check(frames.command_output({Bit.MOVE_ABSOLUTE: True}, "PN", position=50000, speed=20000).hex().upper() == "01200000" + "0000C350" + "00004E20" + "00000000",
          "and on PROFINET the integers are reversed and the control word is not")
    check(frames.command_output({Bit.FAST_STOP: False}, "EI").hex().upper() == "00" * 16, "a fast stop is the control word with bit 0 LOW: all zeros")
    check(words(frames.command_output({Bit.GRIP_AT_POSITION: True, Bit.GRIP_DIRECTION: True}, "EI", position=1, force=50).hex().upper()) == (1 | 1 << 16 | 1 << 7),
          "bits above 15 are in the fourth byte's neighbours: a grip at a position is bit 16")
    check(frames.command_output({Bit.ACKNOWLEDGE: True}, "EI").hex().upper() == "05" + "00" * 15, "an acknowledge is bit 2, with bit 0 high")
    for what, bits, kwargs in (("a reserved control bit", {10: True}, {}), ("a bit past 31", {32: True}, {}), ("a position past int32", {Bit.MOVE_ABSOLUTE: True}, {"position": 2 ** 31}), ("a speed below int32", {Bit.MOVE_ABSOLUTE: True}, {"speed": -2 ** 31 - 1})):
        try:
            frames.command_output(bits, "EI", **kwargs)
            check(False, f"{what} is refused")
        except frames.ProtocolError:
            check(True, f"{what} is refused")
    powerup = frames.parse_input("80000080FB00000000000000D9000000", "EI")
    check(powerup.bit(Status.ERROR) == 1 and powerup.error == 0xD9 and powerup.position_um == 251 and powerup.bit(Status.READY) == 0,
          "the input frame the maker's simulator comes up with is an error, 0xD9, at 251 um, and not ready")
    pn = frames.parse_input("00000000FFFFFE87" + "00" * 8, "PN")
    check(pn.position_um == -377, "a PROFINET position is read reversed")
    for what, text in (("fifteen bytes", "00" * 15), ("not hex", "zz" * 16), ("a number", 12), ("none", None)):
        try:
            frames.parse_input(text, "EI")
            check(False, f"an input frame that is {what} is refused")
        except frames.ProtocolError:
            check(True, f"an input frame that is {what} is refused")
    check(frames.decode_parameter("0x0608", bytes.fromhex("0000F042"), "EI") == 120.0 and frames.decode_parameter("0x0608", bytes.fromhex("42F00000"), "PN") == 120.0,
          "a float parameter is millimetres, little-endian, and reversed on PROFINET")
    check(frames.decode_parameter("0x0500", bytes.fromhex("0B"), "EI") == 11, "an enum is an int16, and a one-byte answer is zero-padded as the maker's client pads it")
    check(frames.decode_parameter("0x1000", b"NSNS0667        ", "EI") == "NSNS0667", "text is ASCII, padding stripped")
    for what, ei, pn in (("a NaN", "0000C07F", "7FC00000"), ("an infinity", "0000807F", "7F800000"), ("a negative infinity", "000080FF", "FF800000")):
        for fieldbus, text in (("EI", ei), ("PN", pn)):
            try:
                frames.decode_parameter("0x0608", bytes.fromhex(text), fieldbus)
                check(False, f"a float parameter that is {what} on {fieldbus} is refused")
            except frames.ProtocolError as error:
                check("0x0608" in str(error), f"a float parameter that is {what} on {fieldbus} is refused, naming the parameter", str(error))
    # The status bits by number, as the maker's own message names them (`bitN_name` in schunk_gripper_driver's status message, and the `{"4": 1, "13": 1}` its client waits for a move to finish). A constant moved
    # to its neighbour moves the driver and the simulator together, and every check that uses the name on both sides agrees with itself: only a literal sees it.
    makers = {"READY": 0, "CONTROL_AUTHORITY": 1, "READY_SHUTDOWN": 2, "NOT_FEASIBLE": 3, "SUCCESS": 4, "TOGGLE": 5, "WARNING": 6, "ERROR": 7, "RELEASED_MANUAL": 8, "SOFT_LIMIT": 9, "NO_WORKPIECE": 11,
              "GRIPPED": 12, "POSITION_REACHED": 13, "PRE_GRIP": 14, "WORKPIECE_LOST": 16, "WRONG_WORKPIECE": 17, "GRIP_FORCE_MAINTENANCE": 31}
    wrong = {name: getattr(Status, name) for name, bit in makers.items() if getattr(Status, name) != bit}
    check(not wrong and sorted(makers.values()) == sorted(Status.NAMES), "every status bit is the number the maker's message gives it, and every one has its name", str(wrong))
    check(frames.MODULE_TYPES[11] == "EGU_60_M_B" and frames.FIELDBUS == {1: "PN", 2: "EI", 3: "EC", 7: "MB"}, "the module and fieldbus enumerations are the maker's")
    check(frames.m_to_um(0.0499996) == 50000 and frames.um_to_m(50000) == 0.05, "metres to micrometres rounds to the nearest")
    try:
        frames.decode_parameter("0x9999", b"", "EI")
        check(False, "a parameter this driver does not read is refused")
    except frames.ProtocolError:
        check(True, "a parameter this driver does not read is refused")

    # ── open ─────────────────────────────────────────────────────────────────
    section("open — what the configuration must say, and what the module must say")
    with SimulatedGripper(time_scale=SCALE) as g:
        for what, extra, needle in (
            ("no stop mode", {"stop_mode": None}, "stop_mode"), ("a stop mode that is not one", {"stop_mode": "gentle"}, "stop_mode"),
            ("no simulator flag", {"simulator": None}, "simulator"), ("a simulator flag that is not a boolean", {"simulator": "yes"}, "simulator"),
            ("no speed", {"speed": None}, "speed"), ("a speed of nothing", {"speed": 0}, "not a number above zero"), ("a speed above what the module can do", {"speed": 5.0}, "outside what the module itself reports"),
            ("a speed below it", {"speed": 0.001}, "outside what the module itself reports"),
            ("no host", {"host": None}, "host"), ("a port that is not one", {"port": 70000}, "port"), ("a key it does not read", {"colour": "red"}, "does not read"),
            ("a role for a joint that is not one", {"roles": {"finger": "x"}}, "roles"), ("a rate of nothing", {"rate_hz": 0}, "rate_hz"),
            ("limits that are not two numbers", {"limits": [0.0]}, "limits"), ("limits that are not a range", {"limits": [0.05, 0.05]}, "not a range"),
            ("limits wider than the module's own travel", {"limits": [0.0, 0.2]}, "narrow it and never widen it"),
        ):
            code, said = refused(config(g, **extra))
            check(code == rhal.SKR_ENODEV and needle in said, f"{what} is refused at open, and the refusal says why", said)
        check(refused("{not json")[0] == rhal.SKR_EINVAL and refused("[1]")[0] == rhal.SKR_EINVAL, "a configuration that is not a JSON object is EINVAL")
        code, said, bridge = opens_with(g, limits=[0.01, 0.1])
        check(code == rhal.SKR_OK and json.loads(bridge.describe())["joints"][0]["limits"]["position_max_rad"] == 0.1, "limits inside the module's own travel narrow it")
        bridge.close()
    code, said = refused(json.dumps({"host": "127.0.0.1", "port": 1, "stop_mode": "controlled", "speed": SPEED, "simulator": True}))
    check(code == rhal.SKR_EIO and "did not answer" in said, "a port nobody answers on is a transport failure (EIO), not a refusal of the body", said)
    with SimulatedGripper(fieldbus="PN", time_scale=SCALE) as g:
        code, said, bridge = opens_with(g)
        check(code == rhal.SKR_ENODEV and "PROFINET" in said and "byte 12" in said and "byte 15" in said,
              "a PROFINET module is refused, saying the maker's client and its own dump disagree on where the error byte is", said)
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        g._data["0x1130"] = "07"
        code, said, bridge = opens_with(g)
        check(code == rhal.SKR_ENODEV and "Modbus" in said, "a Modbus RTU module is refused: this driver has no serial port to open", said)
        g._data["0x1130"] = "09"
        check(opens_with(g)[0] == rhal.SKR_ENODEV, "and a fieldbus the maker does not list is refused")
        g._data["0x1130"] = "02"
        g._data["0x0500"] = "FF"
        code, said, bridge = opens_with(g)
        check(code == rhal.SKR_ENODEV and "not one the maker's client knows" in said, "a module type the maker's client does not know is refused", said)
        g._data["0x0500"] = "0B"
        g._data["0x0600"] = g._data["0x0608"]
        check(opens_with(g)[0] == rhal.SKR_ENODEV, "a travel that is not a range is refused")
    with SimulatedGripper(time_scale=SCALE) as g:
        g._set_bit(Status.READY, False)
        code, said, bridge = opens_with(g)
        check(code == rhal.SKR_ENODEV and "not ready" in said, "a module that is not ready and holds no error is refused", said)
    with SimulatedGripper(start_ready=False, time_scale=SCALE) as g:
        code, said, bridge = opens_with(g)
        check(code == rhal.SKR_OK and "0xD9" in said and "stop latched" in said and "release_stop" in said, "a module that reports its fast stop (0xD9), as the maker's simulator comes up, opens with the stop LATCHED, and says why", said)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        code, s = state(bridge)
        check(bool(s.flags & rhal.FLAG_ESTOP) and bool(s.flags & rhal.FLAG_DEGRADED) and s.fault_code == 0xD9, "and its state is ESTOP and DEGRADED with the maker's error byte as the fault word")
        check(write(bridge, cmd(0.05))[0] == rhal.SKR_EPERM and g.commands == [], "a command is refused, and nothing was written to the module")
        bridge.close()
        check(g.commands == [] and not [r for r in g.requests if r[0] == "POST"], "and neither open nor close wrote anything: nothing here acknowledges a module by itself")
    with SimulatedGripper(start_ready=False, time_scale=SCALE) as g:
        g._input[12] = 0xEF
        code, said, bridge = opens_with(g)
        check(code == rhal.SKR_OK and "0xEF" in said and "does not name" in said, "any other error opens latched too, with the code, and says the maker's client does not name it", said)
        bridge.close()

    # ── the descriptor ───────────────────────────────────────────────────────
    section("the descriptor — the module's word on what it is, the configuration's on the rest")
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g)
        descriptor = json.loads(bridge.describe())
        joint = descriptor["joints"][0]
        check(descriptor["robot_slug"] == "schunk-egu-60-m-b" and descriptor["dof"] == 1 and descriptor["base"] == "fixed" and joint["name"] == "jaw" and joint["type"] == "prismatic",
              "one prismatic joint on a fixed base, named for the module type it read")
        check(joint["limits"]["position_min_rad"] == 0.0 and joint["limits"]["position_max_rad"] == 0.12 and joint["limits"]["velocity_max_rad_s"] == SPEED, "its travel is the module's own, in metres, and its speed is the configuration's")
        check(joint["role"] == "right_hand.gripper.grip" and descriptor["end_effectors"][0]["kind"] == "gripper" and abs(descriptor["end_effectors"][0]["max_opening_m"] - 0.12) < 1e-12,
              "its role is the configuration's, and it declares a gripper whose opening is its travel")
        check(descriptor["simulator"] is True and descriptor["envelope"] is None and descriptor["control"]["modes"] == ["position"] and TOKEN in descriptor["extensions"], "the simulator flag is the configuration's, no envelope is invented, and its one mode is position")
        check([m["name"] for m in descriptor["motions"]] == ["status", "param", "move", "grip", "release"], "its motions are the maker's primitives")
        check(descriptor["verified"] is False and joint["control_modes"] == ["position"] and descriptor["capabilities"] == ["safety.estop"],
              "it claims no verification, takes a position and no other mode, and declares the emergency stop it has and nothing more")
        bridge.close()
    with SimulatedGripper(module="EGK_25_M_B", time_scale=SCALE) as g:
        bridge = opened(g, speed=0.02, simulator=False)
        descriptor = json.loads(bridge.describe())
        check(descriptor["robot_slug"] == "schunk-egk-25-m-b" and abs(descriptor["joints"][0]["limits"]["position_min_rad"] - 0.02) < 1e-9 and descriptor["simulator"] is False,
              "an EGK 25 reports a travel that starts at 20 mm, and a configuration that says it is not a simulation is believed: nothing in the module says")
        bridge.close()
    check(SchunkBridge().describe() == "{}", "a bridge that was never opened describes nothing")

    # ── state ────────────────────────────────────────────────────────────────
    section("read_state — what the module says, stamped when it arrived")
    with SimulatedGripper(time_scale=SCALE) as g:
        g.place(31250)
        bridge = opened(g)
        ready(bridge)
        code, first = state(bridge)
        check(code == rhal.SKR_OK and abs(first.position_rad[0] - 0.03125) < 1e-12 and len(first.position_rad) == 1, "the position is the module's, in metres")
        check(math.isnan(first.velocity_rad_s[0]) and math.isnan(first.effort_nm[0]), "no velocity or effort is invented: the module reports a position only (NaN, §24)")
        check(first.flags == rhal.HOMING_NOT_REQUIRED and first.fault_code == 0, "a ready module reads clean")
        time.sleep(0.1)
        second = state(bridge)[1]
        check(second.seq == first.seq + 1 and second.stamp_ns >= first.stamp_ns, "each read has the next sequence number and a stamp that does not go back")
        g.set_warning(0x31)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 2.0)
        code, warned = state(bridge)
        check(bool(warned.flags & rhal.FLAG_DEGRADED) and not warned.flags & rhal.FLAG_FAULT and warned.fault_code == 0, "a warning is DEGRADED alone, and takes commands")
        check(write(bridge, cmd(0.05))[0] == rhal.SKR_OK, "a warned module still takes a command")
        g.set_warning(None)
        until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 2.0)
        g.set_error(0x42)
        until(lambda: state(bridge)[1].fault_code == 0x42, 2.0)
        code, faulted = state(bridge)
        check(faulted.fault_code == 0x42 and faulted.flags & rhal.FLAG_FAULT and faulted.flags & rhal.FLAG_DEGRADED and not faulted.flags & rhal.FLAG_ESTOP, "an error is FAULT and DEGRADED, with the module's own byte as the fault word")
        g.set_error(None)
        until(lambda: state(bridge)[1].fault_code == 0, 2.0)
        check(state(bridge)[1].fault_code == 0 and not state(bridge)[1].flags & rhal.FLAG_FAULT, "and it is not sticky: the driver reads what the module says now")
        g.set_error(0)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 2.0)
        code, nameless = state(bridge)
        check(nameless.fault_code == 1 and bool(nameless.flags & rhal.FLAG_FAULT), "a module in error that gives no error byte still has a fault word of 1: zero is no fault")
        g.set_error(None)
        until(lambda: state(bridge)[1].fault_code == 0 and not state(bridge)[1].flags & rhal.FLAG_FAULT, 2.0)
        g.set_error(frames.ERROR_FAST_STOP)
        until(lambda: state(bridge)[1].fault_code == 0xD9, 2.0)
        code, stopped = state(bridge)
        check(stopped.fault_code == 0xD9 and stopped.flags & rhal.FLAG_ESTOP and not stopped.flags & rhal.FLAG_FAULT, "the maker's fast-stop error, 0xD9, is the stop chain being open: ESTOP and DEGRADED, not FAULT")
        g.set_error(None)
        until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 2.0)   # the stop error above is DEGRADED until the poller has read it cleared: without this the check below passes on the old sample
        g._set_bit(Status.READY, False)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 2.0)
        check(bool(state(bridge)[1].flags & rhal.FLAG_DEGRADED), "a module that is not ready for operation is DEGRADED")
        g._set_bit(Status.READY, True)
        until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 2.0)
        g.drop = True
        time.sleep(0.9)
        code, stale = state(bridge)
        check(code == rhal.SKR_ETIMEDOUT and stale.flags & rhal.FLAG_DEGRADED, "a module that has stopped answering is ETIMEDOUT and DEGRADED: silence is not stillness")
        check(0 < stale.stamp_ns < rhal.acquisition_ns() - 400_000_000, "and the stamp is still the sample's own, not the read's")
        g.drop = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3.0), "and it recovers when the module does")
        until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0)
        g.reset_connections()
        seen = []
        deadline = time.monotonic() + 0.45
        while time.monotonic() < deadline:
            seen.append(state(bridge))
            time.sleep(0.02)
        check(any(s.flags & rhal.FLAG_DEGRADED for _, s in seen) and all(c == rhal.SKR_OK for c, _ in seen), "a connection the module dropped is DEGRADED for a moment, though the next request went through")
        bridge.close()
        check(state(bridge)[0] == rhal.SKR_EPERM, "a closed bridge reads EPERM")
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g, stale_ms=200, http_timeout_ms=3000)
        ready(bridge)
        g.delay_s = 0.6
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT and bool(state(bridge)[1].flags & rhal.FLAG_DEGRADED), 4.0),
              "a module that answers every request, slowly, past the staleness bound, reads ETIMEDOUT and DEGRADED")
        g.delay_s = 0.0
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 4.0), "and reads clean again when it is quick")
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        # The bounds the configuration does not name are the driver's own: a module that has gone quiet inside a long request is believed for half a second and not for a minute.
        bridge = opened(g, http_timeout_ms=3000)
        ready(bridge)
        g.delay_s = 1.5
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT and bool(state(bridge)[1].flags & rhal.FLAG_DEGRADED), 1.4),
              "with no staleness bound configured, a sample the module has not renewed for a second is ETIMEDOUT and DEGRADED: the default is half a second")
        g.delay_s = 0.0
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 4.0), "and reads clean again when the module is quick")
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        # The default request timeout is half a second: the acknowledge of a released stop waits for the module no longer than that, whatever it is told to wait for elsewhere.
        bridge = opened(g)
        ready(bridge)
        bridge.emergency_stop()
        for thread in bridge._stop_threads:
            thread.join(timeout=2.0)
        g.delay_s = 2.0
        began = time.monotonic()
        code = bridge.release_stop()
        took = time.monotonic() - began
        g.delay_s = 0.0
        check(code == rhal.SKR_EIO and took < 1.5, f"a module that takes two seconds to answer anything is EIO to the acknowledge after the default half-second request timeout, not after waiting it out ({took:.2f} s)")
        bridge.close()

    # ── commands ─────────────────────────────────────────────────────────────
    section("write_command — the handshake, and what is refused before it")
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g)
        ready(bridge)
        before = len(g.requests)
        check(write(bridge, cmd(0.05))[0] == rhal.SKR_OK, "a position inside the travel is accepted")
        posts = [(m, p, f) for m, p, f in g.requests[before:] if m == "POST"]
        check(len(posts) == 2 and posts[0][2]["value"] == frames.clear_output().hex().upper(), "it is the maker's handshake: the cleared control word is written first")
        check(posts[1][2]["value"] == frames.command_output({Bit.MOVE_ABSOLUTE: True}, "EI", position=50000, speed=20000).hex().upper(), "then the command, with the target and the configured speed in micrometres")
        check(all(p[2]["inst"] == frames.Instance.OUTPUT for p in posts), "both written to the output frame, 0x0048")
        gets = [r for r in g.requests[before:] if r[0] == "GET" and q_inst(r[2]) == frames.Instance.INPUT]
        check(len(gets) >= 2, "with the input frame read between them for the toggle, and after it for the flip")
        check(until(lambda: g.position_um == 50000, 3.0) and until(lambda: {Status.SUCCESS, Status.POSITION_REACHED} <= set(g.status_bits()), 3.0), "and the module's jaw gets there, and says so")
        count = len(g.commands)
        check(write(bridge, cmd(0.05))[0] == rhal.SKR_OK and len(g.commands) == count, "the same target again is not sent again")
        check(write(bridge, cmd(0.0499996))[0] == rhal.SKR_OK and len(g.commands) == count, "nor is one that rounds to the same micrometre")
        check(write(bridge, cmd(0.06))[0] == rhal.SKR_OK and g.commands[-1]["position"] == 60000, "a new target is")
        check(write(bridge, cmd(0.0, mask=[0]))[0] == rhal.SKR_OK and len(g.commands) == count + 1, "a masked joint's value is not read, and nothing is sent")
        check(write(bridge, cmd(0.12))[0] == rhal.SKR_OK and g.commands[-1]["position"] == 120000, "the top of the travel is accepted")
        check(write(bridge, cmd(0.0))[0] == rhal.SKR_OK and g.commands[-1]["position"] == 0, "and the bottom")
        sent = len(g.commands)
        for what, value, needle in (("a position past the top", 0.1200011, "outside its travel"), ("one below the bottom", -0.001, "outside its travel"), ("a NaN", math.nan, "not finite"), ("an infinity", math.inf, "not finite")):
            bridge._last_refusal = None   # each is a first of its kind: a run of the same cause is said once, and this is not that check
            code, said = write(bridge, cmd(value))
            check(code == rhal.SKR_EINVAL and needle in said and (f"{value:.6f}" in said or "finite" in said), f"{what} is refused saying so", said)
        check(len(g.commands) == sent, "and none of them reached the module")
        for what, command in (("a velocity command", cmd(0.05, mode=rhal.MODE_VELOCITY)), ("a torque command", cmd(0.05, mode=rhal.MODE_TORQUE)), ("two joints", cmd(0.05, count=2))):
            bridge._last_refusal = None   # each is a first of its kind: a run of the same cause is said once, and this is not that check
            code, said = write(bridge, command)
            check(code == rhal.SKR_EINVAL and said != "", f"{what} is EINVAL, and says why", said)
        code, once = write(bridge, cmd(0.2))
        code, again = write(bridge, cmd(0.2))
        check(code == rhal.SKR_EINVAL and once != "" and again == "", "a refusal is said once for a run of the same cause")
        check(write(bridge, cmd(0.07))[0] == rhal.SKR_OK, "and an accepted command ends the run")
        code, back = write(bridge, cmd(0.2))
        check(code == rhal.SKR_EINVAL and back != "", "so the same cause after it is said again")
        g.not_feasible_above_um = 100000
        code, said = write(bridge, cmd(0.11))
        check(code == rhal.SKR_EINVAL and "not feasible" in said, "a module that answers *not feasible* is EINVAL, and the answer is said", said)
        g.not_feasible_above_um = None
        check(write(bridge, cmd(0.11))[0] == rhal.SKR_OK, "and the next command, which it takes, is accepted")
        g.set_error(0x42)
        until(lambda: state(bridge)[1].fault_code == 0x42, 2.0)
        sent = len(g.commands)
        code, said = write(bridge, cmd(0.01))
        check(code == rhal.SKR_EPERM and "0x42" in said and "release_stop" in said and len(g.commands) == sent, "a module in error takes nothing but an acknowledge: EPERM, the code, the way out, and nothing sent", said)
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g, limits=[0.01, 0.1])
        ready(bridge)
        code, said = write(bridge, cmd(0.11))
        check(code == rhal.SKR_EINVAL and "[0.010000, 0.100000]" in said, "a narrowed range is the one that refuses, and it names its own bounds", said)
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g)
        ready(bridge)
        g.delay_s = 0.4
        began = time.monotonic()
        code, said = write(bridge, cmd(0.05))
        check(code == rhal.SKR_EIO and "did not" in said and time.monotonic() - began < 1.5, f"a module too slow to take the command inside the handshake's budget is EIO, bounded ({time.monotonic() - began:.2f} s)", said)
        g.delay_s = 0.0
        # F256: a command is the control loop's and the loop waits for it, so a quarter of a second is a bound and not an estimate. Whatever one answer costs, the handshake's four requests may not add up to more:
        # the first version gave every request at least 50 ms even with the budget spent, and the chaos contract measured 256 to 329 ms on every degraded link.
        slowest = 0.0
        for delay in (0.03, 0.06, 0.09, 0.12, 0.2, 0.4):
            g.delay_s = delay
            began = time.monotonic()
            write(bridge, cmd(0.05 + delay / 10))
            took = time.monotonic() - began
            slowest = max(slowest, took)
        g.delay_s = 0.0
        check(slowest <= 0.25, f"a command is back inside the control loop's quarter of a second whatever one answer costs, from 30 ms to 400 ms (the slowest was {slowest * 1000:.0f} ms)")
        until(lambda: state(bridge)[0] == rhal.SKR_OK, 3.0)
        g.drop = True
        bridge._last_refusal = None   # the slow module above was the same kind of failure, and was said: this check is about a run that begins here
        code, first = write(bridge, cmd(0.06))
        code2, second = write(bridge, cmd(0.06))
        check(code == rhal.SKR_EIO and "did not answer" in first and second == "", "a module that has gone is EIO, said once, not every period", first)
        g.drop = False
        check(until(lambda: write(bridge, cmd(0.06))[0] == rhal.SKR_OK, 3.0), "and a command is accepted again when it comes back")
        bridge.close()

    # ── the stop ─────────────────────────────────────────────────────────────
    section("the stop — latched at once, written under the lock a command takes")
    for mode, bits, why in (("fast", {Bit.FAST_STOP: False}, "a fast stop is the control word with bit 0 low"), ("controlled", {Bit.STOP: True}, "a controlled stop is the stop bit")):
        with SimulatedGripper(time_scale=SCALE) as g:
            bridge = opened(g, stop_mode=mode)
            ready(bridge)
            write(bridge, cmd(0.1))
            g.delay_s = 0.5
            began = time.monotonic()
            check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.05, f"{mode}: the stop returns in {(time.monotonic() - began) * 1000:.1f} ms with the module taking half a second to answer anything")
            check(write(bridge, cmd(0.05))[0] == rhal.SKR_EPERM, "and the latch is already holding: a command is refused")
            check(bool(state(bridge)[1].flags & rhal.FLAG_ESTOP), "the state says stopped")
            g.delay_s = 0.0
            want = frames.command_output(bits, "EI").hex().upper()
            check(until(lambda: any(c["hex"] == want for c in g.commands), 6.0), f"and the module is told: {why}")
            if mode == "fast":
                check(until(lambda: g.error_code == frames.ERROR_FAST_STOP, 2.0), "the fast stop leaves the module in error 0xD9, which is the maker's own behaviour")
            else:
                check(g.error_code == 0, "the controlled stop leaves no error")
            bridge.release_stop()
            bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        # A request lost on the way, not slow: the stop is asked again until the module has it.
        bridge = opened(g, stop_mode="fast")
        ready(bridge)
        g.drop_posts = 3
        bridge.emergency_stop()
        check(until(lambda: g.error_code == frames.ERROR_FAST_STOP, 6.0), "a stop whose first requests were lost on the way is asked again until the module has it")
        check(g.drop_posts == 0, "and it was three that were lost, not three that were late")
        bridge.release_stop()
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        # The race the writer lock exists for: a command mid-handshake when the stop latches must not be written after it.
        bridge = opened(g, stop_mode="controlled")
        ready(bridge)
        # 40 ms an answer puts the handshake's third request (the command itself) at about 80 ms: the stop at 70 ms lands between the toggle being read and the command being written, inside the 0.2 s budget
        # (with 80 ms an answer the command times out first, F256, and the race is not the one being tested).
        g.delay_s = 0.04
        outcome: list = []
        worker = threading.Thread(target=lambda: outcome.append(write(bridge, cmd(0.1))), daemon=True)
        worker.start()
        time.sleep(0.07)
        bridge.emergency_stop()
        worker.join(3.0)
        g.delay_s = 0.0
        until(lambda: any(c["word"] & (1 << Bit.STOP) for c in g.commands), 5.0)
        time.sleep(0.3)
        words_seen = [c["word"] for c in g.commands]
        stop_at = next((i for i, w in enumerate(words_seen) if w & (1 << Bit.STOP)), None)
        moved_after = [w for w in words_seen[(stop_at or 0) + 1:] if w & (1 << Bit.MOVE_ABSOLUTE)]
        check(stop_at is not None and not moved_after, "a command that was being made when the stop latched is not written after the stop", str(words_seen))
        check(outcome and outcome[0][0] == rhal.SKR_EPERM and "latched" in outcome[0][1], "and its caller was told the stop took it, in so many words", str(outcome))
        bridge.release_stop()
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g, stop_mode="fast")
        ready(bridge)
        write(bridge, cmd(0.05))
        bridge.emergency_stop()
        until(lambda: g.error_code == frames.ERROR_FAST_STOP, 3.0)
        code, said = write(bridge, cmd(0.1))
        check(code == rhal.SKR_EPERM, "after a stop a command is refused")
        sent = len(g.commands)
        check(bridge.release_stop() == rhal.SKR_OK, "release_stop is the maker's acknowledge, and it takes")
        check(g.commands[-1]["word"] == (1 | 1 << Bit.ACKNOWLEDGE) and g.error_code == 0 and sent < len(g.commands), "which is control bit 2, with bit 0 high, and the module's error is gone")
        until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_ESTOP, 2.0)
        check(not state(bridge)[1].flags & rhal.FLAG_ESTOP and state(bridge)[1].fault_code == 0, "and the state reads clean")
        check(write(bridge, cmd(0.06))[0] == rhal.SKR_OK and g.commands[-1]["position"] == 60000, "a command is accepted again, even to the target the stop interrupted")
        bridge.close()
    with SimulatedGripper(start_ready=False, time_scale=SCALE) as g:
        g.sticky_error = 0x42
        bridge = opened(g)
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = bridge.release_stop()
        check(code == rhal.SKR_EIO and "0x42" in said.getvalue() and "still there" in said.getvalue(), "an acknowledge that did not clear the error is EIO, says the cause is still there, and names the code", said.getvalue())
        check(write(bridge, cmd(0.05))[0] == rhal.SKR_EPERM, "and the stop stays latched")
        g.sticky_error = None
        check(bridge.release_stop() == rhal.SKR_OK and write(bridge, cmd(0.05))[0] == rhal.SKR_OK, "until the cause is gone and the stop is released again")
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g)
        g.stop()
        time.sleep(0.1)
        check(bridge.emergency_stop() == rhal.SKR_OK, "a stop to a module that has gone away is still OK: the latch is what holds")
        bridge.close()
    check(SchunkBridge().emergency_stop() == rhal.SKR_OK, "and a stop to a bridge that was never opened is OK")

    # ── the motions ──────────────────────────────────────────────────────────
    section("the extension — status, param, move, grip, release")
    with SimulatedGripper(time_scale=SCALE, workpiece_um=40000) as g:
        bridge = opened(g)
        ready(bridge)
        code, status = ext(bridge, "status")
        check(code == rhal.SKR_OK and status["module"] == "EGU_60_M_B" and status["fieldbus"] == "EI" and status["serial"] == "NSNS0667" and "ready for operation" in status["status_bits"] and status["stop_latched"] is False,
              "status says what the module is, and what it reports")
        code, param = ext(bridge, "param name=max_pos")
        check(code == rhal.SKR_OK and param["value"] == 120.0 and param["param"] == "0x0608" and param["unit"] == "mm", "a parameter is read by name, in the module's own unit")
        check(ext(bridge, "param name=0x0600")[1]["value"] == 0.0, "or by its address")
        check(ext(bridge, "param name=nonsense")[0] == rhal.SKR_EINVAL and ext(bridge, "param")[0] == rhal.SKR_EINVAL, "an unknown parameter, or none, is EINVAL")
        code, moved = ext(bridge, "move position=0.08 speed=0.05")
        check(code == rhal.SKR_OK and abs(moved["position_m"] - 0.08) < 1e-6 and g.commands[-1]["speed"] == 50000, "a move waits for the jaw and answers where it is, at the speed it was given")
        check(ext(bridge, "move position=0.2")[0] == rhal.SKR_EINVAL and ext(bridge, "move")[0] == rhal.SKR_EINVAL and ext(bridge, "move position=0.05 speed=9")[0] == rhal.SKR_EINVAL, "a move outside the travel, with no target, or faster than the module can, is EINVAL")
        code, gripped = ext(bridge, "grip force=50")
        check(code == rhal.SKR_OK and gripped["result"] == "gripped" and abs(gripped["position_m"] - 0.04) < 1e-6, "a grip closes on the workpiece and says it is gripped, and where")
        check(g.commands[-1]["force"] == 50 and g.commands[-1]["word"] == (1 | 1 << Bit.GRIP_WORKPIECE), "with the force in the output frame and the grip bit, not the position one")
        check(ext(bridge, "grip force=0")[0] == rhal.SKR_EINVAL and ext(bridge, "grip force=9999")[0] == rhal.SKR_EINVAL and ext(bridge, "grip")[0] == rhal.SKR_EINVAL, "a force of nothing, past the module's own rating, or none is EINVAL")
        code, released = ext(bridge, "release")
        check(code == rhal.SKR_OK and released["position_m"] > 0.04, "a release opens the jaw by the module's own release distance")
        code, at = ext(bridge, "grip force=30 position=0.07")
        check(code == rhal.SKR_EINVAL or code == rhal.SKR_OK, "a grip at a position is asked for with its own bit")
        check(any(c["word"] & (1 << Bit.GRIP_AT_POSITION) for c in g.commands), "and that bit is 16")
        for text in ("fly", "move position", "status colour=red", ""):
            check(ext(bridge, text)[0] == rhal.SKR_EINVAL, f"{text!r} is EINVAL")
        check(bridge.extension("vendor.other", b"status")[0] == rhal.SKR_ENOSYS, "a token that is not this driver's is ENOSYS")
        bridge.emergency_stop()
        code, said = ext(bridge, "move position=0.05")
        check(code == rhal.SKR_EPERM and "latched" in str(said), "with the stop latched a motion is refused by name")
        check(ext(bridge, "status")[0] == rhal.SKR_OK and ext(bridge, "param name=max_pos")[0] == rhal.SKR_OK, "and only status and param are answered")
        bridge.release_stop()
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g)
        ready(bridge)
        code, result = ext(bridge, "grip force=50")
        check(code == rhal.SKR_OK and result["result"] == "no_workpiece" and abs(result["position_m"]) < 1e-6, "a grip with nothing in the jaws closes to the minimum and says there was no workpiece: an outcome and not an error")
        bridge.close()
    # F253: a command for the target that is already issued is not sent again (a stream at the control rate that asks for the same place says nothing new). A grip and a release move the jaw to somewhere the
    # driver did not choose, so after either the target "already issued" is no longer where the jaw is, and a stream that goes on asking for it must be sent.
    for motion in ("grip force=50", "release"):
        with SimulatedGripper(time_scale=SCALE, workpiece_um=40000) as g:
            bridge = opened(g)
            ready(bridge)
            write(bridge, cmd(0.08))
            until(lambda: abs(g.position_um - 80000) <= 1, 5.0)
            check(ext(bridge, motion)[0] == rhal.SKR_OK and abs(g.position_um - 80000) > 1000, f"after a move to 0.08 a `{motion}` takes the jaw somewhere else ({g.position_um} um)")
            posts = len([r for r in g.requests if r[0] == "POST"])
            code, said = write(bridge, cmd(0.08))
            check(code == rhal.SKR_OK and len([r for r in g.requests if r[0] == "POST"]) == posts + 2,
                  f"and a command for 0.08 m again is sent after the `{motion}`: the jaw is no longer there, and the target it was last given is not where it is", f"{len([r for r in g.requests if r[0] == 'POST']) - posts} frames")
            check(until(lambda: abs(g.position_um - 80000) <= 1, 5.0), "and the jaw goes back to it")
            check(ext(bridge, "status")[1]["target_m"] == 0.08, "and status says that target")
            bridge.close()
    with SimulatedGripper(time_scale=0.5, workpiece_um=40000) as g:
        bridge = opened(g, stop_mode="controlled")
        ready(bridge)
        g.place(120000)
        outcome = []
        worker = threading.Thread(target=lambda: outcome.append(ext(bridge, "grip force=50")), daemon=True)
        worker.start()
        check(until(lambda: g._motion is not None, 2.0), "a grip is moving the jaw")
        began = time.monotonic()
        bridge.emergency_stop()
        worker.join(3.0)
        check(not worker.is_alive() and time.monotonic() - began < 2.0 and outcome and outcome[0][0] == rhal.SKR_EPERM and "stop" in str(outcome[0][1]), "a stop latched while a motion runs ends it at once, saying the stop took it")
        check(until(lambda: g._motion is None, 2.0), "and the module's jaw is stopped")
        bridge.release_stop()
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        # A command that is waiting for the module's toggle when the stop lands: the module never takes it (`no_toggle`), so without the latch looked at inside the wait the command would stand there for the whole
        # of its budget and then say the line was slow. Found by the seeding run (`schunk-latch-wait-loop-unchecked`): the one place a stop could land that no check reached.
        bridge = opened(g)
        ready(bridge)
        g.no_toggle = True
        outcome: list[tuple[float, tuple[int, str]]] = []

        def command_and_note() -> None:
            result = write(bridge, cmd(0.05))
            outcome.append((time.monotonic(), result))

        posts = lambda: len([r for r in g.requests if r[0] == "POST"])  # noqa: E731
        base = posts()
        worker = threading.Thread(target=command_and_note, daemon=True)
        worker.start()
        # The stop lands once the module has been handed the command (the cleared word, then the command: two writes), so that what is being waited for is the toggle and nothing else.
        check(until(lambda: posts() >= base + 2, 3.0), "the module is handed the command, and never takes it")
        stopped_at = time.monotonic()
        bridge.emergency_stop()
        worker.join(2.0)
        check(not worker.is_alive() and outcome and outcome[0][1][0] == rhal.SKR_EPERM and "latched" in outcome[0][1][1] and outcome[0][0] - stopped_at < 0.19,
              f"a stop that lands while a command waits for the module's toggle ends the wait, saying it was latched and not that the line was slow ({(outcome[0][0] - stopped_at) * 1000 if outcome else float('nan'):.0f} ms after the stop)", str(outcome))
        g.no_toggle = False
        bridge.release_stop()
        bridge.close()

    # ── what the seeded-defect run found nobody was checking ────────────────────
    # 21 of the first 130 seeded defects passed this suite, the layout check and the conformance run (DRY-COVERAGE-PLAN.md 12.C.3). Each of these is one of them: the check that was missing, and the lever on the
    # simulator that makes the defect reachable (`error_on_command`, `refuse_writes`, `garble_reads`, `jammed`).
    section("the poller's failures, the latch, and what a module that refuses is taken to have said")
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g, stale_ms=20000)
        ready(bridge)
        g.drop = True
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT and bool(state(bridge)[1].flags & rhal.FLAG_DEGRADED), 1.5),
              "a poller whose requests are failing reads ETIMEDOUT and DEGRADED at once, though its last sample is younger than the staleness bound: a failure is not waited out")
        g.drop = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3.0), "and it reads clean when the module answers again")
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g)
        ready(bridge)
        write(bridge, cmd(0.05))
        until(lambda: g.position_um == 50000, 3.0)
        bridge.emergency_stop()
        for thread in list(bridge._stop_threads):
            thread.join(timeout=3.0)
        for what, command in (("a masked joint", cmd(0.0, mask=[0])), ("the target the module already has", cmd(0.05)), ("a position past the travel", cmd(0.5)), ("a NaN", cmd(math.nan))):
            check(write(bridge, command)[0] == rhal.SKR_EPERM, f"{what} is refused as latched while the stop holds: the latch is looked at before anything else")
        posts = len([r for r in g.requests if r[0] == "POST"])
        try:
            bridge._command({Bit.MOVE_ABSOLUTE: True}, position=1000, speed=1000)
            raised = None
        except BridgeRefusal as error:
            raised = error
        check(raised is not None and raised.code == rhal.SKR_EPERM and len([r for r in g.requests if r[0] == "POST"]) == posts,
              "the handshake looks at the latch before it writes anything: not even the cleared control word goes to the module")
        for text in ("move position=0.05", "grip force=50", "release"):
            code, body = ext(bridge, text)
            check(code == rhal.SKR_EPERM and "Only `status` and `param`" in str(body), f"`{text.split()[0]}` is refused at the extension while the stop holds, saying what is still answered", str(body))
        check(ext(bridge, "status")[0] == rhal.SKR_OK and ext(bridge, "param name=min_pos")[0] == rhal.SKR_OK, "and `status` and `param`, which move nothing, are still answered")
        g.place(80000)
        check(bridge.release_stop() == rhal.SKR_OK, "a stop is released by the acknowledge")
        sent = len(g.commands)
        check(write(bridge, cmd(0.05))[0] == rhal.SKR_OK and len(g.commands) == sent + 1,
              "and the target the module was last given is forgotten with it: the jaw is where the stop left it, and a command for the old place is sent and not taken for one already issued")
        bridge.emergency_stop()
        for thread in list(bridge._stop_threads):
            thread.join(timeout=3.0)
        bridge.close()
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            reopened = bridge.open(config(g))
        check(reopened == rhal.SKR_OK and ready(bridge) and write(bridge, cmd(0.07))[0] == rhal.SKR_OK, "a session opened after a stop is not born stopped: the latch is a session's, and a new one is commanded")
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g)
        ready(bridge)
        g.error_on_command = 0x42
        code, said = write(bridge, cmd(0.05))
        check(code == rhal.SKR_EPERM and "after the command" in said and "0x42" in said, "a command after which the module reports an error is refused, with the error, and not reported as done", said)
        g.error_on_command = None
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g)
        ready(bridge)
        g.reachable = False
        code, body = ext(bridge, "param name=min_pos")
        check(code == rhal.SKR_EIO and "503" in str(body), "a 503 from a module that has gone is a refusal that says 503, not an answer to read", str(body))
        g.reachable = True
        g.garble_reads = True
        code, body = ext(bridge, "param name=min_pos")
        check(code == rhal.SKR_EIO and "no instance" in str(body), "an answer that is not a list of hex text is refused, saying the instance was not there", str(body))
        g.garble_reads = False
        ready(bridge)
        g.refuse_writes = True
        code, said = write(bridge, cmd(0.07))
        check(code == rhal.SKR_EIO and "refused a write" in said, "a write the module answers with a non-zero result is refused, and not taken as written", said)
        g.refuse_writes = False
        bridge.close()
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g)
        ready(bridge)
        bridge._speed = 5000.0   # m/s: 5e9 um/s is more than the int32 the wire has for it
        code, said = write(bridge, cmd(0.07))
        check(code == rhal.SKR_EINVAL and said != "", "a command whose frame does not fit the wire is refused, with the reason, and not reported as sent", said)
        bridge.close()

    section("the extension's move, and a release that does not complete")
    with SimulatedGripper(time_scale=SCALE) as g:
        bridge = opened(g)
        ready(bridge)
        sent = len(g.commands)
        for what, text in (("past the top of the travel", "move position=0.5"), ("below the bottom", "move position=-0.1")):
            code, body = ext(bridge, text)
            check(code == rhal.SKR_EINVAL and "outside" in str(body) and len(g.commands) == sent, f"a move {what} is refused whole, and nothing is sent to the module", str(body))
        code, body = ext(bridge, "move position=0.05")
        check(code == rhal.SKR_OK and g.commands[-1]["speed"] == 20000, "a move that gives no speed goes at the configured one, 20000 um/s, and not at none")
        g.jammed = True
        code, body = ext(bridge, "release")
        check(code == rhal.SKR_EIO and "did not complete" in str(body), "a release that does not complete is EIO, saying so", str(body))
        g.jammed = False
        g.place(80000)
        sent = len(g.commands)
        check(write(bridge, cmd(0.05))[0] == rhal.SKR_OK and len(g.commands) == sent + 1,
              "and the target the module was last given is forgotten the moment it took the release, completed or not (F253): a command for the old place is sent")
        bridge.close()

    # ── close ────────────────────────────────────────────────────────────────
    section("close — it writes nothing")
    with SimulatedGripper(time_scale=0.5, workpiece_um=40000) as g:
        bridge = opened(g)
        ready(bridge)
        write(bridge, cmd(0.1))
        until(lambda: g._motion is not None, 2.0)
        posts = len([r for r in g.requests if r[0] == "POST"])
        check(bridge.close() == rhal.SKR_OK, "a close is OK with the jaw still moving")
        time.sleep(0.2)
        check(len([r for r in g.requests if r[0] == "POST"]) == posts and g._motion is not None, "and wrote nothing: the jaw is not stopped by it, and a held workpiece is not let go")
        check(bridge.write_command(cmd(0.05), 0) == rhal.SKR_EPERM, "a closed bridge takes no command")
        again = SchunkBridge()
        with contextlib.redirect_stderr(io.StringIO()):
            check(again.open(config(g)) == rhal.SKR_OK, "and it opens again on the same module")
        again.close()

    return finish()


def q_inst(form: object) -> str | None:
    if isinstance(form, dict):
        value = form.get("inst")
        return value[0] if isinstance(value, list) else value
    return None


def finish() -> int:
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
