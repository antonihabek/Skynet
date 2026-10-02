"""The LeRobot bridge over LeRobot's own SO-101 class, on an emulated Feetech bus — items 12.D (lerobot) and 12.E.5.

`selftest.py` runs the bridge against `skynet_lerobot.fake`, "a faithful fake" written from LeRobot's documentation, and says in its
last line that it "has not been run against LeRobot itself". This is that run. The robot is `lerobot.robots.so_follower.SO101Follower`
as shipped, constructed the way production constructs it (`open_bridge`: a JSON configuration, LeRobot's own config registry,
`make_robot_from_config`), talking through LeRobot's own `FeetechMotorsBus` and Feetech's own `scservo_sdk` to a pseudo-terminal that
`sim/feetech_bus.py` answers as a bus of six STS3215 servos. Nothing on LeRobot's side is patched or imported for a stub.

    ~/lerobot-venv/bin/python drivers/lerobot/selftest_wire.py

# What it can and cannot show

It shows that the bridge drives the library the library's way: the handshake, the calibration an arm already has, the sync reads and
writes, the torque enable and the EEPROM lock all reach the wire as bytes that Feetech's SDK frames and checksums, and a command
arrives on the servo as the raw step LeRobot's own normalisation says it should. It shows what the bridge does when a servo stops
answering or a reply is corrupt. It is **rung 4** (`docs/robot-support.md` §3): the servo is a counterpart written here from the maker's
control table, not Feetech's firmware, so a pass is not evidence the arm moves, and no number here is a servo's measurement.

# What running the real class found

* **F219** — the real class names itself `so_follower`; the role table was keyed on `so101_follower` (the fake's name), so LeRobot's
  own SO-101 got no roles at all and could not take part in any cross-body remapping. A check below reads every robot class installed
  LeRobot ships and fails for one that is neither in the role table nor on the list of those deliberately without one.
* **F220** — one `unit` for the whole body converted the gripper too, which LeRobot reports in percent of its travel
  (`MotorNormMode.RANGE_0_100`), not degrees. Per-motor `units` exist now; the gripper goes through unconverted and is declared so.
"""

from __future__ import annotations

import json
import math
import os
import re
import sys
import tempfile
import time
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "bindings", "python"))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "sim"))

try:
    import lerobot  # noqa: F401
    import scservo_sdk  # noqa: F401
except ImportError as error:  # a refusal, not a pass: the claim of this file needs the library
    print(f"selftest_wire: {error}. This needs LeRobot and Feetech's SDK (pip install lerobot feetech-servo-sdk), e.g. ~/lerobot-venv.", file=sys.stderr)
    sys.exit(2)

import skynet_rhal as rhal  # noqa: E402
from skynet_rhal import Command, State  # noqa: E402
from skynet_lerobot.bridge import open_bridge  # noqa: E402
from skynet_lerobot.roles import ROLE_TABLES  # noqa: E402
from feetech_bus import FeetechBus  # noqa: E402

_PASSED = 0
_FAILED: list[str] = []

MOTORS = ["shoulder_pan", "shoulder_lift", "elbow_flex", "wrist_flex", "wrist_roll", "gripper"]
#: What the arm was calibrated to: a travel per motor and a homing offset, with both signs of offset so the sign-magnitude path is on the wire.
CALIBRATION = {
    "shoulder_pan": (0, 600, 3500),
    "shoulder_lift": (35, 700, 3400),
    "elbow_flex": (-42, 650, 3300),
    "wrist_flex": (100, 800, 3300),
    "wrist_roll": (-17, 0, 4095),
    "gripper": (0, 1900, 3300),
}
RESOLUTION = 4096

