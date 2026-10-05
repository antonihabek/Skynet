"""The LeRobot bridge's subject for `tools/fault-vocab/contract.py`: what the bridge makes of a servo's own error bits.

    PYTHONPATH=bindings/python:drivers/lerobot <venv>/bin/python tools/fault-vocab/contract.py drivers/lerobot/faultvocab.py

Two vocabularies, one per servo family LeRobot drives: the Feetech `Status` register on an emulated STS3215 bus behind LeRobot's own
SO-101 class, and the Dynamixel `Hardware_Error_Status` register on an emulated X-series bus behind LeRobot's own Koch class. The
setups are the wire suites' own (`selftest_wire.py`, `selftest_wire_dynamixel.py`): the robots are LeRobot's classes as shipped, the
buses are the emulators, and the faults are staged with the emulators' `fault()` levers. **The simulators agree with whoever wrote
them** (F15): the bits' names are the makers' (Feetech's `scservo_sdk`, ROBOTIS's e-manual), and what this holds is the translation —
a servo in error reads FAULT and DEGRADED with the word `servo id << 16 | the servo's bits verbatim`, and commands are refused until
it clears — not what any maker's firmware does.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "bindings", "python"))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "sim"))

try:
    import lerobot  # noqa: F401
    import scservo_sdk  # noqa: F401
    import dynamixel_sdk  # noqa: F401
except ImportError as error:  # a refusal, not a pass: the claim of this file needs the libraries
    print(f"faultvocab: {error}. This needs lerobot[feetech,dynamixel], e.g. ~/lerobot-venv.", file=sys.stderr)
    sys.exit(2)

import skynet_rhal as rhal  # noqa: E402
from skynet_lerobot.bridge import open_bridge  # noqa: E402
from feetech_bus import FeetechBus  # noqa: E402
from dynamixel_bus import DynamixelBus  # noqa: E402

MOTORS = ["shoulder_pan", "shoulder_lift", "elbow_flex", "wrist_flex", "wrist_roll", "gripper"]
#: The wire suites' calibrations, copied so the arms open calibrated and LeRobot rewrites nothing.
FEETECH_CALIBRATION = {
    "shoulder_pan": (0, 600, 3500),
    "shoulder_lift": (35, 700, 3400),
    "elbow_flex": (-42, 650, 3300),
    "wrist_flex": (100, 800, 3300),
    "wrist_roll": (-17, 0, 4095),
    "gripper": (0, 1900, 3300),
}
KOCH_IDS = {1: "xl430-w250", 2: "xl430-w250", 3: "xl330-m288", 4: "xl330-m288", 5: "xl330-m288", 6: "xl330-m288"}
KOCH_CALIBRATION = {
    "shoulder_pan": (0, 0, 4095),
    "shoulder_lift": (35, 700, 3400),
    "elbow_flex": (-42, 650, 3300),
    "wrist_flex": (100, 800, 3300),
    "wrist_roll": (-17, 0, 4095),
    "gripper": (0, 1900, 3300),
}
#: The servo every code is injected on: the shoulder, id 1 on both buses.
FAULT_ID = 1


class Subject:
    driver = "lerobot"
    name = "lerobot (the bridge over LeRobot's own SO-101 and Koch classes on emulated Feetech and Dynamixel buses)"
    #: The health poll runs every 100 ms and a reading counts when two polls in a row agree (F271): a fault needs two
    #: periods to arrive and two to leave, so half a second each way with margin.
    settle_s = 0.5

    def __init__(self) -> None:
        self._arms: dict[str, tuple] = {}
        self._current = "feetech_status"
        self._dirs: list[tempfile.TemporaryDirectory] = []

    def start(self) -> None:
        self._dirs.append(tempfile.TemporaryDirectory())
        directory = self._dirs[-1].name
        febus = FeetechBus()
        fepath = febus.start()
        calibration = {}
        for index, motor in enumerate(MOTORS):
            offset, low, high = FEETECH_CALIBRATION[motor]
            ident = index + 1
            febus.calibrate(ident, homing_offset=offset, range_min=low, range_max=high)
            servo = febus.servo(ident)
            servo.actual = (low + high) / 2 + offset
            servo.goal = servo.reported
            servo._poke_int("Goal_Position", int(servo.goal))
            calibration[motor] = {"id": ident, "drive_mode": 0, "homing_offset": offset, "range_min": low, "range_max": high}
        Path(directory, "emulated.json").write_text(json.dumps(calibration), encoding="utf-8")
        feconfig = json.dumps({"robot": {"type": "so101_follower", "port": fepath, "id": "emulated", "calibration_dir": directory}, "simulator": True})
        febridge = open_bridge(feconfig)
        rc = febridge.open(feconfig)
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the SO-101 bridge did not open against the emulated Feetech bus (rc {rc})")

        self._dirs.append(tempfile.TemporaryDirectory())
        directory = self._dirs[-1].name
        dxbus = DynamixelBus(KOCH_IDS)
        dxpath = dxbus.start()
        calibration = {}
        for ident, motor in zip(KOCH_IDS, MOTORS, strict=True):
            offset, low, high = KOCH_CALIBRATION[motor]
            dxbus.calibrate(ident, homing_offset=offset, range_min=low, range_max=high)
            servo = dxbus.servo(ident)
            servo.actual = (low + high) / 2 - offset
            servo._sync_registers()
            servo.goal = servo.reported
            servo._poke("Goal_Position", int(servo.goal))
            calibration[motor] = {"id": ident, "drive_mode": 0, "homing_offset": offset, "range_min": low, "range_max": high}
        Path(directory, "emulated.json").write_text(json.dumps(calibration), encoding="utf-8")
        dxconfig = json.dumps({"robot": {"type": "koch_follower", "port": dxpath, "id": "emulated", "calibration_dir": directory, "use_degrees": True}, "simulator": True})
        dxbridge = open_bridge(dxconfig)
        rc = dxbridge.open(dxconfig)
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the Koch bridge did not open against the emulated Dynamixel bus (rc {rc})")

        self._arms = {"feetech_status": (febus, febridge), "dynamixel_hw_error": (dxbus, dxbridge)}

    def restart(self) -> None:
        self.stop()
        self.start()

    def stop(self) -> None:
        for _, bridge in self._arms.values():
            bridge.close()
        for bus, _ in self._arms.values():
            bus.stop()
        self._arms = {}
        for directory in self._dirs:
            directory.cleanup()
        self._dirs = []

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        if vocabulary == "feetech_status" and code & 0x80:
            return "Feetech's own scservo_sdk discards any status packet whose error byte exceeds 0x7F as a framing error (rxPacket: 'unavailable Error'), so a servo raising bit 7 never delivers a packet through this library and no subject on this bus can stage it"
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        self._current = vocabulary
        bus, _ = self._arms[vocabulary]
        bus.fault(FAULT_ID, code)

    def clear(self, vocabulary: str) -> None:
        """What a person at the arm does: the error gone and the torque back on. The Feetech lever assigns, so
        faulting zero clears; the Dynamixel one accumulates and shuts the torque off where the Shutdown register
        names the bit, so the register is zeroed and the torque re-enabled by hand — a servo the fault left limp
        would otherwise read torque-off-though-nothing-stopped-it (0x100) for ever."""
        bus, _ = self._arms[vocabulary]
        bus.fault(FAULT_ID, 0)
        if vocabulary == "dynamixel_hw_error":
            servo = bus.servo(FAULT_ID)
            servo.error_status = 0
            servo._poke("Hardware_Error_Status", 0)
            servo._poke("Torque_Enable", 1)

    def expected_code(self, vocabulary: str, code: int) -> int:
        """The servo's id in the high half and its error bits verbatim in the low one: the black box keeps both."""
        return FAULT_ID << 16 | code

    def read(self) -> tuple[int, int, int]:
        _, bridge = self._arms[self._current]
        state = rhal.State()
        rc = bridge.read_state(state, 0)
        return rc, state.flags, state.fault_code

    def write(self) -> int:
        _, bridge = self._arms[self._current]
        state = rhal.State()
        bridge.read_state(state, 0)
        return bridge.write_command(
            rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(state.position_rad), mask=[1] * len(state.position_rad)), 0)


def subject() -> Subject:
    return Subject()
