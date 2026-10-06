"""AgileX Piper on the wire: identifiers, payload layouts, units and the enumerations the arm reports.

Pure. Nothing here opens a socket, reads a clock or holds an arm — every function is bytes in, bytes out
— which is what lets the whole protocol be exercised with no bus of any kind, and what keeps
:mod:`skynet_piper.arm` and :mod:`skynet_piper.bridge` from each growing their own idea of a frame.

# Where every number comes from

``layout_check.py`` holds this module to the maker's own sources, and fails when they disagree. The
source is AgileX's ``piper_sdk`` (MIT, ``agilexrobotics/piper_sdk``), protocol V2 — the one the arm's
firmware speaks from V1.5-2 on — at the commit the check pins:

* the identifiers are ``piper_sdk/piper_msgs/msg_v2/can_id.py``'s ``CanIDPiper``;
* the payload layouts are the maker's own codec, ``piper_sdk/protocol/protocol_v2/piper_protocol_v2.py``
  (``DecodeMessage`` for what the arm sends, ``EncodeMessage`` for what a master sends), which the check
  imports unmodified and drives with this module's bytes in both directions;
* the enumerations (control mode, arm status, move mode) are ``piper_msgs/msg_v2/feedback/arm_feedback_status.py``;
* the travel of each joint and of the gripper is ``piper_sdk/piper_param/piper_param_manager.py``'s published table.

# The wire

Eleven-bit identifiers, eight-byte frames, big-endian throughout (the maker's own convention, and the
reverse of the other CAN drivers here), one megabit. The arm *broadcasts*: once powered it sends its
status, its six joint angles in pairs, the gripper and each driver's own report cyclically, with no
request. A master sends modes and targets and enables the motors one at a time or all together.

| Id | Direction | What |
|---|---|---|
| ``0x2A1`` | arm → master | status: control mode, arm status, move mode, teach, motion, trajectory, error code |
| ``0x2A5``–``0x2A7`` | arm → master | joint angles, two per frame, signed 0.001° |
| ``0x2A8`` | arm → master | gripper stroke (signed 0.001 mm), torque (signed 0.001 N·m), status byte |
| ``0x251``–``0x256`` | arm → master | each driver's speed, current and position |
| ``0x261``–``0x266`` | arm → master | each driver's voltage, temperatures, status byte and bus current |
| ``0x150`` | master → arm | the fast emergency stop, trajectory and teach controls |
| ``0x151`` | master → arm | control mode, move mode, speed percentage |
| ``0x155``–``0x157`` | master → arm | joint targets, two per frame, signed 0.001° |
| ``0x159`` | master → arm | gripper stroke, torque, enable and clear |
| ``0x471`` | master → arm | motor enable and disable |
| ``0x472`` / ``0x473`` | master → arm / arm → master | the angle and speed limits the arm holds, asked for and answered |

# What it will not invent

The 0x150 ``resume`` value (``2``) is the SDK's own ``ResetPiper``: the SDK says it makes the arm *lose
power and fall, clearing every error and internal flag*. This module does not encode it for the driver to
send, because a release that drops a loaded arm is not a release; see :mod:`skynet_piper.bridge`.
"""

from __future__ import annotations

import math
import struct
from dataclasses import dataclass

