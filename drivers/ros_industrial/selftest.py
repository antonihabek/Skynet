"""skynet-ros-industrial's own suite: the wire, and every refusal the bridge makes.

No test framework, `drivers/xarm/selftest.py`'s shape and emphasis: nearly everything asserted here is a **refusal**, each written beside the nearly identical input that succeeds. An arm driver's
failure mode is not a crash. It is a controller that acknowledges a target it will fault on, a stream with a gap in it, and a stop that is a line in a log, and a suite that only drove the happy
path would be checking the one thing that cannot go wrong quietly.

    make selftest      # this file, against the simulated controller
    make check         # and then the whole path through the real ABI

The bytes themselves are held to the maker's own library by `layout_check.py`; this file holds the driver to the protocol's behaviour, against a controller written here.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import socket
import sys
import threading
import time

import skynet_rhal as rhal

from skynet_ros_industrial import bridge as bridge_module
from skynet_ros_industrial import frames
from skynet_ros_industrial.bridge import RosIndustrialBridge
from skynet_ros_industrial.controller import (
    SIX_JOINT_LIMITS,
    SIX_JOINT_MAX_SPEED,
    SimulatedController,
)

FAILURES: list[str] = []
CHECKS = 0


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")


def section(title: str) -> None:
    print(f"\n{title}")


def until(predicate, timeout: float = 2.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.005)
    return bool(predicate())


ROLES = ("right_arm.shoulder.yaw", "right_arm.shoulder.pitch", "right_arm.elbow.flex", "right_arm.wrist.pitch", "right_arm.wrist.yaw", "right_arm.wrist.roll")


def joints(count: int = 6) -> list[dict]:
    return [{"name": f"joint_{i + 1}", "role": ROLES[i], "min_rad": SIX_JOINT_LIMITS[i][0], "max_rad": SIX_JOINT_LIMITS[i][1], "max_speed_rad_s": SIX_JOINT_MAX_SPEED}
            for i in range(count)]


def config(controller: SimulatedController, **extra) -> str:
    settings = {"host": "127.0.0.1", "ports": controller.ports, "simulator": True, "joints": joints(controller.joints), "byte_order": controller.format.byte_order,
                "real_bytes": controller.format.real_bytes}
    settings.update(extra)
    return json.dumps({k: v for k, v in settings.items() if v is not None})


def cmd(values: list[float], mask: list[int] | None = None, mode: int = rhal.MODE_POSITION) -> rhal.Command:
    return rhal.Command(stamp_ns=1, seq=1, mode=mode, values=values, mask=mask or [1] * len(values))


def opened(controller: SimulatedController, **extra) -> tuple[RosIndustrialBridge, str]:
    bridge = RosIndustrialBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(controller, **extra))
    assert code == rhal.SKR_OK, said.getvalue()
    return bridge, said.getvalue()


def refused(config_text: str, controller: SimulatedController | None = None) -> tuple[int, str]:
    """What `open` answers, and what it said, for a configuration."""
    bridge = RosIndustrialBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config_text)
    bridge.close()
    return code, said.getvalue()


def state(bridge: RosIndustrialBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    code = bridge.read_state(out, 0)
    return code, out


def write(bridge: RosIndustrialBridge, command: rhal.Command) -> tuple[int, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def main() -> int:
    print("skynet-ros-industrial selftest\n")

    # ── the wire ─────────────────────────────────────────────────────────────
    section("frames — the envelope")
    body = frames.joint_traj_pt(3, [0.1, -0.2], 0.25, 0.5)
    frame = frames.encode(frames.MsgType.JOINT_TRAJ_PT, frames.Comm.SERVICE_REQUEST, frames.Reply.INVALID, body)
    check(int.from_bytes(frame[:4], "little") == len(frame) - 4 == 12 + len(body), "a message's length counts its header and body and not itself")
    check(frame[4:16] == bytes.fromhex("0b000000 02000000 00000000".replace(" ", "")), "then the type, the comm type and the reply code, as ints")
    check(len(body) == 4 + 40 + 4 + 4, "and a trajectory point is a sequence, ten joints, a velocity and a duration: 52 bytes")
    for what, args in (
        ("a type of 0", (0, frames.Comm.TOPIC, frames.Reply.INVALID)),
        ("a comm type of 0", (frames.MsgType.PING, 0, frames.Reply.INVALID)),
        ("a service reply with no reply code", (frames.MsgType.PING, frames.Comm.SERVICE_REPLY, frames.Reply.INVALID)),
        ("a reply code on a topic", (frames.MsgType.STATUS, frames.Comm.TOPIC, frames.Reply.SUCCESS)),
    ):
        try:
            frames.encode(*args, b"")
            check(False, f"{what} is refused when a message is made")
        except frames.ProtocolError:
            check(True, f"{what} is refused when a message is made, as the maker's `validateMessage` refuses it")
    try:
        frames.encode(frames.MsgType.STATUS, frames.Comm.TOPIC, frames.Reply.INVALID, b"\0" * 1020)
        check(False, "a message that does not fit the maker's socket buffer is refused")
    except frames.ProtocolError:
        check(True, "a message that does not fit the maker's 1,024-byte socket buffer is refused")
    stream = frames.Stream()
    parts = stream.feed(frame[:7]) + stream.feed(frame[7:] + frame[:3]) + stream.feed(frame[3:])
    check(len(parts) == 2 and all(p.type == frames.MsgType.JOINT_TRAJ_PT and p.body == body for p in parts), "messages are cut out of a stream wherever the bytes fall")
    for what, raw in (("a length smaller than a header", (4).to_bytes(4, "little") + b"\0" * 8), ("a length past the maker's buffer", (5000).to_bytes(4, "little") + b"\0" * 8),
                      ("a negative length", (-1).to_bytes(4, "little", signed=True) + b"\0" * 8)):
        try:
            frames.Stream().feed(raw)
            check(False, f"{what} is refused")
        except frames.ProtocolError:
            check(True, f"{what} is refused, not resynchronised")
    try:
        frames.Stream(frames.Format(byte_order="big")).feed(frame)
        check(False, "a stream read in the wrong byte order is refused")
    except frames.ProtocolError as error:
        check("byte_order" in str(error), "a stream read in the wrong byte order is refused, and the refusal names the setting")
    try:
        frames.Stream().feed((12).to_bytes(4, "little") + (0).to_bytes(4, "little") + (1).to_bytes(4, "little") + (0).to_bytes(4, "little"))
        check(False, "a frame with an invalid type is refused when it is read")
    except frames.ProtocolError:
        check(True, "a frame with an invalid type is refused when it is read, as when it is made")
    for fmt in (frames.Format(byte_order="big"), frames.Format(real_bytes=8), frames.Format(byte_order="big", real_bytes=8)):
        point = frames.parse_joint_traj_pt(frames.joint_traj_pt(5, [0.5, -1.5, 2.5], 0.5, 0.125, fmt), fmt)
        check(point.seq == 5 and point.joints[:3] == (0.5, -1.5, 2.5) and point.velocity == 0.5 and point.duration == 0.125, f"a point survives {fmt.byte_order}-endian, {fmt.real_bytes}-byte reals")
    try:
        frames.joint_traj_pt(0, [0.0] * 11, 0.1, 0.1)
        check(False, "eleven joints do not fit a message")
    except frames.ProtocolError:
        check(True, "eleven joints do not fit a message of ten")
    for what, args in (("a NaN joint", ([math.nan], 0.1, 0.1)), ("an infinite velocity", ([0.0], math.inf, 0.1)), ("a NaN duration", ([0.0], 0.1, math.nan))):
        try:
            frames.joint_traj_pt(0, *args)
            check(False, f"{what} is refused when a point is made")
        except frames.ProtocolError:
            check(True, f"{what} is refused when a point is made")
    try:
        frames.parse_joint_traj_pt(body[:-1])
        check(False, "a short point body is refused")
    except frames.ProtocolError:
        check(True, "a short point body is refused")
    try:
        frames.parse_joint_feedback(b"\0" * 10)
        check(False, "a short feedback body is refused")
    except frames.ProtocolError:
        check(True, "a short feedback body is refused")

    # ── open: what it refuses ────────────────────────────────────────────────
    check((frames.Port.MOTION, frames.Port.STATE) == (11000, 11002) and bridge_module._DEFAULT_PORTS == {"motion": 11000, "state": 11002}, "the default ports are the maker's: the motion server on 11000 and the state server on 11002")
    section("open — what the configuration must say, and what the controller must say")
    with SimulatedController() as controller:
        good = json.loads(config(controller))
        check(refused(json.dumps(good))[0] == rhal.SKR_OK, "the configuration that is complete opens")
        for what, mutate, words in (
            ("a configuration with no host", lambda c: c.pop("host"), "host"),
            ("a configuration with no `simulator`", lambda c: c.pop("simulator"), "simulator"),
            ("a `simulator` that is not true or false", lambda c: c.update(simulator="yes"), "simulator"),
            ("a configuration with no joints", lambda c: c.pop("joints"), "joints"),
            ("an empty joint list", lambda c: c.update(joints=[]), "joints"),
            ("eleven joints", lambda c: c.update(joints=joints(6) * 2), "10"),
            ("a joint with no name", lambda c: c["joints"][2].pop("name"), "name"),
            ("a joint with no lower limit", lambda c: c["joints"][1].pop("min_rad"), "min_rad"),
            ("a joint with no upper limit", lambda c: c["joints"][1].pop("max_rad"), "max_rad"),
            ("a joint with no speed ceiling", lambda c: c["joints"][1].pop("max_speed_rad_s"), "max_speed_rad_s"),
            ("a joint whose range is not a range", lambda c: c["joints"][0].update(min_rad=1.0, max_rad=1.0), "not below"),
            ("a joint whose speed ceiling is zero", lambda c: c["joints"][0].update(max_speed_rad_s=0), "not above zero"),
            ("a joint whose limit is not a number", lambda c: c["joints"][0].update(max_rad="3"), "max_rad"),
            ("a byte order that is neither", lambda c: c.update(byte_order="middle"), "byte_order"),
            ("a real that is neither four bytes nor eight", lambda c: c.update(real_bytes=2), "real_bytes"),
            ("a key the driver does not read", lambda c: c.update(speed=1.0), "speed"),
            ("a `rate_hz` of nothing", lambda c: c.update(rate_hz=0), "rate_hz"),
            ("a `ping` that is not a boolean", lambda c: c.update(ping="no"), "ping"),
        ):
            changed = json.loads(json.dumps(good))
            mutate(changed)
            code, said = refused(json.dumps(changed))
            check(code == rhal.SKR_ENODEV and words in said, f"{what} is refused, and the refusal says so")
        check(refused("{not json")[0] == rhal.SKR_EINVAL, "a configuration that is not JSON is EINVAL")
        check(refused("[1]")[0] == rhal.SKR_EINVAL, "and so is one that is not an object")

    with SimulatedController() as controller:
        code, said = refused(json.dumps({**json.loads(config(controller)), "ports": {"motion": 1, "state": controller.ports["state"]}}))
        check(code == rhal.SKR_EIO and "motion" in said, "a motion port nobody answers on is a transport failure (EIO), not a refusal of the body")
        code, said = refused(json.dumps({**json.loads(config(controller)), "ports": {"motion": controller.ports["motion"], "state": 1}}))
        check(code == rhal.SKR_EIO and "state" in said, "and so is a state port")
        controller.silence()
        code, said = refused(config(controller, open_timeout_ms=300))
        check(code == rhal.SKR_EIO and "no joint feedback and status" in said, "a controller whose state server is silent is a body with no state, and open says so")
        controller.silence(False)
        controller.answer_ping = False
        code, said = refused(config(controller, open_timeout_ms=300))
        check(code == rhal.SKR_EIO and "ping" in said, "a motion server that does not answer a ping is refused, with the way to say that is expected")
        check(refused(config(controller, ping=False))[0] == rhal.SKR_OK, "and `ping: false` opens it")
        controller.answer_ping = True
        controller.ping_delay_s = 0.6
        code, said = refused(config(controller, ping_timeout_ms=200))
        check(code == rhal.SKR_EIO and "ping_timeout_ms" in said, "a ping answered late is refused when the wait is short, and the refusal names the setting")
        check(refused(config(controller))[0] == rhal.SKR_OK and bridge_module._PING_TIMEOUT_S > 5.0, "and opens with the default wait, which is longer than the five seconds the maker's own server is slow to accept the next client for (F241)")
        controller.ping_delay_s = 0.0
        check(refused(config(controller, ping_timeout_ms=10))[0] == rhal.SKR_ENODEV, "a ping wait under fifty milliseconds is refused as a setting")

    with SimulatedController() as controller:
        controller.feedback_valid = frames.Valid.TIME
        time.sleep(0.1)
        code, said = refused(config(controller, open_timeout_ms=300))
        check(code == rhal.SKR_EIO, "feedback whose positions the controller marks invalid is not positions: with nothing else the controller has no state, and open says so")
        controller.feedback_valid = frames.Valid.TIME | frames.Valid.POSITION
        time.sleep(0.1)
        bridge, _ = opened(controller)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        check(all(math.isnan(v) for v in state(bridge)[1].velocity_rad_s), "feedback that does not mark its velocities valid has NaN velocities: the arm at rest is not a claim the controller made")
        bridge.close()
    with SimulatedController() as controller:
        controller.status_silent = True
        code, said = refused(config(controller, open_timeout_ms=300))
        check(code == rhal.SKR_EIO and "status" in said, "a controller that sends feedback and no status is refused: a body whose stop and fault state it cannot read is not one to move")
    with SimulatedController() as controller:
        controller.error_flag = False
        controller.fault(77)
        time.sleep(0.15)
        code, said = refused(config(controller))
        check(code == rhal.SKR_ENODEV and "77" in said, "an error code with no `in_error` flag is an error: the code is read as well as the flag")
    with SimulatedController() as controller:
        controller.ping_reply = frames.Reply.FAILURE
        code, said = refused(config(controller, ping_timeout_ms=300))
        check(code == rhal.SKR_EIO and "ping" in said, "a ping the controller answers with FAILURE is not an answer")
    for what, act, words in (
        ("an e-stopped controller", lambda c: c.estop(), "e-stopped"),
        ("a controller holding an error", lambda c: c.fault(4021), "4021"),
        ("a controller with its drives off", lambda c: c.drives(False), "drives are not powered"),
    ):
        with SimulatedController() as controller:
            act(controller)
            time.sleep(0.15)
            code, said = refused(config(controller))
            check(code == rhal.SKR_ENODEV and words in said, f"{what} is refused at open, and the refusal says why")
    with SimulatedController() as controller:
        controller.motion_possible = False
        time.sleep(0.15)
        code, said = refused(config(controller))
        check(code == rhal.SKR_ENODEV and "motion is not possible" in said, "a controller that says motion is not possible (a pendant in manual, a protective stop) is refused at open, whatever else is healthy")
    with SimulatedController() as controller:
        raw = socket.create_connection(("127.0.0.1", controller.ports["motion"]))
        raw.sendall(frames.encode(frames.MsgType.JOINT_TRAJ_PT, frames.Comm.SERVICE_REQUEST, frames.Reply.INVALID, frames.joint_traj_pt(0, [0.5, 0, 0, 0, 0, 0], 0.1, 5.0)))
        until(controller.in_motion)
        time.sleep(0.2)
        code, said = refused(config(controller))
        check(code == rhal.SKR_ENODEV and "moving" in said, "an arm that somebody else is moving is refused at open: two masters is two setpoint streams into one arm")
        raw.close()
    with SimulatedController() as controller:
        controller.unknown = {"drives_powered", "motion_possible"}
        time.sleep(0.25)
        bridge, said = opened(controller)
        check("unknown" in said and "drives_powered" in said, "status fields the controller says it does not know open, and are said aloud and not read as healthy")
        bridge.close()
    with SimulatedController() as controller:
        controller.fault(1)
        controller.clear_error()
        time.sleep(0.15)
        bridge, _ = opened(controller)
        check(True, "an error that was cleared at the controller opens")
        bridge.close()

    # ── the descriptor ───────────────────────────────────────────────────────
    section("the descriptor — what the configuration asserted, and nothing it did not")
    with SimulatedController() as controller:
        bridge, _ = opened(controller, robot="motoman-gp7", rate_hz=20)
        descriptor = json.loads(bridge.describe())
        check(descriptor["robot_slug"] == "motoman-gp7" and descriptor["dof"] == 6 and len(descriptor["joints"]) == 6 and descriptor["base"] == "fixed", "the slug, the joint count and the base are the configuration's")
        first = descriptor["joints"][0]
        check(first["limits"] == {"position_min_rad": SIX_JOINT_LIMITS[0][0], "position_max_rad": SIX_JOINT_LIMITS[0][1], "velocity_max_rad_s": SIX_JOINT_MAX_SPEED}
              and first["role"] == "right_arm.shoulder.yaw" and first["control_modes"] == ["position"], "each joint's range, speed ceiling and role are the configuration's own, and it takes only positions")
        check(descriptor["simulator"] is True and descriptor["envelope"] is None and descriptor["control"]["rate_hz_max"] == 20.0, "the simulator flag is the configuration's, no envelope is invented, and the rate is the one asked for")
        check("safety.estop" in descriptor["capabilities"], "and it declares the software stop it has")
        bridge.close()
        bridge, _ = opened(controller, simulator=False)
        check(json.loads(bridge.describe())["simulator"] is False, "`simulator: false` is believed when it is said")
        bridge.close()
        check(RosIndustrialBridge().describe() == "{}", "a bridge that was never opened describes nothing")

    # ── state ────────────────────────────────────────────────────────────────
    section("read_state — what the controller says, stamped when it arrived")
    with SimulatedController() as controller:
        controller.place([0.1, -0.2, 0.3, -0.4, 0.5, -0.6])
        bridge, _ = opened(controller)
        until(lambda: state(bridge)[1].position_rad and abs(state(bridge)[1].position_rad[0] - 0.1) < 1e-4)
        code, first = state(bridge)
        check(code == rhal.SKR_OK and len(first.position_rad) == 6 and all(abs(a - b) < 1e-4 for a, b in zip(first.position_rad, [0.1, -0.2, 0.3, -0.4, 0.5, -0.6])),
              "the positions are the controller's own, the first six of the ten in a message")
        check(len(first.velocity_rad_s) == 6 and not any(math.isnan(v) for v in first.velocity_rad_s), "the velocities are read when the feedback says they are valid")
        check(all(math.isnan(e) for e in first.effort_nm), "no effort is invented: no `simple_message` carries one (NaN, §24)")
        check(first.flags == rhal.HOMING_NOT_REQUIRED and first.fault_code == 0, "a healthy arm reads clean")
        time.sleep(0.05)
        code, second = state(bridge)
        check(second.seq == first.seq + 1 and second.stamp_ns >= first.stamp_ns, "each read has the next sequence number and a stamp that does not go back")
        controller.fault(4021)
        until(lambda: state(bridge)[1].fault_code == 4021)
        code, faulted = state(bridge)
        check(faulted.fault_code == 4021 and faulted.flags & rhal.FLAG_FAULT and faulted.flags & rhal.FLAG_DEGRADED, "an error is FAULT and DEGRADED, and the controller's code is the fault word")
        controller.clear_error()
        until(lambda: state(bridge)[1].fault_code == 0)
        check(state(bridge)[1].fault_code == 0 and not state(bridge)[1].flags & rhal.FLAG_FAULT, "and it is not sticky: the driver reads what the controller says now")
        controller.estop()
        until(lambda: state(bridge)[1].flags & rhal.FLAG_ESTOP)
        check(bool(state(bridge)[1].flags & rhal.FLAG_ESTOP), "the controller's own e-stop is ESTOP")
        controller.estop(False)
        controller.silence()
        time.sleep(0.8)
        code, stale = state(bridge)
        check(code == rhal.SKR_ETIMEDOUT and stale.flags & rhal.FLAG_DEGRADED, "a controller that has gone quiet is ETIMEDOUT and DEGRADED: silence is not stillness")
        check(0 < stale.stamp_ns < rhal.acquisition_ns() - 400_000_000, "and the stamp is still the sample's own, not the read's: it is the age of the last thing the controller said")
        controller.silence(False)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        check(state(bridge)[0] == rhal.SKR_OK, "and it recovers when the stream does")
        bridge.close()
        check(state(bridge)[0] == rhal.SKR_EPERM, "a closed bridge reads EPERM")
    with SimulatedController(fmt=frames.Format(byte_order="big", real_bytes=8)) as controller:
        controller.place([0.3] * 6)
        bridge, _ = opened(controller)
        until(lambda: abs(state(bridge)[1].position_rad[0] - 0.3) < 1e-9 if state(bridge)[1].position_rad else False)
        check(abs(state(bridge)[1].position_rad[0] - 0.3) < 1e-9, "a big-endian controller with 8-byte reals is read in its own spelling")
        bridge.close()
    with SimulatedController() as controller:
        controller.older_message = True
        controller.place([0.25] * 6)
        bridge, _ = opened(controller)
        until(lambda: abs(state(bridge)[1].position_rad[0] - 0.25) < 1e-4 if state(bridge)[1].position_rad else False)
        code, older = state(bridge)
        check(code == rhal.SKR_OK and abs(older.position_rad[0] - 0.25) < 1e-4 and all(math.isnan(v) for v in older.velocity_rad_s),
              "a controller old enough to send only `JOINT` is read, and its velocities are NaN: it did not say it measured them")
        bridge.close()
    with SimulatedController() as controller:
        bridge, _ = opened(controller)
        controller.stop()
        until(lambda: state(bridge)[0] != rhal.SKR_OK, 2.0)
        code, gone = state(bridge)
        check(code == rhal.SKR_ETIMEDOUT and gone.flags & rhal.FLAG_DEGRADED, "a controller that disappears is reported as one, not as an arm at rest")
        bridge.close()

    # ── commands ─────────────────────────────────────────────────────────────
    section("write_command — the point that goes out, and what is refused before it does")
    with SimulatedController() as controller:
        controller.place([0.0] * 6)
        bridge, _ = opened(controller, rate_hz=10)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        sent = len(controller.points)
        check(write(bridge, cmd([0.01, 0.0, 0.0, 0.0, 0.0, 0.0]))[0] == rhal.SKR_OK, "a command inside every range and speed is accepted")
        until(lambda: len(controller.points) > sent)
        point = controller.points[-1]
        check(point.seq == 0 and abs(point.joints[0] - 0.01) < 1e-6 and abs(point.duration - 0.1) < 1e-6, "it is a trajectory point at sequence 0, with the targets, and the duration the rate gives it")
        check(0.0 < point.velocity <= 1.0 and abs(point.velocity - 0.01 / (0.1 * SIX_JOINT_MAX_SPEED)) < 1e-4, "and a velocity ratio: the largest step over what the duration and the speed ceiling allow, as the maker's own client works it out")
        for expected in (1, 2, 3):
            write(bridge, cmd([0.01 * (expected + 1), 0.0, 0.0, 0.0, 0.0, 0.0]))
        until(lambda: len(controller.points) >= sent + 4)
        check([p.seq for p in controller.points[sent:]] == [0, 1, 2, 3], "the next point is the previous plus one")
        mark = len(controller.points)
        write(bridge, cmd([0.05, 0, 0, 0, 0, 0]))
        write(bridge, cmd([0.05, 0, 0, 0, 0, 0]))
        until(lambda: len(controller.points) >= mark + 2)
        check(controller.points[-1].velocity >= 0.0099 and abs(controller.points[-1].joints[0] - 0.05) < 1e-6, "a hold (the same setpoint again) is sent with a velocity ratio above zero: a ratio of exactly zero is one controllers read as their default speed")
        count = len(controller.points)
        for what, values, mask, mode in (
            ("a setpoint above a joint's range", [3.0, 0.0, 0.0, 0.0, 0.0, 0.0], None, rhal.MODE_POSITION),
            ("a setpoint below a joint's range", [0.0, -2.0, 0.0, 0.0, 0.0, 0.0], None, rhal.MODE_POSITION),
            ("a NaN setpoint", [math.nan, 0.0, 0.0, 0.0, 0.0, 0.0], None, rhal.MODE_POSITION),
            ("an infinite setpoint", [math.inf, 0.0, 0.0, 0.0, 0.0, 0.0], None, rhal.MODE_POSITION),
            ("a step no joint can make in the time given", [0.9, 0.0, 0.0, 0.0, 0.0, 0.0], None, rhal.MODE_POSITION),
            ("a velocity command", [0.0] * 6, None, rhal.MODE_VELOCITY),
            ("a torque command", [0.0] * 6, None, rhal.MODE_TORQUE),
            ("five joints for a six-joint arm", [0.0] * 5, None, rhal.MODE_POSITION),
            ("seven joints for a six-joint arm", [0.0] * 7, None, rhal.MODE_POSITION),
        ):
            code, _ = write(bridge, cmd(values, mask, mode))
            check(code == rhal.SKR_EINVAL, f"{what} is EINVAL")
        time.sleep(0.1)
        check(len(controller.points) == count, "and none of them put a byte on the wire: the controller is never asked to fault on a target the driver could have refused")
        before = len(controller.points)
        check(write(bridge, cmd([0.05, 99.0, 0.0, 0.0, 0.0, 0.0], [1, 0, 1, 1, 1, 1]))[0] == rhal.SKR_OK, "a masked joint's value is not read, so a bad value on it is not a refusal")
        until(lambda: len(controller.points) > before)
        check(abs(controller.points[-1].joints[1]) < 1e-9 and abs(controller.points[-1].joints[0] - 0.05) < 1e-6, "and it holds the setpoint this driver last issued for it")
        bridge.close()

    with SimulatedController() as controller:
        controller.place([0.0] * 6)
        bridge, _ = opened(controller, rate_hz=10, point_duration_s=10.0)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        count = len(controller.points)
        check(write(bridge, cmd([2.95, 0, 0, 0, 0, 0]))[0] == rhal.SKR_EINVAL, "a setpoint just above a joint's range is refused even when the point's duration is long enough for any step: it is the range that refuses it, and not the speed")
        check(write(bridge, cmd([0, -1.95, 0, 0, 0, 0]))[0] == rhal.SKR_EINVAL, "and one just below it")
        check(write(bridge, cmd([2.85, -1.85, 0, 0, 0, 0]))[0] == rhal.SKR_OK, "and the same distance inside both ranges is accepted")
        time.sleep(0.1)
        check(len(controller.points) == count + 1, "so exactly one of the three went out")
        bridge.close()

    with SimulatedController() as controller:
        controller.place([0.0] * 6)
        bridge, _ = opened(controller, rate_hz=10, point_duration_s=0.5)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        check(write(bridge, cmd([0.4, 0.0, 0.0, 0.0, 0.0, 0.0]))[0] == rhal.SKR_OK, "a step 0.4 rad in half a second is within a 3 rad/s joint")
        until(lambda: controller.points)
        check(abs(controller.points[-1].duration - 0.5) < 1e-6, "`point_duration_s` is the point's duration")
        check(until(lambda: abs(controller.positions()[0] - 0.4) < 0.01, 2.0), "and the controller's arm goes where it was sent")
        check(until(lambda: abs(state(bridge)[1].position_rad[0] - 0.4) < 0.01), "and the driver reads it there")
        bridge.close()

    with SimulatedController(queue_depth=2) as controller:
        controller.place([0.0] * 6)
        bridge, _ = opened(controller, rate_hz=10, point_duration_s=1.0)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        codes, told = [], ""
        for i in range(10):
            code, said = write(bridge, cmd([0.02 * (i + 1), 0, 0, 0, 0, 0]))
            codes.append(code)
            told += said
            time.sleep(0.03)
        check(rhal.SKR_EIO in codes and "FAILURE" in told, "a point the controller answered with FAILURE (its queue is full) is told to the next command, which refuses")
        controller.queue_depth = 100
        time.sleep(0.2)
        seqs = [p.seq for p in controller.points]
        failed_at = next(i for i, p in enumerate(controller.points) if i >= 3 and p.seq == 3) if 3 in seqs else len(seqs)
        mark = len(controller.points)
        check(write(bridge, cmd([0.02, 0, 0, 0, 0, 0]))[0] == rhal.SKR_OK, "and the one after it is accepted")
        until(lambda: len(controller.points) > mark)
        check(controller.points[-1].seq == 0 and failed_at < mark, "as the start of a new trajectory: sequence 0, because the old one has a gap")
        bridge.close()

    with SimulatedController() as controller:
        controller.place([0.0] * 6)
        bridge, _ = opened(controller, rate_hz=10)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        controller.replies_withheld = 100
        codes = [write(bridge, cmd([0.001 * (i + 1), 0, 0, 0, 0, 0]))[0] for i in range(10)]
        check(codes[:8] == [rhal.SKR_OK] * 8 and rhal.SKR_EIO in codes[8:], "a controller that stops answering is not queued behind for ever: after eight unanswered points the driver says so")
        bridge.close()

    with SimulatedController() as controller:
        controller.place([0.0] * 6)
        bridge, _ = opened(controller)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        controller.fault(4021)
        until(lambda: state(bridge)[1].fault_code == 4021)
        check(write(bridge, cmd([0.01, 0, 0, 0, 0, 0]))[0] == rhal.SKR_EPERM, "a controller holding an error takes no command: EPERM, and nothing is sent")
        controller.clear_error()
        until(lambda: state(bridge)[1].fault_code == 0)
        controller.drives(False)
        until(lambda: write(bridge, cmd([0.01, 0, 0, 0, 0, 0]))[0] == rhal.SKR_EPERM)
        check(write(bridge, cmd([0.01, 0, 0, 0, 0, 0]))[0] == rhal.SKR_EPERM, "and neither does one whose drives went off")
        bridge.close()

    with SimulatedController() as controller:
        controller.place([0.0] * 6)
        controller.motion_possible = True  # a controller that says motion is possible while it holds an error or a stop: only the flags it also sends say otherwise
        bridge, _ = opened(controller)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        controller.estop()
        until(lambda: state(bridge)[1].flags & rhal.FLAG_ESTOP)
        check(write(bridge, cmd([0.01, 0, 0, 0, 0, 0]))[0] == rhal.SKR_EPERM, "a controller that says it is e-stopped takes no command, whatever it says about motion being possible")
        controller.estop(False)
        controller.error_flag = True
        controller.fault(5)
        until(lambda: state(bridge)[1].fault_code == 5)
        check(write(bridge, cmd([0.01, 0, 0, 0, 0, 0]))[0] == rhal.SKR_EPERM, "and neither does one that holds an error, whatever it says about motion")
        controller.clear_error()
        controller.error_flag = False
        controller.fault(6)
        until(lambda: state(bridge)[1].fault_code == 6)
        check(write(bridge, cmd([0.01, 0, 0, 0, 0, 0]))[0] == rhal.SKR_EPERM, "or one that holds an error code and sets no flag: the code is read as well as the flag")
        bridge.close()

    with SimulatedController() as controller:
        bridge, _ = opened(controller)
        controller.drop_motion_clients()
        until(lambda: write(bridge, cmd([0.0] * 6))[0] == rhal.SKR_EIO, 2.0)
        check(write(bridge, cmd([0.0] * 6))[0] == rhal.SKR_EIO, "a motion connection the controller dropped is EIO on the next command, not a command that was 'sent'")
        bridge.close()

    # ── the stop ─────────────────────────────────────────────────────────────
    section("the stop — latched, on the wire, and never waiting for a command")
    with SimulatedController() as controller:
        controller.place([0.0] * 6)
        bridge, _ = opened(controller, rate_hz=10)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        write(bridge, cmd([0.2, 0, 0, 0, 0, 0]))
        until(lambda: controller.points)
        began = time.monotonic()
        check(bridge.emergency_stop() == rhal.SKR_OK, "the stop returns OK")
        check(time.monotonic() - began < 0.05, f"in {(time.monotonic() - began) * 1000:.1f} ms")
        check(until(lambda: controller.stops == 1), "and it reached the controller as a STOP_TRAJECTORY point")
        stop_point = [p for p in controller.points if p.seq == frames.Seq.STOP_TRAJECTORY][-1]
        check(abs(stop_point.joints[0] - 0.2) < 1e-6 and stop_point.velocity == 0.0, "carrying the arm's own positions and no speed: a server that read it as a point would send the arm where it already is")
        check(not controller.in_motion(), "the controller flushed its queue and stopped")
        code, held = state(bridge)
        check(held.flags & rhal.FLAG_ESTOP, "the state says stopped")
        count = len(controller.points)
        check(write(bridge, cmd([0.1, 0, 0, 0, 0, 0]))[0] == rhal.SKR_EPERM, "a command after the stop is EPERM")
        time.sleep(0.1)
        check(len(controller.points) == count, "and puts nothing on the wire")
        check(bridge.release_stop() == rhal.SKR_OK, "release_stop clears the latch when the controller is healthy")
        check(not state(bridge)[1].flags & rhal.FLAG_ESTOP, "and the state no longer says stopped")
        mark = len(controller.points)
        check(write(bridge, cmd([0.05, 0, 0, 0, 0, 0]))[0] == rhal.SKR_OK, "a command is accepted again")
        until(lambda: len(controller.points) > mark)
        check(controller.points[-1].seq == 0, "as the start of a new trajectory, from where the arm is")
        bridge.close()

    with SimulatedController() as controller:
        controller.place([0.0] * 6)
        bridge, _ = opened(controller, rate_hz=10, point_duration_s=0.5)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        write(bridge, cmd([0.4, 0, 0, 0, 0, 0]))
        until(lambda: controller.in_motion())
        time.sleep(0.05)
        bridge.emergency_stop()
        until(lambda: not controller.in_motion())
        bridge.release_stop()
        code, _ = write(bridge, cmd([1.7, 0, 0, 0, 0, 0]))
        check(code == rhal.SKR_EINVAL, "after a release the next step is worked out from where the arm is, not from where the loop last asked: 1.7 rad is too far in half a second from where it stopped, and not from the 0.4 it was going to")
        bridge.close()

    with SimulatedController() as controller:
        bridge, _ = opened(controller)
        bridge.emergency_stop()
        controller.fault(4021)
        until(lambda: state(bridge)[1].fault_code == 4021)
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = bridge.release_stop()
        check(code == rhal.SKR_EIO and "4021" in said.getvalue() and state(bridge)[1].flags & rhal.FLAG_ESTOP, "an error the controller holds is not cleared by a release: EIO, the stop stays latched, and the code is named")
        bridge.close()

    with SimulatedController() as controller:
        bridge, _ = opened(controller)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        link = bridge._motion
        assert link is not None
        link.send_lock.acquire()
        began = time.monotonic()
        result = bridge.emergency_stop()
        waited = time.monotonic() - began
        link.send_lock.release()
        check(result == rhal.SKR_OK and waited < 0.2, f"with a send stuck on the motion socket the stop still returns, in {waited * 1000:.0f} ms")
        check(link._closing is True and link.sock.fileno() == -1, "and drops the connection: with the socket blocked, that is the only stop left")
        check(write(bridge, cmd([0.0] * 6))[0] == rhal.SKR_EPERM, "and the latch holds")
        bridge.close()

    with SimulatedController() as controller:
        bridge, _ = opened(controller)
        controller.stop()
        time.sleep(0.1)
        check(bridge.emergency_stop() == rhal.SKR_OK, "a stop to a controller that has gone away is still OK: the latch is what holds")
        bridge.close()
    check(RosIndustrialBridge().emergency_stop() == rhal.SKR_OK, "and a stop to a bridge that was never opened is OK")

    with SimulatedController() as controller:
        controller.place([0.0] * 6)
        bridge, _ = opened(controller, rate_hz=100, point_duration_s=0.05)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        running = threading.Event()
        stop_at: list[float] = []

        def stream() -> None:
            i = 0
            while not stop_at or time.monotonic() < stop_at[0] + 0.3:
                running.set()
                i += 1
                write(bridge, cmd([0.0002 * (i % 100), 0, 0, 0, 0, 0]))
                time.sleep(0.005)

        worker = threading.Thread(target=stream)
        worker.start()
        running.wait(2.0)
        time.sleep(0.1)
        stop_at.append(time.monotonic())
        bridge.emergency_stop()
        worker.join()
        stops = [i for i, p in enumerate(controller.points) if p.seq == frames.Seq.STOP_TRAJECTORY]
        after = [p for p in controller.points[stops[0] + 1:] if p.seq >= 0] if stops else [None]
        check(len(stops) >= 1 and not after, "a stop in the middle of a stream is the last thing on the wire: no point follows it, whatever the stream was doing")
        bridge.close()

    # ── close, and a session ─────────────────────────────────────────────────
    section("close and reopen — a stop belongs to a session")
    with SimulatedController() as controller:
        bridge, _ = opened(controller)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        bridge.close()
        check(until(lambda: controller.stops >= 1), "closing a session ends it with a stop trajectory: the arm is not left taking a stream")
        check(write(bridge, cmd([0.0] * 6))[0] == rhal.SKR_EPERM, "and a closed bridge takes no command")
        bridge.emergency_stop()
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = bridge.open(config(controller))
        check(code == rhal.SKR_OK, "it opens again")
        check(not (state(bridge)[1].flags & rhal.FLAG_ESTOP), "and the stop latched before the reopen did not survive it (DRY-COVERAGE-PLAN.md F73)")
        check(bridge.read_sensor("camera", rhal.Frame()) == rhal.SKR_ENOSYS, "a sensor is ENOSYS: the protocol has none")
        bridge.close()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
