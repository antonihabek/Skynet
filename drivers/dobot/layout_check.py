"""This driver's text and packet layout, held to Dobot's own Python client, which is run (DRY-COVERAGE-PLAN.md item 12.E.13).

    DOBOT_PYTHON=/path/to/venv/bin/python (pip install --require-hashes -r requirements-layout.txt)
    DOBOT_SDK=/path/to/TCP-IP-Python-V4          (make fetch-sdk: the directory that holds the `dobot_sdk` package)
    PYTHONPATH=../../bindings/python:. python layout_check.py

A Dobot V4 controller is a box with a text port and a packet port, and its maker publishes a client for both (`Dobot-Arm/TCP-IP-Python-V4`, MIT). It writes a command the way the controller expects it and reads a feedback
packet through a numpy dtype of the document's table, so it is what this driver's bytes are compared with. What is compared, with the client on one side and `frames.py` on the other:

* **the packet's layout**: every field this driver reads, at the offset the client's dtype gives it, and the packet's whole size;
* **the packets**: random ones made with the client's dtype through this driver's `decode_feedback`, and random ones made by this driver's `encode_feedback` through the client's own parser, field by field;
* **every command this driver sends**: RequestControl, PowerOn, EnableRobot, DisableRobot, ClearError, Stop, EmergencyStop, RobotMode, GetAngle, GetErrorID, SpeedFactor, MovJ and ServoJ, byte for byte as the client writes
  them (random angles and parameters);
* **the replies**: the client's own parser of an alarm list against this driver's, on random lists (**and the client's returns an empty list for every one of them**: see below);
* **the two together**: the client connects to the simulated controller, takes control, clears, enables, moves it with MovJ and streams ServoJ at it, and reads its feedback; and this driver, opened on the same
  controller, reads where the client put it and moves it for the client to read.

**Five places the maker's client and the maker's document disagree, or the client falls short, are pinned here and not papered over**: the client's `parse_error_ids` can never read an alarm list (a comment swallowed the line
that assigns `start`, so every call is a NameError that the function catches and answers with an empty list: an arm in alarm is an arm with no alarms to the client's own error monitor); the client names RobotMode 11 `JOG` and the document `COLLISION` (and the client adds a 0
that the document does not have); the client cannot send a load of zero (`EnableRobot(0.0)` becomes `EnableRobot()`, which leaves the controller's own payload); it reads the robot type as a signed byte, so the models
numbered above 127 (the CRxV series) are negative numbers to it; and it always sends ServoJ's three optional parameters, with the document's defaults, where this driver sends only the ones the operator set.

It cannot show that a real controller answers any of this: the controller is the simulated one (F15), written from the interface document and from what the client sends, and it is accepted by the client because it was made to be.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import random
import sys
import time
import types

import skynet_rhal as rhal

from skynet_dobot import bridge as bridge_module, frames
from skynet_dobot.bridge import DobotBridge
from skynet_dobot.controller_sim import SimulatedController

CHECKS = 0
FAILURES: list[str] = []
SDK = os.environ.get("DOBOT_SDK", "")
if SDK:
    sys.path.insert(0, SDK)

try:
    import numpy as np
except ImportError:
    print("layout_check: numpy is not installed (pip install --require-hashes -r requirements-layout.txt in a virtual environment, and run this with that interpreter): nothing was compared.")
    sys.exit(2)
# The client imports `requests` at module level, for the HTTP alarm list this check never asks for: a stub stands in, so that the client's own code is what runs and nothing of the network's is installed.
if "requests" not in sys.modules:
    stub = types.ModuleType("requests")
    stub.exceptions = types.SimpleNamespace(RequestException=Exception)       # type: ignore[attr-defined]
    sys.modules["requests"] = stub
try:
    with contextlib.redirect_stdout(io.StringIO()):
        from dobot_sdk import CoordinateType, DobotRobot
        from dobot_sdk.api.error_controller import parse_error_ids
        from dobot_sdk.api.motion import Motion
        from dobot_sdk.api.robot_control import RobotControl
        from dobot_sdk.models.status import RobotMode as SdkMode
        from dobot_sdk.protocol.data_types import FEEDBACK_PACKET_SIZE, MyType, TEST_VALUE_MAGIC
        from dobot_sdk.protocol.feedback import FeedbackParser
except ImportError as error:
    print(f"layout_check: Dobot's client is not importable ({error}): DOBOT_SDK names the directory that holds the dobot_sdk package (make fetch-sdk). Nothing was compared.")
    sys.exit(2)


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")
        if detail:
            print(f"        {detail}")


def section(title: str) -> None:
    print(f"\n{title}")


def until(predicate, timeout: float = 5.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return True
        except Exception:  # noqa: BLE001
            pass
        time.sleep(0.01)
    return bool(predicate())


class Recorder:
    """What the client's modules are given in place of a connection: it keeps the text they send and answers it as a controller would."""

    def __init__(self) -> None:
        self.sent: list[str] = []

    def send_receive_text(self, text: str, recv_size: int = 1024) -> str:
        self.sent.append(text)
        return f"0,{{}},{text};"


