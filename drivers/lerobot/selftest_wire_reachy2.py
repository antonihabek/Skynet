"""The LeRobot bridge over LeRobot's own Reachy 2 class, against Pollen Robotics' own core in fake mode — item 12.E.12 (Reachy 2).

`selftest.py` runs the bridge against `skynet_lerobot.fake`, whose Reachy 2 is "a faithful fake" written from LeRobot's documentation, and `roles.py` has had a table for `reachy2` since
the bridge was written. Nothing Pollen wrote had ever been run against it. This is that run. The robot is `lerobot.robots.reachy2.Reachy2Robot` as shipped, constructed the way production
constructs it (`open_bridge`: a JSON configuration, LeRobot's own config registry, `make_robot_from_config`), speaking through Pollen's own `reachy2-sdk` over gRPC to Pollen's own core:
the image `pollenrobotics/reachy2` in its fake mode, which is the ROS 2 stack and the SDK server a Reachy 2 runs, over mock hardware (`sim/reachy2_core.py` starts it).

    ~/lerobot-venv/bin/python drivers/lerobot/selftest_wire_reachy2.py        # needs Docker, the image, and `lerobot[reachy2]`

# What it can and cannot show

It shows that the bridge drives the library the library's way, and that the library's server answers it as Pollen's software means to: the handshake and `turn_on` that `connect()` makes,
the goals `send_action` sends for twenty-one joints and a mobile base, and the state the SDK's stream carries back. It is **rung 2** (`docs/robot-support.md` section 3): the maker's own
server answering the maker's own client. The mock hardware follows what it is commanded and has no physics, no camera and no WebRTC, so a pass is not evidence that an arm moves, that
a motor holds, or that anything stays inside its travel: the mock hardware bounds nothing (a goal of 500 degrees reads back as 500), and a real Reachy 2's limits live in hardware this does not run.

# What running it found

* **F329** — the image does not stay up in a container with no USB bus (`sim/reachy2_core.py` says what the video server does and what is put in its way).
* **F330** — LeRobot's `Reachy2Robot` makes blocking gRPC calls with no deadline in every observation; against a core that stopped answering the bridge waited for as long as TCP kept the connection. The Reachy 2
  class is `bounded` in `roles.ROBOT_DEFAULTS` now: its calls are made on a thread of the bridge's own with a deadline, and the stop returns inside 50 ms whatever the core does.
* **F331** — Pollen's SDK keeps its state in a cache a stream fills; a stream that had stopped read as alive. The class has an `update_clock` now, and a read is held to it.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "bindings", "python"))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "sim"))

try:
    import lerobot  # noqa: F401
    import reachy2_sdk  # noqa: F401
except ImportError as error:  # a refusal, not a pass: the claim of this file needs the library
    print(f"selftest_wire_reachy2: {error}. This needs LeRobot and Pollen's SDK (pip install 'lerobot[reachy2]'), e.g. ~/lerobot-venv.", file=sys.stderr)
    sys.exit(2)

import skynet_rhal as rhal  # noqa: E402
from skynet_rhal import Command, State  # noqa: E402
from skynet_lerobot.bridge import open_bridge  # noqa: E402
from skynet_lerobot.roles import ROLE_TABLES  # noqa: E402
from lerobot.robots.reachy2 import robot_reachy2 as lerobot_reachy2  # noqa: E402
from reachy2_core import Core, CoreError, docker_available  # noqa: E402
from wire_common import check, finish, roles_cover_every_lerobot_robot, wait_until  # noqa: E402

#: LeRobot's own lists, in the order its class builds `action_features` from them: the neck, the left arm, the right arm, the antennas. Each maps a LeRobot key to the SDK's name for the joint.
SDK_NAME = {
    **{key.removesuffix(".pos"): value for table in (lerobot_reachy2.REACHY2_NECK_JOINTS, lerobot_reachy2.REACHY2_L_ARM_JOINTS, lerobot_reachy2.REACHY2_R_ARM_JOINTS, lerobot_reachy2.REACHY2_ANTENNAS_JOINTS) for key, value in table.items()},
}
NAMES = list(SDK_NAME)


def command(bridge, values, mask=None, seq=1, mode=None) -> int:
    mask = mask if mask is not None else [1] * len(values)
    return bridge.write_command(Command(stamp_ns=1, seq=seq, mode=rhal.MODE_POSITION if mode is None else mode, values=list(values), mask=list(mask)), 0)


def read(bridge) -> tuple[int, State]:
    state = State()
    return bridge.read_state(state, 0), state


def sdk_degrees(robot, name: str) -> float:
    return float(robot.reachy.joints[SDK_NAME[name]].present_position)


def goal_degrees(robot, name: str) -> float:
    return float(robot.reachy.joints[SDK_NAME[name]].goal_position)


FOOTER = (
    "Rung 2: LeRobot's own Reachy2Robot, Pollen's SDK and Pollen's own core in fake mode. It shows the bridge drives the library as the library is meant to be driven and that the "
    "maker's server answers it; the mock hardware bounds nothing and moves instantly, and no number here is a motor's."
)


def main() -> int:
    roles_cover_every_lerobot_robot()
    table = ROLE_TABLES["reachy2"]
    check("embodiment.roles.coverage", f"the role table names exactly the {len(NAMES)} joints LeRobot's Reachy2Robot names", set(table) == set(NAMES), f"only in the table: {sorted(set(table) - set(NAMES))}; only in LeRobot: {sorted(set(NAMES) - set(table))}")
    check("embodiment.roles.coverage", "and no two of them share a role", len(set(table.values())) == len(table))

    why = docker_available()
    if why:
        print(f"selftest_wire_reachy2: {why}. This runs Pollen's image.", file=sys.stderr)
        return 2
    try:
        core = Core(cpus=os.environ.get("REACHY2_CPUS"))
        with core:
            ready_in = core.wait_ready()
            check("rhal.abi.version", f"Pollen's core in fake mode is up and its SDK server answers ({ready_in:.0f} s)", True)
            try:
                run(core)
            except Exception:  # noqa: BLE001 — a check that raised is a failure to report, and the session it left open is closed by `run`'s own finally
                traceback.print_exc()
                check("rhal.abi.version", "the checks ran to the end", False, "an exception, above")
    except CoreError as error:
        check("rhal.abi.version", "Pollen's core in fake mode comes up", False, str(error))
    status = finish(FOOTER)
    sys.stdout.flush()
    # Pollen's SDK keeps threads of its own; a run that ended is not held up by one that was left.
    os._exit(status)


def run(core: Core) -> None:
    config = json.dumps({"robot": {"type": "reachy2", "ip_address": "127.0.0.1"}, "simulator": True})
    bridge = open_bridge(config)
    try:
        session(core, config, bridge)
    finally:
        with contextlib.suppress(Exception):
            bridge.close()


def session(core: Core, config: str, bridge) -> None:
    table = ROLE_TABLES["reachy2"]
    rc = bridge.open(config)
    check("rhal.abi.version", "opens LeRobot's own Reachy2Robot over Pollen's SDK, `connect()` and `turn_on()` included", rc == rhal.SKR_OK, f"open returned {rc}")
    if rc != rhal.SKR_OK:
        return
    robot = bridge._robot  # noqa: SLF001
    check("rhal.abi.version", "the robot is LeRobot's `Reachy2Robot`, as shipped", type(robot).__module__ == "lerobot.robots.reachy2.robot_reachy2", type(robot).__module__)
    check("rhal.abi.version", "and Pollen's core reports itself a fake, not a robot", robot.reachy.info.mode == "FAKE", str(robot.reachy.info.mode))
    check("rhal.abi.version", "connect turned the robot on", wait_until(lambda: robot.reachy.is_on(), 3.0))

    # The body, as the bridge describes it.
    descriptor = json.loads(bridge.describe())
    check("embodiment.descriptor.valid", f"the joints are LeRobot's {len(NAMES)} motors, in LeRobot's order", [j["name"] for j in descriptor["joints"]] == NAMES, str([j["name"] for j in descriptor["joints"]]))
    check("embodiment.roles.coverage", "each carries the role the table gives it", [j.get("role") for j in descriptor["joints"]] == [table[name] for name in NAMES], str([j.get("role") for j in descriptor["joints"]]))
    check("embodiment.descriptor.valid", "it declares itself a simulator, with no envelope and no ratings it was not given", descriptor["simulator"] is True and descriptor["envelope"] is None and all(j["limits"] == {} for j in descriptor["joints"]))
    check("embodiment.descriptor.valid", "and every joint is a position joint", all(j["control_modes"] == ["position"] for j in descriptor["joints"]) and descriptor["control"]["modes"] == ["position"])
    check("embodiment.descriptor.valid", "the SDK's own unit, degrees, is converted to radians for every joint, the gripper's included", "converted from deg" in descriptor["provenance"]["units"] and "passed through unconverted" not in descriptor["provenance"]["units"], descriptor["provenance"]["units"])

    # The state.
    rc, state = read(bridge)
    check("rhal.state.completeness", "read_state succeeds over Pollen's SDK", rc == rhal.SKR_OK, str(rc))
    expected = [math.radians(sdk_degrees(robot, name)) for name in NAMES]
    check("rhal.state.completeness", "each joint reads as the SDK's degrees, in radians", all(abs(state.position_rad[i] - expected[i]) < 1e-6 for i in range(len(NAMES))), f"{[round(v, 5) for v in state.position_rad]} against {[round(v, 5) for v in expected]}")
    check("rhal.state.completeness", "an unreported velocity and effort are NaN, and a body with no fault word has none", all(math.isnan(v) for v in state.velocity_rad_s) and all(math.isnan(v) for v in state.effort_nm) and state.fault_code == 0 and state.flags == 0)
    check("rhal.state.completeness", "the pose of a base nobody localised is NaN", all(math.isnan(v) for v in state.base_pose))

    # A position command for one joint: the rest are held.
    shoulder = NAMES.index("r_shoulder_pitch")
    values = list(state.position_rad)
    values[shoulder] = math.radians(15.0)
    mask = [0] * len(NAMES)
    mask[shoulder] = 1
    rc = command(bridge, values, mask, seq=1)
    check("rhal.command.rejection", "a position command for one joint is accepted", rc == rhal.SKR_OK, str(rc))
    check("rhal.command.rejection", "the SDK's goal for it is the angle in degrees the bridge converted it to", wait_until(lambda: abs(goal_degrees(robot, "r_shoulder_pitch") - 15.0) < 1e-6, 1.0), str(goal_degrees(robot, "r_shoulder_pitch")))
    check("rhal.command.rejection", "the joint follows it, and the bridge reads the move back as the same angle", wait_until(lambda: abs(read(bridge)[1].position_rad[shoulder] - math.radians(15.0)) < math.radians(0.5), 3.0), f"reads {read(bridge)[1].position_rad[shoulder]:.4f} rad")
    after = read(bridge)[1]
    others = [i for i in range(len(NAMES)) if i != shoulder]
    check("safety.watchdog.fires", "the twenty joints it did not move are where they were: a masked joint is held (§24)", all(abs(after.position_rad[i] - state.position_rad[i]) < math.radians(0.05) for i in others), str([(NAMES[i], round(math.degrees(after.position_rad[i] - state.position_rad[i]), 3)) for i in others if abs(after.position_rad[i] - state.position_rad[i]) >= math.radians(0.05)]))
    check("safety.watchdog.fires", "and their goals are the positions they held, not zero and not unset", all(abs(goal_degrees(robot, NAMES[i]) - math.degrees(state.position_rad[i])) < 0.01 for i in others), str([(NAMES[i], round(goal_degrees(robot, NAMES[i]), 3)) for i in others][:6]))
    check("safety.watchdog.fires", "the mobile base was told to stand still, and did", robot.reachy.mobile_base.odometry["vx"] == 0.0 and robot.reachy.mobile_base.odometry["vtheta"] == 0.0, str(robot.reachy.mobile_base.odometry))

    # Several joints at once, among them a gripper and an antenna.
    values = list(after.position_rad)
    wanted = {"neck_yaw": 20.0, "l_elbow_pitch": -30.0, "r_gripper": 30.0, "l_antenna": 25.0, "l_wrist_yaw": -15.0}
    for name, degrees in wanted.items():
        values[NAMES.index(name)] = math.radians(degrees)
    rc = command(bridge, values, [1] * len(NAMES), seq=2)
    check("rhal.command.rejection", "a command for the neck, an elbow, a wrist, a gripper and an antenna together is accepted", rc == rhal.SKR_OK, str(rc))
    arrived = wait_until(lambda: all(abs(sdk_degrees(robot, name) - degrees) < 0.5 for name, degrees in wanted.items()), 3.0)
    check("rhal.command.rejection", "and each arrives at its own angle", arrived, str({name: round(sdk_degrees(robot, name), 2) for name in wanted}))

    # What the bridge refuses before it reaches Pollen's server.
    check("rhal.command.rejection", "a velocity command is refused: every joint here is a position joint", command(bridge, values, seq=3, mode=rhal.MODE_VELOCITY) == rhal.SKR_EINVAL)
    nan = list(values)
    nan[0] = math.nan
    check("rhal.command.rejection", "a position that is not a number is refused, and nothing is sent", command(bridge, nan, seq=4) == rhal.SKR_EINVAL)
    check("rhal.command.rejection", "a command for the wrong number of joints is refused", command(bridge, values[:-1], [1] * (len(NAMES) - 1), seq=5) == rhal.SKR_EINVAL)

    # The stop. Reachy2Robot has no torque-disable, so the bridge's stop is a freeze in place and a latch, and says so.
    began = time.monotonic()
    with contextlib.redirect_stderr(io.StringIO()) as said:
        rc = bridge.emergency_stop()
    took = time.monotonic() - began
    check("safety.estop.latency", "emergency_stop is acknowledged", rc == rhal.SKR_OK, str(rc))
    check("safety.estop.latency", f"and returns inside the RHAL's 50 ms ({took * 1000:.1f} ms)", took < 0.05)
    check("safety.estop.latency", "and says that it froze the joints and did not cut the drives, which is what it did", "freeze in place" in said.getvalue() and "drives were not cut" in said.getvalue(), said.getvalue()[:300])
    rc, stopped = read(bridge)
    check("safety.estop.latency", "the latch shows in the next state", rc == rhal.SKR_OK and bool(stopped.flags & rhal.FLAG_ESTOP))
    check("safety.estop.latency", "and every later command is refused", command(bridge, values, seq=6) == rhal.SKR_EPERM)
    check("safety.estop.latency", "the frozen goals are the positions the joints had", all(abs(goal_degrees(robot, name) - sdk_degrees(robot, name)) < 0.5 for name in NAMES))
    rc = bridge.release_stop()
    check("safety.estop.latency", "release_stop clears the latch, and says nothing was ever cut", rc == rhal.SKR_OK and not (read(bridge)[1].flags & rhal.FLAG_ESTOP))
    check("rhal.command.rejection", "and a command is accepted again", command(bridge, values, seq=7) == rhal.SKR_OK)

    # A core that has stopped talking: the container is frozen (the cgroup freezer), so the SDK's stream goes quiet and its connection stays open.
    # §24: a producer that has stalled is a sequence that does not move (or a read that is not OK). A read that stamps and counts itself on every call says the core is alive for as long as the cache lasts.
    reads: list[tuple[int, int, int]] = []
    core.pause()
    try:
        began = time.monotonic()
        while time.monotonic() - began < 1.0:
            rc, snapshot = read(bridge)
            reads.append((rc, snapshot.seq, snapshot.stamp_ns))
            time.sleep(0.02)
    finally:
        core.unpause()
    tail = [item for item in reads[len(reads) // 2:]]
    reported = any(rc != rhal.SKR_OK for rc, _, _ in tail) or len({seq for _, seq, _ in tail}) == 1
    check("safety.watchdog.fires", "with the core frozen, a read says so within a second: it is not OK, or it repeats its sequence (§24; F331)", reported,
          f"{len(reads)} reads, the last ten: {[(rc, seq) for rc, seq, _ in reads[-10:]]}")
    check("safety.watchdog.fires", "and a stalled read is stamped with when the state arrived, not with the time of the read", len({stamp for rc, _, stamp in tail if rc == rhal.SKR_OK}) <= 1, f"{len({stamp for _, _, stamp in tail})} distinct stamps in {len(tail)} reads")
    check("rhal.state.completeness", "and when the core runs again the sequence moves and the state reads as before", wait_until(lambda: read(bridge)[0] == rhal.SKR_OK and read(bridge)[1].seq != reads[-1][1] and abs(read(bridge)[1].position_rad[shoulder] - values[shoulder]) < math.radians(1.0), 5.0))

    # Close, and a second session on the same core.
    check("rhal.abi.version", "closes, and Pollen's SDK disconnects", bridge.close() == rhal.SKR_OK and not robot.reachy.is_connected())

    # The same core frozen under a body with no mobile base, where nothing in a read is a call: the cache is all there is, and only the SDK's own clock says the stream has stopped (F331).
    no_base = json.dumps({"robot": {"type": "reachy2", "ip_address": "127.0.0.1", "with_mobile_base": False}, "simulator": True})
    cached = open_bridge(no_base)
    try:
        rc = cached.open(no_base)
        check("rhal.abi.version", "a Reachy 2 with no mobile base opens, and its reads are the SDK's cache alone", rc == rhal.SKR_OK, str(rc))
        if rc == rhal.SKR_OK:
            rc, live = read(cached)
            rc2, live2 = read(cached)
            check("rhal.state.completeness", "and while the stream runs each read is a state the one before was not", rc == rhal.SKR_OK and rc2 == rhal.SKR_OK and live2.seq == live.seq + 1 and live2.stamp_ns > live.stamp_ns, f"{live.seq} -> {live2.seq}")
            core.pause()
            try:
                time.sleep(0.3)
                began = time.monotonic()
                frozen = []
                while time.monotonic() - began < 1.0:
                    rc, snapshot = read(cached)
                    frozen.append((rc, snapshot.seq, snapshot.stamp_ns))
            finally:
                core.unpause()
            tail = frozen[len(frozen) // 2:]
            check("safety.watchdog.fires", "with the core frozen a read of the cache says so: it repeats its sequence and its stamp, where a cache read as before would have counted on (F331)", all(rc == rhal.SKR_OK for rc, _, _ in tail) and len({seq for _, seq, _ in tail}) == 1 and len({stamp for _, _, stamp in tail}) == 1, f"{len(frozen)} reads, the last ten: {[(rc, seq) for rc, seq, _ in frozen[-10:]]}")
            check("rhal.state.completeness", "and when the core runs again the sequence moves", wait_until(lambda: read(cached)[1].seq != frozen[-1][1], 5.0))
    finally:
        with contextlib.suppress(Exception):
            cached.close()

    second = open_bridge(config)
    rc = second.open(config)
    check("rhal.abi.version", "a second session opens on the same core", rc == rhal.SKR_OK, str(rc))
    if rc == rhal.SKR_OK:
        rc, again = read(second)
        check("rhal.state.completeness", "and reads the body where the first left it", rc == rhal.SKR_OK and abs(again.position_rad[shoulder] - values[shoulder]) < math.radians(1.0), f"{again.position_rad[shoulder]:.4f} rad")
        check("rhal.abi.version", "and closes", second.close() == rhal.SKR_OK)


if __name__ == "__main__":
    sys.exit(main())
