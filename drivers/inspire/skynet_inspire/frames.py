"""The wire of an Inspire Robots RH56-series dexterous hand: the RS-485 private protocol, its register map, the maker's tables and the unit conversions (DRY-COVERAGE-PLAN.md item 12.E.8).

Every constant here is read out of the maker's own manuals, and says which:

* **RH56 series user manual**, PRJ-02-TS-U-001 V1.0.9 (January 2024), the DF and BF hands: §2.2 the frames, §2.4 the registers.
* **RH56DFTP user manual**, PRJ-02-TS-U-010 V1.0.0 (December 2024): the same frames (§2.2) and register map (§2.6), a different range for three joints, a different speed, force ranges three times the size, and a signed force.

The two manuals describe the same private protocol and the same addresses, and disagree on the numbers that make the registers mean something, which is why a hand's model is the operator's claim (`PROFILES`) and nothing the hand
says identifies it. The manuals also describe Modbus RTU and Modbus TCP on the same register map, and this module does not speak either (see `bridge.py`, which refuses them by name, and README.md for the reason).

Pure: no I/O, no clock, nothing imported but the standard library.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

__all__ = [
    "ProtocolError", "JOINTS", "HEADER_REQUEST", "HEADER_RESPONSE", "CMD_READ", "CMD_WRITE", "ADDR_CLEAR_ERROR", "ADDR_FORCE_CALIBRATION", "ADDR_ANGLE_SET", "ADDR_FORCE_SET", "ADDR_SPEED_SET", "ADDR_ANGLE_ACT",
    "ADDR_FORCE_ACT", "ADDR_CURRENT", "ADDR_ERROR", "ADDR_STATUS", "ADDR_TEMP", "BLOCK_SHORTS", "BLOCK_BYTES", "COMMAND_BLOCK_ADDRESS", "COMMAND_BLOCK_BYTES", "Profile", "PROFILES", "checksum", "request_read",
    "request_write", "request_write_shorts", "response_length", "find_response", "parse_response", "shorts", "pack_shorts", "STATUS_NAMES", "ERROR_BITS", "CLEARABLE_ERROR_BITS", "classify_status", "classify_error",
    "joint_fault", "describe_joint_fault", "register_to_angle", "angle_to_register", "Reading",
]

#: §5.1's names for the six actuators, in the order of every register group (series manual §2.4: "ANGLE_SET(0) the little finger" to "(5) thumb rotation").
JOINTS = ("pinky", "ring", "middle", "index", "thumb_bend", "thumb_rotation")

HEADER_REQUEST = b"\xEB\x90"      # series manual §2.2.1: byte[0] 0xEB, byte[1] 0x90
HEADER_RESPONSE = b"\x90\xEB"     # the same, reversed, for the answer
CMD_READ = 0x11                   # "Register reading flag"
CMD_WRITE = 0x12                  # "Write Register instruction flag"

# Register addresses (series manual §2.4 table; DFTP manual §2.6 table). A group of six `short` is twelve bytes and an address is a *byte* address: ANGLE_SET is 1486-1497 and FORCE_SET starts at 1498.
ADDR_CLEAR_ERROR = 1004           # 1 byte; "After 1 is written, clearable errors ... will be cleared"
ADDR_FORCE_CALIBRATION = 1009     # 1 byte; GESTURE_FORCE_CLB, the force sensor calibration
ADDR_ANGLE_SET = 1486             # 6 short, -1 or 0-1000
ADDR_FORCE_SET = 1498             # 6 short, grams
ADDR_SPEED_SET = 1522             # 6 short, 0-1000
ADDR_ANGLE_ACT = 1546             # 6 short, 0-1000
ADDR_FORCE_ACT = 1582             # 6 short, grams
ADDR_CURRENT = 1594               # 6 short, mA
ADDR_ERROR = 1606                 # 6 byte
ADDR_STATUS = 1612                # 6 byte (ERROR and STATUS are adjacent: one twelve-byte read returns both)
ADDR_TEMP = 1618                  # 6 byte, degrees Celsius

BLOCK_SHORTS = 12                 # six `short`
BLOCK_BYTES = 6                   # six `byte`
#: The three command registers are contiguous from ANGLE_SET to SPEED_SET with twelve bytes between FORCE_SET and SPEED_SET that neither manual names: one read of this span returns all three groups, and the bytes between are read and never interpreted.
COMMAND_BLOCK_ADDRESS = ADDR_ANGLE_SET
COMMAND_BLOCK_BYTES = (ADDR_SPEED_SET + BLOCK_SHORTS) - ADDR_ANGLE_SET

MAX_WRITE_BYTES = 252             # the length byte counts the command, the address and the data: 3 + 252 is the most a byte holds
MAX_READ_BYTES = 255              # the length of a read is one byte

ANGLE_REGISTER_MIN, ANGLE_REGISTER_MAX = 0, 1000     # series manual §2.4.11: "-1, 0-1000"; -1 is "no response" and this driver never sends it (bridge.py: *Never -1*)
SPEED_REGISTER_MAX = 1000


class ProtocolError(Exception):
    """A frame that is not a frame of this protocol: the wrong length, header, checksum, hand, command or address, or a write the hand refused."""


# ── the frames ────────────────────────────────────────────────────────────────

def checksum(body: bytes) -> int:
    """The low eight bits of the sum of every byte after the header (series manual §2.2.1: "the low-order bytes of the sum of all data before checksum except the response frame header")."""
    return sum(body) & 0xFF


def _hand_id(hand: int) -> int:
    if isinstance(hand, bool) or not isinstance(hand, int) or not 1 <= hand <= 254:
        raise ProtocolError(f"hand id {hand!r} is not 1 to 254 (series manual §2.4.1: \"default value = 1; range: 1-254\").")
    return hand


def _address(address: int) -> int:
    if isinstance(address, bool) or not isinstance(address, int) or not 0 <= address <= 0xFFFF:
        raise ProtocolError(f"register address {address!r} is not 0 to 65535.")
    return address


def request_read(hand: int, address: int, length: int) -> bytes:
    """`EB 90 id 04 11 addr_l addr_h length checksum`: nine bytes, whatever the length asked for (series manual §2.2.1)."""
    _hand_id(hand)
    _address(address)
    if isinstance(length, bool) or not isinstance(length, int) or not 1 <= length <= MAX_READ_BYTES:
        raise ProtocolError(f"a read of {length!r} bytes is not 1 to {MAX_READ_BYTES}: the length is one byte on the wire.")
    body = bytes([hand, 0x04, CMD_READ, address & 0xFF, address >> 8, length])
    return HEADER_REQUEST + body + bytes([checksum(body)])


def request_write(hand: int, address: int, data: bytes) -> bytes:
    """`EB 90 id (len+3) 12 addr_l addr_h data... checksum` (series manual §2.2.2)."""
    _hand_id(hand)
    _address(address)
    if not isinstance(data, (bytes, bytearray)) or not 1 <= len(data) <= MAX_WRITE_BYTES:
        raise ProtocolError(f"a write of {len(data) if isinstance(data, (bytes, bytearray)) else data!r} bytes is not 1 to {MAX_WRITE_BYTES}: the frame's length byte holds the command, the address and the data.")
    body = bytes([hand, len(data) + 3, CMD_WRITE, address & 0xFF, address >> 8]) + bytes(data)
    return HEADER_REQUEST + body + bytes([checksum(body)])


def pack_shorts(values: list[int] | tuple[int, ...]) -> bytes:
    """Sixteen-bit values, little-endian, two's complement: the low byte first (series manual §2.2: 100 is 0x6400 on the wire, 1000 is 0xE803). -1 is 0xFFFF."""
    out = bytearray()
    for value in values:
        if isinstance(value, bool) or not isinstance(value, int) or not -0x8000 <= value <= 0x7FFF:
            raise ProtocolError(f"{value!r} does not fit a signed sixteen-bit register.")
        out += (value & 0xFFFF).to_bytes(2, "little")
    return bytes(out)


