"""The wire of a Rainbow Robotics RB controller (RB3, RB5, RB10, RB16 and the rest of the RB series): the command port (5000), which takes one text script per line, and the data port (5001), which answers `reqdata`
with a 580-byte state packet (DRY-COVERAGE-PLAN.md item 12.E.14).

Two things on one controller, and this module is both: **the commands**, which are the controller's own script functions (`move_servo_j(jnt[...], t1, t2, gain, alpha)`, `task stop`, `mc jall init`) sent one to a line and each
answered with an acknowledgement and then, as they happen, `info[...]`, `warn[...]` and `error[...]` lines; and **the state packet**, little-endian fields at fixed offsets. Everything here is Rainbow's: the script grammar
is the UI Script manual 6.10 and Appendix E of the RB Series user manual, the packet is the `reqdata` Structure document (software 4.3.1, MAX_SHARED_DATA 145), and the message codes are the table in the maker's own client
(`rbpodo`, Apache-2.0). `layout_check.py` runs that client to hold this driver's text and packets to something that was not written here.

Pure: no I/O, no clock, nothing imported but the standard library.
"""

from __future__ import annotations

import math
import struct
from dataclasses import dataclass

from .codes import CODES

__all__ = [
    "ProtocolError", "COMMAND_PORT", "DATA_PORT", "ACK", "NOT_ALLOWED", "REQDATA", "PACKET_BYTES", "JOINTS", "CODES", "ESTOP_CODES", "Event", "State", "parse_event", "bracket_depth", "decode_state", "encode_state",
    "servo_j", "move_j", "task_stop", "halt", "task_pause", "task_resume", "activate", "pgmode", "set_payload_info", "get_control_box_info", "print_variable", "describe_code", "number", "MIN_T1", "T2_RANGE", "MODE_REAL",
    "MODE_SIMULATION", "ACTIVATED", "TASK_IDLE", "TASK_PAUSED", "TASK_RUNNING", "ROBOT_IDLE", "SOS_FLAGS", "EMS_FLAGS", "JOINT_ERROR_BITS", "OFFSETS",
]

COMMAND_PORT = 5000
DATA_PORT = 5001
#: What the controller says after every command it parsed (Appendix E: "The command was executed."), whether or not it then did it: a command it refuses is acknowledged and then answered by a `warn` or an `error`.
ACK = "The command was executed"
#: What Appendix E says the command port answers a first word it does not know. **The Virtual Control Box (software 8.4.4) does not**: it acknowledges `hello world`, and answers a syntax error with `error[msg]`.
NOT_ALLOWED = "The command is not allowed."
REQDATA = "reqdata"
PACKET_BYTES = 580
#: `header[0]`, the packet's first byte: 0x24, "$". `header[1]` and `header[2]` are the size without the header (576, little-endian) and `header[3]` the type of the data (3 is the state).
PACKET_START = 0x24
PACKET_TYPE_STATE = 3

JOINTS = ("j1", "j2", "j3", "j4", "j5", "j6")

#: The ranges the UI Script manual 6.10 gives `move_servo_j` (7.34): the angles in degrees from -360 to 360, `t1` (the time to arrive) at least 0.002 s, `t2` (the time the motion is kept after arrival) between 0.02 and 0.2 s
#: exclusive, `gain` above 0 and `alpha` (a low-pass gain) between 0 and 1 exclusive. **The Virtual Control Box accepts a gain and an alpha of 0**, which the manual excludes (`layout_check.py`).
MIN_T1 = 0.002
T2_RANGE = (0.02, 0.2)
ANGLE_LIMIT_DEG = 360.0

#: `real_vs_simulation_mode`'s low four bits: 0 is a real robot and 1 the simulation, in which "only the internal reference is changed" and the arm does not move.
MODE_REAL, MODE_SIMULATION = 0, 1
#: `init_state_info`'s low six bits: the activation stage, 6 when it is done (the maker's client tests exactly this: `(state & 0x3f) == 6`).
ACTIVATED = 6
TASK_IDLE, TASK_PAUSED, TASK_RUNNING = 1, 2, 3
#: `robot_state`: 1 when no motion command is executing, 3 while one is, 5 for a conveyor or force mode and 60 + n or 560 + n at waypoint n of a blended motion.
ROBOT_IDLE = 1