#: LeRobot robot classes the bridge has no role table for, by decision rather than by oversight (F219's guard). A class LeRobot adds that is in
#: neither this list nor `ROLE_TABLES` fails `roles_cover_every_lerobot_robot`.
NO_TABLE_BY_DECISION = {
    "bi_so_follower": "two arms on one bus object; the motor names are prefixed by side and there is no table that is not a guess about which is which",
    "bi_openarm_follower": "bimanual, as above",
    "bi_rebot_b601_follower": "bimanual, as above",
    "omx_follower": "no role table has been written for the OpenMANIPULATOR-X",
    "openarm_follower": "no role table has been written",
    "rebot_b601_follower": "no role table has been written",
    "earthrover_mini_plus": "a wheeled base with no arm; nothing in §5.1's chain vocabulary to say",
    "unitree_g1": "driven through drivers/unitree, not through this bridge",
    "hope_jr_hand": "a dexterous hand; no finger table has been written",
}


def check(scts_id: str, what: str, held: bool, detail: str = "") -> None:
    global _PASSED
    if held:
        _PASSED += 1
        print(f"ok   {scts_id:<28} {what}")
    else:
        _FAILED.append(f"{scts_id}: {what}")
        print(f"FAIL {scts_id:<28} {what}")
        if detail:
            print(f"     {detail}")


def wait_until(predicate, timeout: float = 4.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.02)
    return predicate()


def wire_arm(directory: str) -> tuple[FeetechBus, str, str]:
    """An arm that has been calibrated: servos holding their offsets and limits, and LeRobot's calibration file saying the same."""
    bus = FeetechBus()
    path = bus.start()
    calibration = {}
    for index, motor in enumerate(MOTORS):
        offset, low, high = CALIBRATION[motor]
        ident = index + 1
        bus.calibrate(ident, homing_offset=offset, range_min=low, range_max=high)
        servo = bus.servo(ident)
        servo.actual = (low + high) / 2 + offset  # at the middle of its travel
        servo.goal = servo.reported
        servo._poke_int("Goal_Position", int(servo.goal))
        calibration[motor] = {"id": ident, "drive_mode": 0, "homing_offset": offset, "range_min": low, "range_max": high}
    Path(directory, "emulated.json").write_text(json.dumps(calibration), encoding="utf-8")
    config = json.dumps({"robot": {"type": "so101_follower", "port": path, "id": "emulated", "calibration_dir": directory}, "simulator": True})
    return bus, path, config


def degrees_to_raw(motor: str, degrees: float) -> int:
    """LeRobot's `MotorNormMode.DEGREES`, from `motors_bus._unnormalize`: raw = degrees × (resolution − 1) / 360 + the middle of the travel."""
    _, low, high = CALIBRATION[motor]
    return int(round(degrees * (RESOLUTION - 1) / 360.0 + (low + high) / 2))


def raw_to_degrees(motor: str, raw: float) -> float:
    _, low, high = CALIBRATION[motor]
    return (raw - (low + high) / 2) * 360.0 / (RESOLUTION - 1)


def percent(motor: str, raw: float) -> float:
    """`MotorNormMode.RANGE_0_100`: the position as a percentage of the calibrated travel."""
    _, low, high = CALIBRATION[motor]
    return (raw - low) / (high - low) * 100.0


def command(bridge, values, mask=None, seq=1) -> int:
    mask = mask if mask is not None else [1] * len(values)
    return bridge.write_command(Command(stamp_ns=1, seq=seq, mode=rhal.MODE_POSITION, values=list(values), mask=list(mask)), 0)


def read(bridge) -> tuple[int, State]:
    state = State()
    return bridge.read_state(state, 0), state


