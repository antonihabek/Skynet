"""The Dobot bridge against a simulated controller written here (DRY-COVERAGE-PLAN.md item 12.E.13): the frames, the two links, what it refuses to open on, the state, the refusals, the faults, the stop, the motions and the
lines.

    PYTHONPATH=../../bindings/python:. python3 selftest.py

Needs python3 on Linux and nothing of the maker's. The simulated controller agrees with whoever wrote it (F15); `layout_check.py` holds the driver's text and packets to Dobot's own Python client, which is run, and what
this shows is the driver's own logic.

Every wait is for a state (`until`), never for a count of polls or a sleep that something else is supposed to have finished inside: a loaded machine stretches every sleep (F210, `tools/flake-hunt.sh`). Private state
is set only on a bridge that has no thread alive to change it.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import select
import socket
import struct
import sys
import threading
import time
from unittest import mock

import skynet_rhal as rhal

from skynet_dobot import bridge as bridge_module, frames, link
from skynet_dobot.bridge import DobotBridge, TOKEN
from skynet_dobot.controller_sim import SimulatedController

CHECKS = 0
FAILURES: list[str] = []
SCALE = 20.0           # the simulated joints move this much faster than their speed says
LIMITS = [[-6.0, 6.0], [-3.0, 3.0], [-2.5, 2.5], [-6.0, 6.0], [-6.0, 6.0], [-6.0, 6.0]]
DEG = math.pi / 180.0
TYPE_OF = {name: number for number, name in frames.ROBOT_TYPES.items()}


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


def raises(kind: type, function, *args, **kwargs) -> str | None:
    """The message of the `kind` the call raises, or None when it raises nothing (or something else)."""
    try:
        function(*args, **kwargs)
    except kind as error:
        return str(error)
    except Exception:  # noqa: BLE001
        return None
    return None


def new_sim(model: str = "CR5", **kw) -> SimulatedController:
    kw.setdefault("time_scale", SCALE)
    kw.setdefault("enabled", True)
    return SimulatedController(model, **kw).start()


def settings(sim: SimulatedController, **extra) -> dict:
    out = {"host": "127.0.0.1", "port": sim.dashboard_port, "feedback_port": sim.feedback_port, "model": sim.model, "stop_mode": "stop", "simulator": True, "limits": LIMITS, "payload_kg": 0.5, "rate_hz": 100, "timeout_ms": 120,
           "roles": {"j2": "right_arm.shoulder.pitch", "j6": "right_arm.wrist.roll"}}
    out.update(extra)
    return {k: v for k, v in out.items() if v is not None}


def config(sim: SimulatedController, **extra) -> str:
    return json.dumps(settings(sim, **extra))


def opened(sim: SimulatedController, **extra) -> DobotBridge:
    # An arm that is still on its way to the last target is a moving one: the open reads `GetAngle()` and the feedback a moment apart, and refuses (with reason) an arm that moved ten degrees between them, which a
    # busy machine makes of a driver closed a moment ago. The open under test is the one on an arm at rest.
    until(lambda: not sim.moving, 3.0)
    bridge = DobotBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    assert code == rhal.SKR_OK, said.getvalue()
    return bridge


def opens_with(sim: SimulatedController, **extra) -> tuple[int, str, DobotBridge]:
    bridge = DobotBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    return code, said.getvalue(), bridge


def refused(text: str) -> tuple[int, str]:
    bridge = DobotBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(text)
    bridge.close()
    return code, said.getvalue()


def state(bridge: DobotBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def cmd(values: list[float], mask: list[int] | None = None, mode: int = rhal.MODE_POSITION, count: int | None = None) -> rhal.Command:
    n = count if count is not None else len(values)
    return rhal.Command(stamp_ns=1, seq=1, mode=mode, values=list(values), mask=mask if mask is not None else [1] * n)


def pose(value: float | list[float]) -> list[float]:
    return [value] * 6 if isinstance(value, (int, float)) else list(value)


def write(bridge: DobotBridge, command: rhal.Command) -> tuple[int, str]:
    bridge._last_refusal = None  # noqa: SLF001 - a refusal is said once for a run of the same cause; each check asks to hear it
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def ext(bridge: DobotBridge, text: str) -> tuple[int, object]:
    code, body = bridge.extension(TOKEN, text.encode())
    try:
        return code, json.loads(body)
    except json.JSONDecodeError:
        return code, body.decode()


def ready(bridge: DobotBridge, timeout: float = 5.0) -> bool:
    return until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, timeout)


def said_by(function, *args) -> tuple[object, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        result = function(*args)
    return result, said.getvalue()


def fds() -> int:
    return len(os.listdir("/proc/self/fd"))


def settled_fds() -> int:
    """The number of open descriptors once nothing is closing them: the simulated controller's own threads close their side of a connection a moment after the driver closes its side."""
    last, since, deadline = fds(), time.monotonic(), time.monotonic() + 3.0
    while time.monotonic() < deadline:
        time.sleep(0.02)
        now = fds()
        if now != last:
            last, since = now, time.monotonic()
        elif time.monotonic() - since >= 0.25:
            break
    return last


def holding_the_dashboard(bridge: DobotBridge, seconds: float) -> threading.Thread:
    """A thread that owns the dashboard's lock for `seconds`, as a command in flight does."""
    taken = threading.Event()

    def hold() -> None:
        with bridge._dashboard_lock:  # noqa: SLF001
            taken.set()
            time.sleep(seconds)

    thread = threading.Thread(target=hold, daemon=True)
    thread.start()
    taken.wait(1.0)
    return thread


def packet_of(**fields) -> bytes:
    return frames.encode_feedback(**fields)


def main_frames() -> None:
    section("frames — the commands, byte for byte")
    check(frames.request_control() == "RequestControl()" and frames.power_on() == "PowerOn()" and frames.disable_robot() == "DisableRobot()" and frames.clear_error() == "ClearError()" and frames.stop() == "Stop()", "the commands that take nothing")
    check(frames.robot_mode() == "RobotMode()" and frames.get_angle() == "GetAngle()" and frames.get_error_id() == "GetErrorID()", "and the three that ask")
    check(frames.emergency_stop(True) == "EmergencyStop(1)" and frames.emergency_stop(False) == "EmergencyStop(0)", "a software emergency stop is pressed with 1 and released with 0")
    check(frames.enable_robot() == "EnableRobot()" and frames.enable_robot(1.5) == "EnableRobot(1.500000)" and frames.enable_robot(0.0) == "EnableRobot(0.000000)", "EnableRobot with a load has six decimals, and a load of zero is sent as the zero it is")
    check(frames.enable_robot(1.5, (0.0, 0.0, 30.5)) == "EnableRobot(1.500000,0.000000,0.000000,30.500000,0)", "with the load's centre it has four numbers and the document's `isCheck` of 0")
    for bad, why in ((-0.1, "a negative load"), (math.nan, "a load that is not a number"), (math.inf, "an infinite one"), (True, "a bool"), ("1", "a string")):
        check(raises(frames.ProtocolError, frames.enable_robot, bad) is not None, f"{why} is refused")
    check(raises(frames.ProtocolError, frames.enable_robot, None, (0.0, 0.0, 1.0)) is not None and raises(frames.ProtocolError, frames.enable_robot, 1.0, (0.0, 1.0)) is not None and raises(frames.ProtocolError, frames.enable_robot, 1.0, (0.0, 0.0, math.nan)) is not None,
          "a centre with no load, with two numbers, or with one that is not a number is refused")
    check([frames.speed_factor(n) for n in (1, 50, 100)] == ["SpeedFactor(1)", "SpeedFactor(50)", "SpeedFactor(100)"], "a speed factor is a whole percentage from 1 to 100")
    for bad in (0, 101, -1, True, 1.5, "50"):
        check(raises(frames.ProtocolError, frames.speed_factor, bad) is not None, f"a speed factor of {bad!r} is refused")
    check(frames.servo_j([0, 0, -90, 0, 90, 0]) == "ServoJ(0.000000,0.000000,-90.000000,0.000000,90.000000,0.000000)", "a ServoJ of six angles has six decimals each, and nothing else when nothing else is given")
    check(frames.servo_j([1, 2, 3, 4, 5, 6], t=0.1, aheadtime=50, gain=500) == "ServoJ(1.000000,2.000000,3.000000,4.000000,5.000000,6.000000,t=0.100,aheadtime=50.0,gain=500.0)", "with its optional parameters: t to three decimals, the others to one, in that order")
    check(frames.servo_j([1, 2, 3, 4, 5, 6], gain=250) .endswith(",gain=250.0)") and "aheadtime" not in frames.servo_j([1, 2, 3, 4, 5, 6], gain=250) and "t=" not in frames.servo_j([1, 2, 3, 4, 5, 6], gain=250), "each is optional on its own")
    check(frames.servo_j([1, 2, 3, 4, 5, 6], t=0.004).endswith(",t=0.004)") and frames.servo_j([1, 2, 3, 4, 5, 6], t=3600.0).endswith(",t=3600.000)"), "t takes both ends of the document's range, 0.004 and 3600 s")
    check(frames.servo_j([1, 2, 3, 4, 5, 6], aheadtime=20).endswith(",aheadtime=20.0)") and frames.servo_j([1, 2, 3, 4, 5, 6], aheadtime=100).endswith(",aheadtime=100.0)") and frames.servo_j([1, 2, 3, 4, 5, 6], gain=200).endswith(",gain=200.0)") and frames.servo_j([1, 2, 3, 4, 5, 6], gain=1000).endswith(",gain=1000.0)"),
          "and so do aheadtime (20 to 100) and gain (200 to 1000)")
    for name, value in (("t", 0.0039), ("t", 3600.1), ("t", math.nan), ("aheadtime", 19.9), ("aheadtime", 100.1), ("gain", 199.9), ("gain", 1000.1), ("gain", math.inf), ("t", True)):
        check(raises(frames.ProtocolError, frames.servo_j, [0] * 6, **{name: value}) is not None, f"{name} of {value!r} is refused")
    for bad, why in (([0] * 5, "five angles"), ([0] * 7, "seven"), ([0, 0, 0, 0, 0, math.nan], "an angle that is not a number"), ([0, 0, 0, 0, 0, math.inf], "an infinite one"), ((0, 0, 0, 0, 0, True), "a bool"), ("012345", "a string"), (None, "nothing")):
        check(raises(frames.ProtocolError, frames.servo_j, bad) is not None, f"{why} is not a ServoJ target")
    check(frames.mov_j([1, 2, 3, 4, 5, 6]) == "MovJ(joint={1.000000,2.000000,3.000000,4.000000,5.000000,6.000000})" and frames.mov_j([0] * 6, a=50, v=60).endswith(",a=50,v=60)") and frames.mov_j([0] * 6, v=1).endswith(",v=1)") and "a=" not in frames.mov_j([0] * 6, v=1),
          "a MovJ of a joint target, with the acceleration and the speed as percentages, each on its own")
    for kw in ({"a": 0}, {"a": 101}, {"v": 0}, {"v": 101}, {"v": 5.5}, {"a": True}, {"v": "50"}):
        check(raises(frames.ProtocolError, frames.mov_j, [0] * 6, **kw) is not None, f"{kw} is refused")
    check(raises(frames.ProtocolError, frames.mov_j, [0] * 5) is not None and raises(frames.ProtocolError, frames.mov_j, [0, 0, 0, 0, 0, math.nan]) is not None, "a MovJ target is six numbers")
    check((frames.DASHBOARD_PORT, frames.FEEDBACK_PORTS, frames.FEEDBACK_BYTES, frames.FEEDBACK_MAGIC, frames.NOT_TCP) == (29999, (30004, 30005, 30006), 1440, 0x0123456789ABCDEF, "Control Mode Is Not Tcp"), "the dashboard is 29999, the feedback ports 30004 to 30006, a packet 1,440 bytes with the document's self-test value, and the text of a controller not in TCP mode")
    check(frames.SERVO_T == (0.004, 3600.0, 0.1) and frames.SERVO_AHEAD == (20.0, 100.0, 50.0) and frames.SERVO_GAIN == (200.0, 1000.0, 500.0), "ServoJ's three ranges and defaults are the document's")

    section("frames — the replies")
    reply = frames.parse_reply("0,{},MovL(-500, 100, 200, 150, 0, 90);")
    check((reply.error, reply.payload, reply.echo) == (0, "", "MovL(-500, 100, 200, 150, 0, 90)"), "the document's own example reply: an ErrorID, an empty value in braces, and the command as heard")
    reply = frames.parse_reply("-10000, {}, Mov(-500,100,200,150,0,90);")
    check((reply.error, reply.payload, reply.echo) == (-10000, "", "Mov(-500,100,200,150,0,90)"), "and the one for a command that does not exist, with its spaces")
    reply = frames.parse_reply("0,{12},ServoJ(0,0,-90,0,90,0);\n")
    check((reply.error, reply.payload) == (0, "12") and frames.parse_reply("0,{[1537,2048]},GetErrorID();").payload == "[1537,2048]" and frames.parse_reply("0,{{1,2},{3}},X();").payload == "{1,2},{3}", "a value is what is between the outer braces, braces inside it and all, and a trailing newline is not part of the reply")
    check(frames.parse_reply("-30002,{},DO(1,\"2\");").error == -30002, "an ErrorID is signed")
    for text, why in (("0,{},X()", "no closing semicolon"), ("", "nothing"), ("0 {} X();", "no comma"), ("x,{},X();", "an ErrorID that is not a number"), ("0,12,X();", "a value with no braces"), ("0,{12,X();", "braces that are not closed"), ("0,{12} X();", "no comma after the value"),
                      (",{},X();", "no ErrorID"), ("Control Mode Is Not Tcp", "a controller that is not in TCP mode")):
        check(raises(frames.ProtocolError, frames.parse_reply, text) is not None, f"a reply with {why} is refused")
    check(frames.numbers("1.5,-2,3e2", 3) == [1.5, -2.0, 300.0] and frames.numbers("") == [] and frames.numbers("7", 1) == [7.0], "the numbers of a value, any count or the count asked for")
    for text, count in (("1,2", 3), ("1,x", None), ("nan", None), ("inf,1", None), ("1,,2", None)):
        check(raises(frames.ProtocolError, frames.numbers, text, count) is not None, f"{text!r} is not {count or 'a list of'} finite numbers")
    check(frames.error_ids("[]") == [] and frames.error_ids("[7]") == [7] and frames.error_ids("[ 1537, 2048 ,2049]") == [1537, 2048, 2049], "an alarm list is its integers, spaces and all")
    for text in ("", "1,2", "[1,x]", "[1", "1]", "{1}"):
        check(raises(frames.ProtocolError, frames.error_ids, text) is not None, f"{text!r} is not an alarm list")
    check(frames.ERRORS == {0: "no error", -1: "the command was received and failed", -2: "the arm is in an alarm state", -3: "the arm is in emergency stop", -4: "the arm is powered off", -5: "the arm is running a script",
                            -6: "the jog axis and its coordinate type do not match", -7: "the arm is running a paused script", -8: "the controller's authorisation has expired", -9: "the command is disabled",
                            -10000: "the command does not exist", -20000: "the number of parameters is wrong"}, "the general error codes are the document's")
    check([frames.describe_error(c) for c in (-30001, -30002, -40001, -50003, -60001)] == ["a mandatory parameter has the wrong type (parameter 1)", "a mandatory parameter has the wrong type (parameter 2)", "a mandatory parameter is out of range (parameter 1)",
                                                                                       "an optional parameter has the wrong type (parameter 3)", "an optional parameter is out of range (parameter 1)"], "and the parameter families say which parameter, and what is wrong with it")
    check("which the interface document does not list" in frames.describe_error(-77) and "which the interface document does not list" in frames.describe_error(-30100) and "which the interface document does not list" in frames.describe_error(5) and frames.describe_error(-30099).endswith("(parameter 99)"), "an ErrorID the document does not list says so")
    check(frames.MODES == {1: "initialising", 2: "a brake is open", 3: "powered off", 4: "disabled", 5: "enabled and idle", 6: "drag mode", 7: "running", 8: "a single move", 9: "alarm", 10: "paused", 11: "collision detected"}
          and (frames.MODE_INIT, frames.MODE_BRAKE_OPEN, frames.MODE_POWEROFF, frames.MODE_DISABLED, frames.MODE_ENABLE, frames.MODE_BACKDRIVE, frames.MODE_RUNNING, frames.MODE_SINGLE_MOVE, frames.MODE_ERROR, frames.MODE_PAUSE, frames.MODE_COLLISION) == tuple(range(1, 12)),
          "RobotMode's eleven values are the document's, and mode 11 is a collision where the maker's client says it is a jog")
    check({n: frames.MODE_NAMES[n] for n in (1, 5, 9, 11)} == {1: "ROBOT_MODE_INIT", 5: "ROBOT_MODE_ENABLE", 9: "ROBOT_MODE_ERROR", 11: "ROBOT_MODE_COLLISION"} and len(frames.MODE_NAMES) == 11, "with the document's names")
    check(frames.ROBOT_TYPES == {3: "CR3", 5: "CR5", 7: "CR7", 10: "CR10", 12: "CR12", 16: "CR16", 101: "Nova 2", 103: "Nova 5", 113: "CR3A", 115: "CR5A", 116: "CR5AF", 117: "CR7A", 120: "CR10A", 121: "CR10AF", 122: "CR12A", 126: "CR16A",
                                 127: "CR20AF", 130: "CR20A", 150: "Magician E6", 160: "Nova 2 (new)", 161: "Nova 5 (new)", 162: "Nova 2S", 203: "CR3V", 205: "CR5V", 207: "CR7V", 210: "CR10V", 212: "CR12V", 216: "CR16V", 220: "CR20V"},
          "the robot types are the document's table")

    section("frames — the feedback packet")
    offsets = {"size": 0, "digital_inputs": 8, "digital_outputs": 16, "robot_mode": 24, "timestamp": 32, "run_time": 40, "test_value": 48, "speed_scaling": 64, "voltage": 88, "current": 96, "program_state": 104, "q_target": 192, "qd_target": 240,
               "qdd_target": 288, "i_target": 336, "m_target": 384, "q_actual": 432, "qd_actual": 480, "i_actual": 528, "tcp_force_actual": 576, "tool_vector_actual": 624, "tcp_speed_actual": 672, "tcp_force": 720, "tool_vector_target": 768,
               "tcp_speed_target": 816, "motor_temperatures": 864, "joint_modes": 912, "v_actual": 960, "user": 1012, "tool": 1013, "run_queued_cmd": 1014, "pause_cmd_flag": 1015, "velocity_ratio": 1016, "acceleration_ratio": 1017,
               "brake_status": 1025, "enable_status": 1026, "drag_status": 1027, "running_status": 1028, "error_status": 1029, "jog_status": 1030, "robot_type": 1031, "collision_state": 1038, "current_command_id": 1112, "m_actual": 1120,
               "load": 1168, "center_x": 1176, "center_y": 1184, "center_z": 1192, "safety_state": 1420}
    check(frames._OFFSETS == offsets, "every offset is the document's, retyped here as literals so that a changed one is a red check")  # noqa: SLF001
    q, target, qd, temps = (10.5, -20.25, 30.125, -40.0, 50.0, 360.0), (11.0, -21.0, 31.0, -41.0, 51.0, 361.0), (1.0, 2.0, 3.0, 4.0, 5.0, 6.0), (31.0, 32.0, 33.0, 34.0, 35.0, 36.0)
    raw = packet_of(robot_mode=7, enable_status=1, brake_status=0x3F, error_status=1, running_status=1, collision_state=1, drag_status=2, robot_type=103, safety_state=0xFE, speed_scaling=42.0, q_actual=q, q_target=target, qd_actual=qd,
                    motor_temperatures=temps, timestamp_ms=1_700_000_000_123, run_time_ms=987_654, current_command_id=77, digital_inputs=0x8000000000000001, digital_outputs=0x0F, load_kg=2.5)
    feedback = frames.decode_feedback(raw)
    check(len(raw) == 1440 and struct.unpack_from("<H", raw, 0)[0] == 1440 and struct.unpack_from("<Q", raw, 48)[0] == 0x0123456789ABCDEF, "a packet is 1,440 bytes, says so in its first two, and holds the self-test value at byte 48")
    check((feedback.robot_mode, feedback.enable_status, feedback.brake_status, feedback.error_status, feedback.running_status, feedback.collision_state, feedback.drag_status, feedback.robot_type, feedback.safety_state) == (7, 1, 0x3F, 1, 1, 1, 2, 103, 0xFE),
          "every one-byte field is read, unsigned")
    check((feedback.q_actual, feedback.q_target, feedback.qd_actual, feedback.motor_temperatures, feedback.speed_scaling) == (q, target, qd, temps, 42.0), "and the six-number fields and the speed scaling, exactly")
    check((feedback.timestamp_ms, feedback.run_time_ms, feedback.current_command_id, feedback.digital_inputs, feedback.digital_outputs, feedback.load_kg) == (1_700_000_000_123, 987_654, 77, 0x8000000000000001, 0x0F, 2.5), "and the counters, the inputs and outputs whole, and the load")
    check(frames.decode_feedback(packet_of(robot_type=220)).robot_type == 220, "a robot type above 127 is read as it is numbered (the maker's client reads it as a signed byte)")
    bad = bytearray(raw)
    bad[48:56] = b"\x00" * 8
    check("self-test" in (raises(frames.ProtocolError, frames.decode_feedback, bytes(bad)) or "") and "1440" in (raises(frames.ProtocolError, frames.decode_feedback, raw[:-1]) or "") and raises(frames.ProtocolError, frames.decode_feedback, raw + b"\x00") is not None,
          "a packet whose self-test field is wrong, or that is a byte short or long, is refused, saying why")
    for name, kw in (("QActual", {"q_actual": (math.nan,) * 6}), ("QTarget", {"q_target": (0.0, 0.0, 0.0, 0.0, 0.0, math.inf)}), ("QDActual", {"qd_actual": (math.nan,) + (0.0,) * 5})):
        check(name in (raises(frames.ProtocolError, frames.decode_feedback, packet_of(**kw)) or ""), f"{name} with a value that is not a number is refused, naming it")


def tcp_server(script) -> tuple[int, threading.Thread]:
    """A listener on a loopback port that hands its first connection to `script(connection)`: a controller scripted by the test."""
    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", 0))
    server.listen(1)

    def serve() -> None:
        connection, _ = server.accept()
        try:
            script(connection)
        finally:
            connection.close()
            server.close()

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    return server.getsockname()[1], thread


