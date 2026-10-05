"""ODrive CANSimple on the wire: identifiers, commands, and the payload layouts.

Pure. Nothing here opens a socket, reads a clock or holds an axis — every function is bytes
in, bytes out — which is what lets the whole protocol be exercised with no bus of any kind, and
what keeps :mod:`skynet_odrive.axis` and :mod:`skynet_odrive.bridge` from each growing their own
idea of a frame.

# Where every number comes from

``layout_check.py`` holds this module to the maker's own sources, and fails when they disagree:

* the command identifiers and the send/receive behaviours (what a request looks like, what
  answers it, what the stop latches) are the firmware's,
  ``Firmware/communication/can/can_simple.{hpp,cpp}`` at the pinned commit;
* the payload layouts (signal offsets, widths, byte order, scales) and the state, mode and
  error enumerations are the maker's own CAN database, ``tools/odrive-cansimple.dbc``, and its
  ``tools/odrive/enums.py``, which the check imports unmodified.

Where those sources disagree with each other the check names the disagreement rather than
picking a side in silence (DRY-COVERAGE-PLAN.md F280: the DBC models the motor error as 32
bits and the firmware sends 64, and the DBC's input-velocity units are swapped).

# The identifiers

An 11-bit arbitration id is a node id in the upper six bits and a command in the lower five
(``can_simple.hpp``: ``node_id << NUM_CMD_ID_BITS | cmd``, six and five). Node ids run 0 to 63
and node 0 is a valid axis — it is the firmware's default — not a broadcast address.

# Requests without RTR

The firmware answers a ``GET_*`` request whose RTR bit is set **or whose length is zero**
(``do_command``: ``if (msg.rtr || msg.len == 0)``). This driver always asks with a zero-length
frame and never sets RTR: the bus layer below (:mod:`skynet_canopen.bus`, shared with the
CANopen driver) has no RTR bit on its frames, and a remote frame sent through SocketCAN would
come back marked with a flag that layer reads as somebody else's traffic. The two requests are
the firmware's equivalent, and the simulated axis answers both the same way.
"""

from __future__ import annotations

import math
import struct

__all__ = [
    "CMD_NMT",
    "CMD_HEARTBEAT",
    "CMD_ESTOP",
    "CMD_GET_MOTOR_ERROR",
    "CMD_GET_ENCODER_ERROR",
    "CMD_GET_SENSORLESS_ERROR",
    "CMD_SET_NODE_ID",
    "CMD_SET_REQUESTED_STATE",
    "CMD_SET_STARTUP_CONFIG",
    "CMD_GET_ENCODER_ESTIMATES",
    "CMD_GET_ENCODER_COUNT",
    "CMD_SET_CONTROLLER_MODES",
    "CMD_SET_INPUT_POS",
    "CMD_SET_INPUT_VEL",
    "CMD_SET_INPUT_TORQUE",
    "CMD_SET_LIMITS",
    "CMD_START_ANTICOGGING",
    "CMD_SET_TRAJ_VEL_LIMIT",
    "CMD_SET_TRAJ_ACCEL_LIMITS",
    "CMD_SET_TRAJ_INERTIA",
    "CMD_GET_IQ",
    "CMD_GET_SENSORLESS_ESTIMATES",
    "CMD_RESET",
    "CMD_GET_BUS_VOLTAGE_CURRENT",
    "CMD_CLEAR_ERRORS",
    "CMD_SET_LINEAR_COUNT",
    "CMD_SET_POS_GAIN",
    "CMD_SET_VEL_GAINS",
    "CMD_GET_ADC_VOLTAGE",
    "CMD_GET_CONTROLLER_ERROR",
    "STATE_IDLE",
    "STATE_CLOSED_LOOP_CONTROL",
    "STATES",
    "CONTROL_MODE_TORQUE",
    "CONTROL_MODE_VELOCITY",
    "CONTROL_MODE_POSITION",
    "INPUT_MODE_PASSTHROUGH",
    "TURNS_TO_RAD",
    "msg_id",
    "node_of",
    "cmd_of",
    "rad_to_turns",
    "turns_to_rad",
    "parse_heartbeat",
    "parse_estimates",
    "parse_error_u32",
    "parse_error_u64",
    "request",
    "set_requested_state",
    "set_controller_modes",
    "set_input_pos",
    "set_input_vel",
    "set_input_torque",
    "estop",
]

