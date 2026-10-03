"""skynet-ros2-base's own suite: the configuration, the graph it will and will not open on, the state, the motions, the stop, and the host path.

The driver runs here against `SimulatedBase`, a ROS 2 mobile base written in this tree that behaves as `ros2_control`'s `diff_drive_controller` does where it matters to a driver (it zeroes
a command that stops arriving, it does not deliver a twist of the wrong type, an inactive controller has no endpoints). It needs no ROS. `live.py` runs the same driver against the real
controller and checks those three properties of the simulated base against it; this file is the other half of that, the part that can be run on any machine and seeded with defects.

Two halves. The first drives `Ros2BaseDriver` directly. The second speaks the `python-host` pipe protocol to `python -m skynet_ros2_base` as a subprocess (`SKYNET_ROS2_BASE_SIM=1`),
which is the driver's half of the `T` verb end to end. `skynet driver check` through the real ABI is `make check`'s third step.
"""

from __future__ import annotations

import base64
import contextlib
import dataclasses
import io
import json
import math
import os
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(ROOT, "..", "..", "bindings", "python"))
sys.path.insert(0, ROOT)

import skynet_rhal as rhal
from skynet_ros2_base import TOKEN, Ros2BaseDriver, SimulatedBase, TransportError
from skynet_ros2_base.transport import TWIST, TWIST_STAMPED, RclpyTransport

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


def configuration(**overrides) -> dict:
    base = {"max_linear": 0.5, "max_angular": 1.0, "open_wait_s": 1.0, "stop_settle_s": 1.0, "stale_after_s": 0.3}
    base.update(overrides)
    return base


class Rig:
    """One driver on one simulated base, with the base left reachable so a check can read what it really did."""

    def __init__(self, sim: dict | None = None, **config) -> None:
        self.base = SimulatedBase(**{"rate_hz": 100.0, **(sim or {})})
        self.driver = Ros2BaseDriver(lambda settings: self.base)
        self.config = configuration(**config)
        self.stderr = ""

    def open(self) -> int:
        buffer = io.StringIO()
        with contextlib.redirect_stderr(buffer):
            code = self.driver.open(json.dumps(self.config))
        self.stderr = buffer.getvalue()
        return code

    def opened(self) -> "Rig":
        code = self.open()
        assert code == rhal.SKR_OK, f"the rig did not open ({code}): {self.stderr}"
        return self

    def say(self, text: str) -> tuple[int, str]:
        code, data = self.driver.extension(TOKEN, text.encode())
        return code, data.decode("utf-8", "replace")

    def motion(self, text: str) -> tuple[int, dict | str]:
        code, text_out = self.say(text)
        return code, (json.loads(text_out) if code == rhal.SKR_OK else text_out)

    def read(self) -> tuple[int, rhal.State]:
        state = rhal.State()
        return self.driver.read_state(state, 0), state

    def commands(self) -> list:
        with self.base._lock:
            return list(self.base.received)

    def close(self) -> None:
        self.driver.close()
        self.base.stop()


def nonzero(received) -> list:
    return [command for command in received if (command.vx, command.vy, command.wz) != (0.0, 0.0, 0.0)]


def refused_with(code: int, text, wanted: int, *words: str) -> bool:
    return code == wanted and all(word in str(text) for word in words)


def configuration_checks() -> None:
    section("open: the configuration is refused in words that name the field")
    check(Rig().open() == rhal.SKR_OK, "a configuration with both speed limits opens against a base that is there")
    rig = Rig().opened()
    check(rig.driver.open(json.dumps(rig.config)) == rhal.SKR_EPERM, "a second open() is refused")
    rig.close()
    for what, text, words in [
        ("not JSON", "not json", ("not JSON",)),
        ("a JSON list", "[1]", ("JSON object",)),
        ("a key it does not read", json.dumps(configuration(max_speed=1)), ("max_speed", "max_linear")),
        ("no max_linear", json.dumps({"max_angular": 1.0}), ("max_linear", "decision")),
        ("no max_angular", json.dumps({"max_linear": 0.5}), ("max_angular", "decision")),
    ]:
        rig = Rig()
        buffer = io.StringIO()
        with contextlib.redirect_stderr(buffer):
            code = rig.driver.open(text)
        check(refused_with(code, buffer.getvalue(), rhal.SKR_EINVAL, *words), f"{what} is EINVAL and the words name the cause", buffer.getvalue())
        check(rig.base.opened == 0, f"and {what} never reaches the graph")
    for field, value in [
        ("max_linear", 0), ("max_linear", -0.5), ("max_linear", "0.5"), ("max_linear", True), ("max_linear", None), ("max_angular", 0), ("max_angular", -1),
        ("max_angular", "fast"), ("message_type", "Accel"), ("message_type", [1]), ("base", "legged"), ("holonomic", "yes"), ("simulator", 1), ("domain_id", 233), ("domain_id", -1),
        ("domain_id", True), ("domain_id", "0"), ("command_topic", "cmd vel"), ("command_topic", ""), ("command_topic", "/"), ("odometry_topic", "/odom/"),
        ("frame_id", ""), ("command_period_s", 0), ("command_period_s", 0.5), ("stale_after_s", -1), ("stop_settle_s", 0), ("rest_tolerance", 0), ("open_wait_s", 0),
    ]:
        rig = Rig(**{field: value})
        check(rig.open() == rhal.SKR_EINVAL and field in rig.stderr, f"{field}={value!r} is refused, and the words name {field}", rig.stderr)
    for text in ('{"max_linear": NaN, "max_angular": 1}', '{"max_linear": Infinity, "max_angular": 1}'):
        rig = Rig()
        buffer = io.StringIO()
        with contextlib.redirect_stderr(buffer):
            code = rig.driver.open(text)
        check(code == rhal.SKR_EINVAL and "finite" in buffer.getvalue(), f"a speed limit of {text.split(':')[1].split(',')[0].strip()} is refused as not finite")
    rig = Rig(command_period_s=0.5)
    rig.open()
    check("0.5" in rig.stderr and "cmd_vel_timeout" in rig.stderr, "a command period past a fifth of a base's own timeout says why it stutters")


