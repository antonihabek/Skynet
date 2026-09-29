"""MAVLink on the wire: framing, the CRC, and the eighteen messages this driver speaks.

Pure and stdlib-only. Every constant that a vehicle's firmware can disagree with carries its source
beside it, and `layout_check.py` opens the reference implementation (`pymavlink`) and requires each of
them to still be what is written here: message ids, CRC seeds, field order and widths, the enums, and
the bytes a frame comes to. The reference is a check-time dependency and never this driver's.

# The frame

MAVLink 2 (`0xFD`) is what this driver sends. It reads MAVLink 1 (`0xFE`) too, because an autopilot
answers in the version it last heard and a link that has heard nothing yet speaks 1.

    v2:  FD  len  incompat  compat  seq  sysid  compid  msgid(3, LE)  payload(len)  crc(2, LE)  [signature(13)]
    v1:  FE  len  seq  sysid  compid  msgid(1)  payload(len)  crc(2, LE)

The CRC is X.25 (CRC-16/MCRF4XX, seed `0xFFFF`) over everything after the start byte, then over one more
byte, the message's `CRC_EXTRA`: a hash of the field names and types, so a sender and a receiver that
disagree about a message's layout disagree about its checksum instead of about its meaning.

A v2 payload has its trailing zero bytes cut (never below one byte) and a receiver pads them back. Fields
are sent widest first, and a message's *extension* fields follow the sorted ones, unsorted, and are left out
of v1 and of the CRC.

# What this driver does not speak

**Signed links.** A v2 frame with incompat flag bit 0 carries a 13-byte signature. This driver holds no
signing key, so it can neither sign a command nor verify a reply; the parser reports such a frame as
signed instead of trusting it, and the bridge refuses the link.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, field
from functools import cached_property

__all__ = [
    "STX_V1",
    "STX_V2",
    "INCOMPAT_SIGNED",
    "Spec",
    "SPECS",
    "BY_ID",
    "Packet",
    "Parser",
    "ProtocolError",
    "crc_x25",
    "encode",
    "Cmd",
    "Result",
    "MavType",
    "MavState",
    "ModeFlag",
    "Landed",
    "Autopilot",
    "Sensor",
    "Frame",
    "COPTER_MODES",
    "PLANE_MODES",
    "ROVER_MODES",
    "SUB_MODES",
    "FAMILY_MODES",
]

STX_V1 = 0xFE
STX_V2 = 0xFD
#: Incompat flag: the frame ends with a 13-byte signature.
INCOMPAT_SIGNED = 0x01
_SIGNATURE_LEN = 13


class ProtocolError(Exception):
    """A frame this module was asked to build and cannot: a value that does not fit its field."""


# ── The messages ────────────────────────────────────────────────────────────────────────────────────
#
# Fields are listed in **wire order** (widest first, stable within a width; extensions last), which is the
# order `pymavlink` reports as `ordered_fieldnames`. `base` counts the fields before the extensions.
# Source of every id and seed: MAVLink's `common.xml` as generated into `pymavlink` 2.4.49.


@dataclass(frozen=True)
class Spec:
    name: str
    msg_id: int
    crc_extra: int
    #: `(field name, struct code)`. A count prefix is an array (`4f`); `Ns` is a fixed-size byte string.
    fields: tuple[tuple[str, str], ...]
    #: How many of `fields` are not extensions.
    base: int

    @cached_property
    def fmt(self) -> struct.Struct:
        return struct.Struct("<" + "".join(code for _, code in self.fields))

    @cached_property
    def base_size(self) -> int:
        return struct.calcsize("<" + "".join(code for _, code in self.fields[: self.base]))

    @cached_property
    def size(self) -> int:
        return self.fmt.size


def _spec(name: str, msg_id: int, crc: int, base: int, *fields: tuple[str, str]) -> Spec:
    return Spec(name, msg_id, crc, tuple(fields), base)


SPECS: dict[str, Spec] = {
    spec.name: spec
    for spec in (
        _spec("HEARTBEAT", 0, 50, 6, ("custom_mode", "I"), ("type", "B"), ("autopilot", "B"), ("base_mode", "B"),
              ("system_status", "B"), ("mavlink_version", "B")),
        _spec("SYS_STATUS", 1, 124, 13, ("onboard_control_sensors_present", "I"),
              ("onboard_control_sensors_enabled", "I"), ("onboard_control_sensors_health", "I"), ("load", "H"),
              ("voltage_battery", "H"), ("current_battery", "h"), ("drop_rate_comm", "H"), ("errors_comm", "H"),
              ("errors_count1", "H"), ("errors_count2", "H"), ("errors_count3", "H"), ("errors_count4", "H"),
              ("battery_remaining", "b")),
        _spec("ATTITUDE", 30, 39, 7, ("time_boot_ms", "I"), ("roll", "f"), ("pitch", "f"), ("yaw", "f"),
              ("rollspeed", "f"), ("pitchspeed", "f"), ("yawspeed", "f")),
        _spec("ATTITUDE_QUATERNION", 31, 246, 8, ("time_boot_ms", "I"), ("q1", "f"), ("q2", "f"), ("q3", "f"),
              ("q4", "f"), ("rollspeed", "f"), ("pitchspeed", "f"), ("yawspeed", "f"), ("repr_offset_q", "4f")),
        _spec("LOCAL_POSITION_NED", 32, 185, 7, ("time_boot_ms", "I"), ("x", "f"), ("y", "f"), ("z", "f"),
              ("vx", "f"), ("vy", "f"), ("vz", "f")),
        _spec("GLOBAL_POSITION_INT", 33, 104, 9, ("time_boot_ms", "I"), ("lat", "i"), ("lon", "i"), ("alt", "i"),
              ("relative_alt", "i"), ("vx", "h"), ("vy", "h"), ("vz", "h"), ("hdg", "H")),
        _spec("GPS_RAW_INT", 24, 24, 10, ("time_usec", "Q"), ("lat", "i"), ("lon", "i"), ("alt", "i"),
              ("eph", "H"), ("epv", "H"), ("vel", "H"), ("cog", "H"), ("fix_type", "B"),
              ("satellites_visible", "B"), ("alt_ellipsoid", "i"), ("h_acc", "I"), ("v_acc", "I"),
              ("vel_acc", "I"), ("hdg_acc", "I"), ("yaw", "H")),
        _spec("COMMAND_LONG", 76, 152, 11, ("param1", "f"), ("param2", "f"), ("param3", "f"), ("param4", "f"),
              ("param5", "f"), ("param6", "f"), ("param7", "f"), ("command", "H"), ("target_system", "B"),
              ("target_component", "B"), ("confirmation", "B")),
        _spec("COMMAND_ACK", 77, 143, 2, ("command", "H"), ("result", "B"), ("progress", "B"),
              ("result_param2", "i"), ("target_system", "B"), ("target_component", "B")),
        _spec("SET_POSITION_TARGET_LOCAL_NED", 84, 143, 16, ("time_boot_ms", "I"), ("x", "f"), ("y", "f"),
              ("z", "f"), ("vx", "f"), ("vy", "f"), ("vz", "f"), ("afx", "f"), ("afy", "f"), ("afz", "f"),
              ("yaw", "f"), ("yaw_rate", "f"), ("type_mask", "H"), ("target_system", "B"),
              ("target_component", "B"), ("coordinate_frame", "B")),
        _spec("SET_POSITION_TARGET_GLOBAL_INT", 86, 5, 16, ("time_boot_ms", "I"), ("lat_int", "i"),
              ("lon_int", "i"), ("alt", "f"), ("vx", "f"), ("vy", "f"), ("vz", "f"), ("afx", "f"), ("afy", "f"),
              ("afz", "f"), ("yaw", "f"), ("yaw_rate", "f"), ("type_mask", "H"), ("target_system", "B"),
              ("target_component", "B"), ("coordinate_frame", "B")),
        _spec("STATUSTEXT", 253, 83, 2, ("severity", "B"), ("text", "50s"), ("id", "H"), ("chunk_seq", "B")),
        _spec("PARAM_REQUEST_READ", 20, 214, 4, ("param_index", "h"), ("target_system", "B"),
              ("target_component", "B"), ("param_id", "16s")),
        _spec("PARAM_VALUE", 22, 220, 5, ("param_value", "f"), ("param_count", "H"), ("param_index", "H"),
              ("param_id", "16s"), ("param_type", "B")),
        _spec("EXTENDED_SYS_STATE", 245, 130, 2, ("vtol_state", "B"), ("landed_state", "B")),
        _spec("AUTOPILOT_VERSION", 148, 178, 11, ("capabilities", "Q"), ("uid", "Q"), ("flight_sw_version", "I"),
              ("middleware_sw_version", "I"), ("os_sw_version", "I"), ("board_version", "I"), ("vendor_id", "H"),
              ("product_id", "H"), ("flight_custom_version", "8B"), ("middleware_custom_version", "8B"),
              ("os_custom_version", "8B"), ("uid2", "18B")),
    )
}
BY_ID: dict[int, Spec] = {spec.msg_id: spec for spec in SPECS.values()}


# ── The enums this driver uses ──────────────────────────────────────────────────────────────────────


class Cmd:
    """`MAV_CMD` numbers."""

    NAV_RETURN_TO_LAUNCH = 20
    NAV_LAND = 21
    NAV_TAKEOFF = 22
    DO_SET_MODE = 176
    COMPONENT_ARM_DISARM = 400
    SET_MESSAGE_INTERVAL = 511
    REQUEST_MESSAGE = 512


class Result:
    """`MAV_RESULT`."""

    ACCEPTED = 0
    TEMPORARILY_REJECTED = 1
    DENIED = 2
    UNSUPPORTED = 3
    FAILED = 4
    IN_PROGRESS = 5
    COMMAND_LONG_ONLY = 7
    COMMAND_INT_ONLY = 8

    #: 6 (`CANCELLED`) is in MAVLink's `common.xml` and not in the reference generated dialect this tree
    #: pins, so it is not named here and `layout_check.py` cannot hold it; an unnamed result prints as its number.
    NAMES = {0: "ACCEPTED", 1: "TEMPORARILY_REJECTED", 2: "DENIED", 3: "UNSUPPORTED", 4: "FAILED",
             5: "IN_PROGRESS", 7: "COMMAND_LONG_ONLY", 8: "COMMAND_INT_ONLY"}


class MavType:
    """`MAV_TYPE`: what the heartbeat says the vehicle is."""

    GENERIC = 0
    FIXED_WING = 1
    QUADROTOR = 2
    COAXIAL = 3
    HELICOPTER = 4
    ANTENNA_TRACKER = 5
    GCS = 6
    GROUND_ROVER = 10
    SURFACE_BOAT = 11
    SUBMARINE = 12
    HEXAROTOR = 13
    OCTOROTOR = 14
    TRICOPTER = 15
    ONBOARD_CONTROLLER = 18
    GIMBAL = 26
    ADSB = 27
    DODECAROTOR = 29
    DECAROTOR = 35

    MULTIROTORS = frozenset({2, 3, 4, 13, 14, 15, 29, 35})
    #: 19 to 25: the names MAVLink gives them have changed between releases (a tilt-rotor is 21 in all of them),
    #: so they are held by number, and by the reference's count of `VTOL` types.
    VTOLS = frozenset({19, 20, 21, 22, 23, 24, 25})


class MavState:
    """`MAV_STATE`."""

    UNINIT = 0
    BOOT = 1
    CALIBRATING = 2
    STANDBY = 3
    ACTIVE = 4
    CRITICAL = 5
    EMERGENCY = 6
    POWEROFF = 7
    FLIGHT_TERMINATION = 8

    NAMES = {0: "UNINIT", 1: "BOOT", 2: "CALIBRATING", 3: "STANDBY", 4: "ACTIVE", 5: "CRITICAL",
             6: "EMERGENCY", 7: "POWEROFF", 8: "FLIGHT_TERMINATION"}


class ModeFlag:
    """`MAV_MODE_FLAG`, the bits of a heartbeat's `base_mode`."""

    CUSTOM_MODE_ENABLED = 1
    GUIDED_ENABLED = 8
    SAFETY_ARMED = 128


