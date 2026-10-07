"""The Inspire bridge's subject for `tools/faultproxy/chaos.py`: the hand's own line, a serial port, through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/inspire python3 tools/faultproxy/chaos.py drivers/inspire/chaos.py

The substrate is `SimulatedHand`, the one the suite uses, served on the substrate's end of two pseudo-terminals with the relay between them: private-protocol frames, a hand whose fingers move at the speed they are
told and stop at an object. The driver is opened on the other pseudo-terminal, which is what a USB RS-485 adapter's cable is to it. **The simulator agrees with whoever wrote it** (F15): what this shows is the driver's own
transaction budget, reading cycle, staleness bound and stop path, and nothing about the maker's firmware.

The serial profiles are the ones that apply: a delay or jitter of a bus's size, a slow link, a stall, a blackhole, bursts of lost bytes, and a truncated or corrupted frame, which on a serial line (unlike TCP) are
real. `reset` is not here, because a serial line cannot be reset by its far end. Nothing in the driver may wait on any of them for longer than its budgets.
"""

from __future__ import annotations

import json
import math

import skynet_rhal as rhal

from skynet_inspire import bridge as bridge_module, frames
from skynet_inspire.bridge import InspireBridge
from skynet_inspire.hand_sim import SimulatedHand

PROFILE = frames.PROFILES["RH56"]
#: Two far-apart poses of all six joints, as registers: the fingers are always on their way from one to the other, so a late reading is a wrong one.
POSES = ([200] * 6, [800] * 6)
SPEED = 0.5


class Subject:
    name = "inspire (the hand's serial line)"
    profiles = ["delay-2ms", "jitter-2ms", "slow-link", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "truncate-5pct", "corrupt-5pct"]
    flows = [("serial", None)]

    def __init__(self) -> None:
        self.hand: SimulatedHand | None = None
        self.bridge = InspireBridge()
        # The fastest joint's rate in rad/s (the fingers': 157.7 degrees in 0.8 s at full speed, at the operator's fraction of it): the lag the check turns into an age is the largest joint's.
        self.speed = SPEED * max(math.radians(abs(high - low)) for low, high in PROFILE.angle_deg) / PROFILE.full_range_s
        self.stale_s = bridge_module._STALE_S

    def start(self) -> None:
        self.hand = SimulatedHand(time_scale=1.0).start()
        self.bridge = InspireBridge()

    def attach(self, addresses) -> None:
        assert self.hand is not None
        self.hand.attach(addresses[0][1])

    def stop(self) -> None:
        if self.hand is not None:
            self.hand.stop()
            self.hand = None

    def config(self, addresses) -> str:
        return json.dumps({"device": addresses[0][0], "protocol": "private", "model": "RH56", "stop_mode": "hold", "speed": SPEED, "force_g": 300, "simulator": True, "rate_hz": 50.0, "open_timeout_ms": 3000})

    def open(self, config: str) -> int:
        return self.bridge.open(config)

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad)

    def target(self, index: int):
        return [frames.register_to_angle(r, j, PROFILE) for j, r in enumerate(POSES[index & 1])]

    def write(self, values) -> int:
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(values), mask=[1] * 6)
        return self.bridge.write_command(command, 0)

    def stop_call(self) -> int:
        return self.bridge.emergency_stop()

    def release(self) -> None:
        self.bridge.release_stop()

    def close(self) -> None:
        self.bridge.close()

    def truth(self):
        if self.hand is None:
            return [0.0] * 6
        return [frames.register_to_angle(a, j, PROFILE) for j, a in enumerate(self.hand.angles)]


def subject() -> Subject:
    return Subject()
