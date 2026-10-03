"""The ROS-Industrial `simple_message` wire: what a controller's ROS-I server puts on a socket, and nothing about what to do with it.

Pure, like `skynet_xarm.frames` and `skynet_mavlink.frames`: no sockets, no clocks, no state, so every byte here can be held to the maker's own library without a controller. Every constant carries
the file and symbol it was read out of, in ROS-Industrial's `industrial_core` (`simple_message`, BSD-3-Clause, not vendored; `melodic-devel`, the branch its controller-side servers —
MotoROS, `abb_driver`'s RAPID, `fanuc_driver` — are written against), because a transcription with no citation is a rumour. `layout_check.py` compiles the maker's library (without ROS) and requires every message here to be the bytes the maker's own `toByteArray` makes, and to read the maker's bytes back.

# The envelope

Every message is, on a TCP stream (`SmplMsgConnection::sendMsg`, `SimpleMessage::toByteArray`):

    length  int   what follows, in bytes: the header and the body (it does not count itself)
    type    int   `StandardMsgTypes` (and the vendors' own ranges from 1000)
    comm    int   `CommTypes`: a topic, a service request, a service reply
    reply   int   `ReplyTypes`: only for a service reply, and INVALID (0) for everything else
    body          the message's own bytes

and the maker's `SimpleMessage::validateMessage` refuses a type of 0, a comm of 0, a service reply with no reply code, and a reply code on anything but a service reply. The maker's socket
reads at most `SimpleSocket::MAX_BUFFER_SIZE` (1024) bytes of a message.

# What an `int` and a `real` are

`shared_types.h`: `shared_int` is 4 bytes and `shared_real` a 4-byte float, *unless the controller's build says otherwise*: `FLOAT64` makes a real 8 bytes, and `BYTE_SWAPPING` reverses the
bytes of both. A controller whose CPU is big-endian is built with the second, and nothing on the wire says which a controller is, so this is a configuration: `byte_order` and `real_bytes`,
with the default the maker's own default (little-endian, 4-byte reals). `INT64` is not supported and the configuration refuses it.

# The messages this driver speaks

* **`JOINT_TRAJ_PT` (11)**, a service request, to the motion port: `seq` int, ten joint reals (radians), `velocity` real, `duration` real. `JointTrajectoryInterface::calc_velocity` makes the
  velocity a *ratio* of the controller's own maximum for the point's critical joint (0 to 1), and the duration the time the move should take. The sequence number is how a controller knows a
  trajectory from a stream of unrelated points: 0 starts one, and the special values below end it.
* **`JOINT_FEEDBACK` (15)** and **`JOINT` (10, `JOINT_POSITION`)**, topics, from the state port: the feedback carries a robot id, a bitmask of the fields that are valid, a time and three
  joint arrays (position, velocity, acceleration); the older message is a sequence number and the ten positions.
* **`STATUS` (13)**, a topic, from the state port: seven ints (`RobotStatus`): drives powered, e-stopped, error code, in error, in motion, mode, motion possible. Each is a tri-state:
  1 true, 0 false, -1 unknown.
* **`PING` (1)**, a service request with no body, answered with a service reply.
"""

from __future__ import annotations

import math
import struct
from dataclasses import dataclass

__all__ = [
    "MAX_JOINTS",
    "MAX_MESSAGE",
    "Comm",
    "Feedback",
    "Format",
    "Message",
    "MsgType",
    "Port",
    "ProtocolError",
    "Reply",
    "RobotMode",
    "Seq",
    "Status",
    "Stream",
    "TrajPt",
    "TriState",
    "Valid",
    "encode",
    "joint_feedback",
    "joint_position",
    "joint_traj_pt",
    "parse_joint_feedback",
    "parse_joint_position",
    "parse_joint_traj_pt",
    "parse_robot_status",
    "ping",
    "robot_status",
]

#: `JointData::MAX_NUM_JOINTS` (`joint_data.h`): a message always carries ten joints, whatever the arm has.
MAX_JOINTS = 10
#: `SimpleSocket::MAX_BUFFER_SIZE` (`socket/simple_socket.h`): the most the maker's socket reads for one message.
MAX_MESSAGE = 1024
#: The header after the length: type, comm, reply, each an int.
_HEADER_INTS = 3