class Landed:
    """`MAV_LANDED_STATE`."""

    UNDEFINED = 0
    ON_GROUND = 1
    IN_AIR = 2
    TAKEOFF = 3
    LANDING = 4

    NAMES = {0: "undefined", 1: "on_ground", 2: "in_air", 3: "takeoff", 4: "landing"}


class Autopilot:
    """`MAV_AUTOPILOT`."""

    ARDUPILOTMEGA = 3
    INVALID = 8
    PX4 = 12


class Sensor:
    """`MAV_SYS_STATUS_SENSOR` bits a health report is read against."""

    GYRO = 1 << 0
    ACCEL = 1 << 1
    MAG = 1 << 2
    ABSOLUTE_PRESSURE = 1 << 3
    GPS = 1 << 5
    AHRS = 1 << 21


class Frame:
    """`MAV_FRAME`."""

    LOCAL_NED = 1
    GLOBAL_RELATIVE_ALT_INT = 6


# ArduPilot's custom modes, by vehicle. Source: ArduPilot 4.7.1's `Mode::Number` in each vehicle
# (ArduCopter/mode.h, ArduPlane/mode.h, Rover/mode.h, ArduSub/defines.h), as `pymavlink`'s
# `ardupilotmega` dialect carries them in `COPTER_MODE_*`, `PLANE_MODE_*`, `ROVER_MODE_*`, `SUB_MODE_*`.
COPTER_MODES: dict[str, int] = {
    "stabilize": 0, "acro": 1, "alt_hold": 2, "auto": 3, "guided": 4, "loiter": 5, "rtl": 6, "circle": 7,
    "land": 9, "drift": 11, "sport": 13, "flip": 14, "autotune": 15, "poshold": 16, "brake": 17,
    "throw": 18, "avoid_adsb": 19, "guided_nogps": 20, "smart_rtl": 21, "flowhold": 22, "follow": 23,
    "zigzag": 24, "systemid": 25, "autorotate": 26, "auto_rtl": 27, "turtle": 28,
}
PLANE_MODES: dict[str, int] = {
    "manual": 0, "circle": 1, "stabilize": 2, "training": 3, "acro": 4, "fly_by_wire_a": 5,
    "fly_by_wire_b": 6, "cruise": 7, "autotune": 8, "auto": 10, "rtl": 11, "loiter": 12, "takeoff": 13,
    "avoid_adsb": 14, "guided": 15, "initializing": 16, "qstabilize": 17, "qhover": 18, "qloiter": 19,
    "qland": 20, "qrtl": 21, "qautotune": 22, "qacro": 23, "thermal": 24, "loiter_alt_qland": 25,
    "autoland": 26,
}
ROVER_MODES: dict[str, int] = {
    "manual": 0, "acro": 1, "steering": 3, "hold": 4, "loiter": 5, "follow": 6, "simple": 7, "dock": 8,
    "circle": 9, "auto": 10, "rtl": 11, "smart_rtl": 12, "guided": 15, "initializing": 16,
}
SUB_MODES: dict[str, int] = {
    "stabilize": 0, "acro": 1, "alt_hold": 2, "auto": 3, "guided": 4, "circle": 7, "surface": 9,
    "poshold": 16, "manual": 19, "motordetect": 20, "surftrak": 21,
}
FAMILY_MODES: dict[str, dict[str, int]] = {
    "copter": COPTER_MODES, "plane": PLANE_MODES, "rover": ROVER_MODES, "sub": SUB_MODES,
}


