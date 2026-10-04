"""The Robotiq bridge's subject for `tools/faultproxy/chaos.py`: the gripper's own line, a serial port, through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/robotiq python3 tools/faultproxy/chaos.py drivers/robotiq/chaos.py

The substrate is `SimulatedGripper`, the one the suite uses, served on the substrate's end of two pseudo-terminals with the relay between them: Modbus RTU frames, a gripper that activates and moves at the speed it is
told, and the one-second no-communication fault. The driver is opened on the other pseudo-terminal, which is what a USB RS-485 adapter's cable is to it. **The simulator agrees with whoever wrote it** (F15): what this
shows is the driver's own transaction budget, exchange cycle, staleness bound and stop path, and nothing about the maker's firmware or its fake.

The serial profiles are the ones that apply: a delay or jitter of a bus's size, a slow link, a stall, a blackhole, bursts of lost bytes, and a truncated or corrupted frame, which on a serial line (unlike TCP) are
real. `reset` is not here, because a serial line cannot be reset by its far end. Nothing in the driver may wait on any of them for longer than its budgets.
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_robotiq import bridge as bridge_module, frames
from skynet_robotiq.bridge import RobotiqBridge
from skynet_robotiq.gripper_sim import SimulatedGripper

#: Two far-apart openings, each inside the stroke (metres): the fingers are always on their way from one to the other, so a late reading is a wrong one.
POSES = ([0.01], [0.08])
SPEED = 0.1
PROFILE = frames.PROFILES["2F-85"]


class Subject:
    name = "robotiq (the gripper's serial line)"
    profiles = ["delay-2ms", "jitter-2ms", "slow-link", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "truncate-5pct", "corrupt-5pct"]
    flows = [("serial", None)]

    def __init__(self) -> None:
        self.gripper: SimulatedGripper | None = None
        self.bridge = RobotiqBridge()
        self.speed = SPEED
        self.stale_s = bridge_module._STALE_S

    def start(self) -> None:
        self.gripper = SimulatedGripper(time_scale=1.0, activated=True).start()
        self.bridge = RobotiqBridge()

    def attach(self, addresses) -> None:
        assert self.gripper is not None
        self.gripper.attach(addresses[0][1])

    def stop(self) -> None:
        if self.gripper is not None:
            self.gripper.stop()
            self.gripper = None

    def config(self, addresses) -> str:
        return json.dumps({"device": addresses[0][0], "model": "2F-85", "stop_mode": "halt", "speed": SPEED, "effort": 0.5, "simulator": True, "rate_hz": 50.0, "open_timeout_ms": 3000})

    def open(self, config: str) -> int:
        return self.bridge.open(config)

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad)

    def target(self, index: int):
        return POSES[index & 1]

    def write(self, values) -> int:
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(values), mask=[1])
        return self.bridge.write_command(command, 0)

    def stop_call(self) -> int:
        return self.bridge.emergency_stop()

    def release(self) -> None:
        self.bridge.release_stop()

    def close(self) -> None:
        self.bridge.close()

    def truth(self):
        if self.gripper is None:
            return [0.0]
        return [frames.opening_from_register(self.gripper.position, PROFILE) or 0.0]


def subject() -> Subject:
    return Subject()
