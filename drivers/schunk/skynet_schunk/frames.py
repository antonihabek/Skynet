"""The wire of a SCHUNK EGU, EGK or EZU gripper's HTTP interface, read out of the maker's own client and simulator (DRY-COVERAGE-PLAN.md item 12.E.9).

SCHUNK publish a Python client (`schunk_gripper_library`, GPL-3.0) and a "minimalist protocol simulator" (`schunk_gripper_dummy`, GPL-3.0) in one repository, `SCHUNK-SE-Co-KG/schunk_mechatronic_gripper`. Nothing of
either is copied here: this module is the protocol they both speak, written from reading them and from the two diagrams of the data frames in the simulator's `doc/`, with each constant's source beside it, and
`layout_check.py` runs the maker's own client and compares what it sends with what this driver sends.

# The wire

The gripper's module (an HMS Anybus board) serves two routes over HTTP:

* ``GET /adi/data.json?inst=0x0040&count=1`` answers a JSON list whose first element is the instance's bytes as upper-case hex;
* ``POST /adi/update.json`` with the form fields ``inst`` and ``value`` (hex) writes one, and answers ``{"result": 0}``.

Two instances carry everything that moves: **the input frame** ``0x0040`` (the gripper's status) and **the output frame** ``0x0048`` (what the controller wants), each 16 bytes:

=====  ===========================  ==========================
bytes  input (``0x0040``)           output (``0x0048``)
=====  ===========================  ==========================
0-3    status double word           control double word
4-7    actual position, µm (int32)  target position, µm (int32)
8-11   reserved                     set speed, µm/s (int32)
12-15  error, warning, additional   gripping force, % (int32)
=====  ===========================  ==========================

Everything else (the limits, the module type, the fieldbus) is a parameter read the same way. Integers are little-endian on EtherNet/IP and EtherCAT and **big-endian on PROFINET** (the maker's client reverses the
four bytes when its fieldbus is ``PN``); the double words are bit-addressed (bit *n* is bit ``n % 8`` of byte ``n // 8``) and are not swapped.

# A command

A command is a handshake, as the maker's client does it (``Driver._send_cmd``): write the control word with every bit clear except bit 0 (which is *low* for a fast stop, so it is high when nothing is asked), read the
input frame to see the *command received toggle* (status bit 5), write the command, and wait for the toggle to flip. A gripper that cannot do what was asked sets *not feasible* (bit 3) instead.
"""

from __future__ import annotations

import math
import struct
from dataclasses import dataclass

__all__ = [
    "Bit", "Status", "Route", "Instance", "FIELDBUS", "MODULE_TYPES", "PARAMETERS", "ProtocolError", "clear_output", "command_output", "parse_input", "decode_parameter",
    "bits_set", "um_to_m", "m_to_um", "ERROR_FAST_STOP",
]

ERROR_FAST_STOP = 0xD9
"""The error byte of a gripper whose fast stop is active: what the maker's simulator reports at power-up and after `fast_stop()`, and what its client's tests expect after one."""


class ProtocolError(ValueError):
    """Something the gripper said that does not fit the wire, or something this driver was asked to say that does not."""


class Route:
    DATA = "/adi/data.json"
    UPDATE = "/adi/update.json"


class Instance:
    INPUT = "0x0040"
    OUTPUT = "0x0048"


class Bit:
    """The control double word's bits (the maker's diagram `plc_control_double_word.png` and `Driver._send_cmd`'s callers)."""

    FAST_STOP = 0            # LOW triggers it: "fast stop triggers on low signal" (`Driver.fast_stop`)
    STOP = 1
    ACKNOWLEDGE = 2
    PREPARE_SHUTDOWN = 3
    SOFT_RESET = 4
    RELEASE_MANUAL = 5
    REPEAT_TOGGLE = 6
    GRIP_DIRECTION = 7       # outward: grip from the inside
    JOG_NEGATIVE = 8
    JOG_POSITIVE = 9
    RELEASE_WORKPIECE = 11
    GRIP_WORKPIECE = 12
    MOVE_ABSOLUTE = 13
    MOVE_RELATIVE = 14
    GRIP_AT_POSITION = 16
    BRAKE_TEST = 30
    GRIP_FORCE_MAINTENANCE = 31  # "GPE"


