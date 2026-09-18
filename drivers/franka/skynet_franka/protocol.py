"""The Franka Control Interface on the wire, transcribed from the vendor's own definitions.

Two transports, and they are not alternatives:

* **TCP 1337** carries commands — connect, move, stop, error recovery. Request and reply, one
  `CommandHeader` each, and every reply carries the id of the request it answers.
* **UDP**, on a port the client names in `Connect`, carries the robot at **1 kHz**: a
  ``RobotState`` out on every tick and a ``RobotCommand`` back. Nothing is acknowledged and
  nothing is retried; a command that misses its tick is a command the robot did not get, and the
  robot counts them (``control_command_success_rate``).

## Where these numbers come from, and why that matters

`DRY-COVERAGE-PLAN.md` F15's rule is that a protocol transcribed from memory and tested against a
fake written from the same memory proves only that two copies of one misunderstanding agree. There
is no Franka simulator to measure against — the FCI needs a Control box — so the fake in
:mod:`skynet_franka.sim` is written here, and everything F15 warns about would apply.

What breaks the symmetry is that the **layout is not transcribed at all**. Franka Robotics publishes
`research_interface/robot/rbk_types.h` and `service_types.h` under Apache-2.0, and every offset and
size this module uses was computed by compiling those headers with ``offsetof`` and ``sizeof``.
``layout.json`` holds the result with the commit it came from, ``selftest.py`` asserts this module
against it on every run, and ``make check-layout`` regenerates it from a checkout. So a field that
moves in a future release fails a check here rather than arriving as a joint angle that is somebody
else's field read as a double.

What that does **not** establish is behaviour: that the robot sends what these structs say when it
says it, that `Move` is answered the way this driver expects, or that a 1 kHz loop holds. Those need
a Control box, and the README says so.

## Two things a careful reading would get wrong

**The state is floats and the command is doubles.** ``RobotState`` carries ``float`` arrays and
``RobotCommand`` carries ``double`` arrays, in the same protocol, in structs that sit beside each
other in one header. A transcription that assumed one or the other for both is wrong by a factor of
two in width on one side, and the first field would still parse.

**Everything is packed and little-endian.** ``#pragma pack(push, 1)`` covers both headers, so there
is no padding anywhere — including between a ``uint8_t`` status and the ``uint16_t`` after it.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, field
from typing import Sequence

#: The protocol version this transcription targets. `Connect` is refused by a Control box running a
#: different one, which is the vendor's own version gate and the only one there is.
VERSION = 10

#: Where a Control box listens for commands.
COMMAND_PORT = 1337

# ── Commands (service_types.h, `enum class Command : uint32_t`) ───────────────

COMMAND_CONNECT = 0
COMMAND_MOVE = 1
COMMAND_STOP_MOVE = 2
COMMAND_SET_COLLISION_BEHAVIOR = 3
COMMAND_SET_JOINT_IMPEDANCE = 4
COMMAND_SET_CARTESIAN_IMPEDANCE = 5
COMMAND_SET_GUIDING_MODE = 6
COMMAND_SET_EE_TO_K = 7
COMMAND_SET_NE_TO_EE = 8
COMMAND_SET_LOAD = 9
COMMAND_AUTOMATIC_ERROR_RECOVERY = 10
COMMAND_GET_ROBOT_MODEL = 11

COMMAND_NAMES = {
    COMMAND_CONNECT: "connect",
    COMMAND_MOVE: "move",
    COMMAND_STOP_MOVE: "stop_move",
    COMMAND_SET_COLLISION_BEHAVIOR: "set_collision_behavior",
    COMMAND_SET_JOINT_IMPEDANCE: "set_joint_impedance",
    COMMAND_SET_CARTESIAN_IMPEDANCE: "set_cartesian_impedance",
    COMMAND_SET_GUIDING_MODE: "set_guiding_mode",
    COMMAND_SET_EE_TO_K: "set_ee_to_k",
    COMMAND_SET_NE_TO_EE: "set_ne_to_ee",
    COMMAND_SET_LOAD: "set_load",
    COMMAND_AUTOMATIC_ERROR_RECOVERY: "automatic_error_recovery",
    COMMAND_GET_ROBOT_MODEL: "get_robot_model",
}

# ── Statuses ─────────────────────────────────────────────────────────────────

CONNECT_SUCCESS = 0
CONNECT_INCOMPATIBLE_LIBRARY_VERSION = 1

MOVE_SUCCESS = 0
MOVE_STARTED = 1
MOVE_PREEMPTED = 2
MOVE_PREEMPTED_DUE_TO_SAFETY = 3
MOVE_REJECTED_DUE_TO_SAFETY = 4
MOVE_COMMAND_NOT_POSSIBLE_REJECTED = 5
MOVE_START_AT_SINGULAR_POSE_REJECTED = 6
MOVE_INVALID_ARGUMENT_REJECTED = 7
MOVE_REFLEX_ABORTED = 8
MOVE_EMERGENCY_ABORTED = 9
MOVE_INPUT_ERROR_ABORTED = 10
MOVE_ABORTED = 11

MOVE_STATUS_NAMES = {
    MOVE_SUCCESS: "success",
    MOVE_STARTED: "motion started",
    MOVE_PREEMPTED: "preempted",
    MOVE_PREEMPTED_DUE_TO_SAFETY: "preempted by an activated safety function",
    MOVE_REJECTED_DUE_TO_SAFETY: "rejected by an activated safety function",
    MOVE_COMMAND_NOT_POSSIBLE_REJECTED: "not possible in this state",
    MOVE_START_AT_SINGULAR_POSE_REJECTED: "rejected: the start pose is singular",
    MOVE_INVALID_ARGUMENT_REJECTED: "rejected: invalid argument",
    MOVE_REFLEX_ABORTED: "aborted by a reflex",
    MOVE_EMERGENCY_ABORTED: "aborted by the emergency stop",
    MOVE_INPUT_ERROR_ABORTED: "aborted: input error",
    MOVE_ABORTED: "aborted",
}

# ── The robot's own modes (rbk_types.h) ──────────────────────────────────────

MOTION_IDLE = 0
MOTION_JOINT_POSITION = 1
MOTION_JOINT_VELOCITY = 2
MOTION_CARTESIAN_POSITION = 3
MOTION_CARTESIAN_VELOCITY = 4
MOTION_NONE = 5

CONTROLLER_JOINT_IMPEDANCE = 0
CONTROLLER_CARTESIAN_IMPEDANCE = 1
CONTROLLER_EXTERNAL = 2
CONTROLLER_OTHER = 3

#: `Move::Request`'s own enums, which are **uint32_t** and not the uint8_t ones above. Two
#: vocabularies for one idea, in one header, and the wire tells them apart only by width.
MOVE_CONTROLLER_JOINT_IMPEDANCE = 0
MOVE_CONTROLLER_CARTESIAN_IMPEDANCE = 1
MOVE_CONTROLLER_EXTERNAL = 2

MOVE_MOTION_JOINT_POSITION = 0
MOVE_MOTION_JOINT_VELOCITY = 1
MOVE_MOTION_CARTESIAN_POSITION = 2
MOVE_MOTION_CARTESIAN_VELOCITY = 3
MOVE_MOTION_NONE = 4

ROBOT_MODE_OTHER = 0
ROBOT_MODE_IDLE = 1
ROBOT_MODE_MOVE = 2
ROBOT_MODE_GUIDING = 3
ROBOT_MODE_REFLEX = 4
ROBOT_MODE_USER_STOPPED = 5
ROBOT_MODE_AUTOMATIC_ERROR_RECOVERY = 6

ROBOT_MODE_NAMES = {
    ROBOT_MODE_OTHER: "other",
    ROBOT_MODE_IDLE: "idle",
    ROBOT_MODE_MOVE: "move",
    ROBOT_MODE_GUIDING: "guiding",
    ROBOT_MODE_REFLEX: "reflex",
    ROBOT_MODE_USER_STOPPED: "user stopped",
    ROBOT_MODE_AUTOMATIC_ERROR_RECOVERY: "automatic error recovery",
}

#: Seven joints, and it is not a configuration: the FCI's structs are fixed at seven and a Franka
#: arm has seven. A body with a different number is a different protocol.
JOINTS = 7

# ── Sizes and offsets, held to `layout.json` ─────────────────────────────────

HEADER = struct.Struct("<III")  # command, command_id, size
HEADER_SIZE = HEADER.size

CONNECT_REQUEST = struct.Struct("<HH")  # version, udp_port
CONNECT_RESPONSE = struct.Struct("<BH")  # status, version

#: `Move::Request`: two uint32 enums, two `Deviation`s of three doubles, a bool, and seven doubles
#: of maximum velocity that only the async generator reads.
MOVE_REQUEST = struct.Struct("<II6d?7d")

ROBOT_STATE_SIZE = 1377
ROBOT_COMMAND_SIZE = 371

#: Offsets into `RobotState` for the fields a §24 driver reads. The rest of the 1,377 bytes is real
#: and is skipped rather than parsed: a Cartesian pose this driver does not publish is a field it
#: would have to keep correct for no reader.
STATE_MESSAGE_ID = 0
STATE_TAU_J = 512
STATE_Q = 596
STATE_DQ = 652
STATE_MOTION_GENERATOR_MODE = 1288
STATE_CONTROLLER_MODE = 1289
STATE_ERRORS = 1290
STATE_REFLEX_REASON = 1331
STATE_ROBOT_MODE = 1372
STATE_SUCCESS_RATE = 1373

#: 41 `bool`s, one byte each, packed.
ERROR_COUNT = 41

COMMAND_MESSAGE_ID = 0
COMMAND_MOTION = 8
COMMAND_CONTROL = 314
MOTION_Q_C = 0
MOTION_DQ_C = 56
MOTION_VALID_ELBOW = 304
MOTION_FINISHED = 305
CONTROL_TAU_J_D = 0
CONTROL_FINISHED = 56

_SEVEN_FLOATS = struct.Struct("<7f")
_SEVEN_DOUBLES = struct.Struct("<7d")


class ProtocolError(Exception):
    """A message that is not the message the protocol says comes next."""


@dataclass
class Header:
    """One `CommandHeader`: what, which exchange, and how many bytes in total."""

    command: int
    command_id: int
    size: int

    def pack(self) -> bytes:
        return HEADER.pack(self.command, self.command_id, self.size)

    @staticmethod
    def unpack(raw: bytes) -> "Header":
        if len(raw) < HEADER_SIZE:
            raise ProtocolError(f"a command header is {HEADER_SIZE} bytes and {len(raw)} arrived")
        command, command_id, size = HEADER.unpack_from(raw, 0)
        if size < HEADER_SIZE:
            raise ProtocolError(
                f"a command header declares {size} bytes in total, which is less than the header "
                f"itself ({HEADER_SIZE}). Nothing can be read from it."
            )
        return Header(command, command_id, size)


@dataclass
class RobotState:
    """The seven-joint half of `RobotState`, and the flags a driver has to act on.

    Everything here is read out of the 1,377 bytes at the offsets above. The rest of the message is
    not parsed and not discarded — :attr:`raw` keeps it — so a caller that needs the Cartesian pose
    later reads it from the bytes the robot sent rather than from a field this class kept warm.
    """

    message_id: int
    q: list[float]
    dq: list[float]
    tau_J: list[float]
    robot_mode: int
    motion_generator_mode: int
    controller_mode: int
    errors: list[bool]
    reflex_reason: list[bool]
    success_rate: float
    raw: bytes = b""

    @staticmethod
    def unpack(raw: bytes) -> "RobotState":
        if len(raw) != ROBOT_STATE_SIZE:
            raise ProtocolError(
                f"a RobotState is {ROBOT_STATE_SIZE} bytes and {len(raw)} arrived. A short read "
                f"here is a different firmware, not a partial message: this is one UDP datagram."
            )
        message_id = struct.unpack_from("<Q", raw, STATE_MESSAGE_ID)[0]
        return RobotState(
            message_id=message_id,
            q=list(_SEVEN_FLOATS.unpack_from(raw, STATE_Q)),
            dq=list(_SEVEN_FLOATS.unpack_from(raw, STATE_DQ)),
            tau_J=list(_SEVEN_FLOATS.unpack_from(raw, STATE_TAU_J)),
            robot_mode=raw[STATE_ROBOT_MODE],
            motion_generator_mode=raw[STATE_MOTION_GENERATOR_MODE],
            controller_mode=raw[STATE_CONTROLLER_MODE],
            errors=[bool(b) for b in raw[STATE_ERRORS : STATE_ERRORS + ERROR_COUNT]],
            reflex_reason=[
                bool(b) for b in raw[STATE_REFLEX_REASON : STATE_REFLEX_REASON + ERROR_COUNT]
            ],
            success_rate=struct.unpack_from("<f", raw, STATE_SUCCESS_RATE)[0],
            raw=raw,
        )

    def faulted(self) -> bool:
        """Whether any of the 41 error bits is set.

        Read as a whole rather than by name. §24 surfaces a vendor fault code verbatim and the OS
        does not interpret it, so which bit is set belongs in `fault_code` and in the black box —
        not in a branch here that would go stale the first time Franka adds a forty-second.
        """
        return any(self.errors)

    def fault_code(self) -> int:
        """The lowest set error bit, one-based, or 0.

        §24's `fault_code` is *vendor-specific and surfaced verbatim*, so this is the index of the
        first error the robot reported rather than anything this driver invented. Zero is a healthy
        drive, which is a measurement, and it is why an unread field is absent rather than zero.
        """
        for index, bit in enumerate(self.errors):
            if bit:
                return index + 1
        return 0


def pack_robot_command(
    message_id: int,
    q_c: Sequence[float] | None = None,
    dq_c: Sequence[float] | None = None,
    tau_J_d: Sequence[float] | None = None,
    motion_finished: bool = False,
) -> bytes:
    """One `RobotCommand`, 371 bytes, with the fields this driver sets and zeros elsewhere.

    Every field is a **double** here and a float in the state, which is the asymmetry this module's
    header calls out. Unset arrays are zeros rather than the last value: the robot reads the mode it
    was started in and ignores the rest, so a stale array left in a field the robot does not read
    costs nothing and a stale array in one it *does* read is a command nobody issued.
    """
    out = bytearray(ROBOT_COMMAND_SIZE)
    struct.pack_into("<Q", out, COMMAND_MESSAGE_ID, message_id)
    if q_c is not None:
        _SEVEN_DOUBLES.pack_into(out, COMMAND_MOTION + MOTION_Q_C, *q_c)
    if dq_c is not None:
        _SEVEN_DOUBLES.pack_into(out, COMMAND_MOTION + MOTION_DQ_C, *dq_c)
    if tau_J_d is not None:
        _SEVEN_DOUBLES.pack_into(out, COMMAND_CONTROL + CONTROL_TAU_J_D, *tau_J_d)
    out[COMMAND_MOTION + MOTION_FINISHED] = 1 if motion_finished else 0
    out[COMMAND_CONTROL + CONTROL_FINISHED] = 1 if motion_finished else 0
    return bytes(out)


def unpack_robot_command(raw: bytes) -> dict:
    """The fields :func:`pack_robot_command` sets, read back. For the fake, and for the suite."""
    if len(raw) != ROBOT_COMMAND_SIZE:
        raise ProtocolError(
            f"a RobotCommand is {ROBOT_COMMAND_SIZE} bytes and {len(raw)} arrived."
        )
    return {
        "message_id": struct.unpack_from("<Q", raw, COMMAND_MESSAGE_ID)[0],
        "q_c": list(_SEVEN_DOUBLES.unpack_from(raw, COMMAND_MOTION + MOTION_Q_C)),
        "dq_c": list(_SEVEN_DOUBLES.unpack_from(raw, COMMAND_MOTION + MOTION_DQ_C)),
        "tau_J_d": list(_SEVEN_DOUBLES.unpack_from(raw, COMMAND_CONTROL + CONTROL_TAU_J_D)),
        "motion_finished": bool(raw[COMMAND_MOTION + MOTION_FINISHED]),
    }


def pack_robot_state(
    message_id: int,
    q: Sequence[float],
    dq: Sequence[float],
    tau_J: Sequence[float],
    robot_mode: int = ROBOT_MODE_IDLE,
    motion_generator_mode: int = MOTION_IDLE,
    controller_mode: int = CONTROLLER_JOINT_IMPEDANCE,
    errors: Sequence[bool] = (),
    success_rate: float = 1.0,
) -> bytes:
    """One `RobotState`, 1,377 bytes. Used by the fake and by the suite's own decoder tests."""
    out = bytearray(ROBOT_STATE_SIZE)
    struct.pack_into("<Q", out, STATE_MESSAGE_ID, message_id)
    _SEVEN_FLOATS.pack_into(out, STATE_Q, *q)
    _SEVEN_FLOATS.pack_into(out, STATE_DQ, *dq)
    _SEVEN_FLOATS.pack_into(out, STATE_TAU_J, *tau_J)
    out[STATE_ROBOT_MODE] = robot_mode
    out[STATE_MOTION_GENERATOR_MODE] = motion_generator_mode
    out[STATE_CONTROLLER_MODE] = controller_mode
    for index, bit in enumerate(errors):
        if index < ERROR_COUNT and bit:
            out[STATE_ERRORS + index] = 1
    struct.pack_into("<f", out, STATE_SUCCESS_RATE, success_rate)
    return bytes(out)