def graph_checks() -> None:
    section("open: it joins only a graph with a base on both topics, and says which is missing")
    rig = Rig().opened()
    code, status = rig.motion("status")
    check(code == rhal.SKR_OK and status["message_type"] == TWIST_STAMPED, "against a controller that takes TwistStamped the driver publishes TwistStamped (Jazzy)", str(status))
    check(status["subscribers"] == 1 and status["command_topic"] == "/cmd_vel" and status["odometry_topic"] == "/odom", "and status names the topics and one connected subscriber")
    rig.close()
    rig = Rig({"command_type": TWIST}).opened()
    code, status = rig.motion("status")
    check(code == rhal.SKR_OK and status["message_type"] == TWIST, "against one that takes plain Twist (Humble) it publishes Twist: the type is read off the graph")
    rig.motion("velocity vx=0.2 duration=0.2")
    check(all(command.type == TWIST for command in rig.commands()), "and every command it sent was that type")
    rig.close()

    rig = Rig({"command_type": TWIST}, message_type="TwistStamped")
    check(rig.open() == rhal.SKR_EIO and "TwistStamped" in rig.stderr and TWIST in rig.stderr and "message_type" in rig.stderr,
          "a configured type the subscriber does not take is refused, naming both types: a publisher of the other one is connected to nobody", rig.stderr)
    check(rig.base.closed == 1, "and the refusal closes the connection it made")
    rig = Rig({"command_type": TWIST}, message_type="Twist")
    check(rig.open() == rhal.SKR_OK, "a configured type that matches the subscriber opens")
    rig.close()
    rig = Rig({"also_takes": TWIST})
    check(rig.open() == rhal.SKR_EIO and "both" in rig.stderr and "message_type" in rig.stderr, "subscribers of both types are refused: the driver does not guess which is the base", rig.stderr)
    rig = Rig({"also_takes": TWIST}, message_type="TwistStamped")
    check(rig.open() == rhal.SKR_OK, "and the configuration settles it")
    rig.close()
    rig = Rig({"command_type": "geometry_msgs/msg/Accel"})
    check(rig.open() == rhal.SKR_EIO and "Accel" in rig.stderr, "a subscriber that takes something that is not a twist is refused, naming what it takes", rig.stderr)

    rig = Rig({"active": False})
    check(rig.open() == rhal.SKR_EIO and "/odom" in rig.stderr and "ros2 control list_controllers" in rig.stderr and "no publisher" in rig.stderr,
          "a controller that is not active (no odometry, and the graph shows no publisher) is refused naming the topic and the command that shows controller state", rig.stderr)
    rig = Rig({"odometry_silent": True})
    check(rig.open() == rhal.SKR_EIO and "shows a publisher" in rig.stderr and "QoS" in rig.stderr,
          "a publisher that is there and silent is refused differently: the words point at QoS and the domain, not at a missing controller", rig.stderr)
    rig = Rig({"listening": False})
    check(rig.open() == rhal.SKR_EIO and "/cmd_vel has no subscriber" in rig.stderr and "nothing would fail" in rig.stderr,
          "odometry with nobody listening on the command topic is refused: a command would go nowhere and nothing would fail", rig.stderr)
    rig = Rig({"unconnectable": True})
    check(rig.open() == rhal.SKR_EIO and "QoS" in rig.stderr and "ROS_DOMAIN_ID" in rig.stderr, "a subscriber the publisher never connects to is refused as a QoS or domain mismatch", rig.stderr)
    for sim in ({"active": False}, {"listening": False}, {"unconnectable": True}):
        rig = Rig(sim)
        rig.open()
        check(rig.base.closed == 1, f"and the refusal ({list(sim)[0]}) leaves nothing joined")
    rig = Rig({"child_frame_blank": True})
    check(rig.open() == rhal.SKR_OK and "child_frame_id" in rig.stderr, "odometry with no child frame still opens, and says the twist's frame is unknown")
    code, status = rig.motion("status")
    check(status["twist_frame"] == "" and status["odometry_frame"] == "odom", "and status carries both frames as published")
    rig.close()
    rig = Rig(command_topic="/diff_drive_controller/cmd_vel", odometry_topic="/diff_drive_controller/odom").opened()
    code, status = rig.motion("status")
    check(status["command_topic"] == "/diff_drive_controller/cmd_vel" and status["odometry_topic"] == "/diff_drive_controller/odom", "the topics are the configuration's, not the defaults")
    rig.close()

    def failing(settings):
        raise TransportError("ROS 2's Python client (rclpy) cannot be imported here. Source the ROS 2 install before the driver starts.")

    driver = Ros2BaseDriver(failing)
    buffer = io.StringIO()
    with contextlib.redirect_stderr(buffer):
        code = driver.open(json.dumps(configuration()))
    check(code == rhal.SKR_EIO and "Source the ROS 2 install" in buffer.getvalue(), "a graph that cannot be reached is EIO, and the words say what to source")
    if "rclpy" not in sys.modules:
        try:
            import rclpy  # noqa: F401
        except ImportError:
            transport = RclpyTransport("/cmd_vel", "/odom")
            try:
                transport.open()
                raised = ""
            except TransportError as error:
                raised = str(error)
            check("rclpy" in raised and "setup" in raised, "and the real transport, with no ROS here, says it needs a sourced install and names the setup script", raised)
        else:
            print("  note  rclpy is importable here, so the missing-ROS check is live.py's")


