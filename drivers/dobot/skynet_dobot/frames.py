"""The wire of a Dobot V4 controller (CR, CRA, E6 and Nova arms): the dashboard's text protocol on port 29999 and the 1440-byte real-time feedback on 30004, as Dobot's TCP/IP interface document V4.6.6 gives them
(DRY-COVERAGE-PLAN.md item 12.E.13).

Two things on one controller, and this module is both: **the text protocol**, whose commands are `Name(param,param,...)` and whose replies are `ErrorID,{value,...},Name(param,...);`, and **the feedback packet**, 1440 bytes
of little-endian fields at fixed offsets, a new one every 8 ms on 30004. Everything here is Dobot's: the command names, the error codes, the mode numbers and the packet layout come from the interface document, and the
maker's own Python client (`Dobot-Arm/TCP-IP-Python-V4`, MIT) is what `layout_check.py` runs to hold these bytes to something that was not written here.

Pure: no I/O, no clock, nothing imported but the standard library.
"""

from __future__ import annotations

import math
import struct
from dataclasses import dataclass

__all__ = [
    "ProtocolError", "DASHBOARD_PORT", "FEEDBACK_PORTS", "FEEDBACK_BYTES", "FEEDBACK_MAGIC", "JOINTS", "MODES", "MODE_NAMES", "ROBOT_TYPES", "ERRORS", "Reply", "Feedback", "request_control", "power_on", "enable_robot",
    "disable_robot", "clear_error", "stop", "emergency_stop", "robot_mode", "get_angle", "get_error_id", "speed_factor", "servo_j", "mov_j", "parse_reply", "numbers", "error_ids", "describe_error", "decode_feedback",
    "encode_feedback", "SERVO_T", "SERVO_AHEAD", "SERVO_GAIN", "MODE_ENABLE", "MODE_RUNNING", "MODE_SINGLE_MOVE", "MODE_ERROR", "MODE_DISABLED", "MODE_POWEROFF", "MODE_COLLISION", "MODE_PAUSE", "MODE_INIT",
    "MODE_BRAKE_OPEN", "MODE_BACKDRIVE", "NOT_TCP",
]

DASHBOARD_PORT = 29999
#: 30004 is the real-time feedback (every 8 ms), 30005 every 200 ms and 30006 every 1000 ms by default; all three send the same 1,440 bytes.
FEEDBACK_PORTS = (30004, 30005, 30006)
FEEDBACK_BYTES = 1440
#: The packet's self-test field, "0x0123 4567 89AB CDEF" in the document, at byte 48.
FEEDBACK_MAGIC = 0x0123456789ABCDEF

JOINTS = ("j1", "j2", "j3", "j4", "j5", "j6")

#: The text a controller answers with, instead of an `ErrorID,{...},Name(...);` reply, while its control mode is not TCP. The maker's own client matches it by this text (`error_controller.parse_error_ids`); the document
#: says nothing of it beyond "only in TCP mode can other TCP commands be executed".
NOT_TCP = "Control Mode Is Not Tcp"

MODE_INIT, MODE_BRAKE_OPEN, MODE_POWEROFF, MODE_DISABLED, MODE_ENABLE, MODE_BACKDRIVE, MODE_RUNNING, MODE_SINGLE_MOVE, MODE_ERROR, MODE_PAUSE, MODE_COLLISION = range(1, 12)
#: RobotMode's values as the interface document gives them. **The maker's own client names 11 `JOG` and adds a 0 `UNKNOWN`; the document names 11 `COLLISION`, "collision detection triggered"**, and this driver reads 11 as the
#: document does (a fault) and pins the disagreement in `layout_check.py`.
MODES = {1: "initialising", 2: "a brake is open", 3: "powered off", 4: "disabled", 5: "enabled and idle", 6: "drag mode", 7: "running", 8: "a single move", 9: "alarm", 10: "paused", 11: "collision detected"}
MODE_NAMES = {1: "ROBOT_MODE_INIT", 2: "ROBOT_MODE_BRAKE_OPEN", 3: "ROBOT_MODE_POWEROFF", 4: "ROBOT_MODE_DISABLED", 5: "ROBOT_MODE_ENABLE", 6: "ROBOT_MODE_BACKDRIVE", 7: "ROBOT_MODE_RUNNING",
              8: "ROBOT_MODE_SINGLE_MOVE", 9: "ROBOT_MODE_ERROR", 10: "ROBOT_MODE_PAUSE", 11: "ROBOT_MODE_COLLISION"}

