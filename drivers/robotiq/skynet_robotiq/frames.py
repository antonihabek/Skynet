"""The Robotiq 2F wire: the command and status blocks, the Modbus RTU frames that carry them, and the maker's unit conversions. Pure, no I/O.

Every constant is read out of Robotiq's own C++ SDK (`github.com/Robotiq/grippers`, BSD-3-Clause, `sdk_cpp/`), which `layout_check.py` runs and holds these bytes to:

* the blocks' byte and bit layout: `include/Robotiq/detail/register_map.hpp` (the instruction manual's table, as the maker transcribed it);
* where they live on the wire: `include/Robotiq/detail/modbus_constants.hpp` (the command block at register 1000 and the status block at 2000, three registers each, packed most significant byte first);
* the fault codes and how serious each is: `include/Robotiq/gripper/fault_status.hpp`;
* the two profiles and the conversions between a register and SI units: `include/Robotiq/gripper/device_profile.hpp` and `src/units.cpp`;
* the slave address: `include/Robotiq/gripper/connection_config.hpp` (9).

The SDK's own exchange is one Modbus function 0x17 (read and write multiple registers) per cycle, writing the command block and reading the status block back; its `readStatus` is function 0x03 and its `writeCommand`
function 0x10. This driver speaks all three in the same bytes.
"""

from __future__ import annotations

import math
import struct
from dataclasses import dataclass

SLAVE_ADDRESS = 9                 # connection_config.hpp: kDefaultModbusSlaveAddress
COMMAND_ADDRESS = 0x03E8          # modbus_constants.hpp: 1000
STATUS_ADDRESS = 0x07D0           # modbus_constants.hpp: 2000
BLOCK_BYTES = 6                   # register_map.hpp: kCommandDocumentedBytes, kStatusDocumentedBytes
BLOCK_REGISTERS = BLOCK_BYTES // 2

FC_READ_HOLDING = 0x03
FC_WRITE_MULTIPLE = 0x10
FC_READ_WRITE = 0x17


class ProtocolError(Exception):
    """A frame that is not one the wire allows, or an answer that is not one a gripper gives."""


# ── the command block ────────────────────────────────────────────────────────────────────────────────────

ACT = 0x01        # rACT: activate (and stay on)
GTO = 0x08        # rGTO: go to the requested position (clear: stop)
ATR = 0x10        # rATR: emergency auto-release (opens or closes the fingers to let go; it is not a stop)
ARD = 0x20        # rARD: the auto-release direction, open when set
ACTION_RESERVED = 0xC6   # bits 7-6 and 2-1


@dataclass(frozen=True)
class Command:
    """The command block: ACTION REQUEST, POSITION REQUEST (rPR), SPEED (rSP) and FORCE (rFR). Bytes 1 and 2 are reserved and travel as zero."""

    action: int = 0
    position: int = 0
    speed: int = 0
    force: int = 0

    def block(self) -> bytes:
        for name, value in (("action", self.action), ("position", self.position), ("speed", self.speed), ("force", self.force)):
            if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 255:
                raise ProtocolError(f"{name} of {value!r} is not a byte.")
        if self.action & ACTION_RESERVED:
            raise ProtocolError(f"the action byte 0x{self.action:02X} sets a reserved bit (0x{self.action & ACTION_RESERVED:02X}): the manual reserves bits 7-6 and 2-1.")
        return bytes([self.action, 0, 0, self.position, self.speed, self.force])


def registers_from_block(block: bytes) -> list[int]:
    """Six bytes as three registers, the first byte of each pair the high one (modbus_constants.hpp)."""
    if len(block) != BLOCK_BYTES:
        raise ProtocolError(f"a block is {BLOCK_BYTES} bytes, not {len(block)}.")
    return [block[0] << 8 | block[1], block[2] << 8 | block[3], block[4] << 8 | block[5]]


def block_from_registers(registers: list[int]) -> bytes:
    if len(registers) != BLOCK_REGISTERS:
        raise ProtocolError(f"a block is {BLOCK_REGISTERS} registers, not {len(registers)}.")
    return bytes(b for r in registers for b in (r >> 8 & 0xFF, r & 0xFF))


# ── the status block ─────────────────────────────────────────────────────────────────────────────────────

STA_RESET, STA_IN_PROGRESS, STA_RESERVED, STA_COMPLETE = 0, 1, 2, 3                    # gSTA
OBJ_MOVING, OBJ_OPENING, OBJ_CLOSING, OBJ_AT_POSITION = 0, 1, 2, 3                    # gOBJ