def request_write_shorts(hand: int, address: int, values: list[int] | tuple[int, ...]) -> bytes:
    return request_write(hand, address, pack_shorts(values))


def shorts(data: bytes, *, signed: bool = False) -> list[int]:
    """The inverse of `pack_shorts`. An odd length is a frame that is not a run of registers."""
    if len(data) % 2:
        raise ProtocolError(f"{len(data)} bytes are not a whole number of sixteen-bit registers.")
    return [int.from_bytes(data[i:i + 2], "little", signed=signed) for i in range(0, len(data), 2)]


# ── the answers ───────────────────────────────────────────────────────────────

def response_length(buffer: bytes) -> int | None:
    """How long the answer that starts at `buffer[0]` is, once its length byte has arrived; None until it has. `buffer` must start at a header (see `find_response`).

    The length byte counts the command, the address and the data, so a frame is that plus the two header bytes, the hand id, the length byte and the checksum: five more. A read of n bytes is n + 8, and a write
    acknowledgement (length 4) is nine bytes."""
    if len(buffer) < 4:
        return None
    return buffer[3] + 5


def find_response(buffer: bytes) -> int | None:
    """The offset of the first `90 EB` in `buffer`, or None. What comes before it is stale input or the echo of the request that a half-duplex RS-485 adapter hears (whose header is the other way round): the line
    skips it, as the maker's own readers do."""
    index = buffer.find(HEADER_RESPONSE)
    return None if index < 0 else index


