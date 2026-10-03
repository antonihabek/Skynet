"""The ROS 2 base driver against ROBOTIS's own TurtleBot3 simulation, in real time (DRY-COVERAGE-PLAN.md item 12.E.3, the forty-seventh session).

Run by `sim/run-tb3.sh` inside `sim/Dockerfile.turtlebot3`'s image, with Gazebo's server, the burger spawned and the bridge up:

    python3 live_tb3.py

`live.py` holds the driver to `ros2_control`'s controller, the ROS project's implementation of the two topics. This holds it to a *robot's* simulator, F118's rung 2: Gazebo's physics
engine with a differential-drive plugin that has an acceleration limit and **holds the last command until it gets another** (the controller `live.py` runs against zeroes a command after
half a second; this does not), and odometry the plugin computes from the wheels it turned.

The second opinion is not another ROS node. It is **Gazebo's own pose of the model**, read with `gz topic` off the world's pose stream: the physics engine's idea of where the base is, that
shares nothing with the bridge, the odometry plugin or this driver. Every claim the driver makes about where the base went is held to it.

What it cannot show: a real TurtleBot3's motors, battery, floor and sensors. The simulation is the maker's, the robot is not.
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
from skynet_ros2_base import TOKEN, RclpyTransport, Ros2BaseDriver

WORLD = "default"
MODEL = "burger"
#: The burger's own figure, from its model.sdf's DiffDrive plugin: `max_linear_acceleration` is 1 m/s². (The same SDF says `odom_publisher_frequency` is 30 Hz and the plugin publishes at
#: about 50: so the odometry's rate is measured by ROS's own tool and held to what the driver hears, and not taken from a file.)
MAX_LINEAR_ACCELERATION = 1.0

_BLOCK = re.compile(r'pose\s*\{\s*name:\s*"%s"(.*?)\n\}' % re.escape(MODEL), re.S)
_NUMBER = r"(-?[0-9.]+(?:e[-+]?[0-9]+)?)"


def truth() -> tuple[float, float, float] | None:
    """Where Gazebo's physics says the model is: x, y, yaw. `gz topic` prints a protobuf text message and leaves out a field that is zero."""
    done = subprocess.run(["gz", "topic", "-e", "-t", f"/world/{WORLD}/pose/info", "-n", "1"], capture_output=True, text=True, timeout=15, check=False)
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