STA_NAMES = {STA_RESET: "reset (not activated)", STA_IN_PROGRESS: "activation in progress", STA_RESERVED: "an unallocated pattern (not activated)", STA_COMPLETE: "activated"}
OBJ_NAMES = {OBJ_MOVING: "moving, or no motion requested", OBJ_OPENING: "an object detected while opening", OBJ_CLOSING: "an object detected while closing", OBJ_AT_POSITION: "at the requested position"}

#: fault_status.hpp: the gripper's own fault (gFLT, the low nibble of byte 2) and how serious the manual says it is. "Major" needs a rising edge on rACT to clear; an unknown code is Major.
GRIPPER_FAULTS = {
    0x00: ("no fault", "none"),
    0x05: ("action delayed: the activation (re-activation) must be completed before the action", "warning"),
    0x07: ("the activation bit must be set before the action", "warning"),
    0x08: ("maximum operating temperature exceeded (85 C internally): let it cool below 80 C", "minor"),
    0x09: ("no communication during at least one second", "minor"),
    0x0A: ("under the minimum operating voltage", "major"),
    0x0B: ("automatic release in progress", "major"),
    0x0C: ("internal fault: contact Robotiq", "major"),
    0x0D: ("activation fault: verify that no interference or other error occurred", "major"),
    0x0E: ("overcurrent triggered", "major"),
    0x0F: ("automatic release completed", "major"),
}
#: The optional Robotiq controller's fault (kFLT, the high nibble).
CONTROLLER_FAULTS = {
    0x00: ("no fault", "none"),
    0x04: ("24 V not detected (reconfiguration through USB is possible)", "warning"),
    0x05: ("no device detected", "warning"),
    0x09: ("communication is not ready", "minor"),
    0x0C: ("emergency stop triggered: a reset is required", "major"),
    0x0E: ("overcurrent protection triggered: a reset is required", "major"),
}
SEVERITY_ORDER = {"none": 0, "warning": 1, "minor": 2, "major": 3}


def severity(table: dict, code: int) -> str:
    return table.get(code, ("an unnamed code", "major"))[1]


@dataclass(frozen=True)
class Status:
    """The status block: GRIPPER STATUS, FAULT STATUS, POSITION REQUEST ECHO (gPR), POSITION (gPO) and CURRENT (gCU)."""

    raw: bytes

    @property
    def activated(self) -> bool:
        return bool(self.raw[0] & 0x01)

    @property
    def go_to(self) -> bool:
        return bool(self.raw[0] & 0x08)

    @property
    def activation_state(self) -> int:
        return (self.raw[0] & 0x30) >> 4

    @property
    def object_detection(self) -> int:
        return (self.raw[0] & 0xC0) >> 6

    @property
    def gripper_fault(self) -> int:
        return self.raw[2] & 0x0F

    @property
    def controller_fault(self) -> int:
        return (self.raw[2] & 0xF0) >> 4

    @property
    def position_echo(self) -> int:
        return self.raw[3]

    @property
    def position(self) -> int:
        return self.raw[4]

    @property
    def current(self) -> int:
        return self.raw[5]

    @property
    def fault_byte(self) -> int:
        return self.raw[2]

    def worst_severity(self) -> str:
        a, b = severity(GRIPPER_FAULTS, self.gripper_fault), severity(CONTROLLER_FAULTS, self.controller_fault)
        return a if SEVERITY_ORDER[a] >= SEVERITY_ORDER[b] else b


def parse_status(block: bytes) -> Status:
    if len(block) != BLOCK_BYTES:
        raise ProtocolError(f"a status block is {BLOCK_BYTES} bytes, not {len(block)}.")
    return Status(bytes(block))


# ── the maker's profiles and conversions (device_profile.hpp, units.cpp) ─────────────────────────────────────

@dataclass(frozen=True)
class Profile:
    """The measured profile of a gripper with its standard fingers. openPosition and closedPosition are what gPO reads after rPR 0 and rPR 255 with nothing between the fingers."""

    name: str
    min_speed: float      # m/s
    max_speed: float      # m/s
    min_opening: float    # m, the opening at closedPosition
    max_opening: float    # m, the opening at openPosition
    open_position: int
    closed_position: int

    @property
    def register_range(self) -> float:
        return float(self.closed_position - self.open_position)

    @property
    def opening_range(self) -> float:
        return self.max_opening - self.min_opening


PROFILES = {
    "2F-85": Profile("2F-85", 0.020, 0.150, 0.0, 0.085, 3, 230),
    "Hand-E": Profile("Hand-E", 0.020, 0.150, 0.0, 0.050, 3, 250),
}


def _round(x: float) -> int:
    """`std::lround`: halves away from zero."""
    return int(math.floor(x + 0.5)) if x >= 0 else -int(math.floor(-x + 0.5))


