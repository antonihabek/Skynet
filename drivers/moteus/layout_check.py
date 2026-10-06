"""This driver's constants and frames against mjbots's own sources.

    make check-layout MOTEUS=/path/to/moteus

`MOTEUS` is a checkout of mjbots's repository (`mjbots/moteus`, Apache-2.0), read unmodified and not vendored here. Four of the maker's sources are held against
each other and against `skynet_moteus.protocol`:

* ``lib/python/moteus/{multiplex,protocol,moteus,transport}.py`` — **the maker's own client, run**: every command this driver builds is built again by the maker's
  `Controller.make_*` and compared byte for byte; every conversion is the maker's `Writer` and `scale_register` against ours, at every width; the identifier is the
  maker's `Transport._command_to_frame`; the replies this driver's simulator makes are read by the maker's `parse_registers`;
* ``fw/bldc_servo_structs.h`` — the mode enumeration (`BldcServoMode`);
* ``fw/error.h`` — the fault and limiting codes (`errc`), which is where this check found that the register reference is not the whole of what the firmware sends;
* ``docs/protocol/registers.md`` — the register numbers and the fault table, parsed.

Plus the behaviours of the mode machine this driver leans on (`bldc_servo_control.h`'s `ISR_MaybeChangeMode`: a fault returns, a stopped controller starts calibrating,
a timeout is left only by a stop; and that a mode is changed only when the command's differs from the status), held as source text: if the firmware rewords them the check
fails and a person re-reads the file rather than a regex guessing.

The maker's client imports `pyserial`, `python-can` and `importlib_metadata` for its transports. This check never opens a port or a bus, so those three are replaced by
`unittest.mock.MagicMock`s for the import and nothing it asserts goes through them; the encoders, the parser and the frame constructor it holds this driver to are the maker's
own code, unmodified.

The checkout must be at the pinned commit. A newer firmware may change the protocol, and a verdict about a different firmware than the pin names would be a verdict about nothing.
"""

from __future__ import annotations

import importlib
import io
import math
import os
import re
import subprocess
import sys
import types
from unittest import mock

from skynet_moteus import protocol as p
from skynet_moteus.bus import FdFrame
from skynet_moteus.sim import SimulatedController

#: The commit this driver's constants were read from (`git log -1` at the clone; committed 2026-09-27).
PIN = "8f747f4ac448d117c1390b77ca2d80c27e751a56"

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


def section(title: str) -> None:
    print(f"\n{title}")


def die(message: str) -> int:
    print(f"::error::{message}")
    return 1


def same(a: float, b: float) -> bool:
    return (math.isnan(a) and math.isnan(b)) or a == b


# ── the maker's client, loaded without its transports ───────────────────────────────────────────────────────────────────────

def load_makers_client(checkout: str):
    """`moteus.multiplex`, `.protocol`, `.moteus` and `.transport`, imported from the checkout under a stub package: the real `moteus/__init__.py` imports every transport,
    each of which wants a third-party package this check has no use for."""
    for name in ("serial", "serial.tools", "serial.tools.list_ports", "can", "importlib_metadata"):
        sys.modules.setdefault(name, mock.MagicMock(name=name))
    root = os.path.join(checkout, "lib", "python", "moteus")
    package = types.ModuleType("moteus")
    package.__path__ = [root]  # type: ignore[attr-defined]
    sys.modules["moteus"] = package
    modules = {}
    for name in ("multiplex", "protocol", "command", "reader", "moteus", "transport"):
        modules[name] = importlib.import_module(f"moteus.{name}")
    return modules


# ── the firmware's enumerations ─────────────────────────────────────────────────────────────────────────────────────────────

def firmware_enum(text: str, header: str) -> dict[str, tuple[int, str]]:
    """The entries of the enum that opens with ``header`` (`enum BldcServoMode {`): name to (value, trailing comment)."""
    body = text.split(header, 1)[1].split("};", 1)[0]
    entries: dict[str, tuple[int, str]] = {}
    value = 0
    for raw in body.splitlines():
        code, _, comment = raw.partition("//")
        code = code.strip()
        if not code or code.startswith("*") or code.startswith("/*"):
            continue
        for item in code.split(","):
            item = item.strip()
            if not item:
                continue
            if "=" in item:
                name, number = (part.strip() for part in item.split("=", 1))
                value = int(number, 0)
            else:
                name = item
            entries[name] = (value, comment.strip())
            value += 1
    return entries