__all__ = [
    "ID_STATUS",
    "ID_JOINTS_12",
    "ID_JOINTS_34",
    "ID_JOINTS_56",
    "ID_GRIPPER",
    "ID_HIGH_SPEED",
    "ID_LOW_SPEED",
    "ID_MOTION_1",
    "ID_MOTION_2",
    "ID_JOINT_CTRL",
    "ID_GRIPPER_CTRL",
    "ID_ENABLE",
    "ID_LIMIT_QUERY",
    "ID_LIMIT_REPORT",
    "EMERGENCY_STOP",
    "CTRL_STANDBY",
    "CTRL_CAN",
    "CTRL_MODES",
    "MOVE_J",
    "MOVE_MODES",
    "ARM_STATUS_NORMAL",
    "ARM_STATUS_EMERGENCY_STOP",
    "ARM_STATUSES",
    "MOTION_REACHED",
    "MOTION_NOT_REACHED",
    "DRIVER_VOLTAGE_LOW",
    "DRIVER_MOTOR_OVERHEAT",
    "DRIVER_OVERCURRENT",
    "DRIVER_OVERHEAT",
    "DRIVER_COLLISION_PROTECTION",
    "DRIVER_ERROR",
    "DRIVER_ENABLED",
    "DRIVER_STALL_PROTECTION",
    "DRIVER_FAULT_BITS",
    "DRIVER_BIT_NAMES",
    "ERR_LIMIT_SHIFT",
    "ERR_COMMS_SHIFT",
    "FAULT_NOT_CAN_CONTROL",
    "FAULT_GRIPPER",
    "GRIPPER_FAULT_BITS",
    "GRIPPER_ENABLED",
    "GRIPPER_ZEROED",
    "GRIPPER_BIT_NAMES",
    "JOINT_NAMES",
    "GRIPPER_NAME",
    "JOINT_LIMITS_RAD",
    "GRIPPER_RANGE_M",
    "MDEG_PER_RAD",
    "COUNTS_PER_METRE",
    "Status",
    "LowSpeed",
    "HighSpeed",
    "LimitReport",
    "rad_to_mdeg",
    "mdeg_to_rad",
    "metres_to_counts",
    "counts_to_metres",
    "motion_ctrl_1",
    "motion_ctrl_2",
    "joint_ctrl",
    "gripper_ctrl",
    "motor_enable",
    "limit_query",
    "parse_status",
    "parse_joint_pair",
    "parse_gripper",
    "parse_high_speed",
    "parse_low_speed",
    "parse_limit_report",
    "motion_ctrl_1_fields",
    "motion_ctrl_2_fields",
    "motor_enable_fields",
    "joint_ctrl_pair",
    "gripper_ctrl_fields",
    "limit_query_fields",
]

# ── identifiers (``CanIDPiper``) ────────────────────────────────────────────

ID_STATUS = 0x2A1
ID_JOINTS_12 = 0x2A5
ID_JOINTS_34 = 0x2A6
ID_JOINTS_56 = 0x2A7
ID_GRIPPER = 0x2A8
#: ``0x251`` + (joint - 1).
ID_HIGH_SPEED = 0x251
#: ``0x261`` + (joint - 1).
ID_LOW_SPEED = 0x261
ID_MOTION_1 = 0x150
ID_MOTION_2 = 0x151
#: ``0x155`` is joints 1 and 2, ``0x156`` 3 and 4, ``0x157`` 5 and 6.
ID_JOINT_CTRL = (0x155, 0x156, 0x157)
ID_GRIPPER_CTRL = 0x159
ID_ENABLE = 0x471
ID_LIMIT_QUERY = 0x472
ID_LIMIT_REPORT = 0x473

# ── the 0x150 byte this driver sends, and the one it never does ─────────────

#: ``emergency_stop`` = 1: the arm's fast stop. The only 0x150 value this driver sends.
EMERGENCY_STOP = 0x01
#: Not exported for sending: ``emergency_stop`` = 2 is the SDK's ``ResetPiper`` (see the header).
_RESUME_IS_A_RESET = 0x02

# ── 0x151: control and move modes ───────────────────────────────────────────

CTRL_STANDBY = 0x00
CTRL_CAN = 0x01
#: What the arm *reports* in 0x2A1's first byte (``ArmMsgFeedbackStatusEnum.CtrlMode``).
CTRL_MODES = {
    0x00: "standby",
    0x01: "CAN control",
    0x02: "teaching",
    0x03: "Ethernet control",
    0x04: "Wi-Fi control",
    0x05: "remote control",
    0x06: "linkage teaching input",
    0x07: "offline trajectory",
}
MOVE_J = 0x01
MOVE_MODES = {0x00: "MOVE P", 0x01: "MOVE J", 0x02: "MOVE L", 0x03: "MOVE C", 0x04: "MOVE M", 0x05: "MOVE CPV"}

# ── 0x2A1: arm status (``ArmMsgFeedbackStatusEnum.ArmStatus``) ──────────────

ARM_STATUS_NORMAL = 0x00
ARM_STATUS_EMERGENCY_STOP = 0x01
ARM_STATUSES = {
    0x00: "normal",
    0x01: "emergency stop",
    0x02: "no solution",
    0x03: "singularity point",
    0x04: "target position exceeds limit",
    0x05: "joint communication error",
    0x06: "joint brake not released",
    0x07: "collision occurred",
    0x08: "overspeed during teaching drag",
    0x09: "joint status error",
    0x0A: "other error",
    0x0B: "teaching record",
    0x0C: "teaching execution",
    0x0D: "teaching pause",
    0x0E: "main controller NTC over temperature",
    0x0F: "release resistor NTC over temperature",
}
MOTION_REACHED = 0x00
MOTION_NOT_REACHED = 0x01

