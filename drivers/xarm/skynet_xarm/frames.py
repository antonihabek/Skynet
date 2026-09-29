"""The xArm controller's wire: what it puts on a socket, and nothing about what to do with it.

Pure, like `skynet_canopen.frames`: no sockets, no clocks, no state, so every byte here can be held to
the vendor's own source without a controller. Every constant carries the file and symbol it was read
out of, in UFACTORY's `xArm-Python-SDK` (BSD-3-Clause, not vendored), because a transcription with no
citation is a rumour and the suite's `layout_check.py` opens those files and requires the text to
still be there.

# Two kinds of socket

**The control port, 502.** A Modbus-TCP-shaped envelope carrying UFACTORY's own register table —
`XCONF.SocketConf.TCP_CONTROL_PORT` and `XCONF.UxbusReg` in `xarm/core/config/x_config.py`. A request is

    transaction id u16 | protocol id u16 = 2 | length u16 = data + 1 | function u8 | data

all big-endian, and a reply repeats the header, echoes the function, and puts a **state byte** in front
of its payload (`UxbusCmdTcp.send_modbus_request` and `check_private_protocol` in
`xarm/core/wrapper/uxbus_cmd_tcp.py`). The state byte is where the controller says *not ready*
(`0x10`), *invalid* (`0x08`), *error* (`0x40`) and *warning* (`0x20`) about the request it is
answering. A command's data is little-endian: `UxbusCmd.move_servoj` sends ten float32 —
seven joint angles, then speed, acceleration and time, which servo mode ignores.

**The report ports, 30000 to 30003.** The controller pushes a length-prefixed frame at a fixed rate
to whoever connects: `data_size u32` big-endian (the frame's whole length, itself included), then
the fields of `xarm/x3/report.py`'s structures, little-endian floats. This driver reads two:
port 30003, the fast one (100 Hz on the simulator's firmware) — state and mode, the command counter and
the seven joint angles — and port 30002, the rich one, for what is true of the robot rather than of the
moment: its type, its axis count, the firmware version, whether the controller says it is a
simulation, the joint ranges, the error and warning codes and which motors are enabled.

A frame is parsed by **offset**, and only as far as this driver reads. The vendor's structures are
newer than some firmware's frames — the 2.4.0 simulator's rich frame is 574 bytes and the current
SDK's structure is 587 — so a parser that demanded the structure's whole size would refuse a
controller for having fewer trailing fields than a library that has not shipped to it.
"""

from __future__ import annotations

import math
import struct
from dataclasses import dataclass

_TAU = 2 * math.pi

#: UFACTORY's own table of every joint's range, per arm: `(axes, type) -> ((low, high) per joint)`, in radians.
#: Source: `xArm-Python-SDK`, `xarm/core/config/x_config.py`, `XCONF.Robot.JOINT_LIMITS`, at
#: d911319cebc45142613e18086aabb8067b18ab7f (the pin `sim/Dockerfile` builds), which the vendor's own client checks a
#: target against before it sends one. `layout_check.py` opens the SDK and requires every entry to still be its.
#: The controller reports a range of its own (`reduced_joint_limits`); the driver takes the narrower of the two
#: (F181: UFACTORY's simulator reports the xArm 6's ranges on every model it starts).
VENDOR_JOINT_LIMITS: dict[tuple[int, int], tuple[tuple[float, float], ...]] = {
    (5, 5): ((-_TAU, _TAU), (-2.059488, 2.094395), (-3.92699, 0.191986), (-1.692969, math.pi), (-_TAU, _TAU)),
    (5, 51305): ((-_TAU, _TAU), (-2.042035, 2.024581), (-3.822271, 0.174532), (-1.692969, math.pi), (-_TAU, _TAU)),
    (6, 6): ((-_TAU, _TAU), (-2.059488, 2.094395), (-3.92699, 0.191986), (-_TAU, _TAU), (-1.692969, math.pi), (-_TAU, _TAU)),
    (6, 8): ((-_TAU, _TAU), (-2.059488, 2.094395), (-0.191986, 3.92699), (-_TAU, _TAU), (-1.692969, math.pi), (-_TAU, _TAU)),
    (6, 9): ((-_TAU, _TAU), (-2.6179938779914944, 2.6179938779914944), (-0.061086523819801536, 5.235987755982989), (-_TAU, _TAU), (-2.1642082724729685, 2.1642082724729685), (-_TAU, _TAU)),
    (6, 11): ((-_TAU, _TAU), (-_TAU, _TAU), (-_TAU, _TAU), (-_TAU, _TAU), (-1.692969, math.pi), (-_TAU, _TAU)),
    (6, 12): ((-_TAU, _TAU), (-2.303834612632515, 2.303834612632515), (-4.223696789826278, 0.061086523819801536), (-_TAU, _TAU), (-2.1642082724729685, 2.1642082724729685), (-_TAU, _TAU)),
    (6, 61305): ((-_TAU, _TAU), (-2.042035, 2.024581), (-3.822271, 0.174532), (-_TAU, _TAU), (-1.692969, math.pi), (-_TAU, _TAU)),
    (7, 3): ((-_TAU, _TAU), (-2.059488, 2.094395), (-_TAU, _TAU), (-3.92699, 0.191986), (-_TAU, _TAU), (-1.692969, math.pi), (-_TAU, _TAU)),
    (7, 7): ((-_TAU, _TAU), (-2.059488, 2.094395), (-_TAU, _TAU), (-0.191986, 3.92699), (-_TAU, _TAU), (-1.692969, math.pi), (-_TAU, _TAU)),
    (7, 13): ((-_TAU, _TAU), (-2.094395, 2.059488), (-_TAU, _TAU), (-3.92699, 0.191986), (-_TAU, _TAU), (-math.pi, 1.692969), (-_TAU, _TAU)),
    (7, 71305): ((-_TAU, _TAU), (-2.042035, 2.024581), (-_TAU, _TAU), (-0.10472, 3.92699), (-_TAU, _TAU), (-1.692969, math.pi), (-_TAU, _TAU)),
}