def main_links() -> None:
    section("the dashboard — one command, its own reply, and what comes instead")
    sim = new_sim()
    try:
        dashboard = link.Dashboard("127.0.0.1", sim.dashboard_port)
        dashboard.open()
        reply = dashboard.command(frames.robot_mode(), 1.0)
        check((reply.error, reply.payload, reply.echo) == (0, "5", "RobotMode()") and dashboard.is_open and dashboard.target == f"127.0.0.1:{sim.dashboard_port}", "a command is answered by its reply, and the line is named by host and port")
        check(frames.numbers(dashboard.command(frames.get_angle(), 1.0).payload, 6) == [0.0] * 6, "a command with a value is answered with it")
        sim.error_next = [-2]
        check(dashboard.command(frames.servo_j([0] * 6), 1.0).error == -2, "an ErrorID other than 0 is a reply, not an exception")
        reply = dashboard.command("Bogus()", 1.0)
        check(reply.error == -10000, "a command the controller does not know is answered -10000 and is accepted as the answer to it")
        sim.delay_s = 0.15
        began = time.monotonic()
        message = raises(link.LinkError, dashboard.command, frames.robot_mode(), 0.05) or ""
        took = time.monotonic() - began
        sim.delay_s = 0.0
        check("did not answer RobotMode in 50 ms" in message and took < 0.2 and not dashboard.is_open, f"a command that is not answered in its budget is a LinkError that names the command and the time, and closes the line ({took:.2f} s)", message)
        check("is not open" in (raises(link.LinkError, dashboard.command, frames.robot_mode(), 0.1) or ""), "a command on a line that is closed is refused")
        dashboard.open()
        check(dashboard.command(frames.robot_mode(), 1.0).payload == "5", "and the line opened again carries the next, with no reply of the one before in it")
        sim.garble = True
        check(raises(frames.ProtocolError, dashboard.command, frames.robot_mode(), 1.0) is not None, "a reply that is damaged is a ProtocolError")
        sim.garble = False
        sim.set_mode_state(tcp=False)
        message = raises(link.NotTcp, dashboard.command, frames.robot_mode(), 1.0) or ""
        check("control mode is not TCP" in message and issubclass(link.NotTcp, link.LinkError) and dashboard.is_open, "a controller that is not in TCP mode is a NotTcp, which is a LinkError that leaves the line open", message)
        sim.set_mode_state(tcp=True)
        sim.hang_up()
        check(until(lambda: raises(link.LinkError, dashboard.command, frames.robot_mode(), 0.3) is not None and not dashboard.is_open, 3), "a controller that hangs up is a line that closes")
        dashboard.close()
        sim.stop_listening()
        check("could not be opened" in (raises(link.LinkError, dashboard.open, 0.3) or ""), "a controller that is not listening is a line that could not be opened")
        sim.resume_listening()
        held = settled_fds()
        for _ in range(4):
            dashboard.open()
        check(settled_fds() <= held + 2, "a line opened again lets go of the one it had: four reopenings leave one open (and the controller's end of it), not five")
        dashboard.close()
        with mock.patch.object(socket, "create_connection", side_effect=OSError("no route to host")) as patched:
            message = raises(link.LinkError, link.Dashboard("gateway.example", 29999).open, 0.7) or ""
        check("gateway.example:29999 could not be opened" in message and patched.call_args.args == (("gateway.example", 29999),) and patched.call_args.kwargs == {"timeout": 0.7}, "a controller is waited for as long as the caller says, and named by host and port when it cannot be reached", message)
    finally:
        sim.stop()

    section("the dashboard — a reply to something else is thrown away, and a reply may be a line or a sentence")
    sent = threading.Event()

    def other_replies(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.recv(1024)
        connection.sendall(b"0,{},Stop();\n0,{3},ServoJ(1.000000,2.000000,3.000000,4.000000,5.000000,6.000000);\n0,{5},RobotMode();\n")
        sent.set()
        time.sleep(0.3)

    port, thread = tcp_server(other_replies)
    dashboard = link.Dashboard("127.0.0.1", port)
    dashboard.open()
    reply = dashboard.command(frames.robot_mode(), 1.0)
    check(reply.payload == "5" and reply.echo == "RobotMode()", "the replies of other commands that arrive first are thrown away: the reply is the one that echoes the command that was sent")
    dashboard.close()
    thread.join(3.0)

    def spaced(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.recv(1024)
        connection.sendall(b"0, {7}, SERVOJ(1.0,  2.0,3.0, 4.0,5.0,6.0);")      # no newline, upper case, spaces added after the commas
        time.sleep(0.3)

    port, thread = tcp_server(spaced)
    dashboard = link.Dashboard("127.0.0.1", port)
    dashboard.open()
    reply = dashboard.command("ServoJ(1.0,2.0,3.0,4.0,5.0,6.0)", 1.0)
    check(reply.payload == "7", "an echo is matched without regard to case or spaces, and a reply that ends at its semicolon with no newline is a whole reply")
    dashboard.close()
    thread.join(3.0)

    def stale_first(connection: socket.socket) -> None:
        connection.sendall(b"0,{},RobotMode();\n0,{},RobotMode();\n")             # two replies nobody asked for, waiting when the command is sent
        sent.set()
        connection.settimeout(3.0)
        connection.recv(1024)
        connection.sendall(b"0,{4},RobotMode();\n")
        time.sleep(0.3)

    sent.clear()
    port, thread = tcp_server(stale_first)
    dashboard = link.Dashboard("127.0.0.1", port)
    dashboard.open()
    sent.wait(2.0)
    time.sleep(0.1)
    check(dashboard.command(frames.robot_mode(), 1.0).payload == "4", "whatever is waiting on the line when a command is sent is thrown away first, so that a reply that came late is not taken for this command's twin")
    dashboard.close()
    thread.join(3.0)

    def endless_other(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.recv(1024)
        try:
            for _ in range(1000):
                connection.sendall(b"0,{},Stop();\n")
        except OSError:
            pass                    # the driver gave up and hung up, as it should
        time.sleep(0.3)

    port, thread = tcp_server(endless_other)
    dashboard = link.Dashboard("127.0.0.1", port)
    dashboard.open()
    check("none to RobotMode" in (raises(link.LinkError, dashboard.command, frames.robot_mode(), 2.0) or "") and not dashboard.is_open, "a line that answers other commands for ever and never this one is given up on, and closed")
    thread.join(3.0)

    def noise(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.recv(1024)
        connection.sendall(b"x" * 6000)
        time.sleep(0.3)

    port, thread = tcp_server(noise)
    dashboard = link.Dashboard("127.0.0.1", port)
    dashboard.open()
    check("that are not a reply" in (raises(link.LinkError, dashboard.command, frames.robot_mode(), 2.0) or "") and not dashboard.is_open, "bytes that never form a reply are a line that is not the controller's, and it is closed")
    thread.join(3.0)

    def hangs_up(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.recv(1024)
        connection.shutdown(socket.SHUT_WR)
        time.sleep(0.5)

    port, thread = tcp_server(hangs_up)
    dashboard = link.Dashboard("127.0.0.1", port)
    dashboard.open()
    message = raises(link.LinkError, dashboard.command, frames.robot_mode(), 1.0) or ""
    check("closed the connection" in message and not dashboard.is_open, "a controller that ends the connection is a line that is closed, and says so, and not silence that is waited out", message)
    thread.join(3.0)

    section("the feedback — the newest packet, and a stream that has lost its place")
    packets = [packet_of(timestamp_ms=1), packet_of(timestamp_ms=2), packet_of(timestamp_ms=3)]

    def three_at_once(connection: socket.socket) -> None:
        connection.sendall(b"".join(packets))
        time.sleep(0.5)

    port, thread = tcp_server(three_at_once)
    feedback = link.FeedbackStream("127.0.0.1", port)
    feedback.open()
    time.sleep(0.1)
    got = feedback.read_packet(1.0)
    check(got == packets[2] and feedback.target == f"127.0.0.1:{port}", "of three packets that piled up while nobody looked, the newest is the one read, and the line is named by host and port")
    check(feedback.read_packet(0.0) is None, "and a look at a stream with nothing new is None, at once")
    feedback.close()
    thread.join(3.0)

    def junk_then_packets(connection: socket.socket) -> None:
        connection.sendall(b"\x01\x02\x03" * 100 + packets[0][:700])
        time.sleep(0.03)
        connection.sendall(packets[0][700:] + packets[1][:5])
        time.sleep(0.03)
        connection.sendall(packets[1][5:])
        time.sleep(0.5)

    port, thread = tcp_server(junk_then_packets)
    feedback = link.FeedbackStream("127.0.0.1", port)
    feedback.open()
    seen = []
    deadline = time.monotonic() + 3.0
    while len(seen) < 2 and time.monotonic() < deadline:
        packet = feedback.read_packet(0.2)
        if packet is not None:
            seen.append(packet)
    check(seen == [packets[0], packets[1]], "a stream that begins with junk finds its first packet, and a packet that comes in pieces is read whole")
    feedback.close()
    thread.join(3.0)

    def stops_half_way(connection: socket.socket) -> None:
        connection.sendall(packets[0][:800])
        time.sleep(0.6)

    port, thread = tcp_server(stops_half_way)
    feedback = link.FeedbackStream("127.0.0.1", port)
    feedback.open()
    began = time.monotonic()
    message = ""
    try:
        for _ in range(40):
            feedback.read_packet(0.1)
    except link.LinkError as error:
        message = str(error)
    took = time.monotonic() - began
    check("stopped sending 800 bytes into a packet of 1440" in message and took < 0.5, f"a packet that stops half way is given up on after the byte timeout, saying how far it got ({took:.2f} s)", message)
    feedback.close()
    thread.join(3.0)

    def only_noise(connection: socket.socket) -> None:
        connection.sendall(b"\xaa" * 20000)
        time.sleep(0.5)

    port, thread = tcp_server(only_noise)
    feedback = link.FeedbackStream("127.0.0.1", port)
    feedback.open()
    message = ""
    try:
        for _ in range(40):
            feedback.read_packet(0.2)
    except link.LinkError as error:
        message = str(error)
    check("none of them begins a feedback packet" in message and not feedback.is_open, "bytes that never begin a packet are a line that is noise, and it is closed", message)
    thread.join(3.0)

    def hangs_up_feedback(connection: socket.socket) -> None:
        connection.shutdown(socket.SHUT_WR)
        time.sleep(0.5)

    port, thread = tcp_server(hangs_up_feedback)
    feedback = link.FeedbackStream("127.0.0.1", port)
    feedback.open()
    message = raises(link.LinkError, feedback.read_packet, 1.0) or ""
    check("closed the connection" in message and not feedback.is_open and "is not open" in (raises(link.LinkError, feedback.read_packet, 0.1) or ""), "a controller that ends the connection is a line that is closed, and a read from a closed line is refused", message)
    thread.join(3.0)
    sim = new_sim()
    try:
        feedback = link.FeedbackStream("127.0.0.1", sim.feedback_port)
        feedback.open()
        got = feedback.read_packet(1.0)
        check(got is not None and frames.decode_feedback(got).robot_mode == 5 and len(got) == 1440, "a simulated controller's feedback is read, a packet of 1,440 bytes")
        sim.feedback_split = 700
        got = feedback.read_packet(1.0)
        check(got is not None and len(got) == 1440, "a packet the controller sends in two pieces is read whole")
        sim.feedback_split = 0
        feedback.close()
        held = settled_fds()
        for _ in range(4):
            feedback.open()
        check(settled_fds() <= held + 2, "a feedback line opened again lets go of the one it had")
        feedback.close()
    finally:
        sim.stop()


def main_open() -> None:
    section("open — what a configuration may not say")
    sim = new_sim()
    try:
        code, said = refused("not json")
        check(code == rhal.SKR_EINVAL and "not JSON" in said, "a configuration that is not JSON is EINVAL", said)
        code, said = refused("[1]")
        check(code == rhal.SKR_EINVAL and "not a JSON object" in said, "and so is one that is not an object", said)
        code, said = refused("")
        check(code == rhal.SKR_ENODEV and "`model`" in said and "not JSON" not in said, "an empty configuration is an empty object, refused for what it lacks", said)
        absent = object()
        for key, bad, word in (
                ("model", absent, "`model` of None is not in the configuration"), ("model", "CR9", "`model` of 'CR9'"), ("model", 5, "`model` of 5"), ("model", "cr5", "`model` of 'cr5'"),
                ("host", absent, "`host`"), ("host", "", "`host`"), ("host", 5, "`host`"),
                ("port", True, "`port`"), ("port", 0, "`port`"), ("port", 65536, "`port`"), ("port", "29999", "`port`"), ("port", 1.5, "`port`"),
                ("feedback_port", True, "`feedback_port`"), ("feedback_port", 0, "`feedback_port`"), ("feedback_port", 70000, "`feedback_port`"),
                ("stop_mode", absent, "stop_mode"), ("stop_mode", "hold", "stop_mode"), ("stop_mode", "Stop", "stop_mode"), ("stop_mode", True, "stop_mode"),
                ("simulator", absent, "simulator"), ("simulator", "yes", "simulator"), ("simulator", 1, "simulator"),
                ("limits", absent, "`limits` is not in the configuration"), ("limits", [[0, 1]] * 5, "six [min, max] pairs"), ("limits", [[0, 1]] * 7, "six [min, max] pairs"), ("limits", "x", "six [min, max] pairs"),
                ("limits", [[0, 1], [0, 1], [0, 1], [0, 1], [0, 1], [0]], "six [min, max] pairs"), ("limits", [[0, 1], [0, 1], [0, 1], [0, 1], [0, 1], [0, math.nan]], "six [min, max] pairs"),
                ("limits", [[0, 1], [0, 1], [0, 1], [0, 1], [0, 1], [True, 1]], "six [min, max] pairs"), ("limits", [[0, 1], [0, 1], [0, 1], [0, 1], [0, 1], [0, "1"]], "six [min, max] pairs"),
                ("limits", [[1, 0]] + [[0, 1]] * 5, "not a range"), ("limits", [[0.5, 0.5]] + [[0, 1]] * 5, "not a range"), ("limits", [[0, 1]] * 5 + [[-7, 1]], "past a turn either way"), ("limits", [[0, 7]] + [[0, 1]] * 5, "past a turn either way"),
                ("payload_kg", absent, "payload_kg"), ("payload_kg", -0.1, "payload_kg"), ("payload_kg", 100.1, "payload_kg"), ("payload_kg", True, "payload_kg"), ("payload_kg", "1", "payload_kg"), ("payload_kg", math.nan, "payload_kg"),
                ("servo", 5, "`servo`"), ("servo", {"speed": 1}, "`servo`"), ("servo", {"t": 0.003}, "servo` t of"), ("servo", {"t": 3601}, "servo` t of"), ("servo", {"aheadtime": 19}, "servo` aheadtime"), ("servo", {"aheadtime": 101}, "servo` aheadtime"),
                ("servo", {"gain": 199}, "servo` gain"), ("servo", {"gain": 1001}, "servo` gain"), ("servo", {"gain": True}, "servo` gain"), ("servo", {"t": math.nan}, "servo` t of"),
                ("rate_hz", 0.9, "rate_hz"), ("rate_hz", 250.1, "rate_hz"), ("rate_hz", True, "rate_hz"), ("rate_hz", "33", "rate_hz"), ("timeout_ms", 9.9, "timeout_ms"), ("timeout_ms", 5001, "timeout_ms"), ("stale_ms", 19.9, "stale_ms"),
                ("stale_ms", 30001, "stale_ms"), ("stale_ms", True, "stale_ms"), ("open_timeout_ms", 99.9, "open_timeout_ms"), ("open_timeout_ms", 60001, "open_timeout_ms"),
                ("roles", {"j7": "x"}, "roles"), ("roles", ["j1"], "roles")):
            if bad is absent:
                code, said = refused(json.dumps({k: v for k, v in settings(sim).items() if k != key}))
            else:
                code, said, bridge = opens_with(sim, **{key: bad})
                bridge.close()
            check(code == rhal.SKR_ENODEV and word in said, f"{key} of {'absent' if bad is absent else repr(bad)} is refused and says so", said)
        code, said = refused(json.dumps({**settings(sim), "frobnicate": 1, "debug": True}))
        check(code == rhal.SKR_ENODEV and "'debug'" in said and "'frobnicate'" in said and "keys it reads are" in said, "keys the driver does not read are refused, naming them and the keys it does read", said)
        check(bridge_module._KNOWN_KEYS == {"model", "host", "port", "feedback_port", "stop_mode", "simulator", "limits", "payload_kg", "servo", "roles", "rate_hz", "timeout_ms", "stale_ms", "open_timeout_ms"}, "the fourteen keys a configuration may carry, and no other")  # noqa: SLF001
        check((bridge_module._RATE_HZ, bridge_module._MAX_RATE_HZ, bridge_module._TRANSACTION_S, bridge_module._OPEN_TIMEOUT_S, bridge_module._STALE_S, bridge_module._COMMAND_BUDGET_S, bridge_module._FAULT_THRESHOLD,  # noqa: SLF001
               bridge_module._REOPEN_S, bridge_module._UNITS_TOLERANCE_DEG, bridge_module._ARRIVAL_TOLERANCE_DEG, bridge_module._ALARM_POLL_S) == (33.0, 250.0, 0.08, 3.0, 0.5, 0.25, 3, 0.5, 2.0, 0.5, 0.5),
              "33 Hz (250 at most), 80 ms a transaction, 3 s to open, 0.5 s before a reading is stale, 0.25 s a command, three lost packets to a dead line, half a second between reopenings, two degrees of disagreement between GetAngle() and the feedback, half a degree to arrive, and the alarm list read twice a second")
        check(bridge_module.ESTOP_ALARMS == frozenset({116, 132, 133}), "the alarms the maker's table describes as an emergency stop are 116, 132 and 133")  # noqa: SLF001
        for model, number in (("CR3", 3), ("CR5", 5), ("CR20V", 220), ("Nova 2", 101), ("Nova 2 (new)", 160), ("Magician E6", 150)):
            check(TYPE_OF[model] == number, f"{model} is robot type {number} in the document's table")
        code, said, bridge = opens_with(sim, model="CR3")
        check(code == rhal.SKR_ENODEV and "robot type 5" in said and "CR5" in said and "a CR3 is 3" in said and "check `model`" in said, "a CR5 opened as a CR3 is refused, naming both types", said)
        bridge.close()
        sim.set_mode_state(tcp=True)
        code, said, bridge = opens_with(sim, rate_hz=33.5, timeout_ms=105.5, stale_ms=123.5, open_timeout_ms=500.5, servo={"t": 0.05, "aheadtime": 20, "gain": 1000})
        check(code == rhal.SKR_OK and json.loads(bridge.describe())["control"]["rate_hz_max"] == 33.5 and abs(bridge._timeout - 0.1055) < 1e-12 and abs(bridge._stale_s - 0.1235) < 1e-12, "settings with a fraction keep it; the edges of servo's ranges are accepted", said)  # noqa: SLF001
        bridge.close()
        code, said, bridge = opens_with(sim, rate_hz=None)
        check(code == rhal.SKR_OK and json.loads(bridge.describe())["control"] == {"modes": ["position"], "rate_hz_max": 33.0, "rate_hz_min": 1.0}, "a rate that is not given is 33 Hz, the document's recommended rate for ServoJ", said)
        bridge.close()
        code, said, bridge = opens_with(sim, port=None, model="CR5")
        check(code == rhal.SKR_EIO and f"127.0.0.1:{frames.DASHBOARD_PORT}" in said + str(bridge._dashboard.target if bridge._dashboard else ""), "a dashboard port that is not given is the factory's 29999", said)  # noqa: SLF001
        bridge.close()
        for kw in ({"feedback_port": None},):
            code, said, bridge = opens_with(sim, **kw)
            check(code == rhal.SKR_EIO and "30004" in said, "and a feedback port that is not given is 30004", said)
            bridge.close()
    finally:
        sim.stop()

    section("open — the controller must answer, be the arm the operator said, and agree with itself")
    sim = new_sim()
    try:
        sim.silent_feedback = True
        began = time.monotonic()
        code, said, bridge = opens_with(sim, open_timeout_ms=300)
        took = time.monotonic() - began
        sim.silent_feedback = False
        check(code == rhal.SKR_EIO and "no feedback packet arrived" in said and took < 1.0, f"a controller whose feedback says nothing is EIO, and an open budget of 300 ms is 300 milliseconds ({took:.2f} s)", said)
        bridge.close()
        sim.stop_listening()
        code, said, bridge = opens_with(sim, open_timeout_ms=300)
        check(code == rhal.SKR_EIO and "could not be opened" in said, "a controller that is not listening is EIO, and says the line could not be opened", said)
        bridge.close()
        sim.resume_listening()
        sim.drop = True
        code, said, bridge = opens_with(sim, open_timeout_ms=300)
        sim.drop = False
        check(code == rhal.SKR_EIO and "dashboard did not answer" in said and "RobotMode" in said, "a dashboard that does not answer is EIO, and names the command", said)
        bridge.close()
        sim.place((30.0, -20.0, 10.0, 0.0, 0.0, 0.0))
        sim.feedback_q_scale = DEG
        code, said, bridge = opens_with(sim)
        sim.feedback_q_scale = 1.0
        check(code == rhal.SKR_ENODEV and "GetAngle()" in said and "differ" in said and "degrees" in said and "nothing was commanded" in said, "a controller whose feedback and GetAngle() disagree is refused: one of them is not in the degrees the document uses", said)
        bridge.close()
        sim.place((0.0,) * 6)
        sim.set_mode_state(tcp=False, enabled=False)        # this simulator gives control to a client only while the arm is not enabled
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_OK and bridge._tcp is False, "a controller that is not in TCP mode is opened for reading, and the driver knows it takes no command", said)  # noqa: SLF001
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3), "its feedback is read all the same")
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EPERM and "not TCP" in said and "request_control" in said, "a command to it is EPERM, naming the motion that asks for control", said)
        code, body = ext(bridge, "request_control")
        check(code == rhal.SKR_OK and body["result"] == "requested" and sim.tcp_mode and bridge._tcp is True, "the `request_control` motion takes control", str(body))  # noqa: SLF001
        sim.set_mode_state(enabled=True)
        check(until(lambda: write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, 3), "and then a command is taken")
        bridge.close()
        sim.set_mode_state(enabled=False)
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_OK and until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 2) and state(bridge)[0] == rhal.SKR_OK, "a disabled arm is opened, and its state is DEGRADED: it takes no command", said)
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EPERM and "disabled" in said and "`enable`" in said, "and a command is EPERM, naming the motion that enables it", said)
        bridge.close()
        sim.set_mode_state(powered=False)
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_OK, "an arm that is powered off is opened too", said)
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EPERM and "powered off" in said and "`power_on`" in said and state(bridge)[1].flags & rhal.FLAG_DEGRADED, "and a command is EPERM, naming the motion that powers it", said)
        bridge.close()
        sim.set_mode_state(powered=True, enabled=True)
        sim.raise_alarm(17)
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_OK and until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "an arm in its alarm state is opened, and its state is a fault", said)
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim()
    try:
        held = settled_fds()
        code, said, bridge = opens_with(sim, model="CR3")
        check(code == rhal.SKR_ENODEV and bridge._dashboard is None and bridge._feedback is None and bridge._loop is None and settled_fds() <= held, "an open that is refused leaves no line and no thread")  # noqa: SLF001
        bridge.close()
        for what, error in (("an OS error the line did not wrap", OSError(24, "Too many open files")), ("a protocol error", frames.ProtocolError("not a reply")), ("a line error", link.LinkError("the line is gone"))):
            with mock.patch.object(DobotBridge, "_probe_control", side_effect=error):
                code, said, bridge = opens_with(sim)
            check(code == rhal.SKR_EIO and "could not be read" in said and str(error) in said and bridge._dashboard is None and settled_fds() <= held, f"{what} at open is EIO, says what it was, and leaves no line open: it does not escape into the host", said)  # noqa: SLF001
            bridge.close()
        with mock.patch.object(DobotBridge, "_build_descriptor", side_effect=OSError(24, "Too many open files")):
            code, said, failed = opens_with(sim)
        check(code == rhal.SKR_EIO and failed._loop is None and failed._dashboard is None and settled_fds() <= held and not any(t.name == "dobot-exchange" and t.is_alive() for t in threading.enumerate()),  # noqa: SLF001
              "an open that fails with an error it did not expect, after its reading thread has started, leaves no thread and no line", said)
        failed.close()
        recorded: list[object] = []
        real_open = link.FeedbackStream.open

        def noting(self, timeout=1.0):
            recorded.append(timeout)
            return real_open(self, timeout)

        for open_ms, wanted in ((60000, 3.0), (500, 0.5)):
            recorded.clear()
            with mock.patch.object(link.FeedbackStream, "open", noting):
                refused(config(sim, open_timeout_ms=open_ms))
            check(recorded and recorded[0] == wanted, f"a connection is waited for at most three seconds, or the open budget when that is shorter ({open_ms} ms gives {wanted} s)")
    finally:
        sim.stop()


def lifeless_bridge(**over) -> DobotBridge:
    """A bridge with no line and no thread, holding a reading made here: for what `read_state`, `write_command` and the waits make of one, with nothing else able to change it."""
    bridge = DobotBridge()
    bridge._opened = True  # noqa: SLF001
    bridge._dashboard = link.Dashboard("127.0.0.1", 1)  # noqa: SLF001 - not open
    bridge._limits = [(a, b) for a, b in LIMITS]  # noqa: SLF001
    bridge._tcp = True  # noqa: SLF001
    set_reading(bridge, **over)
    return bridge


def set_reading(bridge: DobotBridge, *, alarms: list[int] | None = None, **fields) -> frames.Feedback:
    base = dict(robot_mode=5, enable_status=1, brake_status=0x3F, error_status=0, running_status=0, collision_state=0, robot_type=5, safety_state=0xFF)
    base.update(fields)
    reading = frames.decode_feedback(packet_of(**base))
    with bridge._lock:  # noqa: SLF001
        bridge._latest, bridge._stamp_ns, bridge._dead, bridge._alarms = reading, rhal.acquisition_ns(), None, list(alarms or [])  # noqa: SLF001
    return reading


def servo_texts(sim: SimulatedController) -> list[str]:
    return [r for r in list(sim.requests) if r.startswith("ServoJ")]