def simulated_base_checks() -> None:
    section("the simulated base is what it says it is (these are the three properties live.py holds to the real controller)")
    base = SimulatedBase(rate_hz=100.0)
    base.start()
    base.prepare(TWIST_STAMPED)
    start = base.truth()
    base.publish(0.3, 0.0, 0.0)
    time.sleep(0.9)
    moved = base.truth()[0] - start[0]
    check(0.05 < moved < 0.2, f"a base with a 0.5 s command timeout, given one command, stops by itself: {moved:.3f} m, not the {0.3 * 0.9:.2f} m of a held command")
    base.stop()
    held = SimulatedBase(rate_hz=100.0, command_timeout_s=None)
    held.start()
    held.prepare(TWIST_STAMPED)
    held.publish(0.3, 0.0, 0.0)
    time.sleep(0.6)
    check(abs(held.truth()[3] - 0.3) < 0.01, "and one with no timeout is still going at 0.3 m/s 0.6 s after its one command")
    held.stop()
    wrong = SimulatedBase(rate_hz=100.0, command_type=TWIST_STAMPED)
    wrong.start()
    wrong.prepare(TWIST)
    wrong.publish(0.3, 0.0, 0.0)
    time.sleep(0.2)
    check(wrong.subscribers() == 0 and not wrong.received and wrong.truth()[3] == 0.0, "a Twist published at a TwistStamped subscriber is delivered to nobody, and subscribers() is the only place that shows")
    wrong.stop()
    idle = SimulatedBase(rate_hz=100.0, active=False)
    idle.start()
    time.sleep(0.1)
    check(idle.odometry() is None and idle.command_types() == () and idle.odometry_publishers() == 0, "an inactive controller has no odometry and no endpoints")
    idle.stop()


def descriptor_checks() -> None:
    section("the body: no joints, a base, and the motions it declares")
    rig = Rig(command_topic="/diff_drive_controller/cmd_vel").opened()
    descriptor = json.loads(rig.driver.describe())
    check(descriptor["robot_slug"] == "ros2-base-diff-drive-controller", "the slug names the controller's namespace, not the verb", descriptor["robot_slug"])
    check(descriptor["dof"] == 0 and descriptor["joints"] == [] and descriptor["end_effectors"] == [], "zero joints: a base has none the OS can see")
    check(descriptor["base"] == "wheeled" and descriptor["control"]["modes"] == [], "the base is wheeled, and no joint control mode is claimed")
    check(descriptor["simulator"] is True and descriptor["verified"] is False, "it says simulator unless the operator says otherwise, and that nothing is verified")
    check(descriptor["extensions"] == [TOKEN] and descriptor["envelope"] is None, "it lists the one hatch, and states no envelope: the speeds are the operator's, not a rating")
    motions = {motion["name"]: motion for motion in descriptor["motions"]}
    check(sorted(motions) == ["hold", "status", "velocity"], "the motions are status, velocity and hold")
    velocity = {parameter["name"]: parameter for parameter in motions["velocity"]["parameters"]}
    check(sorted(velocity) == ["duration", "vx", "wz"], "a non-holonomic base declares no vy")
    check(velocity["vx"]["min"] == -0.5 and velocity["vx"]["max"] == 0.5 and velocity["wz"]["max"] == 1.0, "and the bounds are the configured limits")
    check(velocity["duration"]["required"] is True and velocity["duration"]["min"] == 0.1 and velocity["duration"]["max"] == 5.0, "a duration is required and bounded")
    check(all(parameter["unit"] for parameter in velocity.values()), "and every parameter says its unit")
    rig.close()
    rig = Rig(holonomic=True, base="tracked", simulator=False, command_topic="/mecanum/cmd_vel").opened()
    descriptor = json.loads(rig.driver.describe())
    check(descriptor["base"] == "tracked" and descriptor["simulator"] is False, "the base kind and the simulator flag are the operator's")
    check(descriptor["robot_slug"] == "ros2-base-mecanum", "the slug follows the topic")
    check("vy" in {parameter["name"] for parameter in descriptor["motions"][1]["parameters"]}, "a holonomic base declares vy")
    rig.close()
    # Humble's controller takes a plain Twist on `<name>/cmd_vel_unstamped`, and it is the same base: a slug that kept the suffix would name it `…-cmd-vel-unstamped`
    # (found by the first live run against Humble's own controller).
    rig = Rig(sim={"command_type": "geometry_msgs/msg/Twist"}, command_topic="/diff_drive_controller/cmd_vel_unstamped").opened()
    check(json.loads(rig.driver.describe())["robot_slug"] == "ros2-base-diff-drive-controller", "the slug of Humble's unstamped command topic is the controller's namespace too", json.loads(rig.driver.describe())["robot_slug"])
    rig.close()
    rig = Rig(command_topic="/cmd_vel")
    check(json.loads(Ros2BaseDriver(lambda s: None).describe() or "{}") == {}, "before open the descriptor is empty rather than invented")
    rig.open()
    check(json.loads(rig.driver.describe())["robot_slug"] == "ros2-base-base", "a bare /cmd_vel is a base with no namespace and the slug says so")
    rig.close()


def state_checks() -> None:
    section("read_state: pose, twist and the clock, as the odometry says them")
    driver = Ros2BaseDriver(lambda s: SimulatedBase())
    check(driver.read_state(rhal.State(), 0) == rhal.SKR_EPERM, "before open the state read is refused")
    rig = Rig().opened()
    code, state = rig.read()
    check(code == rhal.SKR_OK and state.base_pose == [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0], "at the start the base is at the origin with the identity orientation (x y z qx qy qz qw)", str(state.base_pose))
    check(state.position_rad == [] and state.velocity_rad_s == [] and state.effort_nm == [], "and reports no joints")
    check(state.fault_code == 0 and state.flags == rhal.HOMING_NOT_REQUIRED, "no fault word (ROS defines none), no flags, and homing not required", f"flags {state.flags:#x}")
    seq = state.seq
    rig.read()
    code, state = rig.read()
    check(state.seq == seq + 2, "the sequence advances by one on every read")
    now = rhal.acquisition_ns()
    check(0 <= now - state.stamp_ns < 0.2e9, "the stamp is when the sample was received, on the OS's clock, not when it was read", str(now - state.stamp_ns))
    rig.motion("velocity wz=1.0 duration=1.0")
    code, state = rig.read()
    yaw = rig.base.truth()[2]
    check(abs(state.base_pose[5] - math.sin(yaw / 2)) < 0.02 and abs(state.base_pose[6] - math.cos(yaw / 2)) < 0.02 and state.base_pose[3] == state.base_pose[4] == 0.0,
          f"after a turn of {yaw:.2f} rad the quaternion is (0, 0, sin, cos) in that order", str(state.base_pose))
    rig.motion("velocity vx=0.3 duration=1.0")
    code, state = rig.read()
    x, y = rig.base.truth()[:2]
    check(abs(state.base_pose[0] - x) < 0.02 and abs(state.base_pose[1] - y) < 0.02, "the position is the odometry's, x then y then z")
    check(state.base_pose[2] == 0.0, "with z as published")
    mover = threading.Thread(target=lambda: rig.motion("velocity vx=0.3 wz=0.5 duration=1.0"))
    mover.start()
    time.sleep(0.6)
    code, state = rig.read()
    check(abs(state.base_twist[0] - 0.3) < 0.03 and abs(state.base_twist[5] - 0.5) < 0.05 and state.base_twist[1] == 0.0,
          "in flight the twist is vx, vy, vz, wx, wy, wz as the base reports it", str(state.base_twist))
    mover.join()
    rig.close()

    rig = Rig().opened()
    rig.base.odometry_silent = True
    time.sleep(0.5)
    code, state = rig.read()
    check(code == rhal.SKR_ETIMEDOUT and state.flags & rhal.FLAG_DEGRADED, "odometry that has stopped arriving for longer than stale_after_s is ETIMEDOUT and DEGRADED, not a base at rest")
    check(state.base_pose[6] == 1.0, "and the last pose is still given, because it is what was last known")
    check(rhal.acquisition_ns() - state.stamp_ns > 0.4e9, "and its stamp is when that sample arrived, not when it was read: a stale sample is not re-dated as new")
    rig.base.odometry_silent = False
    time.sleep(0.2)
    code, state = rig.read()
    check(code == rhal.SKR_OK and not state.flags & rhal.FLAG_DEGRADED, "and it recovers when the odometry does")
    rig.close()
    check(rig.driver.read_state(rhal.State(), 0) == rhal.SKR_EPERM, "after close the state read is refused")