def parse_response(frame: bytes, hand: int, command: int, address: int, length: int = 0) -> bytes:
    """Check one answer completely and return its data (a read) or nothing (a write acknowledgement). Header, length, checksum, hand, command, the echoed address, and for a write the result byte, which is 1 for
    success: a hand that answers a write with anything else has refused it."""
    _hand_id(hand)
    _address(address)
    if command not in (CMD_READ, CMD_WRITE):
        raise ProtocolError(f"command 0x{command:02X} is neither a read (0x11) nor a write (0x12).")
    expected = length + 8 if command == CMD_READ else 9
    if len(frame) != expected:
        raise ProtocolError(f"the answer is {len(frame)} bytes and a {'read of ' + str(length) + ' bytes' if command == CMD_READ else 'write acknowledgement'} is {expected}.")
    if frame[:2] != HEADER_RESPONSE:
        raise ProtocolError(f"the answer starts {frame[0]:02X} {frame[1]:02X}, and an answer starts 90 EB.")
    if frame[3] + 5 != len(frame):
        raise ProtocolError(f"the answer's length byte is {frame[3]} and the frame is {len(frame)} bytes (it should say {len(frame) - 5}).")
    if checksum(frame[2:-1]) != frame[-1]:
        raise ProtocolError(f"the answer's checksum is 0x{frame[-1]:02X} and its bytes sum to 0x{checksum(frame[2:-1]):02X}.")
    if frame[2] != hand:
        raise ProtocolError(f"the answer is from hand {frame[2]} and the request was for hand {hand}.")
    if frame[4] != command:
        raise ProtocolError(f"the answer is for command 0x{frame[4]:02X} and the request was 0x{command:02X}.")
    echoed = frame[5] | frame[6] << 8
    if echoed != address:
        raise ProtocolError(f"the answer is for register {echoed} and the request was for {address}.")
    if command == CMD_WRITE:
        if frame[7] != 1:
            raise ProtocolError(f"the hand refused the write to register {address}: its result byte is {frame[7]} and success is 1.")
        return b""
    return bytes(frame[7:-1])


# ── what a model is ───────────────────────────────────────────────────────────

@dataclass(frozen=True)
class Profile:
    """What the registers mean on one model. Everything here is one manual's table; a hand of another build is `custom`, with a profile somebody measured."""
    name: str
    source: str
    #: (degrees at register 0, degrees at register 1000) of each joint, in `JOINTS` order.
    angle_deg: tuple[tuple[float, float], ...]
    #: Seconds a joint takes to cross its whole range at speed register 1000, with no load.
    full_range_s: float
    #: The largest FORCE_SET the manual lists, in grams.
    force_set_max_g: int


#: The orientation is the one the series manual states for the fingers (register 1000 is "fully open", §2.4.12; the actuator's 0 is "the maximum finger angle (i.e., holding fingers open)", §2.4.10) applied to every
#: joint. For the thumb the manuals give the two ends of the range and not which end 1000 is: README.md says so, and `status` reports the mapping.
PROFILES: dict[str, Profile] = {
    "RH56": Profile("RH56", "RH56 series user manual PRJ-02-TS-U-001 V1.0.9, §2.4.11 and §2.4.13",
                    ((19.0, 176.7), (19.0, 176.7), (19.0, 176.7), (19.0, 176.7), (-13.0, 53.6), (90.0, 165.0)), 0.8, 1000),
    "RH56DFTP": Profile("RH56DFTP", "RH56DFTP user manual PRJ-02-TS-U-010 V1.0.0, §2.6.11 and §2.6.13",
                        ((20.0, 176.0), (20.0, 176.0), (20.0, 176.0), (20.0, 176.0), (-13.0, 70.0), (90.0, 165.0)), 0.6, 3000),
}


