"""The LeRobot bridge over LeRobot's own Koch and OpenMANIPULATOR-X classes, on an emulated Dynamixel bus — items 12.D (lerobot) and 12.E.5.

`selftest_wire.py` did this for the SO-101 on Feetech servos. This is the other servo family LeRobot drives, and the two robot classes that use it:
`lerobot.robots.koch_follower.KochFollower` and `lerobot.robots.omx_follower.OmxFollower` as shipped, constructed the way production constructs them (`open_bridge`:
a JSON configuration, LeRobot's own config registry, `make_robot_from_config`), talking through LeRobot's own `DynamixelMotorsBus` and ROBOTIS's own `dynamixel_sdk`
(protocol 2.0) to a pseudo-terminal that `sim/dynamixel_bus.py` answers as a bus of six X-series servos. Nothing on LeRobot's side is patched or imported for a stub.

    ~/lerobot-venv/bin/python drivers/lerobot/selftest_wire_dynamixel.py

# Two arms, because they start differently

* The **Koch** is opened *calibrated*: servos holding their homing offsets and travel, and LeRobot's calibration file saying the same, so LeRobot finds nothing to
  rewrite. Its `use_degrees` is on, so the joints are LeRobot's degrees and the gripper its percentage.
* The **OpenMANIPULATOR-X** is opened *as it ships*: no calibration file and servos with no offset and the full travel. LeRobot's own `OmxFollower.calibrate()` runs
  (it is non-interactive) and writes the factory values to the EEPROM and to the file, which is the one path here that makes the class write calibration itself.
  Its `use_degrees` is off, LeRobot's default, so every joint is -100..100 and nothing may be converted to radians.

# What it can and cannot show

It shows that the bridge drives the library the library's way: the handshake, the calibration an arm already has or writes for itself, the sync reads and writes,
the torque enable and the EEPROM lock all reach the wire as bytes that ROBOTIS's SDK frames and checks, and a command arrives on the servo as the raw step LeRobot's
own normalisation says it should. It shows what the bridge does when a servo stops answering, a reply is corrupt, or a servo reports a hardware error. It is
**rung 4** (`docs/robot-support.md` §3): the servo is a counterpart written here from the maker's control table, not ROBOTIS's firmware, so a pass is not evidence the
arm moves, and no number here is a servo's measurement.
"""

from __future__ import annotations

import json
import math
import os
import sys
import tempfile
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "bindings", "python"))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "sim"))

try:
    import dynamixel_sdk  # noqa: F401
    import lerobot  # noqa: F401
except ImportError as error:  # a refusal, not a pass: the claim of this file needs the library
    print(f"selftest_wire_dynamixel: {error}. This needs LeRobot and ROBOTIS's SDK (pip install 'lerobot[dynamixel]'), e.g. ~/lerobot-venv.", file=sys.stderr)
    sys.exit(2)

import skynet_rhal as rhal  # noqa: E402
from skynet_rhal import Command, State  # noqa: E402
from skynet_lerobot.bridge import open_bridge  # noqa: E402
from skynet_lerobot.roles import ROLE_TABLES  # noqa: E402
from dynamixel_bus import DynamixelBus, HW_OVERLOAD  # noqa: E402
from wire_common import check, finish, holds_for, roles_cover_every_lerobot_robot, wait_until  # noqa: E402

MOTORS = ["shoulder_pan", "shoulder_lift", "elbow_flex", "wrist_flex", "wrist_roll", "gripper"]
RESOLUTION = 4096
#: Control-table addresses written out from ROBOTIS's e-manual (the emulator reads them from LeRobot's table), so that a disagreement shows.
HOMING_OFFSET, MAX_LIMIT, MIN_LIMIT, GOAL_POSITION = 20, 48, 52, 116
INST_WRITE, INST_SYNC_WRITE = 0x03, 0x83

