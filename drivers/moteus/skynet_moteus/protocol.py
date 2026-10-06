"""mjbots moteus's register protocol (the "multiplex" format), as arithmetic over bytes: no bus, no clock, no state.

A moteus speaks one thing on CAN-FD: **subframes**. A frame is a run of them — *write these registers*, *read those registers*, the reply to a read, an
error, a tunnelled text stream, a no-op — and which register means what is the table in `docs/protocol/registers.md`. The format is the maker's
(`mjlib`'s `multiplex/format.h`, documented in `docs/protocol/can.md`); everything here is held to the maker's own Python client by `layout_check.py`
(`moteus/multiplex.py`, `moteus/protocol.py`, `moteus/moteus.py`), and to the firmware's own enumerations (`fw/bldc_servo_structs.h`, `fw/error.h`).

Three things about the wire shape the rest of this driver:

* **Values are quantities in a resolution.** The same register is an int8, an int16, an int32 or a float, each with its own scale per quantity
  (a position is 0.01, 0.0001 or 0.00001 of a revolution), and the integer's most negative value is the wire's NaN. This driver writes everything it
  commands as a float and asks for floats and int8s back, the maker's own client's defaults, so every conversion here is exercised by a real
  counterpart; the integer mappings exist for what a reply may carry and for the layout check.
* **A frame's id is a conversation.** The high byte is the sender — its top bit meaning *reply to this* — and the low byte the controller's id, so a
  command to controller 1 from source 0 asking for an answer is `0x8001` and the answer is `0x0100`.
* **Nothing here is ever "no answer, so zero".** A reply that is short, or that carries an error subframe, or that names a register nobody asked for,
  raises `ValueError`; it is the bridge's job to count it and the caller's to refuse it.
"""

from __future__ import annotations

import math
import struct
from dataclasses import dataclass

# ── resolutions, and the quantities they scale ──────────────────────────────────────────────────────────────────────────

INT8, INT16, INT32, F32 = 0, 1, 2, 3

_STRUCT = (struct.Struct("<b"), struct.Struct("<h"), struct.Struct("<i"), struct.Struct("<f"))
_SIZE = (1, 2, 4, 4)
#: The integer most negative value is the wire's NaN, at each width (docs/protocol/registers.md).
_NAN_INT = (-128, -32768, -2147483648)
_MAX_INT = (127, 32767, 2147483647)

#: A quantity's LSB at each integer width (docs/protocol/registers.md, *Mappings*). A float is the quantity itself, in the unit named.
POSITION = (0.01, 0.0001, 0.00001)  #: revolutions
VELOCITY = (0.1, 0.00025, 0.00001)  #: revolutions per second
ACCEL = (0.05, 0.001, 0.00001)  #: revolutions per second squared
TORQUE = (0.5, 0.01, 0.001)  #: newton metres
VOLTAGE = (0.5, 0.1, 0.001)  #: volts
TEMPERATURE = (1.0, 0.1, 0.001)  #: degrees Celsius
TIME = (0.01, 0.001, 0.000001)  #: seconds
CURRENT = (1.0, 0.1, 0.001)  #: amperes
POWER = (10.0, 0.05, 0.0001)  #: watts
PWM = (1.0 / 127.0, 1.0 / 32767.0, 1.0 / 2147483647.0)  #: unitless


def resolution_size(resolution: int) -> int:
    return _SIZE[resolution]


def encode(value: float, resolution: int, scales: tuple[float, float, float]) -> float | int:
    """A quantity as the raw number its register carries at ``resolution``: scaled and saturated for an integer, itself for a float, and the
    wire's NaN for a value that is not a number or not finite."""
    if resolution == F32:
        return float(value)
    if not math.isfinite(value):
        return _NAN_INT[resolution]
    scaled = value / scales[resolution]
    limit = float(_MAX_INT[resolution])
    if scaled > limit:
        return _MAX_INT[resolution]
    if scaled < -limit:
        return -_MAX_INT[resolution]
    return int(scaled)


def decode(raw: float | int, resolution: int, scales: tuple[float, float, float]) -> float:
    """A raw register value as the quantity, with the integer NaN as NaN."""
    if resolution == F32:
        return float(raw)
    if raw == _NAN_INT[resolution]:
        return math.nan
    return float(raw) * scales[resolution]


def pack_value(raw: float | int, resolution: int) -> bytes:
    return _STRUCT[resolution].pack(raw)


# ── varuints ────────────────────────────────────────────────────────────────────────────────────────────────────────────