#: `CRRobotType`, the byte at 1031 of the feedback packet, as the document's table gives it.
ROBOT_TYPES = {3: "CR3", 5: "CR5", 7: "CR7", 10: "CR10", 12: "CR12", 16: "CR16", 101: "Nova 2", 103: "Nova 5", 113: "CR3A", 115: "CR5A", 116: "CR5AF", 117: "CR7A", 120: "CR10A", 121: "CR10AF", 122: "CR12A",
               126: "CR16A", 127: "CR20AF", 130: "CR20A", 150: "Magician E6", 160: "Nova 2 (new)", 161: "Nova 5 (new)", 162: "Nova 2S", 203: "CR3V", 205: "CR5V", 207: "CR7V", 210: "CR10V", 212: "CR12V", 216: "CR16V",
               220: "CR20V"}

#: The general error codes (document, section 4): what an `ErrorID` other than 0 means, for the codes that are one number. The parameter errors are a family (-3000X, -4000X, -5000X, -6000X: the last digit is the
#: parameter's position) and `describe_error` words them.
ERRORS = {
    0: "no error", -1: "the command was received and failed", -2: "the arm is in an alarm state", -3: "the arm is in emergency stop", -4: "the arm is powered off", -5: "the arm is running a script",
    -6: "the jog axis and its coordinate type do not match", -7: "the arm is running a paused script", -8: "the controller's authorisation has expired", -9: "the command is disabled",
    -10000: "the command does not exist", -20000: "the number of parameters is wrong",
}

#: The ranges the document gives `ServoJ`'s optional parameters, and the default of each.
SERVO_T = (0.004, 3600.0, 0.1)
SERVO_AHEAD = (20.0, 100.0, 50.0)
SERVO_GAIN = (200.0, 1000.0, 500.0)


class ProtocolError(Exception):
    """A reply that is not one the interface document allows, or a command whose parameters cannot be sent."""


# ── the commands ─────────────────────────────────────────────────────────────────────────────────

def _finite(*values: float) -> None:
    for value in values:
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
            raise ProtocolError(f"{value!r} is not a finite number.")


def request_control() -> str:
    return "RequestControl()"


def power_on() -> str:
    return "PowerOn()"


def enable_robot(load_kg: float | None = None, center_mm: tuple[float, float, float] | None = None) -> str:
    """`EnableRobot()`, `EnableRobot(load)` or `EnableRobot(load,centerX,centerY,centerZ)`, with six decimals as the maker's client writes them. **The maker's client cannot send a load of zero** (it sends
    `EnableRobot()` for 0.0, which leaves the controller's own payload); this sends the zero it was given."""
    if load_kg is None:
        if center_mm is not None:
            raise ProtocolError("an eccentric centre needs a load.")
        return "EnableRobot()"
    _finite(load_kg)
    if load_kg < 0:
        raise ProtocolError(f"a load of {load_kg} kg is negative.")
    if center_mm is None:
        return f"EnableRobot({load_kg:.6f})"
    if len(center_mm) != 3:
        raise ProtocolError("the load's centre is three distances.")
    _finite(*center_mm)
    return f"EnableRobot({load_kg:.6f},{center_mm[0]:.6f},{center_mm[1]:.6f},{center_mm[2]:.6f},0)"


def disable_robot() -> str:
    return "DisableRobot()"


def clear_error() -> str:
    return "ClearError()"


def stop() -> str:
    return "Stop()"


def emergency_stop(engage: bool) -> str:
    """`EmergencyStop(1)` presses the controller's software emergency stop and `EmergencyStop(0)` releases it."""
    return f"EmergencyStop({1 if engage else 0})"


def robot_mode() -> str:
    return "RobotMode()"


def get_angle() -> str:
    return "GetAngle()"


def get_error_id() -> str:
    return "GetErrorID()"


def speed_factor(percent: int) -> str:
    if isinstance(percent, bool) or not isinstance(percent, int) or not 1 <= percent <= 100:
        raise ProtocolError(f"a speed factor of {percent!r} is not a whole number from 1 to 100.")
    return f"SpeedFactor({percent})"


