"""The ROS 2 base driver against ros2_control's own `diff_drive_controller`, in real time (DRY-COVERAGE-PLAN.md item 12.E.3).

Run by `sim/run-ros.sh` inside `sim/Dockerfile`'s image, with the controller and its mock hardware already active:

    python3 live.py <command topic> <odometry topic> <joint states topic> <TwistStamped|Twist>

The driver is opened the way the OS opens it and asked for motions; every claim it makes is held against **a second reader of the same base**, `sim/observer.py`, a process of its own
that uses `rclpy` directly and none of this repository's code: what arrived on the command topic (the driver's own commands, as a second subscriber receives them), what the wheels did
(`joint_state_broadcaster`, off the hardware interface), and what the controller's odometry said. The graph's own facts (a topic's type, who is on it, which controller is active) come
from ROS's command-line tools, not from this driver. Three things the simulated base in `selftest.py` is built to behave like are checked here against the real controller, because the
offline suite stands on them: a command that stops arriving stops the base, a twist of the wrong type is delivered to nobody, and a controller that is not active has no endpoints.

What it cannot show is a motor: the hardware is `mock_components/GenericSystem`, which is never late, never stalls and has no inertia, so what is measured is the driver's and the
controller's arithmetic and the graph's behaviour, and nothing about a base on a floor.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import re
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(ROOT, "..", "..", "bindings", "python"))
sys.path.insert(0, ROOT)

import skynet_rhal as rhal
from skynet_ros2_base import TOKEN, RclpyTransport, Ros2BaseDriver

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


def geometry() -> tuple[float, float]:
    """The wheel radius and separation the controller was configured with, read from the same file the controller read."""
    text = open(os.path.join(ROOT, "sim", "controllers.yaml"), encoding="utf-8").read()
    radius = float(re.search(r"^\s*wheel_radius:\s*([0-9.]+)", text, re.M).group(1))
    separation = float(re.search(r"^\s*wheel_separation:\s*([0-9.]+)", text, re.M).group(1))
    return radius, separation


def ros(*args: str, timeout: float = 30.0) -> str:
    """ROS 2's own command-line tool, as the maker wrote it."""
    done = subprocess.run(["ros2", *args], capture_output=True, text=True, timeout=timeout, check=False)
    return done.stdout + done.stderr


class Observer:
    """`sim/observer.py` as a subprocess, its lines kept in memory."""

    def __init__(self, command_topic: str, message: str, odometry_topic: str, joint_topic: str) -> None:
        self._lock = threading.Lock()
        self.samples: list[dict] = []
        self._child = subprocess.Popen(
            [sys.executable, os.path.join(ROOT, "sim", "observer.py"), command_topic, message, odometry_topic, joint_topic],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1,
        )
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self) -> None:
        assert self._child.stdout is not None
        for line in self._child.stdout:
            try:
                sample = json.loads(line)
            except json.JSONDecodeError:
                continue
            with self._lock:
                self.samples.append(sample)

    def ready(self, timeout: float = 20.0) -> bool:
        deadline = time.time() + timeout
        while time.time() < deadline:
            if any(sample["k"] == "ready" for sample in self.snapshot()):
                return True
            time.sleep(0.1)
        return False

    def snapshot(self, kind: str | None = None, since: float = 0.0, until: float = math.inf) -> list[dict]:
        with self._lock:
            return [s for s in self.samples if (kind is None or s["k"] == kind) and since <= s["t"] <= until]

    def stop(self) -> None:
        self._child.terminate()
        try:
            self._child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self._child.kill()


def wheel_velocities(observer: Observer, since: float, until: float) -> tuple[list[float], list[float]]:
    left, right = [], []
    for sample in observer.snapshot("joint", since, until):
        names = sample["name"]
        if "left_wheel_joint" in names and "right_wheel_joint" in names:
            left.append(sample["velocity"][names.index("left_wheel_joint")])
            right.append(sample["velocity"][names.index("right_wheel_joint")])
    return left, right