#: `op_stat_sos_flag`: "Robot Arm device error code during operation", 0 for none.
SOS_FLAGS = {1: "encoder error (PVL)", 2: "CPU error", 3: "big error", 4: "input error", 5: "JAM error", 6: "overcurrent error", 7: "position bound error", 8: "mode error", 9: "match error",
             10: "overcurrent or low voltage error", 11: "temperature error", 12: "speed over error"}
#: `op_stat_ems_flag`: "Software (kinematics) emergency stop situation", 0 for none.
EMS_FLAGS = {1: "arm stretch", 2: "Cartesian limit", 3: "joint limit", 4: "unsolvable posture"}
#: The bits of a joint's `jnt_info` word that are an error: the encoder's (Nonius, low battery, calibration mode, multi-turn) from 4 to 7 and the joint's own (JAM, CUR, BIG, INP, FLT, TMP, PS1, PS2) from 8 to 15.
JOINT_ERROR_BITS = {4: "encoder Nonius error", 5: "encoder low battery", 6: "encoder calibration mode", 7: "encoder multi-turn error", 8: "JAM error", 9: "CUR error", 10: "BIG error", 11: "INP error", 12: "FLT error",
                    13: "TMP error", 14: "PS1 error", 15: "PS2 error"}

#: The codes whose text is a safety switch having powered the arm off or being open: 485 to 488 "Robot arm has been powered off by the EMS 2 / PRS / HSS / SSS Safety switch", 253 to 258 "Check whether the Safety ...
#: switch is open or closed" and 60 "Control Box GP Switch Error, check the protective stop connector". The maker does not say which of its codes is a stop; these are the ones whose words are about the stop chain, and every other code is a fault.
ESTOP_CODES = frozenset({60, 253, 254, 255, 256, 257, 258, 485, 486, 487, 488})


class ProtocolError(Exception):
    """A line or a packet that is not one the maker's documents allow, or a command whose parameters cannot be sent."""


# ── the commands ─────────────────────────────────────────────────────────────────────────────────

def _finite(*values: float) -> None:
    for value in values:
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
            raise ProtocolError(f"{value!r} is not a finite number.")


def number(value: float) -> str:
    """A number as the controller reads it: at most six decimals, no trailing zeros, no `-0`. **The maker's client writes six significant digits** (C++'s default stream), which cuts 123.456789 to 123.457; this keeps six decimals."""
    _finite(value)
    text = f"{value:.6f}".rstrip("0").rstrip(".")
    return "0" if text in ("", "-0") else text


def _jnt(angles_deg: list[float] | tuple[float, ...]) -> str:
    if not isinstance(angles_deg, (list, tuple)) or len(angles_deg) != 6:
        raise ProtocolError("a joint target is six angles.")
    _finite(*angles_deg)
    for angle in angles_deg:
        if not -ANGLE_LIMIT_DEG <= angle <= ANGLE_LIMIT_DEG:
            raise ProtocolError(f"{angle} degrees is outside the -360 to 360 the manual gives a joint angle.")
    return "jnt[" + ", ".join(number(angle) for angle in angles_deg) + "]"


def servo_j(angles_deg: list[float] | tuple[float, ...], t1: float, t2: float, gain: float, alpha: float) -> str:
    """`move_servo_j(jnt[j0, ..., j5], t1, t2, gain, alpha)` (UI Script 7.34), each checked against the range the manual gives."""
    jnt = _jnt(angles_deg)
    _finite(t1, t2, gain, alpha)
    if t1 < MIN_T1:
        raise ProtocolError(f"t1 of {t1} is below {MIN_T1}, the least time to arrive.")
    if not T2_RANGE[0] < t2 < T2_RANGE[1]:
        raise ProtocolError(f"t2 of {t2} is not between {T2_RANGE[0]} and {T2_RANGE[1]} s.")
    if not gain > 0:
        raise ProtocolError(f"gain of {gain} is not above 0.")
    if not 0 < alpha < 1:
        raise ProtocolError(f"alpha of {alpha} is not between 0 and 1.")
    return f"move_servo_j({jnt},{number(t1)},{number(t2)},{number(gain)},{number(alpha)})"


def move_j(angles_deg: list[float] | tuple[float, ...], speed_deg_s: float, accel_deg_s2: float) -> str:
    """`move_j(jnt[...], speed, acceleration)` (UI Script 7.4): a joint move in degrees, deg/s and deg/s^2."""
    jnt = _jnt(angles_deg)
    _finite(speed_deg_s, accel_deg_s2)
    if not speed_deg_s > 0 or not accel_deg_s2 > 0:
        raise ProtocolError("a move's speed and acceleration are above 0.")
    return f"move_j({jnt},{number(speed_deg_s)},{number(accel_deg_s2)})"


