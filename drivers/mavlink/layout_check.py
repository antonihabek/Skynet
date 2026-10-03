"""This driver's MAVLink against the reference implementation's.

    python3 -m venv .venv && .venv/bin/pip install pymavlink==2.4.49
    .venv/bin/python layout_check.py

`skynet_mavlink.frames` transcribes nineteen messages — ids, CRC seeds, field order and widths — the
enums a command and a heartbeat are read against, ArduPilot's mode numbers for four vehicle families, and
PX4's packed mode word.
A transcription is right on the day it is made. `pymavlink` is the reference implementation MAVLink's own
generator writes from `common.xml`, so this is the check that what is here is still what the protocol says.

What it holds, and what that would catch:

* **The seeds** — each message's `CRC_EXTRA` is *recomputed* here from the field list and compared with the
  reference's. A seed that matches proves the non-extension fields are the reference's, in name, type and
  order, which is the property the checksum exists to protect.
* **The bytes** — every message encoded by this driver and by the reference from the same values must be the
  same bytes; every frame the reference builds must decode to the same fields here. A field the driver
  packs one place off is a command that moves a vehicle somewhere else.
* **The parser** — a corrupted frame is refused by both, an id nobody here knows is passed and not dropped,
  a frame split across reads is one frame, and line noise before a frame costs only the noise.
* **The enums** — `MAV_CMD`, `MAV_RESULT`, `MAV_TYPE`, `MAV_STATE`, the mode-flag bits, the landed states,
  the sensor bits, and every custom mode of the four ArduPilot vehicle families, in both directions.
* **PX4's modes** — each name here is the reference's main and sub mode packed into the word a heartbeat carries, the
  reference reads every one back as the name it was given, and the two it has that this driver leaves out are named.
* **The checksum** — this module's CRC against the reference's on the frames above.

Exits non-zero on the first disagreement it cannot explain, and says what was compared.
"""

from __future__ import annotations

import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from skynet_mavlink import frames  # noqa: E402

FAILURES: list[str] = []
CHECKS = 0


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")


# MAVLink's own type names, for recomputing a seed the way its generator does (mavgen.py, `message_checksum`).
_TYPE_NAME = {"B": "uint8_t", "b": "int8_t", "H": "uint16_t", "h": "int16_t", "I": "uint32_t", "i": "int32_t",
              "Q": "uint64_t", "q": "int64_t", "f": "float", "d": "double", "s": "char"}


def recompute_crc_extra(spec: frames.Spec) -> int:
    """The seed mavgen derives from a message's name and its non-extension fields, in wire order."""
    crc = frames.crc_x25(f"{spec.name} ".encode("ascii"))
    for name, code in spec.fields[: spec.base]:
        count = int(code[:-1]) if len(code) > 1 else 0
        kind = code[-1]
        crc = frames.crc_x25(f"{_TYPE_NAME[kind]} ".encode("ascii"), crc)
        crc = frames.crc_x25(f"{name} ".encode("ascii"), crc)
        if count:
            crc = frames.crc_x25(bytes([count]), crc)
    return (crc & 0xFF) ^ (crc >> 8)


def sample(spec: frames.Spec) -> dict:
    """Deterministic, non-zero values for every field, so a field packed one place off shows."""
    values: dict = {}
    for index, (name, code) in enumerate(spec.fields, start=1):
        count = int(code[:-1]) if len(code) > 1 else 0
        kind = code[-1]
        if kind == "s":
            values[name] = f"text{index}".encode("ascii")
        elif kind == "f":
            one = lambda i: float(index * 3 + i) + 0.5  # noqa: E731 (exactly representable in float32)
            values[name] = [one(i) for i in range(count)] if count else one(0)
        elif kind in "Bb":
            one = lambda i: (index * 7 + i) % 100 + 1  # noqa: E731
            values[name] = [one(i) for i in range(count)] if count else one(0)
        else:
            width = struct.calcsize(kind)
            top = (1 << (8 * width - 2)) - 1
            one = lambda i: (index * 100003 + i * 17 + 5) % top  # noqa: E731
            values[name] = [one(i) for i in range(count)] if count else one(0)
    return values