def varuint(value: int) -> bytes:
    """The wire's unsigned integer: seven bits at a time, least significant first, the top bit of each byte saying *more follow*. At most five bytes,
    which is a uint32."""
    if not 0 <= value <= 0xFFFFFFFF:
        raise ValueError(f"{value} is not a uint32, which is all a varuint holds")
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        out.append(byte | (0x80 if value else 0))
        if not value:
            return bytes(out)


def read_varuint(data: bytes, offset: int) -> tuple[int, int]:
    """(value, next offset). ``ValueError`` for a varuint that runs off the end of the frame or past five bytes."""
    result = 0
    shift = 0
    for index in range(5):
        if offset + index >= len(data):
            raise ValueError("a varuint runs off the end of the frame")
        byte = data[offset + index]
        result |= (byte & 0x7F) << shift
        shift += 7
        if not byte & 0x80:
            if result > 0xFFFFFFFF:
                raise ValueError("a varuint does not fit a uint32")
            return result, offset + index + 1
    raise ValueError("a varuint of more than five bytes")


# ── the frame's identifier ──────────────────────────────────────────────────────────────────────────────────────────────

#: The sender's top bit: *reply to this*. Without it a controller acts on the frame and says nothing, even to a query in it.
REPLY_WANTED = 0x80


def can_id(source: int, destination: int, reply_wanted: bool) -> int:
    """The identifier of a frame from ``source`` to controller ``destination`` (docs/protocol/can.md, *CAN ID*)."""
    if not 0 <= source <= 0x7F:
        raise ValueError(f"source {source} is not a 7-bit id")
    if not 1 <= destination <= 0x7F:
        raise ValueError(f"controller id {destination} is not 1 to 127 (0 is the broadcast address and no controller has it)")
    return ((source | (REPLY_WANTED if reply_wanted else 0)) << 8) | destination


def split_id(ident: int) -> tuple[int, int, bool]:
    """(source, destination, reply wanted) of an identifier."""
    if not 0 <= ident <= 0xFFFF:
        raise ValueError(f"{ident:#x} is not a 16-bit conversation id")
    high = ident >> 8
    return high & 0x7F, ident & 0x7F, bool(high & REPLY_WANTED)


#: Valid CAN-FD payload lengths: a frame's data is padded up to the next of these with no-ops (docs/protocol/can.md, *Subframes*).
FD_LENGTHS = (0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64)


def pad(data: bytes) -> bytes:
    """``data`` padded with NOP subframes to a length a CAN-FD frame can carry. ``ValueError`` past 64 bytes: it is not split, because two frames are
    two commands to the controller and the second is not the first's tail."""
    if len(data) > 64:
        raise ValueError(f"{len(data)} bytes do not fit one CAN-FD frame (64)")
    for length in FD_LENGTHS:
        if length >= len(data):
            return data + bytes([NOP]) * (length - len(data))
    raise AssertionError("unreachable")


# ── subframes ───────────────────────────────────────────────────────────────────────────────────────────────────────────

WRITE_BASE = 0x00
READ_BASE = 0x10
REPLY_BASE = 0x20
WRITE_ERROR = 0x30
READ_ERROR = 0x31
STREAM_CLIENT_DATA = 0x40
STREAM_SERVER_DATA = 0x41
STREAM_CLIENT_POLL = 0x42
NOP = 0x50


@dataclass(frozen=True)
class Register:
    """One register of a write, a read or a reply. ``value`` is None for a read (nothing is carried)."""

    kind: str  # "write" | "read" | "reply"
    register: int
    resolution: int
    value: float | int | None


@dataclass(frozen=True)
class RegisterError:
    kind: str  # "write" | "read"
    register: int
    code: int


@dataclass(frozen=True)
class StreamData:
    """Bytes tunnelled for the diagnostic channel (docs/protocol/diagnostic.md)."""

    kind: str  # "client" (a write to the controller) | "server" (its answer) | "poll"
    channel: int
    data: bytes  # for a poll, one byte: the most the client will take


def subframe_header(base: int, resolution: int, count: int) -> bytes:
    """The bytes that open a run of ``count`` registers of one resolution: a 2-bit count folded into the command byte when it is 1 to 3, a separate
    count otherwise (docs/protocol/can.md)."""
    if count < 1:
        raise ValueError("a run of registers has at least one")
    command = base | (resolution << 2)
    if count <= 3:
        return bytes([command | count])
    return bytes([command]) + varuint(count)


