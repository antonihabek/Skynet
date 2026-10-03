"""The ROS 2 base driver against iRobot's own Create 3 simulation, in real time (DRY-COVERAGE-PLAN.md item 12.E.3, the forty-seventh session).

Run by `sim/run-create3.sh` inside `sim/Dockerfile.create3`'s image, with Gazebo's server, the Create 3 spawned and the maker's nodes up:

    python3 live_create3.py

`live.py` holds the driver to `ros2_control`'s controller on mock hardware and `live_tb3.py` to ROBOTIS's TurtleBot3 simulation. This holds it to a second maker's simulator, F118's rung 2, and
to something neither of those has: **the maker's own safety layer between the command topic and the controller**. On the Create 3 the topic a driver publishes to is read by iRobot's `motion_control`
node (hazard reflexes, a speed bound when the safety override is on), which republishes to `diffdrive_controller` — `ros2_control`'s controller, with the Create 3's own wheel geometry, an
acceleration limit and a command timeout of half a second *of simulation time*. The odometry is the controller's, from the wheels' positions.

The second opinion is **Gazebo's own pose of the model**, read with `gz topic` off the world's pose stream: the physics engine's idea of where the base is, which shares nothing with the
controller's odometry, `motion_control` or this driver. Every claim the driver makes about where the base went is held to it.

Nothing here assumes the simulation runs in real time: it runs a CPU renderer for the Create 3's cliff and IR sensors, on a machine that may have other work, and the factor is measured and
printed. What is asserted is the relation between the driver and the physics, in the simulation's own time.

What it cannot show: a real Create 3's motors, battery, floor, bumpers and firmware. The simulation is the maker's, the robot is not.
"""

from __future__ import annotations

import json
import math
import re
import subprocess
import sys
import threading
import time

import live as L
import skynet_rhal as rhal
from skynet_ros2_base import RclpyTransport, Ros2BaseDriver

WORLD = "flat"
MODEL = "create3"

_BLOCK = re.compile(r'pose\s*\{\s*name:\s*"%s"(.*?)\n\}' % re.escape(MODEL), re.S)
_NUMBER = r"(-?[0-9.]+(?:e[-+]?[0-9]+)?)"


def truth() -> tuple[float, float, float] | None:
    """Where Gazebo's physics says the model is: x, y, yaw. `gz topic` prints a protobuf text message and leaves out a field that is zero."""
    done = subprocess.run(["gz", "topic", "-e", "-t", f"/world/{WORLD}/pose/info", "-n", "1"], capture_output=True, text=True, timeout=30, check=False)
    block = _BLOCK.search(done.stdout)
    if block is None:
        return None
    body = block.group(1)

    def part(section: str, field: str) -> float:
        inner = re.search(section + r"\s*\{(.*?)\}", body, re.S)
        found = re.search(field + r":\s*" + _NUMBER, inner.group(1)) if inner else None
        return float(found.group(1)) if found else 0.0

    z, w = part("orientation", "z"), part("orientation", "w")
    return part("position", "x"), part("position", "y"), 2.0 * math.atan2(z, w)


def travelled(before: tuple[float, float, float], after: tuple[float, float, float]) -> float:
    return math.hypot(after[0] - before[0], after[1] - before[1])


def wrap(angle: float) -> float:
    return math.atan2(math.sin(angle), math.cos(angle))


def at_rest(window_s: float = 1.0) -> float:
    """How far the physics moved the base in `window_s` of wall time, in metres."""
    a = truth()
    time.sleep(window_s)
    b = truth()
    assert a is not None and b is not None
    return travelled(a, b)