def task_stop() -> str:
    """`task stop` (Appendix E): "stops the robot operation that is currently in progress". The UI Script manual spells the same function `halt` (5.1)."""
    return "task stop"


def halt() -> str:
    return "halt"


def task_pause() -> str:
    return "task pause"


def task_resume(collision: bool) -> str:
    """`task resume_a` resumes a general pause (a `task pause`, an alarm or a debug pause) and `task resume_b` one that external collision detection caused."""
    return "task resume_b" if collision else "task resume_a"


def activate() -> str:
    """`mc jall init`: "apply DC power to the robot arm and turn the servo-on to activate it"."""
    return "mc jall init"


def pgmode(real: bool) -> str:
    """`pgmode real` or `pgmode simulation`: in `real` the arm physically moves on a motion command, and in `simulation` only the controller's internal reference changes."""
    return "pgmode real" if real else "pgmode simulation"


def set_payload_info(weight_kg: float, com_mm: tuple[float, float, float]) -> str:
    """`set_payload_info(kg, x, y, z)`: the tool's payload and its centre of mass, in the maker's default tool frame, which "returns to the default value after the program ends"."""
    _finite(weight_kg)
    if len(com_mm) != 3:
        raise ProtocolError("a payload's centre is three distances.")
    _finite(*com_mm)
    if weight_kg < 0:
        raise ProtocolError(f"a payload of {weight_kg} kg is negative.")
    return f"set_payload_info({number(weight_kg)},{number(com_mm[0])},{number(com_mm[1])},{number(com_mm[2])})"


def get_control_box_info() -> str:
    return "get_control_box_info()"


def print_variable(name: str, tag: str) -> str:
    """`print(NAME,"tag[", "]")`, which the controller answers `info[print_var][tag[value]]` (the maker's client reads a system variable this way)."""
    if not name.replace("_", "").isalnum() or not tag.replace("_", "").replace("#", "").isalnum():
        raise ProtocolError("a variable name is letters, digits and underscores, and a tag is those and `#` (the maker's client makes its tags of letters, digits, `#` and `_`).")
    return f'print({name},"{tag}[", "]")'


# ── what the controller says ─────────────────────────────────────────────────────────────────────

@dataclass(frozen=True)
class Event:
    kind: str           # "ack", "rejected", "info", "warn", "error" or "unknown"
    category: str       # the first bracket: `code`, `motion_changed`, `system`, `controlbox`, `print_var`, `load`, `program`, `msg`
    message: str        # the second, which may hold brackets and line breaks of its own
    raw: str

    @property
    def code(self) -> int | None:
        """The number of a `code` event, and None for anything else (or a code that is not a whole number)."""
        if self.category != "code":
            return None
        try:
            return int(self.message.strip())
        except ValueError:
            return None


def bracket_depth(text: str) -> int:
    """Open brackets minus closed ones: a message whose brackets are not balanced has more lines to come (the controller's syntax error is five of them)."""
    return text.count("[") - text.count("]")


def parse_event(text: str) -> Event:
    """One line, or the lines a bracketed message was spread over joined with newlines, as an Event. Never raises: what is not the controller's grammar is `unknown`, which the caller counts and does not act on."""
    raw = text.strip()
    if raw == ACK:
        return Event("ack", "", "", raw)
    if raw == NOT_ALLOWED:
        return Event("rejected", "", "", raw)
    kind = raw.split("[", 1)[0] if "[" in raw else ""
    if kind not in ("info", "warn", "error"):
        return Event("unknown", "", "", raw)
    parts: list[str] = []
    depth, start = 0, -1
    for at, char in enumerate(raw):
        if char == "[":
            if depth == 0:
                start = at + 1
            depth += 1
        elif char == "]":
            depth -= 1
            if depth == 0 and start >= 0:
                parts.append(raw[start:at])
                start = -1
                if len(parts) == 2:
                    break
    if len(parts) < 2:
        # `kind[category]` with no message, or an unclosed bracket: what there is.
        if depth > 0 and start >= 0:
            parts.append(raw[start:])
        return Event(kind, parts[0] if parts else "", parts[1] if len(parts) > 1 else "", raw)
    return Event(kind, parts[0], parts[1], raw)