class Port:
    """`StandardSocketPorts` (`socket/simple_socket.h`): the ports the maker's own client and server use, and a controller's server listens on by convention."""

    MOTION = 11000
    SYSTEM = 11001
    STATE = 11002
    IO = 11003


class MsgType:
    """`StandardMsgTypes::StandardMsgType` (`simple_message.h`)."""

    INVALID = 0
    PING = 1
    JOINT = 10  # also JOINT_POSITION
    JOINT_POSITION = 10
    JOINT_TRAJ_PT = 11
    JOINT_TRAJ = 12
    STATUS = 13
    JOINT_TRAJ_PT_FULL = 14
    JOINT_FEEDBACK = 15
    READ_INPUT = 20
    WRITE_OUTPUT = 21


class Comm:
    """`CommTypes::CommType`."""

    INVALID = 0
    TOPIC = 1
    SERVICE_REQUEST = 2
    SERVICE_REPLY = 3


class Reply:
    """`ReplyTypes::ReplyType`."""

    INVALID = 0
    SUCCESS = 1
    FAILURE = 2


class Seq:
    """`SpecialSeqValues::SpecialSeqValue` (`joint_traj_pt.h`): the sequence numbers that are not a point's place in a trajectory."""

    START_TRAJECTORY_DOWNLOAD = -1
    START_TRAJECTORY_STREAMING = -2
    END_TRAJECTORY = -3
    STOP_TRAJECTORY = -4


class Valid:
    """`ValidFieldTypes` (`joint_feedback.h`): which fields of a feedback message the controller filled."""

    TIME = 0x01
    POSITION = 0x02
    VELOCITY = 0x04
    ACCELERATION = 0x08


class TriState:
    """`TriState` (`robot_status.h`): unknown is -1, and a status field that is unknown is *not* false."""

    UNKNOWN = -1
    FALSE = 0
    TRUE = 1


class RobotMode:
    """`RobotMode` (`robot_status.h`)."""

    UNKNOWN = -1
    MANUAL = 1
    AUTO = 2


class ProtocolError(Exception):
    """Bytes that are not a `simple_message`, or one this driver will not read."""


@dataclass(frozen=True)
class Format:
    """How a controller's build spells `shared_int` and `shared_real` (`shared_types.h`): the maker's `BYTE_SWAPPING` and `FLOAT64`."""

    byte_order: str = "little"
    real_bytes: int = 4

    def __post_init__(self) -> None:
        if self.byte_order not in ("little", "big"):
            raise ValueError(f"`byte_order` is `little` or `big`, not {self.byte_order!r}")
        if self.real_bytes not in (4, 8):
            raise ValueError(f"`real_bytes` is 4 or 8, not {self.real_bytes!r}")

    @property
    def _prefix(self) -> str:
        return "<" if self.byte_order == "little" else ">"

    @property
    def int_size(self) -> int:
        return 4

    def pack_int(self, value: int) -> bytes:
        return struct.pack(self._prefix + "i", value)

    def unpack_int(self, raw: bytes) -> int:
        return struct.unpack(self._prefix + "i", raw)[0]

    def pack_real(self, value: float) -> bytes:
        return struct.pack(self._prefix + ("f" if self.real_bytes == 4 else "d"), value)

    def unpack_real(self, raw: bytes) -> float:
        return struct.unpack(self._prefix + ("f" if self.real_bytes == 4 else "d"), raw)[0]

    def reals(self, values: list[float]) -> bytes:
        return b"".join(self.pack_real(value) for value in values)

    def read_reals(self, raw: bytes, count: int) -> list[float]:
        return [self.unpack_real(raw[i * self.real_bytes:(i + 1) * self.real_bytes]) for i in range(count)]

    @property
    def joint_data_size(self) -> int:
        return MAX_JOINTS * self.real_bytes


#: The maker's own default build: little-endian, 4-byte reals. A frozen value, so it is safe as a default argument.
DEFAULT_FORMAT = Format()


@dataclass(frozen=True)
class Message:
    type: int
    comm: int
    reply: int
    body: bytes


