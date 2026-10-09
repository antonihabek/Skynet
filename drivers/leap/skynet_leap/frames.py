"""The wire of a LEAP Hand: Dynamixel protocol 2.0, the X-series control table as the hand's servos keep it, and the unit conversions the hand's makers use (DRY-COVERAGE-PLAN.md item 12.E).

A LEAP Hand (CMU, v1) is sixteen ROBOTIS Dynamixel servos on one bus: the *Lite* has sixteen XL330-M288-T and the *Full* sixteen XC330-M288-T. The hand has no controller of its own; the host speaks protocol 2.0 to the
servos through a USB serial adapter at 4 Mbaud. So this module is two things, and says which:

* **The protocol and the servos** are ROBOTIS's: the packet format (header `FF FF FD 00`, the length, a CRC-16 of polynomial 0x8005, and byte stuffing of `FF FF FD`), the instructions, the status packet's error byte,
  and the control table. The control table's addresses and widths are the e-manual's (https://emanual.robotis.com/docs/en/dxl/x/xc330-m288/) and are checked in `layout_check.py` against the table in LeRobot's
  `motors/dynamixel/tables.py` (Apache-2.0, which cites the same manual) and against ROBOTIS's own `dynamixel_sdk`, which builds the packets the way the maker's client does.
* **The hand** is the LEAP Hand authors': which servo is which joint (ids 0 to 15, the index finger first, each finger from its side-to-side MCP to its fingertip), the joint ranges (`LEAPsim_limits`), and the
  offset of 3.14159 rad between the servo's angle and the hand's (`LEAPsim_to_LEAPhand`), all from `leap_hand_utils.py` in https://github.com/leap-hand/LEAP_Hand_API. **That repository's LICENSE is Creative Commons
  Attribution-NonCommercial 4.0 (Kenny Shaw, Deepak Pathak), and its code is therefore not vendored, fetched or run by anything that runs by default**: the numbers above are physical facts about the hand (a joint's
  range, a zero), they are cited to the file they came from, and `layout_check.py` compares them to that file only when a person points `LEAP_API` at a checkout. See README.md.

Pure: no I/O, no clock, nothing imported but the standard library.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

__all__ = [
    "ProtocolError", "JOINTS", "IDS", "HEADER", "BROADCAST", "INST_PING", "INST_READ", "INST_WRITE", "INST_REBOOT", "INST_SYNC_READ", "INST_SYNC_WRITE", "INST_FAST_SYNC_READ", "INST_STATUS", "ALERT",
    "ADDR_MODEL_NUMBER", "ADDR_OPERATING_MODE", "ADDR_TORQUE_ENABLE", "ADDR_HARDWARE_ERROR", "ADDR_POSITION_D", "ADDR_POSITION_I", "ADDR_POSITION_P", "ADDR_GOAL_CURRENT", "ADDR_GOAL_POSITION",
    "ADDR_MOVING", "ADDR_PRESENT_CURRENT", "ADDR_PRESENT_VELOCITY", "ADDR_PRESENT_POSITION", "ADDR_PRESENT_VOLTAGE", "ADDR_PRESENT_TEMPERATURE", "ADDR_CURRENT_LIMIT", "ADDR_SHUTDOWN",
    "SPAN_STATE", "SPAN_GAINS", "SPAN_MOTION", "Variant", "VARIANTS", "HW_ERROR_BITS", "KNOWN_HW_ERROR_MASK", "ERROR_NUMBERS", "crc16", "stuff", "unstuff", "packet", "ping", "read", "write", "sync_read",
    "sync_write", "reboot", "StatusPacket", "find_status", "parse_status", "u16", "u32", "i16", "i32", "pack_u16", "pack_u32", "position_to_register", "register_to_position", "Reading", "joint_fault",
    "describe_hw_error", "LIMITS_MIN", "LIMITS_MAX", "OFFSET_RAD", "RAD_PER_TICK", "VELOCITY_RAD_S_PER_UNIT", "CURRENT_MA_PER_UNIT",
]

#: The maker's numbering (`main.py`): "Index (0-3), Middle (4-7), Ring (8-11) to Thumb (12-15) and from MCP Side, MCP Forward, PIP, DIP for each finger".
JOINTS = tuple(f"{finger}_{joint}" for finger in ("index", "middle", "ring", "thumb") for joint in ("mcp_side", "mcp_forward", "pip", "dip"))
IDS = tuple(range(16))

# ── protocol 2.0 ─────────────────────────────────────────────────────────────────────────────────

HEADER = b"\xff\xff\xfd\x00"
BROADCAST = 0xFE
INST_PING, INST_READ, INST_WRITE, INST_REBOOT = 0x01, 0x02, 0x03, 0x08
INST_SYNC_READ, INST_SYNC_WRITE, INST_FAST_SYNC_READ = 0x82, 0x83, 0x8A
INST_STATUS = 0x55

#: The status packet's error byte: bits 0 to 6 are the error number, bit 7 is the alert (a hardware error is set; read Hardware_Error_Status).
ERROR_NUMBERS = {1: "result fail", 2: "instruction error", 3: "CRC error", 4: "data range error", 5: "data length error", 6: "data limit error", 7: "access error"}
ALERT = 0x80

_CRC_TABLE: list[int] = []
for _i in range(256):
    _crc = _i << 8
    for _ in range(8):
        _crc = ((_crc << 1) ^ 0x8005) if _crc & 0x8000 else (_crc << 1)
    _CRC_TABLE.append(_crc & 0xFFFF)


class ProtocolError(Exception):
    """A packet that is not one protocol 2.0 allows, or a value the registers cannot carry."""


def crc16(data: bytes) -> int:
    """CRC-16, polynomial 0x8005, initial value 0, no reflection (the e-manual's `update_crc`)."""
    crc = 0
    for byte in data:
        crc = ((crc << 8) ^ _CRC_TABLE[((crc >> 8) ^ byte) & 0xFF]) & 0xFFFF
    return crc


def stuff(params: bytes) -> bytes:
    """A `FF FF FD` inside the parameters is followed by one more `FD`, so that it cannot be read as the start of a packet."""
    out = bytearray()
    for byte in params:
        out.append(byte)
        if len(out) >= 3 and out[-3:] == b"\xff\xff\xfd":
            out.append(0xFD)
    return bytes(out)


def unstuff(params: bytes) -> bytes:
    out = bytearray()
    skip = False
    for byte in params:
        if skip:
            skip = False
            continue
        out.append(byte)
        if len(out) >= 3 and out[-3:] == b"\xff\xff\xfd":
            skip = True
    return bytes(out)


def packet(servo: int, instruction: int, params: bytes = b"") -> bytes:
    body = stuff(params)
    length = len(body) + 3                              # the instruction, the parameters and the CRC
    head = HEADER + bytes([servo, length & 0xFF, length >> 8, instruction]) + body
    return head + (crc16(head)).to_bytes(2, "little")


def ping(servo: int) -> bytes:
    return packet(servo, INST_PING)


def read(servo: int, address: int, length: int) -> bytes:
    return packet(servo, INST_READ, address.to_bytes(2, "little") + length.to_bytes(2, "little"))


def write(servo: int, address: int, data: bytes) -> bytes:
    return packet(servo, INST_WRITE, address.to_bytes(2, "little") + data)


def sync_read(address: int, length: int, servos: tuple[int, ...] | list[int]) -> bytes:
    return packet(BROADCAST, INST_SYNC_READ, address.to_bytes(2, "little") + length.to_bytes(2, "little") + bytes(servos))


def fast_sync_read(address: int, length: int, servos: tuple[int, ...] | list[int]) -> bytes:
    """The same request with instruction 0x8A: the servos answer in one packet, each servo's block following the last's with no gap (ROBOTIS's Fast Sync Read). This driver does not send it; the simulated hand answers it so
    that ROBOTIS's own `dynamixel_sdk`, whose `fastSyncRead` the hand's authors' client prefers, can read the hand."""
    return packet(BROADCAST, INST_FAST_SYNC_READ, address.to_bytes(2, "little") + length.to_bytes(2, "little") + bytes(servos))


def sync_write(address: int, length: int, entries: dict[int, bytes]) -> bytes:
    """One data block per servo: `entries` maps a servo id to its `length` bytes."""
    body = bytearray(address.to_bytes(2, "little") + length.to_bytes(2, "little"))
    for servo, data in entries.items():
        if len(data) != length:
            raise ProtocolError(f"servo {servo} was given {len(data)} bytes where {length} are written.")
        body += bytes([servo]) + data
    return packet(BROADCAST, INST_SYNC_WRITE, bytes(body))


def reboot(servo: int) -> bytes:
    return packet(servo, INST_REBOOT)


@dataclass(frozen=True)
class StatusPacket:
    servo: int
    error: int                  # the status packet's error byte (the alert flag included)
    params: bytes               # unstuffed

    @property
    def alert(self) -> bool:
        return bool(self.error & ALERT)

    @property
    def error_number(self) -> int:
        return self.error & 0x7F


def find_status(buffer: bytes) -> int | None:
    """Where in `buffer` a status packet's header starts, or None. A header is `FF FF FD 00`; a stuffed `FF FF FD FD` is not one."""
    start = buffer.find(HEADER)
    return None if start == -1 else start


#: The most a servo id can be (ROBOTIS's SDK: 252), and the most a packet may claim to be.
MAX_ID = 0xFC
MAX_PACKET = 1024


def parse_status(buffer: bytes) -> tuple[StatusPacket, int] | None:
    """The first complete status packet in `buffer` and how many bytes it ends at, or None if more is wanted. Raises ProtocolError for a packet whose CRC is wrong.

    Something that begins with a header and is not a status packet is not one: an id above 252, a length that no packet has, or an instruction that is not 0x55 (the request itself, heard back by a half-duplex
    adapter, or noise that happens to start `FF FF FD 00`) is skipped a byte at a time, as ROBOTIS's SDK skips it, and the search goes on."""
    position = 0
    while True:
        start = buffer.find(HEADER, position)
        if start == -1 or len(buffer) - start < 8:
            return None
        servo = buffer[start + 4]
        length = buffer[start + 5] | buffer[start + 6] << 8
        if servo > MAX_ID or length < 4 or length > MAX_PACKET or buffer[start + 7] != INST_STATUS:
            position = start + 1
            continue
        end = start + 7 + length
        if len(buffer) < end:
            return None
        frame = buffer[start:end]
        if crc16(frame[:-2]) != int.from_bytes(frame[-2:], "little"):
            raise ProtocolError(f"a status packet from servo {servo} has a wrong CRC.")
        return StatusPacket(servo, frame[8], unstuff(frame[9:-2])), end


# ── the control table (XC330-M288-T and XL330-M288-T; e-manual, and LeRobot's `X_SERIES_CONTROL_TABLE`) ─────────────

ADDR_MODEL_NUMBER = 0                   # 2 bytes
ADDR_OPERATING_MODE = 11                # 1 byte, EEPROM: 5 is current-based position control
ADDR_CURRENT_LIMIT = 38                 # 2 bytes, EEPROM, 1 mA
ADDR_SHUTDOWN = 63                      # 1 byte, EEPROM: the hardware errors that switch torque off
ADDR_TORQUE_ENABLE = 64                 # 1 byte
ADDR_HARDWARE_ERROR = 70                # 1 byte
ADDR_POSITION_D = 80                    # 2 bytes
ADDR_POSITION_I = 82                    # 2 bytes
ADDR_POSITION_P = 84                    # 2 bytes
ADDR_GOAL_CURRENT = 102                 # 2 bytes, signed, 1 mA
ADDR_GOAL_POSITION = 116                # 4 bytes, signed, 4096 to a turn
ADDR_MOVING = 122                       # 1 byte
ADDR_PRESENT_CURRENT = 126              # 2 bytes, signed, 1 mA (the e-manual's unit for the XC330-M288)
ADDR_PRESENT_VELOCITY = 128             # 4 bytes, signed, 0.229 rev/min
ADDR_PRESENT_POSITION = 132             # 4 bytes, signed
ADDR_PRESENT_VOLTAGE = 144              # 2 bytes, 0.1 V
ADDR_PRESENT_TEMPERATURE = 146          # 1 byte, degrees Celsius

#: What one cycle reads, as three synchronised reads: torque and hardware error (64..70), the gains and the goal current (80..103), the goal, motion and present state (116..135).
SPAN_STATE = (ADDR_TORQUE_ENABLE, ADDR_HARDWARE_ERROR - ADDR_TORQUE_ENABLE + 1)
SPAN_GAINS = (ADDR_POSITION_D, ADDR_GOAL_CURRENT + 2 - ADDR_POSITION_D)
SPAN_MOTION = (ADDR_GOAL_POSITION, ADDR_PRESENT_POSITION + 4 - ADDR_GOAL_POSITION)

OPERATING_MODE_CURRENT_POSITION = 5

RAD_PER_TICK = 2.0 * math.pi / 4096.0
#: One unit of Present_Velocity is 0.229 rev/min (e-manual).
VELOCITY_RAD_S_PER_UNIT = 0.229 * 2.0 * math.pi / 60.0
#: One unit of Present_Current is 1 mA on the XC330-M288 and XL330-M288 (e-manual). (The maker's client scales it by 1.34, which is another servo's figure: `layout_check.py` pins the disagreement.)
CURRENT_MA_PER_UNIT = 1.0

#: Hardware_Error_Status (e-manual, XC330-M288): bit 0 input voltage, bit 2 overheating, bit 4 electrical shock, bit 5 overload. The other bits are not named.
HW_ERROR_BITS = {0x01: "input voltage error", 0x04: "overheating error", 0x10: "electrical shock error", 0x20: "overload error"}
KNOWN_HW_ERROR_MASK = 0x01 | 0x04 | 0x10 | 0x20


@dataclass(frozen=True)
class Variant:
    name: str
    model_name: str
    model_number: int
    max_current_ma: int         # the most the maker's readme suggests a goal current be raised to for this build


#: The Lite has XL330-M288-T servos (model number 1200) and the Full XC330-M288-T (1240), from ROBOTIS's bundle listings; the maker's readme suggests a current limit of about 300 mA for the Lite and up to 550 for the Full.
VARIANTS = {
    "lite": Variant("lite", "XL330-M288-T", 1200, 350),
    "full": Variant("full", "XC330-M288-T", 1240, 550),
}

# ── the hand ─────────────────────────────────────────────────────────────────────────────────────

#: `leap_hand_utils.LEAPsim_limits("regular")`: the joint ranges in the hand's "sim" convention, where 0 is the fully extended, allegro-like zero; the servo's angle is that plus 3.14159 (`LEAPsim_to_LEAPhand`).
LIMITS_MIN = (-1.047, -0.314, -0.506, -0.366, -1.047, -0.314, -0.506, -0.366, -1.047, -0.314, -0.506, -0.366, -0.349, -0.47, -1.20, -1.34)
LIMITS_MAX = (1.047, 2.23, 1.885, 2.042, 1.047, 2.23, 1.885, 2.042, 1.047, 2.23, 1.885, 2.042, 2.094, 2.443, 1.90, 1.88)
OFFSET_RAD = 3.14159


def position_to_register(angle_rad: float) -> int:
    """The Goal_Position that stands for a joint at `angle_rad` (the hand's sim convention): the servo's angle in 4096ths of a turn, rounded to the nearest."""
    if not math.isfinite(angle_rad):
        raise ProtocolError(f"{angle_rad!r} is not an angle.")
    return int(round((angle_rad + OFFSET_RAD) / RAD_PER_TICK))


def register_to_position(register: int) -> float:
    return register * RAD_PER_TICK - OFFSET_RAD


def u16(data: bytes, at: int = 0) -> int:
    return int.from_bytes(data[at:at + 2], "little")


def u32(data: bytes, at: int = 0) -> int:
    return int.from_bytes(data[at:at + 4], "little")


def i16(data: bytes, at: int = 0) -> int:
    return int.from_bytes(data[at:at + 2], "little", signed=True)


def i32(data: bytes, at: int = 0) -> int:
    return int.from_bytes(data[at:at + 4], "little", signed=True)


def pack_u16(value: int) -> bytes:
    if not 0 <= value <= 0xFFFF:
        raise ProtocolError(f"{value} does not fit a sixteen-bit register.")
    return value.to_bytes(2, "little")


def pack_u32(value: int) -> bytes:
    """A signed or unsigned value into four bytes: the servo reads Goal_Position as signed, and the register is the same four bytes either way."""
    if not -(1 << 31) <= value < (1 << 32):
        raise ProtocolError(f"{value} does not fit a thirty-two-bit register.")
    return (value & 0xFFFFFFFF).to_bytes(4, "little")


@dataclass(frozen=True)
class Reading:
    """One cycle's worth of what the sixteen servos say, as their registers say it. Every field is a tuple of sixteen, in the order of `IDS`."""
    torque: tuple[int, ...]
    hw_error: tuple[int, ...]
    gain_d: tuple[int, ...]
    gain_i: tuple[int, ...]
    gain_p: tuple[int, ...]
    goal_current: tuple[int, ...]
    goal: tuple[int, ...]
    moving: tuple[int, ...]
    current: tuple[int, ...]
    velocity: tuple[int, ...]
    position: tuple[int, ...]


def describe_hw_error(bits: int) -> str:
    named = [name for bit, name in HW_ERROR_BITS.items() if bits & bit]
    unnamed = bits & ~KNOWN_HW_ERROR_MASK & 0xFF
    if unnamed:
        named.append(f"bits {unnamed:#04x}, which the e-manual does not name")
    return ", ".join(named) if named else "no error"


def joint_fault(hw_error: int) -> int | None:
    """None for a servo with no hardware error. Any bit set, named or not, is a fault: the servo has switched its torque off (the factory shutdown setting) or is about to."""
    return hw_error if hw_error & 0xFF else None