def main_state() -> None:
    section("read_state — what is made of a reading: which field is which, in what unit, and what the flags are")
    bridge = lifeless_bridge()
    q = (10.0, -20.0, 30.0, 45.0, -90.0, 180.0)
    set_reading(bridge, q_actual=q, q_target=(11.0,) * 6, qd_actual=(1.0,) * 6, motor_temperatures=(40.0,) * 6)
    rc, out = state(bridge)
    check(rc == rhal.SKR_OK and all(abs(out.position_rad[j] - q[j] * DEG) < 1e-12 for j in range(6)), "a position is QActual, not QTarget, in radians")
    check(all(math.isnan(v) for v in out.velocity_rad_s) and all(math.isnan(v) for v in out.effort_nm), "a velocity and an effort are not reported: the document gives their fields no unit, and a number in an unknown unit is worse than none")
    check(out.flags == rhal.HOMING_NOT_REQUIRED and out.fault_code == 0 and out.seq >= 1 and out.stamp_ns > 0, "a healthy arm: OK, no homing required, no fault, no stop and not degraded")
    first = out.seq
    check(state(bridge)[1].seq == first + 1, "and each state has a sequence number one past the one before")
    bridge._opened = False  # noqa: SLF001
    check(state(bridge)[0] == rhal.SKR_EPERM, "a driver that is not open reads EPERM")
    bridge._opened = True  # noqa: SLF001
    with bridge._lock:  # noqa: SLF001
        bridge._latest = None  # noqa: SLF001
    rc, out = state(bridge)
    check(rc == rhal.SKR_ETIMEDOUT and out.flags & rhal.FLAG_DEGRADED and all(math.isnan(v) for v in out.position_rad) and out.stamp_ns > 0, "an arm that has not been read has no position at all, not a position of zero, and the state is ETIMEDOUT and DEGRADED, stamped when it was asked for")
    set_reading(bridge)
    old = rhal.acquisition_ns() - 2_000_000_000
    with bridge._lock:  # noqa: SLF001
        bridge._stamp_ns = old  # noqa: SLF001
    rc, out = state(bridge)
    check(rc == rhal.SKR_ETIMEDOUT and out.flags & rhal.FLAG_DEGRADED and out.stamp_ns == old, "a reading older than the staleness bound is ETIMEDOUT and DEGRADED, and keeps the stamp of when it was taken")
    with bridge._lock:  # noqa: SLF001
        bridge._stamp_ns, bridge._dead = rhal.acquisition_ns(), "the line is dead"  # noqa: SLF001
    rc, out = state(bridge)
    check(rc == rhal.SKR_ETIMEDOUT and out.flags & rhal.FLAG_DEGRADED, "a dead line is ETIMEDOUT and DEGRADED though the reading is inside the staleness bound")

    section("read_state — one mode at a time: which are healthy, which are degraded, which are a fault, and which is a stop")
    for mode, running in ((5, True), (7, True), (8, True), (1, False), (2, False), (3, False), (4, False), (6, False), (10, False)):
        set_reading(bridge, robot_mode=mode, enable_status=1 if running else 0)
        rc, out = state(bridge)
        degraded = bool(out.flags & rhal.FLAG_DEGRADED)
        check(rc == rhal.SKR_OK and degraded == (not running) and not out.flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP) and out.fault_code == 0, f"mode {mode} ({frames.MODES[mode]}) is {'healthy' if running else 'DEGRADED and not a fault'}: it {'takes' if running else 'takes no'} command")
    set_reading(bridge, robot_mode=9, error_status=1)
    rc, out = state(bridge)
    check(rc == rhal.SKR_OK and out.flags & rhal.FLAG_FAULT and out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_ESTOP and out.fault_code == 9, "mode 9 (alarm) with no alarm identifier read yet is a fault, DEGRADED, and not a stop, and its word is the mode: 9, which the maker's alarm table (it starts at 16) cannot be taken for")
    set_reading(bridge, robot_mode=9, error_status=1, alarms=[17, 1537])
    rc, out = state(bridge)
    check(out.flags & rhal.FLAG_FAULT and not out.flags & rhal.FLAG_ESTOP and out.fault_code == 17, "with the alarm list read, the fault word is the first alarm, as the controller lists it")
    for alarm in (116, 132, 133):
        set_reading(bridge, robot_mode=9, error_status=1, alarms=[alarm])
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_ESTOP and out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_FAULT and out.fault_code == alarm, f"alarm {alarm}, which the maker's table describes as an emergency stop, is ESTOP and DEGRADED, and the fault word is the alarm")
    set_reading(bridge, robot_mode=9, error_status=1, alarms=[17, 132])
    check(state(bridge)[1].flags & rhal.FLAG_ESTOP and state(bridge)[1].fault_code == 17, "an emergency stop anywhere in the list is a stop, and the word is the first alarm")
    set_reading(bridge, robot_mode=5, error_status=1)
    rc, out = state(bridge)
    check(out.flags & rhal.FLAG_FAULT and out.fault_code == 9, "an error flag in the packet is a fault though the mode says enabled, and its word is 9")
    set_reading(bridge, robot_mode=11)
    rc, out = state(bridge)
    check(out.flags & rhal.FLAG_FAULT and out.flags & rhal.FLAG_DEGRADED and out.fault_code == 11, "mode 11 is a collision: a fault, and its word is 11")
    set_reading(bridge, robot_mode=5, collision_state=1)
    rc, out = state(bridge)
    check(out.flags & rhal.FLAG_FAULT and out.fault_code == 11, "and so is a collision flag in the packet, with the same word")
    set_reading(bridge, robot_mode=9, error_status=1, collision_state=1)
    check(state(bridge)[1].fault_code == 9, "an alarm and a collision together are the alarm: the word is 9")
    for unlisted in (0, 12, 99, 255):
        set_reading(bridge, robot_mode=unlisted)
        rc, out = state(bridge)
        check(rc == rhal.SKR_OK and out.flags & rhal.FLAG_FAULT and out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_ESTOP and out.fault_code == bridge_module.UNLISTED_MODE_BASE | unlisted,
              f"mode {unlisted}, which the interface document does not list, is a fault and not a healthy arm: an unknown state is not a good one, and its word is {bridge_module.UNLISTED_MODE_BASE} plus the mode, above any alarm of the maker's table")
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EPERM and f"mode {unlisted}, which the interface document does not list" in said, f"and a command to it is EPERM, saying so", said)
    check(bridge_module.UNLISTED_MODE_BASE == 65536 and bridge_module.UNLISTED_MODE_BASE > 4193, "(and the base is above the largest alarm identifier in the maker's table, 4193)")
    set_reading(bridge, robot_mode=5, safety_state=0x00)
    check(not state(bridge)[1].flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP), "the safety state byte is not read for a stop: its bits are low-active and no state of them was seen on a controller")
    set_reading(bridge)
    bridge._estop = True  # noqa: SLF001
    rc, out = state(bridge)
    check(out.flags & rhal.FLAG_ESTOP and out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_FAULT, "a latched stop is ESTOP and DEGRADED, and not a fault")
    bridge._estop = False  # noqa: SLF001

    section("the exchange thread — the alarm list, read when the arm is in alarm and not otherwise")
    sim = new_sim()
    try:
        bridge = opened(sim, rate_hz=100, stale_ms=30000)
        check(ready(bridge), "ready")
        sim.requests.clear()
        time.sleep(0.3)
        check(not any(r.lower().startswith("geterrorid") for r in sim.requests), "the alarm list is not asked for while the arm is well")
        sim.raise_alarm(17)
        check(until(lambda: state(bridge)[1].fault_code == 17 and state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "an alarm is read from the controller and is the fault word")
        asked = sum(1 for r in sim.requests if r.lower().startswith("geterrorid"))
        time.sleep(1.1)
        asked_later = sum(1 for r in sim.requests if r.lower().startswith("geterrorid")) - asked
        check(1 <= asked_later <= 4, f"and is read again about twice a second while it lasts, not at the packet rate ({asked_later} in 1.1 s)")
        sim.raise_alarm(132)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_ESTOP and state(bridge)[1].fault_code == 17, 3), "an emergency stop among the alarms is ESTOP")
        code, status = ext(bridge, "status")
        check(code == rhal.SKR_OK and status["alarms"] == [17, 132] and status["alarm"] is True and status["fault"] == {"class": "estop", "code": 17} and status["robot_mode"] == 9, "and `status` lists the alarms and the class", str(status))
        with sim._lock:  # noqa: SLF001
            sim._alarms = []  # noqa: SLF001
        check(until(lambda: not state(bridge)[1].flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP), 3), "an alarm that is gone is gone: nothing is remembered")
        check(ext(bridge, "status")[1]["alarms"] == [], "(and the list is empty again)")
        sim.collide()
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT and state(bridge)[1].fault_code == 11, 3), "a collision is a fault, and its word is the mode's 11")
        sim.set_mode_state(tcp=False)
        sim.raise_alarm(17)
        sim._collision = False  # noqa: SLF001
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "an alarm on a controller that takes no TCP is a fault all the same")
        sim.set_mode_state(tcp=True)
        bridge.close()
    finally:
        sim.stop()


def main_commands() -> None:
    section("write_command — a command is checked whole, and sent as one ServoJ")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        target = [0.1, -0.2, 0.3, 1.0, -1.5, 2.0]
        code, said = write(bridge, cmd(target))
        check(code == rhal.SKR_OK, "a command in range is accepted", said)
        check(until(lambda: len(sim.servo_calls) == 1, 2), "and is one ServoJ")
        called = sim.servo_calls[0]
        check(all(abs(called[1][j] - target[j] / DEG) < 1e-6 for j in range(6)) and called[2:] == (0.1, 50.0, 500.0), "whose angles are the command's in degrees, and whose other parameters are the controller's own defaults, because none was configured")
        check(servo_texts(sim)[-1] == frames.servo_j([v / DEG for v in target]), "(and the text is exactly what the frame builder writes)")
        check(until(lambda: max(abs(a - b / DEG) for a, b in zip(sim.joints_deg, target)) < 0.05, 3), "and the arm follows it")
        check(bridge._goal is not None and all(abs(bridge._goal[j] - target[j] / DEG) < 1e-9 for j in range(6)), "and the driver remembers what it asked for")  # noqa: SLF001
        sim.servo_calls.clear()
        code, said = write(bridge, cmd(pose(0.0), [1, 0, 0, 0, 0, 1]))
        check(code == rhal.SKR_OK and until(lambda: len(sim.servo_calls) == 1, 2) and abs(sim.servo_calls[0][1][0]) < 1e-9 and abs(sim.servo_calls[0][1][5]) < 1e-9, "a masked command sends the joints it applies at their targets")
        check(all(abs(sim.servo_calls[0][1][j] - target[j] / DEG) < 1e-6 for j in (1, 2, 3, 4)), "and the others where the last command put them, and not limp, and not at zero")
        sim.servo_calls.clear()
        code, said = write(bridge, cmd(pose(0.5), [0] * 6))
        time.sleep(0.1)
        check(code == rhal.SKR_OK and not sim.servo_calls, "a command that applies to no joint is OK and sends nothing")
        sim.servo_calls.clear()
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=pose(0.2), mask=[])
        code, said = write(bridge, command)
        check(code == rhal.SKR_OK and until(lambda: len(sim.servo_calls) == 1, 2) and all(abs(v - 0.2 / DEG) < 1e-6 for v in sim.servo_calls[0][1]), "a command with no mask applies to every joint")
        bridge.close()
        sim.place((12.0, -10.0, 5.0, 0.0, 0.0, 0.0))
        sim.servo_calls.clear()
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, said = write(bridge, cmd(pose(0.0), [0, 0, 0, 0, 0, 1]))
        check(code == rhal.SKR_OK and until(lambda: len(sim.servo_calls) == 1, 2) and all(abs(sim.servo_calls[0][1][j] - sim.joints_deg[j]) < 1.0 for j in range(5)), "the first command of a session that applies one joint holds the others where the arm is")
        bridge.close()
        bridge = opened(sim, servo={"t": 0.05, "aheadtime": 40, "gain": 800})
        check(ready(bridge), "ready")
        sim.servo_calls.clear()
        write(bridge, cmd(pose(0.1)))
        check(until(lambda: len(sim.servo_calls) == 1, 2) and sim.servo_calls[0][2:] == (0.05, 40.0, 800.0) and servo_texts(sim)[-1].endswith(",t=0.050,aheadtime=40.0,gain=800.0)"), "the operator's `servo` parameters are sent, in the order the document gives them")
        bridge.close()
        bridge = opened(sim, servo={"gain": 250})
        sim.servo_calls.clear()
        write(bridge, cmd(pose(0.1)))
        check(until(lambda: len(sim.servo_calls) == 1, 2) and sim.servo_calls[0][2:] == (0.1, 50.0, 250.0) and servo_texts(sim)[-1].endswith(",gain=250.0)") and "t=" not in servo_texts(sim)[-1], "and only the ones that were set")
        bridge.close()

    finally:
        sim.stop()
    section("write_command — what it refuses, whole, and without touching the arm")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.requests.clear()
        sim.servo_calls.clear()
        code, said = write(bridge, cmd([0.0] * 5))
        check(code == rhal.SKR_EINVAL and "5 joints" in said and "six" in said, "a command with five joints is refused, saying this body has six", said)
        code, said = write(bridge, cmd(pose(0.0), mode=rhal.MODE_VELOCITY))
        check(code == rhal.SKR_EINVAL and "positions" in said, "a velocity command is refused: there is no velocity route", said)
        for bad, word in ((math.nan, "not finite"), (math.inf, "not finite")):
            code, said = write(bridge, cmd([0.0, 0.0, bad, 0.0, 0.0, 0.0]))
            check(code == rhal.SKR_EINVAL and "j3" in said and word in said and "whole" in said, f"a joint commanded to {bad} is refused whole, naming the joint", said)
        for joint, (low, high) in enumerate(LIMITS):
            for value in (low - 0.01, high + 0.01):
                values = [0.0] * 6
                values[joint] = value
                code, said = write(bridge, cmd(values))
                check(code == rhal.SKR_EINVAL and f"j{joint + 1}" in said and "outside its range" in said and f"[{low:.6f}, {high:.6f}]" in said, f"j{joint + 1} at {value} rad is outside its range [{low}, {high}] and is refused whole, saying what the range is", said)
        code, said = write(bridge, cmd([0.0, 99.0, 0.0, 0.0, 0.0, 0.0], [1, 0, 1, 1, 1, 1]))
        check(code == rhal.SKR_OK, "a joint that is out of range but not applied does not refuse the command")
        sim.servo_calls.clear()
        code, said = write(bridge, cmd([0.0, 0.0, 0.0, 0.0, 0.0, 99.0]))
        check(code == rhal.SKR_EINVAL and not sim.servo_calls, "and one that is applied refuses it, and nothing is sent")
        code, said = write(bridge, cmd([LIMITS[0][1] + 5e-10, LIMITS[1][0] - 5e-10, 0.0, 0.0, 0.0, 0.0]))
        check(code == rhal.SKR_OK and until(lambda: sim.servo_calls and abs(sim.servo_calls[-1][1][0] - LIMITS[0][1] / DEG) < 1e-6 and abs(sim.servo_calls[-1][1][1] - LIMITS[1][0] / DEG) < 1e-6, 2), "an angle a hair outside the range, within the tolerance of rounding, is brought to the range's edge and sent, at either end")
        code, said = write(bridge, cmd([LIMITS[0][1], LIMITS[1][0], LIMITS[2][1], 0.0, 0.0, 0.0]))
        check(code == rhal.SKR_OK, "and the edges themselves are in")
        bridge.close()

        def raw(command: rhal.Command) -> tuple[int, str]:
            said_io = io.StringIO()
            with contextlib.redirect_stderr(said_io):
                code = bridge.write_command(command, 0)
            return code, said_io.getvalue()

        bridge = opened(sim)
        check(ready(bridge), "ready")
        too_wide, fine = cmd([0.0] * 5 + [99.0]), cmd(pose(0.1))
        first, again = raw(too_wide), raw(too_wide)
        check(first[0] == rhal.SKR_EINVAL and "outside its range" in first[1] and again[0] == rhal.SKR_EINVAL and again[1] == "", "a refusal is said once for a run of the same cause")
        check(raw(fine)[0] == rhal.SKR_OK and "outside its range" in raw(too_wide)[1], "and is said again when a command has been taken in between")
        bridge.close()
        check(raw(fine)[0] == rhal.SKR_EPERM, "a command on a driver that is closed is EPERM")
        closed = DobotBridge()
        check(closed.write_command(fine, 0) == rhal.SKR_EPERM, "and on one that was never opened")
    finally:
        sim.stop()
    section("write_command — the arm's state decides, and says which motion changes it")
    sim = new_sim()
    try:
        bridge = opened(sim, rate_hz=100)
        check(ready(bridge), "ready")
        sim.servo_calls.clear()
        for what, change, wanted in (("disabled", lambda: sim.set_mode_state(enabled=False), "disabled"), ("powered off", lambda: sim.set_mode_state(powered=False), "powered off")):
            change()
            check(until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 2), f"an arm that is {what} is DEGRADED")
            code, said = write(bridge, cmd(pose(0.1)))
            check(code == rhal.SKR_EPERM and wanted in said and ("`power_on`" if what == "powered off" else "`enable`") in said and not sim.servo_calls, f"a command to it is EPERM, says it is {what}, and names the motion that changes it", said)
            sim.set_mode_state(powered=True, enabled=True)
            check(ready(bridge, 3), "and it is taken again when the arm is back")
        sim.raise_alarm(17)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT and state(bridge)[1].fault_code == 17, 3), "an alarm")
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EPERM and "alarm 17" in said and "clear_errors" in said and not sim.servo_calls, "a command to an arm in alarm is EPERM, names the alarm and the way out", said)
        with sim._lock:  # noqa: SLF001
            sim._alarms = []  # noqa: SLF001
        sim.set_mode_state(enabled=True)
        check(ready(bridge, 3), "and is taken when it is cleared")
        sim.raise_alarm(132)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_ESTOP, 3), "an emergency stop")
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EPERM and "emergency stop" in said and "alarm 132" in said, "a command to it is EPERM, and says it is a stop", said)
        with sim._lock:  # noqa: SLF001
            sim._alarms = []  # noqa: SLF001
        sim.set_mode_state(enabled=True)
        check(ready(bridge, 3), "ready again")
        sim.collide()
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "a collision")
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EPERM and "collision detected" in said, "a command to an arm that has collided is EPERM, and says so", said)
        sim._collision = False  # noqa: SLF001
        check(ready(bridge, 3), "ready again")
        sim.set_mode_state(tcp=False)
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EPERM and "not TCP" in said and bridge._tcp is False, "a controller that has stopped taking TCP is noticed by the command, and the command is EPERM", said)  # noqa: SLF001
        code, body = ext(bridge, "status")
        check(code == rhal.SKR_OK and body["tcp_control"] is False, "and `status` says so")
        sim.set_mode_state(tcp=True)
        code, body = ext(bridge, "request_control")
        check(code == rhal.SKR_OK and bridge._tcp is True, "until control is asked for again")  # noqa: SLF001
        bridge.close()

        bridge = opened(sim, rate_hz=100)
        check(ready(bridge), "ready")
        for error, code_wanted, word in ((-1, rhal.SKR_EIO, "failed"), (-2, rhal.SKR_EPERM, "alarm"), (-3, rhal.SKR_EPERM, "emergency stop"), (-4, rhal.SKR_EPERM, "powered off"), (-5, rhal.SKR_EPERM, "script"), (-7, rhal.SKR_EPERM, "paused script"),
                                         (-8, rhal.SKR_EPERM, "authorisation"), (-9, rhal.SKR_EPERM, "disabled"), (-6, rhal.SKR_EINVAL, "jog axis"), (-20000, rhal.SKR_EINVAL, "number of parameters"), (-30003, rhal.SKR_EINVAL, "wrong type (parameter 3)"),
                                         (-40001, rhal.SKR_EINVAL, "out of range (parameter 1)"), (-60001, rhal.SKR_EINVAL, "optional parameter is out of range"), (-10000, rhal.SKR_EIO, "does not exist"), (-1234, rhal.SKR_EIO, "does not list")):
            sim.error_next = [error]
            code, said = write(bridge, cmd(pose(0.1)))
            check(code == code_wanted and f"error {error}" in said and word in said, f"an ErrorID of {error} is {'EIO' if code_wanted == rhal.SKR_EIO else 'EPERM' if code_wanted == rhal.SKR_EPERM else 'EINVAL'}, and says what the document says it is", said)
        sim.garble = True
        code, said = write(bridge, cmd(pose(0.1)))
        sim.garble = False
        check(code == rhal.SKR_EIO and "not one the interface document allows" in said, "a reply that is damaged is EIO: not an invalid command, and it does not escape", said)
        reopened: list[float] = []
        real_open = link.Dashboard.open

        def counting(self, timeout=1.0):
            reopened.append(timeout)
            return real_open(self, timeout)

        sim.delay_s = 0.4
        with mock.patch.object(link.Dashboard, "open", counting):
            began = time.monotonic()
            code, said = write(bridge, cmd(pose(0.2)))
            took = time.monotonic() - began
            sim.delay_s = 0.0
            check(code == rhal.SKR_EIO and 0.1 <= took < 0.45 and "slow or dead line" in said, f"a command the controller does not answer in the transaction budget is EIO, a slow line and not a refusal ({took:.2f} s)", said)
            check(until(lambda: bool(reopened), 3) and reopened[0] == 0.12, "its line is closed, and the reading thread opens it again, with a connect timeout of the transaction budget")
        check(until(lambda: write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, 4), "and the next command is taken")
        owner = holding_the_dashboard(bridge, 0.5)
        began = time.monotonic()
        code, said = write(bridge, cmd(pose(0.1)))
        took = time.monotonic() - began
        owner.join()
        check(code == rhal.SKR_EIO and 0.2 <= took <= 0.4 and "slow or dead line" in said, f"a command that cannot have the line is EIO after its quarter of a second ({took:.2f} s), not when the line is free again (F299)", said)
        bridge.close()
    finally:
        sim.stop()