def servo_j(joints_deg: list[float] | tuple[float, ...], t: float | None = None, aheadtime: float | None = None, gain: float | None = None) -> str:
    """`ServoJ(j1,...,j6[,t=,aheadtime=,gain=])`: the six joint angles in degrees with six decimals, and, where given, the optional parameters as the maker's client writes them (`t` to three decimals, the other two to one),
    each checked against the range the document gives."""
    if not isinstance(joints_deg, (list, tuple)) or len(joints_deg) != 6:
        raise ProtocolError("a ServoJ target is six joint angles.")
    _finite(*joints_deg)
    text = ",".join(f"{angle:.6f}" for angle in joints_deg)
    for name, value, (low, high, _) in (("t", t, SERVO_T), ("aheadtime", aheadtime, SERVO_AHEAD), ("gain", gain, SERVO_GAIN)):
        if value is None:
            continue
        _finite(value)
        if not low <= value <= high:
            raise ProtocolError(f"{name} of {value} is outside {low:g} to {high:g}.")
        text += f",{name}={value:.3f}" if name == "t" else f",{name}={value:.1f}"
    return f"ServoJ({text})"


def mov_j(joints_deg: list[float] | tuple[float, ...], a: int | None = None, v: int | None = None) -> str:
    """`MovJ(joint={j1,...,j6}[,a=][,v=])`: a joint move in degrees, with the acceleration and speed as percentages of the maximum (1 to 100)."""
    if not isinstance(joints_deg, (list, tuple)) or len(joints_deg) != 6:
        raise ProtocolError("a MovJ target is six joint angles.")
    _finite(*joints_deg)
    text = "joint={" + ",".join(f"{angle:.6f}" for angle in joints_deg) + "}"
    for name, value in (("a", a), ("v", v)):
        if value is None:
            continue
        if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= 100:
            raise ProtocolError(f"{name} of {value!r} is not a whole percentage from 1 to 100.")
        text += f",{name}={value}"
    return f"MovJ({text})"


# ── the replies ──────────────────────────────────────────────────────────────────────────────────

@dataclass(frozen=True)
class Reply:
    error: int              # ErrorID: 0 for a command that was received and taken
    payload: str            # what is between the braces, as sent
    echo: str               # the command as the controller says it heard it, `Name(params)`


def parse_reply(text: str) -> Reply:
    """`ErrorID,{value,...},Name(param,...);` as a Reply. ProtocolError for anything else, the text of a controller that is not in TCP mode included (`NOT_TCP`, which the caller tells apart)."""
    body = text.strip()
    if not body.endswith(";"):
        raise ProtocolError(f"a reply ends with a semicolon, and this one does not: {body[:80]!r}.")
    body = body[:-1]
    comma = body.find(",")
    if comma < 1:
        raise ProtocolError(f"a reply begins with an ErrorID and a comma: {body[:80]!r}.")
    head = body[:comma].strip()
    try:
        error = int(head)
    except ValueError:
        raise ProtocolError(f"an ErrorID is a whole number, not {head!r}.") from None
    rest = body[comma + 1:].lstrip()
    if not rest.startswith("{"):
        raise ProtocolError(f"the ErrorID is followed by a value in braces: {rest[:80]!r}.")
    depth, end = 0, -1
    for at, char in enumerate(rest):
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                end = at
                break
    if end < 0:
        raise ProtocolError(f"the braces of a reply's value are not closed: {rest[:80]!r}.")
    tail = rest[end + 1:].lstrip()
    if not tail.startswith(","):
        raise ProtocolError(f"the value is followed by the command, after a comma: {tail[:80]!r}.")
    return Reply(error, rest[1:end], tail[1:].strip())


def numbers(payload: str, count: int | None = None) -> list[float]:
    """The comma-separated numbers of a reply's value, `{a,b,c}`'s inside. ProtocolError for anything that is not a finite number, or for the wrong count."""
    parts = [part.strip() for part in payload.split(",")] if payload.strip() else []
    out: list[float] = []
    for part in parts:
        try:
            value = float(part)
        except ValueError:
            raise ProtocolError(f"{part!r} is not a number.") from None
        if not math.isfinite(value):
            raise ProtocolError(f"{part!r} is not a finite number.")
        out.append(value)
    if count is not None and len(out) != count:
        raise ProtocolError(f"the reply holds {len(out)} numbers and {count} were expected.")
    return out