# ── CRC ─────────────────────────────────────────────────────────────────────────────────────────────


def crc_x25(data: bytes, crc: int = 0xFFFF) -> int:
    """CRC-16/MCRF4XX, MAVLink's checksum (the reference calls it X.25)."""
    for byte in data:
        tmp = byte ^ (crc & 0xFF)
        tmp = (tmp ^ (tmp << 4)) & 0xFF
        crc = ((crc >> 8) ^ (tmp << 8) ^ (tmp << 3) ^ (tmp >> 4)) & 0xFFFF
    return crc


# ── Encoding ────────────────────────────────────────────────────────────────────────────────────────


def _pack(spec: Spec, values: dict) -> bytes:
    out = bytearray()
    for name, code in spec.fields:
        given = values.get(name)
        count = int(code[:-1]) if len(code) > 1 and code[:-1].isdigit() else 1
        kind = code[-1]
        try:
            if kind == "s":
                data = given if isinstance(given, (bytes, bytearray)) else str(given or "").encode("utf-8")
                if len(data) > count:
                    raise ProtocolError(f"{spec.name}.{name} is {len(data)} bytes and the field holds {count}")
                out += data.ljust(count, b"\0")
            elif count > 1:
                items = list(given) if given is not None else [0] * count
                if len(items) != count:
                    raise ProtocolError(f"{spec.name}.{name} needs {count} elements, got {len(items)}")
                out += struct.pack(f"<{count}{kind}", *items)
            else:
                out += struct.pack(f"<{kind}", 0 if given is None else given)
        except struct.error as error:
            raise ProtocolError(f"{spec.name}.{name}={given!r} does not fit `{code}`: {error}") from error
    return bytes(out)