def write_registers(start: int, resolution: int, values: list[float | int]) -> bytes:
    """A write subframe: ``values`` are raw (already scaled), at consecutive registers from ``start``."""
    return subframe_header(WRITE_BASE, resolution, len(values)) + varuint(start) + b"".join(pack_value(v, resolution) for v in values)


def read_registers(start: int, resolution: int, count: int) -> bytes:
    return subframe_header(READ_BASE, resolution, count) + varuint(start)


def reply_registers(start: int, resolution: int, values: list[float | int]) -> bytes:
    return subframe_header(REPLY_BASE, resolution, len(values)) + varuint(start) + b"".join(pack_value(v, resolution) for v in values)


def error_subframe(kind: str, register: int, code: int) -> bytes:
    return bytes([WRITE_ERROR if kind == "write" else READ_ERROR]) + varuint(register) + varuint(code)


def stream_client_data(channel: int, data: bytes) -> bytes:
    return bytes([STREAM_CLIENT_DATA]) + varuint(channel) + varuint(len(data)) + data


def stream_poll(channel: int, max_length: int) -> bytes:
    return bytes([STREAM_CLIENT_POLL]) + varuint(channel) + varuint(max_length)


def stream_server_data(channel: int, data: bytes) -> bytes:
    return bytes([STREAM_SERVER_DATA]) + varuint(channel) + varuint(len(data)) + data


def parse_frame(data: bytes) -> list[Register | RegisterError | StreamData]:
    """Every subframe of a frame, in order. ``ValueError`` on anything this driver cannot account for: a truncated subframe, a command byte that is none
    of the format's, a register count of zero. (The maker's own parser stops quietly at an unknown byte; a driver that read half a reply and called
    it a reply would be reporting a body it did not hear.)"""
    out: list[Register | RegisterError | StreamData] = []
    offset = 0
    while offset < len(data):
        command = data[offset]
        offset += 1
        upper = command & 0xF0
        resolution = (command & 0x0C) >> 2
        count = command & 0x03
        if upper in (WRITE_BASE, READ_BASE, REPLY_BASE) and command not in (WRITE_ERROR, READ_ERROR):
            if count == 0:
                count, offset = read_varuint(data, offset)
            if count == 0:
                raise ValueError("a subframe of zero registers")
            first, offset = read_varuint(data, offset)
            kind = {WRITE_BASE: "write", READ_BASE: "read", REPLY_BASE: "reply"}[upper]
            for index in range(count):
                if kind == "read":
                    out.append(Register(kind, first + index, resolution, None))
                    continue
                size = _SIZE[resolution]
                if offset + size > len(data):
                    raise ValueError(f"a {kind} subframe is cut off at register {first + index:#x}")
                (raw,) = _STRUCT[resolution].unpack_from(data, offset)
                offset += size
                out.append(Register(kind, first + index, resolution, raw))
        elif command in (WRITE_ERROR, READ_ERROR):
            register, offset = read_varuint(data, offset)
            code, offset = read_varuint(data, offset)
            out.append(RegisterError("write" if command == WRITE_ERROR else "read", register, code))
        elif command in (STREAM_CLIENT_DATA, STREAM_SERVER_DATA):
            channel, offset = read_varuint(data, offset)
            size, offset = read_varuint(data, offset)
            if offset + size > len(data):
                raise ValueError("a stream subframe is cut off")
            out.append(StreamData("client" if command == STREAM_CLIENT_DATA else "server", channel, bytes(data[offset:offset + size])))
            offset += size
        elif command == STREAM_CLIENT_POLL:
            channel, offset = read_varuint(data, offset)
            longest, offset = read_varuint(data, offset)
            out.append(StreamData("poll", channel, bytes([min(longest, 255)])))
        elif command == NOP:
            continue
        else:
            raise ValueError(f"{command:#04x} is not a subframe this format defines")
    return out


# ── registers ───────────────────────────────────────────────────────────────────────────────────────────────────────────

MODE = 0x000
POSITION_REG = 0x001
VELOCITY_REG = 0x002
TORQUE_REG = 0x003
Q_CURRENT = 0x004
D_CURRENT = 0x005
ABS_POSITION = 0x006
POWER_REG = 0x007
MOTOR_TEMPERATURE = 0x00A
TRAJECTORY_COMPLETE = 0x00B
HOME_STATE = 0x00C
VOLTAGE_REG = 0x00D
TEMPERATURE_REG = 0x00E
FAULT = 0x00F