KOCH_IDS = {1: "xl430-w250", 2: "xl430-w250", 3: "xl330-m288", 4: "xl330-m288", 5: "xl330-m288", 6: "xl330-m288"}
OMX_IDS = {11: "xl430-w250", 12: "xl430-w250", 13: "xl430-w250", 14: "xl330-m288", 15: "xl330-m288", 16: "xl330-m288"}
#: What the Koch was calibrated to: (homing offset, travel low, travel high) per motor, with both signs of offset so two's complement is on the wire.
KOCH_CALIBRATION = {
    "shoulder_pan": (0, 0, 4095),
    "shoulder_lift": (35, 700, 3400),
    "elbow_flex": (-42, 650, 3300),
    "wrist_flex": (100, 800, 3300),
    "wrist_roll": (-17, 0, 4095),
    "gripper": (0, 1900, 3300),
}
#: What LeRobot writes for an OpenMANIPULATOR-X that has no calibration file (`OmxFollower.calibrate`).
OMX_CALIBRATION = {motor: (0, 0, 4095) for motor in MOTORS}


def normalise(mode: str, raw: float, low: int, high: int) -> float:
    """LeRobot's `MotorNormMode`, from `motors_bus._normalize`: degrees about the middle of the travel, -100..100 across it, or 0..100 across it."""
    if mode == "deg":
        return (raw - (low + high) / 2) * 360.0 / (RESOLUTION - 1)
    if mode == "m100":
        return (raw - low) / (high - low) * 200.0 - 100.0
    return (raw - low) / (high - low) * 100.0


def unnormalise(mode: str, value: float, low: int, high: int) -> int:
    """LeRobot's `_unnormalize`: the inverse of the above, to the raw step."""
    if mode == "deg":
        return int(round(value * (RESOLUTION - 1) / 360.0 + (low + high) / 2))
    if mode == "m100":
        return int(round((value + 100.0) / 200.0 * (high - low) + low))
    return int(round(value / 100.0 * (high - low) + low))


def command(bridge, values, mask=None, seq=1) -> int:
    mask = mask if mask is not None else [1] * len(values)
    return bridge.write_command(Command(stamp_ns=1, seq=seq, mode=rhal.MODE_POSITION, values=list(values), mask=list(mask)), 0)


def read(bridge) -> tuple[int, State]:
    state = State()
    return bridge.read_state(state, 0), state


def eeprom_writes(bus: DynamixelBus, start: int = 0) -> list[int]:
    """The addresses of every WRITE the bus has seen since `start` that landed on a calibration register."""
    found = []
    for _, instruction, params in bus.packets[start:]:
        if instruction == INST_WRITE and len(params) >= 2:
            address = params[0] | (params[1] << 8)
            if address in (HOMING_OFFSET, MAX_LIMIT, MIN_LIMIT):
                found.append(address)
    return found