def main() -> int:
    try:
        from pymavlink import mavutil  # noqa: PLC0415
        from pymavlink.dialects.v20 import ardupilotmega as ap  # noqa: PLC0415
        from pymavlink.dialects.v20 import common as common  # noqa: PLC0415
    except ImportError:
        print(
            "::error::pymavlink is not importable. This check compares the driver with the reference\n"
            "implementation and has nothing to compare without it:\n"
            "  python3 -m venv .venv && .venv/bin/pip install pymavlink==2.4.49 && .venv/bin/python layout_check.py"
        )
        return 2

    import pymavlink  # noqa: PLC0415

    print(f"skynet-mavlink layout check, against pymavlink {getattr(pymavlink, '__version__', '?')}\n")

    print("== the CRC")
    check(frames.crc_x25(b"123456789") == 0x6F91, "X.25 of the standard check string is 0x6F91 (CRC-16/MCRF4XX)")
    ref = mavutil.x25crc()
    ref.accumulate(b"123456789")
    check(ref.crc == frames.crc_x25(b"123456789"), "the reference's accumulator agrees on the check string")

    print("\n== every message: id, seed, wire order, widths")
    for spec in frames.SPECS.values():
        cls = getattr(common, f"MAVLink_{spec.name.lower()}_message", None)
        if cls is None:
            check(False, f"{spec.name}: the reference has no such message")
            continue
        check(cls.id == spec.msg_id, f"{spec.name}: id {spec.msg_id}")
        check(cls.crc_extra == spec.crc_extra, f"{spec.name}: transcribed seed {spec.crc_extra} is the reference's")
        check(
            recompute_crc_extra(spec) == cls.crc_extra,
            f"{spec.name}: the seed recomputed from these fields ({recompute_crc_extra(spec)}) is the reference's",
        )
        check(
            [name for name, _ in spec.fields] == list(cls.ordered_fieldnames),
            f"{spec.name}: field names in wire order",
        )
        check(cls.unpacker.format.lstrip("<") == "".join(code for _, code in spec.fields),
              f"{spec.name}: widths and counts ({''.join(code for _, code in spec.fields)})")

    print("\n== the bytes, both directions")
    for spec in frames.SPECS.values():
        values = sample(spec)
        ours = frames.encode(spec, values, seq=9, system_id=255, component_id=190)
        mav = common.MAVLink(None, srcSystem=255, srcComponent=190)
        mav.seq = 9
        cls = getattr(common, f"MAVLink_{spec.name.lower()}_message")
        args = []
        for name in cls.fieldnames:
            v = values[name]
            args.append(v)
        theirs = cls(*args).pack(mav, force_mavlink1=False)
        check(bytes(theirs) == ours, f"{spec.name}: the frame this driver builds is the reference's, byte for byte")
        parser = frames.Parser()
        got = parser.feed(bytes(theirs))
        good = len(got) == 1 and got[0].name == spec.name
        if good:
            decoded = got[0].fields
            for name, code in spec.fields:
                expect = values[name]
                have = decoded[name]
                if code.endswith("s"):
                    expect = expect.decode("ascii")
                if isinstance(expect, list):
                    good = good and [float(x) for x in have] == [float(x) for x in expect]
                else:
                    good = good and have == expect
        check(good, f"{spec.name}: the reference's frame decodes here to the values it was built from")

    print("\n== the parser")
    hb = frames.SPECS["HEARTBEAT"]
    frame = frames.encode(hb, {"type": 2, "autopilot": 3, "base_mode": 89, "system_status": 4, "mavlink_version": 3},
                          seq=1, system_id=1, component_id=1)
    check(len(frames.Parser().feed(frame)) == 1, "one heartbeat is one packet")
    parser = frames.Parser()
    pieces = [parser.feed(frame[i : i + 1]) for i in range(len(frame))]
    check(sum(len(p) for p in pieces) == 1 and len(pieces[-1]) == 1, "a frame delivered a byte at a time arrives whole, at its last byte")
    parser = frames.Parser()
    check(len(parser.feed(b"\x00\x01\x02garbage" + frame + frame)) == 2 and parser.skipped_bytes >= 10,
          "line noise before frames costs the noise and not the frames")
    damaged = bytearray(frame)
    damaged[13] ^= 0x40
    parser = frames.Parser()
    check(parser.feed(bytes(damaged)) == [] and parser.bad_crc >= 1, "a frame with one payload bit flipped is refused (CRC)")
    theirs_parser = mavutil.mavlink.MAVLink(None)
    theirs_parser.robust_parsing = True
    refused = [m for m in theirs_parser.parse_buffer(bytes(damaged)) or [] if m.get_type() == "HEARTBEAT"]
    check(refused == [], "the reference refuses the same frame")
    # An id nobody here knows: RAW_IMU (27) built by the reference.
    raw = common.MAVLink(None, srcSystem=1, srcComponent=1)
    imu = bytes(raw.raw_imu_encode(1, 2, 3, 4, 5, 6, 7, 8, 9, 10).pack(raw))
    unknown = frames.Parser().feed(imu)
    check(len(unknown) == 1 and unknown[0].name is None and unknown[0].msg_id == 27, "a message this module does not know is passed with no name, not dropped")
    # MAVLink 1: the reference builds one, this parser reads it, and extensions are absent from it.
    v1 = common.MAVLink(None, srcSystem=1, srcComponent=1)
    v1frame = bytes(v1.heartbeat_encode(2, 3, 89, 0, 4, 3).pack(v1, force_mavlink1=True))
    packet = frames.Parser().feed(v1frame)
    check(len(packet) == 1 and packet[0].version == 1 and packet[0].fields["type"] == 2, "a MAVLink 1 heartbeat is read")
    v1ack = bytes(v1.command_ack_encode(400, 4).pack(v1, force_mavlink1=True))
    packet = frames.Parser().feed(v1ack)
    check(len(packet) == 1 and packet[0].fields["command"] == 400 and packet[0].fields["result"] == 4 and packet[0].fields["target_system"] == 0,
          "a MAVLink 1 command ack decodes and its extension fields read as zero")
    signed = bytearray(frames.encode(hb, {"type": 2, "autopilot": 3, "mavlink_version": 3}, seq=2, system_id=1, component_id=1))
    signed[2] |= 1
    signed = bytes(signed[:10 + signed[1]]) + b"\0\0" + b"\0" * 13
    body = bytearray(signed)
    crc = frames.crc_x25(bytes(body[1:10 + body[1]]))
    crc = frames.crc_x25(bytes([hb.crc_extra]), crc)
    body[10 + body[1] : 12 + body[1]] = crc.to_bytes(2, "little")
    packets = frames.Parser().feed(bytes(body))
    check(len(packets) == 1 and packets[0].signed, "a frame with the signed flag carries its 13-byte signature and is reported as signed")

    print("\n== the enums")
    pairs = {
        "MAV_CMD": [(frames.Cmd, "NAV_RETURN_TO_LAUNCH"), (frames.Cmd, "NAV_LAND"), (frames.Cmd, "NAV_TAKEOFF"),
                    (frames.Cmd, "DO_SET_MODE"), (frames.Cmd, "DO_REPOSITION"), (frames.Cmd, "COMPONENT_ARM_DISARM"),
                    (frames.Cmd, "SET_MESSAGE_INTERVAL"), (frames.Cmd, "REQUEST_MESSAGE")],
        "MAV_RESULT": [(frames.Result, n) for n in ("ACCEPTED", "TEMPORARILY_REJECTED", "DENIED", "UNSUPPORTED", "FAILED", "IN_PROGRESS", "COMMAND_LONG_ONLY", "COMMAND_INT_ONLY")],
        "MAV_TYPE": [(frames.MavType, n) for n in (
            "GENERIC", "FIXED_WING", "QUADROTOR", "COAXIAL", "HELICOPTER", "ANTENNA_TRACKER", "GCS", "GROUND_ROVER", "SURFACE_BOAT",
            "SUBMARINE", "HEXAROTOR", "OCTOROTOR", "TRICOPTER", "ONBOARD_CONTROLLER", "GIMBAL",
            "ADSB", "DODECAROTOR", "DECAROTOR")],
        "MAV_STATE": [(frames.MavState, n) for n in ("UNINIT", "BOOT", "CALIBRATING", "STANDBY", "ACTIVE", "CRITICAL", "EMERGENCY", "POWEROFF", "FLIGHT_TERMINATION")],
        "MAV_MODE_FLAG": [(frames.ModeFlag, n) for n in ("CUSTOM_MODE_ENABLED", "GUIDED_ENABLED", "SAFETY_ARMED")],
        "MAV_LANDED_STATE": [(frames.Landed, n) for n in ("UNDEFINED", "ON_GROUND", "IN_AIR", "TAKEOFF", "LANDING")],
        "MAV_AUTOPILOT": [(frames.Autopilot, n) for n in ("ARDUPILOTMEGA", "INVALID", "PX4")],
        "MAV_SYS_STATUS_SENSOR": [(frames.Sensor, "GYRO"), (frames.Sensor, "ACCEL"), (frames.Sensor, "MAG"),
                                  (frames.Sensor, "ABSOLUTE_PRESSURE"), (frames.Sensor, "GPS"), (frames.Sensor, "AHRS")],
        "MAV_FRAME": [(frames.Frame, "GLOBAL"), (frames.Frame, "LOCAL_NED"), (frames.Frame, "GLOBAL_RELATIVE_ALT_INT")],
    }
    theirs_names = {
        ("MAV_SYS_STATUS_SENSOR", "GYRO"): "MAV_SYS_STATUS_SENSOR_3D_GYRO",
        ("MAV_SYS_STATUS_SENSOR", "ACCEL"): "MAV_SYS_STATUS_SENSOR_3D_ACCEL",
        ("MAV_SYS_STATUS_SENSOR", "MAG"): "MAV_SYS_STATUS_SENSOR_3D_MAG",
        ("MAV_SYS_STATUS_SENSOR", "ABSOLUTE_PRESSURE"): "MAV_SYS_STATUS_SENSOR_ABSOLUTE_PRESSURE",
        ("MAV_SYS_STATUS_SENSOR", "AHRS"): "MAV_SYS_STATUS_AHRS",
        ("MAV_CMD", "NAV_RETURN_TO_LAUNCH"): "MAV_CMD_NAV_RETURN_TO_LAUNCH",
        ("MAV_MODE_FLAG", "CUSTOM_MODE_ENABLED"): "MAV_MODE_FLAG_CUSTOM_MODE_ENABLED",
        ("MAV_MODE_FLAG", "GUIDED_ENABLED"): "MAV_MODE_FLAG_GUIDED_ENABLED",
        ("MAV_MODE_FLAG", "SAFETY_ARMED"): "MAV_MODE_FLAG_SAFETY_ARMED",
    }
    for enum, members in pairs.items():
        for holder, name in members:
            theirs = theirs_names.get((enum, name), f"{enum}_{name}")
            have = getattr(ap, theirs, None)
            check(have is not None and have == getattr(holder, name), f"{enum}.{name} = {getattr(holder, name)}")
    vtols = {getattr(ap, n) for n in dir(ap) if n.startswith("MAV_TYPE_VTOL_") and isinstance(getattr(ap, n), int)}
    check(vtols == set(frames.MavType.VTOLS), f"MAV_TYPE: the VTOL types are {sorted(vtols)} in the reference and here")
    for family, table, prefix in (("copter", frames.COPTER_MODES, "COPTER_MODE_"), ("plane", frames.PLANE_MODES, "PLANE_MODE_"),
                                  ("rover", frames.ROVER_MODES, "ROVER_MODE_"), ("sub", frames.SUB_MODES, "SUB_MODE_")):
        theirs = {n[len(prefix):].lower(): getattr(ap, n) for n in dir(ap) if n.startswith(prefix) and isinstance(getattr(ap, n), int)}
        check(all(theirs.get(name) == number for name, number in table.items()),
              f"{family}: every mode here is the reference's, by name and number ({len(table)} modes)")
        missing = sorted(set(theirs) - set(table) - {"enum_end"})
        check(not missing, f"{family}: no mode the reference has is missing here" + (f" — missing {missing}" if missing else ""))

    print("\n== PX4's custom modes")
    # The reference's own table: name -> (base mode flags, main mode, sub mode). Its names are older than PX4's: what PX4 calls hold the
    # reference calls LOITER (AUTO_LOITER), and follow-target FOLLOWME.
    reference = {name.lower(): (flags, main, sub) for name, (flags, main, sub) in mavutil.px4_map.items()}
    pxnames = {"hold": "loiter", "follow_target": "followme"}
    for name, word in frames.PX4_MODES.items():
        theirs = reference.get(pxnames.get(name, name))
        check(theirs is not None and frames.px4_mode(theirs[1], theirs[2]) == word,
              f"PX4 {name}: {word:#010x} is the reference's {pxnames.get(name, name)} (main {theirs[1] if theirs else '?'}, sub {theirs[2] if theirs else '?'})")
        if theirs is not None:
            check(mavutil.interpret_px4_mode(theirs[0], word).lower() == pxnames.get(name, name),
                  f"PX4 {name}: the reference reads {word:#010x} back as {pxnames.get(name, name)}")
    absent = sorted(set(reference) - {pxnames.get(name, name) for name in frames.PX4_MODES})
    check(absent == ["rattitude", "rtgs"],
          f"PX4: the modes the reference has and this driver leaves out are rattitude (removed from PX4) and rtgs (deleted in 2020): {absent}")
    check(frames.px4_mode(4, 3) == 0x03040000, "and the packed word of PX4's boot mode, hold, is the one the firmware's own heartbeat carries (0x03040000)")

    print(f"\n{CHECKS - len(FAILURES)} passed, {len(FAILURES)} failed")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())