def velocity_checks() -> None:
    section("velocity: refusals send nothing")
    rig = Rig().opened()
    for text, words in [
        ("velocity vx=0.2", ("duration",)),
        ("velocity vx=0.2 duration=0.05", ("duration", "0.1")),
        ("velocity vx=0.2 duration=6", ("duration", "5")),
        ("velocity vx=0.2 duration=nan", ("finite",)),
        ("velocity vx=inf duration=1", ("finite",)),
        ("velocity vx=fast duration=1", ("not a number",)),
        ("velocity vx=0.51 duration=1", ("max_linear", "0.5")),
        ("velocity vx=-0.51 duration=1", ("max_linear",)),
        ("velocity wz=1.01 duration=1", ("max_angular",)),
        ("velocity wz=-1.01 duration=1", ("max_angular",)),
        ("velocity vy=0.1 duration=1", ("holonomic",)),
        ("velocity vx=0.1 duration=1 speed=2", ("speed",)),
        ("velocity vx duration=1", ("malformed",)),
        ("fly", ("unknown motion", "status")),
        ("hold now=1", ("no arguments",)),
        ("status verbose=1", ("no arguments",)),
    ]:
        code, text_out = rig.say(text)
        check(refused_with(code, text_out, rhal.SKR_EINVAL, *words), f"`{text}` is EINVAL and the words say why", f"{code} {text_out}")
    check(rig.say("")[0] == rhal.SKR_EINVAL and "Motions" in rig.say("")[1], "an empty request is EINVAL and lists the motions")
    code, data = rig.driver.extension(TOKEN, b"\xff\xfe")
    check(code == rhal.SKR_EINVAL, "a request that is not UTF-8 is EINVAL")
    check(rig.driver.extension("vendor.other.thing", b"status")[0] == rhal.SKR_ENOSYS, "another vendor's token is ENOSYS")
    check(rig.commands() == [], "and not one of those sent a command to the base")
    rig.close()
    check(rig.commands() == [], "and closing a session whose every motion was refused publishes nothing either: a refusal leaves no mark")
    check(rig.driver.extension(TOKEN, b"status")[0] == rhal.SKR_EPERM, "a motion on a closed driver is EPERM")

    section("velocity: a stream for its duration, then a stop")
    rig = Rig().opened()
    began = time.monotonic()
    code, reply = rig.motion("velocity vx=0.3 duration=1.0")
    elapsed = time.monotonic() - began
    received = rig.commands()
    moving = nonzero(received)
    check(code == rhal.SKR_OK and reply["at_rest"] is True, "a velocity of 0.3 m/s for one second returns OK with the base at rest", f"{code} {reply}")
    check(1.0 <= elapsed < 2.0, f"it takes the duration and the stop, not less and not much more: {elapsed:.2f} s")
    check(15 <= len(moving) <= 21 and reply["commands"] == len(moving), f"it streams: {len(moving)} commands in 1 s at a 0.05 s period, and the reply counts them as the base got them")
    check({(c.vx, c.vy, c.wz) for c in moving} == {(0.3, 0.0, 0.0)}, "and every one carries exactly the velocity that was asked for")
    gaps = [b.at - a.at for a, b in zip(moving, moving[1:])]
    check(bool(gaps) and max(gaps) < 0.25, f"the longest silence in the stream is {max(gaps) * 1000:.0f} ms, well inside a base's 500 ms timeout")
    check((received[-1].vx, received[-1].vy, received[-1].wz) == (0.0, 0.0, 0.0), "the last command the base got is zero")
    check(received[-1].at > moving[-1].at and reply["zero_commands"] >= 1, "and it came after the last motion command")
    x, y, yaw, vx, vy, wz = rig.base.truth()
    check(0.22 < x < 0.36 and abs(y) < 0.01 and abs(yaw) < 0.01, f"the base went {x:.3f} m along x and nowhere else, which is what 0.3 m/s for a second with its ramps comes to")
    check(abs(reply["displacement"][0] - x) < 0.02 and abs(reply["displacement"][1] - y) < 0.02, "and the reply's displacement is the odometry's, which is the truth")
    check(abs(vx) < 0.01 and abs(reply["final_twist"][0]) < 0.01, "at rest, in the reply and in fact")
    check(all(c.type == TWIST_STAMPED and c.frame_id == "base_link" for c in received), "stamped commands carry the configured frame")
    code, reply = rig.motion("velocity vx=-0.3 duration=0.6")
    check(code == rhal.SKR_OK and reply["displacement"][0] < -0.05, "a negative vx moves the base backwards: the sign is not flipped")
    yaw_before = rig.base.truth()[2]
    code, reply = rig.motion("velocity wz=0.8 duration=1.0")
    check(code == rhal.SKR_OK and 0.45 < reply["yaw_change"] < 0.9, f"a positive wz turns it counter-clockwise ({reply['yaw_change']:.2f} rad, ramps included)")
    check(abs(reply["yaw_change"] - (rig.base.truth()[2] - yaw_before)) < 0.02, "and the reply's yaw change is the one the base made, from the odometry's quaternion: not a scaled one")
    check(abs(reply["final_twist"][2]) <= 0.011, "and a turn is at rest only when it has stopped turning, not only when it has stopped going forward")
    code, reply = rig.motion("velocity wz=-0.8 duration=1.0")
    check(code == rhal.SKR_OK and -0.9 < reply["yaw_change"] < -0.45, "a negative wz turns it clockwise")
    rig.motion("velocity wz=0.8 duration=1.0")
    heading = rig.base.truth()[2]
    code, reply = rig.motion("velocity vx=0.3 duration=1.0")
    forward = (math.cos(heading), math.sin(heading))
    along = reply["displacement"][0] * forward[0] + reply["displacement"][1] * forward[1]
    check(code == rhal.SKR_OK and along > 0.2 and reply["displacement"][1] > 0.15, f"forward is along the heading the base has ({heading:.2f} rad), not along the odometry's x")
    before = len(rig.commands())
    code, reply = rig.motion("velocity duration=0.3")
    check(code == rhal.SKR_OK and len(rig.commands()) > before and nonzero(rig.commands()[before:]) == [], "a velocity of zero is allowed and sends only zeros")
    rig.close()

    rig = Rig({"command_timeout_s": None}).opened()
    code, reply = rig.motion("velocity vx=0.3 duration=0.6")
    check(code == rhal.SKR_OK and reply["at_rest"] and abs(rig.base.truth()[3]) < 0.01,
          "a base that holds its last command is at rest after a motion, because the motion ends with a zero")
    rig.close()
    rig = Rig({"holonomic": True}, holonomic=True).opened()
    code, reply = rig.motion("velocity vy=0.3 duration=1.0")
    check(code == rhal.SKR_OK and reply["displacement"][1] > 0.2 and abs(reply["displacement"][0]) < 0.02, "a holonomic base moves sideways on vy")
    check(abs(reply["final_twist"][1]) <= 0.011, "and is at rest only when its sideways speed is zero too")
    rig.close()

    rig = Rig()
    rig.base._yaw = 3.0
    rig.opened()
    code, reply = rig.motion("velocity wz=0.5 duration=1.0")
    check(code == rhal.SKR_OK and 0.3 < reply["yaw_change"] < 0.6, f"a turn from a heading of 3 rad through the half turn is a turn of {reply.get('yaw_change')} rad, not a whole circle the other way", str(reply))
    rig.close()

    section("velocity: when the graph fails under it")
    rig = Rig().opened()
    rig.base.odometry_silent = True
    time.sleep(0.5)
    code, text = rig.say("velocity vx=0.3 duration=1.0")
    check(refused_with(code, text, rhal.SKR_EIO, "not current", "/odom"), "a velocity asked of a base whose odometry is stale is refused, naming the topic", text)
    check(rig.commands() == [], "and no command was sent: a base that cannot be seen is not moved")
    rig.base.odometry_silent = False
    rig.close()

    rig = Rig().opened()
    threading.Timer(0.4, lambda: setattr(rig.base, "odometry_silent", True)).start()
    began = time.monotonic()
    code, text = rig.say("velocity vx=0.3 duration=4.0")
    elapsed = time.monotonic() - began
    check(refused_with(code, text, rhal.SKR_EIO, "stopped arriving"), "odometry lost mid-stream ends the stream with EIO, saying the topic stopped arriving", text)
    check(elapsed < 3.0, f"and does so in {elapsed:.1f} s, not at the end of the four seconds")
    received = rig.commands()
    check(received[-1].vx == 0.0 and (rig.base.truth()[3] < 0.01), "the base was told to stop, and it stopped")
    rig.base.odometry_silent = False
    time.sleep(0.3)
    code, state = rig.read()
    check(code == rhal.SKR_OK and not state.flags & rhal.FLAG_FAULT, "and once the odometry returns it shows at rest, so there is no fault to report")
    rig.close()

    rig = Rig().opened()
    threading.Timer(0.3, lambda: setattr(rig.base, "odometry_silent", True)).start()
    code, text = rig.say("velocity duration=1.5")
    check(refused_with(code, text, rhal.SKR_EIO, "stopped arriving", "did not come to rest"),
          "a base the driver can no longer see is not claimed to be at rest, even when its last sample said so", text)
    rig.base.odometry_silent = False
    rig.close()

    rig = Rig({"fail_publish": True}).opened()
    code, text = rig.say("velocity vx=0.3 duration=0.5")
    check(code == rhal.SKR_EIO and "publishing" in text, "a publish that fails is EIO and the words say it was the publish", text)
    rig.close()

    section("velocity: a base that does not come to rest")
    rig = Rig({}, stop_settle_s=0.6).opened()
    threading.Timer(0.4, lambda: setattr(rig.base, "stuck", True)).start()
    code, text = rig.say("velocity vx=0.3 duration=1.0")
    check(refused_with(code, text, rhal.SKR_EIO, "did not come to rest", "0.6"), "a base that ignores the stop is EIO, and the words say how long the driver waited", text)
    zeros = sum(1 for c in rig.commands() if (c.vx, c.wz) == (0.0, 0.0))
    check(rig.commands()[-1].vx == 0.0 and 5 <= zeros <= 25, f"and the zero command was repeated at the stream's pace ({zeros} in 0.6 s), not said once and not as fast as the loop can go")
    code, state = rig.read()
    check(state.flags & rhal.FLAG_FAULT and state.flags & rhal.FLAG_DEGRADED, "the state reads FAULT and DEGRADED while the odometry shows it still going")
    check(not state.flags & rhal.FLAG_ESTOP, "which is not the emergency stop, and the flags say so")
    rig.base.stuck = False
    time.sleep(0.6)
    code, state = rig.read()
    check(code == rhal.SKR_OK and not state.flags & rhal.FLAG_FAULT, "and it clears when the base is seen at rest")
    rig.base.publish(0.3, 0.0, 0.0)
    time.sleep(0.3)
    code, state = rig.read()
    check(rig.base.truth()[3] > 0.2, "(the base is moving again, on a command that is not the driver's)")
    check(code == rhal.SKR_OK and not state.flags & rhal.FLAG_FAULT, "and a base moved later by someone else's command is not a fault: only a base that was told to stop and did not is")
    rig.close()

    section("velocity: one motion at a time")
    rig = Rig().opened()
    first: list = []
    runner = threading.Thread(target=lambda: first.append(rig.say("velocity vx=0.2 duration=1.0")))
    runner.start()
    time.sleep(0.3)
    code, text = rig.say("velocity vx=0.2 duration=0.5")
    check(refused_with(code, text, rhal.SKR_EPERM, "another motion"), "a second motion while one runs is EPERM, not interleaved commands", text)
    code, text = rig.say("hold")
    check(code == rhal.SKR_EPERM and "another motion" in text, "and hold is refused the same way")
    runner.join()
    check(first[0][0] == rhal.SKR_OK, "and the first completes")
    rig.close()