def error_ids(payload: str) -> list[int]:
    """`[1537,2048]` or `[]` from `GetErrorID`: the alarm identifiers, in the order the controller lists them."""
    text = payload.strip()
    if not (text.startswith("[") and text.endswith("]")):
        raise ProtocolError(f"the alarm list is a bracketed list: {text[:80]!r}.")
    inner = text[1:-1].strip()
    ids: list[int] = []
    for part in (inner.split(",") if inner else []):
        try:
            ids.append(int(part.strip()))
        except ValueError:
            raise ProtocolError(f"{part.strip()!r} is not an alarm identifier.") from None
    return ids


def describe_error(code: int) -> str:
    """The words for an ErrorID. The parameter families say which parameter, and whether it is a mandatory or an optional one, a type or a range."""
    if code in ERRORS:
        return ERRORS[code]
    for base, what in ((-30000, "a mandatory parameter has the wrong type"), (-40000, "a mandatory parameter is out of range"), (-50000, "an optional parameter has the wrong type"), (-60000, "an optional parameter is out of range")):
        if base - 99 <= code <= base - 1:
            return f"{what} (parameter {base - code})"
    return f"error {code}, which the interface document does not list"


# ── the feedback packet ──────────────────────────────────────────────────────────────────────────

#: Offsets in the 1,440 bytes, from the document's table (bytes counted from 0, little-endian). The maker's own client reads the same offsets through a numpy dtype, which `layout_check.py` compares.
_OFFSETS = {
    "size": 0, "digital_inputs": 8, "digital_outputs": 16, "robot_mode": 24, "timestamp": 32, "run_time": 40, "test_value": 48, "speed_scaling": 64, "voltage": 88, "current": 96, "program_state": 104,
    "q_target": 192, "qd_target": 240, "qdd_target": 288, "i_target": 336, "m_target": 384, "q_actual": 432, "qd_actual": 480, "i_actual": 528, "tcp_force_actual": 576, "tool_vector_actual": 624,
    "tcp_speed_actual": 672, "tcp_force": 720, "tool_vector_target": 768, "tcp_speed_target": 816, "motor_temperatures": 864, "joint_modes": 912, "v_actual": 960, "user": 1012, "tool": 1013,
    "run_queued_cmd": 1014, "pause_cmd_flag": 1015, "velocity_ratio": 1016, "acceleration_ratio": 1017, "brake_status": 1025, "enable_status": 1026, "drag_status": 1027, "running_status": 1028,
    "error_status": 1029, "jog_status": 1030, "robot_type": 1031, "collision_state": 1038, "current_command_id": 1112, "m_actual": 1120, "load": 1168, "center_x": 1176, "center_y": 1184, "center_z": 1192,
    "safety_state": 1420,
}


@dataclass(frozen=True)
class Feedback:
    """The fields of the packet this driver reads. Joint values are as the controller sends them: degrees (positions) and the unit the document does not state (the rest)."""
    robot_mode: int
    enable_status: int
    brake_status: int
    error_status: int
    running_status: int
    collision_state: int
    drag_status: int
    robot_type: int
    safety_state: int
    speed_scaling: float
    q_actual: tuple[float, ...]
    q_target: tuple[float, ...]
    qd_actual: tuple[float, ...]
    motor_temperatures: tuple[float, ...]
    timestamp_ms: int
    run_time_ms: int
    current_command_id: int
    digital_inputs: int
    digital_outputs: int
    load_kg: float


def _doubles(packet: bytes, name: str) -> tuple[float, ...]:
    return struct.unpack_from("<6d", packet, _OFFSETS[name])