#: The error word is bytes 6 and 7 of 0x2A1 read as one big-endian ``uint16`` (the maker's decoder:
#: ``ConvertBytesToInt(can_data, 6, 8)``). Byte 6 is the joint *angle-limit* bits (joints 1–6 in bits 0–5 of
#: the byte, so bits 8–13 of the word) and byte 7 the *communication* bits (bits 0–5 of the word).
ERR_LIMIT_SHIFT = 8
ERR_COMMS_SHIFT = 0
#: The one bit of the fault word this driver defines and the maker does not: bit 15, which the maker lists as *reserved* in the
#: angle-limit byte. Set while the arm reports a control mode other than CAN control — a teach pendant, a remote controller, another
#: host has it — so that an arm that is ignoring this master's commands reads as a fault *with a word*, and not as a fault flag over a
#: healthy-looking zero (§24).
FAULT_NOT_CAN_CONTROL = 1 << 15
#: The other: bit 14, reserved in the same byte. Set while the gripper's status byte holds a fault bit. A gripper's byte has the joint
#: drivers' layout for bits 0 to 3 and 5, bit 4 is its *sensor* where a joint driver's is collision protection, and bit 7 is its zeroing
#: state where a joint driver's is stall protection, so its bits are not mixed into the top byte of the word, which is the six joint
#: drivers' union.
FAULT_GRIPPER = 1 << 14

# ── 0x261–0x266: a driver's status byte ─────────────────────────────────────

DRIVER_VOLTAGE_LOW = 1 << 0
DRIVER_MOTOR_OVERHEAT = 1 << 1
DRIVER_OVERCURRENT = 1 << 2
DRIVER_OVERHEAT = 1 << 3
DRIVER_COLLISION_PROTECTION = 1 << 4
DRIVER_ERROR = 1 << 5
#: Set while the driver is enabled — the one bit here that means *good* when it is on.
DRIVER_ENABLED = 1 << 6
DRIVER_STALL_PROTECTION = 1 << 7
#: Every bit of the status byte that says something is wrong when it is set.
DRIVER_FAULT_BITS = (
    DRIVER_VOLTAGE_LOW
    | DRIVER_MOTOR_OVERHEAT
    | DRIVER_OVERCURRENT
    | DRIVER_OVERHEAT
    | DRIVER_COLLISION_PROTECTION
    | DRIVER_ERROR
    | DRIVER_STALL_PROTECTION
)
DRIVER_BIT_NAMES = {
    DRIVER_VOLTAGE_LOW: "supply voltage too low",
    DRIVER_MOTOR_OVERHEAT: "motor over temperature",
    DRIVER_OVERCURRENT: "driver over current",
    DRIVER_OVERHEAT: "driver over temperature",
    DRIVER_COLLISION_PROTECTION: "collision protection triggered",
    DRIVER_ERROR: "driver error",
    DRIVER_ENABLED: "driver enabled",
    DRIVER_STALL_PROTECTION: "stall protection triggered",
}

# ── 0x2A8: the gripper's status byte ────────────────────────────────────────

#: Bits 0 to 5 say something is wrong when set: supply voltage low, motor over temperature, driver over current, driver over temperature,
#: *sensor* abnormal, driver error.
GRIPPER_FAULT_BITS = 0x3F
GRIPPER_ENABLED = 1 << 6
#: Set once the gripper has been zeroed — or has been, at some earlier power-on. Not a fault when clear.
GRIPPER_ZEROED = 1 << 7
GRIPPER_BIT_NAMES = {
    1 << 0: "supply voltage too low",
    1 << 1: "motor over temperature",
    1 << 2: "driver over current",
    1 << 3: "driver over temperature",
    1 << 4: "sensor abnormal",
    1 << 5: "driver error",
}

# ── the body ────────────────────────────────────────────────────────────────

JOINT_NAMES = ("joint1", "joint2", "joint3", "joint4", "joint5", "joint6")
GRIPPER_NAME = "gripper"

#: The travel the maker publishes, in radians: ``C_PiperParamManager``'s ``joint_limit`` table. Joint 1 is
#: ±150°, joint 2 0 to 180°, joint 3 −170° to 0, joint 4 ±100°, joint 5 ±70°, joint 6 ±120°, as the SDK rounds
#: them. The arm also *reports* limits (0x473); the driver declares the narrower of the two, never the wider
#: (DRY-COVERAGE-PLAN.md F181).
JOINT_LIMITS_RAD = (
    (-2.6179, 2.6179),
    (0.0, 3.14),
    (-2.967, 0.0),
    (-1.745, 1.745),
    (-1.22, 1.22),
    (-2.09439, 2.09439),
)
#: The gripper's stroke in metres: ``gripper_range`` (0 to 70 mm).
GRIPPER_RANGE_M = (0.0, 0.07)