def describe_code(code: int) -> str:
    """The maker's English for a message code, or a sentence saying the table does not list it."""
    return CODES.get(code, f"code {code}, which the maker's message table does not list")


# ── the state packet ─────────────────────────────────────────────────────────────────────────────

#: Offsets in the 580 bytes, from the `reqdata` Structure document (bytes counted from 0, little-endian, no padding: every field is four bytes). The maker's own client reads the same offsets through a C union,
#: which `layout_check.py` compares field by field.
OFFSETS = {
    "time": 4, "jnt_ref": 8, "jnt_ang": 32, "jnt_cur": 56, "tcp_ref": 80, "tcp_pos": 104, "analog_in": 128, "analog_out": 144, "digital_in": 160, "digital_out": 224, "jnt_temperature": 288, "task_pc": 312,
    "task_repeat": 316, "task_run_id": 320, "task_run_num": 324, "task_run_time": 328, "task_state": 332, "default_speed": 336, "robot_state": 340, "information_chunk_1": 344, "reserved_1": 348, "jnt_info": 372,
    "collision_detect_onoff": 396, "is_freedrive_mode": 400, "real_vs_simulation_mode": 404, "init_state_info": 408, "init_error": 412, "tfb_analog_in": 416, "tfb_digital_in": 424, "tfb_digital_out": 432,
    "tfb_voltage_out": 440, "op_stat_collision_occur": 444, "op_stat_sos_flag": 448, "op_stat_self_collision": 452, "op_stat_soft_estop_occur": 456, "op_stat_ems_flag": 460, "information_chunk_2": 464,
    "information_chunk_3": 468, "inbox_trap_flag": 472, "inbox_check_mode": 480, "eft": 488, "information_chunk_4": 512, "extend_io1_analog_in": 516, "extend_io1_analog_out": 532,
    "extend_io1_digital_info": 548, "aa_joint_ref": 552, "safety_board_stat_info": 576,
}


@dataclass(frozen=True)
class State:
    """The fields of the packet this driver reads. Joint angles are as the controller sends them, in degrees; the rest are the maker's integers, masked to the bits the document says are valid."""
    ref: tuple[float, ...]              # jnt_ref: the angle the controller is asking each joint for
    ang: tuple[float, ...]              # jnt_ang: the encoder's angle. Per the maker's client "these values do not change in simulation mode" (the Virtual Control Box moves them with the reference)
    cur: tuple[float, ...]              # jnt_cur, amperes
    temperature: tuple[float, ...]      # jnt_temperature, degrees Celsius
    task_state: int                     # 1 idle, 2 paused, 3 running
    robot_state: int                    # 1 when no motion command is executing
    speed_bar: float                    # default_speed, 0 to 1
    chunk1: int                         # information_chunk_1, unmasked
    joint_info: tuple[int, ...]         # jnt_info, one word a joint
    mode: int                           # real_vs_simulation_mode & 0xF
    init_state: int                     # init_state_info & 0x3F
    collision: int                      # op_stat_collision_occur & 3
    sos: int                            # op_stat_sos_flag & 0x3F
    ems: int                            # op_stat_ems_flag & 0x3F
    paused_flag: int                    # op_stat_soft_estop_occur: "pause state flag"
    freedrive: int                      # is_freedrive_mode & 3
    time_raw: int                       # `time`, as the integer the Virtual Control Box sends (the document says a float second, and the value does not advance)

    @property
    def power_applied(self) -> bool:
        """`information_chunk_1` bit 6: "whether DC power is applied to the robot arm"."""
        return bool(self.chunk1 >> 6 & 1)

    @property
    def sos_state(self) -> bool:
        """`information_chunk_1` bit 12: "is SOS state"."""
        return bool(self.chunk1 >> 12 & 1)

    @property
    def estop_bits(self) -> tuple[int, int]:
        """`information_chunk_1` bits 3 and 5, "the control box's E-Stop state 1" and "2". **The document does not say which value is the stop pressed**, so nothing decides on them and `status` reports them as sent."""
        return (self.chunk1 >> 3 & 1, self.chunk1 >> 5 & 1)


def _floats(packet: bytes, name: str, count: int) -> tuple[float, ...]:
    return struct.unpack_from(f"<{count}f", packet, OFFSETS[name])


def _int(packet: bytes, name: str) -> int:
    return struct.unpack_from("<i", packet, OFFSETS[name])[0]