def koch() -> None:
    """LeRobot's own `KochFollower`, calibrated, in degrees."""
    with tempfile.TemporaryDirectory() as directory:
        bus = DynamixelBus(KOCH_IDS)
        path = bus.start()
        try:
            calibration = {}
            for ident, motor in zip(KOCH_IDS, MOTORS):
                offset, low, high = KOCH_CALIBRATION[motor]
                bus.calibrate(ident, homing_offset=offset, range_min=low, range_max=high)
                servo = bus.servo(ident)
                servo.actual = (low + high) / 2 - offset  # at the middle of its travel: Present_Position = Actual_Position + Homing_Offset
                servo._sync_registers()
                servo.goal = servo.reported
                servo._poke("Goal_Position", int(servo.goal))
                calibration[motor] = {"id": ident, "drive_mode": 0, "homing_offset": offset, "range_min": low, "range_max": high}
            Path(directory, "emulated.json").write_text(json.dumps(calibration), encoding="utf-8")
            config = json.dumps({"robot": {"type": "koch_follower", "port": path, "id": "emulated", "calibration_dir": directory, "use_degrees": True}, "simulator": True})
            bridge = open_bridge(config)
            rc = bridge.open(config)
            check("rhal.abi.version", "opens LeRobot's own Koch class over the emulated Dynamixel bus, with the calibration an arm already has", rc == rhal.SKR_OK, f"open returned {rc}")
            if rc != rhal.SKR_OK:
                return
            robot = bridge._robot
            ids = list(KOCH_IDS)
            check("rhal.abi.version", "the robot is LeRobot's `KochFollower`, as shipped", type(robot).__module__ == "lerobot.robots.koch_follower.koch_follower", type(robot).__module__)
            check("rhal.abi.version", "LeRobot found the arm calibrated and did not rewrite its calibration", eeprom_writes(bus) == [], str(eeprom_writes(bus)))
            check("rhal.abi.version", "connect configured the motors: return delay 0 on every servo", all(bus.servo(i).peek("Return_Delay_Time") == 0 for i in ids))
            check("rhal.abi.version", "extended position on the five joints and current-based position on the gripper, as `KochFollower.configure` says",
                  [bus.servo(i).peek("Operating_Mode") for i in ids] == [4, 4, 4, 4, 4, 5], str([bus.servo(i).peek("Operating_Mode") for i in ids]))
            check("rhal.abi.version", "the elbow's position gains are the ones LeRobot sets (P 1500, I 0, D 600)",
                  (bus.servo(3).peek("Position_P_Gain"), bus.servo(3).peek("Position_I_Gain"), bus.servo(3).peek("Position_D_Gain")) == (1500, 0, 600))
            check("rhal.abi.version", "and torque is on, because `torque_disabled` gives it back", all(bus.servo(i).peek("Torque_Enable") == 1 for i in ids))

            descriptor = json.loads(bridge.describe())
            check("embodiment.descriptor.valid", "the joints are LeRobot's motors, in LeRobot's order", [j["name"] for j in descriptor["joints"]] == MOTORS, str([j["name"] for j in descriptor["joints"]]))
            check("embodiment.roles.coverage", "all six carry the roles §67.2 maps, the Koch's own table", all(j["role"] for j in descriptor["joints"]) and descriptor["joints"][2]["role"] == "right_arm.elbow.flex",
                  str([j["role"] for j in descriptor["joints"]]))
            check("embodiment.descriptor.valid", "it declares itself a simulator, with no envelope and no ratings it was not given", descriptor["simulator"] is True and descriptor["envelope"] is None and all(j["limits"] == {} for j in descriptor["joints"]))
            units = descriptor["provenance"]["units"]
            check("embodiment.descriptor.valid", "and says the gripper is passed through in percent while the arm is converted from degrees (F220)", "gripper: percent, passed through unconverted" in units and "converted from deg" in units, units)

            rc, state = read(bridge)
            check("rhal.state.completeness", "read_state succeeds over the wire", rc == rhal.SKR_OK)
            expected = [math.radians(normalise("deg", bus.servo(i).reported, *KOCH_CALIBRATION[m][1:])) for i, m in zip(ids[:5], MOTORS[:5])]
            check("rhal.state.completeness", "each arm joint reads the shaft as LeRobot's degrees, in radians, to a step",
                  all(abs(state.position_rad[i] - expected[i]) < math.radians(0.2) for i in range(5)), f"read {[round(v, 4) for v in state.position_rad[:5]]}, expected {[round(v, 4) for v in expected]}")
            check("rhal.state.completeness", "the gripper reads as a percentage of its travel, unconverted", abs(state.position_rad[5] - normalise("pct", bus.servo(6).reported, *KOCH_CALIBRATION["gripper"][1:])) < 0.2, str(state.position_rad[5]))
            check("rhal.state.completeness", "an unreported velocity and effort are NaN", all(math.isnan(v) for v in state.velocity_rad_s) and all(math.isnan(v) for v in state.effort_nm))

            # A command lands on the servo as the raw step LeRobot's own normalisation says, in one SYNC_WRITE for all six.
            before = len(bus.packets)
            target_rad = 0.4
            values = list(state.position_rad)
            values[0] = target_rad
            rc = command(bridge, values)
            check("rhal.command.rejection", "a position command is accepted", rc == rhal.SKR_OK, str(rc))
            wait_until(lambda: any(inst == INST_SYNC_WRITE for _, inst, _ in bus.packets[before:]), timeout=1.0)
            writes = [params for _, inst, params in bus.packets[before:] if inst == INST_SYNC_WRITE]
            check("rhal.command.rejection", "it reaches the wire as one SYNC_WRITE to Goal_Position, four bytes, for all six servos",
                  len(writes) == 1 and (writes[0][0] | writes[0][1] << 8) == GOAL_POSITION and (writes[0][2] | writes[0][3] << 8) == 4 and (len(writes[0]) - 4) // 5 == 6,
                  str(list(writes[0][:6])) if writes else "no SYNC_WRITE")
            goal_raw = bus.servo(1).peek("Goal_Position")
            wanted = unnormalise("deg", math.degrees(target_rad), *KOCH_CALIBRATION["shoulder_pan"][1:])
            check("rhal.command.rejection", "and the servo's goal is the step LeRobot's degrees-to-raw says (±1)", abs(goal_raw - wanted) <= 1, f"goal {goal_raw}, wanted {wanted}")
            check("rhal.command.rejection", "the arm follows it, and the bridge reads the move back as the same angle",
                  wait_until(lambda: abs(read(bridge)[1].position_rad[0] - target_rad) < math.radians(0.5), 8.0), f"reads {read(bridge)[1].position_rad[0]:.4f} rad")

            # §24: a masked joint holds where it was last told to be.
            held = bus.servo(2).peek("Goal_Position")
            values[0] = 0.0
            rc = command(bridge, values, mask=[1, 0, 1, 1, 1, 1], seq=2)
            check("safety.watchdog.fires", "a masked joint is held at the setpoint it was last given, not at its reading and not at the command's value",
                  rc == rhal.SKR_OK and abs(bus.servo(2).peek("Goal_Position") - held) <= 1, f"goal {bus.servo(2).peek('Goal_Position')}, was {held}")

            # LeRobot's `sync_read` makes one try (`num_retry` is 0 unless a caller says otherwise), so a reply that fails its CRC makes that read an I/O error and never a
            # position taken from a packet that did not check out; the very next read, with the bus as it was, succeeds. A servo that stops answering is the same error.
            check("rhal.state.completeness", "a healthy arm's state carries no flag and no fault word", read(bridge)[1].flags == 0 and read(bridge)[1].fault_code == 0)
            bus.corrupt(1)
            check("rhal.state.completeness", "one reply that fails its CRC makes that read an I/O error, not a position (LeRobot makes one try)", read(bridge)[0] == rhal.SKR_EIO)
            check("rhal.state.completeness", "and the next read succeeds", read(bridge)[0] == rhal.SKR_OK)
            bus.silence(4)
            rc, _ = read(bridge)
            check("rhal.state.completeness", "a servo that stops answering is SKR_EIO, with no exception out of the driver", rc == rhal.SKR_EIO, str(rc))
            bus.silence(4, False)
            check("rhal.state.completeness", "and the next read succeeds once it answers again", wait_until(lambda: read(bridge)[0] == rhal.SKR_OK))

            # The stop: torque off on every servo of the bus, the latch visible in the state, commands refused; the release returns torque.
            rc = bridge.emergency_stop()
            check("safety.estop.latency", "emergency_stop is acknowledged", rc == rhal.SKR_OK)
            check("safety.estop.latency", "and torque is off on all six servos of the bus", all(bus.servo(i).peek("Torque_Enable") == 0 for i in ids), str([bus.servo(i).peek("Torque_Enable") for i in ids]))
            rc, stopped = read(bridge)
            check("safety.estop.latency", "the latch shows in the next state", rc == rhal.SKR_OK and bool(stopped.flags & rhal.FLAG_ESTOP))
            check("safety.estop.latency", "and a stop the bridge itself made is not read as a servo letting go: ESTOP, and no FAULT at any look for half a second (the servos are asked every 100 ms)",
                  holds_for(lambda: bool(read(bridge)[1].flags & rhal.FLAG_ESTOP) and not read(bridge)[1].flags & rhal.FLAG_FAULT, 0.5))
            check("safety.estop.latency", "and every later command is refused", command(bridge, values, seq=3) == rhal.SKR_EPERM)
            pushed = bus.servo(1).reported
            bus.push(1, 100)
            check("safety.estop.latency", "a servo with torque off can be moved by hand, and the bridge reads where it was moved to",
                  abs(read(bridge)[1].position_rad[0] - math.radians(normalise("deg", pushed + 100, *KOCH_CALIBRATION["shoulder_pan"][1:]))) < math.radians(0.3))
            rc = bridge.release_stop()
            check("safety.estop.latency", "release returns torque to all six and clears the latch", rc == rhal.SKR_OK and all(bus.servo(i).peek("Torque_Enable") == 1 for i in ids))

            # A servo that has shut itself down on an overload (F232). LeRobot reads the error byte of every reply and throws it away, and its observation is positions
            # only, so without the bridge asking the servo's own error register this arm read as healthy while one joint had let go: a read that succeeded, a state with no
            # flag, a command accepted, and an arm that did not move.
            health = json.loads(bridge.describe())["provenance"].get("health", "")
            check("embodiment.descriptor.valid", "the descriptor says the servos' error register is read, and that torque is held to be on", "`Hardware_Error_Status`" in health and "torque is off" in health, health)
            bus.fault(2, HW_OVERLOAD)
            check("safety.watchdog.fires", "a servo that shuts down on an overload is a FAULT in the state within half a second, and DEGRADED with it (§27 reads that bit)",
                  wait_until(lambda: read(bridge)[1].flags & rhal.FLAG_FAULT != 0, 0.5) and read(bridge)[1].flags & rhal.FLAG_DEGRADED != 0)
            check("safety.watchdog.fires", "the fault word is the servo's id and its own error bits, verbatim", read(bridge)[1].fault_code == (2 << 16 | HW_OVERLOAD), hex(read(bridge)[1].fault_code))
            check("rhal.command.rejection", "every command is refused while it stands, because the servo ignores a goal and a success would be one the arm does not make", command(bridge, values, seq=4) == rhal.SKR_EPERM)
            # LeRobot's `disable_torque()` raises at the first reply that carries the alert, which is servo 2, so servos 3 to 6 kept their torque when the stop was
            # one call. Each is tried now, and the register is read back.
            check("safety.estop.latency", "the stop on a bus where one servo has an alert takes the torque off the other five too, not only the ones before it",
                  bridge.emergency_stop() == rhal.SKR_OK and all(bus.servo(i).peek("Torque_Enable") == 0 for i in ids), str([bus.servo(i).peek("Torque_Enable") for i in ids]))
            bus.servo(2).reboot()
            check("safety.estop.latency", "and the release returns it to every servo, the one that was rebooted included", bridge.release_stop() == rhal.SKR_OK and all(bus.servo(i).peek("Torque_Enable") == 1 for i in ids),
                  str([bus.servo(i).peek("Torque_Enable") for i in ids]))
            # A servo that does not obey: its torque stays on whatever it is told. The stop cannot be done, and must say so rather than return success; the others are still cut.
            bus.servo(4).stuck_torque = True
            check("safety.estop.latency", "a stop that one servo does not obey is an I/O error, and the other five are cut all the same",
                  bridge.emergency_stop() == rhal.SKR_EIO and bus.servo(4).peek("Torque_Enable") == 1 and all(bus.servo(i).peek("Torque_Enable") == 0 for i in ids if i != 4),
                  str([bus.servo(i).peek("Torque_Enable") for i in ids]))
            check("safety.estop.latency", "and the latch holds all the same: a stop that failed still refuses every command", command(bridge, values, seq=7) == rhal.SKR_EPERM)
            bus.servo(4).stuck_torque = False
            check("safety.estop.latency", "released once the servo obeys, torque is back on all six", bridge.release_stop() == rhal.SKR_OK and all(bus.servo(i).peek("Torque_Enable") == 1 for i in ids))
            bus.servo(2).reboot()
            bus.servo(2)._poke("Torque_Enable", 1)
            check("safety.watchdog.fires", "rebooted, with its torque back, the fault clears and commands are taken again",
                  wait_until(lambda: read(bridge)[1].flags == 0 and read(bridge)[1].fault_code == 0, 0.5) and command(bridge, values, seq=5) == rhal.SKR_OK)
            # A servo that lets go with no error bit: a power dip, or a reboot a person did. Nothing in its register says so, and the arm is as limp.
            bus.servo(3)._poke("Torque_Enable", 0)
            check("safety.watchdog.fires", "a servo whose torque is off though nothing stopped it is a FAULT too, with a word that is its id and a bit no error register uses",
                  wait_until(lambda: read(bridge)[1].fault_code == (3 << 16 | 0x100), 0.5), hex(read(bridge)[1].fault_code))
            check("rhal.command.rejection", "and commands are refused until its torque is back", command(bridge, values, seq=6) == rhal.SKR_EPERM)
            bus.servo(3)._poke("Torque_Enable", 1)
            check("safety.watchdog.fires", "which lifts when it is", wait_until(lambda: read(bridge)[1].flags == 0, 0.5))

            check("rhal.abi.version", "closes, and LeRobot disconnects with torque off", bridge.close() == rhal.SKR_OK and all(bus.servo(i).peek("Torque_Enable") == 0 for i in ids))

            # A bus with a dead servo at the start is refused at open, with LeRobot's sentence, not raised.
            bus.silence(5)
            second = open_bridge(config)
            rc = second.open(config)
            check("rhal.abi.version", "an arm with a dead servo is refused at open as an I/O error, not raised", rc == rhal.SKR_EIO, str(rc))
            bus.silence(5, False)
        finally:
            bus.stop()