def roles_cover_every_lerobot_robot() -> None:
    """F219's guard: every robot class installed LeRobot ships either has a role table or is on the list of those that deliberately do not."""
    package = Path(lerobot.__file__).parent / "robots"
    names = {}
    for source in package.rglob("*.py"):
        for match in re.finditer(r'^\s+name = "([a-z0-9_]+)"\s*$', source.read_text(encoding="utf-8"), re.M):
            names[match.group(1)] = source.relative_to(package).as_posix()
    check("embodiment.roles.coverage", f"found the {len(names)} robot class names this LeRobot ships", len(names) >= 10, str(sorted(names)))
    undecided = sorted(name for name in names if name not in ROLE_TABLES and name not in NO_TABLE_BY_DECISION)
    check(
        "embodiment.roles.coverage",
        "every one has a role table or is listed as deliberately without one",
        not undecided,
        f"undecided: {undecided} — add a table to skynet_lerobot/roles.py, or a reason to NO_TABLE_BY_DECISION here",
    )
    stale = sorted(name for name in NO_TABLE_BY_DECISION if name not in names)
    check("embodiment.roles.coverage", "and the list of exceptions names no class this LeRobot does not have", not stale, str(stale))
    check("embodiment.roles.coverage", "the class the real SO-101 reports, `so_follower`, has the SO table", ROLE_TABLES.get("so_follower") is ROLE_TABLES["so101_follower"])