COMMAND_POSITION = 0x020
COMMAND_VELOCITY = 0x021
COMMAND_FEEDFORWARD_TORQUE = 0x022
COMMAND_KP_SCALE = 0x023
COMMAND_KD_SCALE = 0x024
COMMAND_MAX_TORQUE = 0x025
COMMAND_STOP_POSITION = 0x026
COMMAND_WATCHDOG = 0x027
COMMAND_VELOCITY_LIMIT = 0x028
COMMAND_ACCEL_LIMIT = 0x029

MODEL_NUMBER = 0x100
FIRMWARE_VERSION = 0x101
REGISTER_MAP_VERSION = 0x102

# ── modes (docs/protocol/registers.md; the firmware's `BldcServoMode`) ─────────────────────────────────────────────────

MODE_STOPPED = 0
MODE_FAULT = 1
MODE_ENABLING = 2
MODE_CALIBRATING = 3
MODE_CALIBRATION_COMPLETE = 4
MODE_PWM = 5
MODE_VOLTAGE = 6
MODE_VOLTAGE_FOC = 7
MODE_VOLTAGE_DQ = 8
MODE_CURRENT = 9
MODE_POSITION = 10
MODE_TIMEOUT = 11
MODE_ZERO_VELOCITY = 12
MODE_STAY_WITHIN = 13
MODE_MEASURE_INDUCTANCE = 14
MODE_BRAKE = 15

MODES = {
    MODE_STOPPED: "stopped",
    MODE_FAULT: "fault",
    MODE_ENABLING: "enabling",
    MODE_CALIBRATING: "calibrating",
    MODE_CALIBRATION_COMPLETE: "calibration complete",
    MODE_PWM: "pwm",
    MODE_VOLTAGE: "voltage",
    MODE_VOLTAGE_FOC: "voltage foc",
    MODE_VOLTAGE_DQ: "voltage dq",
    MODE_CURRENT: "current",
    MODE_POSITION: "position",
    MODE_TIMEOUT: "timeout",
    MODE_ZERO_VELOCITY: "zero velocity",
    MODE_STAY_WITHIN: "stay within",
    MODE_MEASURE_INDUCTANCE: "measure inductance",
    MODE_BRAKE: "brake",
}

#: Modes in which the controller is turning a command into torque (or has just been told to hold at nothing); a state the driver reads and does not command.
PREPARING_MODES = (MODE_ENABLING, MODE_CALIBRATING, MODE_CALIBRATION_COMPLETE)

# ── fault codes (docs/protocol/registers.md, *0x00f*; the firmware's `moteus::errc`) ───────────────────────────────────

FAULTS = {
    32: "calibration fault",
    33: "motor driver fault",
    34: "over voltage",
    35: "encoder fault",
    36: "motor not configured",
    37: "pwm cycle overrun",
    38: "over temperature",
    39: "outside limit",
    40: "under voltage",
    41: "config changed",
    42: "theta invalid",
    43: "position invalid",
    44: "driver enable fault",
    45: "stop position deprecated",
    46: "timing violation",
    47: "bemf feedforward no accel",
    48: "invalid limits",
    49: "position control error",
    50: "velocity control error",
}

#: Non-zero codes that are *not* faults: in a valid control mode they say which function is limiting the output. `docs/protocol/registers.md` lists 96 to 103;
#: the firmware (`fw/error.h`, `errc`) defines two more, 104 and 105, which the register reference does not mention: the maker's own sources disagree (F285),
#: and this table is the firmware's, because it is the firmware that sends them.
LIMITING = {
    96: "servo.max_velocity",
    97: "servo.max_power_W",
    98: "the maximum system voltage",
    99: "servo.max_current_A",
    100: "servo.fault_temperature",
    101: "servo.motor_fault_temperature",
    102: "the commanded maximum torque",
    103: "servopos.position_min or servopos.position_max",
    104: "flux braking is active",
    105: "field weakening is active",
}


def fault_name(code: int) -> str:
    return FAULTS.get(code) or LIMITING.get(code) or f"code {code}"


# ── what a controller says, and what this driver asks and tells ────────────────────────────────────────────────────────

#: The registers this driver reads every cycle, in the order it reads them: mode (int8), position, velocity, torque (float), then voltage, temperature
#: and fault (int8) — the maker's own client's defaults, so every reply is one the maker's parser also reads.
def query() -> bytes:
    return (
        read_registers(MODE, INT8, 1)
        + read_registers(POSITION_REG, F32, 3)
        + read_registers(HOME_STATE, INT8, 4)  # home state, voltage, temperature, fault: one run, as the maker's own client groups them
    )