def main_stop() -> None:
    section("the stop — Stop(): latched, written by the reading thread, and not released by an arm that has not taken it")
    sim = new_sim(time_scale=1.0)
    try:
        sim.place((0.0,) * 6)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        write(bridge, cmd([1.0, 0.0, 0.0, 0.0, 0.0, 0.0]))
        check(until(lambda: sim.moving, 2), "the arm is moving")
        sim.requests.clear()
        began = time.monotonic()
        check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.02, "emergency_stop returns at once, without touching the line")
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2), "and the reading thread writes the stop")
        check([r for r in sim.requests if r != "RobotMode()" and not r.lower().startswith("geterrorid")] == ["Stop()"], "the controller's own: Stop(), and nothing else")
        check(until(lambda: not sim.moving, 2) and sim.joints_deg[0] < 57.0, "the arm stopped where it was")
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_ESTOP, 1), "the state says the stop is latched")
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EPERM and said == "", "a command while latched is EPERM at once, and says nothing: the latch is the answer, and nothing else is looked at")
        status = ext(bridge, "status")
        check(status[0] == rhal.SKR_OK and status[1]["stop_latched"] is True and status[1]["stop_delivered"] is True and status[1]["stop_mode"] == "stop", "a status is still answered, and says the stop was delivered")
        check(ext(bridge, "move j1=0.1")[0] == rhal.SKR_EPERM and "stop is latched" in ext(bridge, "move j1=0.1")[1] and ext(bridge, "enable")[0] == rhal.SKR_EPERM, "a motion is refused, and says why")
        check(bridge.release_stop() == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_ESTOP, "release_stop clears the latch once a reading shows the arm not moving")
        status = ext(bridge, "status")[1]
        check(status["stop_latched"] is False and status["stop_delivered"] is False and bridge._goal is None, "and nothing of the stop is left in the status, and no goal is remembered from before it")  # noqa: SLF001
        check(write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, "and commands are accepted again")
        sim.requests.clear()
        bridge.emergency_stop()
        check(until(lambda: "Stop()" in sim.requests, 2), "a second stop is written again")
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2), "and delivered")
        sim.requests.clear()
        bridge.emergency_stop()
        check(until(lambda: "Stop()" in sim.requests, 2), "a second stop, called before the first was released, is written again: it is owed until it has been seen written")
        bridge.release_stop()
        bridge.close()
    finally:
        sim.stop()
    section("the stop — an arm that is slow to stop, a line that is down when it is called, and a command that is waiting")
    sim = new_sim(time_scale=1.0)
    try:
        sim.place((0.0,) * 6)
        bridge = opened(sim, rate_hz=100, stale_ms=300)
        check(ready(bridge), "ready")
        sim.drop = True
        began = time.monotonic()
        check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.02, "a stop with the dashboard down still returns OK within its budget: the latch is the stop (F298: the call is 20 ms at most)")
        check(ext(bridge, "status")[1]["stop_delivered"] is False, "and the status says it has not been delivered")
        said_release, said = said_by(bridge.release_stop)
        check(said_release == rhal.SKR_EIO and "stays latched" in said, "release_stop on an arm that has not taken the stop is EIO and the latch stays", said)
        check(write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_EPERM, "and a command is still refused")
        sim.drop = False
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 4), "the thread writes the stop as soon as the line carries it")
        check(bridge.release_stop() == rhal.SKR_OK, "and then the stop can be released")
        bridge.close()
        bridge = opened(sim, timeout_ms=300)
        check(ready(bridge), "ready")
        sim.requests.clear()
        owner = holding_the_dashboard(bridge, 0.15)
        results: list[int] = []
        sender = threading.Thread(target=lambda: results.append(write(bridge, cmd(pose(0.1)))[0]))
        sender.start()
        time.sleep(0.05)
        bridge.emergency_stop()
        owner.join()
        sender.join(3.0)
        until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3)
        check(results == [rhal.SKR_EPERM] and not [r for r in sim.requests if r.startswith("ServoJ")], f"a command that was waiting for the line when the stop was called is refused, and never writes its ServoJ after the stop ({results})")
        bridge.release_stop()
        bridge.close()
        for stopped, wanted in ((False, rhal.SKR_EIO), (True, rhal.SKR_EPERM)):
            bridge = opened(sim, timeout_ms=300)
            check(ready(bridge), "ready")
            owner = holding_the_dashboard(bridge, 0.5)
            results = []
            sender = threading.Thread(target=lambda: results.append(write(bridge, cmd(pose(0.1)))[0]))
            sender.start()
            time.sleep(0.05)
            if stopped:
                bridge.emergency_stop()
            sender.join(3.0)
            owner.join()
            check(results == [wanted], f"a command that ran out of time waiting for a busy line is {'refused by the stop that was called while it waited' if stopped else 'a slow line'} ({results}, wanted {[wanted]})")
            if stopped:
                bridge.release_stop()
            bridge.close()
        real_command = DobotBridge._command
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.requests.clear()

        def latching_command(self, text, budget=None, by=None, **kw):
            reply = real_command(self, text, budget, by, **kw)
            if text.startswith("ServoJ"):
                self._estop = True                  # a stop called after the command was written and before it was answered
            return reply

        with mock.patch.object(DobotBridge, "_command", latching_command):
            code, said = write(bridge, cmd(pose(0.2)))
        check(code == rhal.SKR_EPERM and "latched while the command was being made" in said, "a stop called while a command was in flight makes the command a refusal, whatever the controller answered", said)
        bridge._estop = False  # noqa: SLF001
        bridge.close()
    finally:
        sim.stop()
    section("the stop — the software emergency stop: pressed, delivered, released, cleared, and the arm left disabled")
    sim = new_sim(time_scale=1.0)
    try:
        bridge = opened(sim, stop_mode="emergency")
        check(ready(bridge), "ready")
        sim.requests.clear()
        bridge.emergency_stop()
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2) and "EmergencyStop(1)" in sim.requests, "in `emergency` mode the stop is EmergencyStop(1)")
        check(sim.estop_pressed and not sim.enabled and 132 in sim.alarms, "which disables the arm and raises the controller's alarm 132, the software emergency stop")
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_ESTOP and state(bridge)[1].fault_code == 132, 3), "and the state is ESTOP with the alarm as its word")
        sim.requests.clear()
        check(bridge.release_stop() == rhal.SKR_OK, "release_stop releases it")
        check([r for r in sim.requests if r.startswith(("EmergencyStop", "ClearError"))] == ["EmergencyStop(0)", "ClearError()"], "with EmergencyStop(0) and then ClearError(), in that order")
        check(not sim.estop_pressed and sim.alarms == [] and not sim.enabled, "the arm is no longer in alarm, and is left disabled: enabling it is a person's")
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and state(bridge)[1].flags & rhal.FLAG_DEGRADED and not state(bridge)[1].flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP), 3), "and its state is DEGRADED and no longer a stop")
        code, body = ext(bridge, "enable")
        check(code == rhal.SKR_OK and body["result"] == "enabled" and write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, "`enable` then enables it, and a command is taken")
        bridge.emergency_stop()
        until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2)
        real_estop = sim._cmd_emergencystop  # noqa: SLF001
        sim._cmd_emergencystop = lambda args: (-1, "") if args == ["0"] else real_estop(args)  # type: ignore[method-assign]  # noqa: SLF001 - a controller that will not release it
        said_release, said = said_by(bridge.release_stop)
        del sim._cmd_emergencystop  # noqa: SLF001
        check(said_release == rhal.SKR_EIO and "could not be released" in said and bridge._estop, "a software stop that the controller will not release stays latched, and says so", said)  # noqa: SLF001
        check(bridge.release_stop() == rhal.SKR_OK, "and is released when it will")
        bridge.close()
        with sim._lock:  # noqa: SLF001
            sim._alarms, sim._estop = [], False  # noqa: SLF001
        sim.set_mode_state(tcp=False, enabled=True)
        bridge = opened(sim)
        sim.requests.clear()
        bridge.emergency_stop()
        time.sleep(0.4)
        check(ext(bridge, "status")[1]["stop_delivered"] is False and ext(bridge, "status")[1]["stop_latched"] is True and bridge._stop_retry_at > 0, "on a controller that takes no TCP a stop cannot be delivered, and the status says it was not")  # noqa: SLF001
        check(sum(1 for r in sim.requests if r == "Stop()") <= 12, f"and the attempt is not made at the packet rate ({sum(1 for r in sim.requests if r == 'Stop()')} tries in 0.4 s)")
        said_release, said = said_by(bridge.release_stop)
        check(said_release == rhal.SKR_EIO and "stays latched" in said, "and it cannot be released", said)
        sim.set_mode_state(tcp=True)
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3) and bridge.release_stop() == rhal.SKR_OK, "until control is back")
        bridge.close()
        check(DobotBridge().release_stop() == rhal.SKR_OK, "a stop released on a driver that was never opened is OK")
        latched = DobotBridge()
        latched._estop = True  # noqa: SLF001
        check(latched.release_stop() == rhal.SKR_OK and latched._estop is False, "and leaves no latch")  # noqa: SLF001
    finally:
        sim.stop()


def main_motions() -> None:
    section("extension — the request itself")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, body = bridge.extension("vendor.other.task", b"status")
        check(code == rhal.SKR_ENOSYS and b"vendor.dobot.task" in body, "another vendor's token is ENOSYS and names the one this driver serves")
        code, body = bridge.extension(TOKEN, b"\xff\xfe")
        check(code == rhal.SKR_EINVAL and b"UTF-8" in body, "a request that is not UTF-8 is EINVAL")
        code, body = bridge.extension(TOKEN, b"   ")
        check(code == rhal.SKR_EINVAL and b"empty" in body and b"status" in body, "an empty one is EINVAL and lists the motions")
        code, body = bridge.extension(TOKEN, b"dance")
        check(code == rhal.SKR_EINVAL and b"unknown motion" in body, "an unknown motion is EINVAL")
        for text, word in (("status loud", b"malformed"), ("status =5", b"malformed"), ("status loud=1", b"does not take"), ("enable x=1", b"does not take"), ("disable x=1", b"does not take"), ("power_on x=1", b"does not take"), ("request_control x=1", b"does not take"),
                           ("clear_errors x=1", b"does not take")):
            code, body = bridge.extension(TOKEN, text.encode())
            check(code == rhal.SKR_EINVAL and word in body, f"`{text}` is EINVAL: {word.decode()}")
        code, body = ext(bridge, "move width=0.05=3")
        check(code == rhal.SKR_EINVAL and "not a joint" in str(body), "an argument is split at its first =")
        for what, error, wanted in (("a line error", link.LinkError("the line is gone"), rhal.SKR_EIO), ("a request that does not fit the wire", frames.ProtocolError("not a reply"), rhal.SKR_EINVAL), ("a controller that is not in TCP mode", link.NotTcp("no"), rhal.SKR_EPERM)):
            with mock.patch.object(DobotBridge, "_motion_status", side_effect=error):
                code, body = ext(bridge, "status")
            check(code == wanted and str(error) in str(body), f"{what} in a motion is {'EIO' if wanted == rhal.SKR_EIO else 'EINVAL' if wanted == rhal.SKR_EINVAL else 'EPERM'} and says what it was, and does not escape")
        code, status = ext(bridge, "status")
        check(code == rhal.SKR_OK and status["model"] == "CR5" and status["simulator"] is True and status["robot_mode"] == 5 and status["robot_mode_name"] == "enabled and idle" and status["tcp_control"] is True and status["enabled"] is True, "`status` names the model, that it is a simulation, the mode in numbers and words, that control is TCP and that the arm is enabled")
        check(status["alarm"] is False and status["alarms"] == [] and status["fault"] is None and status["collision"] is False and status["running"] is False and status["robot_type"] == 5 and status["payload_kg_configured"] == 0.5, "no alarm, no fault, no collision, not running, the robot type, and the payload that was configured")
        check(status["link"] == f"127.0.0.1:{sim.dashboard_port}" and status["feedback"] == f"127.0.0.1:{sim.feedback_port}" and status["stop_latched"] is False and status["stop_mode"] == "stop" and status["link_dead"] is None and status["packets"] >= 1, "the two lines, no stop, the stop mode, a line that is alive and the packets read")
        check(len(status["position_rad"]) == 6 and len(status["position_deg"]) == 6 and status["goal_deg"] is None and status["temperatures"] == [30.0] * 6 and status["speed_factor"] == 100.0, "six positions in each unit, no goal yet, the temperatures and the controller's speed scaling")
        check(until(lambda: ext(bridge, "status")[1]["packets"] > status["packets"], 2), "(and the packets go on)")
        write(bridge, cmd([0.1, 0.0, 0.0, 0.0, 0.0, 0.0]))
        check(abs(ext(bridge, "status")[1]["goal_deg"][0] - 0.1 / DEG) < 1e-9, "after a command, the goal is what was asked, in degrees")
        bridge.close()
        code, body = bridge.extension(TOKEN, b"status")
        check(code == rhal.SKR_EPERM and b"not open" in body, "a request to a driver that is not open is EPERM")
        bridge._opened, bridge._dashboard = True, None  # noqa: SLF001
        check(bridge.extension(TOKEN, b"status")[0] == rhal.SKR_EPERM, "and to one that is open and has no line")
        bridge._opened = False  # noqa: SLF001
    finally:
        sim.stop()

    section("extension — power, control, enabling, clearing")
    sim = new_sim(powered=False, enabled=False, tcp_mode=False, time_scale=SCALE)
    try:
        bridge = opened(sim)
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3) and bridge._tcp is False, "an arm that is off and not under TCP control is opened for reading")  # noqa: SLF001
        code, body = ext(bridge, "enable")
        check(code == rhal.SKR_EPERM and "not TCP" in str(body), "`enable` without control is EPERM: the controller says so, and the driver says what to do")
        code, body = ext(bridge, "request_control")
        check(code == rhal.SKR_OK and body["result"] == "requested" and bridge._tcp is True and sim.tcp_mode, "`request_control` takes control")  # noqa: SLF001
        code, body = ext(bridge, "enable")
        check(code == rhal.SKR_EPERM and "powered off" in str(body) and "-4" in str(body), "`enable` on an arm that is powered off is refused by the controller (ErrorID -4), and the driver says so")
        code, body = ext(bridge, "power_on")
        check(code == rhal.SKR_OK and body["result"] == "requested" and "ten seconds" in body["note"] and until(lambda: sim.mode == frames.MODE_INIT or sim.mode == frames.MODE_DISABLED, 2), "`power_on` powers the arm, and says that it takes about ten seconds")
        sim.requests.clear()
        code, body = ext(bridge, "enable")
        check(code == rhal.SKR_EIO and "-1" in str(body), "`enable` while the controller is still coming up is refused by it (ErrorID -1)")
        check(until(lambda: sim.mode == frames.MODE_DISABLED, 8), "the arm comes up disabled")
        sim.requests.clear()
        code, body = ext(bridge, "enable")
        check(code == rhal.SKR_OK and body == {"result": "enabled", "payload_kg": 0.5} and "EnableRobot(0.500000)" in sim.requests and sim.load_kg == 0.5 and sim.enabled, "`enable` enables the arm with the operator's payload, to six decimals, and says what it told it")
        check(ext(bridge, "enable")[1] == {"result": "already_enabled"} and sum(1 for r in sim.requests if r.startswith("EnableRobot")) == 1, "and an arm that is enabled is not enabled again")
        code, body = ext(bridge, "disable")
        check(code == rhal.SKR_OK and body["result"] == "requested" and until(lambda: not sim.enabled, 2) and until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 2), "`disable` disables it")
        write_code, said = write(bridge, cmd(pose(0.1)))
        check(write_code == rhal.SKR_EPERM, "and it takes no command")
        code, body = ext(bridge, "enable")
        check(code == rhal.SKR_OK and body["result"] == "enabled", "until it is enabled again")
        sim.raise_alarm(17)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "an alarm")
        code, body = ext(bridge, "enable")
        check(code == rhal.SKR_EPERM and "-2" in str(body), "`enable` in alarm is refused by the controller (ErrorID -2)")
        code, body = ext(bridge, "clear_errors")
        check(code == rhal.SKR_OK and body["result"] == "cleared" and body["robot_mode"] == frames.MODE_DISABLED and "enable" in body["note"] and sim.alarms == [], "`clear_errors` clears the alarm and leaves the arm disabled, and says what to do next")
        sim.raise_alarm(17, persistent=True)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 3), "an alarm whose cause is still there")
        code, body = ext(bridge, "clear_errors")
        check(code == rhal.SKR_OK and body["result"] == "not_cleared" and body["robot_mode"] == frames.MODE_ERROR, "is `not_cleared`, and the mode says why")
        sim.silent_feedback = True
        check(until(lambda: bridge._dead is not None, 4), "the feedback goes quiet and the line is dead")  # noqa: SLF001
        code, body = ext(bridge, "clear_errors")
        check(code == rhal.SKR_EIO and "ClearError was accepted and no reading has shown what it did" in str(body), "a clear that no reading shows the result of, on a line that is dead, is EIO: the line did not say, and an alarm that is gone is not claimed")
        sim.silent_feedback = False
        sim._persistent.clear()  # noqa: SLF001
        with sim._lock:  # noqa: SLF001
            sim._alarms = []  # noqa: SLF001
        sim.set_mode_state(enabled=False)
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and bridge._latest is not None and not bridge._latest.enable_status and bridge._latest.robot_mode == frames.MODE_DISABLED, 6), "the feedback is back and the arm is disabled")  # noqa: SLF001
        sim.silent_feedback = True
        check(until(lambda: bridge._dead is not None, 4), "quiet again")  # noqa: SLF001
        code, body = ext(bridge, "enable")
        check(code == rhal.SKR_EIO and "EnableRobot was accepted and no reading has shown the arm enabled" in str(body), "and so is an enable that no reading shows")
        sim.silent_feedback = False
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(enabled=False)
    try:
        bridge = opened(sim)
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3), "ready")
        with mock.patch.object(DobotBridge, "_wait_for_packet", lambda self, predicate, timeout, **kw: frames.decode_feedback(packet_of(robot_mode=9, error_status=1))):
            code, body = ext(bridge, "enable")
        check(code == rhal.SKR_EIO and "went into its alarm state while it was being enabled" in str(body), "an arm that goes into alarm while it is being enabled is EIO, and says so")
        bridge.close()
    finally:
        sim.stop()

    section("extension — move: its bounds, its goal, its report and its time")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, body = ext(bridge, "move j1=0.5 j3=-0.4 timeout=5")
        check(code == rhal.SKR_OK and body["result"] == "arrived" and body["result_id"].isdigit() and abs(body["position_deg"][0] - 0.5 / DEG) < 0.5 and abs(body["position_deg"][2] + 0.4 / DEG) < 0.5, "a move of two joints waits for them and says they arrived, and where", str(body))
        check(abs(sim.joints_deg[1]) < 1e-6 and abs(sim.joints_deg[3]) < 1e-6 and abs(body["goal_deg"][1] - body["position_deg"][1]) < 0.5 and any(r.startswith("MovJ(joint={") for r in sim.requests), "and the others were not moved: their goals are where they were")
        code, body = ext(bridge, "move j2=0.2 v=50 timeout=5")
        check(code == rhal.SKR_OK and body["result"] == "arrived" and any(r.startswith("MovJ(") and r.endswith(",v=50)") for r in sim.requests), "a speed percentage is sent as the document's v")
        for text, word in (("move", "at least one joint"), ("move wrist=0.1", "not a joint"), ("move j1=abc", "not a number"), ("move j1=nan", "outside"), ("move j1=7.0", "outside"), ("move j2=3.1", "outside"), ("move j1=0.1 timeout=0.4", "outside"),
                           ("move j1=0.1 timeout=301", "outside"), ("move j1=0.1 timeout=x", "not a number"), ("move j1=0.1 v=0", "outside"), ("move j1=0.1 v=101", "outside"), ("move j1=0.1 v=2.5", "whole")):
            code, body = ext(bridge, text)
            check(code == rhal.SKR_EINVAL and word in str(body), f"`{text}` is EINVAL: {word}", str(body))
        for text in ("move j2=3.0 timeout=5", "move j2=-3.0 timeout=5", "move j1=6.0 timeout=5", "move j1=-6.0 timeout=5", "move j3=2.5 timeout=5"):
            code, body = ext(bridge, text)
            check(code == rhal.SKR_OK and body["result"] == "arrived", f"`{text}` is a move to the edge of the range, and the edge is inside it", str(body))
        ext(bridge, "move j1=0 j2=0 j3=0 timeout=5")
        sim.requests.clear()
        code, body = ext(bridge, "move j1=0.0 timeout=5")
        check(code == rhal.SKR_OK and body["result"] == "arrived" and any(r.startswith("MovJ(joint={") for r in sim.requests), "a move to where the arm already is has arrived")
        sim.set_mode_state(enabled=False)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 2)
        code, body = ext(bridge, "move j1=0.1")
        check(code == rhal.SKR_EPERM and "enable" in str(body), "a move on an arm that is not enabled is EPERM, naming the way out")
        sim.set_mode_state(enabled=True)
        ready(bridge, 3)
        sim.raise_alarm(17)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT, 3)
        code, body = ext(bridge, "move j1=0.1")
        check(code == rhal.SKR_EPERM and "alarm 17" in str(body), "and on one in alarm")
        with sim._lock:  # noqa: SLF001
            sim._alarms = []  # noqa: SLF001
        sim.set_mode_state(enabled=True)
        ready(bridge, 3)
        sim.error_next = [-40001]
        code, body = ext(bridge, "move j1=0.1")
        check(code == rhal.SKR_EINVAL and "-40001" in str(body), "a move the controller refuses for a parameter is EINVAL, and says what the document says")
        bridge.close()
    finally:
        sim.stop()
    slow = new_sim(time_scale=0.05)
    try:
        bridge = opened(slow)
        check(ready(bridge), "ready")
        code, body = ext(bridge, "move j1=1.5 timeout=0.6")
        check(code == rhal.SKR_EIO and "no reading showed" in str(body) and "never started" not in str(body), "a move that does not arrive in its timeout is EIO and says so", str(body))
        slow.place((0.0,) * 6)
        results: list[tuple[int, object]] = []
        mover = threading.Thread(target=lambda: results.append(ext(bridge, "move j1=1.5 timeout=60")))
        mover.start()
        check(until(lambda: slow.moving, 3), "a move is under way")
        began = time.monotonic()
        bridge.emergency_stop()
        mover.join(3.0)
        check(not mover.is_alive() and time.monotonic() - began < 1.5 and results and results[0][0] == rhal.SKR_EPERM and "stop was latched" in str(results[0][1]), "a stop ends a move that is waiting, and the move says why",
              f"{results} after {time.monotonic() - began:.2f} s, alive {mover.is_alive()}")
        bridge.release_stop()
        slow.place((0.0,) * 6)
        results.clear()
        mover = threading.Thread(target=lambda: results.append(ext(bridge, "move j1=1.5 timeout=60")))
        mover.start()
        check(until(lambda: slow.moving, 3), "another is under way")
        slow.raise_alarm(17)
        mover.join(5.0)
        check(not mover.is_alive() and results and results[0][0] == rhal.SKR_EIO and "went into an alarm during the move" in str(results[0][1]), "an alarm during a move ends it, EIO, and says what")
        with slow._lock:  # noqa: SLF001
            slow._alarms = []  # noqa: SLF001
        slow.set_mode_state(enabled=True)
        ready(bridge, 4)
        slow.place((0.0,) * 6)
        results.clear()
        mover = threading.Thread(target=lambda: results.append(ext(bridge, "move j1=1.5 timeout=60")))
        mover.start()
        until(lambda: slow.moving, 3)
        slow.collide()
        mover.join(5.0)
        check(not mover.is_alive() and results and results[0][0] == rhal.SKR_EIO and "went into a collision during the move" in str(results[0][1]), "and so does a collision", str(results))
        slow._collision = False  # noqa: SLF001
        ready(bridge, 4)
        slow.place((0.0,) * 6)
        results.clear()
        mover = threading.Thread(target=lambda: results.append(ext(bridge, "move j1=1.5 timeout=60")))
        mover.start()
        until(lambda: slow.moving, 3)
        began = time.monotonic()
        bridge.close()
        mover.join(2.0)
        check(not mover.is_alive() and time.monotonic() - began < 1.5 and results and results[0][0] == rhal.SKR_EPERM and "closed" in str(results[0][1]), "and when the driver is closed", str(results))
        code, body = ext(bridge, "move j1=0.1")
        check(code == rhal.SKR_EPERM, "a move on a driver that has been closed is refused at once")
    finally:
        slow.stop()
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim._cmd_movj = lambda args: (0, "9")  # type: ignore[method-assign]  # noqa: SLF001 - a controller that acknowledges a MovJ and never moves
        began = time.monotonic()
        code, body = ext(bridge, "move j1=0.3 timeout=0.7")
        took = time.monotonic() - began
        check(code == rhal.SKR_EIO and "never started moving" in str(body) and 0.6 <= took <= 1.3, f"a controller that acknowledges a MovJ and never moves is not a move that arrived: it times out, and says it never started ({took:.2f} s)", str(body))
        began = time.monotonic()
        code, body = ext(bridge, "move j1=0.3")
        took = time.monotonic() - began
        check(code == rhal.SKR_EIO and 6.2 <= took <= 7.5, f"a move with no timeout waits five seconds and a tenth of a second for each degree of its longest travel ({took:.1f} s for 17 degrees: 6.7)")
        bridge.close()
    finally:
        sim.stop()


def live_exchange_threads() -> int:
    return sum(1 for thread in threading.enumerate() if thread.name == "dobot-exchange" and thread.is_alive())


