"""The LEAP bridge's subject for `tools/faultproxy/chaos.py`: the hand's own line, a serial port, through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/leap python3 tools/faultproxy/chaos.py drivers/leap/chaos.py

The substrate is `SimulatedHand`, the one the suite uses, served on the substrate's end of two pseudo-terminals with the relay between them: protocol 2.0 packets from sixteen servos that move at the speed the
simulation gives them and stop where they are told. The driver is opened on the other pseudo-terminal, which is what a USB adapter's cable is to it. **The simulator agrees with whoever wrote it** (F15): what this
shows is the driver's own transaction budget, reading cycle, staleness bound and stop path, and nothing about the servos' firmware.

The serial profiles are the ones that apply: a delay or jitter of a bus's size, a slow link, a stall, a blackhole, bursts of lost bytes, and a truncated or corrupted packet, which on a serial line (unlike a
stream socket) is real. `reset` is not here, because a serial line cannot be reset by its far end. Nothing in the driver may wait on any of them for longer than its budgets.
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_leap import bridge as bridge_module, frames, hand_sim
from skynet_leap.bridge import LeapBridge
from skynet_leap.hand_sim import SimulatedHand

#: Two far-apart poses of all sixteen joints (a fifth and four fifths of each range): the servos are always on their way from one to the other, so a late reading is a wrong one.
POSES = ([lo + 0.2 * (hi - lo) for lo, hi in zip(frames.LIMITS_MIN, frames.LIMITS_MAX)], [lo + 0.8 * (hi - lo) for lo, hi in zip(frames.LIMITS_MIN, frames.LIMITS_MAX)])


class Subject:
    name = "leap (the hand's serial line)"
    profiles = ["delay-2ms", "jitter-2ms", "slow-link", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "truncate-5pct", "corrupt-5pct"]
    flows = [("serial", None)]

    def __init__(self) -> None:
        self.hand: SimulatedHand | None = None
        self.bridge = LeapBridge()
        # How fast a servo moves when told to go somewhere else, in radians a second (the simulation's own figure): the lag the check turns into an age.
        self.speed = hand_sim.SPEED_RAD_S
        self.stale_s = bridge_module._STALE_S

    def start(self) -> None:
        self.hand = SimulatedHand("full", time_scale=1.0).start()
        self.bridge = LeapBridge()

    def attach(self, addresses) -> None:
        assert self.hand is not None
        self.hand.attach(addresses[0][1])

    def stop(self) -> None:
        if self.hand is not None:
            self.hand.stop()
            self.hand = None

    def config(self, addresses) -> str:
        return json.dumps({"device": addresses[0][0], "variant": "full", "stop_mode": "hold", "current_ma": 350, "gains": {"kp": 600, "ki": 0, "kd": 200}, "side_scale": 0.75, "simulator": True,
                           "rate_hz": 50.0, "open_timeout_ms": 3000})

    def open(self, config: str) -> int:
        return self.bridge.open(config)

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad)

    def target(self, index: int):
        return list(POSES[index & 1])

    def write(self, values) -> int:
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(values), mask=[1] * 16)
        return self.bridge.write_command(command, 0)

    def stop_call(self) -> int:
        return self.bridge.emergency_stop()

    def release(self) -> None:
        self.bridge.release_stop()

    def close(self) -> None:
        self.bridge.close()

    def truth(self):
        if self.hand is None:
            return [0.0] * 16
        return [frames.register_to_position(p) for p in self.hand.positions]


def subject() -> Subject:
    return Subject()