class Status:
    """The status double word's bits (`plc_status_double_word.png` and the patterns `Driver` waits for)."""

    READY = 0                # ready for operation
    CONTROL_AUTHORITY = 1    # control authority fieldbus
    READY_SHUTDOWN = 2
    NOT_FEASIBLE = 3
    SUCCESS = 4              # command successfully processed
    TOGGLE = 5               # command received toggle
    WARNING = 6
    ERROR = 7
    RELEASED_MANUAL = 8
    SOFT_LIMIT = 9
    NO_WORKPIECE = 11
    GRIPPED = 12
    POSITION_REACHED = 13
    PRE_GRIP = 14
    WORKPIECE_LOST = 16
    WRONG_WORKPIECE = 17
    GRIP_FORCE_MAINTENANCE = 31

    NAMES = {
        0: "ready for operation", 1: "control authority fieldbus", 2: "ready for shutdown", 3: "not feasible", 4: "command successfully processed", 5: "command received toggle", 6: "warning",
        7: "error", 8: "released for manual movement", 9: "software limit reached", 11: "no workpiece detected", 12: "workpiece gripped", 13: "position reached", 14: "workpiece pre-grip started",
        16: "workpiece lost", 17: "wrong workpiece gripped", 31: "grip force and position maintenance activated",
    }
    #: The bits the maker's client lists as valid (`Driver.valid_status_bits`); the others are reserved.
    VALID = tuple(list(range(0, 10)) + [11, 12, 13, 14, 16, 17, 31])
    #: The control bits it lists as valid (`Driver.valid_control_bits`).
    VALID_CONTROL = tuple(list(range(0, 10)) + [11, 12, 13, 14, 16, 30, 31])


#: The fieldbus enumeration (`config/fieldbus_types.json`). `MB` is Modbus RTU, which is not this interface.
FIELDBUS = {1: "PN", 2: "EI", 3: "EC", 7: "MB"}
HTTP_FIELDBUSES = ("PN", "EI", "EC")

#: The module types (`config/module_types.json`): `<family>_<size>_<N or M>_<B or SD>`; `M` is the variant with grip force and position maintenance.
MODULE_TYPES = {
    6: "EGU_50_N_B", 7: "EGU_60_N_B", 8: "EGU_70_N_B", 9: "EGU_80_N_B", 10: "EGU_50_M_B", 11: "EGU_60_M_B", 12: "EGU_70_M_B", 13: "EGU_80_M_B", 14: "EGK_25_N_B", 15: "EGK_40_N_B",
    16: "EGK_50_N_B", 17: "EGK_25_M_B", 18: "EGK_40_M_B", 19: "EGK_50_M_B", 20: "EGU_50_N_SD", 21: "EGU_60_N_SD", 22: "EGU_70_N_SD", 23: "EGU_80_N_SD", 24: "EGU_50_M_SD", 25: "EGU_60_M_SD",
    26: "EGU_70_M_SD", 27: "EGU_80_M_SD", 28: "EZU_30_N_B", 29: "EZU_35_N_B", 30: "EZU_40_N_B", 31: "EZU_30_M_B", 32: "EZU_35_M_B", 33: "EZU_40_M_B", 34: "EZU_30_N_SD", 35: "EZU_35_N_SD",
    36: "EZU_40_N_SD", 37: "EZU_30_M_SD", 38: "EZU_35_M_SD", 39: "EZU_40_M_SD",
}


@dataclass(frozen=True)
class Parameter:
    name: str
    registers: int
    kind: str      # "enum", "float" or "char[N]"
    unit: str = ""


#: The parameters this driver reads (`config/readable_parameters.json`, a subset): its own limits, and what the module is. A float is in millimetres, mm/s or percent; a register is two bytes.
PARAMETERS = {
    "0x0500": Parameter("module_type", 1, "enum"),
    "0x0540": Parameter("wp_release_delta", 2, "float", "mm"),
    "0x0588": Parameter("max_phys_stroke", 2, "float", "mm"),
    "0x0600": Parameter("min_pos", 2, "float", "mm"),
    "0x0608": Parameter("max_pos", 2, "float", "mm"),
    "0x0628": Parameter("min_vel", 2, "float", "mm/s"),
    "0x0630": Parameter("max_vel", 2, "float", "mm/s"),
    "0x0650": Parameter("max_grp_vel", 2, "float", "mm/s"),
    "0x0660": Parameter("max_grp_force", 2, "float", "%"),
    "0x1000": Parameter("serial_no_txt", 8, "char[16]"),
    "0x1118": Parameter("sw_version_txt", 11, "char[22]"),
    "0x1130": Parameter("fieldbus_type", 1, "enum"),
}


