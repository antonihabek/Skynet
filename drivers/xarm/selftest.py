"""skynet-xarm's own suite: the wire, and every refusal the bridge makes.

No test framework, `drivers/canopen/selftest.py`'s shape and emphasis: nearly everything asserted here
is a **refusal**, each written beside the nearly identical input that succeeds. An arm driver's failure
mode is not a crash. It is a controller that acknowledges a target it is about to fault on, and a suite
that only drove the happy path would be checking the one thing that cannot go wrong quietly.

    make selftest                          # this file, against the simulated controller
    make check                             # and then the whole path through the real ABI
    python3 selftest.py --live <host>      # the live block, against a real controller's firmware
                                           # (`make check-sim-docker` runs it against UFACTORY's)
"""

from __future__ import annotations

import json
import math
import os
import struct
import sys
import threading
import time

import skynet_rhal as rhal

from skynet_xarm import bridge as bridge_module
from skynet_xarm import frames
from skynet_xarm.bridge import XArmBridge
from skynet_xarm.controller import XARM6_LIMITS, SimulatedController

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


def config(controller: SimulatedController, count: int | None = None, **extra) -> str:
    count = count if count is not None else controller.axis
    return json.dumps(
        {
            "host": "127.0.0.1",
            "ports": controller.ports,
            "joints": [{"name": f"joint{i + 1}", "role": None} for i in range(count)],
            **extra,
        }
    )


def heard(controller: SimulatedController, function: int) -> list[bytes]:
    with controller._lock:
        return [data for fun, data in controller.log if fun == function]


def servoj_angles(data: bytes) -> tuple[float, ...]:
    return struct.unpack("<10f", data)[:7]


def cmd(values: list[float], mask: list[int] | None = None) -> rhal.Command:
    return rhal.Command(
        stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=values, mask=mask or [1] * len(values)
    )


