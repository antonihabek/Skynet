"""The Reachy Mini bridge's subject for `tools/faultproxy/chaos.py`: its daemon's one TCP port, through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/reachy_mini python3 tools/faultproxy/chaos.py drivers/reachy_mini/chaos.py

The substrate is `SimulatedDaemon`, the one the suite uses: HTTP routes for the state, a target and a stop, and a robot that follows its target at the speed each joint has. The driver's three connections to
the daemon (the command's, the state poller's and the stop's) go through the one relay, which is what a cable, a switch or a busy daemon is to it. **The simulator agrees with whoever wrote it** (F15): what
this shows is the driver's own timeout, staleness and stop path, and nothing about the maker's daemon.
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_reachy_mini import bridge as bridge_module
from skynet_reachy_mini.bridge import ReachyMiniBridge
from skynet_reachy_mini.daemon_sim import RANGES, SPEEDS, SimulatedDaemon

#: Two far-apart poses, each inside every range, differing in the two joints that move fastest (radians): the robot is always on its way from one to the other, so a late reading is a wrong one.
POSES = (
    [0.0, 0.0, 0.0, 0.0, 0.4, 0.0, 0.5, 0.0, 0.0],
    [0.0, 0.0, 0.0, 0.0, -0.4, 0.0, -0.5, 0.0, 0.0],
)
RATE_HZ = 20.0


class Subject:
    name = "reachy_mini (one TCP port: the daemon's REST interface)"
    #: `reset` is not here, and the reason is the protocol's: its expectation is that a reset connection is an outage that is still there when the profile ends, which is true of a stream socket nobody opens
    #: again. This driver is HTTP, and a connection the daemon dropped is made again on the next request (the daemon is still there, so the link carries data throughout); what it does say, and `selftest.py` holds,
    #: is DEGRADED for the staleness bound after one. A persistent outage is `blackhole`'s, and it is here.
    profiles = ["delay-50ms", "jitter-30ms", "slow-link", "stall-1500ms", "blackhole"]

    def __init__(self) -> None:
        self.daemon: SimulatedDaemon | None = None
        self.bridge = ReachyMiniBridge()
        self.flows: list = []
        self.speed = SPEEDS["head_pitch"]
        self.stale_s = bridge_module._STALE_S

    def start(self) -> None:
        self.daemon = SimulatedDaemon().start()
        self.flows = [("tcp", ("127.0.0.1", self.daemon.port))]
        self.bridge = ReachyMiniBridge()

    def stop(self) -> None:
        if self.daemon is not None:
            self.daemon.stop()
            self.daemon = None

    def config(self, addresses) -> str:
        return json.dumps({
            "host": "127.0.0.1", "port": addresses[0][1], "stop_mode": "hold", "rate_hz": RATE_HZ,
            "limits": {name: list(RANGES[name]) for name in ("head_x", "head_y", "head_z", "antenna_right", "antenna_left")}, "speeds": dict(SPEEDS),
        })

    def open(self, config: str) -> int:
        return self.bridge.open(config)

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad)

    def target(self, index: int):
        return POSES[index & 1]

    def write(self, values) -> int:
        # One step towards the pose, no further than the driver's own speed arithmetic allows in one command period: a step past that is the driver's refusal and not the link's.
        held = self.bridge._held or [0.0] * 9
        goal = []
        for index, (h, v) in enumerate(zip(held, values)):
            step = list(SPEEDS.values())[index] / RATE_HZ * 0.9
            goal.append(max(h - step, min(h + step, v)))
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=goal, mask=[1] * len(goal))
        return self.bridge.write_command(command, 0)

    def stop_call(self) -> int:
        return self.bridge.emergency_stop()

    def release(self) -> None:
        self.bridge.release_stop()

    def close(self) -> None:
        self.bridge.close()

    def truth(self):
        return self.daemon.present() if self.daemon is not None else [0.0] * 9


def subject() -> Subject:
    return Subject()