def pack_move_payload(
    controller_mode: int,
    motion_generator_mode: int,
    path_deviation: tuple[float, float, float] = (10.0, 10.0, 10.0),
    goal_deviation: tuple[float, float, float] = (10.0, 10.0, 10.0),
) -> bytes:
    """A `Move::Request`, 113 bytes, without its header.

    The deviations default wide deliberately. They are the Control box's own tolerance on how far a
    motion may stray before it aborts, and a driver that narrowed them would be putting a second
    envelope in front of §7's — one the Safety Kernel cannot see, cannot report and did not choose.
    """
    return MOVE_REQUEST.pack(
        controller_mode,
        motion_generator_mode,
        *path_deviation,
        *goal_deviation,
        False,
        *([0.0] * JOINTS),
    )


def pack_move_request(command_id: int, controller_mode: int, motion_generator_mode: int) -> bytes:
    """The same, as a whole command message."""
    payload = pack_move_payload(controller_mode, motion_generator_mode)
    return Header(COMMAND_MOVE, command_id, HEADER_SIZE + len(payload)).pack() + payload


def pack_command(command: int, command_id: int, payload: bytes = b"") -> bytes:
    """Any command message: a header sized to its payload, then the payload."""
    return Header(command, command_id, HEADER_SIZE + len(payload)).pack() + payload


def pack_response(command: int, command_id: int, status: int, extra: bytes = b"") -> bytes:
    """A reply: the same header, then a one-byte status and whatever the reply carries after it."""
    payload = bytes([status]) + extra
    return Header(command, command_id, HEADER_SIZE + len(payload)).pack() + payload