def _validate(msg_type: int, comm: int, reply: int) -> None:
    """`SimpleMessage::validateMessage`, word for word."""
    if msg_type == MsgType.INVALID:
        raise ProtocolError("a message of type 0 (INVALID)")
    if comm == Comm.INVALID:
        raise ProtocolError("a message whose comm type is 0 (INVALID)")
    if comm == Comm.SERVICE_REPLY and reply == Reply.INVALID:
        raise ProtocolError("a service reply with no reply code")
    if comm != Comm.SERVICE_REPLY and reply != Reply.INVALID:
        raise ProtocolError(f"a reply code ({reply}) on a message that is not a service reply")


def encode(msg_type: int, comm: int, reply: int, body: bytes, fmt: Format = DEFAULT_FORMAT) -> bytes:
    """One message as it goes on a stream: the length, the header and the body."""
    _validate(msg_type, comm, reply)
    size = _HEADER_INTS * fmt.int_size + len(body)
    if fmt.int_size + size >= MAX_MESSAGE:
        # `SimpleSocket::sendBytes` refuses a buffer of `MAX_BUFFER_SIZE` or more, counting the length.
        raise ProtocolError(f"a message of {fmt.int_size + size} bytes does not fit the {MAX_MESSAGE}-byte buffer the maker's socket reads")
    return fmt.pack_int(size) + fmt.pack_int(msg_type) + fmt.pack_int(comm) + fmt.pack_int(reply) + body


class Stream:
    """A TCP stream cut into messages. Bytes are fed in as they arrive; whole messages come out.

    A message is complete when `length` bytes have followed its length. A length that is not a message (smaller than a header, or larger than the maker's own socket would read) is a stream
    this driver has lost its place in, and it says so rather than reading on from a guess.
    """

    def __init__(self, fmt: Format = DEFAULT_FORMAT) -> None:
        self._fmt = fmt
        self._buffer = bytearray()

    def feed(self, data: bytes) -> list[Message]:
        self._buffer += data
        out: list[Message] = []
        size = self._fmt.int_size
        while len(self._buffer) >= size:
            length = self._fmt.unpack_int(bytes(self._buffer[:size]))
            if length < _HEADER_INTS * size or size + length >= MAX_MESSAGE:
                raise ProtocolError(
                    f"a length prefix of {length} bytes. The smallest message is {_HEADER_INTS * size} (a header with no body) and the maker's socket reads under {MAX_MESSAGE}: "
                    "this stream is not simple_message, or it is one this driver has lost its place in (a wrong `byte_order` reads a length of 0x0C000000)."
                )
            if len(self._buffer) < size + length:
                break
            frame = bytes(self._buffer[size:size + length])
            del self._buffer[:size + length]
            msg_type, comm, reply = (self._fmt.unpack_int(frame[i * size:(i + 1) * size]) for i in range(_HEADER_INTS))
            _validate(msg_type, comm, reply)
            out.append(Message(msg_type, comm, reply, frame[_HEADER_INTS * size:]))
        return out


def _padded(joints: list[float]) -> list[float]:
    if len(joints) > MAX_JOINTS:
        raise ProtocolError(f"{len(joints)} joints do not fit a message of {MAX_JOINTS}")
    return list(joints) + [0.0] * (MAX_JOINTS - len(joints))


@dataclass(frozen=True)
class TrajPt:
    seq: int
    joints: tuple[float, ...]
    velocity: float
    duration: float


def joint_traj_pt(seq: int, joints: list[float], velocity: float, duration: float, fmt: Format = DEFAULT_FORMAT) -> bytes:
    """The body of a `JOINT_TRAJ_PT` (`JointTrajPt::load`): the sequence, the ten joints, the velocity ratio, the duration."""
    for name, value in (("velocity", velocity), ("duration", duration), *((f"joint {i}", v) for i, v in enumerate(joints))):
        if not math.isfinite(value):
            raise ProtocolError(f"{name} is {value}: a message that carries a NaN is a message nobody can act on")
    return fmt.pack_int(seq) + fmt.reals(_padded(joints)) + fmt.pack_real(velocity) + fmt.pack_real(duration)