__all__ = [
    "VENDOR_JOINT_LIMITS",
    "CONTROL_PORT",
    "REPORT_RICH_PORT",
    "REPORT_REAL_PORT",
    "Fun",
    "State",
    "StateByte",
    "PROTOCOL_ID",
    "request",
    "parse_reply",
    "servoj_data",
    "ReportStream",
    "FastReport",
    "RichReport",
    "parse_fast",
    "parse_rich",
    "ProtocolError",
]

#: `XCONF.SocketConf` — the control socket, and the two report sockets this driver reads.
CONTROL_PORT = 502
REPORT_RICH_PORT = 30002
REPORT_REAL_PORT = 30003

#: `UxbusCmdTcp.PRIVATE_MODBUS_TCP_PROTOCOL` — the protocol identifier of UFACTORY's own register table.
PROTOCOL_ID = 0x02


class Fun:
    """`XCONF.UxbusReg`: the registers this driver speaks, and no others.

    The firmware **does not answer a register it does not know** — `GET_MAX_JOINT_VELOCITY` (231),
    which the SDK sends, timed out on the 2.4.0 simulator's controller — so the list is closed on
    purpose: a driver that probed for what a firmware supports would wait out a timeout on every
    miss.
    """

    GET_VERSION = 1
    MOTION_EN = 11
    SET_STATE = 12
    GET_STATE = 13
    GET_ERROR = 15
    CLEAN_ERR = 16
    CLEAN_WAR = 17
    SET_MODE = 19
    MOVE_SERVOJ = 29


class State:
    """The controller's `state`, the low nibble of a report's `state_mode` byte, and `GET_STATE`'s answer.

    Only the values this driver acts on are named. `xarm/x3/base.py` documents 1 as moving, 2 as
    ready, 3 as paused and 4 as stopped; 0 is what `SET_STATE 0` asks for and is not a state a report
    holds for long.
    """

    MOVING = 1
    READY = 2
    PAUSED = 3
    STOPPED = 4

    #: `SET_STATE`'s argument to make the arm take motion commands, and to stop it.
    RUN = 0
    STOP = 4


class StateByte:
    """The bits of a reply's state byte (`UxbusCmdTcp.check_private_protocol`)."""

    INVALID = 0x08
    NOT_READY = 0x10
    WARNING = 0x20
    ERROR = 0x40


class ProtocolError(ValueError):
    """A frame that is not what the register table says it must be."""


def request(transaction: int, function: int, data: bytes = b"") -> bytes:
    """One request, as `send_modbus_request` frames it."""
    if not 0 <= transaction <= 0xFFFF:
        raise ProtocolError(f"transaction id {transaction} does not fit in 16 bits")
    if not 0 <= function <= 0xFF:
        raise ProtocolError(f"register {function} does not fit in a byte")
    return struct.pack(">HHHB", transaction, PROTOCOL_ID, len(data) + 1, function) + data


@dataclass(frozen=True)
class Reply:
    transaction: int
    function: int
    state_byte: int
    payload: bytes

    @property
    def not_ready(self) -> bool:
        return bool(self.state_byte & StateByte.NOT_READY)

    @property
    def error(self) -> bool:
        return bool(self.state_byte & StateByte.ERROR)

    @property
    def warning(self) -> bool:
        return bool(self.state_byte & StateByte.WARNING)

    @property
    def invalid(self) -> bool:
        return bool(self.state_byte & StateByte.INVALID)


def parse_reply(header: bytes, body: bytes, *, transaction: int, function: int) -> Reply:
    """A reply from its six header bytes and its body, held to the request it answers.

    A reply that echoes another transaction or another register is somebody else's answer — a late
    one from a request that timed out — and reading it as this one's is how a driver reports the
    state the arm was in a second ago.
    """
    if len(header) != 6:
        raise ProtocolError(f"a reply header is six bytes, not {len(header)}")
    got_transaction, protocol, length = struct.unpack(">HHH", header)
    if protocol != PROTOCOL_ID:
        raise ProtocolError(
            f"the controller answered under protocol {protocol}, and only {PROTOCOL_ID} — UFACTORY's own "
            "register table — is spoken here"
        )
    if length < 2 or len(body) != length:
        raise ProtocolError(f"a reply of {length} bytes with a body of {len(body)}")
    if got_transaction != transaction:
        raise ProtocolError(f"a reply to transaction {got_transaction} where {transaction} was asked")
    if body[0] != function:
        raise ProtocolError(f"a reply about register {body[0]} where register {function} was asked")
    return Reply(got_transaction, body[0], body[1], bytes(body[2:]))