@dataclass(frozen=True)
class Status:
    """What one controller said in answer to `query()`: the quantities in SI-ish units (revolutions of the output shaft, per second, newton metres)."""

    mode: int
    position: float
    velocity: float
    torque: float
    home_state: int
    voltage: float
    temperature: float
    fault: int


def parse_status(data: bytes) -> Status:
    """The controller's answer to `query()`. ``ValueError`` if anything the query asked for is missing, in the wrong resolution, or carries an error
    subframe, or if the frame carries a register nobody asked for: a body whose status is half a status has not said it is well."""
    got: dict[int, tuple[int, float | int]] = {}
    for item in parse_frame(data):
        if isinstance(item, RegisterError):
            raise ValueError(f"the controller answered register {item.register:#x} with an error ({item.code})")
        if not isinstance(item, Register) or item.kind != "reply":
            raise ValueError("a status frame carries something other than register replies")
        got[item.register] = (item.resolution, item.value)  # type: ignore[assignment]
    expected = {MODE: INT8, POSITION_REG: F32, VELOCITY_REG: F32, TORQUE_REG: F32, HOME_STATE: INT8, VOLTAGE_REG: INT8, TEMPERATURE_REG: INT8, FAULT: INT8}
    extra = sorted(set(got) - set(expected))
    if extra:
        raise ValueError(f"the reply carries register {extra[0]:#x}, which was not asked for")
    for register, resolution in expected.items():
        if register not in got:
            raise ValueError(f"the reply does not carry register {register:#x}")
        if got[register][0] != resolution:
            raise ValueError(f"register {register:#x} came back in resolution {got[register][0]}, not the {resolution} it was asked in")
    return Status(
        mode=int(got[MODE][1]),
        position=decode(got[POSITION_REG][1], F32, POSITION),
        velocity=decode(got[VELOCITY_REG][1], F32, VELOCITY),
        torque=decode(got[TORQUE_REG][1], F32, TORQUE),
        home_state=int(got[HOME_STATE][1]),
        voltage=decode(got[VOLTAGE_REG][1], INT8, VOLTAGE),
        temperature=decode(got[TEMPERATURE_REG][1], INT8, TEMPERATURE),
        fault=int(got[FAULT][1]),
    )


def stop_command(mode: int) -> bytes:
    """One write of the mode register: stopped, zero velocity or brake — the three a stop may be (docs/protocol/registers.md, *0x000*)."""
    if mode not in (MODE_STOPPED, MODE_ZERO_VELOCITY, MODE_BRAKE):
        raise ValueError(f"mode {mode} is not one of the three stops")
    return write_registers(MODE, INT8, [mode])


def position_command(
    position: float,
    velocity: float,
    feedforward_torque: float,
    kp_scale: float,
    kd_scale: float,
    maximum_torque: float,
    watchdog_s: float | None,
) -> bytes:
    """Position mode with the six fields the control law takes, all as floats, and optionally the watchdog: the mode write first (docs/protocol/registers.md:
    *this register must be written before any other registers in the command frame*), then registers 0x020 to 0x025 as one run, then 0x027.

    ``position`` NaN means *hold where you are*; ``velocity`` NaN is read as 0.0; ``maximum_torque`` NaN is the controller's own configured ceiling."""
    out = write_registers(MODE, INT8, [MODE_POSITION])
    out += write_registers(
        COMMAND_POSITION,
        F32,
        [
            encode(position, F32, POSITION),
            encode(velocity, F32, VELOCITY),
            encode(feedforward_torque, F32, TORQUE),
            encode(kp_scale, F32, PWM),
            encode(kd_scale, F32, PWM),
            encode(maximum_torque, F32, TORQUE),
        ],
    )
    if watchdog_s is not None:
        out += write_registers(COMMAND_WATCHDOG, F32, [encode(watchdog_s, F32, TIME)])
    return out


def diagnostic_write(channel: int, text: bytes) -> bytes:
    return stream_client_data(channel, text)


def diagnostic_poll(channel: int, max_length: int = 48) -> bytes:
    return stream_poll(channel, max_length)


def parse_diagnostic_reply(data: bytes, channel: int) -> bytes:
    """The text a controller's diagnostic channel returned in one frame (empty when it had nothing)."""
    out = b""
    for item in parse_frame(data):
        if isinstance(item, StreamData) and item.kind == "server" and item.channel == channel:
            out += item.data
    return out