def main() -> int:
    config = {"command_topic": "/cmd_vel", "odometry_topic": "/odom", "max_linear": 0.3, "max_angular": 1.8, "simulator": True, "open_wait_s": 30.0, "stop_settle_s": 12.0, "stale_after_s": 2.0}

    L.section("the graph, by ROS's own tools and by Gazebo's")
    info = L.ros("topic", "info", "-v", "/cmd_vel")
    L.check("Type: geometry_msgs/msg/TwistStamped" in info and "Node name: motion_control" in info, "`ros2 topic info` says /cmd_vel takes geometry_msgs/msg/TwistStamped and its subscriber is iRobot's `motion_control`, not the controller", info[:400])
    forwarded = L.ros("topic", "info", "-v", "/diffdrive_controller/cmd_vel")
    L.check("Node name: motion_control" in forwarded and "Node name: diffdrive_controller" in forwarded, "and `motion_control` is what republishes to the controller's own command topic: the driver talks to the maker's safety layer", forwarded[:400])
    odom_info = L.ros("topic", "info", "-v", "/odom")
    L.check("Type: nav_msgs/msg/Odometry" in odom_info and "Publisher count: 1" in odom_info and "Node name: diffdrive_controller" in odom_info.split("Subscription count")[0], "and /odom is nav_msgs/Odometry with one publisher, the controller", odom_info[:400])
    start = truth()
    L.check(start is not None, f"Gazebo's pose stream names the model: {MODEL} is at {None if start is None else tuple(round(v, 3) for v in start)}")
    if start is None:
        print(subprocess.run(["gz", "topic", "-e", "-t", f"/world/{WORLD}/pose/info", "-n", "1"], capture_output=True, text=True, timeout=30, check=False).stdout[:600])
        return 1

    L.section("open: the type is read off the graph, and the frames are the maker's")
    driver = Ros2BaseDriver(lambda settings: RclpyTransport(settings.command_topic, settings.odometry_topic, domain_id=settings.domain_id, frame_id=settings.frame_id))
    began = time.monotonic()
    L.check(driver.open(json.dumps(config)) == rhal.SKR_OK, f"the driver opens on /cmd_vel and /odom in {time.monotonic() - began:.1f} s")
    code, status = L.run(driver, "status")
    L.check(code == rhal.SKR_OK and status["message_type"] == "geometry_msgs/msg/TwistStamped" and status["subscribers"] >= 1, "it chose TwistStamped, the type motion_control's subscriber reports, and is connected to it", str(status))
    L.check(status["odometry_frame"] == "odom", f"the odometry's frame is the controller's own, `odom`; the commands carry `{status.get('twist_frame')}`")
    transport = driver._transport

    def sim_now() -> float | None:
        latest = transport.odometry()
        return None if latest is None else latest.header_s

    first_sim, first_wall = sim_now(), time.monotonic()
    samples = set()
    deadline = time.monotonic() + 4.0
    while time.monotonic() < deadline:
        latest = transport.odometry()
        if latest is not None:
            samples.add(latest.header_s)
        time.sleep(0.01)
    last_sim, last_wall = sim_now(), time.monotonic()
    assert first_sim is not None and last_sim is not None
    factor = (last_sim - first_sim) / (last_wall - first_wall)
    L.check(0.05 < factor < 3.0, f"the simulation runs at {factor:.2f} of real time (a CPU renderer for the sensors); what follows is held in its own time")
    rate = len(samples) / 4.0
    hz = subprocess.run(["timeout", "8", "ros2", "topic", "hz", "--window", "50", "/odom"], capture_output=True, text=True, check=False).stdout
    measured = re.findall(r"average rate:\s*([0-9.]+)", hz)
    reference = float(measured[-1]) if measured else math.nan
    L.check(not math.isnan(reference) and abs(rate - reference) < 0.25 * reference, f"it hears the controller's odometry at the rate ROS's own `ros2 topic hz` measures: {rate:.1f} Hz against {reference:.1f} Hz")
    state = rhal.State()
    L.check(driver.read_state(state, 0) == rhal.SKR_OK and state.flags == rhal.HOMING_NOT_REQUIRED, "and the state reads clean")

    def run_motion(text: str, settle_s: float = 1.0):
        """A motion, and the physics before and after it, with the simulation's own seconds the call took."""
        before = truth()
        t0 = sim_now()
        code, reply = L.run(driver, text)
        time.sleep(settle_s)
        after = truth()
        t1 = sim_now()
        assert before is not None and after is not None and t0 is not None and t1 is not None
        return before, after, t1 - t0, code, reply

    L.section("velocity: where Gazebo's physics says the base went")
    before, after, span, code, reply = run_motion("velocity vx=0.15 duration=4.0")
    L.check(code == rhal.SKR_OK and reply["at_rest"], "0.15 m/s for four seconds returns OK with the base at rest", f"{code} {reply}")
    went = travelled(before, after)
    reported = math.hypot(*reply["displacement"])
    L.check(went > 0.01 and abs(reported - went) < max(0.005, 0.05 * went), f"the driver's displacement ({reported:.3f} m) is Gazebo's own ({went:.3f} m): the controller's odometry and the physics agree on what the wheels did")
    ideal = 0.15 * span
    L.check(0.35 * ideal <= went <= 1.3 * ideal, f"and it is of the order of 0.15 m/s over the {span:.1f} s of simulation the call took ({went:.3f} m of {ideal:.2f}), with the controller's 0.9 m/s² ramp and coast")
    along = (after[0] - before[0]) * math.cos(before[2]) + (after[1] - before[1]) * math.sin(before[2])
    L.check(along > 0.9 * went and abs(wrap(after[2] - before[2])) < 0.05, "straight and forward along the heading it had: neither the physics nor the driver saw a turn", f"{along:.3f} {wrap(after[2] - before[2]):.3f}")
    L.check(abs(reply["final_twist"][0]) < 0.011, "at rest, in the reply")
    before, after, span, code, reply = run_motion("velocity vx=-0.1 duration=4.0")
    along = (after[0] - before[0]) * math.cos(before[2]) + (after[1] - before[1]) * math.sin(before[2])
    L.check(code == rhal.SKR_OK and along < -0.01, f"a negative vx drives it backwards, along the heading it has: {along:+.3f} m")

    before, after, span, code, reply = run_motion("velocity wz=0.5 duration=4.0")
    turned = wrap(after[2] - before[2])
    L.check(code == rhal.SKR_OK and turned > 0.1, f"wz = +0.5 rad/s is a counter-clockwise turn in the physics: {turned:+.3f} rad over {span:.1f} s of simulation")
    L.check(abs(reply["yaw_change"] - turned) < 0.05 * abs(turned) + 0.01, f"and the driver's yaw change ({reply['yaw_change']:+.3f}) is the physics' ({turned:+.3f}): the quaternion's order and the factor of two are right")
    L.check(travelled(before, after) < 0.02, "on the spot: the base turned without going anywhere")

    L.section("a command that is said once (the controller's lease is half a second of simulation time)")
    before = truth()
    transport.publish(0.15, 0.0, 0.0)
    time.sleep(1.0)
    mid = truth()
    moved_alone = travelled(before, mid)
    L.check(moved_alone > 0.0005, f"one command, said once, moves the base: {moved_alone * 1000:.1f} mm in the next second")
    lease_wall = 0.5 / max(factor, 0.05)
    time.sleep(lease_wall + 3.0)
    a = truth()
    time.sleep(2.0)
    b = truth()
    L.check(travelled(a, b) < 0.003, f"and it stops by itself, as the controller's timeout says it will ({travelled(a, b) * 1000:.1f} mm in the last two seconds): this is why a velocity is a stream")
    code, reply = L.run(driver, "hold")
    L.check(code == rhal.SKR_OK and reply["at_rest"], "`hold` reports the base at rest")

    L.section("the stop")
    outcome: list = []
    runner = threading.Thread(target=lambda: outcome.append(L.run(driver, "velocity vx=0.2 duration=5.0")))
    runner.start()
    time.sleep(1.5)
    began = time.time()
    L.check(driver.emergency_stop() == rhal.SKR_OK, "the stop returns OK")
    returned = time.time()
    L.check(returned - began < 0.05, f"in {(returned - began) * 1000:.1f} ms, with a velocity in flight")
    runner.join()
    code, text = outcome[0]
    L.check(code == rhal.SKR_EPERM and "emergency stop" in str(text), "and the velocity ends with EPERM, saying it was the stop", str(text))
    time.sleep(3.0 / max(factor, 0.05))
    L.check(at_rest(1.0) < 0.003, "and Gazebo's physics has the base at rest, though the controller would have held the last command until its timeout")
    state = rhal.State()
    driver.read_state(state, 0)
    L.check(state.flags & rhal.FLAG_ESTOP and not state.flags & rhal.FLAG_FAULT, "the state says stopped, and not faulted: the base did stop")
    L.check(driver.release_stop() == rhal.SKR_OK and L.run(driver, "velocity vx=0.1 duration=1.0")[0] == rhal.SKR_OK, "released, it moves again")

    L.section("close")
    driver.close()
    time.sleep(3.0 / max(factor, 0.05))
    L.check(at_rest(1.0) < 0.003, "after close the base is at rest: the last thing it was told was a zero")

    print(f"\n{L.CHECKS} checks, {len(L.FAILURES)} failed")
    for failure in L.FAILURES:
        print(f"  FAIL {failure}")
    return 1 if L.FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