def main() -> int:
    if "--live" in sys.argv[1:]:
        at = sys.argv.index("--live")
        # `--live HOST [AXIS TYPE]`: the arm the firmware was started as (`xarm_start.sh AXIS TYPE`), default 6 6.
        rest = sys.argv[at + 1 :]
        live_checks(rest[0], int(rest[1]) if len(rest) > 1 else 6, int(rest[2]) if len(rest) > 2 else 6)
        return finish()

    print("skynet-xarm selftest\n")

    # ── the wire ─────────────────────────────────────────────────────────────
    section("frames — the controller's wire")

    # The vendor's own framing of `set_state(4)`: transaction, protocol 2, length = data + 1, register.
    stop = frames.request(1, frames.Fun.SET_STATE, bytes([frames.State.STOP]))
    check(stop == bytes.fromhex("0001 0002 0002 0c 04".replace(" ", "")), "a request is framed as the vendor's SDK frames it")
    check(
        frames.request(0xFFFF, frames.Fun.GET_STATE)[:6] == bytes.fromhex("ffff00020001"),
        "and an empty request has a length of one: the register byte",
    )
    try:
        frames.request(70000, 1)
        check(False, "a transaction id that does not fit is refused")
    except frames.ProtocolError:
        check(True, "a transaction id that does not fit is refused")

    reply = frames.parse_reply(bytes.fromhex("000700020003"), bytes([13, 0x10, 4]), transaction=7, function=13)
    check(reply.payload == b"\x04" and reply.not_ready and not reply.error, "a reply's state byte is read for its bits")
    for what, header, body, transaction, function in (
        ("another transaction's", "000800020003", bytes([13, 0, 4]), 7, 13),
        ("another register's", "000700020003", bytes([14, 0, 4]), 7, 13),
        ("another protocol's", "000700030003", bytes([13, 0, 4]), 7, 13),
        ("a truncated", "000700020004", bytes([13, 0, 4]), 7, 13),
    ):
        try:
            frames.parse_reply(bytes.fromhex(header), body, transaction=transaction, function=function)
            check(False, f"{what} reply is refused")
        except frames.ProtocolError:
            check(True, f"{what} reply is refused, so a late answer is never read as this one's")

    data = frames.servoj_data([0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.0])
    check(len(data) == 40 and struct.unpack("<10f", data)[3] == struct.unpack("<f", struct.pack("<f", 0.4))[0], "a servo target is ten little-endian float32s")
    check(struct.unpack("<10f", data)[7:] == (0.0, 0.0, 0.0), "with speed, acceleration and time zero: servo mode takes them from the stream")

    stream = frames.ReportStream()
    with SimulatedController() as probe:
        probe._step(0.0)
        fast = probe._fast_frame()
        rich = probe._rich_frame()
    parts = stream.feed(fast[:50]) + stream.feed(fast[50:] + rich[:10]) + stream.feed(rich[10:])
    check([len(p) for p in parts] == [135, 574], "report frames are cut out of a stream wherever the bytes fall")
    try:
        frames.ReportStream().feed(b"\xff\xff\xff\xff" + b"\0" * 8)
        check(False, "a stream that announces a frame no report has is refused")
    except frames.ProtocolError:
        check(True, "a stream that announces a frame no report has is refused, not resynchronised")

    head = frames.parse_fast(fast)
    check(head.state == 2 and head.mode == 0 and len(head.angles) == 7, "the fast report's head parses")
    info = frames.parse_rich(rich)
    check(info.axis == 6 and info.robot_type == 6 and info.version == "v2.4.0" and info.is_simulation, "and the rich report says which arm and that it is a simulation")
    check(info.joint_limits[1] == (struct.unpack("<f", struct.pack("<f", -2.059488))[0], struct.unpack("<f", struct.pack("<f", 2.094395))[0]), "with the joint ranges the controller holds")
    try:
        frames.parse_rich(rich[:500])
        check(False, "a rich report that ends before the joint ranges is refused")
    except frames.ProtocolError:
        check(True, "a rich report that ends before the joint ranges is refused, not guessed at")

    # ── the bridge, against a simulated controller ───────────────────────────
    section("bridge — an xArm 6 behind a simulated controller")

    with SimulatedController().start() as controller:
        bridge = XArmBridge()
        check(bridge.open(config(controller)) == rhal.SKR_OK, "a controller comes up")
        check(controller.mode == 1 and controller.state == frames.State.READY, "in servo mode, and ready, by its own report")
        check(len(heard(controller, frames.Fun.MOTION_EN)) == 0, "with no MOTION_EN sent: every motor was already on")

        descriptor = json.loads(bridge.describe())
        check(descriptor["dof"] == 6 and len(descriptor["joints"]) == 6, "the descriptor declares six joints")
        check(descriptor["simulator"] is True, "and declares itself simulated, because the controller says so (§35.3)")
        check(descriptor["envelope"] is None, "and declares no safety envelope")
        check(descriptor["capabilities"] == ["safety.estop"], "and the software stop it has")
        check("effort_max_nm" not in descriptor["joints"][0]["limits"], "and no torque rating: the controller publishes none")
        check(
            all(
                abs(j["limits"]["position_min_rad"] - lo) < 1e-5 and abs(j["limits"]["position_max_rad"] - hi) < 1e-5
                for j, (lo, hi) in zip(descriptor["joints"], XARM6_LIMITS)
            ),
            "travel is the range the controller holds, joint by joint — not one range for every joint",
        )
        check(abs(descriptor["joints"][0]["limits"]["velocity_max_rad_s"] - math.pi) < 1e-6, "and the speed ceiling is its joint speed maximum")

        state = rhal.State()
        check(bridge.read_state(state, 0) == rhal.SKR_OK, "read_state answers")
        check(len(state.position_rad) == 6 and all(math.isfinite(p) for p in state.position_rad), "with a finite position per joint")
        check(all(math.isnan(v) for v in state.velocity_rad_s) and all(math.isnan(e) for e in state.effort_nm), "velocity and effort are NaN: the fast report carries neither")
        check(state.flags & rhal.HOMING_MASK == rhal.HOMING_NOT_REQUIRED, "and says nothing needs homing")
        first, stamp = state.seq, state.stamp_ns
        time.sleep(0.03)
        bridge.read_state(state, 0)
        check(state.seq != first and state.stamp_ns > stamp, "the sequence advances and so does the stamp, which is the newest frame's arrival")

        before = len(heard(controller, frames.Fun.MOVE_SERVOJ))
        for _ in range(60):
            bridge.write_command(cmd([0.3, -0.2, -0.5, 0.0, 0.0, 0.0], [1, 1, 1, 0, 0, 0]), 0)
            time.sleep(0.01)
        until(lambda: abs(controller.position[0] - 0.3) < 1e-3)
        bridge.read_state(state, 0)
        check(
            abs(state.position_rad[0] - 0.3) < 2e-3 and abs(state.position_rad[2] + 0.5) < 2e-3,
            "commanded joints reach their setpoints, read back off the report",
        )
        check(abs(state.position_rad[3]) < 1e-9, "while a masked-out joint stays where it was")
        check(len(heard(controller, frames.Fun.MOVE_SERVOJ)) > before, "and the controller heard the servo targets")

        # A held joint repeats the setpoint the driver last issued, not the measured angle.
        controller.position[0] = 0.25  # the arm sags under a load the target does not know about
        bridge.write_command(cmd([0.0] * 6, [0] * 6), 0)
        last = servoj_angles(heard(controller, frames.Fun.MOVE_SERVOJ)[-1])
        check(abs(last[0] - 0.3) < 1e-6, "a held joint repeats the setpoint last issued, not the sagged measurement")
        controller.position[0] = 0.3

        # The refusals, each beside what succeeds.
        sent = len(heard(controller, frames.Fun.MOVE_SERVOJ))
        check(bridge.write_command(rhal.Command(mode=rhal.MODE_POSITION, values=[0.0] * 5, mask=[1] * 5), 0) == rhal.SKR_EINVAL, "a command with the wrong joint count is refused")
        check(bridge.write_command(rhal.Command(mode=rhal.MODE_TORQUE, values=[0.0] * 6, mask=[1] * 6), 0) == rhal.SKR_EINVAL, "and so is a mode this driver has not wired")
        check(bridge.write_command(cmd([math.nan, 0, 0, 0, 0, 0]), 0) == rhal.SKR_EINVAL, "and so is a setpoint that is not a number")
        check(bridge.write_command(cmd([0.0, 3.0, 0.0, 0.0, 0.0, 0.0]), 0) == rhal.SKR_EINVAL, "and so is one beyond a joint's range (F175)")
        check(bridge.write_command(cmd([0.0, 0.0, 1.0, 0.0, 0.0, 0.0]), 0) == rhal.SKR_EINVAL, "on joint three, whose range is not joint two's")
        check(bridge.write_command(cmd([7.0, 0, 0, 0, 0, 0]), 0) == rhal.SKR_EINVAL, "and on joint one")
        check(bridge.write_command(cmd([0.0, 0.0, 0.19, 0.0, 0.0, 0.0]), 0) == rhal.SKR_OK, "while a setpoint just inside the range is accepted")
        check(len(heard(controller, frames.Fun.MOVE_SERVOJ)) == sent + 1, "and the controller heard only that one: a refused command sends nothing at all")
        check(bridge.write_command(cmd([0.0, 3.0, 0.0, 0.0, 0.0, 0.0], [1, 0, 1, 1, 1, 1]), 0) == rhal.SKR_OK, "a bad setpoint on a masked joint is not a bad command: it was never applied")

        # The stop.
        check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
        check(until(lambda: controller.state == frames.State.STOPPED and controller.stop_count == 1), "and the controller's own state is stopped, told once")
        bridge.read_state(state, 0)
        check(state.flags & rhal.FLAG_ESTOP != 0, "the stop is visible in the next state")
        check(bridge.write_command(cmd([0.1] * 3 + [0.0] * 3), 0) == rhal.SKR_EPERM, "and a command after it is refused")

        # A stop never waits for a command in flight, and never shares its socket.
        with bridge._control_lock:  # a command holding the control socket, as one mid-reply does
            started = time.monotonic()
            done = threading.Thread(target=bridge.emergency_stop)
            done.start()
            done.join(2.0)
            took = time.monotonic() - started
        check(not done.is_alive() and took < 0.5, "emergency_stop does not wait for a command in flight")
        check(until(lambda: controller.stop_count >= 2), "and its frame reaches the controller while the control socket is held")

        check(bridge.release_stop() == rhal.SKR_OK, "release_stop returns")
        check(until(lambda: controller.state in (frames.State.READY, frames.State.MOVING)), "and runs the controller again")
        check(bridge.write_command(cmd([0.1] * 3 + [0.0] * 3), 0) == rhal.SKR_OK, "so a command is obeyed")
        bridge.read_state(state, 0)
        check(state.flags & rhal.FLAG_ESTOP == 0, "with the stop no longer in the state")

        # A controller that raises its own error mid-session: reported, and not cleared.
        controller.latch_error(31)
        until(lambda: bridge._rich is not None and bridge._rich.latest()[0] is not None and bridge._rich.latest()[0].error_code == 31, 3.0)
        bridge.read_state(state, 0)
        check(state.flags & rhal.FLAG_FAULT != 0 and state.fault_code == 31, "an error the controller raises is in the next state, with its code")
        check(state.flags & rhal.FLAG_ESTOP != 0, "and the arm it stopped reads as stopped")
        check(bridge.emergency_stop() == rhal.SKR_OK and bridge.release_stop() == rhal.SKR_EIO, "release_stop over a controller's own error is refused, and re-latches")
        bridge.read_state(state, 0)
        check(state.flags & rhal.FLAG_ESTOP != 0, "so the latch the session set is still set")
        check(controller.error_code == 31, "and the controller's error was not cleared by it")

        check(bridge.close() == rhal.SKR_OK, "close")
        check(controller.state == frames.State.STOPPED, "and leaves the controller stopped where it was, not still taking servo targets")
        check(bridge.read_state(state, 0) == rhal.SKR_EPERM, "a closed body refuses")

    # F73's shape: the stop's latch belongs to its session.
    with SimulatedController().start() as controller:
        bridge = XArmBridge()
        bridge.open(config(controller))
        bridge.emergency_stop()
        bridge.close()
        check(bridge.open(config(controller)) == rhal.SKR_OK, "the same bridge opens again after a stop")
        check(bridge.write_command(cmd([0.1] * 3 + [0.0] * 3), 0) == rhal.SKR_OK, "and the stop's latch belonged to the session it was asserted in")
        bridge.close()

    # ── what it will not bring up ────────────────────────────────────────────
    section("bridge — what it refuses to bring up")

    with SimulatedController().start() as controller:
        for what, body in (
            ("no host", json.dumps({"joints": [{"name": "j"}] * 6, "ports": controller.ports})),
            ("no joints", json.dumps({"host": "127.0.0.1", "ports": controller.ports})),
            ("five joints for a six-axis arm", config(controller, 5)),
            ("an unnamed joint", json.dumps({"host": "127.0.0.1", "ports": controller.ports, "joints": [{"name": "j"}] * 5 + [{}]})),
            ("a stale_ms of zero", config(controller, stale_ms=0)),
        ):
            check(XArmBridge().open(body) == rhal.SKR_ENODEV, f"{what} is refused")
        check(XArmBridge().open("not json") == rhal.SKR_EINVAL, "and so is a configuration that is not JSON")
        check(XArmBridge().open(config(controller)) == rhal.SKR_OK, "while the same controller with a whole configuration comes up")

    dead = SimulatedController().start()
    ports = dict(dead.ports)
    dead.stop()
    time.sleep(0.05)
    check(
        XArmBridge().open(json.dumps({"host": "127.0.0.1", "ports": ports, "joints": [{"name": "j"}] * 6})) == rhal.SKR_EIO,
        "a controller that is not there is a transport failure, and says so",
    )

    with SimulatedController(error_code=31).start() as controller:
        check(XArmBridge().open(config(controller)) == rhal.SKR_ENODEV, "a controller holding an error is refused, rather than cleaned and commanded")
        check(len(heard(controller, frames.Fun.CLEAN_ERR)) == 0 and controller.error_code == 31, "and nothing was cleared: the error is the evidence")

    with SimulatedController().start() as controller:
        controller.position[0] = 0.0
        controller.state = frames.State.MOVING
        check(XArmBridge().open(config(controller)) == rhal.SKR_ENODEV, "an arm somebody else is moving is refused")

    with SimulatedController(motors_enabled=False).start() as controller:
        check(XArmBridge().open(config(controller)) == rhal.SKR_OK, "an arm whose motors are off comes up once it has been asked to enable them")
        check(len(heard(controller, frames.Fun.MOTION_EN)) == 1 and heard(controller, frames.Fun.MOTION_EN)[0] == bytes([8, 1]), "with one MOTION_EN, for every axis")

    saved = bridge_module._SETTLE_S
    bridge_module._SETTLE_S = 0.3
    try:
        with SimulatedController(motors_enabled=False).start() as controller:
            controller.mute.add(frames.Fun.MOTION_EN)  # asked, never answered, and the byte never moves
            check(XArmBridge().open(config(controller)) == rhal.SKR_ENODEV, "motors that stay off are refused: the byte is read back, not the reply trusted")
    finally:
        bridge_module._SETTLE_S = saved

    class WrongVersion(SimulatedController):
        def _get_version(self, _):
            return b"6,6, ,XX0000,v9.9.9".ljust(40, b"\0"), True

    with WrongVersion().start() as controller:
        check(XArmBridge().open(config(controller)) == rhal.SKR_ENODEV, "two answers about one controller that disagree are refused")

    saved = bridge_module._OPEN_TIMEOUT_S
    bridge_module._OPEN_TIMEOUT_S = 0.3
    try:
        with SimulatedController().start() as controller:
            controller.fast_paused.set()
            check(XArmBridge().open(config(controller)) == rhal.SKR_EIO, "a controller whose fast report never arrives is a body with no state, and is refused")
    finally:
        bridge_module._OPEN_TIMEOUT_S = saved

    with SimulatedController(axis=4).start() as controller:
        check(XArmBridge().open(config(controller)) == rhal.SKR_ENODEV, "an arm of four axes is not an xArm this driver has read")
    with SimulatedController(limits=((1.0, 1.0),) * 7).start() as controller:
        check(XArmBridge().open(config(controller)) == rhal.SKR_ENODEV, "a joint range that is not a range is refused, not replaced by one")
    for axis, robot_type in ((5, 5), (7, 3)):
        limits = tuple((-2 * math.pi, 2 * math.pi) for _ in range(7))
        with SimulatedController(axis=axis, robot_type=robot_type, limits=limits).start() as controller:
            b = XArmBridge()
            check(
                b.open(config(controller)) == rhal.SKR_OK and json.loads(b.describe())["dof"] == axis,
                f"an arm of {axis} axes comes up as {axis} joints",
            )
            b.close()

    with SimulatedController(simulation=False).start() as controller:
        b = XArmBridge()
        b.open(config(controller))
        check(json.loads(b.describe())["simulator"] is False, "a controller that says it is not a simulation is declared as one that is not")
        b.close()
        b = XArmBridge()
        b.open(config(controller, simulator=True))
        check(json.loads(b.describe())["simulator"] is True, "and the configuration can only make it more cautious, never less")
        b.close()

    # The controller's ranges against UFACTORY's own table for the arm (F181). UFACTORY's simulator reports the
    # xArm 6's ranges whatever arm it was started as, which puts an xArm 5's wrist bend on the wrong joint.
    section("bridge — the controller's ranges against the vendor's own table (F181)")
    tau = 2 * math.pi
    as_reported_on_every_model = XARM6_LIMITS[:6] + ((-tau, tau),)
    with SimulatedController(axis=5, robot_type=5, limits=as_reported_on_every_model).start() as controller:
        b = XArmBridge()
        check(b.open(config(controller)) == rhal.SKR_OK, "an xArm 5 whose controller reports the xArm 6's ranges comes up")
        joints = json.loads(b.describe())["joints"]
        travel = [(j["limits"]["position_min_rad"], j["limits"]["position_max_rad"]) for j in joints]
        check(
            abs(travel[3][0] + 1.692969) < 1e-9 and abs(travel[3][1] - math.pi) < 1e-9,
            "joint four's travel is the vendor's wrist bend, not the full turn the controller claimed for it",
        )
        check(
            abs(travel[4][0] + 1.692969) < 1e-5 and abs(travel[4][1] - math.pi) < 1e-5,
            "and joint five keeps the range the controller gave it, which is the narrower of the two",
        )
        sent = len(heard(controller, frames.Fun.MOVE_SERVOJ))
        check(b.write_command(cmd([0.0, 0.0, 0.0, 3.5, 0.0]), 0) == rhal.SKR_EINVAL, "a joint-four target the controller would allow and the vendor's table does not is refused")
        check(len(heard(controller, frames.Fun.MOVE_SERVOJ)) == sent, "before anything is sent")
        check(b.write_command(cmd([0.0, 0.0, 0.0, 3.0, 0.0]), 0) == rhal.SKR_OK, "while one inside both is taken")
        b.close()
    # Reduced mode: a controller narrower than the vendor's table keeps its own range.
    narrowed = ((-1.0, 1.0),) + XARM6_LIMITS[1:6] + ((-tau, tau),)
    with SimulatedController(limits=narrowed).start() as controller:
        b = XArmBridge()
        b.open(config(controller))
        first = json.loads(b.describe())["joints"][0]["limits"]
        check(abs(first["position_max_rad"] - 1.0) < 1e-5, "an operator's narrower reduced mode still narrows it")
        b.close()
    # An arm the SDK's table does not list has one source, and the controller's is used, said aloud.
    with SimulatedController(robot_type=99, limits=XARM6_LIMITS[:6] + ((-tau, tau),)).start() as controller:
        b = XArmBridge()
        check(b.open(config(controller)) == rhal.SKR_OK, "an arm type the vendor's table does not list comes up on the controller's own ranges")
        b.close()
    # Two sources with nothing in common are a controller misread.
    disjoint = (XARM6_LIMITS[0], (3.0, 4.0)) + XARM6_LIMITS[2:6] + ((-tau, tau),)
    with SimulatedController(limits=disjoint).start() as controller:
        check(XArmBridge().open(config(controller)) == rhal.SKR_ENODEV, "a controller whose range for a joint shares nothing with the vendor's is refused")

    # A controller that has gone quiet, and one that will not answer a command.
    with SimulatedController().start() as controller:
        b = XArmBridge()
        b.open(config(controller, stale_ms=60, control_timeout_ms=150))
        state = rhal.State()
        check(b.read_state(state, 0) == rhal.SKR_OK, "a live controller reads")
        controller.fast_paused.set()
        time.sleep(0.15)
        check(b.read_state(state, 0) == rhal.SKR_ETIMEDOUT and state.flags & rhal.FLAG_DEGRADED != 0, "one whose stream has gone quiet is reported degraded, not read as still")
        controller.fast_paused.clear()
        controller.mute.add(frames.Fun.MOVE_SERVOJ)
        started = time.monotonic()
        check(b.write_command(cmd([0.1] * 3 + [0.0] * 3), 0) == rhal.SKR_EIO and time.monotonic() - started < 1.0, "a controller that will not answer a command is a transport failure, inside its timeout")
        b.close()

    finish_line()
    return finish()