#: 0.001° per count, 1000 counts per degree.
MDEG_PER_RAD = 1000.0 * 180.0 / math.pi
#: 0.001 mm per count.
COUNTS_PER_METRE = 1_000_000.0

_I32_MIN, _I32_MAX = -(2**31), 2**31 - 1


# ── units ───────────────────────────────────────────────────────────────────

def rad_to_mdeg(radians: float) -> int:
    """A joint angle in radians as the wire's signed 0.001° count. ``ValueError`` for anything that is not
    a finite angle a ``int32`` holds — a non-finite value never becomes a number on the wire."""
    if not math.isfinite(radians):
        raise ValueError(f"{radians!r} is not a finite angle")
    counts = round(radians * MDEG_PER_RAD)
    if not _I32_MIN <= counts <= _I32_MAX:
        raise ValueError(f"{radians!r} rad does not fit the wire's 32-bit 0.001 degree count")
    return counts


def mdeg_to_rad(counts: int) -> float:
    return counts / MDEG_PER_RAD


def metres_to_counts(metres: float) -> int:
    """A gripper stroke in metres as the wire's signed 0.001 mm count (``ValueError`` as above)."""
    if not math.isfinite(metres):
        raise ValueError(f"{metres!r} is not a finite stroke")
    counts = round(metres * COUNTS_PER_METRE)
    if not _I32_MIN <= counts <= _I32_MAX:
        raise ValueError(f"{metres!r} m does not fit the wire's 32-bit 0.001 mm count")
    return counts


def counts_to_metres(counts: int) -> float:
    return counts / COUNTS_PER_METRE


# ── what the arm says ───────────────────────────────────────────────────────

@dataclass(frozen=True)
class Status:
    """0x2A1."""

    ctrl_mode: int
    arm_status: int
    mode_feed: int
    teach_status: int
    motion_status: int
    trajectory_num: int
    #: Bytes 6 and 7 as one big-endian word: angle-limit bits in 8–13, communication bits in 0–5.
    err_code: int

    def limit_bits(self) -> int:
        """Joint 1–6 angle-limit flags, as a six-bit mask."""
        return (self.err_code >> ERR_LIMIT_SHIFT) & 0x3F

    def comms_bits(self) -> int:
        """Joint 1–6 communication flags, as a six-bit mask."""
        return (self.err_code >> ERR_COMMS_SHIFT) & 0x3F


@dataclass(frozen=True)
class HighSpeed:
    """0x251–0x256: one driver's fast report."""

    #: Signed, 0.001 rad/s of the *motor*.
    motor_speed: int
    #: Signed, 0.001 A.
    current: int
    #: Signed, 0.001 rad of the *motor*.
    pos: int


@dataclass(frozen=True)
class LowSpeed:
    """0x261–0x266: one driver's slow report."""

    #: Unsigned, 0.1 V.
    vol: int
    #: Signed, °C.
    foc_temp: int
    #: Signed, °C.
    motor_temp: int
    #: The status byte; see ``DRIVER_*``.
    status: int
    #: Unsigned, 0.001 A (0 on the 1.5 kg joint motors, which sample none).
    bus_current: int

    def enabled(self) -> bool:
        return bool(self.status & DRIVER_ENABLED)

    def faults(self) -> int:
        return self.status & DRIVER_FAULT_BITS


@dataclass(frozen=True)
class LimitReport:
    """0x473: the limits one joint's driver holds."""

    motor_num: int
    #: Signed, 0.1°.
    max_angle: int
    #: Signed, 0.1°.
    min_angle: int
    #: Unsigned, 0.001 rad/s.
    max_speed: int

    def max_rad(self) -> float:
        return math.radians(self.max_angle / 10.0)

    def min_rad(self) -> float:
        return math.radians(self.min_angle / 10.0)


def _need(data: bytes, length: int, what: str) -> None:
    if len(data) != length:
        raise ValueError(f"{what} is {length} bytes and this payload is {len(data)}")


def parse_status(data: bytes) -> Status:
    _need(data, 8, "the status frame")
    ctrl, arm, mode, teach, motion, trajectory = data[0], data[1], data[2], data[3], data[4], data[5]
    (err,) = struct.unpack(">H", data[6:8])
    return Status(ctrl, arm, mode, teach, motion, trajectory, err)


def parse_joint_pair(data: bytes) -> tuple[int, int]:
    """Two signed 0.001° counts (0x2A5–0x2A7 and, in the other direction, 0x155–0x157)."""
    _need(data, 8, "a joint pair")
    return struct.unpack(">ii", data)