def main() -> int:
    roles_cover_every_lerobot_robot()

    with tempfile.TemporaryDirectory() as directory:
        bus, path, config = wire_arm(directory)
        try:
            bridge = open_bridge(config)
            rc = bridge.open(config)
            check("rhal.abi.version", "opens LeRobot's own SO-101 class over the emulated bus, with the calibration an arm already has", rc == rhal.SKR_OK, f"open returned {rc}")
            if rc != rhal.SKR_OK:
                return finish()
            robot = bridge._robot
            check("rhal.abi.version", "the robot is LeRobot's `SOFollower`, as shipped", type(robot).__module__ == "lerobot.robots.so_follower.so_follower", type(robot).__module__)
            check("rhal.abi.version", "LeRobot found the arm calibrated and did not rewrite its calibration", not any(inst == 0x03 and params[0] in (9, 11, 31) for _, inst, params in bus.packets))
            check("rhal.abi.version", "connect configured the motors: return delay 0, acceleration 254, Phase bit 4 cleared, on every servo",
                  all(bus.servo(i).peek("Return_Delay_Time") == 0 and bus.servo(i).peek("Acceleration") == 254 and bus.servo(i).peek("Phase") & 0x10 == 0 for i in range(1, 7)))
            check("rhal.abi.version", "and left torque on and the EEPROM locked, as `enable_torque` does", all(bus.servo(i).peek("Torque_Enable") == 1 and bus.servo(i).peek("Lock") == 1 for i in range(1, 7)))

            descriptor = json.loads(bridge.describe())
            check("embodiment.descriptor.valid", "the joints are LeRobot's motors, in LeRobot's order", [j["name"] for j in descriptor["joints"]] == MOTORS, str([j["name"] for j in descriptor["joints"]]))
            check("embodiment.roles.coverage", "all six carry the roles §67.2 maps (F219: the real class is `so_follower`)", all(j["role"] for j in descriptor["joints"]) and descriptor["joints"][2]["role"] == "right_arm.elbow.flex",
                  str([j["role"] for j in descriptor["joints"]]))
            check("embodiment.descriptor.valid", "it declares itself a simulator, with no envelope and no ratings it was not given", descriptor["simulator"] is True and descriptor["envelope"] is None and all(j["limits"] == {} for j in descriptor["joints"]))
            units = descriptor["provenance"]["units"]
            check("embodiment.descriptor.valid", "and says the gripper is passed through in percent while the arm is converted from degrees (F220)", "gripper: percent, passed through unconverted" in units and "converted from deg" in units, units)

            rc, state = read(bridge)
            check("rhal.state.completeness", "read_state succeeds over the wire", rc == rhal.SKR_OK)
            expected = [math.radians(raw_to_degrees(m, bus.servo(i + 1).reported)) for i, m in enumerate(MOTORS[:5])]
            check("rhal.state.completeness", "each arm joint reads the shaft as LeRobot's degrees, in radians, to a step",
                  all(abs(state.position_rad[i] - expected[i]) < math.radians(0.2) for i in range(5)),
                  f"read {[round(v, 4) for v in state.position_rad[:5]]}, expected {[round(v, 4) for v in expected]}")
            check("rhal.state.completeness", "the gripper reads as a percentage of its travel, unconverted", abs(state.position_rad[5] - percent("gripper", bus.servo(6).reported)) < 0.2, str(state.position_rad[5]))
            check("rhal.state.completeness", "an unreported velocity and effort are NaN", all(math.isnan(v) for v in state.velocity_rad_s) and all(math.isnan(v) for v in state.effort_nm))

            # A command lands on the servo as the raw step LeRobot's own normalisation says, in one SYNC_WRITE for all six.
            before = len(bus.packets)
            target_rad = 0.4
            values = list(state.position_rad)
            values[0] = target_rad
            rc = command(bridge, values)
            check("rhal.command.rejection", "a position command is accepted", rc == rhal.SKR_OK, str(rc))
            # The bus reads the pty on its own thread, so the frame is on the wire when `send_action` returns and in `packets` a moment after.
            wait_until(lambda: any(inst == 0x83 for _, inst, _ in bus.packets[before:]), timeout=1.0)
            writes = [(inst, params) for _, inst, params in bus.packets[before:] if inst == 0x83]
            check("rhal.command.rejection", "it reaches the wire as one SYNC_WRITE to Goal_Position for all six servos", len(writes) == 1 and writes[0][1][0] == 42 and writes[0][1][1] == 2 and (len(writes[0][1]) - 2) // 3 == 6,
                  str(writes[0][1][:4]) if writes else "no SYNC_WRITE")
            goal_raw = bus.servo(1).peek("Goal_Position")
            wanted = degrees_to_raw("shoulder_pan", math.degrees(target_rad))
            check("rhal.command.rejection", "and the servo's goal is the step LeRobot's degrees-to-raw says (±1)", abs(goal_raw - wanted) <= 1, f"goal {goal_raw}, wanted {wanted}")
            check("rhal.command.rejection", "the arm follows it, and the bridge reads the move back as the same angle",
                  wait_until(lambda: abs(read(bridge)[1].position_rad[0] - target_rad) < math.radians(0.5)), f"reads {read(bridge)[1].position_rad[0]:.4f} rad")

            # §24: a masked joint holds where it was last told to be.
            held = bus.servo(2).peek("Goal_Position")
            values[0] = 0.0
            rc = command(bridge, values, mask=[1, 0, 1, 1, 1, 1], seq=2)
            check("safety.watchdog.fires", "a masked joint is held at the setpoint it was last given, not at its reading and not at the command's value",
                  rc == rhal.SKR_OK and abs(bus.servo(2).peek("Goal_Position") - held) <= 1, f"goal {bus.servo(2).peek('Goal_Position')}, was {held}")

            # The maker's own retry covers one corrupt reply; a servo that stops answering is an I/O error, not a stale number.
            bus.corrupt(1)
            check("rhal.state.completeness", "one corrupt reply is retried by LeRobot's `num_read_retries` and the read still succeeds", read(bridge)[0] == rhal.SKR_OK)
            bus.silence(4)
            rc, _ = read(bridge)
            check("rhal.state.completeness", "a servo that stops answering is SKR_EIO, with no exception out of the driver", rc == rhal.SKR_EIO, str(rc))
            bus.silence(4, False)
            check("rhal.state.completeness", "and the next read succeeds once it answers again", wait_until(lambda: read(bridge)[0] == rhal.SKR_OK))

            # The stop: torque off on every servo of the bus, the latch visible in the state, commands refused; the release returns torque.
            rc = bridge.emergency_stop()
            check("safety.estop.latency", "emergency_stop is acknowledged", rc == rhal.SKR_OK)
            check("safety.estop.latency", "and torque is off on all six servos of the bus", all(bus.servo(i).peek("Torque_Enable") == 0 for i in range(1, 7)), str([bus.servo(i).peek("Torque_Enable") for i in range(1, 7)]))
            rc, stopped = read(bridge)
            check("safety.estop.latency", "the latch shows in the next state", rc == rhal.SKR_OK and bool(stopped.flags & rhal.FLAG_ESTOP))
            check("safety.estop.latency", "and every later command is refused", command(bridge, values, seq=3) == rhal.SKR_EPERM)
            pushed = bus.servo(1).reported
            bus.push(1, 100)
            check("safety.estop.latency", "a servo with torque off can be moved by hand, and the bridge reads where it was moved to",
                  abs(read(bridge)[1].position_rad[0] - math.radians(raw_to_degrees("shoulder_pan", pushed + 100))) < math.radians(0.3))
            rc = bridge.release_stop()
            check("safety.estop.latency", "release returns torque to all six and clears the latch", rc == rhal.SKR_OK and all(bus.servo(i).peek("Torque_Enable") == 1 for i in range(1, 7)))

            check("rhal.abi.version", "closes, and LeRobot disconnects with torque off", bridge.close() == rhal.SKR_OK and all(bus.servo(i).peek("Torque_Enable") == 0 for i in range(1, 7)))

            # A bus with a dead servo at the start is refused at open, with LeRobot's sentence, not raised.
            bus.silence(5)
            second = open_bridge(config)
            rc = second.open(config)
            check("rhal.abi.version", "an arm with a dead servo is refused at open as an I/O error, not raised", rc == rhal.SKR_EIO, str(rc))
            bus.silence(5, False)

            # The same arm configured the other way: `use_degrees` off puts the arm joints in LeRobot's -100..100 normalisation, and the SO class's
            # degrees default must yield to what the robot says it was configured as (F220's other half). Nothing may be converted to radians.
            normalised = json.dumps({"robot": {"type": "so101_follower", "port": path, "id": "emulated", "calibration_dir": directory, "use_degrees": False}, "simulator": True})
            third = open_bridge(normalised)
            rc = third.open(normalised)
            check("rhal.abi.version", "the same arm opens with use_degrees off", rc == rhal.SKR_OK, str(rc))
            if rc == rhal.SKR_OK:
                declared = json.loads(third.describe())["provenance"]["units"]
                check("embodiment.descriptor.valid", "and the descriptor says every joint is passed through, normalised, and converts none from degrees", "converted from deg" not in declared and declared.count("passed through unconverted") >= 6, declared)
                rc, state = read(third)
                low_high = {m: (CALIBRATION[m][1], CALIBRATION[m][2]) for m in MOTORS}
                expected = [(bus.servo(i + 1).reported - low_high[m][0]) / (low_high[m][1] - low_high[m][0]) * 200.0 - 100.0 for i, m in enumerate(MOTORS[:5])]
                check("rhal.state.completeness", "so the arm reads in LeRobot's normalised units, not in radians",
                      rc == rhal.SKR_OK and all(abs(state.position_rad[i] - expected[i]) < 0.5 for i in range(5)),
                      f"read {[round(v, 2) for v in state.position_rad[:5]]}, expected {[round(v, 2) for v in expected]}")
                third.close()
        finally:
            bus.stop()
    return finish()


def finish() -> int:
    print(f"\n{_PASSED + len(_FAILED)} checks, {len(_FAILED)} failed")
    for failure in _FAILED:
        print(f"  FAIL {failure}")
    print(
        "\nRung 4: LeRobot's own SO-101 class and Feetech's own SDK, against a counterpart bus written here. It shows the bridge drives the library as the "
        "library is meant to be driven; it does not show a servo moves."
    )
    return 1 if _FAILED else 0


if __name__ == "__main__":
    sys.exit(main())