def hold_checks() -> None:
    section("hold: publish zero until the base is seen at rest")
    rig = Rig().opened()
    code, reply = rig.motion("hold")
    check(code == rhal.SKR_OK and reply["at_rest"] and reply["zero_commands"] >= 1 and reply["commands"] == 0, "hold at rest returns at rest with only zeros sent")
    rig.base.publish(0.3, 0.0, 0.0)
    time.sleep(0.3)
    check(rig.base.truth()[3] > 0.2, "(a base moving on another publisher's command)")
    code, reply = rig.motion("hold")
    check(code == rhal.SKR_OK and reply["at_rest"] and abs(rig.base.truth()[3]) < 0.01, "hold brings it to rest, and the reply is from the odometry")
    rig.close()
    rig = Rig({"stuck": False}, stop_settle_s=0.4).opened()
    rig.base.publish(0.3, 0.0, 0.0)
    time.sleep(0.3)
    rig.base.stuck = True
    code, text = rig.say("hold")
    check(refused_with(code, text, rhal.SKR_EIO, "did not come to rest"), "hold on a base that will not stop is EIO, not success", text)
    rig.close()


def stop_checks() -> None:
    section("emergency stop: latch, zero, keep saying it, never a command after it")
    rig = Rig().opened()
    began = time.monotonic()
    check(rig.driver.emergency_stop() == rhal.SKR_OK, "the stop returns OK")
    check(time.monotonic() - began < 0.05, f"and in {(time.monotonic() - began) * 1000:.1f} ms")
    check(rig.commands() and (rig.commands()[-1].vx, rig.commands()[-1].vy, rig.commands()[-1].wz) == (0.0, 0.0, 0.0), "and the zero command had already gone when it returned: the keeper is for the second one")
    check(rig.commands()[0].thread == threading.current_thread().name, f"said by the stop itself, in the caller's thread, and not left to the thread it starts ({rig.commands()[0].thread})")
    code, state = rig.read()
    check(state.flags & rhal.FLAG_ESTOP and code == rhal.SKR_OK, "the state carries the stop")
    count = len(rig.commands())
    time.sleep(0.4)
    after = rig.commands()
    check(len(after) - count >= 4 and all((c.vx, c.vy, c.wz) == (0.0, 0.0, 0.0) for c in after), f"it keeps publishing zero while latched ({len(after) - count} in 0.4 s), and nothing else")
    for text in ("velocity vx=0.2 duration=0.5", "hold"):
        code, reply = rig.say(text)
        check(refused_with(code, reply, rhal.SKR_EPERM, "emergency stop", "latched"), f"`{text}` is refused while stopped, saying why", reply)
    code, reply = rig.say("status")
    check(code == rhal.SKR_OK and json.loads(reply)["stop_latched"] is True, "status is still answered, and says latched")
    check(rig.driver.write_command(rhal.Command(values=[], mask=[]), 0) == rhal.SKR_EPERM, "write_command stays refused")
    check(rig.driver.emergency_stop() == rhal.SKR_OK, "a second stop is idempotent")
    keepers = [thread for thread in threading.enumerate() if thread.name == "skynet-ros2-base-stop"]
    check(len(keepers) == 1, "and there is still exactly one thread keeping the base stopped", str(keepers))
    check(rig.driver.release_stop() == rhal.SKR_OK, "release_stop clears the latch")
    time.sleep(0.2)
    settled = len(rig.commands())
    time.sleep(0.3)
    check(len(rig.commands()) == settled, "and the zeros stop: the driver does not keep a released base held")
    code, state = rig.read()
    check(not state.flags & rhal.FLAG_ESTOP, "the state no longer says stopped")
    code, reply = rig.motion("velocity vx=0.2 duration=0.5")
    check(code == rhal.SKR_OK, "and motions are accepted again")
    rig.driver.emergency_stop()
    rig.driver.release_stop()
    rig.driver.emergency_stop()
    count = len(rig.commands())
    time.sleep(0.3)
    check(len(rig.commands()) - count >= 3, "a stop asked for right after a release is kept just the same")
    rig.close()

    rig = Rig().opened()
    outcome: list = []
    runner = threading.Thread(target=lambda: outcome.append((rig.say("velocity vx=0.4 duration=3.0"), time.monotonic())))
    runner.start()
    time.sleep(0.5)
    check(rig.base.truth()[3] > 0.2, "(the base is moving)")
    began = time.monotonic()
    rig.driver.emergency_stop()
    returned = time.monotonic()
    runner.join()
    (code, text), ended = outcome[0]
    check(returned - began < 0.05, f"a stop in the middle of a stream returns in {(returned - began) * 1000:.1f} ms, never waiting behind it")
    check(refused_with(code, text, rhal.SKR_EPERM, "emergency stop"), "the stream ends with EPERM, saying it was the stop", text)
    check(ended - returned < 1.0, f"and ends {(ended - returned) * 1000:.0f} ms after, not at the end of its three seconds")
    late = [c for c in nonzero(rig.commands()) if c.at > returned]
    check(late == [], "and no command asking for motion was sent after the stop returned", str(late[:2]))
    check(abs(rig.base.truth()[3]) < 0.01, "the base is at rest")
    rig.close()

    rig = Rig().opened()
    runner = threading.Thread(target=lambda: rig.say("velocity vx=0.4 duration=2.0"))
    runner.start()
    time.sleep(0.5)
    rig.driver.emergency_stop()
    code, state = rig.read()
    check(state.flags & rhal.FLAG_ESTOP and not state.flags & rhal.FLAG_FAULT and rig.base.truth()[3] > 0.0,
          "a stop is not a fault while the base is still slowing: FAULT waits for the settle time to have gone by")
    runner.join()
    rig.close()

    rig = Rig({"publish_delay_s": 0.2}).opened()
    outcome = []
    runner = threading.Thread(target=lambda: outcome.append(rig.say("velocity vx=0.4 duration=3.0")))
    runner.start()
    time.sleep(0.5)
    rig.driver.emergency_stop()
    returned = time.monotonic()
    runner.join()
    time.sleep(0.5)
    late = [c for c in nonzero(rig.commands()) if c.at > returned]
    check(late == [] and (rig.commands()[-1].vx, rig.commands()[-1].wz) == (0.0, 0.0),
          "even on a graph slow enough that a publish is in flight when the stop is asked, no command asking for motion lands after the stop returned", str(late[:2]))
    rig.close()

    # A publish that takes longer than the stop's own: the stop has to wait for the one in flight, because a stop that does not leaves the base following a command that lands after it.
    rig = Rig({"publish_delay_s": 0.4}).opened()
    outcome = []
    runner = threading.Thread(target=lambda: outcome.append(rig.say("velocity vx=0.4 duration=3.0")))
    runner.start()
    time.sleep(0.5)
    rig.base.publish_delay_s = 0.0
    rig.driver.emergency_stop()
    returned = time.monotonic()
    runner.join()
    time.sleep(0.5)
    late = [c for c in nonzero(rig.commands()) if c.at > returned]
    check(late == [] and (rig.commands()[-1].vx, rig.commands()[-1].wz) == (0.0, 0.0),
          "a stop asked for while a slow publish is in flight waits for it, so the command lands before the stop's zero and not after (the lock the stream and the stop share)", str(late[:2]))
    rig.close()

    # The stop and a stream's odometry going stale in the same period: the stop owns the vehicle, so that is what the stream says ended it.
    class LatchesThenGoesBlind(SimulatedBase):
        """After its first command, the stop is latched on the driver and the odometry it reads is old: both are true at the top of the stream's next period."""

        driver = None

        def publish(self, vx, vy, wz):
            super().publish(vx, vy, wz)
            if self.driver is not None and (vx, vy, wz) != (0.0, 0.0, 0.0):
                self.driver._estop = True
                self.blind = True

        def odometry(self):
            latest = super().odometry()
            if getattr(self, "blind", False) and latest is not None:
                return dataclasses.replace(latest, received_ns=latest.received_ns - int(60e9))
            return latest

    base = LatchesThenGoesBlind(rate_hz=100.0)
    driver = Ros2BaseDriver(lambda settings: base)
    base.driver = driver
    with contextlib.redirect_stderr(io.StringIO()):
        driver.open(json.dumps(configuration()))
    code, data = driver.extension(TOKEN, b"velocity vx=0.2 duration=1.0")
    check(refused_with(code, data.decode("utf-8", "replace"), rhal.SKR_EPERM, "emergency stop"), "a stream whose odometry goes stale in the period the stop is latched ends as the stop's, not as a blind one", data.decode("utf-8", "replace"))
    driver.close()

    rig = Rig({"active": False}, open_wait_s=2.0)
    opener = threading.Thread(target=rig.open)
    opener.start()
    time.sleep(0.3)
    rig.driver.emergency_stop()
    rig.base.active = True
    opener.join()
    code, state = rig.read()
    check(code == rhal.SKR_OK and state.flags & rhal.FLAG_ESTOP and rig.motion("velocity vx=0.2 duration=0.2")[0] == rhal.SKR_EPERM,
          "a stop asked for while the driver is still joining the graph is kept: it opens stopped, and refuses motion until released")
    rig.close()

    rig = Rig().opened()
    rig.driver.emergency_stop()
    rig.close()
    check(rig.open() == rhal.SKR_OK and rig.motion("velocity vx=0.2 duration=0.2")[0] == rhal.SKR_OK, "a stop does not outlive its session: a driver closed while stopped opens and moves")
    rig.close()

    rig = Rig().opened()
    outcome = []
    runner = threading.Thread(target=lambda: outcome.append(rig.say("velocity vx=0.2 duration=2.0")))
    runner.start()
    time.sleep(0.4)
    rig.driver.close()
    runner.join()
    check(outcome and outcome[0][0] in (rhal.SKR_EIO, rhal.SKR_EPERM) and "Traceback" not in outcome[0][1], "a close that lands under a stream ends the stream with an answer, not an exception", str(outcome))
    rig.base.stop()

    rig = Rig({"fail_publish": True}).opened()
    check(rig.driver.emergency_stop() == rhal.SKR_OK, "a stop on a graph that cannot be published to still returns OK: the latch is held, and nothing raises")
    code, state = rig.read()
    check(state.flags & rhal.FLAG_ESTOP, "and the state says stopped")
    rig.close()
    never = Ros2BaseDriver(lambda s: None)
    check(never.emergency_stop() == rhal.SKR_OK and never.release_stop() == rhal.SKR_OK, "a stop and a release on a driver that was never opened are harmless")

    rig = Rig({}, stop_settle_s=0.4).opened()
    rig.base.publish(0.3, 0.0, 0.0)
    time.sleep(0.3)
    rig.base.stuck = True
    rig.driver.emergency_stop()
    time.sleep(0.6)
    code, state = rig.read()
    check(state.flags & rhal.FLAG_ESTOP and state.flags & rhal.FLAG_FAULT, "a stop the base ignores reads ESTOP and FAULT: the latch holds, and the odometry says it did not work")
    rig.base.stuck = False
    time.sleep(0.5)
    code, state = rig.read()
    check(state.flags & rhal.FLAG_ESTOP and not state.flags & rhal.FLAG_FAULT, "and the fault clears when the base is at rest, with the stop still latched")
    rig.close()