def parse_joint_traj_pt(body: bytes, fmt: Format = DEFAULT_FORMAT) -> TrajPt:
    want = fmt.int_size + fmt.joint_data_size + 2 * fmt.real_bytes
    if len(body) != want:
        raise ProtocolError(f"a JOINT_TRAJ_PT body of {len(body)} bytes; with {fmt.real_bytes}-byte reals it is {want}")
    seq = fmt.unpack_int(body[:fmt.int_size])
    joints = fmt.read_reals(body[fmt.int_size:fmt.int_size + fmt.joint_data_size], MAX_JOINTS)
    velocity, duration = fmt.read_reals(body[fmt.int_size + fmt.joint_data_size:], 2)
    return TrajPt(seq, tuple(joints), velocity, duration)


@dataclass(frozen=True)
class Feedback:
    robot_id: int
    valid: int
    time: float
    positions: tuple[float, ...]
    velocities: tuple[float, ...]
    accelerations: tuple[float, ...]


def joint_feedback(robot_id: int, valid: int, time: float, positions: list[float], velocities: list[float], accelerations: list[float], fmt: Format = DEFAULT_FORMAT) -> bytes:
    """The body of a `JOINT_FEEDBACK` (`JointFeedback::load`): robot id, valid fields, time, then positions, velocities and accelerations."""
    return (fmt.pack_int(robot_id) + fmt.pack_int(valid) + fmt.pack_real(time)
            + fmt.reals(_padded(positions)) + fmt.reals(_padded(velocities)) + fmt.reals(_padded(accelerations)))


def parse_joint_feedback(body: bytes, fmt: Format = DEFAULT_FORMAT) -> Feedback:
    want = 2 * fmt.int_size + fmt.real_bytes + 3 * fmt.joint_data_size
    if len(body) != want:
        raise ProtocolError(f"a JOINT_FEEDBACK body of {len(body)} bytes; with {fmt.real_bytes}-byte reals it is {want}")
    size = fmt.int_size
    robot_id, valid = fmt.unpack_int(body[:size]), fmt.unpack_int(body[size:2 * size])
    at = 2 * size
    time = fmt.unpack_real(body[at:at + fmt.real_bytes])
    at += fmt.real_bytes
    arrays = []
    for _ in range(3):
        arrays.append(tuple(fmt.read_reals(body[at:at + fmt.joint_data_size], MAX_JOINTS)))
        at += fmt.joint_data_size
    return Feedback(robot_id, valid, time, *arrays)


def joint_position(seq: int, joints: list[float], fmt: Format = DEFAULT_FORMAT) -> bytes:
    """The body of the older `JOINT` message (`JointMessage::load`): a sequence number, then the ten positions."""
    return fmt.pack_int(seq) + fmt.reals(_padded(joints))


def parse_joint_position(body: bytes, fmt: Format = DEFAULT_FORMAT) -> tuple[int, tuple[float, ...]]:
    """The older `JOINT` message: its sequence number and the ten positions. Found by `layout_check.py`: the first version of this reader took the positions alone."""
    want = fmt.int_size + fmt.joint_data_size
    if len(body) != want:
        raise ProtocolError(f"a JOINT body of {len(body)} bytes; with {fmt.real_bytes}-byte reals it is {want}")
    return fmt.unpack_int(body[:fmt.int_size]), tuple(fmt.read_reals(body[fmt.int_size:], MAX_JOINTS))


@dataclass(frozen=True)
class Status:
    drives_powered: int
    e_stopped: int
    error_code: int
    in_error: int
    in_motion: int
    mode: int
    motion_possible: int


def robot_status(status: Status, fmt: Format = DEFAULT_FORMAT) -> bytes:
    """The body of a `STATUS` (`RobotStatus::load`): seven ints, in the maker's order."""
    return b"".join(fmt.pack_int(value) for value in (status.drives_powered, status.e_stopped, status.error_code, status.in_error, status.in_motion, status.mode, status.motion_possible))


def parse_robot_status(body: bytes, fmt: Format = DEFAULT_FORMAT) -> Status:
    want = 7 * fmt.int_size
    if len(body) != want:
        raise ProtocolError(f"a STATUS body of {len(body)} bytes; it is {want}")
    return Status(*(fmt.unpack_int(body[i * fmt.int_size:(i + 1) * fmt.int_size]) for i in range(7)))


def ping(fmt: Format = DEFAULT_FORMAT) -> bytes:
    """A `PING`: a service request with no body."""
    return encode(MsgType.PING, Comm.SERVICE_REQUEST, Reply.INVALID, b"", fmt)