#: The firmware's ``CANSimple`` command enumeration, in order (``can_simple.hpp``). The DBC
#: carries the same ids on its ``BO_`` lines for axes 0 to 7; ``layout_check.py`` holds both.
CMD_NMT = 0x00
CMD_HEARTBEAT = 0x01
CMD_ESTOP = 0x02
CMD_GET_MOTOR_ERROR = 0x03
CMD_GET_ENCODER_ERROR = 0x04
CMD_GET_SENSORLESS_ERROR = 0x05
CMD_SET_NODE_ID = 0x06
CMD_SET_REQUESTED_STATE = 0x07
CMD_SET_STARTUP_CONFIG = 0x08
CMD_GET_ENCODER_ESTIMATES = 0x09
CMD_GET_ENCODER_COUNT = 0x0A
CMD_SET_CONTROLLER_MODES = 0x0B
CMD_SET_INPUT_POS = 0x0C
CMD_SET_INPUT_VEL = 0x0D
CMD_SET_INPUT_TORQUE = 0x0E
CMD_SET_LIMITS = 0x0F
CMD_START_ANTICOGGING = 0x10
CMD_SET_TRAJ_VEL_LIMIT = 0x11
CMD_SET_TRAJ_ACCEL_LIMITS = 0x12
CMD_SET_TRAJ_INERTIA = 0x13
CMD_GET_IQ = 0x14
CMD_GET_SENSORLESS_ESTIMATES = 0x15
CMD_RESET = 0x16
CMD_GET_BUS_VOLTAGE_CURRENT = 0x17
CMD_CLEAR_ERRORS = 0x18
CMD_SET_LINEAR_COUNT = 0x19
CMD_SET_POS_GAIN = 0x1A
CMD_SET_VEL_GAINS = 0x1B
CMD_GET_ADC_VOLTAGE = 0x1C
CMD_GET_CONTROLLER_ERROR = 0x1D

#: The two axis states this driver acts on, and the full table it names the rest by
#: (``enums.py`` ``AXIS_STATE_*``, the DBC's ``Axis_State`` value table).
STATE_IDLE = 1
STATE_CLOSED_LOOP_CONTROL = 8
STATES = {
    0: "UNDEFINED",
    1: "IDLE",
    2: "STARTUP_SEQUENCE",
    3: "FULL_CALIBRATION_SEQUENCE",
    4: "MOTOR_CALIBRATION",
    6: "ENCODER_INDEX_SEARCH",
    7: "ENCODER_OFFSET_CALIBRATION",
    8: "CLOSED_LOOP_CONTROL",
    9: "LOCKIN_SPIN",
    10: "ENCODER_DIR_FIND",
    11: "HOMING",
    12: "ENCODER_HALL_POLARITY_CALIBRATION",
    13: "ENCODER_HALL_PHASE_CALIBRATION",
}

#: The control modes this driver sets, and the one input mode (``enums.py``,
#: the DBC's ``Control_Mode`` / ``Input_Mode`` tables). Passthrough feeds the setpoint straight
#: to the controller: a ramp or filter here would reshape the kernel's command between the kernel
#: and the shaft, and the kernel's setpoint is the authority.
CONTROL_MODE_TORQUE = 1
CONTROL_MODE_VELOCITY = 2
CONTROL_MODE_POSITION = 3
INPUT_MODE_PASSTHROUGH = 1

#: One shaft revolution, in radians. The wire speaks turns (``rev``, ``rev/s`` on every DBC
#: signal that carries a unit); the OS speaks radians (§24).
TURNS_TO_RAD = 2.0 * math.pi

#: What fits in the wire's 32-bit floats. A double beyond it is not a setpoint the firmware can
#: read, and packing it would raise ``struct.error`` from somewhere with no joint in its message.
_F32_MAX = 3.4028235e38


def msg_id(node: int, cmd: int) -> int:
    """The arbitration id for ``cmd`` on ``node``."""
    if not 0 <= node <= 63:
        raise ValueError(f"{node} is not a CANSimple node id (0 to 63)")
    if not 0 <= cmd <= 0x1F:
        raise ValueError(f"{cmd:#x} is not a CANSimple command (0x00 to 0x1f)")
    return (node << 5) | cmd


def node_of(arbitration_id: int) -> int:
    """The node id in the upper six bits."""
    return arbitration_id >> 5


