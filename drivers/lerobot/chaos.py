"""The LeRobot bridge's subject for `tools/faultproxy/chaos.py`: LeRobot's own SO-101 class on an emulated Feetech bus, the serial line between them through a fault-injecting relay.

    ~/lerobot-venv/bin/python is an interpreter with `lerobot[feetech]`; then, from the repository root,

    sh tools/seed-defects/in-netns.sh env PYTHONPATH=bindings/python:drivers/lerobot ~/lerobot-venv/bin/python tools/faultproxy/chaos.py drivers/lerobot/chaos.py

The robot is `lerobot.robots.so_follower.SO101Follower`, as shipped, opened the way production opens it (`open_bridge`), talking through LeRobot's own `FeetechMotorsBus` and Feetech's own `scservo_sdk` to the near end of a
pseudo-terminal pair; the relay is between that and the far end, where `sim/feetech_bus.py` answers as six STS3215 servos. **The servos agree with whoever wrote them** (F15): what this shows is the bridge's own
bounds, staleness and stop path against a line that delays, stalls, loses and corrupts bytes, and nothing about a servo or about Feetech's firmware.
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

import skynet_rhal as rhal  # noqa: E402
from skynet_rhal import Command, State  # noqa: E402
from skynet_lerobot.bridge import open_bridge  # noqa: E402
from feetech_bus import FeetechBus  # noqa: E402

import selftest_wire as wire  # noqa: E402

#: Two far-apart poses: five arm joints in radians (LeRobot's degrees, converted) and the gripper in percent of its travel, which the bridge passes through.
POSES = ([0.4, 0.3, -0.3, 0.2, 0.0, 30.0], [-0.4, -0.3, 0.3, -0.2, 0.0, 60.0])


class Subject:
    name = "lerobot (an SO-101 on an emulated Feetech bus, its serial line through a relay)"
    profiles = ["delay-2ms", "jitter-2ms", "slow-link", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "truncate-5pct", "corrupt-5pct"]
    flows = [("serial", None)]
    #: A sync read is a request and six replies: seven messages that must all arrive. On a line that loses one message in ten that is 0.9^7, about 48% of reads whole, and the SDK's retry does not make the loss
    #: independent of the stall that follows it, so `min_ok_fraction` is what the arithmetic allows (measured 42% and 64% over two runs) and not the 70% a one-message exchange can meet.
    expect = {"loss-10pct": {"min_ok_fraction": 0.3}}
    speed = 0.0  # the servos are physics threads in this process, but their units are LeRobot's: no age to measure
    stale_s = 0.5

    def __init__(self) -> None:
        self.bus: FeetechBus | None = None
        self.bridge = None
        self.directory: tempfile.TemporaryDirectory | None = None

    def start(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.bus = FeetechBus()
        calibration = {}
        for index, motor in enumerate(wire.MOTORS):
            offset, low, high = wire.CALIBRATION[motor]
            ident = index + 1
            self.bus.calibrate(ident, homing_offset=offset, range_min=low, range_max=high)
            servo = self.bus.servo(ident)
            servo.actual = (low + high) / 2 + offset
            servo.goal = servo.reported
            servo._poke_int("Goal_Position", int(servo.goal))  # noqa: SLF001
            calibration[motor] = {"id": ident, "drive_mode": 0, "homing_offset": offset, "range_min": low, "range_max": high}
        Path(self.directory.name, "emulated.json").write_text(json.dumps(calibration), encoding="utf-8")

    def attach(self, addresses) -> None:
        assert self.bus is not None
        self.bus.attach(addresses[0][1])

    def stop(self) -> None:
        if self.bus is not None:
            self.bus.stop()
            self.bus = None
        if self.directory is not None:
            self.directory.cleanup()
            self.directory = None

    def config(self, addresses) -> str:
        assert self.directory is not None
        return json.dumps({"robot": {"type": "so101_follower", "port": addresses[0][0], "id": "emulated", "calibration_dir": self.directory.name}, "simulator": True})

    def open(self, config: str) -> int:
        self.bridge = open_bridge(config)
        return self.bridge.open(config)

    def read(self):
        state = State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad), state.seq

    def target(self, index: int):
        return POSES[index & 1]

    def write(self, values) -> int:
        return self.bridge.write_command(Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(values), mask=[1] * len(values)), 0)

    def stop_call(self) -> int:
        return self.bridge.emergency_stop()

    def release(self) -> None:
        self.bridge.release_stop()

    def close(self) -> None:
        if self.bridge is not None:
            self.bridge.close()

    def truth(self):
        return list(POSES[0])


def subject() -> Subject:
    return Subject()