def encode(spec: Spec, values: dict, *, seq: int, system_id: int, component_id: int, incompat: int = 0) -> bytes:
    """One MAVLink 2 frame. The payload's trailing zeros are cut, but never below a byte.

    Unsigned: this driver holds no key. `incompat` exists so the simulated vehicle can send a frame with the
    signed flag set (the flag is part of what the checksum covers, so it cannot be set afterwards); the
    signature bytes are the caller's to append.
    """
    payload = _pack(spec, values).rstrip(b"\0") or b"\0"
    header = bytes([len(payload), incompat & 0xFF, 0, seq & 0xFF, system_id & 0xFF, component_id & 0xFF]) + spec.msg_id.to_bytes(3, "little")
    crc = crc_x25(header + payload)
    crc = crc_x25(bytes([spec.crc_extra]), crc)
    return bytes([STX_V2]) + header + payload + crc.to_bytes(2, "little")


# ── Decoding ────────────────────────────────────────────────────────────────────────────────────────


@dataclass
class Packet:
    """One frame the parser accepted."""

    msg_id: int
    system_id: int
    component_id: int
    seq: int
    version: int
    signed: bool
    payload: bytes
    #: The name and decoded fields when this module knows the message; `None` for an id it does not.
    name: str | None = None
    fields: dict = field(default_factory=dict)