def main_session() -> None:
    section("the lines — a controller that goes quiet, hangs up and comes back, and a feedback that is wrong")
    sim = new_sim()
    try:
        bridge = opened(sim, rate_hz=100, stale_ms=300)
        check(ready(bridge), "ready")
        sim.silent_feedback = True
        began = time.monotonic()
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 3), "a controller whose feedback goes quiet is ETIMEDOUT")
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_FAULT, "and degraded, and not a fault")
        check(until(lambda: bridge._dead is not None, 3), "after three failed cycles the line is called dead")  # noqa: SLF001
        code, text = ext(bridge, "status")
        check(code == rhal.SKR_OK and text["link_dead"] and "no packet" in text["link_dead"], "and status says why", str(text))
        write_code, said = write(bridge, cmd(pose(0.1)))
        check(write_code in (rhal.SKR_OK, rhal.SKR_EPERM), "(a command is still judged by the last reading)")
        sim.silent_feedback = False
        check(ready(bridge, 6), "and it is read again when the feedback is back")
        opened_feedback: list[float] = []
        real_feedback_open = link.FeedbackStream.open

        def counting_feedback(self, timeout=1.0):
            opened_feedback.append(timeout)
            return real_feedback_open(self, timeout)

        with mock.patch.object(link.FeedbackStream, "open", counting_feedback):
            sim.hang_up()
            check(until(lambda: bool(opened_feedback), 3), "a controller that hangs up is seen, and the feedback is opened again")
            check(opened_feedback[0] == 0.12, f"with a connect timeout of the transaction budget ({opened_feedback[:1]})")
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 8), "and the arm is read again")
        check(until(lambda: write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, 8), "and commanded again (the dashboard is opened again too)")
        sim.stop_listening()
        sim.hang_up()
        check(until(lambda: bridge._dead is not None and "could not be opened" in bridge._dead, 5), "a controller that is not listening is reported as a line that could not be opened, not as the packet that failed last")  # noqa: SLF001
        sim.resume_listening()
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 8), "and is read again when it listens")
        sim.refuse_connections = True
        sim.hang_up()
        check(until(lambda: bridge._dead is not None and "closed the connection" in bridge._dead, 5) and "could not be opened" not in bridge._dead, f"a controller that takes the connection and drops it is reported as what it is, and not as the open that failed before ({bridge._dead!r})")  # noqa: SLF001
        sim.refuse_connections = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 8), "and is read again when it talks")
        sim.feedback_split = 700
        base = bridge._packets  # noqa: SLF001
        check(until(lambda: bridge._packets >= base + 5, 3) and bridge._dead is None, "a feedback that comes in two pieces is read")  # noqa: SLF001
        sim.feedback_split = 0
        sim.feedback_garble = True
        check(until(lambda: bridge._dead is not None, 5), "a feedback whose self-test field is wrong is a dead line")  # noqa: SLF001
        sim.feedback_garble = False
        check(ready(bridge, 8), "and the arm is read again when it is right")
        bridge.close()
        slow_budget = opened(sim, timeout_ms=2000)
        check(ready(slow_budget), "ready")
        opened_feedback.clear()
        with mock.patch.object(link.FeedbackStream, "open", counting_feedback):
            sim.hang_up()
            check(until(lambda: bool(opened_feedback), 3) and opened_feedback[0] == 0.5, f"and a connect is never given more than half a second ({opened_feedback[:1]})")
        check(ready(slow_budget, 8), "and the arm is read again")
        slow_budget.close()
    finally:
        sim.stop()

    section("open and close — a close ends the thread and the lines, and a session starts with nothing of the last one's")
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
        check(bridge._loop is None and bridge._latest is None and bridge._dashboard is None and bridge._feedback is None, "a closed driver holds no thread, no reading and no line")  # noqa: SLF001
        check(bridge.close() == rhal.SKR_OK and DobotBridge().close() == rhal.SKR_OK, "a close is OK, and so is a second one, and so is one of a driver that was never opened")
        bridge = opened(sim)
        check(ready(bridge), "ready")
        write(bridge, cmd(pose(0.1)))
        bridge.emergency_stop()
        until(lambda: ext(bridge, "status")[1]["stop_delivered"])
        text = config(sim)
        bridge.close()
        check(said_by(lambda: bridge.open(text))[0] == rhal.SKR_OK and ready(bridge), "the same driver opened again")
        status = ext(bridge, "status")[1]
        check(status["stop_latched"] is False and status["stop_delivered"] is False and status["goal_deg"] is None and not state(bridge)[1].flags & rhal.FLAG_ESTOP, "has no stop, none delivered, and no goal remembered")
        first, threads_before = bridge._loop, live_exchange_threads()  # noqa: SLF001
        check(said_by(lambda: bridge.open(text))[0] == rhal.SKR_OK and ready(bridge) and first is not None and not first.is_alive() and live_exchange_threads() == threads_before, "a driver opened again without being closed ends the reading thread it had: two threads do not share the lines")
        bridge.close()
        bridge._estop, bridge._alarms, bridge._tcp, bridge._goal, bridge._stop_delivered = True, [17], False, [1.0] * 6, True  # noqa: SLF001
        check(said_by(lambda: bridge.open(config(sim, model="CR3")))[0] == rhal.SKR_ENODEV and bridge.describe() == "{}", "a session that is refused describes nothing: not the arm of the session before it")
        check(bridge._estop is False and bridge._alarms == [] and bridge._tcp is None and bridge._goal is None and bridge._stop_delivered is False and bridge._latest is None, "and starts with no stop, no alarm, no knowledge of the control mode, no goal, nothing delivered and no reading")  # noqa: SLF001
        bridge.close()

        section("describe — the descriptor")
        check(DobotBridge().describe() == "{}", "a driver that is not open describes nothing")
        bridge = opened(sim, roles={"j1": "right_arm.shoulder.yaw", "j6": "right_arm.wrist.roll"}, rate_hz=50)
        descriptor = json.loads(bridge.describe())
        check(descriptor["robot_slug"] == "dobot-cr5" and descriptor["simulator"] is True and descriptor["base"] == "fixed" and descriptor["dof"] == 6 and descriptor["verified"] is False and descriptor["digest"] is None, "the descriptor names the arm, says it is a simulation, fixed, of six degrees of freedom, and unverified")
        joints = descriptor["joints"]
        check([j["name"] for j in joints] == ["j1", "j2", "j3", "j4", "j5", "j6"] and all(j["type"] == "revolute" and j["control_modes"] == ["position"] for j in joints), "six revolute joints in the controller's order, position-controlled")
        check([[j["limits"]["position_min_rad"], j["limits"]["position_max_rad"]] for j in joints] == [list(map(float, pair)) for pair in LIMITS] and all("velocity_max_rad_s" not in j["limits"] for j in joints), "with the operator's ranges, and no speed: nothing here says what the controller's limit is")
        check(joints[0]["role"] == "right_arm.shoulder.yaw" and joints[5]["role"] == "right_arm.wrist.roll" and joints[1]["role"] is None, "the roles the operator asserted, and none that were not")
        check(descriptor["end_effectors"] == [{"name": "flange", "role": None, "kind": "flange", "dof": 0, "max_grip_force_n": None, "max_opening_m": None}], "a flange, with no tool described")
        check(descriptor["envelope"] is None and descriptor["capabilities"] == ["safety.estop"] and descriptor["extensions"] == [TOKEN] and descriptor["control"] == {"modes": ["position"], "rate_hz_max": 50.0, "rate_hz_min": 1.0} and descriptor["sensors"] == [] and descriptor["solvers"] == [], "no envelope, an e-stop capability, one extension and a control rate")
        check([m["name"] for m in descriptor["motions"]] == ["status", "request_control", "power_on", "enable", "disable", "clear_errors", "move"] and [m["parameters"] for m in descriptor["motions"] if m["name"] != "move"] == [[]] * 6, "seven motions, six of which take nothing")
        move = [m for m in descriptor["motions"] if m["name"] == "move"][0]
        check([p["name"] for p in move["parameters"]] == ["j1", "j2", "j3", "j4", "j5", "j6", "v", "timeout"] and all(p["required"] is False for p in move["parameters"]) and [p["unit"] for p in move["parameters"]] == ["rad"] * 6 + ["%", "s"], "`move` takes any of the joints in radians, a speed in percent and a timeout in seconds, and none of them is required")
        bridge.close()
        for model, slug in (("Nova 2", "dobot-nova-2"), ("Nova 2 (new)", "dobot-nova-2-new"), ("CR20V", "dobot-cr20v")):
            sim.robot_type = TYPE_OF[model]
            bridge = opened(sim, model=model)
            check(json.loads(bridge.describe())["robot_slug"] == slug, f"{model} is {slug}")
            bridge.close()
        sim.robot_type = TYPE_OF["CR5"]
        bridge = opened(sim, simulator=False)
        check(json.loads(bridge.describe())["simulator"] is False, "a driver told it is not a simulation says so")
        check(bridge.read_sensor("camera", rhal.Frame()) == rhal.SKR_ENOSYS, "there is no sensor to read")
        bridge.close()
        stuck, probe = threading.Event(), DobotBridge()
        probe._loop = threading.Thread(target=lambda: stuck.wait(6.0), daemon=True, name="stuck-exchange")  # noqa: SLF001
        probe._loop.start()  # noqa: SLF001
        began = time.monotonic()
        probe._teardown()  # noqa: SLF001
        took = time.monotonic() - began
        stuck.set()
        check(1.5 <= took <= 3.5 and probe._loop is None, f"a teardown gives a reading thread that is stuck in a transaction two seconds and then lets it go ({took:.1f} s)")  # noqa: SLF001
    finally:
        sim.stop()

    section("the budget and the waits, one at a time")
    bridge = lifeless_bridge()
    check(bridge._timeout == 0.08, "a transaction is given 80 ms unless the operator says otherwise")  # noqa: SLF001
    with bridge._lock:  # noqa: SLF001
        bridge._latest = None  # noqa: SLF001
    check("no reading showed" in (raises(bridge_module.BridgeRefusal, bridge._wait_for_packet, lambda r: True, 0.3) or ""), "a wait is for a reading taken after it began")  # noqa: SLF001
    bridge._dead = "the line is gone"  # noqa: SLF001
    check("the line is gone" in (raises(bridge_module.BridgeRefusal, bridge._wait_for_packet, lambda r: False, 0.2) or ""), "and a wait that times out on a dead line says that the line is dead, and why")  # noqa: SLF001
    bridge._dead = None  # noqa: SLF001
    check("the line is answering" in (raises(bridge_module.BridgeRefusal, bridge._wait_for_packet, lambda r: False, 0.2) or ""), "and on one that is answering, that it is")  # noqa: SLF001
    bridge._closing.set()  # noqa: SLF001
    check("closed" in (raises(bridge_module.BridgeRefusal, bridge._wait_for_packet, lambda r: True, 0.5) or ""), "a wait on a driver that is closing ends, and says so")  # noqa: SLF001
    bridge._closing.clear()  # noqa: SLF001
    bumper = threading.Timer(0.1, lambda: bridge._closing.set())  # noqa: SLF001
    bumper.start()
    began = time.monotonic()
    raises(bridge_module.BridgeRefusal, bridge._wait_for_packet, lambda r: False, 2.0)  # noqa: SLF001
    took = time.monotonic() - began
    bumper.join()
    check(took < 0.5, f"a wait that is not notified looks again within 50 ms and not when its whole time has gone ({took:.2f} s of 2.0 s)")
    bridge._closing.clear()  # noqa: SLF001
    bridge._estop = True  # noqa: SLF001
    check("stop was latched" in (raises(bridge_module.BridgeRefusal, bridge._wait_for_packet, lambda r: True, 0.5) or ""), "a wait ends when a stop is latched")  # noqa: SLF001
    began = time.monotonic()
    message = raises(bridge_module.BridgeRefusal, lambda: bridge._wait_for_packet(lambda r: False, 0.3, honour_stop=False)) or ""  # noqa: SLF001
    check("no reading showed" in message and time.monotonic() - began >= 0.25, "unless it is the wait for the stop itself, which waits out its time")
    bridge._estop = False  # noqa: SLF001
    bridge2 = lifeless_bridge()
    bridge2._dashboard = None  # noqa: SLF001
    check("not open" in (raises(link.LinkError, bridge2._command, frames.robot_mode()) or ""), "a command on a driver with no dashboard is a LinkError, not an AttributeError")  # noqa: SLF001
    bridge3 = DobotBridge()
    bridge3._dashboard = link.Dashboard("127.0.0.1", 1)  # noqa: SLF001
    owner = holding_the_dashboard(bridge3, 0.4)
    began = time.monotonic()
    message = raises(link.LinkError, bridge3._command, frames.robot_mode(), None, time.monotonic() + 0.1) or ""  # noqa: SLF001
    took = time.monotonic() - began
    owner.join()
    check("in use" in message and took < 0.3, f"a command waits for the dashboard's lock only as long as its caller was given ({took:.2f} s)", message)
    check("ran out before the dashboard was free" in (raises(link.LinkError, bridge3._command, frames.robot_mode(), None, time.monotonic() - 1.0) or ""), "and not at all when its time has run out")  # noqa: SLF001
    bridge3._estop = True  # noqa: SLF001
    check("stop was latched before the command was written" in (raises(link.LinkError, lambda: bridge3._command(frames.robot_mode(), None, time.monotonic() + 1.0, unless_stopped=True)) or "") and bridge3._dashboard_lock.acquire(blocking=False), "a command that is not to be written once a stop is latched is not, and lets the line go")  # noqa: SLF001
    bridge3._dashboard_lock.release()
    check(bridge_module._code_of(-2) == rhal.SKR_EPERM and bridge_module._code_of(-9) == rhal.SKR_EPERM and bridge_module._code_of(-6) == rhal.SKR_EINVAL and bridge_module._code_of(-20000) == rhal.SKR_EINVAL  # noqa: SLF001
          and bridge_module._code_of(-1) == rhal.SKR_EIO and bridge_module._code_of(-10000) == rhal.SKR_EIO and bridge_module._code_of(-60002) == rhal.SKR_EINVAL, "an ErrorID is mapped to the RHAL code that says what to do about it: a state that refuses is EPERM, a parameter that is wrong is EINVAL, and a command that failed is EIO")


# ── survivors begin (assembled from the seeded-defect campaign's first run) ──
def survivors_links() -> None:
    """What a seeded-defect campaign found the suite let through, one at a time (the survivors of its first run)."""
    section("the dashboard — a reply to a command the controller does not know, a lower-case control-mode sentence, and replies that share a line")

    def unknown_command(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.recv(1024)
        connection.sendall(b"-10000,{},NotWhatWasSent();\n")
        time.sleep(0.3)

    port, thread = tcp_server(unknown_command)
    dashboard = link.Dashboard("127.0.0.1", port)
    dashboard.open()
    began = time.monotonic()
    reply = None
    with contextlib.suppress(link.LinkError):
        reply = dashboard.command("Bogus()", 1.0)
    check(reply is not None and reply.error == -10000 and time.monotonic() - began < 0.5, "a reply of -10000 is the answer to whatever was sent, whatever it echoes: the controller says what it heard, which is not what was sent")
    dashboard.close()
    thread.join(3.0)

    def shouting(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.recv(1024)
        connection.sendall(b"CONTROL MODE IS NOT TCP\n")
        time.sleep(0.3)

    port, thread = tcp_server(shouting)
    dashboard = link.Dashboard("127.0.0.1", port)
    dashboard.open()
    check("not TCP" in (raises(link.NotTcp, dashboard.command, frames.robot_mode(), 1.0) or ""), "the controller's sentence about its control mode is known in capitals too: a NotTcp, and not a reply that cannot be parsed")
    dashboard.close()
    thread.join(3.0)

    def joined(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.recv(1024)
        connection.sendall(b"0,{},Stop();0,{4},RobotMode();\n")          # two replies, the first ending at its semicolon, and one newline at the very end
        time.sleep(0.3)

    port, thread = tcp_server(joined)
    dashboard = link.Dashboard("127.0.0.1", port)
    dashboard.open()
    reply = None
    with contextlib.suppress(link.LinkError, frames.ProtocolError):
        reply = dashboard.command(frames.robot_mode(), 1.0)
    check(reply is not None and reply.payload == "4", "a reply ends at the first terminator it has, a semicolon or a newline: two replies on one line are two replies, and the second is the one that was asked for")
    dashboard.close()
    thread.join(3.0)

    def stale_flood(connection: socket.socket) -> None:
        connection.sendall(b"0,{},RobotMode();\n" * 700)                 # twelve thousand bytes of replies nobody asked for, more than one read of the discard takes
        sent.set()
        connection.settimeout(3.0)
        connection.recv(1024)
        connection.sendall(b"0,{4},RobotMode();\n")
        time.sleep(0.3)

    sent = threading.Event()
    port, thread = tcp_server(stale_flood)
    dashboard = link.Dashboard("127.0.0.1", port)
    dashboard.open()
    sent.wait(2.0)
    time.sleep(0.2)
    check(dashboard.command(frames.robot_mode(), 1.0).payload == "4", "and all of what was waiting is thrown away, not the first few thousand bytes of it")
    dashboard.close()
    thread.join(3.0)

    section("a line opened again lets go of the one it had")

    class Closeable:
        def __init__(self) -> None:
            self.closed = False

        def setsockopt(self, *args) -> None:
            pass

        def close(self) -> None:
            self.closed = True

    first, second = Closeable(), Closeable()
    for line in (link.Dashboard("gateway.example", 29999), link.FeedbackStream("gateway.example", 30004)):
        first, second = Closeable(), Closeable()
        with mock.patch.object(socket, "create_connection", side_effect=[first, second]):
            line.open()
            line.open()
        check(first.closed and not second.closed, f"a {type(line).__name__.lower()} opened again closes the connection it had, and keeps the new one")

    section("the feedback — what is left of a packet that stopped, junk that looks a little like a packet, and a stream that has lost its place by an odd number of bytes")
    packets = [packet_of(timestamp_ms=1), packet_of(timestamp_ms=2)]
    go_on = threading.Event()

    def stops_then_goes_on(connection: socket.socket) -> None:
        connection.sendall(packets[0][:800])
        go_on.wait(3.0)
        connection.sendall(packets[1])
        time.sleep(0.5)

    port, thread = tcp_server(stops_then_goes_on)
    feedback = link.FeedbackStream("127.0.0.1", port)
    feedback.open()
    message = ""
    try:
        for _ in range(40):
            feedback.read_packet(0.1)
    except link.LinkError as error:
        message = str(error)
    go_on.set()
    got = feedback.read_packet(1.0)
    check("stopped sending 800 bytes" in message and got == packets[1], "after a packet that stopped half way, the half is forgotten: the next packet is read whole and is not the half with the start of the next on its tail")
    feedback.close()
    thread.join(3.0)

    wrong_magic = struct.pack("<H", 1440) + b"\x00" * 1438
    wrong_size = struct.pack("<H", 1000) + b"\x00" * 46 + struct.pack("<Q", frames.FEEDBACK_MAGIC) + b"\x00" * (1440 - 56)

    def lookalikes(connection: socket.socket) -> None:
        connection.sendall(packets[0] + wrong_magic + wrong_size)          # the real one first: the newest packet is what is read, so lookalikes behind it are the ones that could be taken
        time.sleep(0.5)

    port, thread = tcp_server(lookalikes)
    feedback = link.FeedbackStream("127.0.0.1", port)
    feedback.open()
    time.sleep(0.1)
    check(feedback.read_packet(1.0) == packets[0], "a packet is 1440 bytes that say they are, with the self-test field where it belongs: neither the length alone nor the self-test alone will do")
    feedback.close()
    thread.join(3.0)

    def off_by_one(connection: socket.socket) -> None:
        connection.sendall(b"\x01" * 301 + packets[0] + packets[1])
        time.sleep(0.5)

    port, thread = tcp_server(off_by_one)
    feedback = link.FeedbackStream("127.0.0.1", port)
    feedback.open()
    time.sleep(0.1)
    check(feedback.read_packet(1.0) == packets[1], "a stream that has lost its place by an odd number of bytes finds it again, one byte at a time")
    feedback.close()
    thread.join(3.0)

def publishing(bridge: DobotBridge, readings: list[dict], gap: float = 0.03) -> threading.Thread:
    """A thread that gives a lifeless bridge one reading after another, as the reading thread would: each is the latest, counted, and anyone waiting is told."""
    def publish() -> None:
        for fields in readings:
            time.sleep(gap)
            base = dict(robot_mode=5, enable_status=1, brake_status=0x3F, error_status=0, running_status=0, collision_state=0, robot_type=5, safety_state=0xFF)
            base.update(fields)
            reading = frames.decode_feedback(packet_of(**base))
            with bridge._changed:  # noqa: SLF001
                bridge._latest, bridge._stamp_ns, bridge._dead = reading, rhal.acquisition_ns(), None  # noqa: SLF001
                bridge._packets += 1  # noqa: SLF001
                bridge._changed.notify_all()  # noqa: SLF001

    thread = threading.Thread(target=publish, daemon=True)
    thread.start()
    return thread


def survivors_stop() -> None:
    section("the stop — written at once, by a thread that waits for it, and what a stop leaves behind")
    probe = DobotBridge()
    probe._open_wake()  # noqa: SLF001
    probe.emergency_stop()
    try:
        woke = os.read(probe._wake_r, 8)  # noqa: SLF001
    except BlockingIOError:
        woke = b""
    probe._close_wake()  # noqa: SLF001
    check(woke == b"x", "emergency_stop writes to the pipe that the reading thread waits on, so that the stop is on the wire when the thread wakes and not when its poll comes round")
    sim = new_sim(time_scale=1.0)
    try:
        sim.place((0.0,) * 6)
        calls: list[tuple[list, object]] = []
        real_select = select.select

        def recording(rlist, wlist, xlist, timeout=None):
            calls.append((list(rlist), timeout))
            return real_select(rlist, wlist, xlist, timeout)

        with mock.patch.object(select, "select", recording):
            bridge = opened(sim, rate_hz=1)
            check(ready(bridge), "ready")
            time.sleep(0.3)
            wake_r = bridge._wake_r  # noqa: SLF001
            bridge.emergency_stop()
            check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2), "a stop is delivered")
            check(until(lambda: not real_select([wake_r], [], [], 0)[0], 1.0), "and the wake-up is taken off the pipe once the thread has it: a pipe left readable would keep the thread from ever waiting")
            bridge.release_stop()
            bridge.close()
        loop = [(rlist, timeout) for rlist, timeout in calls if wake_r in rlist]
        check(len(loop) > 5 and all(len(rlist) == 2 for rlist, _ in loop), f"the reading thread waits on the feedback socket and on the pipe a stop writes to, together ({len(loop)} waits)")
        check(loop and max(timeout for _, timeout in loop) <= 0.05 + 1e-9, f"and never for longer than 50 ms, whatever the control rate: at 1 Hz it still looks at the stop that is owed twenty times a second ({max(t for _, t in loop) if loop else None})")

        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.silent_feedback = True
        time.sleep(0.15)
        sim.requests.clear()
        bridge.emergency_stop()
        check(until(lambda: "Stop()" in sim.requests, 1.5), "a stop is written even when the arm's feedback has gone quiet: it does not wait for a packet to come")
        sim.silent_feedback = False
        check(until(lambda: bridge._dead is None and state(bridge)[0] == rhal.SKR_OK, 3), "ready again")  # noqa: SLF001
        bridge.release_stop()
        sim.requests.clear()
        sim.error_next = [-4]
        bridge.emergency_stop()
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3) and sim.requests.count("Stop()") == 2, f"a controller that refuses the stop has not taken it: it is written again, and delivered when it is taken ({sim.requests.count('Stop()')} writes)")
        bridge.release_stop()

        real_command = DobotBridge._command
        fired: list[int] = []

        def second_stop_meanwhile(self, text, budget=None, by=None, **kw):
            if text == "Stop()" and not fired:
                fired.append(1)
                self.emergency_stop()               # a second stop, called while the first is on the wire
            return real_command(self, text, budget, by, **kw)

        sim.requests.clear()
        with mock.patch.object(DobotBridge, "_command", second_stop_meanwhile):
            bridge.emergency_stop()
            delivered = until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True and sim.requests.count("Stop()") >= 2, 3)
        check(delivered and sim.requests.count("Stop()") == 2, f"a stop called while an earlier one is being written is owed its own write: the first is not taken for it ({sim.requests.count('Stop()')} writes)")
        bridge.release_stop()
        write(bridge, cmd(pose(0.1)))
        check(bridge._goal is not None, "a command leaves the arm a target to hold")  # noqa: SLF001
        bridge.emergency_stop()
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2) and bridge._goal is None, "and a stop that has been written takes it away: the next command holds the joints it does not apply where they are, and not where they were told to be before")  # noqa: SLF001
        bridge._goal = [1.0] * 6  # noqa: SLF001 - what only a command puts there, and none is taken while the stop is latched
        for text in ("disable", "power_on", "request_control", "clear_errors", "enable", "move j1=0.1"):
            code, body = ext(bridge, text)
            check(code == rhal.SKR_EPERM and "stop is latched" in str(body), f"`{text}` is refused while the stop is latched", str(body))
        check(bridge.release_stop() == rhal.SKR_OK and bridge._goal is None, "a release leaves no target of the session before it")  # noqa: SLF001
        bridge.close()
        first = DobotBridge()
        bridge = first
        check(bridge.open(config(sim)) == rhal.SKR_OK and ready(bridge), "ready")
        too_wide = cmd([0.0] * 5 + [99.0])
        said_io = io.StringIO()
        with contextlib.redirect_stderr(said_io):
            bridge.write_command(too_wide, 0)
        before = said_io.getvalue()
        bridge.close()
        check(bridge.open(config(sim)) == rhal.SKR_OK and ready(bridge), "opened again")
        said_io = io.StringIO()
        with contextlib.redirect_stderr(said_io):
            bridge.write_command(too_wide, 0)
        check("outside its range" in before and "outside its range" in said_io.getvalue(), "a refusal is said again in a new session: the one before it had its say")
        bridge.close()
    finally:
        sim.stop()

    section("the stop — a release waits for the arm to have stopped, in the mode the arm is in as well as the flag it sets")
    probe = lifeless_bridge()
    probe._dashboard, probe._loop = object(), object()  # type: ignore[assignment]  # noqa: SLF001
    probe._estop, probe._stop_delivered = True, True  # noqa: SLF001
    feeder = publishing(probe, [dict(robot_mode=7, running_status=0), dict(robot_mode=5, running_status=0)], 0.1)
    code = probe.release_stop()
    seen = probe._packets  # noqa: SLF001
    feeder.join()
    check(code == rhal.SKR_OK and seen == 2, f"a stop in `stop` mode is released on a reading that shows the arm not running in both the flag and the mode, and not on one that shows the flag down and the mode running ({seen} readings)")
    probe = lifeless_bridge()
    probe._dashboard = object()  # type: ignore[assignment]  # noqa: SLF001
    probe._loop = None  # noqa: SLF001
    probe._estop = True  # noqa: SLF001
    began = time.monotonic()
    check(probe.release_stop() == rhal.SKR_OK and not probe._estop and time.monotonic() - began < 0.5, "a stop on a driver whose reading thread has ended is released at once: there is nothing to wait for")  # noqa: SLF001
    sim = new_sim(time_scale=1.0)
    try:
        bridge = opened(sim, stop_mode="emergency")
        check(ready(bridge), "ready")
        sim.set_mode_state(enabled=False)
        check(until(lambda: bridge._latest is not None and bridge._latest.robot_mode == frames.MODE_DISABLED, 3), "an arm that is disabled")  # noqa: SLF001
        sim.drop = True
        bridge.emergency_stop()
        time.sleep(0.3)
        said_release, said = said_by(bridge.release_stop)
        check(said_release == rhal.SKR_EIO and "no reading has shown the arm take it" in said, "an emergency stop that has not been delivered is not released because the arm happens to be disabled already", said)
        sim.drop = False
        until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3)
        bridge.release_stop()
        bridge.close()
    finally:
        sim.stop()

    section("the close — wakes the thread that is waiting, and leaves the pipe of one that did not end")
    sim = new_sim(time_scale=1.0)
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        wake_w = bridge._wake_w  # noqa: SLF001
        written: list[int] = []
        real_write = os.write

        def recording_write(fd, data):
            written.append(fd)
            return real_write(fd, data)

        with mock.patch.object(os, "write", recording_write):
            bridge.close()
        check(wake_w in written, "a close writes to the pipe the reading thread waits on: the thread does not find out at its next poll")
    finally:
        sim.stop()
    stuck, probe = threading.Event(), DobotBridge()
    probe._open_wake()  # noqa: SLF001
    probe._loop = threading.Thread(target=lambda: stuck.wait(6.0), daemon=True, name="stuck-exchange")  # noqa: SLF001
    probe._loop.start()  # noqa: SLF001
    wake_r = probe._wake_r  # noqa: SLF001
    probe._teardown()  # noqa: SLF001
    open_still = True
    try:
        os.fstat(wake_r)
    except OSError:
        open_still = False
    stuck.set()
    check(open_still and probe._wake_r == wake_r, "a thread that did not end keeps its end of the pipe: it is let go with the process, not closed under it")  # noqa: SLF001
    probe._wake_r = wake_r  # noqa: SLF001
    probe._close_wake()  # noqa: SLF001

