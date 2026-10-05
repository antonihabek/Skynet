"""This driver's constants and frame layouts against ODrive's own sources.

    make check-layout ODRIVE_FW=/path/to/ODrive

`ODRIVE_FW` is a checkout of ODrive Robotics' firmware (`odriverobotics/ODrive`), read
unmodified and not vendored here. Three of the maker's sources are held against each other
and against `skynet_odrive.cansimple`:

* ``Firmware/communication/can/can_simple.hpp`` — the command enumeration (the ids);
* ``tools/odrive-cansimple.dbc`` — the CAN database: message ids, signal offsets, widths,
  byte order, scales, units, and the state/mode/error value tables (the layouts);
* ``tools/odrive/enums.py`` — imported unmodified: the same enumerations as Python.

Plus six behaviours out of ``can_simple.cpp`` the driver leans on (zero-length requests,
the 64-bit motor error, what the stop latches, what clear wipes, the float estimates, the
position setpoint's feed-forwards), held as source text: if the firmware rewords them the
check fails and a person re-reads the file rather than a regex guessing.

The checkout must be at the pinned commit. A newer firmware may change the protocol, and a
verdict about a different firmware than the pin names would be a verdict about nothing.
"""

from __future__ import annotations

import importlib.util
import os
import re
import subprocess
import sys

from skynet_odrive import cansimple

#: The firmware this driver's constants were read from. `git log --oneline -1` at the clone.
PIN = "3a2e4dd1bfbda9e4c534d5c7d8428a99c361e783"

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


#: `BO_ <id> <name>: <len> <sender>`
_BO = re.compile(r"^BO_\s+(\d+)\s+(\w+):\s+(\d+)\s+(\w+)")
#: `SG_ <name> : <start>|<len>@<order><sign> (<factor>,<offset>) [...] "<unit>" ...`
_SG = re.compile(r'^\s*SG_\s+(\w+)\s*:\s*(\d+)\|(\d+)@([01])([+-])\s*\(([^,]+),([^)]+)\)\s*\[[^\]]*\]\s*"([^"]*)"')
#: `VAL_ <id> <signal> <value> "<name>" ... ;`
_VAL_HEAD = re.compile(r"^VAL_\s+(\d+)\s+(\w+)\s+(.*);")


def parse_dbc(path: str) -> tuple[dict[int, dict], dict[tuple[int, str], dict[int, str]]]:
    """Message ids to their lengths and signals, and value tables by (id, signal)."""
    messages: dict[int, dict] = {}
    values: dict[tuple[int, str], dict[int, str]] = {}
    current: dict | None = None
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = _BO.match(line)
            if match:
                ident = int(match.group(1))
                current = {"name": match.group(2), "length": int(match.group(3)), "signals": {}}
                messages[ident] = current
                continue
            match = _SG.match(line)
            if match and current is not None:
                current["signals"][match.group(1)] = {
                    "start": int(match.group(2)),
                    "length": int(match.group(3)),
                    "order": match.group(4),
                    "signed": match.group(5),
                    "factor": float(match.group(6)),
                    "unit": match.group(8),
                }
                continue
            match = _VAL_HEAD.match(line)
            if match:
                table: dict[int, str] = {}
                for value, name in re.findall(r'(\d+)\s+"([^"]+)"', match.group(3)):
                    table[int(value)] = name
                values[(int(match.group(1)), match.group(2))] = table
    return messages, values


def parse_command_enum(path: str) -> dict[str, int]:
    """The `CANSimple` command enumeration: names to values, in firmware order."""
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    body = text.split("enum {", 1)[1].split("};", 1)[0]
    # Line comments first: splitting on commas first would let a trailing comment swallow
    # the entry on the next line (as `// Errors` did to `MSG_GET_ENCODER_ERROR` on the
    # first version of this check).
    body = "\n".join(line.split("//", 1)[0] for line in body.splitlines())
    entries: dict[str, int] = {}
    value = 0
    for chunk in body.split(","):
        chunk = chunk.strip()
        if not chunk:
            continue
        name, _, explicit = chunk.partition("=")
        name = name.strip()
        if explicit.strip():
            value = int(explicit.strip(), 0)
        entries[name] = value
        value += 1
    return entries