def finish_line() -> None:
    print()


def finish() -> int:
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 1 if FAILURES else 0


# ── the live block ──────────────────────────────────────────────────────────


def live_checks(host: str, axis: int = 6, arm_type: int = 6) -> None:
    """The bridge against a controller's own firmware (`make check-sim-docker` supplies UFACTORY's).

    `axis` and `arm_type` are what the firmware was started as (`xarm_start.sh AXIS TYPE`): UFACTORY's image
    carries the xArm 5, the xArm 6, the Lite 6 (type 9), the 850 (type 12) and the xArm 7.

    Asked for and not there is a failure, never a skip. Everything a controller's state is claimed to
    be here is read a second way — over a fresh control socket, and through the vendor's own client when
    `XARM_SDK` names its checkout — because the bridge reading its own writes back would agree with
    itself.
    """
    section(f"bridge — against the controller at {host}, started as an xArm {axis} of type {arm_type}")

    def raw_state() -> int:
        import socket

        with socket.create_connection((host, frames.CONTROL_PORT), timeout=3) as sock:
            sock.sendall(frames.request(1, frames.Fun.GET_STATE))
            header = sock.recv(6)
            body = sock.recv(int.from_bytes(header[4:6], "big"))
            return frames.parse_reply(header, body, transaction=1, function=frames.Fun.GET_STATE).payload[0]

    sdk = os.environ.get("XARM_SDK")
    vendor = None
    if sdk:
        sys.path.insert(0, sdk)
        from xarm.wrapper import XArmAPI  # noqa: PLC0415 — the vendor's own client

        vendor = XArmAPI(host, is_radian=True, do_not_open=False)

    # The vendor's own table of ranges for this arm: its SDK when it is there, else the one this tree
    # carries for the one arm it carries a table for. Asked for and not there is a failure, never a skip.
    table = None
    if sdk:
        from xarm.core.config.x_config import XCONF  # noqa: PLC0415

        table = XCONF.Robot.JOINT_LIMITS[axis][arm_type]
    elif (axis, arm_type) == (6, 6):
        table = XARM6_LIMITS
    check(table is not None and len(table) == axis, f"the vendor's own joint table for an xArm {axis} of type {arm_type} is at hand to compare against")
    if table is None:
        return

    bridge = XArmBridge()
    body = json.dumps({"host": host, "joints": [{"name": f"joint{i + 1}", "role": None} for i in range(axis)]})
    opened = bridge.open(body)
    check(opened == rhal.SKR_OK, "the controller's firmware comes up as a body")
    if opened != rhal.SKR_OK:
        return
    descriptor = json.loads(bridge.describe())
    check(descriptor["dof"] == axis and descriptor["simulator"] is True, f"as {axis} joints, declared simulated because the firmware's own report says so")
    check(descriptor["robot_slug"] == f"xarm{axis}-type{arm_type}", f"named for what the firmware reports it is ({descriptor['robot_slug']})")
    # Two sources for a joint's range, read separately: the firmware's own report, over a fresh connection to the
    # rich port, and the vendor's SDK table for this arm. The firmware rounds what it reports to a milliradian
    # (F176), and UFACTORY's simulator reports the xArm 6's ranges on every model it starts (F181). What the
    # driver promises is the narrower of the two, never the wider, and that is what is asserted.
    import socket

    def firmware_ranges() -> tuple[tuple[float, float], ...]:
        with socket.create_connection((host, frames.REPORT_RICH_PORT), timeout=3) as sock:
            stream = frames.ReportStream()
            end = time.monotonic() + 3.0
            while time.monotonic() < end:
                for frame in stream.feed(sock.recv(65536)):
                    return frames.parse_rich(frame).joint_limits[:axis]
        raise AssertionError("the rich report never arrived")

    reported = firmware_ranges()
    wider = [
        f"joint {i}: firmware [{a:+.3f}, {b:+.3f}] against the vendor's [{x:+.3f}, {y:+.3f}]"
        for i, ((a, b), (x, y)) in enumerate(zip(reported, table), start=1)
        if a < x - 1e-3 or b > y + 1e-3
    ]
    print(f"  note  the firmware's ranges are wider than the vendor's table on {len(wider)} of {axis} joints" + (": " + "; ".join(wider) if wider else ""))
    check(
        all(
            j["limits"]["position_min_rad"] >= x - 1e-9 and j["limits"]["position_max_rad"] <= y + 1e-9
            for j, (x, y) in zip(descriptor["joints"], table)
        ),
        f"travel is never wider than the vendor's own table for an xArm {axis} of type {arm_type}",
    )
    check(
        all(
            abs(j["limits"]["position_min_rad"] - max(a, x)) < 1e-5 and abs(j["limits"]["position_max_rad"] - min(b, y)) < 1e-5
            for j, (a, b), (x, y) in zip(descriptor["joints"], reported, table)
        ),
        "and is, joint by joint, the narrower of that and what the firmware reports",
    )
    check(raw_state() in (frames.State.READY, frames.State.MOVING), "and a second connection sees it ready")

    state = rhal.State()
    check(bridge.read_state(state, 0) == rhal.SKR_OK, "read_state answers")
    start = list(state.position_rad)
    home = [0.0] * axis
    target = [0.2] + home[1:]
    for _ in range(120):
        bridge.write_command(cmd(target), 0)
        time.sleep(0.01)
    until(lambda: bridge.read_state(state, 0) == rhal.SKR_OK and abs(state.position_rad[0] - 0.2) < 3e-3, 3.0)
    check(abs(state.position_rad[0] - 0.2) < 3e-3, "joint one moves 0.2 rad at the controller's own pace and the report says so")
    if vendor is not None:
        check(abs(vendor.angles[0] - 0.2) < 3e-3, "and the vendor's own client, reading the same controller, agrees")

    beyond = list(home)
    beyond[1] = descriptor["joints"][1]["limits"]["position_max_rad"] + 0.3
    check(bridge.write_command(cmd(beyond), 0) == rhal.SKR_EINVAL, "a target beyond joint two's range is refused before it is sent (F175)")
    time.sleep(0.4)
    bridge.read_state(state, 0)
    check(state.flags & rhal.FLAG_FAULT == 0 and raw_state() != frames.State.STOPPED, "so the firmware never raised the error it raises when it is sent one")

    for _ in range(120):
        bridge.write_command(cmd(home), 0)
        time.sleep(0.01)
    until(lambda: bridge.read_state(state, 0) == rhal.SKR_OK and abs(state.position_rad[0]) < 3e-3, 3.0)
    check(abs(state.position_rad[0]) < 3e-3, "and joint one comes back")

    check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    check(until(lambda: raw_state() == frames.State.STOPPED, 2.0), "and the firmware's own state, read over a fresh connection, is stopped")
    if vendor is not None:
        check(until(lambda: vendor.state == frames.State.STOPPED, 2.0), "and the vendor's client sees state 4")
    check(bridge.write_command(cmd(home), 0) == rhal.SKR_EPERM, "a command after the stop is refused")
    check(bridge.release_stop() == rhal.SKR_OK, "release_stop runs the firmware again")
    check(until(lambda: raw_state() in (frames.State.READY, frames.State.MOVING), 2.0), "as a fresh connection sees")
    check(bridge.write_command(cmd(home), 0) == rhal.SKR_OK, "and a command is taken")
    check(bridge.close() == rhal.SKR_OK, "close")
    check(until(lambda: raw_state() == frames.State.STOPPED, 2.0), "leaves the firmware stopped")
    check(bridge.open(body) == rhal.SKR_OK, "and the same firmware comes up again")
    bridge.close()
    if vendor is not None:
        vendor.disconnect()
    del start


if __name__ == "__main__":
    raise SystemExit(main())