def omx() -> None:
    """LeRobot's own `OmxFollower`, opened as it ships, in LeRobot's default -100..100."""
    with tempfile.TemporaryDirectory() as directory:
        bus = DynamixelBus(OMX_IDS)
        path = bus.start()
        try:
            ids = list(OMX_IDS)
            for ident in ids:  # servos as they leave the factory: one turn of travel, no offset, torque off, the middle of the turn
                servo = bus.servo(ident)
                servo.actual = RESOLUTION / 2 + 100.0
                servo._sync_registers()
                servo.goal = servo.reported
                servo._poke("Goal_Position", int(servo.goal))
            config = json.dumps({"robot": {"type": "omx_follower", "port": path, "id": "emulated", "calibration_dir": directory}, "simulator": True})
            bridge = open_bridge(config)
            rc = bridge.open(config)
            check("rhal.abi.version", "opens LeRobot's own OpenMANIPULATOR-X class over the emulated bus with no calibration file", rc == rhal.SKR_OK, f"open returned {rc}")
            if rc != rhal.SKR_OK:
                return
            robot = bridge._robot
            check("rhal.abi.version", "the robot is LeRobot's `OmxFollower`, as shipped", type(robot).__module__ == "lerobot.robots.omx_follower.omx_follower", type(robot).__module__)
            written = eeprom_writes(bus)
            check("rhal.abi.version", "LeRobot found nothing to match and ran its own calibration, which wrote an offset, a minimum and a maximum to each of the six servos",
                  sorted(written) == sorted([HOMING_OFFSET, MAX_LIMIT, MIN_LIMIT] * 6), str(written))
            check("rhal.abi.version", "the factory values: no offset and the full turn of travel on every servo",
                  all((bus.servo(i).peek("Homing_Offset"), bus.servo(i).peek("Min_Position_Limit"), bus.servo(i).peek("Max_Position_Limit")) == (0, 0, 4095) for i in ids))
            saved = json.loads(Path(directory, "emulated.json").read_text(encoding="utf-8")) if Path(directory, "emulated.json").exists() else {}
            check("rhal.abi.version", "and LeRobot saved them, with the ids this class gives its motors (11 to 16)", [saved.get(m, {}).get("id") for m in MOTORS] == ids, str(saved))
            check("rhal.abi.version", "extended position on the five joints and current-based position on the gripper", [bus.servo(i).peek("Operating_Mode") for i in ids] == [4, 4, 4, 4, 4, 5])
            check("rhal.abi.version", "torque is on and the return delay is 0 on every servo", all(bus.servo(i).peek("Torque_Enable") == 1 and bus.servo(i).peek("Return_Delay_Time") == 0 for i in ids))

            descriptor = json.loads(bridge.describe())
            check("embodiment.roles.coverage", "the six motors carry the roles the OpenMANIPULATOR-X table maps", all(j["role"] for j in descriptor["joints"]) and descriptor["joints"][4]["role"] == "right_arm.wrist.roll",
                  str([j["role"] for j in descriptor["joints"]]))
            units = descriptor["provenance"]["units"]
            check("embodiment.descriptor.valid", "with `use_degrees` off the descriptor says every joint is passed through, normalised, and converts none from degrees",
                  "converted from deg" not in units and units.count("passed through unconverted") >= 6, units)
            rc, state = read(bridge)
            expected = [normalise("m100", bus.servo(i).reported, 0, 4095) for i in ids[:5]]
            check("rhal.state.completeness", "so the arm reads in LeRobot's normalised units, not in radians",
                  rc == rhal.SKR_OK and all(abs(state.position_rad[i] - expected[i]) < 0.1 for i in range(5)), f"read {[round(v, 2) for v in state.position_rad[:5]]}, expected {[round(v, 2) for v in expected]}")
            check("rhal.state.completeness", "and the gripper in its percentage", abs(state.position_rad[5] - normalise("pct", bus.servo(16).reported, 0, 4095)) < 0.1, str(state.position_rad[5]))

            before = len(bus.packets)
            values = list(state.position_rad)
            values[1] = 30.0
            rc = command(bridge, values)
            wait_until(lambda: any(inst == INST_SYNC_WRITE for _, inst, _ in bus.packets[before:]), timeout=1.0)
            check("rhal.command.rejection", "a position command is accepted and is one SYNC_WRITE for all six", rc == rhal.SKR_OK and sum(1 for _, inst, _ in bus.packets[before:] if inst == INST_SYNC_WRITE) == 1, str(rc))
            wanted = unnormalise("m100", 30.0, 0, 4095)
            check("rhal.command.rejection", "the servo's goal is the step LeRobot's -100..100 normalisation says (±1)", abs(bus.servo(12).peek("Goal_Position") - wanted) <= 1, f"goal {bus.servo(12).peek('Goal_Position')}, wanted {wanted}")
            check("rhal.command.rejection", "the arm follows it, and the bridge reads the move back as the same value",
                  wait_until(lambda: abs(read(bridge)[1].position_rad[1] - 30.0) < 0.3, 8.0), f"reads {read(bridge)[1].position_rad[1]:.2f}")

            rc = bridge.emergency_stop()
            check("safety.estop.latency", "emergency_stop takes torque off all six, and release gives it back",
                  rc == rhal.SKR_OK and all(bus.servo(i).peek("Torque_Enable") == 0 for i in ids) and bridge.release_stop() == rhal.SKR_OK and all(bus.servo(i).peek("Torque_Enable") == 1 for i in ids))
            check("rhal.abi.version", "closes, and LeRobot disconnects with torque off", bridge.close() == rhal.SKR_OK and all(bus.servo(i).peek("Torque_Enable") == 0 for i in ids))

            # The same arm opened again now finds a calibration file that matches its servos, and writes nothing.
            again_start = len(bus.packets)
            bridge = open_bridge(config)
            rc = bridge.open(config)
            check("rhal.abi.version", "opened again, the arm finds its saved calibration matching and rewrites none of it", rc == rhal.SKR_OK and eeprom_writes(bus, again_start) == [], str(eeprom_writes(bus, again_start)))
            if rc == rhal.SKR_OK:
                bridge.close()
        finally:
            bus.stop()


FOOTER = (
    "Rung 4: LeRobot's own Koch and OpenMANIPULATOR-X classes and ROBOTIS's own SDK, against a counterpart bus written here. It shows the bridge drives the library as "
    "the library is meant to be driven; it does not show a servo moves."
)


def main() -> int:
    roles_cover_every_lerobot_robot()
    check("embodiment.roles.coverage", "the Koch and the OpenMANIPULATOR-X each have a table", "koch_follower" in ROLE_TABLES and "omx_follower" in ROLE_TABLES)
    koch()
    omx()
    return finish(FOOTER)


if __name__ == "__main__":
    sys.exit(main())