def _unpack(spec: Spec, payload: bytes) -> dict:
    """Fields from a payload of any length: short ones are zero-padded, long ones' tail is ignored."""
    size = spec.size
    data = payload[:size].ljust(size, b"\0")
    values = spec.fmt.unpack(data)
    out: dict = {}
    index = 0
    for name, code in spec.fields:
        count = int(code[:-1]) if len(code) > 1 and code[:-1].isdigit() else 1
        kind = code[-1]
        if kind == "s":
            raw = values[index]
            out[name] = raw.split(b"\0", 1)[0].decode("utf-8", "replace")
            index += 1
        elif count > 1:
            out[name] = list(values[index : index + count])
            index += count
        else:
            out[name] = values[index]
            index += 1
    return out


class Parser:
    """A byte stream in, packets out.

    Bytes that are not a frame are skipped one at a time to the next start byte, so a stream that starts
    mid-frame, or carries line noise, costs the frames it damaged and no more. A frame whose CRC fails
    is counted and dropped; the bytes after its first are searched again, because a start byte inside
    a payload can begin a real frame and a length inside noise cannot be trusted.

    An id this module does not know cannot be checked (the CRC needs its seed), so it is *passed* with
    `name=None` rather than dropped; nothing here acts on it.
    """

    def __init__(self) -> None:
        self._buffer = bytearray()
        self.bad_crc = 0
        self.skipped_bytes = 0
        self.unknown = 0

    def feed(self, data: bytes) -> list[Packet]:
        self._buffer += data
        out: list[Packet] = []
        buf = self._buffer
        while True:
            start = -1
            for index, byte in enumerate(buf):
                if byte in (STX_V1, STX_V2):
                    start = index
                    break
            if start < 0:
                self.skipped_bytes += len(buf)
                buf.clear()
                break
            if start:
                self.skipped_bytes += start
                del buf[:start]
            stx = buf[0]
            if stx == STX_V2:
                if len(buf) < 12:
                    break
                length, incompat = buf[1], buf[2]
                total = 10 + length + 2 + (_SIGNATURE_LEN if incompat & INCOMPAT_SIGNED else 0)
            else:
                if len(buf) < 8:
                    break
                length = buf[1]
                total = 6 + length + 2
            if len(buf) < total:
                break
            frame = bytes(buf[:total])
            packet = self._accept(frame, stx)
            if packet is None:
                # Not a frame after all: drop this start byte only and look again.
                del buf[:1]
                continue
            del buf[:total]
            out.append(packet)
        return out

    def _accept(self, frame: bytes, stx: int) -> Packet | None:
        if stx == STX_V2:
            length, incompat = frame[1], frame[2]
            signed = bool(incompat & INCOMPAT_SIGNED)
            seq, system_id, component_id = frame[4], frame[5], frame[6]
            msg_id = int.from_bytes(frame[7:10], "little")
            payload = frame[10 : 10 + length]
            crc_end = 10 + length
            version = 2
            if incompat & ~INCOMPAT_SIGNED:
                # A flag this parser does not know: the frame's layout is not the one described above.
                self.bad_crc += 1
                return None
        else:
            length = frame[1]
            seq, system_id, component_id, msg_id = frame[2], frame[3], frame[4], frame[5]
            payload = frame[6 : 6 + length]
            crc_end = 6 + length
            signed = False
            version = 1
        spec = BY_ID.get(msg_id)
        got = int.from_bytes(frame[crc_end : crc_end + 2], "little")
        if spec is not None:
            want = crc_x25(frame[1:crc_end])
            want = crc_x25(bytes([spec.crc_extra]), want)
            if want != got:
                self.bad_crc += 1
                return None
            return Packet(msg_id, system_id, component_id, seq, version, signed, bytes(payload), spec.name,
                          _unpack(spec, bytes(payload)))
        self.unknown += 1
        return Packet(msg_id, system_id, component_id, seq, version, signed, bytes(payload))