def close_checks() -> None:
    section("close: the base is left told to stop, and a driver that moved nothing says nothing")
    rig = Rig().opened()
    rig.motion("velocity vx=0.2 duration=0.3")
    rig.base.publish(0.2, 0.0, 0.0)
    rig.driver.close()
    check(rig.commands()[-1].vx == 0.0, "close after a motion sends zero last, even to a base that was left going")
    check(rig.base.closed == 1, "and closes the connection")
    rig = Rig().opened()
    rig.driver.close()
    check(rig.commands() == [], "close on a session that commanded nothing publishes nothing")
    check(rig.driver.close() == rhal.SKR_OK, "and close twice is harmless")
    check(rig.driver.open(json.dumps(rig.config)) == rhal.SKR_OK, "a closed driver opens again")
    check(rig.motion("velocity vx=0.2 duration=0.2")[0] == rhal.SKR_OK, "and moves the base")
    rig.close()

    section("write_command: refused, in the shape order")
    rig = Rig().opened()
    check(rig.driver.write_command(rhal.Command(values=[0.1], mask=[1], mode=rhal.MODE_POSITION), 0) == rhal.SKR_EINVAL, "a command with a joint is EINVAL: this body has none")
    check(rig.driver.write_command(rhal.Command(values=[], mask=[], mode=99), 0) == rhal.SKR_EINVAL, "an unknown mode is EINVAL, even from a driver that refuses everything")
    buffer = io.StringIO()
    with contextlib.redirect_stderr(buffer):
        code = rig.driver.write_command(rhal.Command(values=[], mask=[], mode=rhal.MODE_VELOCITY), 0)
    check(code == rhal.SKR_EPERM and TOKEN in buffer.getvalue(), "a well-formed one is EPERM, and the words name the hatch that moves the base")
    check(rig.driver.read_sensor("camera", rhal.Frame()) == rhal.SKR_ENOSYS, "there are no sensors")
    check(Ros2BaseDriver(lambda s: None).write_command(rhal.Command(), 0) == rhal.SKR_EPERM, "before open a command is EPERM")
    rig.close()