def servoj_data(angles: list[float] | tuple[float, ...]) -> bytes:
    """`UxbusCmd.move_servoj`'s data: seven angles then speed, acceleration and time, all float32.

    Servo mode takes its trajectory from the stream of targets, so the last three are zero: the
    driver has no view of speed or time that the loop's period does not already carry, and one that
    sent a number there would be second-guessing the mode it chose.
    """
    if len(angles) != 7:
        raise ProtocolError(f"servo targets are seven joints wide (unused axes zero), not {len(angles)}")
    return struct.pack("<10f", *angles, 0.0, 0.0, 0.0)


class ReportStream:
    """Cuts a report socket's bytes into frames.

    The prefix is the frame's whole length, and a stream whose prefix is not one a report can have
    is refused rather than resynchronised: nothing in the format marks where a frame starts, so a
    cut made at the wrong byte reads the next four bytes of a float as a length and produces a
    plausible frame from noise.
    """

    #: A report is never smaller than its fixed head, and none this driver reads is bigger than a page.
    MIN = 7 + 28
    MAX = 4096

    def __init__(self) -> None:
        self._buffer = bytearray()

    def feed(self, data: bytes) -> list[bytes]:
        self._buffer += data
        frames = []
        while len(self._buffer) >= 4:
            (size,) = struct.unpack(">I", self._buffer[:4])
            if not self.MIN <= size <= self.MAX:
                raise ProtocolError(
                    f"a report frame announces {size} bytes. That is not a frame this controller "
                    "sends, and there is no marker to find the next one by"
                )
            if len(self._buffer) < size:
                break
            frames.append(bytes(self._buffer[:size]))
            del self._buffer[:size]
        return frames


#: Offsets in every report, from `xarm/x3/report.py`: the head 30001, 30002 and 30003 share.
_STATE_MODE = 4
_CMD_NUM = 5
_ANGLES = 7  # seven float32, little-endian
_HEAD_END = _ANGLES + 28

#: Offsets in the rich report (30002) — `_Report30002DataStructure`.
_ENABLE = 88
_ERROR = 89
_WARN = 90
_ROBOT_TYPE = 145
_ROBOT_AXIS = 146
_VERSION = 151  # 30 bytes
_JOINT_MAX_SPEED = 217  # float32
_IS_SIMULATION = 314
_IS_REDUCED = 495
_JOINT_LIMITS = 516  # 7 × (min, max) float32
_RICH_END = _JOINT_LIMITS + 56


@dataclass(frozen=True)
class FastReport:
    state: int
    mode: int
    command_number: int
    angles: tuple[float, ...]


@dataclass(frozen=True)
class RichReport:
    state: int
    mode: int
    angles: tuple[float, ...]
    motors_enabled: int
    error_code: int
    warning_code: int
    robot_type: int
    axis: int
    version: str
    joint_max_speed: float
    is_simulation: bool
    is_reduced_mode: bool
    joint_limits: tuple[tuple[float, float], ...]


def parse_fast(frame: bytes) -> FastReport:
    if len(frame) < _HEAD_END:
        raise ProtocolError(f"a report of {len(frame)} bytes has no joint angles in it")
    state_mode = frame[_STATE_MODE]
    return FastReport(
        state=state_mode & 0x0F,
        mode=state_mode >> 4,
        command_number=struct.unpack(">H", frame[_CMD_NUM : _CMD_NUM + 2])[0],
        angles=struct.unpack("<7f", frame[_ANGLES:_HEAD_END]),
    )


def parse_rich(frame: bytes) -> RichReport:
    if len(frame) < _RICH_END:
        raise ProtocolError(
            f"a rich report of {len(frame)} bytes ends before the joint ranges do ({_RICH_END}). "
            "This firmware is older than the fields this driver refuses to guess."
        )
    head = parse_fast(frame)
    limits = struct.unpack("<14f", frame[_JOINT_LIMITS:_RICH_END])
    return RichReport(
        state=head.state,
        mode=head.mode,
        angles=head.angles,
        motors_enabled=frame[_ENABLE],
        error_code=frame[_ERROR],
        warning_code=frame[_WARN],
        robot_type=frame[_ROBOT_TYPE],
        axis=frame[_ROBOT_AXIS],
        version=frame[_VERSION : _VERSION + 30].split(b"\0", 1)[0].decode("ascii", "replace"),
        joint_max_speed=struct.unpack("<f", frame[_JOINT_MAX_SPEED : _JOINT_MAX_SPEED + 4])[0],
        is_simulation=frame[_IS_SIMULATION] == 1,
        is_reduced_mode=frame[_IS_REDUCED] == 1,
        joint_limits=tuple((limits[2 * i], limits[2 * i + 1]) for i in range(7)),
    )