def survivors_commands() -> None:
    section("write_command — the edge of a range, a driver that is closed, a wire that will not carry the command, and the words a refusal uses")
    bridge = lifeless_bridge()
    low, high = LIMITS[0]
    code, said = write(bridge, cmd([high + 1e-6, 0.0, 0.0, 0.0, 0.0, 0.0]))
    check(code == rhal.SKR_EINVAL and "outside its range" in said, "a target a millionth of a radian past the range is outside it: the allowance is for rounding, a billionth, and not for a hair", said)
    bridge._opened = False  # noqa: SLF001
    check(write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_EPERM, "a command to a driver that is not open is EPERM though its dashboard is there")
    bridge._opened = True  # noqa: SLF001
    with mock.patch.object(DobotBridge, "_send_servo", side_effect=frames.ProtocolError("not a ServoJ")):
        code, said = write(bridge, cmd(pose(0.1)))
    check(code == rhal.SKR_EINVAL and "not a ServoJ" in said, "a command the wire cannot carry is EINVAL and says why: it is not taken, and it does not escape", said)
    with bridge._lock:  # noqa: SLF001
        bridge._latest = None  # noqa: SLF001
    code, said = write(bridge, cmd(pose(0.1)))
    check(code == rhal.SKR_ETIMEDOUT and "has not been read yet" in said, "a command before the arm has been read is ETIMEDOUT, and says so", said)
    set_reading(bridge, alarms=[116], robot_mode=frames.MODE_ERROR, error_status=1)
    code, said = write(bridge, cmd(pose(0.1)))
    check(code == rhal.SKR_EPERM and "emergency stop (alarm 116)" in said, "an emergency stop is called an emergency stop, with its alarm", said)
    set_reading(bridge, alarms=[17], robot_mode=frames.MODE_ERROR, error_status=1)
    code, said = write(bridge, cmd(pose(0.1)))
    check(code == rhal.SKR_EPERM and "an alarm state (alarm 17)" in said, "and any other alarm an alarm state", said)

    contrived = LIMITS[0][0], 10.00000049 * math.pi / 180.0                # an upper limit whose degrees are 10.00000049: half a millionth of a degree past the sixth decimal
    sim = new_sim()
    try:
        sim.place((0.0,) * 6)
        bridge = opened(sim, limits=[[contrived[0], contrived[1]]] + [list(pair) for pair in LIMITS[1:]])
        check(ready(bridge), "ready")
        sim.servo_calls.clear()
        code, said = write(bridge, cmd([contrived[1] + 5e-10, 0.0, 0.0, 0.0, 0.0, 0.0]))
        check(code == rhal.SKR_OK and until(lambda: bool(servo_texts(sim)), 2) and servo_texts(sim)[-1].startswith("ServoJ(10.000000,"), f"a target a billionth past the edge is sent as the edge, and not as itself ({servo_texts(sim)[-1:]})")
        bridge.close()

        bridge = opened(sim, timeout_ms=500)
        check(ready(bridge), "ready")
        dead_seen: list[object] = []
        sim.feedback_gap_s = 0.18
        deadline = time.monotonic() + 1.2
        while time.monotonic() < deadline:
            dead_seen.append(bridge._dead)  # noqa: SLF001
            time.sleep(0.002)
        sim.feedback_gap_s = 0.0
        check(all(d is None for d in dead_seen), "packets that come a fifth of a second apart are a slow arm, and not a silent one: a line is silent when it has said nothing for three transactions")
        bridge.close()
    finally:
        sim.stop()

    section("the reading thread — two lost packets and a good one, over and over, are never a dead line")
    sim = new_sim()
    try:
        bridge = opened(sim, rate_hz=100)
        check(ready(bridge), "ready")
        feedback = bridge._feedback  # noqa: SLF001
        real_read = feedback.read_packet
        seen: list[tuple[int, object]] = []
        script = ["fail", "fail", "ok"] * 4 + ["ok"] * 3

        def scripted(wait):
            seen.append((bridge._failures, bridge._dead))  # noqa: SLF001
            step = script[len(seen) - 1] if len(seen) <= len(script) else "ok"
            if step == "fail":
                raise link.LinkError("a packet was lost")
            return real_read(wait)

        feedback.read_packet = scripted  # type: ignore[method-assign]
        check(until(lambda: len(seen) >= len(script) + 2, 5), "the reading thread goes through a script of lost packets")
        feedback.read_packet = real_read  # type: ignore[method-assign]
        check(max(failures for failures, _ in seen) <= 2 and all(dead is None for _, dead in seen) and bridge._dead is None,  # noqa: SLF001
              f"a good packet forgives the failures before it, and three in a row are what make a line dead (failures at the start of each cycle: {[f for f, _ in seen]})")
        bridge.close()
    finally:
        sim.stop()

def survivors_threads() -> None:
    section("the reading thread — it opens a line again at most twice a second, and starts counting silence from the new line")
    sim = new_sim()
    try:
        bridge = opened(sim, rate_hz=100)
        check(ready(bridge), "ready")
        dashboard_opens: list[float] = []
        feedback_opens: list[tuple[float, int]] = []
        real_dashboard, real_feedback = link.Dashboard.open, link.FeedbackStream.open

        def counted_dashboard(self, timeout=1.0):
            dashboard_opens.append(time.monotonic())
            return real_dashboard(self, timeout)

        def counted_feedback(self, timeout=1.0):
            result = real_feedback(self, timeout)
            feedback_opens.append((time.monotonic(), bridge._failures))  # noqa: SLF001
            return result

        with mock.patch.object(link.Dashboard, "open", counted_dashboard), mock.patch.object(link.FeedbackStream, "open", counted_feedback):
            sim.stop_listening()
            sim.hang_up()
            write(bridge, cmd(pose(0.1)))               # a command is how the dashboard finds out that it has been hung up on
            time.sleep(1.3)
            check(until(lambda: bridge._dead is not None, 2), "a controller that has gone is a dead line")  # noqa: SLF001
            tried_dashboard, tried_feedback = len(dashboard_opens), bridge._failures  # noqa: SLF001
        check(1 <= tried_dashboard <= 5, f"a dashboard the controller dropped is opened again at most twice a second, and not as fast as the thread goes ({tried_dashboard} tries in about 1.3 s)")
        sim.resume_listening()
        sim.silent_feedback = True
        with mock.patch.object(link.FeedbackStream, "open", counted_feedback):
            feedback_opens.clear()
            check(until(lambda: bool(feedback_opens), 3), "the feedback is opened again when the controller is back")
            opened_at, failures_then = feedback_opens[0]
            time.sleep(0.2)
            failures_after = bridge._failures  # noqa: SLF001
        sim.silent_feedback = False
        check(failures_after - failures_then <= 1, f"and a feedback line that has only just been opened has not been silent yet: silence is counted from the new line and not from the last good packet ({failures_after - failures_then} failures in 0.2 s)")
        check(until(lambda: bridge._dead is None and state(bridge)[0] == rhal.SKR_OK, 4), "ready again")  # noqa: SLF001
        counted_opens: list[float] = []

        def counted_feedback_again(self, timeout=1.0):
            counted_opens.append(time.monotonic())
            return real_feedback(self, timeout)

        with mock.patch.object(link.FeedbackStream, "open", counted_feedback_again):
            sim.stop_listening()
            sim.hang_up()
            time.sleep(1.3)
        check(1 <= len(counted_opens) <= 4, f"a feedback line is opened again at most twice a second ({len(counted_opens)} tries in about 1.3 s)")
        sim.resume_listening()
        bridge.close()

        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.silent_feedback = True
        entered, release = threading.Event(), threading.Event()
        real_select = select.select
        waits: list[int] = []

        def held_select(rlist, wlist, xlist, timeout=None):
            if len(rlist) == 2 and not entered.is_set():
                entered.set()
                release.wait(2.0)
                return [], [], []
            return real_select(rlist, wlist, xlist, timeout)

        delivered: list[int] = []

        def counting_deliver(self):
            if entered.is_set():
                delivered.append(1)

        with mock.patch.object(select, "select", held_select), mock.patch.object(DobotBridge, "_deliver_if_owed", counting_deliver):
            check(entered.wait(2.0), "the reading thread waits")
            bridge._closing.set()  # noqa: SLF001
            release.set()
            time.sleep(0.2)
        check(not delivered, "a thread that is told to close while it waits does nothing more: it does not deliver a stop, or read another packet, on its way out")
        bridge._closing.clear()  # noqa: SLF001
        sim.silent_feedback = False
        bridge.close()
    finally:
        sim.stop()

    section("the alarm list — asked for only of a controller that takes commands, and read only as an answer")
    sim = new_sim()
    try:
        bridge = opened(sim, rate_hz=100, stale_ms=30000)
        check(ready(bridge), "ready")
        sim.set_mode_state(tcp=False)
        sim.raise_alarm(17)
        check(until(lambda: bridge._tcp is False, 3), "a controller that takes no TCP is noticed")  # noqa: SLF001
        asked = sum(1 for r in sim.requests if r.lower().startswith("geterrorid"))
        time.sleep(1.2)
        check(sum(1 for r in sim.requests if r.lower().startswith("geterrorid")) == asked, "and it is not asked for the alarm list again: it would only say again that it takes no TCP")
        sim.set_mode_state(tcp=True)
        bridge.close()
    finally:
        sim.stop()

    bridge = lifeless_bridge()
    asked_for: list[int] = []

    def counting_read(mode: int) -> None:
        asked_for.append(mode)
        with bridge._lock:  # noqa: SLF001
            bridge._alarms, bridge._alarms_read_at, bridge._alarms_for_mode = [17], time.monotonic(), mode  # noqa: SLF001

    bridge._read_alarms = counting_read  # type: ignore[method-assign]  # noqa: SLF001
    bridge._observe(frames.decode_feedback(packet_of(robot_mode=9, error_status=1)))  # noqa: SLF001
    bridge._observe(frames.decode_feedback(packet_of(robot_mode=9, error_status=1)))  # noqa: SLF001
    check(asked_for == [9], "the alarm list is read once in the first half second of an alarm")
    bridge._observe(frames.decode_feedback(packet_of(robot_mode=5)))  # noqa: SLF001
    check(bridge._alarms == [] and bridge._alarms_read_at == 0.0 and bridge._alarms_for_mode == 0, "an arm that is well holds no alarm and no memory of when they were read")  # noqa: SLF001
    bridge._observe(frames.decode_feedback(packet_of(robot_mode=0, error_status=1)))  # noqa: SLF001
    check(asked_for == [9, 0], "and the next alarm is read at once, though the last was read a moment ago, in whatever mode it comes")
    bridge._observe(frames.decode_feedback(packet_of(robot_mode=5)))  # noqa: SLF001
    bridge._observe(frames.decode_feedback(packet_of(robot_mode=5, error_status=1)))  # noqa: SLF001
    check(asked_for == [9, 0, 5], "an alarm that only the packet's error status shows is read, too")

    bridge = lifeless_bridge()
    for reply, what in (("-2,{[17]},GetErrorID();", "an error reply, whatever it holds, is not an alarm list"), ("0,{garbage},GetErrorID();", "a list that is not one is not read, and does not escape")):
        bridge._command = lambda text, budget=None, by=None, text_of=reply, **kw: frames.parse_reply(text_of)  # type: ignore[method-assign]  # noqa: SLF001
        try:
            bridge._read_alarms(9)  # noqa: SLF001
            escaped = ""
        except Exception as error:  # noqa: BLE001
            escaped = repr(error)
        check(bridge._alarms == [] and not escaped, what, escaped)  # noqa: SLF001

    def not_tcp(text, budget=None, by=None, **kw):
        raise link.NotTcp("the controller says its control mode is not TCP")

    bridge._command = not_tcp  # type: ignore[method-assign]  # noqa: SLF001
    bridge._read_alarms(9)  # noqa: SLF001
    check(bridge._tcp is False, "a controller that answers the alarm list with the sentence about its control mode is noted as one that takes no TCP")  # noqa: SLF001

def survivors_motions() -> None:
    section("the motions — what each waits for, and what each leaves behind")
    ok = frames.parse_reply("0,{3},Ok();")

    def predicate_of(bridge: DobotBridge, text: str):
        """The predicate a motion hands to the wait, taken out by a wait that records it and ends at once."""
        caught: list = []

        def catch(self, predicate, timeout, *, honour_stop=True):
            caught.append(predicate)
            raise bridge_module.BridgeRefusal("caught", rhal.SKR_EIO)

        with mock.patch.object(DobotBridge, "_wait_for_packet", catch), mock.patch.object(DobotBridge, "_do", lambda self, command: ok):
            ext(bridge, text)
        return caught[0]

    def reading_of(**fields) -> frames.Feedback:
        base = dict(robot_mode=5, enable_status=1, brake_status=0x3F, error_status=0, running_status=0, collision_state=0, robot_type=5, safety_state=0xFF)
        base.update(fields)
        return frames.decode_feedback(packet_of(**base))

    bridge = lifeless_bridge(robot_mode=frames.MODE_DISABLED, enable_status=0)
    enough = predicate_of(bridge, "enable")
    check([enough(reading_of(robot_mode=m)) for m in (3, 4, 5, 7, 8, 9)] == [False, False, True, True, True, True], "an enable is done when a reading shows the arm enabled, or in alarm: a disabled arm, or a powered-off one, is not done yet")
    bridge = lifeless_bridge(robot_mode=frames.MODE_RUNNING, enable_status=1)
    sent: list[str] = []
    with mock.patch.object(DobotBridge, "_do", lambda self, command: sent.append(command) or ok):
        code, body = ext(bridge, "enable")
    check(code == rhal.SKR_OK and body == {"result": "already_enabled"} and not sent, "an arm that is running is enabled already, and is not sent the enable again: it would reset the payload mid-motion")
    bridge._goal = [1.0] * 6  # noqa: SLF001
    bridge = lifeless_bridge(robot_mode=frames.MODE_DISABLED, enable_status=0)
    bridge._goal = [1.0] * 6  # noqa: SLF001
    with mock.patch.object(DobotBridge, "_do", lambda self, command: ok), mock.patch.object(DobotBridge, "_wait_for_packet", lambda self, predicate, timeout, **kw: reading_of()):
        code, body = ext(bridge, "enable")
    check(code == rhal.SKR_OK and body["result"] == "enabled" and bridge._goal is None, "an enable leaves no target of the arm's last enablement: a command after it holds the joints it does not apply where they are")  # noqa: SLF001
    bridge._goal = [1.0] * 6  # noqa: SLF001
    with mock.patch.object(DobotBridge, "_do", lambda self, command: ok):
        code, body = ext(bridge, "disable")
    check(code == rhal.SKR_OK and bridge._goal is None, "and so does a disable")  # noqa: SLF001
    bridge = lifeless_bridge(robot_mode=frames.MODE_ERROR, error_status=1, alarms=[17])
    cleared = predicate_of(bridge, "clear_errors")
    check([cleared(reading_of(robot_mode=m)) for m in (9, 11)] == [False, False] and cleared(reading_of(error_status=1)) is False and cleared(reading_of(collision_state=1)) is False and cleared(reading_of()) is True and cleared(reading_of(robot_mode=4)) is True,
          "an alarm is cleared when a reading shows no alarm, no collision and no error flag: any one of the four is not cleared yet")

    start = (10.0, 20.0, 30.0, 40.0, 50.0, 60.0)
    goal = (0.1 / DEG, 20.0, 30.0, 40.0, 50.0, 60.0)

    def goes(script: list[dict], text: str = "move j1=0.1"):
        mover = lifeless_bridge(q_actual=start)
        texts: list[str] = []
        used: list[int] = []

        def wait(self, predicate, timeout, *, honour_stop=True):
            for index, fields in enumerate(script):
                reading = reading_of(**{"q_actual": start, **fields})
                used.append(index)
                if predicate(reading):
                    return reading
            raise bridge_module.BridgeRefusal("no reading showed what was waited for in 1.0 s (the line is answering).", rhal.SKR_EIO)

        with mock.patch.object(DobotBridge, "_wait_for_packet", wait), mock.patch.object(DobotBridge, "_do", lambda self, command: texts.append(command) or ok):
            code, body = ext(mover, text)
        return code, body, texts, used, mover

    code, body, texts, used, mover = goes([dict(robot_mode=7, running_status=1), dict(q_actual=goal)])
    check(code == rhal.SKR_OK and body["result"] == "arrived" and used == [0, 1], "a move is done when the arm is not running and is at its goal", str((code, body, used)))
    check(texts == [frames.mov_j(list(goal))], f"and the joints it names are the only ones it moves: the others are sent where they are ({texts})")
    check(mover._goal == list(goal), "and the target of the move is remembered, for the commands that follow it")  # noqa: SLF001
    code, body, texts, used, mover = goes([dict(robot_mode=7, running_status=1), dict(q_actual=(20.0,) + start[1:])])
    check(code == rhal.SKR_OK and body["result"] == "stopped_short", "a move that ends short of its goal is stopped short and not arrived", str((code, body)))
    code, body, texts, used, mover = goes([dict(robot_mode=11, collision_state=1)])
    check(code == rhal.SKR_EIO and "a collision during the move" in str(body), "a collision ends a move, EIO, naming it", str((code, body)))
    code, body, texts, used, mover = goes([dict(robot_mode=7, running_status=0), dict(q_actual=goal)])
    check(code == rhal.SKR_OK and body["result"] == "arrived" and used == [0, 1], f"an arm that the mode says is running is not done, whatever the flag says ({used})", str((code, body)))
    code, body, texts, used, mover = goes([dict(q_actual=(20.0,) + start[1:])])
    check(code == rhal.SKR_OK and body["result"] == "stopped_short", "an arm that has moved and is not moving and is not there has started and stopped, short of its goal, and the move says so", str((code, body)))
    code, body, texts, used, mover = goes([dict()])
    check(code == rhal.SKR_EIO and "never started" in str(body), "and one that has not moved, is not moving and is not at its goal has not begun", str((code, body)))

def survivors_open() -> None:
    section("a command's budget is the caller's, and what the open asks of an arm that cannot answer")
    sim = new_sim()
    try:
        bridge = opened(sim, timeout_ms=1000)
        check(ready(bridge), "ready")
        sim.delay_s = 0.3
        began = time.monotonic()
        message = raises(link.LinkError, bridge._command, frames.robot_mode(), 0.05) or ""  # noqa: SLF001
        took = time.monotonic() - began
        check("did not answer RobotMode in 50 ms" in message and took < 0.25, f"a command given a budget waits that long for its reply and not the transaction bound ({took:.2f} s of 0.05, against a bound of 1 s)", message)
        bridge.close()
        bridge = opened(sim, timeout_ms=1000)
        check(ready(bridge), "ready")
        sim.delay_s = 0.3
        began = time.monotonic()
        message = raises(link.LinkError, bridge._command, frames.robot_mode(), 1.0, time.monotonic() + 0.05) or ""  # noqa: SLF001
        took = time.monotonic() - began
        sim.delay_s = 0.0
        check("did not answer RobotMode" in message and took < 0.25, f"and a command whose caller has only a little time left is given that and no more, though its own budget is longer ({took:.2f} s of 0.05)", message)
        bridge.close()
    finally:
        sim.stop()
    for what, prepare in (("in alarm", lambda s: s.raise_alarm(17)), ("powered off", lambda s: s.set_mode_state(powered=False))):
        sim = new_sim()
        try:
            prepare(sim)
            sim.requests.clear()
            code, said, bridge = opens_with(sim)
            asked = list(sim.requests)
            check(code == rhal.SKR_OK and "GetAngle()" not in asked and "RobotMode()" in asked, f"an arm {what} is opened without being asked where it is: the controller does not answer that in this state, and the units check waits for an arm that can ({asked})", said)
            bridge.close()
        finally:
            sim.stop()
    sim = new_sim()
    try:
        sim.error_next = [-1]
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_ENODEV and "answered RobotMode() with error -1" in said, "a controller that answers RobotMode() with an error is refused as it is, with the document's words for the error, and not as an arm that could not be read", said)
    finally:
        sim.stop()