def main() -> int:
    config = {"command_topic": "/cmd_vel", "odometry_topic": "/odom", "max_linear": 0.22, "max_angular": 2.0, "simulator": True, "open_wait_s": 20.0, "stop_settle_s": 4.0, "stale_after_s": 0.5}

    L.section("the graph, by ROS's own tools and by Gazebo's")
    info = L.ros("topic", "info", "-v", "/cmd_vel")
    L.check("Type: geometry_msgs/msg/TwistStamped" in info and "ros_gz_bridge" in info, "`ros2 topic info` says /cmd_vel takes geometry_msgs/msg/TwistStamped, from the bridge", info[:300])
    odom_info = L.ros("topic", "info", "/odom")
    L.check("Type: nav_msgs/msg/Odometry" in odom_info and "Publisher count: 1" in odom_info, "and /odom is nav_msgs/Odometry with one publisher", odom_info[:300])
    listed = subprocess.run(["gz", "topic", "-l"], capture_output=True, text=True, timeout=15, check=False).stdout
    L.check("/cmd_vel" in listed and "/odom" in listed, "and Gazebo's own side of the same two topics exists: the plugin subscribes /cmd_vel and publishes /odom")
    start = truth()
    L.check(start is not None, f"Gazebo's pose stream names the model: {MODEL} is at {None if start is None else tuple(round(v, 3) for v in start)}")
    if start is None:
        print(subprocess.run(["gz", "topic", "-e", "-t", f"/world/{WORLD}/pose/info", "-n", "1"], capture_output=True, text=True, timeout=15, check=False).stdout[:600])
        return 1

    L.section("open: the type is read off the graph, and the frames are the maker's")
    driver = Ros2BaseDriver(lambda settings: RclpyTransport(settings.command_topic, settings.odometry_topic, domain_id=settings.domain_id, frame_id=settings.frame_id))
    began = time.monotonic()
    L.check(driver.open(json.dumps(config)) == rhal.SKR_OK, f"the driver opens on /cmd_vel and /odom in {time.monotonic() - began:.1f} s")
    code, status = L.run(driver, "status")
    L.check(code == rhal.SKR_OK and status["message_type"] == "geometry_msgs/msg/TwistStamped" and status["subscribers"] >= 1, "it chose TwistStamped, the type the bridge's subscriber reports, and is connected to it", str(status))
    L.check(status["odometry_frame"] == "odom" and status.get("twist_frame") is not None, f"the odometry's frame is the plugin's own, `odom`; the commands carry `{status.get('twist_frame')}`")
    transport = driver._transport
    samples = set()
    deadline = time.monotonic() + 3.0
    while time.monotonic() < deadline:
        latest = transport.odometry()
        if latest is not None:
            samples.add(latest.header_s)
        time.sleep(0.01)
    rate = len(samples) / 3.0
    hz = subprocess.run(["timeout", "6", "ros2", "topic", "hz", "--window", "100", "/odom"], capture_output=True, text=True, check=False).stdout
    measured = re.findall(r"average rate:\s*([0-9.]+)", hz)
    reference = float(measured[-1]) if measured else math.nan
    L.check(not math.isnan(reference) and abs(rate - reference) < 0.15 * reference, f"it hears the plugin's odometry at the rate ROS's own `ros2 topic hz` measures: {rate:.1f} Hz against {reference:.1f} Hz (none dropped by a short queue)")
    state = rhal.State()
    L.check(driver.read_state(state, 0) == rhal.SKR_OK and state.flags == rhal.HOMING_NOT_REQUIRED, "and the state reads clean")

    L.section("velocity: where Gazebo's physics says the base went")
    before = truth()
    code, reply = L.run(driver, "velocity vx=0.15 duration=3.0")
    time.sleep(0.5)
    after = truth()
    assert before is not None and after is not None
    L.check(code == rhal.SKR_OK and reply["at_rest"], "0.15 m/s for three seconds returns OK with the base at rest", f"{code} {reply}")
    went = travelled(before, after)
    ideal = 0.15 * 3.0
    L.check(abs(reply["displacement"][0] - went) < 0.03 and abs(math.hypot(*reply["displacement"]) - went) < 0.03,
            f"the driver's displacement ({math.hypot(*reply['displacement']):.3f} m) is Gazebo's own ({went:.3f} m): physics and the plugin's odometry agree on what the wheels did")
    L.check(0.7 * ideal <= went <= 1.1 * ideal, f"and it is what 0.15 m/s for three seconds comes to with a {MAX_LINEAR_ACCELERATION:g} m/s² ramp, give or take the simulation's clock ({went:.3f} m of {ideal:.2f})")
    L.check(abs(wrap(after[2] - before[2])) < 0.03 and abs(reply["yaw_change"]) < 0.03, "straight: neither the physics nor the driver saw a turn")
    L.check(reply["final_twist"][0] == 0.0 or abs(reply["final_twist"][0]) < 0.011, "at rest, in the reply")
    heading = after[2]
    before = after
    code, reply = L.run(driver, "velocity vx=-0.1 duration=2.0")
    time.sleep(0.5)
    after = truth()
    along = (after[0] - before[0]) * math.cos(heading) + (after[1] - before[1]) * math.sin(heading)
    L.check(code == rhal.SKR_OK and along < -0.12, f"a negative vx drives it backwards, along the heading it has: {along:+.3f} m")

    before = truth()
    code, reply = L.run(driver, "velocity wz=0.5 duration=2.0")
    time.sleep(0.5)
    after = truth()
    turned = wrap(after[2] - before[2])
    L.check(code == rhal.SKR_OK and 0.7 < turned < 1.2, f"wz = +0.5 rad/s for two seconds is a counter-clockwise turn of {turned:+.2f} rad in the physics")
    L.check(abs(reply["yaw_change"] - turned) < 0.05, f"and the driver's yaw change ({reply['yaw_change']:+.3f}) is the physics' ({turned:+.3f}): the quaternion's order and the factor of two are right")
    L.check(travelled(before, after) < 0.03, "on the spot: the base turned without going anywhere")

    L.section("a base that holds its last command (what the controller of live.py does not do)")
    before = truth()
    transport.publish(0.15, 0.0, 0.0)
    time.sleep(2.5)
    mid = truth()
    moved_alone = travelled(before, mid)
    time.sleep(0.5)
    later = truth()
    L.check(moved_alone > 0.25 and travelled(mid, later) > 0.05, f"one command, said once, and the base is still going {3.0:.0f} seconds later: {moved_alone:.2f} m, and {travelled(mid, later):.2f} m more in the last half second. This is why a velocity ends in a zero")
    state = rhal.State()
    driver.read_state(state, 0)
    L.check(abs(state.base_twist[0] - 0.15) < 0.03, "and the driver reads the same from the odometry: 0.15 m/s still")
    code, reply = L.run(driver, "hold")
    time.sleep(0.5)
    a, b = truth(), None
    time.sleep(0.5)
    b = truth()
    L.check(code == rhal.SKR_OK and reply["at_rest"] and travelled(a, b) < 0.003, "and `hold` brings it to rest, which the physics agrees with: two poses half a second apart are the same")

    L.section("the stop")
    outcome: list = []
    runner = threading.Thread(target=lambda: outcome.append(L.run(driver, "velocity vx=0.2 duration=4.0")))
    runner.start()
    time.sleep(1.2)
    began = time.time()
    L.check(driver.emergency_stop() == rhal.SKR_OK, "the stop returns OK")
    returned = time.time()
    L.check(returned - began < 0.05, f"in {(returned - began) * 1000:.1f} ms, with a velocity in flight")
    runner.join()
    code, text = outcome[0]
    L.check(code == rhal.SKR_EPERM and "emergency stop" in str(text), "and the velocity ends with EPERM, saying it was the stop", str(text))
    time.sleep(1.0)
    a = truth()
    time.sleep(0.6)
    b = truth()
    L.check(travelled(a, b) < 0.003, f"and Gazebo's physics has the base at rest within a second and a half of the stop ({travelled(a, b) * 1000:.1f} mm in the next 0.6 s), though the plugin would have held the last command for ever")
    state = rhal.State()
    driver.read_state(state, 0)
    L.check(state.flags & rhal.FLAG_ESTOP and not state.flags & rhal.FLAG_FAULT, "the state says stopped, and not faulted: the base did stop")
    L.check(driver.release_stop() == rhal.SKR_OK and L.run(driver, "velocity vx=0.1 duration=0.5")[0] == rhal.SKR_OK, "released, it moves again")

    L.section("close")
    driver.close()
    time.sleep(0.5)
    a = truth()
    time.sleep(0.6)
    b = truth()
    L.check(travelled(a, b) < 0.003, "after close the base is at rest: the last thing it was told was a zero")

    print(f"\n{L.CHECKS} checks, {len(L.FAILURES)} failed")
    for failure in L.FAILURES:
        print(f"  FAIL {failure}")
    return 1 if L.FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