def decode_state(packet: bytes) -> State:
    """The fields of one 580-byte packet. ProtocolError for a packet of the wrong length, one that does not begin `$`, carry its own size and the state's type, or one whose joint values are not numbers."""
    if len(packet) != PACKET_BYTES:
        raise ProtocolError(f"a state packet is {PACKET_BYTES} bytes, not {len(packet)}.")
    size = packet[1] | packet[2] << 8
    if packet[0] != PACKET_START or packet[3] != PACKET_TYPE_STATE or size != PACKET_BYTES - 4:
        raise ProtocolError(f"the packet's header is {packet[:4].hex()}, and a state packet's is 24 {PACKET_BYTES - 4:04x} 03 (a `$`, its own size without the header, and the type 3).")
    ref, ang, cur, temperature = _floats(packet, "jnt_ref", 6), _floats(packet, "jnt_ang", 6), _floats(packet, "jnt_cur", 6), _floats(packet, "jnt_temperature", 6)
    for name, values in (("jnt_ref", ref), ("jnt_ang", ang)):
        if not all(math.isfinite(v) for v in values):
            raise ProtocolError(f"{name} holds a value that is not a number.")
    speed = struct.unpack_from("<f", packet, OFFSETS["default_speed"])[0]
    return State(
        ref=ref, ang=ang, cur=cur, temperature=temperature, task_state=_int(packet, "task_state"), robot_state=_int(packet, "robot_state"), speed_bar=speed if math.isfinite(speed) else math.nan,
        chunk1=_int(packet, "information_chunk_1") & 0xFFFFFFFF, joint_info=struct.unpack_from("<6i", packet, OFFSETS["jnt_info"]), mode=_int(packet, "real_vs_simulation_mode") & 0xF,
        init_state=_int(packet, "init_state_info") & 0x3F, collision=_int(packet, "op_stat_collision_occur") & 3, sos=_int(packet, "op_stat_sos_flag") & 0x3F, ems=_int(packet, "op_stat_ems_flag") & 0x3F,
        paused_flag=_int(packet, "op_stat_soft_estop_occur") & 1, freedrive=_int(packet, "is_freedrive_mode") & 3, time_raw=_int(packet, "time"))


def encode_state(*, ref: tuple[float, ...] = (0.0,) * 6, ang: tuple[float, ...] | None = None, cur: tuple[float, ...] = (0.0,) * 6, temperature: tuple[float, ...] = (30.0,) * 6, task_state: int = TASK_IDLE,
                 robot_state: int = ROBOT_IDLE, speed_bar: float = 1.0, chunk1: int = 0, joint_info: tuple[int, ...] = (0b110,) * 6, mode: int = MODE_SIMULATION, init_state: int = 0, collision: int = 0, sos: int = 0, ems: int = 0,
                 paused_flag: int = 0, freedrive: int = 0, time_raw: int = 0) -> bytes:
    """One packet, for the simulated controller and for the tests: every field this module reads, at the document's offset, and the header. The rest is zero."""
    packet = bytearray(PACKET_BYTES)
    packet[0], packet[1], packet[2], packet[3] = PACKET_START, (PACKET_BYTES - 4) & 0xFF, (PACKET_BYTES - 4) >> 8, PACKET_TYPE_STATE
    struct.pack_into("<i", packet, OFFSETS["time"], time_raw)
    struct.pack_into("<6f", packet, OFFSETS["jnt_ref"], *ref)
    struct.pack_into("<6f", packet, OFFSETS["jnt_ang"], *(ang if ang is not None else ref))
    struct.pack_into("<6f", packet, OFFSETS["jnt_cur"], *cur)
    struct.pack_into("<6f", packet, OFFSETS["jnt_temperature"], *temperature)
    for name, value in (("task_state", task_state), ("robot_state", robot_state), ("information_chunk_1", chunk1 - (1 << 32) if chunk1 >= 1 << 31 else chunk1), ("collision_detect_onoff", 1), ("is_freedrive_mode", freedrive),
                        ("real_vs_simulation_mode", mode), ("init_state_info", init_state), ("op_stat_collision_occur", collision), ("op_stat_sos_flag", sos), ("op_stat_soft_estop_occur", paused_flag), ("op_stat_ems_flag", ems)):
        struct.pack_into("<i", packet, OFFSETS[name], value)
    struct.pack_into("<f", packet, OFFSETS["default_speed"], speed_bar)
    struct.pack_into("<6i", packet, OFFSETS["jnt_info"], *joint_info)
    return bytes(packet)