def parse_gripper(data: bytes) -> tuple[int, int, int]:
    """Stroke (signed 0.001 mm), torque (signed 0.001 N·m), status byte (0x2A8)."""
    _need(data, 8, "the gripper frame")
    stroke, torque, status = struct.unpack(">ihB", data[:7])
    return stroke, torque, status


def parse_high_speed(data: bytes) -> HighSpeed:
    _need(data, 8, "a driver's high-speed frame")
    speed, current, pos = struct.unpack(">hhi", data)
    return HighSpeed(speed, current, pos)


def parse_low_speed(data: bytes) -> LowSpeed:
    _need(data, 8, "a driver's low-speed frame")
    vol, temperature, motor_temperature, status, current = struct.unpack(">HhbBH", data)
    return LowSpeed(vol, temperature, motor_temperature, status, current)


def parse_limit_report(data: bytes) -> LimitReport:
    _need(data, 8, "a limit report")
    motor, high, low, speed = struct.unpack(">BhhH", data[:7])
    return LimitReport(motor, high, low, speed)


# ── what a master says ──────────────────────────────────────────────────────

def motion_ctrl_1(emergency_stop: int = 0, track_ctrl: int = 0, teach_ctrl: int = 0) -> bytes:
    """0x150. Only ``emergency_stop`` = 1 is ever sent by this driver (see the header)."""
    return bytes([emergency_stop, track_ctrl, teach_ctrl, 0, 0, 0, 0, 0])


def motion_ctrl_2(ctrl_mode: int, move_mode: int, speed_percent: int,
                  mit_mode: int = 0, residence_time: int = 0, installation_pos: int = 0) -> bytes:
    """0x151: control mode, move mode and the speed as a percentage of the arm's own maximum."""
    if not 0 <= speed_percent <= 100:
        raise ValueError(f"a speed of {speed_percent} % is not 0 to 100")
    return bytes([ctrl_mode, move_mode, speed_percent, mit_mode, residence_time, installation_pos, 0, 0])


def joint_ctrl(first: int, second: int) -> bytes:
    """One 0x155–0x157 frame: two signed 0.001° counts."""
    return struct.pack(">ii", first, second)


def gripper_ctrl(stroke_counts: int, torque_counts: int, status_code: int, set_zero: int = 0) -> bytes:
    """0x159: stroke (signed 0.001 mm), torque (unsigned 0.001 N·m, 0 to 5000), the enable code, and the zero command.

    ``ValueError`` for a torque past 5000: the maker's own constructor (``ArmMsgGripperCtrl``) refuses it, and a frame the maker's library
    will not build is not one this driver sends."""
    if not 0 <= torque_counts <= 5000:
        raise ValueError(f"a gripper torque of {torque_counts} (0.001 N·m) is not 0 to 5000")
    return struct.pack(">iHBB", stroke_counts, torque_counts, status_code, set_zero)


def motor_enable(motor_num: int, enable_flag: int) -> bytes:
    """0x471: ``motor_num`` 1–6 a joint, 7 the gripper, 0xFF all of them; ``enable_flag`` 1 disables, 2 enables."""
    return bytes([motor_num, enable_flag, 0, 0, 0, 0, 0, 0])


def limit_query(motor_num: int, content: int = 0x01) -> bytes:
    """0x472: ask joint ``motor_num`` (1–6) for its angle and speed limits (``content`` 1) or its acceleration limit (2)."""
    return bytes([motor_num, content, 0, 0, 0, 0, 0, 0])


# ── the same frames, read back (what the arm sees) ──────────────────────────

def motion_ctrl_1_fields(data: bytes) -> tuple[int, int, int]:
    _need(data, 8, "0x150")
    return data[0], data[1], data[2]


def motion_ctrl_2_fields(data: bytes) -> tuple[int, int, int, int, int, int]:
    _need(data, 8, "0x151")
    return data[0], data[1], data[2], data[3], data[4], data[5]


def motor_enable_fields(data: bytes) -> tuple[int, int]:
    _need(data, 8, "0x471")
    return data[0], data[1]


def joint_ctrl_pair(data: bytes) -> tuple[int, int]:
    return parse_joint_pair(data)


def gripper_ctrl_fields(data: bytes) -> tuple[int, int, int, int]:
    _need(data, 8, "0x159")
    stroke, torque, status, zero = struct.unpack(">iHBB", data)
    return stroke, torque, status, zero


def limit_query_fields(data: bytes) -> tuple[int, int]:
    _need(data, 8, "0x472")
    return data[0], data[1]