def register_to_angle(register: int, joint: int, profile: Profile) -> float:
    """Radians, linearly from the model's two ends of this joint's range. The manuals call the register range dimensionless and give the physical range at its two ends: that the scale between them is linear is an
    assumption, and README.md lists it."""
    low, high = profile.angle_deg[joint]
    return math.radians(low + (high - low) * register / ANGLE_REGISTER_MAX)


def angle_to_register(angle: float, joint: int, profile: Profile) -> int | None:
    """The register nearest an angle in radians, or None when the angle is outside the joint's range (by more than half a step) or is not a number."""
    if not math.isfinite(angle):
        return None
    low, high = profile.angle_deg[joint]
    register = round((math.degrees(angle) - low) / (high - low) * ANGLE_REGISTER_MAX)
    if not ANGLE_REGISTER_MIN <= register <= ANGLE_REGISTER_MAX:
        return None
    return int(register)


# ── the maker's tables ────────────────────────────────────────────────────────

#: Series manual §2.4.19 and DFTP manual: the status byte of each joint. 4 and anything above 7 are not in either table.
STATUS_NAMES = {
    0: "unclenching",
    1: "grasping",
    2: "stopped after reaching the target position",
    3: "stopped after reaching the force control value",
    5: "stopped by the current protection",
    6: "stopped by a locked rotor",
    7: "stopped by an actuator fault",
}

#: Series manual §2.4.18: the error byte's bits.
ERROR_BITS = {
    0: "locked-rotor error",
    1: "over-temperature error",
    2: "over-current error",
    3: "abnormal operation of the motor",
    4: "communication error",
}
#: §2.4.3: writing 1 to CLEAR_ERROR clears the locked-rotor, over-current, abnormal-operation and communication errors; "the over temperature error of the actuator is not clearable. When the temperature falls, such error
#: will be cleared automatically."
CLEARABLE_ERROR_BITS = 0b11101
_DEFINED_ERROR_BITS = 0b11111


def classify_status(code: int) -> str:
    """`ok` for the four codes that are a finger doing its job, `fault` for the three the maker says are a stop for cause, and `fault` for a code the maker does not name: a driver that cannot tell what a finger is
    doing does not command it (§3.6 of the agent standard, fail closed)."""
    if code in (0, 1, 2, 3):
        return "ok"
    return "fault"


def classify_error(byte: int) -> str:
    """Any error bit is a fault; so is a bit the maker does not define, for the same reason."""
    return "ok" if byte == 0 else "fault"


def joint_fault(errors: list[int] | tuple[int, ...], statuses: list[int] | tuple[int, ...]) -> tuple[int, int, int] | None:
    """The first joint (in `JOINTS` order) that holds a fault, as (joint, status, error), or None."""
    for joint, (error, status) in enumerate(zip(errors, statuses)):
        if classify_error(error) == "fault" or classify_status(status) == "fault":
            return joint, status, error
    return None


def describe_joint_fault(joint: int, status: int, error: int) -> str:
    parts = []
    for bit in range(8):
        if error & 1 << bit:
            parts.append(ERROR_BITS.get(bit, f"an error bit ({bit}) the maker does not define"))
    if classify_status(status) == "fault":
        parts.append(STATUS_NAMES.get(status, f"a status code ({status}) the maker does not name"))
    return f"{JOINTS[joint]}: {', '.join(parts)} (status {status}, error 0x{error:02X})"


@dataclass(frozen=True)
class Reading:
    """One cycle's worth of what the hand says, as the registers say it."""
    angle: tuple[int, ...]              # ANGLE_ACT
    target: tuple[int, ...]             # ANGLE_SET as read back
    force_limit: tuple[int, ...]        # FORCE_SET as read back
    speed: tuple[int, ...]              # SPEED_SET as read back
    error: tuple[int, ...]              # ERROR
    status: tuple[int, ...]             # STATUS