def survivors_status() -> None:
    section("the status motion — each field from its own place in the packet and in its own unit, and what a reading that was never made says")
    bridge = lifeless_bridge()
    angles = (10.1234567, -20.7654321, 30.0, 40.0, 50.0, 60.0)
    set_reading(bridge, robot_mode=5, enable_status=0, collision_state=1, running_status=1, error_status=1, q_actual=angles, motor_temperatures=(31.26, 32.0, 33.0, 34.0, 35.0, 36.0), speed_scaling=77.0, load_kg=1.25)
    code, body = ext(bridge, "status")
    check(code == rhal.SKR_OK and isinstance(body, dict), "status answers", str(body))
    body = body if isinstance(body, dict) else {}
    check(body.get("enabled") is False and body.get("collision") is True and body.get("running") is True and body.get("alarm") is True,
          f"enabled, collision, running and alarm are each the packet's own flag and not read off the mode: an arm in mode 5 with its enable flag down, its collision, running and error flags up ({body.get('enabled')}, {body.get('collision')}, {body.get('running')}, {body.get('alarm')})")
    check(body.get("position_rad") == [round(q * DEG, 6) for q in angles] and body.get("position_deg") == [round(q, 4) for q in angles],
          f"a position is the packet's degrees to four places and those degrees in radians to six ({body.get('position_rad')}, {body.get('position_deg')})")
    check(body.get("temperatures") == [31.3, 32.0, 33.0, 34.0, 35.0, 36.0] and body.get("speed_factor") == 77.0 and body.get("load_kg") == 1.25, "the temperatures are to a tenth of a degree, and the speed factor and the load are the packet's", str(body.get("temperatures")))
    set_reading(bridge, robot_mode=9, enable_status=1, error_status=0)
    body = ext(bridge, "status")[1]
    check(isinstance(body, dict) and body.get("alarm") is True and body.get("enabled") is True and body.get("collision") is False and body.get("running") is False, "an arm in mode 9 is in alarm with its error flag down, and the other flags are their own")
    set_reading(bridge, robot_mode=5, enable_status=1)
    body = ext(bridge, "status")[1]
    check(isinstance(body, dict) and (body.get("alarm"), body.get("enabled"), body.get("collision"), body.get("running"), body.get("fault")) == (False, True, False, False, None), "and a well arm has none of the three up and is enabled")
    set_reading(bridge, robot_mode=77)
    body = ext(bridge, "status")[1]
    check(isinstance(body, dict) and body.get("robot_mode_name") == "not in the document" and body.get("fault", {}).get("code") == bridge_module.UNLISTED_MODE_BASE | 77, "a mode the document does not list is called that, and is a fault")
    empty = lifeless_bridge()
    with empty._lock:  # noqa: SLF001
        empty._latest = None  # noqa: SLF001
    code, body = ext(empty, "status")
    check(code == rhal.SKR_ETIMEDOUT and "has not been read" in str(body), f"status of an arm that has not been read is a timeout and says so (code {code})", str(body))

    section("read_state — a State that is read again, and a reading that is exactly as old as the bound")
    bridge = lifeless_bridge()
    out = rhal.State()
    set_reading(bridge, robot_mode=9, error_status=1, alarms=[17])
    bridge.read_state(out, 0)
    check(out.fault_code == 17 and out.flags & rhal.FLAG_FAULT, "an alarm is the fault word and the flag")
    set_reading(bridge)
    code = bridge.read_state(out, 0)
    check(code == rhal.SKR_OK and out.fault_code == 0 and not out.flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP | rhal.FLAG_DEGRADED), "and the State read again with the arm well has no fault word and none of the flags: nothing of the last reading is left in it")
    check(all(math.isnan(v) for v in out.velocity_rad_s) and all(math.isnan(v) for v in out.effort_nm) and len(list(out.velocity_rad_s)) == 6, "a velocity and an effort are not reported: they are not numbers, six of each")
    fresh = rhal.State()
    set_reading(bridge)
    bridge.read_state(fresh, 0)
    check(all(math.isnan(v) for v in fresh.velocity_rad_s) and all(math.isnan(v) for v in fresh.effort_nm), "even in a State that has never held a reading")
    unread = lifeless_bridge()
    with unread._lock:  # noqa: SLF001
        unread._latest = None  # noqa: SLF001
    out = rhal.State()
    code = unread.read_state(out, 0)
    check(code == rhal.SKR_ETIMEDOUT and out.flags & rhal.FLAG_DEGRADED and len(list(out.position_rad)) == 6 and all(math.isnan(v) for v in out.position_rad) and all(math.isnan(v) for v in out.velocity_rad_s), "a bridge that has no reading yet is a timeout, degraded, with six positions that are not numbers")
    bridge = lifeless_bridge()
    bridge._stale_s = 0.5  # noqa: SLF001
    with bridge._lock:  # noqa: SLF001
        bridge._stamp_ns = 1_000_000_000  # noqa: SLF001
    for age_ns, held, what in ((1_500_000_000, True, "exactly as old as the bound is not yet stale"), (1_500_000_001, False, "and a nanosecond older is")):
        clock = iter([2_000_000_000, age_ns])
        with mock.patch.object(rhal, "acquisition_ns", side_effect=lambda: next(clock)):
            out = rhal.State()
            code = bridge.read_state(out, 0)
        check((code == rhal.SKR_OK and not out.flags & rhal.FLAG_DEGRADED) is held, f"a reading {what}")


def survivors_session() -> None:
    section("a fresh bridge, a closed one, and a new session")
    fresh = DobotBridge()
    out = rhal.State()
    check(fresh.read_state(out, 0) == rhal.SKR_EPERM, "a bridge that was never opened is not readable: EPERM")
    check(fresh.write_command(cmd(pose(0.0)), 0) == rhal.SKR_EPERM, "nor writable")
    check(bridge_module.DobotBridge.stop_during_extension is True and bridge_module.Unreachable("x").code == rhal.SKR_EIO and bridge_module.BridgeRefusal("x").code == rhal.SKR_ENODEV,
          "a stop may be written while a motion runs, a controller that does not answer is EIO and a refusal is ENODEV unless it says otherwise")
    probe = DobotBridge()
    probe._dashboard = link.Dashboard("127.0.0.1", 1)  # noqa: SLF001 - there is a line and the driver is not open
    code, body = probe.extension(TOKEN, b"status")
    check(code == rhal.SKR_EPERM and b"not open" in body and probe.write_command(cmd(pose(0.0)), 0) == rhal.SKR_EPERM, "a driver that is not open answers no motion and takes no command, whatever line it holds", body.decode())
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(bridge._loop is not None and bridge._loop.daemon is True, "the reading thread is a daemon: a driver nobody closed does not keep the host alive")  # noqa: SLF001
        first, second = rhal.State(), rhal.State()
        bridge.read_state(first, 0)
        bridge.read_state(second, 0)
        check((first.seq, second.seq) == (1, 2), f"the first reading of a bridge is 1 and the next 2 ({first.seq}, {second.seq})")
        check(bridge._stale_s == 0.5, "with no `stale_ms` a reading is stale at half a second")  # noqa: SLF001
        bridge.close()
        check(bridge.read_state(rhal.State(), 0) == rhal.SKR_EPERM, "a closed driver is not readable: EPERM, and not a timeout of a reading that will not come")
        bridge.close()
        check(bridge.read_state(rhal.State(), 0) == rhal.SKR_EPERM, "and closing it again changes nothing")
    finally:
        sim.stop()
    bridge = DobotBridge()
    bridge._open_wake()  # noqa: SLF001
    ends = (bridge._wake_r, bridge._wake_w)  # noqa: SLF001
    bridge._teardown()  # noqa: SLF001
    gone = []
    for end in ends:
        try:
            os.fstat(end)
            gone.append(False)
        except OSError:
            gone.append(True)
    check(gone == [True, True] and (bridge._wake_r, bridge._wake_w) == (-1, -1), f"a teardown with no thread to wait for closes the wake pipe it made ({gone})")  # noqa: SLF001
    bridge = lifeless_bridge()
    with bridge._lock:  # noqa: SLF001
        bridge._alarms, bridge._alarms_read_at, bridge._alarms_for_mode, bridge._goal, bridge._stop_delivered, bridge._tcp = [17], 123.0, 9, [1.0] * 6, True, False  # noqa: SLF001
    bridge._estop, bridge._last_refusal, bridge._descriptor = True, "x", "{}"  # noqa: SLF001
    bridge._reset_session()  # noqa: SLF001
    check((bridge._alarms, bridge._alarms_read_at, bridge._alarms_for_mode, bridge._goal, bridge._stop_delivered, bridge._tcp, bridge._latest) == ([], 0.0, 0, None, False, None, None)  # noqa: SLF001
          and (bridge._estop, bridge._last_refusal, bridge._descriptor) == (False, None, None),  # noqa: SLF001
          "a new session starts with no stop, no alarm remembered (nor when it was read, nor in which mode), no goal, no reading and no word about the control mode")


def survivors_open_settings() -> None:
    section("what a configuration may say: the edges of each number, and how long each step of the open may take")
    sim = new_sim()
    try:
        for key in ("rate_hz", "timeout_ms", "stale_ms", "open_timeout_ms"):
            code, said, probe = opens_with(sim, **{key: 0})
            probe.close()
            check(code == rhal.SKR_ENODEV and "is not a number from" in said, f"{key} of 0 is refused as below its range and not taken for an absent one", said)
        for extra, what in (({"payload_kg": 0}, "a payload of nothing"), ({"payload_kg": 100}, "one of a hundred kilograms"), ({"open_timeout_ms": 100}, "an open timeout of a tenth of a second")):
            code, said, probe = opens_with(sim, **extra)
            probe.close()
            check(code == rhal.SKR_OK, f"{what} is accepted", said)
            until(lambda: not sim.moving, 3.0)
        code, said, probe = opens_with(sim, open_timeout_ms=99)
        probe.close()
        check(code == rhal.SKR_ENODEV and "open_timeout_ms" in said, "and 99 ms is not", said)
        for given, what in (({"t": True}, "a bool"), ({"gain": True}, "a bool for the gain"), ({"t": "0.1"}, "a string")):
            code, said, probe = opens_with(sim, servo=given)
            probe.close()
            check(code == rhal.SKR_ENODEV and "is not a number from" in said, f"a servo parameter that is {what} is refused by name", said)
        seen: list[float] = []
        real = link.Dashboard.open

        def dashboard_spy(self, timeout: float = 1.0):
            seen.append(timeout)
            return real(self, timeout)

        for open_ms, wanted in ((10000, 3.0), (1500, 1.5)):
            until(lambda: not sim.moving, 3.0)
            del seen[:]
            with mock.patch.object(link.Dashboard, "open", dashboard_spy):
                probe = opened(sim, open_timeout_ms=open_ms)
            probe.close()
            check(seen and seen[0] == wanted, f"the dashboard is waited for three seconds at most, or the open timeout when that is shorter ({seen[:1]} for {open_ms} ms, wanted {wanted})")
        waits: list[float] = []
        real_read = link.FeedbackStream.read_packet

        def read_spy(self, wait: float):
            waits.append(wait)
            return real_read(self, wait)

        for open_ms in (10000, 100):
            until(lambda: not sim.moving, 3.0)
            del waits[:]
            with mock.patch.object(link.FeedbackStream, "read_packet", read_spy):
                probe = opened(sim, open_timeout_ms=open_ms)
            probe.close()
            check(bool(waits) and ((open_ms == 10000 and waits[0] == 0.25) or (open_ms == 100 and 0.0 < waits[0] <= 0.1)),
                  f"the first packet is waited for a quarter of a second at a time, or what is left of the open timeout ({waits[:1]} for {open_ms} ms)")
        calls: list[tuple[str, float | None]] = []
        real_command = DobotBridge._command

        def command_spy(self, text, budget=None, *args, **kwargs):
            calls.append((text, budget))
            return real_command(self, text, budget, *args, **kwargs)

        for open_ms, wanted in ((10000, 1.0), (500, 0.5)):
            until(lambda: not sim.moving, 3.0)
            del calls[:]
            with mock.patch.object(DobotBridge, "_command", command_spy):
                probe = opened(sim, open_timeout_ms=open_ms)
            probe.close()
            asked = [(text, budget) for text, budget in calls if text in ("RobotMode()", "GetAngle()")][:2]
            check(asked == [("RobotMode()", wanted), ("GetAngle()", wanted)], f"the two questions of the open are given a second each, or the open timeout when that is shorter ({asked} for {open_ms} ms)")
        until(lambda: not sim.moving, 3.0)
        sim.place((10.0,) * 6)
        until(lambda: not sim.moving, 3.0)
        real_command = DobotBridge._command
        for offset, held, what in ((2.0, True, "two degrees"), (2.01, False, "more than two")):
            def command_lie(self, text, budget=None, *args, _offset=offset, **kwargs):
                if text == "GetAngle()":
                    return frames.Reply(0, ",".join(str(10.0 + _offset) if j == 0 else "10.0" for j in range(6)), "GetAngle()")
                return real_command(self, text, budget, *args, **kwargs)

            with mock.patch.object(DobotBridge, "_command", command_lie):
                code, said, probe = opens_with(sim)
            probe.close()
            check((code == rhal.SKR_OK) is held, f"the feedback and GetAngle() may differ by {what} and be taken for one unit" if held else f"and when they differ by {what} one of them is in another unit", said)
        error_reply = frames.Reply(-1, "", "RobotMode()")
        probe = lifeless_bridge()
        probe._command = lambda text, budget=None, *a, **k: error_reply  # type: ignore[method-assign]  # noqa: SLF001
        first = frames.decode_feedback(packet_of(robot_mode=5, robot_type=5))
        message = ""
        code = None
        try:
            probe._probe_control(first, 1.0)  # noqa: SLF001
        except bridge_module.BridgeRefusal as refusal:
            message, code = str(refusal), refusal.code
        check(code == rhal.SKR_EIO and "answered RobotMode() with error -1" in message, "a controller that answers RobotMode() with an error is refused as an I/O error", message)
    finally:
        sim.stop()
    probe = lifeless_bridge()
    low, high = -2 * math.pi, 2 * math.pi
    rows = ((low, high, True, "exactly a turn either way"), (low - 1e-9, high + 1e-9, True, "a turn either way and a nano-radian over, the tolerance"), (low - 1.5e-9, 1.0, False, "a turn and a half nano-radian under"),
            (-1.0, high + 1.5e-9, False, "a turn and a half nano-radian over"))
    for given_low, given_high, held, what in rows:
        message = raises(bridge_module.BridgeRefusal, probe._read_limits, [[given_low, given_high]] + [[-1.0, 1.0]] * 5)  # noqa: SLF001
        check((message is None) is held, f"a limit of {what} is {'accepted' if held else 'refused'}", str(message))
    check(raises(bridge_module.BridgeRefusal, probe._read_limits, [[1.0, 1.0]] + [[-1.0, 1.0]] * 5) is not None, "and a range that is a point is not a range")  # noqa: SLF001


def survivors_commands2() -> None:
    section("a command's range edge, what a refusal's words are counted by, and what a stop leaves behind")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        low = LIMITS[0][0]
        code, said = write(bridge, cmd([low - 1.5e-9] + [0.0] * 5))
        check(code == rhal.SKR_EINVAL and "outside its range" in said, "a joint a nano-radian and a half under its range is refused whole, as one a nano-radian under is not", said)
        code, said = write(bridge, cmd([low - 5e-10] + [0.0] * 5))
        check("outside its range" not in said, "a joint half a nano-radian under is the range's edge and is not refused for it", said)
        bridge.close()
    finally:
        sim.stop()
    bridge = lifeless_bridge()
    prefix = "the line is not what it was: " + "x" * 11
    messages = iter([prefix + "A", prefix + "B", prefix + "C"])

    def gate_says():
        raise bridge_module.BridgeRefusal(next(messages), rhal.SKR_EPERM)

    bridge._gate = gate_says  # type: ignore[method-assign]  # noqa: SLF001
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        codes = [bridge.write_command(cmd(pose(0.1)), 0) for _ in range(3)]
    check(codes == [rhal.SKR_EPERM] * 3 and said.getvalue().count("skynet-dobot:") == 1, f"refusals that share their first forty characters and their code are one cause, said once ({said.getvalue().count('skynet-dobot:')} times)")
    gate = lifeless_bridge()
    for fields, wanted, unwanted, what in (({"robot_mode": 5, "collision_state": 1}, "collision detected", None, "a collision flag in an enabled arm"), ({"robot_mode": 9}, frames.MODES[9], "collision", "an alarm with no list read"),
                                           ({"robot_mode": 11}, "collision detected", None, "mode 11")):
        set_reading(gate, **fields)
        message = raises(bridge_module.BridgeRefusal, gate._gate) or ""  # noqa: SLF001
        check(wanted in message and (unwanted is None or unwanted not in message), f"a command refused for {what} says {wanted!r}", message)


def survivors_stop2() -> None:
    section("the stop — the waits of a release, what a release clears, a wake that never blocks, and the owed stop")
    for mode in ("emergency", "stop"):
        bridge = lifeless_bridge()
        bridge._loop = threading.Thread()  # noqa: SLF001 - a thread object that was never started: there is a loop to wait on
        bridge._stop_mode, bridge._timeout = mode, 0.12  # noqa: SLF001
        waits: list[float] = []

        def waiting(predicate, timeout, *, honour_stop=True, _waits=waits):
            _waits.append(timeout)
            raise bridge_module.BridgeRefusal("no reading showed it.", rhal.SKR_EIO)

        bridge._wait_for_packet = waiting  # type: ignore[method-assign]  # noqa: SLF001
        bridge._estop = True  # noqa: SLF001
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = bridge.release_stop()
        check(code == rhal.SKR_EIO and waits == [bridge_module._COMMAND_BUDGET_S * 4 + 0.12 * 6] and bridge._estop is True, f"a release in {mode} mode waits a second for a reading that shows the stop taken, and the latch stays when none does ({waits})")  # noqa: SLF001
        bridge = lifeless_bridge()
        bridge._loop = threading.Thread()  # noqa: SLF001
        bridge._stop_mode = mode  # noqa: SLF001
        bridge._wait_for_packet = lambda predicate, timeout, *, honour_stop=True: frames.decode_feedback(packet_of(robot_mode=4))  # type: ignore[method-assign]  # noqa: SLF001
        bridge._release_software_stop = lambda: None  # type: ignore[method-assign]  # noqa: SLF001
        with bridge._lock:  # noqa: SLF001
            bridge._estop, bridge._stop_delivered, bridge._goal = True, True, [1.0] * 6  # noqa: SLF001
        bridge._last_refusal = "command:1:x"  # noqa: SLF001
        code = bridge.release_stop()
        check(code == rhal.SKR_OK and (bridge._estop, bridge._stop_delivered, bridge._goal, bridge._last_refusal) == (False, False, None, None), f"a release that saw the stop taken clears the latch, the delivery, the goal and the last refusal ({mode})")  # noqa: SLF001
    reply = frames.Reply(-1, "", "Stop()")
    bridge = lifeless_bridge()
    bridge._command = lambda text, budget=None, *a, **k: reply  # type: ignore[method-assign]  # noqa: SLF001
    for name, call in (("a stop the controller answered with an error", bridge._deliver_stop), ("the release of a software emergency stop that it answered so", bridge._release_software_stop)):  # noqa: SLF001
        code, message = None, ""
        try:
            call()
        except bridge_module.BridgeRefusal as refusal:
            code, message = refusal.code, str(refusal)
        check(code == rhal.SKR_EIO and "with error -1" in message, f"{name} is an I/O error that says what the controller said", message)
    probe = DobotBridge()
    probe._open_wake()  # noqa: SLF001

    def spam() -> None:
        for _ in range(150_000):
            probe._wake()  # noqa: SLF001

    thread = threading.Thread(target=spam, daemon=True)
    thread.start()
    thread.join(20.0)
    finished = not thread.is_alive()
    if not finished:
        probe._close_wake()  # noqa: SLF001 - lets a writer that is blocked on a full pipe go
        thread.join(2.0)
    check(finished, "a stop wakes the reading thread without waiting for it, however many wakes have not been read: the pipe's write end never blocks")
    probe._close_wake()  # noqa: SLF001
    probe._open_wake()  # noqa: SLF001
    os.write(probe._wake_w, b"x" * 300)  # noqa: SLF001
    DobotBridge._drain_wake(probe._wake_r)  # noqa: SLF001
    check(raises(BlockingIOError, os.read, probe._wake_r, 1) is not None, "and the reading thread drains the pipe whole, however much is in it")  # noqa: SLF001
    probe._close_wake()  # noqa: SLF001
    bridge = lifeless_bridge()
    owed: list[int] = []
    bridge._deliver_stop = lambda: owed.append(1)  # type: ignore[method-assign]  # noqa: SLF001
    with bridge._lock:  # noqa: SLF001
        bridge._estop, bridge._stop_delivered = True, False  # noqa: SLF001
    bridge._stop_retry_at = 10.0  # noqa: SLF001
    with mock.patch.object(time, "monotonic", return_value=10.0):
        bridge._deliver_if_owed()  # noqa: SLF001
    check(owed == [1], "a stop whose retry time has come is written")
    with mock.patch.object(time, "monotonic", return_value=9.99):
        bridge._deliver_if_owed()  # noqa: SLF001
    check(owed == [1], "and one whose time has not is not")
    owed.clear()
    bridge._stop_retry_at = 0.0  # noqa: SLF001
    bridge._observe(frames.decode_feedback(packet_of(robot_mode=5)))  # noqa: SLF001
    check(owed == [1], f"a reading that shows the arm well still writes a stop that is owed ({len(owed)})")
    with bridge._lock:  # noqa: SLF001
        bridge._alarms, bridge._alarms_read_at, bridge._alarms_for_mode = [17], 5.0, 9  # noqa: SLF001
    bridge._observe(frames.decode_feedback(packet_of(robot_mode=5)))  # noqa: SLF001
    check((bridge._alarms, bridge._alarms_read_at, bridge._alarms_for_mode) == ([], 0.0, 0), "and forgets the alarms of an arm that is well, and when they were read")  # noqa: SLF001
    reads: list[int] = []
    bridge._read_alarms = lambda mode: reads.append(mode)  # type: ignore[method-assign]  # noqa: SLF001
    bridge._tcp = True  # noqa: SLF001
    alarmed = frames.decode_feedback(packet_of(robot_mode=9))
    for now, held, what in ((10.5, False, "exactly half a second after the last"), (10.51, True, "more than half a second after")):
        with bridge._lock:  # noqa: SLF001
            bridge._alarms_read_at, bridge._alarms_for_mode = 10.0, 9  # noqa: SLF001
        del reads[:]
        with mock.patch.object(time, "monotonic", return_value=now):
            bridge._observe(alarmed)  # noqa: SLF001
        check(bool(reads) is held, f"an alarm list is read again {what}" if held else f"an alarm list is not read again {what}")


def survivors_dashboard2() -> None:
    section("the dashboard — a reopening that has its own clock, and the edges of the words that make it")
    class Stub:
        def __init__(self, is_open: bool) -> None:
            self.is_open, self.opened = is_open, []

        def open(self, timeout: float = 1.0) -> None:
            self.opened.append(timeout)

    bridge = lifeless_bridge()
    bridge._timeout = 0.12  # noqa: SLF001
    stub = Stub(True)
    bridge._dashboard = stub  # type: ignore[assignment]  # noqa: SLF001
    bridge._reopen_dashboard()  # noqa: SLF001
    check(stub.opened == [], "a dashboard that is open is left alone")
    bridge._dashboard = None  # noqa: SLF001
    bridge._reopen_dashboard()  # noqa: SLF001
    check(True, "and so is none")
    stub = Stub(False)
    bridge._dashboard = stub  # type: ignore[assignment]  # noqa: SLF001
    bridge._dashboard_reopened = 10.0  # noqa: SLF001
    with mock.patch.object(time, "monotonic", return_value=10.49):
        bridge._reopen_dashboard()  # noqa: SLF001
    check(stub.opened == [], "a dashboard that was reopened less than half a second ago is not reopened")
    with mock.patch.object(time, "monotonic", return_value=10.5):
        bridge._reopen_dashboard()  # noqa: SLF001
    check(stub.opened == [0.12] and bridge._dashboard_reopened == 10.5, f"and one that was, half a second ago, is, for the transaction bound at most ({stub.opened})")  # noqa: SLF001
    check("does not list" in frames.describe_error(-30000) and "does not list" in frames.describe_error(-29999) and "does not list" in frames.describe_error(-30100) and "(parameter 99)" in frames.describe_error(-30099)
          and "(parameter 1)" in frames.describe_error(-30001) and "(parameter 99)" in frames.describe_error(-60099) and "does not list" in frames.describe_error(-40000),
          "a parameter family has ninety-nine ErrorIDs under its base and the base itself is not one of them")
    again = frames.decode_feedback(frames.encode_feedback())
    check(again.current_command_id == 0 and again.load_kg == 0.0 and again.motor_temperatures == (30.0,) * 6 and again.timestamp_ms == 0, "a packet made with nothing said has no command id and no load")