def pipe_checks() -> None:
    """The driver's half of `T`, through the real pipe protocol as a subprocess, on the in-process simulated base."""
    section("through the pipe: serve() carries T to this driver")
    env = dict(os.environ)
    env["PYTHONPATH"] = os.path.join(ROOT, "..", "..", "bindings", "python") + os.pathsep + ROOT
    env["SKYNET_ROS2_BASE_SIM"] = "1"
    child = subprocess.Popen(
        [sys.executable, "-m", "skynet_ros2_base"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1, cwd=ROOT, env=env
    )

    def send(line: str) -> None:
        assert child.stdin is not None
        child.stdin.write(line + "\n")
        child.stdin.flush()

    def receive() -> str:
        assert child.stdout is not None
        line = child.stdout.readline()
        if not line:
            raise RuntimeError("the child went quiet")
        return line.rstrip("\n")

    try:
        hello = receive()
        check(hello.startswith(f"H {rhal.ABI_VERSION} {rhal._PROTOCOL_VERSION} guest ros2-base "), f"the child announces itself: {hello}")
        text = json.dumps(configuration())
        child.stdin.write(f"O {len(text)}\n{text}\n")
        child.stdin.flush()
        check(receive() == "R 0", "open() through the pipe joins the simulated base")
        send("D")
        headline = receive()
        body = child.stdout.read(int(headline.split()[1]))
        child.stdout.readline()
        check(headline.startswith("D ") and TOKEN in body and '"simulator": true' in body, "describe() answers, lists the hatch, and says simulator")
        send("S 0")
        check(receive().startswith("S 0 "), "read_state answers through the pipe")
        send("C 0 1 0 0")
        check(receive() == f"R {rhal.SKR_EPERM}", "write_command refuses through the pipe")
        payload = base64.b64encode(b"velocity vx=0.2 duration=0.3").decode("ascii")
        send(f"T {TOKEN} {payload}")
        parts = receive().split()
        check(len(parts) == 3 and parts[0] == "R" and parts[1] == "0", "a velocity answers in-frame with rc 0")
        answer = json.loads(base64.b64decode(receive().encode("ascii")))
        check(answer["at_rest"] is True and answer["commands"] >= 4, "and the reply says the base is at rest after a stream")
        payload = base64.b64encode(b"velocity vx=9 duration=0.3").decode("ascii")
        send(f"T {TOKEN} {payload}")
        parts = receive().split()
        check(parts[1] == str(rhal.SKR_EINVAL), "a speed past the limit is refused through the pipe")
        receive()
        send(f"T {TOKEN}")
        check(receive().split()[1] == str(rhal.SKR_EINVAL), "an empty T payload is malformed through the pipe")
        receive()
        payload = base64.b64encode(b"velocity vx=0.2 duration=4.0").decode("ascii")
        began = time.monotonic()
        send(f"T {TOKEN} {payload}")
        time.sleep(0.5)
        send("E")
        parts = receive().split()
        elapsed = time.monotonic() - began
        check(parts[1] == str(rhal.SKR_EPERM) and elapsed < 2.0, f"a stop written to the pipe while a four-second velocity runs ends it in {elapsed:.1f} s with EPERM, not at its end (F234)", " ".join(parts))
        receive()
        check(receive() == "R 0", "and the stop's own reply follows the motion's, in the order the host asked and drains")
        send("R")
        check(receive() == "R 0", "release_stop is answered through the pipe")
        send("E")
        check(receive() == "R 0", "the emergency stop goes through the pipe")
        send("X")
        check(receive() == "R 0", "close() stops the child")
    finally:
        child.stdin.close()
        child.wait(timeout=10)


def main() -> int:
    configuration_checks()
    graph_checks()
    simulated_base_checks()
    descriptor_checks()
    state_checks()
    velocity_checks()
    hold_checks()
    stop_checks()
    close_checks()
    pipe_checks()
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())