def load_enums(path: str):
    """The maker's `enums.py`, imported unmodified from the checkout."""
    spec = importlib.util.spec_from_file_location("odrive_enums", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> int:
    print("skynet-odrive layout check\n")
    fw = sys.argv[1] if len(sys.argv) > 1 else ""
    if not fw or not os.path.isfile(os.path.join(fw, "tools", "odrive-cansimple.dbc")):
        return die(
            "ODRIVE_FW does not point at ODrive's firmware checkout"
            + (f" ({fw})." if fw else ".")
            + " The layout claim needs the maker's own sources beside this tree's, so this "
            "gate refuses without them:\n\n"
            "  git clone https://github.com/odriverobotics/ODrive /tmp/odrive-fw\n"
            f"  git -C /tmp/odrive-fw checkout {PIN}\n"
            "  make check-layout ODRIVE_FW=/tmp/odrive-fw"
        )
    for need in ("tools/odrive/enums.py", "Firmware/communication/can/can_simple.hpp",
                 "Firmware/communication/can/can_simple.cpp"):
        if not os.path.isfile(os.path.join(fw, need)):
            return die(f"the checkout at {fw} has no {need}; it is not ODrive's firmware.")

    section("the pin")
    try:
        head = subprocess.run(["git", "-C", fw, "rev-parse", "HEAD"], capture_output=True,
                              text=True, check=True).stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return die(f"the checkout at {fw} is not a git tree; the pin cannot be verified.")
    check(head == PIN, f"the checkout is at the pinned commit ({PIN[:9]})")
    if head != PIN:
        print(f"  ----  it is at {head}. A verdict about another firmware is a verdict about")
        print("        nothing: check out the pin and run again.")
        return 1

    messages, values = parse_dbc(os.path.join(fw, "tools", "odrive-cansimple.dbc"))
    commands = parse_command_enum(os.path.join(fw, "Firmware/communication/can/can_simple.hpp"))
    enums = load_enums(os.path.join(fw, "tools/odrive/enums.py"))
    with open(os.path.join(fw, "Firmware/communication/can/can_simple.cpp"), encoding="utf-8") as handle:
        impl = handle.read()

    section("command ids — the firmware's enum against this driver's")
    firmware_to_local = {
        "MSG_CO_NMT_CTRL": "CMD_NMT",
        "MSG_ODRIVE_HEARTBEAT": "CMD_HEARTBEAT",
        "MSG_ODRIVE_ESTOP": "CMD_ESTOP",
        "MSG_GET_MOTOR_ERROR": "CMD_GET_MOTOR_ERROR",
        "MSG_GET_ENCODER_ERROR": "CMD_GET_ENCODER_ERROR",
        "MSG_GET_SENSORLESS_ERROR": "CMD_GET_SENSORLESS_ERROR",
        "MSG_SET_AXIS_NODE_ID": "CMD_SET_NODE_ID",
        "MSG_SET_AXIS_REQUESTED_STATE": "CMD_SET_REQUESTED_STATE",
        "MSG_SET_AXIS_STARTUP_CONFIG": "CMD_SET_STARTUP_CONFIG",
        "MSG_GET_ENCODER_ESTIMATES": "CMD_GET_ENCODER_ESTIMATES",
        "MSG_GET_ENCODER_COUNT": "CMD_GET_ENCODER_COUNT",
        "MSG_SET_CONTROLLER_MODES": "CMD_SET_CONTROLLER_MODES",
        "MSG_SET_INPUT_POS": "CMD_SET_INPUT_POS",
        "MSG_SET_INPUT_VEL": "CMD_SET_INPUT_VEL",
        "MSG_SET_INPUT_TORQUE": "CMD_SET_INPUT_TORQUE",
        "MSG_SET_LIMITS": "CMD_SET_LIMITS",
        "MSG_START_ANTICOGGING": "CMD_START_ANTICOGGING",
        "MSG_SET_TRAJ_VEL_LIMIT": "CMD_SET_TRAJ_VEL_LIMIT",
        "MSG_SET_TRAJ_ACCEL_LIMITS": "CMD_SET_TRAJ_ACCEL_LIMITS",
        "MSG_SET_TRAJ_INERTIA": "CMD_SET_TRAJ_INERTIA",
        "MSG_GET_IQ": "CMD_GET_IQ",
        "MSG_GET_SENSORLESS_ESTIMATES": "CMD_GET_SENSORLESS_ESTIMATES",
        "MSG_RESET_ODRIVE": "CMD_RESET",
        "MSG_GET_BUS_VOLTAGE_CURRENT": "CMD_GET_BUS_VOLTAGE_CURRENT",
        "MSG_CLEAR_ERRORS": "CMD_CLEAR_ERRORS",
        "MSG_SET_LINEAR_COUNT": "CMD_SET_LINEAR_COUNT",
        "MSG_SET_POS_GAIN": "CMD_SET_POS_GAIN",
        "MSG_SET_VEL_GAINS": "CMD_SET_VEL_GAINS",
        "MSG_GET_ADC_VOLTAGE": "CMD_GET_ADC_VOLTAGE",
        "MSG_GET_CONTROLLER_ERROR": "CMD_GET_CONTROLLER_ERROR",
    }
    for firmware_name, local_name in firmware_to_local.items():
        expected = commands.get(firmware_name)
        held = expected is not None and getattr(cansimple, local_name) == expected
        check(held, f"{local_name} is {expected:#04x}" if expected is not None
              else f"{firmware_name} is in the firmware enum")
    check(commands.get("MSG_CO_HEARTBEAT_CMD") == 0x700, "the CANOpen heartbeat cmd is untouched at 0x700")

    section("message ids — the DBC's BO_ lines against node << 5 | command")
    for node in (0, 1):
        for firmware_name, local_name in firmware_to_local.items():
            if firmware_name in ("MSG_CO_NMT_CTRL", "MSG_ODRIVE_ESTOP", "MSG_SET_AXIS_STARTUP_CONFIG"):
                continue  # the DBC models no BO_ for these; the firmware enum above carries them
            ident = node << 5 | getattr(cansimple, local_name)
            held = ident in messages
            if node == 0:
                check(held, f"axis 0 carries BO_ {ident} ({firmware_name})")
            elif not held:
                check(False, f"axis 1 carries BO_ {ident} ({firmware_name})")
    check((1 << 5 | cansimple.CMD_HEARTBEAT) in messages, "axis 1 heartbeats on 0x021")
    check((7 << 5 | cansimple.CMD_HEARTBEAT) in messages, "axis 7 heartbeats on 0x0e1")

    def signal(ident: int, name: str) -> dict:
        return messages[ident]["signals"][name]

    section("payload layouts — the DBC's SG_ lines against this driver's parses")
    heart = messages[0x01]["signals"]
    check((heart["Axis_Error"]["start"], heart["Axis_Error"]["length"]) == (0, 32),
          "heartbeat: the axis error first, 32 bits")
    check((heart["Axis_State"]["start"], heart["Axis_State"]["length"]) == (32, 8),
          "the state next, one byte")
    check(heart["Motor_Error_Flag"]["start"] == 40 and heart["Encoder_Error_Flag"]["start"] == 48
          and heart["Controller_Error_Flag"]["start"] == 56
          and heart["Trajectory_Done_Flag"]["start"] == 63,
          "then the three subsystem flags and trajectory-done")
    check(all(heart[name]["order"] == "1" for name in heart), "all little-endian")
    est = messages[0x09]["signals"]
    check((est["Pos_Estimate"]["start"], est["Pos_Estimate"]["length"]) == (0, 32)
          and (est["Vel_Estimate"]["start"], est["Vel_Estimate"]["length"]) == (32, 32),
          "estimates: position then velocity, 32 bits each")
    pos = messages[0x0C]["signals"]
    check((pos["Input_Pos"]["start"], pos["Input_Pos"]["length"]) == (0, 32),
          "set_input_pos: position first, 32 bits")
    check((pos["Vel_FF"]["start"], pos["Vel_FF"]["length"], pos["Vel_FF"]["signed"],
           pos["Vel_FF"]["factor"]) == (32, 16, "-", 0.001),
          "then a signed 16-bit velocity feed-forward at 0.001")
    check((pos["Torque_FF"]["start"], pos["Torque_FF"]["length"], pos["Torque_FF"]["signed"],
           pos["Torque_FF"]["factor"]) == (48, 16, "-", 0.001),
          "then a signed 16-bit torque feed-forward at 0.001")
    vel = messages[0x0D]["signals"]
    check((vel["Input_Vel"]["start"], vel["Input_Vel"]["length"]) == (0, 32)
          and (vel["Input_Torque_FF"]["start"], vel["Input_Torque_FF"]["length"]) == (32, 32),
          "set_input_vel: velocity then torque feed-forward, 32 bits each")
    check((signal(0x0E, "Input_Torque")["start"], signal(0x0E, "Input_Torque")["length"]) == (0, 32),
          "set_input_torque: one 32-bit field")
    check((signal(0x0B, "Control_Mode")["start"], signal(0x0B, "Input_Mode")["start"]) == (0, 32),
          "set_controller_modes: control first, input second")
    check((signal(0x07, "Axis_Requested_State")["start"],
           signal(0x07, "Axis_Requested_State")["length"]) == (0, 32),
          "set_axis_state: one 32-bit field")
    check((signal(0x03, "Motor_Error")["start"], signal(0x03, "Motor_Error")["length"]) == (0, 32),
          "F280a, pinned as an erratum: the DBC models the motor error as 32 bits")
    check(vel["Input_Vel"]["unit"] == "rev" and vel["Input_Torque_FF"]["unit"] == "rev/s",
          "F280b, pinned as an erratum: the DBC's input-velocity units are swapped")
    check(2 not in messages and 8 not in messages,
          "the committed DBC models no Estop or StartupConfig BO_: it predates the generator's")
    check((signal(0x04, "Encoder_Error")["start"], signal(0x04, "Encoder_Error")["length"]) == (0, 32),
          "the encoder error is 32 bits")
    check((signal(0x1D, "Controller_Error")["start"],
           signal(0x1D, "Controller_Error")["length"]) == (0, 32),
          "the controller error is 32 bits")

    section("enumerations — the DBC's VAL_ tables against the maker's enums.py")
    tables = {
        (0x01, "Axis_State"): ("AXIS_STATE_", 13),
        (0x01, "Axis_Error"): ("AXIS_ERROR_", 13),
        (0x03, "Motor_Error"): ("MOTOR_ERROR_", 28),
        (0x04, "Encoder_Error"): ("ENCODER_ERROR_", 11),
        (0x05, "Sensorless_Error"): ("SENSORLESS_ESTIMATOR_ERROR_", 3),
        (0x0B, "Control_Mode"): ("CONTROL_MODE_", 4),
        (0x0B, "Input_Mode"): ("INPUT_MODE_", 9),
        (0x1D, "Controller_Error"): ("CONTROLLER_ERROR_", 9),
    }
    for (ident, signal_name), (prefix, count) in tables.items():
        table = values.get((ident, signal_name), {})
        # `enums.py` spells what the DBC spells, with the enumeration's prefix in front.
        enum_names = {name[len(prefix):] for name, value in vars(enums).items()
                      if name.startswith(prefix) and isinstance(value, int)}
        check(set(table.values()) == enum_names and len(table) == count,
              f"{signal_name}: the DBC and enums.py name the same {count} values")

    section("enumerations — the maker's values against this driver's")
    check(cansimple.STATES == dict(values[(0x01, "Axis_State")]),
          "all thirteen axis states, by number (there is no state 5)")
    check(cansimple.STATE_IDLE == enums.AXIS_STATE_IDLE
          and cansimple.STATE_CLOSED_LOOP_CONTROL == enums.AXIS_STATE_CLOSED_LOOP_CONTROL,
          "idle is 1 and closed loop is 8")
    check(cansimple.CONTROL_MODE_POSITION == enums.CONTROL_MODE_POSITION_CONTROL
          and cansimple.CONTROL_MODE_VELOCITY == enums.CONTROL_MODE_VELOCITY_CONTROL
          and cansimple.CONTROL_MODE_TORQUE == enums.CONTROL_MODE_TORQUE_CONTROL,
          "position 3, velocity 2, torque 1")
    check(cansimple.INPUT_MODE_PASSTHROUGH == enums.INPUT_MODE_PASSTHROUGH, "passthrough is 1")

    section("behaviours — the firmware's can_simple.cpp against this driver's bets")
    check("msg.rtr || msg.len == 0" in impl,
          "a GET answers a zero-length frame as it answers an RTR")
    check("can_setSignal(txmsg, axis.motor_.error_, 0, 64, true)" in impl,
          "the motor error goes out as 64 bits (F280a: the DBC's 32 are stale)")
    check("axis.error_ |= Axis::ERROR_ESTOP_REQUESTED" in impl,
          "the stop latches ESTOP_REQUESTED in the axis")
    check("odrv.clear_errors()" in impl,
          "clear_errors wipes board-wide: no path of this driver sends it")
    check("can_setSignal<float>(txmsg, axis.controller_.pos_estimate_linear_src_" in impl
          and "can_setSignal<float>(txmsg, axis.controller_.vel_estimate_src_" in impl,
          "the estimates are floats, which the DBC cannot say")
    check("can_getSignal<float>(msg, 0, 32, true)" in impl
          and "can_getSignal<int16_t>(msg, 32, 16, true, 0.001f, 0)" in impl
          and "can_getSignal<int16_t>(msg, 48, 16, true, 0.001f, 0)" in impl,
          "the position setpoint is float32 then two int16s at 0.001")

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())