def um_to_m(value: int) -> float:
    return value / 1e6


def m_to_um(value: float) -> int:
    """Micrometres, rounded to the nearest: the gripper's unit is the micrometre and an int32 is what it takes."""
    return int(round(value * 1e6))


def bits_set(word: int, valid: tuple[int, ...] = Status.VALID) -> list[int]:
    return [bit for bit in valid if word >> bit & 1]


def _int32(value: int, fieldbus: str) -> bytes:
    try:
        raw = struct.pack("<i", value)
    except struct.error as error:
        raise ProtocolError(f"{value} does not fit the gripper's 32-bit signed integer field.") from error
    return raw[::-1] if fieldbus == "PN" else raw


def _word(bits: dict[int, bool]) -> int:
    word = 0
    for bit, value in bits.items():
        if bit < 0 or bit > 31 or bit not in Status.VALID_CONTROL:
            raise ProtocolError(f"control bit {bit} is reserved or does not exist.")
        if value:
            word |= 1 << bit
    return word


def clear_output() -> bytes:
    """The output frame that asks for nothing: every control bit clear but bit 0, which is low for a fast stop and so is high when none is asked (`Driver.clear_plc_output`)."""
    return bytes.fromhex("01" + "00" * 15)


def command_output(bits: dict[int, bool], fieldbus: str, position: int | None = None, speed: int | None = None, force: int | None = None) -> bytes:
    """The output frame of a command: the control bits (bit 0 high unless `bits` says otherwise: `Driver._send_cmd` starts from the cleared frame) and the integers the command carries."""
    merged = {Bit.FAST_STOP: True, **bits}
    frame = bytearray(struct.pack("<I", _word(merged)))
    frame += _int32(position or 0, fieldbus) + _int32(speed or 0, fieldbus) + _int32(force or 0, fieldbus)
    return bytes(frame)


@dataclass(frozen=True)
class Input:
    """The input frame, decoded."""

    word: int
    position_um: int
    error: int
    warning: int
    additional: int
    raw: bytes

    def bit(self, bit: int) -> int:
        return self.word >> bit & 1

    @property
    def toggle(self) -> int:
        return self.bit(Status.TOGGLE)


def parse_input(hex_text: object, fieldbus: str) -> Input:
    """The input frame from what the route answered: sixteen bytes of hex, or a refusal naming what was wrong."""
    if not isinstance(hex_text, str) or len(hex_text) != 32:
        raise ProtocolError(f"the input frame is not 32 hex characters (sixteen bytes): {hex_text!r}.")
    try:
        raw = bytes.fromhex(hex_text)
    except ValueError as error:
        raise ProtocolError(f"the input frame is not hex: {hex_text!r}.") from error
    word = struct.unpack("<I", raw[0:4])[0]
    position = raw[4:8][::-1] if fieldbus == "PN" else raw[4:8]
    return Input(word=word, position_um=struct.unpack("<i", position)[0], error=raw[12], warning=raw[14], additional=raw[15], raw=raw)


def decode_parameter(param: str, raw: bytes, fieldbus: str) -> object:
    """A parameter's value as the maker's client decodes it (`Driver.update_module_parameters`): an enum is an int16, a float is in millimetres (and is reversed on PROFINET), text is ASCII."""
    spec = PARAMETERS.get(param)
    if spec is None:
        raise ProtocolError(f"{param} is not a parameter this driver reads.")
    size = spec.registers * 2
    data = raw + bytes(max(0, size - len(raw)))
    if spec.kind == "enum":
        return struct.unpack("<h", data[:2])[0]
    if spec.kind == "float":
        chunk = data[:4][::-1] if fieldbus == "PN" else data[:4]
        value = struct.unpack("<f", chunk)[0]
        if not math.isfinite(value):
            raise ProtocolError(f"{spec.name} ({param}) is {value}.")
        return value
    if spec.kind.startswith("char["):
        length = int(spec.kind[5:-1])
        return data[:length].decode("ascii", errors="replace").strip("\x00 ")
    raise ProtocolError(f"{param} has a type, {spec.kind}, this driver does not decode.")