def cmd_of(arbitration_id: int) -> int:
    """The command in the lower five bits."""
    return arbitration_id & 0x1F


def rad_to_turns(radians: float) -> float:
    """Radians to shaft revolutions. Raises for a value the wire cannot carry."""
    if not math.isfinite(radians):
        raise ValueError(f"{radians} is not a finite angle")
    turns = radians / TURNS_TO_RAD
    if abs(turns) > _F32_MAX:
        raise ValueError(f"{radians} rad does not fit in the wire's float32")
    return turns


def turns_to_rad(turns: float) -> float:
    """Shaft revolutions to radians. Raises for a non-finite reading."""
    if not math.isfinite(turns):
        raise ValueError(f"{turns} turns is not a finite reading")
    return turns * TURNS_TO_RAD


def _need(data: bytes, size: int, what: str) -> None:
    if len(data) < size:
        raise ValueError(f"a {what} of {len(data)} byte(s) is short of the {size} {what} carries")


def parse_heartbeat(data: bytes) -> tuple[int, int, int, int, int]:
    """An axis error, a state byte, and the motor, encoder and controller flag bytes.

    The DBC models the flags as the single bits 40, 48, 56 (and trajectory-done as bit 63);
    the firmware sends whole bytes holding 0 or 1. Both readings agree on the values this
    driver acts on, and the bytes are returned whole so a firmware that ever sends more than
    a bit does not parse as healthy here.
    """
    _need(data, 8, "heartbeat")
    axis_error, state, motor, encoder, controller = struct.unpack_from("<IBBBB", data)
    return axis_error, state, motor, encoder, controller


def parse_estimates(data: bytes) -> tuple[float, float]:
    """Position and velocity estimates, in turns and turns/s."""
    _need(data, 8, "encoder estimates")
    return struct.unpack_from("<ff", data)


def parse_error_u32(data: bytes) -> int:
    """A 32-bit error word (encoder, controller, sensorless). The firmware sends it in an
    eight-byte frame with the word first; the tail is not read."""
    _need(data, 4, "error word")
    return struct.unpack_from("<I", data)[0]


def parse_error_u64(data: bytes) -> int:
    """The 64-bit motor error word. The DBC models 32 bits and the firmware sends 64 (F280);
    parsing 32 would drop every bit the maker added past the first word."""
    _need(data, 8, "motor error word")
    return struct.unpack_from("<Q", data)[0]


def request(node: int, cmd: int) -> tuple[int, bytes]:
    """A zero-length ``GET_*`` request: the firmware's equivalent of an RTR (see the module
    docstring), and the only request shape this driver sends."""
    return msg_id(node, cmd), b""


def set_requested_state(state: int) -> bytes:
    """An axis state as the firmware reads it: one int32, little-endian."""
    return struct.pack("<i", state)


def set_controller_modes(control_mode: int, input_mode: int) -> bytes:
    """A control mode and an input mode: two int32s, little-endian."""
    return struct.pack("<ii", control_mode, input_mode)


def set_input_pos(pos_turns: float, vel_ff_turns_s: float = 0.0, torque_ff_nm: float = 0.0) -> bytes:
    """A position setpoint: float32 turns, then two int16 feed-forwards at 0.001 (turns/s,
    Nm). The driver sends zero feed-forwards: the loop's own rate shapes the motion, and a
    feed-forward the kernel did not ask for would move the shaft past its setpoint."""
    return struct.pack("<fhh", pos_turns, _ff(vel_ff_turns_s), _ff(torque_ff_nm))


def set_input_vel(vel_turns_s: float, torque_ff_nm: float = 0.0) -> bytes:
    """A velocity setpoint: float32 turns/s, then a float32 torque feed-forward in Nm."""
    return struct.pack("<ff", vel_turns_s, torque_ff_nm)


def set_input_torque(torque_nm: float) -> bytes:
    """A torque setpoint: one float32 in Nm."""
    return struct.pack("<f", torque_nm)


def estop() -> bytes:
    """The stop frame's payload: none. The command id is the message; the firmware latches
    ``AXIS_ERROR_ESTOP_REQUESTED`` on receipt and the axis leaves closed loop."""
    return b""


def _ff(value: float) -> int:
    scaled = round(value * 1000.0)
    if not -32768 <= scaled <= 32767:
        raise ValueError(f"a feed-forward of {value} does not fit in the wire's int16")
    return scaled