def words(name: str) -> str:
    """`kCalibrationComplete` as `calibrationcomplete`: the form a name and a documentation phrase can be compared in."""
    return re.sub(r"[^a-z0-9]", "", re.sub(r"^k", "", name).lower())


def squash(text: str) -> str:
    return re.sub(r"\s+", " ", text)


def main(argv: list[str]) -> int:
    if len(argv) != 1 or not argv[0]:
        return die("give the checkout of mjbots/moteus: `make check-layout MOTEUS=/path/to/moteus`. This check reads the maker's own sources and refuses to run without them: "
                   "a verdict about a protocol nobody read is not one.")
    checkout = os.path.abspath(argv[0])
    needed = [os.path.join(checkout, *parts) for parts in (("lib", "python", "moteus", "multiplex.py"), ("fw", "error.h"), ("fw", "bldc_servo_structs.h"),
                                                          ("fw", "bldc_servo_control.h"), ("docs", "protocol", "registers.md"))]
    missing = [path for path in needed if not os.path.exists(path)]
    if missing:
        return die(f"{checkout} is not a moteus checkout: {os.path.relpath(missing[0], checkout)} is missing.")
    try:
        head = subprocess.run(["git", "-C", checkout, "rev-parse", "HEAD"], capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        head = ""
    if head != PIN:
        return die(f"{checkout} is at {head or 'no commit git can name'} and this driver's constants were read at {PIN}. `git -C {checkout} checkout {PIN}`, then run this again; a "
                   "newer firmware may have changed the protocol, and a verdict about a different one than the pin names would be a verdict about nothing.")

    print("skynet-moteus layout check\n")
    maker = load_makers_client(checkout)
    mp, mpp, mm, transport = maker["multiplex"], maker["protocol"], maker["moteus"], maker["transport"]

    # ── registers ────────────────────────────────────────────────────────────
    section("registers — this driver's numbers against the maker's own client")
    names = {
        "MODE": "MODE", "POSITION_REG": "POSITION", "VELOCITY_REG": "VELOCITY", "TORQUE_REG": "TORQUE", "Q_CURRENT": "Q_CURRENT", "D_CURRENT": "D_CURRENT",
        "ABS_POSITION": "ABS_POSITION", "POWER_REG": "POWER", "MOTOR_TEMPERATURE": "MOTOR_TEMPERATURE", "TRAJECTORY_COMPLETE": "TRAJECTORY_COMPLETE", "HOME_STATE": "HOME_STATE",
        "VOLTAGE_REG": "VOLTAGE", "TEMPERATURE_REG": "TEMPERATURE", "FAULT": "FAULT", "COMMAND_POSITION": "COMMAND_POSITION", "COMMAND_VELOCITY": "COMMAND_VELOCITY",
        "COMMAND_FEEDFORWARD_TORQUE": "COMMAND_FEEDFORWARD_TORQUE", "COMMAND_KP_SCALE": "COMMAND_KP_SCALE", "COMMAND_KD_SCALE": "COMMAND_KD_SCALE",
        "COMMAND_MAX_TORQUE": "COMMAND_POSITION_MAX_TORQUE", "COMMAND_STOP_POSITION": "COMMAND_STOP_POSITION", "COMMAND_WATCHDOG": "COMMAND_TIMEOUT",
        "COMMAND_VELOCITY_LIMIT": "COMMAND_VELOCITY_LIMIT", "COMMAND_ACCEL_LIMIT": "COMMAND_ACCEL_LIMIT", "MODEL_NUMBER": "MODEL_NUMBER",
        "FIRMWARE_VERSION": "FIRMWARE_VERSION", "REGISTER_MAP_VERSION": "REGISTER_MAP_VERSION",
    }
    for ours, theirs in names.items():
        check(getattr(p, ours) == int(mpp.Register[theirs]), f"{ours} is register {getattr(p, ours):#05x}, the maker's `Register.{theirs}`")
    registers_md = open(os.path.join(checkout, "docs", "protocol", "registers.md"), encoding="utf-8").read()
    headings = {int(m.group(1), 16): m.group(2) for m in re.finditer(r"^### (0x[0-9a-fA-F]+) - ([^\n]+)$", registers_md, re.M)}
    for ours in ("MODE", "POSITION_REG", "VELOCITY_REG", "TORQUE_REG", "HOME_STATE", "VOLTAGE_REG", "TEMPERATURE_REG", "FAULT", "COMMAND_POSITION", "COMMAND_VELOCITY",
                 "COMMAND_FEEDFORWARD_TORQUE", "COMMAND_KP_SCALE", "COMMAND_KD_SCALE", "COMMAND_MAX_TORQUE", "COMMAND_WATCHDOG"):
        check(getattr(p, ours) in headings, f"…and the register reference has a section for {getattr(p, ours):#05x} ({headings.get(getattr(p, ours), '—')})")

    # ── modes ────────────────────────────────────────────────────────────────
    section("modes — against the maker's client and the firmware's `BldcServoMode`")
    firmware_modes = firmware_enum(open(os.path.join(checkout, "fw", "bldc_servo_structs.h"), encoding="utf-8").read(), "enum BldcServoMode {")
    firmware_modes.pop("kNumModes", None)
    check(sorted(v for v, _ in firmware_modes.values()) == sorted(p.MODES), f"the firmware has {len(firmware_modes)} modes, numbered as this driver's {len(p.MODES)}")
    aliases = {"timeout": "positiontimeout", "staywithin": "staywithinbounds"}
    by_value = {value: words(name) for name, (value, _) in firmware_modes.items()}
    for value, name in p.MODES.items():
        wanted = aliases.get(re.sub(r"[^a-z0-9]", "", name), re.sub(r"[^a-z0-9]", "", name))
        check(by_value.get(value) == wanted, f"mode {value} is `{name}` here and `{firmware_modes and next(n for n, (v, _) in firmware_modes.items() if v == value)}` in the firmware")
    for member in mpp.Mode:
        check(p.MODES.get(int(member)) is not None and int(member) in by_value, f"the maker's `Mode.{member.name}` ({int(member)}) is a mode this driver names and the firmware has")
    for ours in ("MODE_STOPPED", "MODE_FAULT", "MODE_POSITION", "MODE_TIMEOUT", "MODE_ZERO_VELOCITY", "MODE_BRAKE"):
        check(getattr(p, ours) == int(mpp.Mode[ours.removeprefix("MODE_")]), f"{ours} is {getattr(p, ours)}, the maker's `Mode.{ours.removeprefix('MODE_')}`")

    # ── faults ───────────────────────────────────────────────────────────────
    section("faults — against the firmware's `errc` and the register reference's table")
    codes = firmware_enum(open(os.path.join(checkout, "fw", "error.h"), encoding="utf-8").read(), "enum class errc {")
    servo = {name: (v, c) for name, (v, c) in codes.items() if 32 <= v <= 50}
    limiting = {name: (v, c) for name, (v, c) in codes.items() if v >= 96}
    transport_codes = {name: v for name, (v, _) in codes.items() if 1 <= v <= 7}
    check(sorted(v for v, _ in servo.values()) == sorted(p.FAULTS), f"the firmware's servo faults are {min(p.FAULTS)} to {max(p.FAULTS)}, all {len(p.FAULTS)} of them this driver's")
    check(sorted(v for v, _ in limiting.values()) == sorted(p.LIMITING), "…its limiting codes are 96 to 105, all ten of them this driver's")
    docs_bullets = {int(n): re.sub(r"\s+", " ", t) for n, t in re.findall(r"^\* (\d+) - \*([^*]+)\*", registers_md, re.M)}
    check({k: v for k, v in docs_bullets.items() if k >= 32} == p.FAULTS, "…and the register reference's table of fault codes is this driver's, name for name")
    check({k: v for k, v in docs_bullets.items() if k < 32} == {0: "relative only", 1: "rotor", 2: "output"},
          "…and its home states are 0 relative only, 1 rotor and 2 output: only the last is a position referenced to the output shaft, which is the one this driver reads as homed")
    special = {"startoutsidelimit": "outsidelimit", "bemffeedforwardnoaccellimit": "bemffeedforwardnoaccel", "pwmcycleoverrun": "pwmcycleoverrun"}
    for name, (value, _) in servo.items():
        ours = re.sub(r"[^a-z0-9]", "", p.FAULTS[value])
        check(special.get(words(name), words(name)) == ours, f"fault {value} is `{p.FAULTS[value]}` here and `{name}` in the firmware")
    docs_limits = {int(n): c for n, c in re.findall(r"^\* (\d+) - `([^`]+)`", registers_md, re.M)}
    for value, text in docs_limits.items():
        check(value in p.LIMITING and p.LIMITING[value].startswith(text), f"the register reference's limiting code {value} (`{text}`) is this driver's, name for name")
    documented_limit_codes = sorted(int(n) for n in re.findall(r"^\* (\d+) - ", registers_md, re.M) if int(n) >= 96)
    check(documented_limit_codes == list(range(96, 104)), "the register reference documents the limiting codes 96 to 103 and no others")
    check(104 in p.LIMITING and 105 in p.LIMITING and 104 not in documented_limit_codes and 105 not in documented_limit_codes
          and codes["kLimitFluxBraking"][0] == 104 and codes["kLimitFieldWeakening"][0] == 105,
          "the firmware sends 104 (flux braking) and 105 (field weakening), which the register reference does not list: the maker's own sources disagree (F285), and this driver follows the firmware")
    check(sorted(transport_codes.values()) == list(range(1, 8)), "the firmware's codes 1 to 7 are UART and DMA errors of the diagnostic port, not servo states, and this driver reads none of them as one")

    # ── quantities ───────────────────────────────────────────────────────────
    section("quantities — the maker's `Writer` and `scale_register` against this driver's, at every width")
    samples = [0.0, 0.123, -0.123, 1.0, -1.0, 12.5, -12.5, 100.0, -100.0, 1e6, -1e6, 3.2767, 0.0096, math.nan, math.inf, -math.inf]
    for name, scales in (("position", p.POSITION), ("velocity", p.VELOCITY), ("accel", p.ACCEL), ("torque", p.TORQUE), ("pwm", p.PWM), ("voltage", p.VOLTAGE),
                         ("temperature", p.TEMPERATURE), ("time", p.TIME), ("current", p.CURRENT), ("power", p.POWER)):
        agree = True
        for resolution in (mp.INT8, mp.INT16, mp.INT32, mp.F32):
            for value in samples:
                buffer = io.BytesIO()
                getattr(mpp.Writer(buffer), f"write_{name}")(value, resolution)
                ours = p.pack_value(p.encode(value, resolution, scales), resolution)
                if buffer.getvalue() != ours:
                    agree = False
                    print(f"        {name} {value!r} at {resolution}: maker {buffer.getvalue().hex()}, here {ours.hex()}")
        check(agree, f"{name} writes the same bytes as the maker's `write_{name}` for {len(samples)} values at int8, int16, int32 and float")
    reading = (("POSITION", p.POSITION), ("VELOCITY", p.VELOCITY), ("TORQUE", p.TORQUE), ("VOLTAGE", p.VOLTAGE), ("TEMPERATURE", p.TEMPERATURE))
    raws = {mp.INT8: [0, 1, -1, 50, -127, -128, 127], mp.INT16: [0, 1, -1, 1234, -32767, -32768, 32767], mp.INT32: [0, 1, -1, 123456, -2147483647, -2147483648, 2147483647],
            mp.F32: [0.0, 0.5, -0.5, 12.25, math.nan]}
    for name, scales in reading:
        agree = True
        for resolution, values in raws.items():
            for raw in values:
                theirs = mpp.scale_register(mpp.Register[name], resolution, raw)
                ours = p.decode(raw, resolution, scales)
                if not same(float(theirs), ours) and not (abs(float(theirs) - ours) <= 1e-12 * max(1.0, abs(ours))):
                    agree = False
                    print(f"        {name} {raw!r} at {resolution}: maker {theirs!r}, here {ours!r}")
        check(agree, f"{name.lower()} reads the same value as the maker's `scale_register` for every raw value at every width, the most negative integer as NaN")

    # ── frames ───────────────────────────────────────────────────────────────
    section("frames — every command this driver builds, built again by the maker's own `Controller`")
    qr = mm.QueryResolution()
    qr.home_state = mp.INT8
    controller = mm.Controller(id=1, query_resolution=qr)
    check(controller.make_query().data == p.query(), "the query is the maker's own, with home state in the run beside voltage, temperature and fault")
    default = mm.Controller(id=1).make_query().data
    check(default == bytes.fromhex("11001f01130d") and p.query().startswith(default[:4]), "…and the maker's default query is the same registers without home state, so the first two runs are shared")
    for label, kwargs, ours in (
        ("a position", dict(position=0.25, velocity=0.0, feedforward_torque=0.0, kp_scale=1.0, kd_scale=1.0, maximum_torque=math.nan), p.position_command(0.25, 0.0, 0.0, 1.0, 1.0, math.nan, None)),
        ("a negative position with a ceiling", dict(position=-0.4, velocity=0.0, feedforward_torque=0.0, kp_scale=1.0, kd_scale=1.0, maximum_torque=5.0), p.position_command(-0.4, 0.0, 0.0, 1.0, 1.0, 5.0, None)),
        ("a position with a watchdog", dict(position=0.25, velocity=0.0, feedforward_torque=0.0, kp_scale=1.0, kd_scale=1.0, maximum_torque=5.0, watchdog_timeout=0.2),
         p.position_command(0.25, 0.0, 0.0, 1.0, 1.0, 5.0, 0.2)),
        ("a hold (no position)", dict(position=math.nan, velocity=0.0, feedforward_torque=0.0, kp_scale=1.0, kd_scale=1.0, maximum_torque=math.nan),
         p.position_command(math.nan, 0.0, 0.0, 1.0, 1.0, math.nan, None)),
        ("a velocity", dict(position=math.nan, velocity=0.5, feedforward_torque=0.0, kp_scale=1.0, kd_scale=1.0, maximum_torque=math.nan), p.position_command(math.nan, 0.5, 0.0, 1.0, 1.0, math.nan, None)),
        ("a torque", dict(position=math.nan, velocity=0.0, feedforward_torque=0.75, kp_scale=0.0, kd_scale=0.0, maximum_torque=math.nan), p.position_command(math.nan, 0.0, 0.75, 0.0, 0.0, math.nan, None)),
    ):
        check(controller.make_position(**kwargs).data == ours, f"{label} is the maker's `make_position`, byte for byte")
    check(controller.make_stop().data == p.stop_command(p.MODE_STOPPED), "a stop is the maker's `make_stop`")
    check(controller.make_zero_velocity().data == p.stop_command(p.MODE_ZERO_VELOCITY), "…zero velocity is `make_zero_velocity`")
    check(controller.make_brake().data == p.stop_command(p.MODE_BRAKE), "…and brake is `make_brake`")
    check(controller.make_diagnostic_write(b"conf get servopos.position_min\n").data == p.diagnostic_write(1, b"conf get servopos.position_min\n"), "the diagnostic write is the maker's `make_diagnostic_write`")
    check(controller.make_diagnostic_read(48).data == p.diagnostic_poll(1, 48), "…and the poll is `make_diagnostic_read`")

    # ── the conversation id ──────────────────────────────────────────────────
    section("the identifier — the maker's `Transport._command_to_frame`")
    shell = transport.Transport.__new__(transport.Transport)
    for source in (0, 1, 0x40, 0x7F):
        for destination in (1, 2, 0x7F):
            for reply in (False, True):
                command = mm.Controller(id=destination, source_can_id=source).make_query() if reply else mm.Controller(id=destination, source_can_id=source).make_stop()
                frame = shell._command_to_frame(command)
                ours = p.can_id(source, destination, reply)
                good = frame.arbitration_id == ours and bool(frame.is_extended_id) == (ours > 0x7FF) and p.split_id(ours) == (source, destination, reply)
                check(good, f"source {source}, controller {destination}, reply {'wanted' if reply else 'not wanted'}: {ours:#06x}, {'extended' if ours > 0x7FF else 'standard'}, as the maker sends it")
    check(FdFrame(0x8001, b"", extended=True).extended and (mm.Controller(id=1).make_query().reply_required is True), "a frame that asks for a reply is the one that needs an extended identifier, and the maker's query asks for one")

    # ── replies ──────────────────────────────────────────────────────────────
    section("replies — what this driver's simulated controller sends, read by the maker's parser")
    sim = SimulatedController(1, position_min=-0.25, position_max=0.25)
    sim.mode, sim.position, sim.velocity, sim.torque, sim.voltage, sim.temperature, sim.fault = p.MODE_POSITION, 0.1234, -0.0625, 0.4, 24.0, 31.0, 0
    frame = sim.handle(p.query(), True, 0)
    assert frame is not None
    parsed = mpp.parse_registers(frame.data)
    check(parsed.response[mpp.Register.MODE] == p.MODE_POSITION and abs(parsed.response[mpp.Register.POSITION] - 0.1234) < 1e-6 and abs(parsed.response[mpp.Register.VELOCITY] + 0.0625) < 1e-6,
          "the maker's parser reads the simulated controller's mode, position and velocity as the values it holds")
    check(abs(parsed.response[mpp.Register.TORQUE] - 0.4) < 1e-6 and parsed.response[mpp.Register.VOLTAGE] == 24.0 and parsed.response[mpp.Register.TEMPERATURE] == 31.0
          and parsed.response[mpp.Register.FAULT] == 0 and parsed.response[mpp.Register.HOME_STATE] == 2, "…and its torque, voltage, temperature, fault and home state")
    check(not parsed.errors and parsed.query == [] and parsed.command == {}, "…with no error, no query and no command in a reply")
    check(frame.arbitration_id == 0x0100 and not frame.extended and mm.parse_message(types.SimpleNamespace(arbitration_id=frame.arbitration_id, data=frame.data)).id == 1,
          "the reply is from controller 1 to sender 0 (0x0100) and the maker reads it as controller 1's")
    status = p.parse_status(frame.data)
    check(status.mode == p.MODE_POSITION and abs(status.position - 0.1234) < 1e-6 and abs(status.torque - 0.4) < 1e-6 and status.voltage == 24.0 and status.home_state == 2,
          "this driver's own parse of the same frame agrees")
    sim.inject_fault(33)
    parsed = mpp.parse_registers(sim.handle(p.query(), True, 0).data)  # type: ignore[union-attr]
    check(parsed.response[mpp.Register.MODE] == p.MODE_FAULT and parsed.response[mpp.Register.FAULT] == 33, "a faulted controller is a mode 1 and a code, as the maker reads it")
    diagnostic = sim.handle(p.diagnostic_write(1, b"conf get servopos.position_min\n") + p.diagnostic_poll(1, 48), True, 0)
    assert diagnostic is not None
    check(mm.parse_diagnostic_message(types.SimpleNamespace(data=diagnostic.data), 1) == b"-0.25\r\n" and p.parse_diagnostic_reply(diagnostic.data, 1) == b"-0.25\r\n",
          "the diagnostic channel's answer is read the same by the maker's parser and by this driver's")
    command_frame = controller.make_position(position=0.25, velocity=0.0, feedforward_torque=0.0, kp_scale=1.0, kd_scale=1.0, maximum_torque=5.0, watchdog_timeout=0.2).data
    maker_view = mpp.parse_registers(command_frame).command
    ours_view = {i.register: i.value for i in p.parse_frame(command_frame) if isinstance(i, p.Register)}
    check(len(maker_view) == len(ours_view) and all(same(float(maker_view[r]) if r != 0 else 0.0, float(p.decode(ours_view[r], p.F32, (1.0, 1.0, 1.0))) if r != 0 else 0.0) for r in maker_view),
          "a command the maker built is read by this driver's parser as the maker's own reads it: the same registers, the same values")

    # ── the mode machine, as source text ─────────────────────────────────────
    section("the firmware's mode machine — the behaviours this driver and its simulator lean on, held as source text")
    control = squash(open(os.path.join(checkout, "fw", "bldc_servo_control.h"), encoding="utf-8").read())
    structs = squash(open(os.path.join(checkout, "fw", "bldc_servo_structs.h"), encoding="utf-8").read().replace("//", " "))
    for label, snippet in (
        ("a mode is changed only when the command's differs from the status", "if (data->mode != self().status_.mode) { ISR_MaybeChangeMode(data); }"),
        ("a fault ignores a command that is not a stop", "case kFault: { return; }"),
        ("a stopped controller starts calibrating on the first command", "case kStopped: { ISR_StartCalibrating(); return; }"),
        ("position control refuses to start outside its bounds", "(data->mode == kPosition || data->mode == kStayWithinBounds) && !data->ignore_position_bounds && ISR_IsOutsideLimits()"),
        ("…and the fault it sets is the start-outside-limit one", "self().status_.fault = errc::kStartOutsideLimit;"),
        ("a stop to `stopped` is taken from any mode", "case kStopped: { self().status_.mode = kStopped; return; }"),
    ):
        check(snippet in control, f"{label}")
    for label, snippet in (
        ("a position timeout is left only by a stop", "The only way to exit this state is through a stop command."),
        ("a fault is left only by a stop", "The only valid transition from this state is to kStopped."),
        ("a stop disables the whole motor driver", "In this mode, the entire motor driver will be disabled."),
    ):
        check(snippet in structs, f"{label}")
    registers_text = squash(registers_md)
    check("this register must be written before any other registers in the command frame" in registers_text, "the register reference says the mode register is written first in a command frame")
    check("The default is 0.0, which means to use the system-wide configured default. NaN / maximally negative means apply no enforced timeout." in registers_text,
          "…and that a watchdog of 0.0 is the configured default and NaN is none — which is why this driver never writes NaN there")

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 0 if not FAILURES else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