class ScriptedLine:
    """link's clock and link's reads, both the test's own: time moves only when the stream waits, and what arrives, arrives at the times it is given, so a byte timeout is met exactly and nothing depends on the machine."""

    TICK = 0.0009765625          # a wait that finds nothing still takes a moment (a binary fraction, so sums are exact)

    def __init__(self, arrivals: list[tuple[float, bytes]]) -> None:
        self.now = 100.0
        self.arrivals = sorted((100.0 + at, data) for at, data in arrivals)
        self.limits: list[float] = []

    def monotonic(self) -> float:
        return self.now

    def read(self, wait: float) -> bytes:
        self.limits.append(wait)
        end = self.now + max(wait, 0.0)
        if self.arrivals and self.arrivals[0][0] <= end:
            at, data = self.arrivals.pop(0)
            self.now = max(self.now, at)
            return data
        self.now = max(end, self.now + self.TICK)
        return b""

    def stream(self) -> link.FeedbackStream:
        stream = link.FeedbackStream("127.0.0.1", 1)

        class Closable:
            def close(self) -> None:
                return None

        stream._sock = Closable()  # type: ignore[assignment]  # noqa: SLF001
        stream._read = self.read  # type: ignore[method-assign]  # noqa: SLF001
        return stream

    def patched(self):
        clock = type("Clock", (), {"monotonic": staticmethod(self.monotonic)})
        return mock.patch.object(link, "time", clock)


def survivors_feedback_clock() -> None:
    section("the feedback stream — a packet that stops half way, the byte timeout met exactly, and a wait that is a look")
    packet = packet_of(robot_mode=5)
    check(link.BYTE_TIMEOUT_S == 0.05, "the byte timeout of a half packet is fifty milliseconds, this driver's own figure")
    with mock.patch.object(link, "BYTE_TIMEOUT_S", 0.5):
        line = ScriptedLine([(0.0, packet[:100])])
        stream = line.stream()
        with line.patched():
            message = raises(link.LinkError, stream.read_packet, 2.0) or ""
        check("stopped sending 100 bytes" in message and line.limits == [2.0, 0.5, 0.0], f"a packet that stops half way is given up on once the byte timeout has gone by and not before: the waits were {line.limits}", message)
        check(stream._buffer == b"" and stream._partial_since is None, "and nothing of it is left to be taken for the start of the next")  # noqa: SLF001
        line = ScriptedLine([(0.0, packet[:100]), (0.375, packet[100:200]), (0.75, packet[200:300])])
        stream = line.stream()
        with line.patched():
            message = raises(link.LinkError, stream.read_packet, 3.0) or ""
        check("stopped sending 200 bytes" in message, "a packet that trickles in is given up on by the time since its first byte and not the time since its last", message)
        line = ScriptedLine([])
        stream = line.stream()
        with line.patched():
            looked = stream.read_packet(0.5)
        check(looked is None and line.limits == [0.5], f"a wait that finds nothing is over when its time is: one look for the whole wait, not a second ({line.limits})")
    stream = link.FeedbackStream("127.0.0.1", 1)
    stream._buffer = b"\xaa" * 6000  # noqa: SLF001
    popped, dropped = stream._pop()  # noqa: SLF001
    check(popped is None and dropped == 6000 - frames.FEEDBACK_BYTES + 1 and len(stream._buffer) == frames.FEEDBACK_BYTES - 1, f"noise is counted a byte at a time as it is dropped ({dropped})")  # noqa: SLF001
    check(link.Dashboard._take(b";") == (b";", True) and link.Dashboard._take(b"\n") == (b"\n", True) and link.Dashboard._take(b"0,{},X();0,{},Y();") == (b"0,{},X();", True) and link.Dashboard._take(b"0,{},X()") == (b"", False),
          "a reply ends at its semicolon or its newline, wherever the first of them is, even the first byte")


def survivors_dashboard_write() -> None:
    section("the dashboard's write — it takes the time it was given and not more, and a time that has gone is gone")
    a, b = socket.socketpair()
    try:
        a.settimeout(None)
        dashboard = link.Dashboard("127.0.0.1", 1)
        dashboard._sock = a  # noqa: SLF001
        outcome: list[str] = []

        def push() -> None:
            try:
                dashboard._write(b"x" * (32 * 1024 * 1024), time.monotonic() + 0.2)  # noqa: SLF001
                outcome.append("sent")
            except link.LinkError as error:
                outcome.append("late: " + str(error))
            except OSError as error:
                outcome.append("os: " + str(error))

        thread = threading.Thread(target=push, daemon=True)
        thread.start()
        thread.join(5.0)
        finished = not thread.is_alive()
        if not finished:
            b.close()               # lets a writer that is blocked on a full buffer go
            thread.join(2.0)
        check(finished and len(outcome) == 1 and outcome[0].startswith("late:") and "did not take the request in time" in outcome[0], f"a peer that takes nothing is given up on at the deadline, and the write does not wait for it ({outcome})")
    finally:
        a.close()
        b.close()
    a, b = socket.socketpair()
    try:
        dashboard = link.Dashboard("127.0.0.1", 1)
        dashboard._sock = a  # noqa: SLF001
        with mock.patch.object(time, "monotonic", return_value=10.0):
            message = raises(link.LinkError, dashboard._write, b"x", 10.0) or ""  # noqa: SLF001
            sent = b.recv(8, socket.MSG_DONTWAIT) if not message else b""
        check("did not take the request in time" in message and sent == b"", "a write whose time is exactly gone writes nothing", message)
    finally:
        a.close()
        b.close()

class LoopRig:
    """The reading thread's loop, run by the test itself on a clock and a select of its own: time passes only when the loop waits, by a binary fraction of a second, so a boundary is met exactly and nothing depends on the machine."""

    TICK = 0.125

    def __init__(self, bridge: DobotBridge, feedback, *, limit: int = 300) -> None:
        self.bridge, self.feedback, self.limit = bridge, feedback, limit
        self.now = 1000.0
        self.timeouts: list[float | None] = []
        self.sleeps: list[float] = []
        self.stop_when = lambda: False
        self.crash: BaseException | None = None

    def monotonic(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.sleeps.append(self.now)
        self.now += self.TICK
        self._bound()

    def select(self, readable, writable, errors, timeout=None):
        self.timeouts.append(timeout)
        self.now += self.TICK
        self._bound()
        return [], [], []

    def _bound(self) -> None:
        if len(self.timeouts) + len(self.sleeps) >= self.limit or self.stop_when():
            self.bridge._closing.set()  # noqa: SLF001

    def run(self) -> None:
        clock = type("Clock", (), {"monotonic": staticmethod(self.monotonic), "sleep": staticmethod(self.sleep)})
        chooser = type("Chooser", (), {"select": staticmethod(self.select)})
        wake_r, wake_w = os.pipe()
        os.set_blocking(wake_r, False)
        self.bridge._closing.clear()  # noqa: SLF001
        try:
            with mock.patch.object(bridge_module, "time", clock), mock.patch.object(bridge_module, "select", chooser):
                try:
                    self.bridge._exchange_loop(wake_r)  # noqa: SLF001
                except BaseException as error:  # noqa: BLE001 - what the thread would have died of
                    self.crash = error
        finally:
            os.close(wake_r)
            os.close(wake_w)


class ScriptedFeedback:
    """What the loop reads from: a stream that is open or is not, answers each look as it is told, and opens or does not."""

    target = "scripted:1"

    def __init__(self, on_read, *, is_open: bool = True, on_open=None) -> None:
        self.on_read, self.is_open, self.on_open = on_read, is_open, on_open
        self._sock = object()

    def read_packet(self, wait: float):
        return self.on_read()

    def open(self, timeout: float = 1.0) -> None:
        if self.on_open is not None:
            self.on_open(timeout)

    def close(self) -> None:
        self.is_open = False


class CountingCondition(threading.Condition):
    def __init__(self, lock) -> None:
        super().__init__(lock)
        self.notified = 0

    def notify_all(self) -> None:
        self.notified += 1
        super().notify_all()


def survivors_loop() -> None:
    section("the reading thread's loop — how long it waits, when a silent line is called silent, when a shut one is opened again, and what it leaves alone")
    for rate, wanted in ((100.0, 0.01), (10.0, 0.05)):
        bridge = lifeless_bridge()
        bridge._dashboard, bridge._rate_hz = None, rate  # noqa: SLF001
        rig = LoopRig(bridge, None, limit=4)
        rig.feedback = ScriptedFeedback(lambda: None)
        bridge._feedback = rig.feedback  # noqa: SLF001
        rig.run()
        check(rig.crash is None and rig.timeouts[:1] == [wanted], f"at {rate:g} Hz the loop waits for the feedback a period, or fifty milliseconds if that is shorter ({rig.timeouts[:1]}, wanted {wanted})", repr(rig.crash))
    bridge = lifeless_bridge()
    bridge._dashboard, bridge._timeout = None, 0.25  # noqa: SLF001
    rig = LoopRig(bridge, None)
    rig.feedback = ScriptedFeedback(lambda: None)
    bridge._feedback = rig.feedback  # noqa: SLF001
    rig.stop_when = lambda: bool(rig.sleeps)
    rig.run()
    check(rig.crash is None and rig.sleeps and rig.sleeps[0] - 1000.0 == 0.875, f"a line that sends nothing is called silent when it has sent nothing for more than three transactions (0.75 s here), not at three and not before ({[s - 1000.0 for s in rig.sleeps[:1]]})", repr(rig.crash))
    bridge = lifeless_bridge()
    bridge._dashboard, bridge._timeout = None, 0.01  # noqa: SLF001
    rig = LoopRig(bridge, None)
    rig.feedback = ScriptedFeedback(lambda: None)
    bridge._feedback = rig.feedback  # noqa: SLF001
    rig.stop_when = lambda: bool(rig.sleeps)
    rig.run()
    check(rig.crash is None and rig.sleeps and rig.sleeps[0] - 1000.0 == 0.375, f"and never before a quarter of a second, however short the transaction ({[s - 1000.0 for s in rig.sleeps[:1]]})", repr(rig.crash))

    bridge = lifeless_bridge()
    bridge._dashboard, bridge._timeout = None, 0.25  # noqa: SLF001
    bridge._changed = CountingCondition(bridge._lock)  # type: ignore[assignment]  # noqa: SLF001
    attempts: list[float] = []
    rig = LoopRig(bridge, None)

    def refuse(timeout: float) -> None:
        attempts.append(rig.now - 1000.0)
        if len(attempts) >= 3:
            bridge._closing.set()  # noqa: SLF001
        raise link.LinkError("scripted:1 refused the connection.")

    def closed() -> None:
        raise link.LinkError("scripted:1 is not open.")

    rig.feedback = ScriptedFeedback(closed, is_open=False, on_open=refuse)
    bridge._feedback = rig.feedback  # noqa: SLF001
    rig.run()
    check(rig.crash is None and attempts == [0.125, 0.625, 1.125], f"a feedback line that is shut is opened again at the first failure and then every half second, at the half second and not after it ({attempts})", repr(rig.crash))
    check(bridge._dead == "scripted:1 refused the connection." and bridge._failures >= 3 and bridge._changed.notified >= bridge._failures, f"and what it says while it stays shut is what it was refused with, and every failure tells whoever is waiting ({bridge._dead!r}, {bridge._failures} failures, {bridge._changed.notified} told)")  # noqa: SLF001

    class DashboardStub:
        def __init__(self, is_open: bool) -> None:
            self.is_open, self.opened = is_open, []

        def open(self, timeout: float = 1.0) -> None:
            self.opened.append(timeout)

    good = packet_of(robot_mode=5, enable_status=1)
    for closing_in_read, what, wanted in ((True, "closing", 0), (False, "not closing", 1)):
        bridge = lifeless_bridge()
        stub = DashboardStub(False)
        bridge._dashboard, bridge._timeout = stub, 0.25  # type: ignore[assignment]  # noqa: SLF001
        reads = []

        def good_packet(_closing=closing_in_read):
            reads.append(1)
            if _closing or len(reads) > 1:
                bridge._closing.set()  # noqa: SLF001
            return good

        rig = LoopRig(bridge, None)
        rig.feedback = ScriptedFeedback(good_packet)
        bridge._feedback = rig.feedback  # noqa: SLF001
        rig.run()
        check(rig.crash is None and len(stub.opened) == wanted, f"a dashboard that has shut is opened again by the loop while the driver is not closing, and left shut while it is ({what}: {len(stub.opened)} openings)", repr(rig.crash))
    bridge = lifeless_bridge()
    bridge._dashboard = None  # noqa: SLF001
    reads = []

    def good_twice():
        reads.append(1)
        if len(reads) > 1:
            bridge._closing.set()  # noqa: SLF001
        return good

    rig = LoopRig(bridge, None)
    rig.feedback = ScriptedFeedback(good_twice)
    bridge._feedback = rig.feedback  # noqa: SLF001
    rig.run()
    check(rig.crash is None and len(reads) == 2, f"a loop with no dashboard (the driver is being closed) goes on reading and does not fall over ({rig.crash!r})")


def survivors_motion_waits() -> None:
    section("the motions' waits — how long each is given, what counts as having moved, and an arrival exactly at its tolerance")
    sim = new_sim(enabled=False)
    try:
        sim.place((30.0, -20.0, 10.0, 0.0, 0.0, 0.0))
        until(lambda: not sim.moving, 3.0)
        bridge = opened(sim, timeout_ms=120)
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK or bridge._latest is not None, 5.0), "open")  # noqa: SLF001
        waits: list[tuple[object, float]] = []
        real = bridge._wait_for_packet  # noqa: SLF001

        def spy(predicate, timeout, **kwargs):
            waits.append((predicate, timeout))
            return real(predicate, timeout, **kwargs)

        bridge._wait_for_packet = spy  # type: ignore[method-assign]  # noqa: SLF001
        code, body = ext(bridge, "enable")
        check(code == rhal.SKR_OK and waits and waits[0][1] == bridge._timeout * 6 + 2.0, f"an enable waits for the reading that shows the arm enabled for the transaction bound six times and two seconds ({waits[:1] and waits[0][1]})", str(body))  # noqa: SLF001
        check(ready(bridge), "enabled")
        del waits[:]
        sim.raise_alarm(17)
        check(until(lambda: bridge._alarms == [17], 5.0), "the alarm is read")  # noqa: SLF001
        code, body = ext(bridge, "clear_errors")
        check(waits and waits[0][1] == bridge._timeout * 6 + 1.0, f"a clear waits for the reading that shows the alarm gone for the bound six times and a second ({waits[:1] and waits[0][1]})", str(body))  # noqa: SLF001
        check(until(lambda: sim.mode in (4, 5), 3.0), "cleared")
        code, body = ext(bridge, "enable")
        check(ready(bridge), "and enabled again")
        until(lambda: not sim.moving, 3.0)
        del waits[:]
        bridge._wait_for_packet = lambda predicate, timeout, **kwargs: (waits.append((predicate, timeout)), frames.decode_feedback(packet_of(robot_mode=5, enable_status=1, q_actual=(0.0, -28.64788975654116, 10.0, 0.0, 0.0, 0.0))))[1]  # type: ignore[method-assign]  # noqa: SLF001
        code, body = ext(bridge, "move j1=0.0 j2=-0.5 v=1")
        check(code == rhal.SKR_OK, "a move at one percent is a move: the speed's range starts at 1", str(body))
        distance = max(abs(0.0 / DEG - 30.0), abs(-0.5 / DEG - -20.0))
        check(waits and abs(waits[0][1] - (distance / 10.0 + 5.0)) < 1e-6, f"a move waits for the farthest joint's distance in degrees over ten, and five seconds ({waits[:1] and waits[0][1]}, wanted {distance / 10.0 + 5.0:.4f})", str(body))
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim()
    try:
        bridge = opened(sim, timeout_ms=120)
        check(ready(bridge), "ready")
        captured: list[object] = []
        far = frames.decode_feedback(packet_of(robot_mode=5, enable_status=1, q_actual=(0.0,) * 6))

        def hand_back(after):
            def stub(predicate, timeout, **kwargs):
                captured.append(predicate)
                return after
            return stub

        here = (0.0,) * 6

        def at_rest() -> None:
            sim.place(here)
            until(lambda: not sim.moving and bridge._latest is not None and tuple(bridge._latest.q_actual) == here, 3.0)  # noqa: SLF001

        at_rest()
        cases = {
            "not yet moved: the arm is where it started and the goal is elsewhere": (dict(q_actual=here), False),
            "moved on one of two joints": (dict(q_actual=(here[0] + 10.0,) + here[1:]), True),
        }
        goal_rad = 0.349066
        text = f"move j1={goal_rad} j2={here[1] * DEG}"
        for what, (fields, wanted) in cases.items():
            bridge._wait_for_packet = hand_back(far)  # type: ignore[method-assign]  # noqa: SLF001
            del captured[:]
            at_rest()
            ext(bridge, text)
            predicate = captured[-1] if captured else None
            packet = frames.decode_feedback(packet_of(robot_mode=5, enable_status=1, **fields))
            check(predicate is not None and predicate(packet) is wanted, f"the move is not done when the arm has {what}" if not wanted else f"the move is done when the arm has {what}: it started, and it is not running")  # type: ignore[operator]
        bridge._wait_for_packet = hand_back(far)  # type: ignore[method-assign]  # noqa: SLF001
        del captured[:]
        at_rest()
        ext(bridge, text)
        predicate = captured[-1]
        running = frames.decode_feedback(packet_of(robot_mode=7, enable_status=1, running_status=1, q_actual=here))
        stopped_short = frames.decode_feedback(packet_of(robot_mode=5, enable_status=1, q_actual=(here[0] + 0.25,) + here[1:]))
        check(predicate(running) is False and predicate(stopped_short) is True, "a move that was seen running and has stopped is done, wherever it stopped")  # type: ignore[operator]
        bridge._wait_for_packet = hand_back(frames.decode_feedback(packet_of(robot_mode=5, enable_status=1, q_actual=(0.5,) + here[1:])))  # type: ignore[method-assign]  # noqa: SLF001
        at_rest()
        code, body = ext(bridge, "move j1=0.0")
        check(code == rhal.SKR_OK and isinstance(body, dict) and body.get("result") == "arrived", "an arm half a degree from its goal has arrived: that is the tolerance, and it is inclusive", str(body))
        bridge._wait_for_packet = hand_back(frames.decode_feedback(packet_of(robot_mode=5, enable_status=1, q_actual=(0.5009765625,) + here[1:])))  # type: ignore[method-assign]  # noqa: SLF001
        at_rest()
        code, body = ext(bridge, "move j1=0.0")
        check(code == rhal.SKR_OK and isinstance(body, dict) and body.get("result") == "stopped_short", "and a little more is stopped short", str(body))
        bridge.close()
    finally:
        sim.stop()


def survivors_triage3() -> None:
    section("what a command holds the other joints at, which alarm list is current, how long a release gives each line, and when a move of two joints has arrived")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.freeze(True)
        check(until(lambda: len(servo_texts(sim)) == 0), "an arm whose reading stays where it was while commands are sent")
        code, _ = write(bridge, cmd(pose(0.5), mask=[1, 0, 0, 0, 0, 0]))
        check(code == rhal.SKR_OK and until(lambda: len(servo_texts(sim)) == 1), "j1 alone is sent")
        code, _ = write(bridge, cmd(pose(0.2), mask=[0, 1, 0, 0, 0, 0]))
        check(code == rhal.SKR_OK and until(lambda: len(servo_texts(sim)) == 2), "then j2 alone")
        second = [float(v) for v in servo_texts(sim)[1][len("ServoJ("):servo_texts(sim)[1].index(")")].split(",")[:6]]
        check(abs(second[0] - 0.5 / DEG) < 1e-3 and abs(second[1] - 0.2 / DEG) < 1e-3, "holds j1 where the driver last asked it to be, and not where the frozen reading says it is", str(second))
        bridge.close()
    finally:
        sim.stop()
    bridge = lifeless_bridge()
    calls: list[int] = []
    bridge._read_alarms = lambda mode: calls.append(mode)  # type: ignore[method-assign]  # noqa: SLF001
    with bridge._lock:  # noqa: SLF001
        bridge._alarms_read_at, bridge._alarms_for_mode = time.monotonic(), frames.MODE_COLLISION  # noqa: SLF001
    bridge._observe(set_reading(bridge, robot_mode=frames.MODE_COLLISION, error_status=1))  # noqa: SLF001
    check(calls == [], "an alarm list read a moment ago for the mode the arm is in is current")
    bridge._observe(set_reading(bridge, robot_mode=frames.MODE_ERROR, error_status=1))  # noqa: SLF001
    check(calls == [frames.MODE_ERROR], "and one read for another mode is not, however recent: an arm that goes from a collision to an alarm has its list read at once", str(calls))
    with bridge._lock:  # noqa: SLF001
        bridge._alarms_read_at = time.monotonic() - 1.0  # noqa: SLF001
        bridge._alarms_for_mode = frames.MODE_ERROR  # noqa: SLF001
    bridge._observe(set_reading(bridge, robot_mode=frames.MODE_ERROR, error_status=1))  # noqa: SLF001
    check(calls == [frames.MODE_ERROR, frames.MODE_ERROR], "and one read a second ago for the same mode is read again")
    bridge = lifeless_bridge()
    budgets: list[tuple[str, float | None]] = []
    bridge._timeout = 0.12  # noqa: SLF001
    bridge._command = lambda text, budget=None, *a, **k: (budgets.append((text, budget)), frames.Reply(0, "", text))[1]  # type: ignore[method-assign]  # noqa: SLF001
    bridge._release_software_stop()  # noqa: SLF001
    check([text for text, _ in budgets] == ["EmergencyStop(0)", "ClearError()"] and all(budget is not None and abs(budget - 0.36) < 1e-9 for _, budget in budgets), f"a release gives each of its two lines three times the transaction bound ({budgets})")
    sim = new_sim()
    try:
        bridge = opened(sim, timeout_ms=120)
        check(ready(bridge), "ready")
        here = (0.0,) * 6
        sim.place(here)
        until(lambda: not sim.moving and bridge._latest is not None and tuple(bridge._latest.q_actual) == here, 3.0)  # noqa: SLF001
        for what, after in (("j1 at its goal and j2 far from it", (0.0, 3.0, 0.0, 0.0, 0.0, 0.0)), ("j2 at its goal and j1 far from it", (3.0, 28.6478897565, 0.0, 0.0, 0.0, 0.0))):
            bridge._wait_for_packet = lambda predicate, timeout, _after=after, **kw: frames.decode_feedback(packet_of(robot_mode=5, enable_status=1, q_actual=_after))  # type: ignore[method-assign]  # noqa: SLF001
            code, body = ext(bridge, "move j1=0.0 j2=0.5")
            check(code == rhal.SKR_OK and isinstance(body, dict) and body.get("result") == "stopped_short", f"a move of two joints that ends with {what} has stopped short, not arrived", str(body))
        bridge._wait_for_packet = lambda predicate, timeout, **kw: frames.decode_feedback(packet_of(robot_mode=5, enable_status=1, q_actual=(0.2, 28.6, 0.0, 0.0, 0.0, 0.0)))  # type: ignore[method-assign]  # noqa: SLF001
        code, body = ext(bridge, "move j1=0.0 j2=0.5")
        check(code == rhal.SKR_OK and isinstance(body, dict) and body.get("result") == "arrived", "and one that ends with both within the tolerance has arrived", str(body))
        bridge.close()
    finally:
        sim.stop()


def main_survivors() -> None:
    """What a seeded-defect campaign found the suite let through, one group at a time."""
    survivors_links()
    survivors_stop()
    survivors_commands()
    survivors_threads()
    survivors_motions()
    survivors_open()
    survivors_status()
    survivors_session()
    survivors_open_settings()
    survivors_commands2()
    survivors_stop2()
    survivors_dashboard2()
    survivors_feedback_clock()
    survivors_dashboard_write()
    survivors_loop()
    survivors_motion_waits()
    survivors_triage3()
# ── survivors end ──


def main() -> int:
    if os.environ.get("SELFTEST_ONLY") == "survivors":          # the group the seeded-defect campaign added, on its own: a mutant is tried against it in seconds
        main_survivors()
        print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
        for failure in FAILURES:
            print(f"  FAILED: {failure}")
        return 1 if FAILURES else 0
    main_frames()
    main_links()
    main_open()
    main_state()
    main_commands()
    main_stop()
    main_motions()
    main_session()
    main_survivors()
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAILED: {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