def opening_to_register(opening: float, profile: Profile) -> int | None:
    """rPR for an opening in metres: units.cpp `openingToRegister`. None where the maker's returns nothing."""
    span = profile.opening_range
    if not math.isfinite(opening) or not math.isfinite(span) or span <= 0.0 or profile.register_range <= 0.0:
        return None
    fraction = min(max((opening - profile.min_opening) / span, 0.0), 1.0)
    return _round(profile.open_position + (1.0 - fraction) * profile.register_range) & 0xFF


def opening_from_register(value: int, profile: Profile) -> float | None:
    """The opening in metres for a gPO: units.cpp `openingFromRegister`."""
    band, span = profile.register_range, profile.opening_range
    if band <= 0.0 or not math.isfinite(span) or span <= 0.0:
        return None
    closed = min(max((value - profile.open_position) / band, 0.0), 1.0)
    return profile.min_opening + (1.0 - closed) * span


def _span_to_register(value: float, minimum: float, maximum: float) -> int | None:
    if not math.isfinite(value) or value < 0.0 or not math.isfinite(minimum) or not math.isfinite(maximum) or maximum <= minimum:
        return None
    fraction = min(max((value - minimum) / (maximum - minimum), 0.0), 1.0)
    return _round(fraction * 0xFF)


def speed_to_register(speed: float, profile: Profile) -> int | None:
    return _span_to_register(speed, profile.min_speed, profile.max_speed)


def effort_to_register(effort: float) -> int | None:
    return _span_to_register(effort, 0.0, 1.0)


# ── Modbus RTU ───────────────────────────────────────────────────────────────────────────────────────────

def crc16(data: bytes) -> int:
    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc


def with_crc(pdu: bytes) -> bytes:
    return pdu + struct.pack("<H", crc16(pdu))


def request_read(slave: int, address: int, count: int) -> bytes:
    return with_crc(struct.pack(">BBHH", slave, FC_READ_HOLDING, address, count))


def request_write(slave: int, address: int, registers: list[int]) -> bytes:
    return with_crc(struct.pack(">BBHHB", slave, FC_WRITE_MULTIPLE, address, len(registers), 2 * len(registers)) + struct.pack(f">{len(registers)}H", *registers))


def request_exchange(slave: int, command: bytes) -> bytes:
    """Function 0x17: write the command block, read the status block back, in one transaction (the SDK's `exchange`)."""
    registers = registers_from_block(command)
    return with_crc(struct.pack(">BBHHHHB", slave, FC_READ_WRITE, STATUS_ADDRESS, BLOCK_REGISTERS, COMMAND_ADDRESS, BLOCK_REGISTERS, 2 * BLOCK_REGISTERS)
                    + struct.pack(f">{BLOCK_REGISTERS}H", *registers))


def response_length(prefix: bytes) -> int | None:
    """How many bytes the answer is, from its first three, or None while they are not all here. An exception answer is five."""
    if len(prefix) < 3:
        return None
    if prefix[1] & 0x80:
        return 5
    if prefix[1] in (FC_READ_HOLDING, FC_READ_WRITE):
        return 3 + prefix[2] + 2
    if prefix[1] == FC_WRITE_MULTIPLE:
        return 8
    raise ProtocolError(f"function 0x{prefix[1]:02X} is not one this gripper's answers use.")


EXCEPTIONS = {1: "illegal function", 2: "illegal data address", 3: "illegal data value", 4: "server device failure"}


def parse_response(frame: bytes, slave: int, function: int) -> bytes:
    """The data of an answer to `function`, or a ProtocolError saying what was wrong with it: a bad CRC, another slave, an exception."""
    if len(frame) < 5:
        raise ProtocolError(f"the answer is {len(frame)} bytes, which is shorter than any Modbus RTU frame.")
    if crc16(frame[:-2]) != struct.unpack("<H", frame[-2:])[0]:
        raise ProtocolError(f"the answer's CRC is wrong ({frame.hex()}).")
    if frame[0] != slave:
        raise ProtocolError(f"the answer is from slave {frame[0]} and the request was to {slave}.")
    if frame[1] == function | 0x80:
        code = frame[2]
        raise ProtocolError(f"the gripper answered function 0x{function:02X} with exception {code} ({EXCEPTIONS.get(code, 'unknown')}).")
    if frame[1] != function:
        raise ProtocolError(f"the answer is to function 0x{frame[1]:02X} and the request was 0x{function:02X}.")
    if function in (FC_READ_HOLDING, FC_READ_WRITE):
        if frame[2] != len(frame) - 5:
            raise ProtocolError(f"the answer says {frame[2]} data bytes and carries {len(frame) - 5}.")
        return bytes(frame[3:-2])
    if len(frame) != 8:
        raise ProtocolError(f"the answer to a write is {len(frame)} bytes, not 8.")
    return bytes(frame[2:6])