def main() -> int:
    rng = random.Random(0xD0B07)

    # ── the packet's layout ───────────────────────────────────────────────────────────────────────
    section("the feedback packet — every field this driver reads is where the client's dtype puts it")
    check(FEEDBACK_PACKET_SIZE == frames.FEEDBACK_BYTES == MyType.itemsize == 1440, "the packet is 1,440 bytes: the client's constant, its dtype and this driver's")
    check(TEST_VALUE_MAGIC == frames.FEEDBACK_MAGIC, "and its self-test field is 0x0123456789ABCDEF")
    names = {"size": "len", "digital_inputs": "DigitalInputs", "digital_outputs": "DigitalOutputs", "robot_mode": "RobotMode", "timestamp": "TimeStamp", "run_time": "RunTime", "test_value": "TestValue", "speed_scaling": "SpeedScaling",
             "voltage": "VRobot", "current": "IRobot", "program_state": "ProgramState", "q_target": "QTarget", "qd_target": "QDTarget", "qdd_target": "QDDTarget", "i_target": "ITarget", "m_target": "MTarget", "q_actual": "QActual",
             "qd_actual": "QDActual", "i_actual": "IActual", "tcp_force_actual": "ActualTCPForce", "tool_vector_actual": "ToolVectorActual", "tcp_speed_actual": "TCPSpeedActual", "tcp_force": "TCPForce",
             "tool_vector_target": "ToolVectorTarget", "tcp_speed_target": "TCPSpeedTarget", "motor_temperatures": "MotorTemperatures", "joint_modes": "JointModes", "v_actual": "VActual", "user": "User", "tool": "Tool",
             "run_queued_cmd": "RunQueuedCmd", "pause_cmd_flag": "PauseCmdFlag", "velocity_ratio": "VelocityRatio", "acceleration_ratio": "AccelerationRatio", "brake_status": "BrakeStatus", "enable_status": "EnableStatus",
             "drag_status": "DragStatus", "running_status": "RunningStatus", "error_status": "ErrorStatus", "jog_status": "JogStatusCR", "robot_type": "CRRobotType", "collision_state": "CollisionState",
             "current_command_id": "CurrentCommandId", "m_actual": "MActual", "load": "Load", "center_x": "CenterX", "center_y": "CenterY", "center_z": "CenterZ", "safety_state": "SafetyState"}
    for ours, theirs in names.items():
        check(theirs in MyType.names and MyType.fields[theirs][1] == frames._OFFSETS[ours], f"{ours} is at byte {frames._OFFSETS[ours]}, where the client's dtype puts {theirs}" + ("" if theirs in MyType.names else " (the client has no such field)"))  # noqa: SLF001
    check(set(frames._OFFSETS) == set(names), "(and every offset this driver has is one of those)")  # noqa: SLF001

    section("the feedback packet — random ones through each side's reader")
    parser = FeedbackParser()
    for trial in range(40):
        q_actual = tuple(round(rng.uniform(-360, 360), 6) for _ in range(6))
        q_target = tuple(round(rng.uniform(-360, 360), 6) for _ in range(6))
        qd = tuple(round(rng.uniform(-100, 100), 6) for _ in range(6))
        temperatures = tuple(round(rng.uniform(20, 80), 3) for _ in range(6))
        mode = rng.choice([1, 2, 3, 4, 5, 6, 7, 8, 9, 10])
        fields = dict(robot_mode=mode, enable_status=rng.randrange(2), brake_status=rng.randrange(64), error_status=rng.randrange(2), running_status=rng.randrange(2), collision_state=rng.randrange(2), robot_type=rng.choice([t for t in frames.ROBOT_TYPES if t < 128]),
                      safety_state=rng.randrange(256), speed_scaling=float(rng.randrange(1, 101)), q_actual=q_actual, q_target=q_target, qd_actual=qd, motor_temperatures=temperatures, timestamp_ms=rng.randrange(1 << 40),
                      run_time_ms=rng.randrange(1 << 32), current_command_id=rng.randrange(1 << 20), digital_inputs=rng.randrange(1 << 63), digital_outputs=rng.randrange(1 << 63), load_kg=round(rng.uniform(0, 20), 3))
        packet = frames.encode_feedback(**fields)
        sdk_status = parser.parse(packet)
        ok = (sdk_status is not None and int(sdk_status.robot_mode) == mode and sdk_status.enable_status == bool(fields["enable_status"]) and sdk_status.error_status == bool(fields["error_status"]) and sdk_status.running_status == fields["running_status"]
              and sdk_status.robot_type == fields["robot_type"] and sdk_status.joint_state.q_actual == list(q_actual) and sdk_status.joint_state.q_target == list(q_target) and sdk_status.joint_state.qd_actual == list(qd)
              and sdk_status.joint_state.temperatures == list(temperatures) and sdk_status.timestamp == fields["timestamp_ms"] and sdk_status.run_time == fields["run_time_ms"] and sdk_status.digital_inputs == fields["digital_inputs"]
              and sdk_status.digital_outputs == fields["digital_outputs"] and sdk_status.current_command_id == fields["current_command_id"] and sdk_status.speed_scaling == fields["speed_scaling"] and abs(sdk_status.load - fields["load_kg"]) < 1e-12
              and sdk_status.collision_state == fields["collision_state"] and sdk_status.safety_state == (fields["safety_state"] - 256 if fields["safety_state"] > 127 else fields["safety_state"]))
        check(ok, f"a packet this driver made is read by the client's own parser, field by field (trial {trial})", "" if ok else str(sdk_status))
        array = np.zeros(1, dtype=MyType)
        array["TestValue"][0] = TEST_VALUE_MAGIC
        array["len"][0] = 1440
        array["RobotMode"][0] = mode
        array["QActual"][0] = q_actual
        array["QTarget"][0] = q_target
        array["QDActual"][0] = qd
        array["EnableStatus"][0] = fields["enable_status"]
        array["ErrorStatus"][0] = fields["error_status"]
        array["RunningStatus"][0] = fields["running_status"]
        array["CRRobotType"][0] = fields["robot_type"]
        array["CollisionState"][0] = fields["collision_state"]
        array["TimeStamp"][0] = fields["timestamp_ms"]
        array["CurrentCommandId"][0] = fields["current_command_id"]
        array["SpeedScaling"][0] = fields["speed_scaling"]
        array["Load"][0] = fields["load_kg"]
        ours = frames.decode_feedback(array.tobytes())
        ok = (ours.robot_mode == mode and ours.enable_status == fields["enable_status"] and ours.error_status == fields["error_status"] and ours.running_status == fields["running_status"] and ours.robot_type == fields["robot_type"]
              and ours.q_actual == q_actual and ours.q_target == q_target and ours.qd_actual == qd and ours.collision_state == fields["collision_state"] and ours.timestamp_ms == fields["timestamp_ms"]
              and ours.current_command_id == fields["current_command_id"] and ours.speed_scaling == fields["speed_scaling"] and ours.load_kg == fields["load_kg"])
        check(ok, f"and one the client's dtype made is read by this driver's (trial {trial})", "" if ok else str(ours))
    packet = frames.encode_feedback(robot_type=220)
    sdk_status = parser.parse(packet)
    check(sdk_status is not None and sdk_status.robot_type == -36 and frames.decode_feedback(packet).robot_type == 220 and frames.ROBOT_TYPES[220] == "CR20V",
          "**the client reads the robot type as a signed byte**: a CR20V, which the document numbers 220, is -36 to it (and so is every model numbered above 127: the CR3V to the CR20V); this driver reads the byte unsigned, as the document's table is numbered")
    bad = bytearray(frames.encode_feedback())
    bad[48:56] = b"\x00" * 8
    check(parser.parse(bytes(bad)) is None and not _decodes(bytes(bad)), "a packet whose self-test field is wrong is refused by both")
    check(parser.parse(frames.encode_feedback()[:-1]) is None and not _decodes(frames.encode_feedback()[:-1]), "and so is one a byte short")

    # ── the commands ──────────────────────────────────────────────────────────────────────────────
    section("the commands — as the client writes them")
    recorder = Recorder()
    control, motion = RobotControl(recorder), Motion(recorder)
    plain = [(control.RequestControl, frames.request_control()), (control.PowerOn, frames.power_on()), (control.DisableRobot, frames.disable_robot()), (control.ClearError, frames.clear_error()), (control.Stop, frames.stop()),
             (lambda: control.EmergencyStop(1), frames.emergency_stop(True)), (lambda: control.EmergencyStop(0), frames.emergency_stop(False)), (control.RobotMode, frames.robot_mode()), (control.GetAngle, frames.get_angle()),
             (control.GetErrorID, frames.get_error_id())]
    for call, wanted in plain:
        recorder.sent.clear()
        call()
        check(recorder.sent == [wanted], f"{wanted} is what the client sends", str(recorder.sent))
    for percent in (1, 37, 100):
        recorder.sent.clear()
        control.SpeedFactor(percent)
        check(recorder.sent == [frames.speed_factor(percent)], f"SpeedFactor({percent}) is what the client sends", str(recorder.sent))
    for _ in range(20):
        load = round(rng.uniform(0.001, 20), 3)
        centre = tuple(round(rng.uniform(-100, 100), 3) for _ in range(3))
        recorder.sent.clear()
        control.EnableRobot(load=load)
        control.EnableRobot(load=load, centerX=centre[0], centerY=centre[1], centerZ=centre[2], isCheck=0)
        check(recorder.sent == [frames.enable_robot(load), frames.enable_robot(load, centre)], f"EnableRobot with a load of {load} kg, with and without its centre, is what the client sends", str(recorder.sent))
    recorder.sent.clear()
    control.EnableRobot(load=0.0)
    check(recorder.sent == ["EnableRobot()"] and frames.enable_robot(0.0) == "EnableRobot(0.000000)" and frames.enable_robot(0.0) != recorder.sent[0], "**a load of zero is where they differ**: the client sends EnableRobot(), which leaves the controller's own payload, and this driver sends the zero it was given (the maker's client cannot)")
    for _ in range(30):
        joints = [round(rng.uniform(-360, 360), 6) for _ in range(6)]
        t, ahead, gain = round(rng.uniform(0.004, 3600), 3), round(rng.uniform(20, 100), 1), round(rng.uniform(200, 1000), 1)
        recorder.sent.clear()
        motion.ServoJ(joints, t=t, aheadtime=ahead, gain=gain)
        check(recorder.sent == [frames.servo_j(joints, t=t, aheadtime=ahead, gain=gain)], "ServoJ with all its parameters is what the client sends", str(recorder.sent))
        a, v = rng.randrange(1, 101), rng.randrange(1, 101)
        recorder.sent.clear()
        motion.MovJ(joints, CoordinateType.JOINT, a=a, v=v)
        motion.MovJ(joints, CoordinateType.JOINT)
        check(recorder.sent == [frames.mov_j(joints, a=a, v=v), frames.mov_j(joints)], "and MovJ of a joint target, with and without a and v", str(recorder.sent))
    recorder.sent.clear()
    motion.ServoJ([1, 2, 3, 4, 5, 6])
    check(recorder.sent == ["ServoJ(1.000000,2.000000,3.000000,4.000000,5.000000,6.000000,t=0.100,aheadtime=50.0,gain=500.0)"] and frames.servo_j([1, 2, 3, 4, 5, 6]) == "ServoJ(1.000000,2.000000,3.000000,4.000000,5.000000,6.000000)",
          "**the client always sends ServoJ's three optional parameters, with the document's defaults; this driver sends only the ones the operator set**, and the controller's own defaults are the same numbers")

    # ── the replies and the modes ─────────────────────────────────────────────────────────────────
    section("the replies and the modes")
    for _ in range(40):
        ids = [rng.randrange(1, 9000) for _ in range(rng.randrange(0, 6))]
        text = f"0,{{[{','.join(map(str, ids))}]}},GetErrorID();"
        reply = frames.parse_reply(text)
        with contextlib.redirect_stdout(io.StringIO()):
            theirs = parse_error_ids(text)
        check(reply.error == 0 and frames.error_ids(reply.payload) == ids, f"an alarm list {ids} is read by this driver")
        check(theirs == [], "**and the client's parse_error_ids returns nothing for it**: its `start` is a comment (a line break was lost), so it fails with a NameError that it catches, and prints in Chinese, and answers []" if ids else "(an empty list is [] to both)")
    check(parse_error_ids(frames.NOT_TCP) == [] and raises_protocol(frames.parse_reply, frames.NOT_TCP), "a controller that is not in TCP mode answers with text that is no reply: the client reads no alarms from it, and this driver refuses it as a reply (the link tells it apart by the text)")
    check([int(m) for m in SdkMode if int(m) != 0] == list(range(1, 12)) and set(frames.MODES) == set(range(1, 12)), "the client's modes are the document's 1 to 11, and a 0 of its own")
    check(SdkMode(11).name == "JOG" and frames.MODE_NAMES[11] == "ROBOT_MODE_COLLISION" and frames.MODE_COLLISION == 11, "**mode 11 is where they differ**: the client names it JOG and the document COLLISION (collision detection triggered); this driver reads it as the document does, a fault")
    for number in range(1, 11):
        check(SdkMode(number).name.replace("_", "") in frames.MODE_NAMES[number].replace("_", "").replace("ROBOTMODE", ""), f"mode {number} is {frames.MODE_NAMES[number]}, which the client names {SdkMode(number).name}")

    # ── the two together ──────────────────────────────────────────────────────────────────────────
    section("the client against the simulated controller, and this driver against it, each reading where the other put the arm")
    arm = SimulatedController("CR5", time_scale=20.0, powered=True, enabled=False, tcp_mode=False).start()
    robot = None
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            robot = DobotRobot("127.0.0.1")
            robot._dashboard_conn.port = arm.dashboard_port   # noqa: SLF001 - the client accepts only the factory ports; the simulated controller listens on others, so its connections are pointed at them after the check
            robot._feedback_conn.port = arm.feedback_port     # noqa: SLF001
            robot.Connect()
            robot.StartFeedbackMonitor()
        check(robot.IsConnected, "the client connects to both ports")
        check(until(lambda: robot.GetStatus() is not None, 3), "and reads the controller's feedback")
        status = robot.GetStatus()
        check(status is not None and int(status.robot_mode) == frames.MODE_DISABLED and status.robot_type == 5 and not status.enable_status, "a controller that is powered and not enabled: the client reads mode 4, a CR5, and not enabled")
        answer = robot.robot_control.RobotMode()
        check("Control Mode Is Not Tcp" in answer, "a controller that is not in TCP mode answers the client's RobotMode() with the text the client matches", answer)
        answer = robot.robot_control.RequestControl()
        check(frames.parse_reply(answer).error == 0 and arm.tcp_mode, "RequestControl() takes control", answer)
        check(frames.parse_reply(robot.robot_control.ClearError()).error == 0, "ClearError() is taken")
        reply = frames.parse_reply(robot.robot_control.EnableRobot(load=1.0))
        check(reply.error == 0 and until(lambda: arm.enabled and int(robot.GetStatus().robot_mode) == frames.MODE_ENABLE, 3) and arm.load_kg == 1.0, "EnableRobot(1.0) enables the arm with its load, and the client's feedback shows mode 5")
        check(frames.parse_reply(robot.robot_control.SpeedFactor(50)).error == 0 and arm.speed_factor == 50, "SpeedFactor(50) is taken")
        angles = frames.numbers(frames.parse_reply(robot.robot_control.GetAngle()).payload, 6)
        check(max(abs(a - b) for a, b in zip(angles, arm.joints_deg)) < 1e-3, "GetAngle() answers the six joint angles in degrees", str(angles))
        reply = frames.parse_reply(robot.motion.MovJ([10.0, 20.0, 30.0, 0.0, 0.0, 0.0], CoordinateType.JOINT, v=100))
        check(reply.error == 0 and reply.payload.strip().isdigit(), "MovJ of a joint target is queued, and answers its queue id", str(reply))
        check(until(lambda: max(abs(a - b) for a, b in zip(arm.joints_deg, [10.0, 20.0, 30.0, 0.0, 0.0, 0.0])) < 0.05 and int(robot.GetStatus().robot_mode) == frames.MODE_ENABLE, 8), "the arm gets there, and the client's feedback shows it idle again")
        check(max(abs(a - b) for a, b in zip(robot.GetStatus().joint_state.q_actual, arm.joints_deg)) < 0.5, "and its QActual is where the arm is")
        for step in range(12):
            reply = frames.parse_reply(robot.motion.ServoJ([10.0 + step, 20.0, 30.0, 0.0, 0.0, 0.0], t=0.1))
            time.sleep(0.03)
        check(reply.error == 0 and len(arm.servo_calls) == 12 and arm.servo_calls[-1][2] == 0.1, "twelve ServoJ calls at 33 Hz are all taken, with their t")
        check(until(lambda: abs(arm.joints_deg[0] - 21.0) < 0.2, 3), "and the arm follows them")
        # this driver, on the same controller
        config = {"host": "127.0.0.1", "port": arm.dashboard_port, "feedback_port": arm.feedback_port, "model": "CR5", "stop_mode": "stop", "simulator": True,
                  "limits": [[-6.0, 6.0], [-3.0, 3.0], [-2.5, 2.5], [-6.0, 6.0], [-6.0, 6.0], [-6.0, 6.0]], "payload_kg": 1.0}
        ours = DobotBridge()
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = ours.open(json.dumps(config))
        check(code == rhal.SKR_OK, "this driver opens the arm the client has enabled and moved", said.getvalue())
        state = rhal.State()
        check(until(lambda: ours.read_state(state, 0) == rhal.SKR_OK, 3) and abs(state.position_rad[0] - math.radians(arm.joints_deg[0])) < 1e-3, "and reads where it is, in radians")
        target = [math.radians(v) for v in (30.0, 10.0, 5.0, 0.0, 0.0, 0.0)]
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=target, mask=[1] * 6)
        with contextlib.redirect_stderr(said):
            code = ours.write_command(command, 0)
        check(code == rhal.SKR_OK and until(lambda: max(abs(a - b) for a, b in zip(robot.GetStatus().joint_state.q_actual, (30.0, 10.0, 5.0, 0, 0, 0))) < 0.5, 5), "this driver commands it with a ServoJ, and the client's feedback shows it there")
        check(abs(arm.servo_calls[-1][1][0] - 30.0) < 1e-5 and arm.servo_calls[-1][2] == 0.1, "(the ServoJ it sent carries the angles in degrees, and the controller's own default t)")
        ours.emergency_stop()
        check(until(lambda: ours._stop_delivered, 2) and "Stop()" in arm.requests, "its stop is the controller's Stop(), and the client's feedback shows the arm idle")  # noqa: SLF001
        ours.release_stop()
        ours.close()
        arm.raise_alarm(132)
        listed = [frames.error_ids(frames.parse_reply(robot.robot_control.GetErrorID()).payload) for _ in range(1)]
        check(listed == [[132]], "an alarm raised on the controller is listed by GetErrorID() as the client asks for it, and this driver reads it from the client's reply", str(listed))
        check(until(lambda: int(robot.GetStatus().robot_mode) == frames.MODE_ERROR, 3), "and the client's feedback shows mode 9")
    finally:
        if robot is not None:
            with contextlib.redirect_stdout(io.StringIO()):
                robot.Disconnect()
        arm.stop()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAILED: {failure}")
    return 1 if FAILURES else 0


def _decodes(packet: bytes) -> bool:
    try:
        frames.decode_feedback(packet)
    except frames.ProtocolError:
        return False
    return True


def raises_protocol(function, *args) -> bool:
    try:
        function(*args)
    except frames.ProtocolError:
        return True
    return False


if __name__ == "__main__":
    sys.exit(main())