def decode_feedback(packet: bytes) -> Feedback:
    """The fields of one 1,440-byte packet. ProtocolError for a packet of the wrong length or one whose self-test field is not the document's, and for a joint value that is not a finite number."""
    if len(packet) != FEEDBACK_BYTES:
        raise ProtocolError(f"a feedback packet is {FEEDBACK_BYTES} bytes, not {len(packet)}.")
    magic = struct.unpack_from("<Q", packet, _OFFSETS["test_value"])[0]
    if magic != FEEDBACK_MAGIC:
        raise ProtocolError(f"the packet's self-test field is {magic:#018x} and the document's is {FEEDBACK_MAGIC:#018x}.")
    q_actual, q_target, qd_actual, temperatures = _doubles(packet, "q_actual"), _doubles(packet, "q_target"), _doubles(packet, "qd_actual"), _doubles(packet, "motor_temperatures")
    for name, values in (("QActual", q_actual), ("QTarget", q_target), ("QDActual", qd_actual)):
        if not all(math.isfinite(v) for v in values):
            raise ProtocolError(f"{name} holds a value that is not a number.")
    return Feedback(
        robot_mode=struct.unpack_from("<Q", packet, _OFFSETS["robot_mode"])[0], enable_status=packet[_OFFSETS["enable_status"]], brake_status=packet[_OFFSETS["brake_status"]],
        error_status=packet[_OFFSETS["error_status"]], running_status=packet[_OFFSETS["running_status"]], collision_state=packet[_OFFSETS["collision_state"]], drag_status=packet[_OFFSETS["drag_status"]],
        robot_type=packet[_OFFSETS["robot_type"]], safety_state=packet[_OFFSETS["safety_state"]], speed_scaling=struct.unpack_from("<d", packet, _OFFSETS["speed_scaling"])[0], q_actual=q_actual, q_target=q_target,
        qd_actual=qd_actual, motor_temperatures=temperatures, timestamp_ms=struct.unpack_from("<Q", packet, _OFFSETS["timestamp"])[0], run_time_ms=struct.unpack_from("<Q", packet, _OFFSETS["run_time"])[0],
        current_command_id=struct.unpack_from("<Q", packet, _OFFSETS["current_command_id"])[0], digital_inputs=struct.unpack_from("<Q", packet, _OFFSETS["digital_inputs"])[0],
        digital_outputs=struct.unpack_from("<Q", packet, _OFFSETS["digital_outputs"])[0], load_kg=struct.unpack_from("<d", packet, _OFFSETS["load"])[0])


def encode_feedback(*, robot_mode: int = MODE_DISABLED, enable_status: int = 0, brake_status: int = 0, error_status: int = 0, running_status: int = 0, collision_state: int = 0, drag_status: int = 0, robot_type: int = 5,
                    safety_state: int = 0xFF, speed_scaling: float = 100.0, q_actual: tuple[float, ...] = (0.0,) * 6, q_target: tuple[float, ...] | None = None, qd_actual: tuple[float, ...] = (0.0,) * 6,
                    motor_temperatures: tuple[float, ...] = (30.0,) * 6, timestamp_ms: int = 0, run_time_ms: int = 0, current_command_id: int = 0, digital_inputs: int = 0, digital_outputs: int = 0,
                    load_kg: float = 0.0) -> bytes:
    """One packet, for the simulated controller and for the tests: every field this module reads, at the document's offset, and the self-test field. The rest is zero."""
    packet = bytearray(FEEDBACK_BYTES)
    struct.pack_into("<H", packet, _OFFSETS["size"], FEEDBACK_BYTES)
    struct.pack_into("<Q", packet, _OFFSETS["digital_inputs"], digital_inputs)
    struct.pack_into("<Q", packet, _OFFSETS["digital_outputs"], digital_outputs)
    struct.pack_into("<Q", packet, _OFFSETS["robot_mode"], robot_mode)
    struct.pack_into("<Q", packet, _OFFSETS["timestamp"], timestamp_ms)
    struct.pack_into("<Q", packet, _OFFSETS["run_time"], run_time_ms)
    struct.pack_into("<Q", packet, _OFFSETS["test_value"], FEEDBACK_MAGIC)
    struct.pack_into("<d", packet, _OFFSETS["speed_scaling"], speed_scaling)
    struct.pack_into("<6d", packet, _OFFSETS["q_target"], *(q_target if q_target is not None else q_actual))
    struct.pack_into("<6d", packet, _OFFSETS["q_actual"], *q_actual)
    struct.pack_into("<6d", packet, _OFFSETS["qd_actual"], *qd_actual)
    struct.pack_into("<6d", packet, _OFFSETS["motor_temperatures"], *motor_temperatures)
    packet[_OFFSETS["brake_status"]] = brake_status & 0xFF
    packet[_OFFSETS["enable_status"]] = enable_status & 0xFF
    packet[_OFFSETS["drag_status"]] = drag_status & 0xFF
    packet[_OFFSETS["running_status"]] = running_status & 0xFF
    packet[_OFFSETS["error_status"]] = error_status & 0xFF
    packet[_OFFSETS["robot_type"]] = robot_type & 0xFF
    packet[_OFFSETS["collision_state"]] = collision_state & 0xFF
    struct.pack_into("<Q", packet, _OFFSETS["current_command_id"], current_command_id)
    struct.pack_into("<d", packet, _OFFSETS["load"], load_kg)
    packet[_OFFSETS["safety_state"]] = safety_state & 0xFF
    return bytes(packet)