def wheel_positions(observer: Observer, at: float, after: bool) -> tuple[float, float] | None:
    candidates = observer.snapshot("joint", at if after else 0.0, math.inf if after else at)
    if not candidates:
        return None
    sample = candidates[0] if after else candidates[-1]
    names = sample["name"]
    return sample["position"][names.index("left_wheel_joint")], sample["position"][names.index("right_wheel_joint")]


def near(values: list[float], target: float, tolerance: float) -> bool:
    return bool(values) and all(abs(value - target) <= tolerance for value in values)


def wait_until(wall_clock_s: float) -> None:
    """Sleep until `time.time()` is `wall_clock_s`. A window of the observer's samples that ends after a motion is read once the window has happened: an empty one is a failure, and a failure for
    having looked early is the check's, not the driver's."""
    remaining = wall_clock_s - time.time()
    if remaining > 0:
        time.sleep(remaining)


def run(driver: Ros2BaseDriver, text: str) -> tuple[int, dict | str]:
    code, data = driver.extension(TOKEN, text.encode())
    decoded = data.decode("utf-8", "replace")
    return code, (json.loads(decoded) if code == rhal.SKR_OK else decoded)


def main() -> int:
    command_topic, odometry_topic, joint_topic, message = sys.argv[1:5]
    radius, separation = geometry()
    base_config = {
        "command_topic": command_topic, "odometry_topic": odometry_topic, "max_linear": 0.5, "max_angular": 1.0, "simulator": True,
        "open_wait_s": 10.0, "stop_settle_s": 3.0, "stale_after_s": 0.5,
    }
    full_type = f"geometry_msgs/msg/{message}"

    section("the graph, by ROS's own tools")
    info = ros("topic", "info", "-v", command_topic)
    check(f"Type: {full_type}" in info, f"`ros2 topic info` says the command topic takes {full_type}", info[:300])
    check("diff_drive_controller" in ros("control", "list_controllers") and "active" in ros("control", "list_controllers"), "`ros2 control list_controllers` shows diff_drive_controller active")
    odom_info = ros("topic", "info", odometry_topic)
    check("Type: nav_msgs/msg/Odometry" in odom_info and "Publisher count: 1" in odom_info, "and the odometry topic is nav_msgs/Odometry with one publisher", odom_info[:300])

    observer = Observer(command_topic, message, odometry_topic, joint_topic)
    check(observer.ready(), "the second reader is up on the command topic, the odometry and the wheels' joint state")
    time.sleep(1.0)
    check(bool(observer.snapshot("joint")) and bool(observer.snapshot("odom")), "and it is hearing both")

    section("open: the type is read off the graph and agrees with ROS's own tool")
    driver = Ros2BaseDriver(lambda settings: RclpyTransport(settings.command_topic, settings.odometry_topic, domain_id=settings.domain_id, frame_id=settings.frame_id))
    began = time.monotonic()
    check(driver.open(json.dumps(base_config)) == rhal.SKR_OK, "the driver opens on the controller's two topics")
    check(time.monotonic() - began < 8.0, f"in {time.monotonic() - began:.1f} s")
    code, status = run(driver, "status")
    check(code == rhal.SKR_OK and status["message_type"] == full_type, f"it chose {full_type}: the type the controller's own subscriber reports", str(status))
    check(status["subscribers"] >= 1 and status["twist_frame"] == "base_link" and status["odometry_frame"] == "odom", "it is connected to the controller, and the odometry's frames are the controller's own (odom, base_link)", str(status))
    code, state = 0, rhal.State()
    code = driver.read_state(state, 0)
    check(code == rhal.SKR_OK and state.flags == rhal.HOMING_NOT_REQUIRED, "and the state reads clean", f"{code} {state.flags:#x}")
    descriptor = json.loads(driver.describe())
    check(descriptor["robot_slug"] == "ros2-base-diff-drive-controller" and descriptor["base"] == "wheeled", "the descriptor names the controller's namespace and a wheeled base", descriptor["robot_slug"])
    # The driver's publisher is matched with the controller at open (it refuses to open otherwise) and with the second reader a little later: DDS discovers each endpoint on its
    # own schedule, and a motion begun before the second reader is matched is half-heard (19 of 40 commands, the first time this was run). The driver counts its own subscribers,
    # so the wait is on the driver's own fact, not on a sleep.
    matched_by = time.monotonic() + 15.0
    while time.monotonic() < matched_by and run(driver, "status")[1]["subscribers"] < 2:
        time.sleep(0.2)
    check(run(driver, "status")[1]["subscribers"] >= 2, "the driver's publisher is connected to both the controller and the second reader before the first motion")

    section("velocity: what arrives on the topic, and what the wheels do")
    started = time.time()
    code, reply = run(driver, "velocity vx=0.2 duration=2.0")
    finished = time.time()
    check(code == rhal.SKR_OK and reply["at_rest"], "0.2 m/s for two seconds returns OK with the base at rest", f"{code} {reply}")
    commands = observer.snapshot("command", started - 0.05, finished + 0.2)
    moving = [c for c in commands if (c["vx"], c["vy"], c["wz"]) != (0.0, 0.0, 0.0)]
    check(30 <= len(moving) <= 42 and {(c["vx"], c["vy"], c["wz"]) for c in moving} == {(0.2, 0.0, 0.0)}, f"a second reader on the topic got {len(moving)} commands, every one exactly 0.2 m/s, as the driver says it sent", str(moving[:1]))
    gaps = [b["t"] - a["t"] for a, b in zip(moving, moving[1:])]
    check(bool(gaps) and max(gaps) < 0.25, f"with the longest silence {max(gaps) * 1000:.0f} ms against the controller's 500 ms timeout")
    check(commands[-1]["vx"] == 0.0 and commands[-1]["wz"] == 0.0, "and the last command it got is zero")
    if message == "TwistStamped":
        check(all(c.get("frame") == "base_link" for c in moving) and all(abs(c["stamp"] - c["t"]) < 1.0 for c in moving), "stamped commands carry the configured frame and the ROS clock's time of sending")
    left, right = wheel_velocities(observer, started + 0.5, finished - 0.3)
    check(near(left, 0.2 / radius, 0.2) and near(right, 0.2 / radius, 0.2), f"the wheels turned at v/r = {0.2 / radius:.1f} rad/s, both the same way", f"left {left[:1]}.. right {right[:1]}..")
    wait_until(finished + 1.2)
    after = wheel_velocities(observer, finished + 0.5, finished + 1.0)
    check(near(after[0], 0.0, 0.05) and near(after[1], 0.0, 0.05), "and are still afterwards")
    before_pos, after_pos = wheel_positions(observer, started, False), wheel_positions(observer, finished + 0.5, True)
    if before_pos and after_pos:
        travelled = radius * ((after_pos[0] - before_pos[0]) + (after_pos[1] - before_pos[1])) / 2.0
        check(abs(reply["displacement"][0] - travelled) < 0.03 and abs(reply["displacement"][1]) < 0.02, f"the driver's displacement {reply['displacement'][0]:.3f} m is what the wheels' own positions integrate to ({travelled:.3f} m)")
    else:
        check(False, "the wheels' positions were read before and after")
    code, state = 0, rhal.State()
    driver.read_state(state, 0)
    last = observer.snapshot("odom")[-1]
    heading = math.atan2(2.0 * (state.base_pose[6] * state.base_pose[5] + state.base_pose[3] * state.base_pose[4]), 1.0 - 2.0 * (state.base_pose[4] ** 2 + state.base_pose[5] ** 2))
    check(abs(state.base_pose[0] - last["x"]) < 1e-3 and abs(state.base_pose[1] - last["y"]) < 1e-3 and abs(heading - last["yaw"]) < 1e-3, "the pose the driver reports is the one a second subscriber read off the same odometry", f"{state.base_pose[:3]} vs {last['x']}, {last['y']}")
    check(state.base_pose[3:7] == [0.0, 0.0, state.base_pose[5], state.base_pose[6]] or abs(state.base_pose[3]) < 1e-9, "with the quaternion in x y z w order")

    section("velocity: a turn, and a turn while going forward")
    started = time.time()
    code, reply = run(driver, "velocity wz=0.5 duration=2.0")
    finished = time.time()
    left, right = wheel_velocities(observer, started + 0.5, finished - 0.3)
    expect = 0.5 * separation / 2.0 / radius
    wait_until(finished + 1.2)
    check(code == rhal.SKR_OK and near(left, -expect, 0.2) and near(right, expect, 0.2), f"wz = +0.5 rad/s turns the left wheel back and the right forward at {expect:.1f} rad/s: counter-clockwise, as ROS means it", f"{code} left {left[:1]} right {right[:1]}")
    check(0.8 < reply["yaw_change"] < 1.2, f"and the driver's yaw change is {reply['yaw_change']:.2f} rad, about 0.5 × 2 s")
    before, after = wheel_positions(observer, started, False), wheel_positions(observer, finished + 0.5, True)
    if before and after:
        wheels = radius * ((after[1] - before[1]) - (after[0] - before[0])) / separation
        check(abs(reply["yaw_change"] - wheels) < 0.05, f"which is what the wheels' positions integrate to ({wheels:.2f} rad)")
    started = time.time()
    code, reply = run(driver, "velocity vx=0.15 wz=0.4 duration=2.0")
    finished = time.time()
    left, right = wheel_velocities(observer, started + 0.5, finished - 0.3)
    expect_left, expect_right = (0.15 - 0.4 * separation / 2.0) / radius, (0.15 + 0.4 * separation / 2.0) / radius
    check(code == rhal.SKR_OK and near(left, expect_left, 0.2) and near(right, expect_right, 0.2), f"both together are v ∓ wL/2 over r: {expect_left:.1f} and {expect_right:.1f} rad/s", f"left {left[:1]} right {right[:1]}")

    section("a command that stops arriving stops the base (what the simulated base's timeout stands on)")
    transport = driver._transport
    started = time.time()
    transport.publish(0.2, 0.0, 0.0)
    time.sleep(2.0)
    left, right = wheel_velocities(observer, started, started + 2.0)
    moving = [t for t, v in zip(observer.snapshot("joint", started, started + 2.0), left) if abs(v) > 0.5 / 1.0]
    check(bool(moving), "one command moves the wheels")
    stopped_at = next((s["t"] for s, v in zip(observer.snapshot("joint", started, started + 2.0), left) if s["t"] > started + 0.2 and abs(v) < 0.05), None)
    check(stopped_at is not None and 0.35 < stopped_at - started < 1.0, f"and they stop by themselves {stopped_at - started:.2f} s later, the controller's 0.5 s timeout" if stopped_at else "and they stop by themselves")

    section("a twist of the wrong type reaches nobody (what the simulated base's type matching stands on)")
    import rclpy
    from geometry_msgs.msg import Twist, TwistStamped

    rclpy.init()
    node = rclpy.create_node("skynet_ros2_base_live_wrong")
    wrong_type = Twist if message == "TwistStamped" else TwistStamped
    publisher = node.create_publisher(wrong_type, command_topic, 10)
    time.sleep(1.5)
    check(publisher.get_subscription_count() == 0, f"a {wrong_type.__name__} publisher on the command topic is connected to no subscriber, and says so")
    started = time.time()
    for _ in range(20):
        sample = wrong_type()
        (sample.twist if wrong_type is TwistStamped else sample).linear.x = 0.2
        publisher.publish(sample)
        time.sleep(0.05)
    time.sleep(0.5)
    left, right = wheel_velocities(observer, started, time.time())
    check(near(left, 0.0, 0.05) and near(right, 0.0, 0.05), "and a second of those moves nothing: no error anywhere, only the connection count, which is what the driver reads")
    node.destroy_node()
    rclpy.try_shutdown()

    section("the stop")
    outcome: list = []
    runner = threading.Thread(target=lambda: outcome.append(run(driver, "velocity vx=0.3 duration=4.0")))
    runner.start()
    time.sleep(1.2)
    began = time.time()
    check(driver.emergency_stop() == rhal.SKR_OK, "the stop returns OK")
    returned = time.time()
    check(returned - began < 0.05, f"in {(returned - began) * 1000:.1f} ms, with a velocity in flight")
    runner.join()
    code, text = outcome[0]
    check(code == rhal.SKR_EPERM and "emergency stop" in str(text), "and the velocity ends with EPERM, saying it was the stop", str(text))
    wait_until(returned + 1.2)
    late = [c for c in observer.snapshot("command", returned + 0.002) if (c["vx"], c["wz"]) != (0.0, 0.0)]
    check(late == [], "no command asking for motion reached the second reader after the stop returned", str(late[:1]))
    left, right = wheel_velocities(observer, returned + 0.7, time.time())
    check(near(left, 0.0, 0.05) and near(right, 0.0, 0.05), "and the wheels are at rest 0.7 s after it")
    zeros = [c for c in observer.snapshot("command", returned + 0.1)]
    check(len(zeros) >= 5 and all(c["vx"] == 0.0 for c in zeros), "while latched the driver keeps publishing zero, and the second reader hears it", str(len(zeros)))
    code, state = 0, rhal.State()
    driver.read_state(state, 0)
    check(state.flags & rhal.FLAG_ESTOP and not state.flags & rhal.FLAG_FAULT, "the state says stopped, and not faulted: the base did stop")
    check(driver.release_stop() == rhal.SKR_OK and run(driver, "velocity vx=0.1 duration=0.5")[0] == rhal.SKR_OK, "released, it moves again")

    section("the controller leaves the graph and comes back (what the simulated base's lifecycle stands on)")
    ros("control", "set_controller_state", "diff_drive_controller", "inactive")
    time.sleep(2.5)
    state = rhal.State()
    code = driver.read_state(state, 0)
    check(code == rhal.SKR_ETIMEDOUT and state.flags & rhal.FLAG_DEGRADED, "with the controller inactive the odometry stops and the state is ETIMEDOUT and DEGRADED", f"{code} {state.flags:#x}")
    code, text = run(driver, "velocity vx=0.1 duration=0.5")
    check(code == rhal.SKR_EIO and "not current" in str(text), "a velocity is refused, saying the odometry is not current", str(text))
    second = Ros2BaseDriver(lambda settings: RclpyTransport(settings.command_topic, settings.odometry_topic))
    check(second.open(json.dumps({**base_config, "open_wait_s": 2.0})) == rhal.SKR_EIO, "and a driver opened now is refused: no odometry and nobody on the command topic")
    ros("control", "set_controller_state", "diff_drive_controller", "active")
    time.sleep(2.5)
    state = rhal.State()
    code = driver.read_state(state, 0)
    check(code == rhal.SKR_OK and not state.flags & rhal.FLAG_DEGRADED, "reactivated, the state recovers by itself", f"{code} {state.flags:#x}")
    code, reply = run(driver, "velocity vx=0.1 duration=0.5")
    check(code == rhal.SKR_OK, "and the next motion runs: the publisher reconnected to the controller that came back", str(reply))

    section("the refusals a real graph makes possible")
    for what, overrides, words, wait in [
        ("a command topic nobody listens on", {"command_topic": "/cmd_vel"}, ("has no subscriber",), 8.0),
        ("the wrong message type", {"message_type": "Twist" if message == "TwistStamped" else "TwistStamped"}, ("message_type", message), 8.0),
        # Nothing is there to be found in another domain, so this waits its whole window: the short one.
        ("another ROS domain", {"domain_id": 77}, ("nothing arrived",), 2.0),
    ]:
        other = Ros2BaseDriver(lambda settings: RclpyTransport(settings.command_topic, settings.odometry_topic, domain_id=settings.domain_id))
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = other.open(json.dumps({**base_config, "open_wait_s": wait, **overrides}))
        check(code == rhal.SKR_EIO and all(word in said.getvalue() for word in words), f"{what} is refused at open, and the words say why", said.getvalue()[:300])

    section("close")
    driver.close()
    time.sleep(0.5)
    last = observer.snapshot("command")[-1]
    check((last["vx"], last["wz"]) == (0.0, 0.0), "after close the last thing on the command topic is zero")
    observer.stop()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())
