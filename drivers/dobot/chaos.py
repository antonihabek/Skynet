"""The Dobot bridge's subject for `tools/faultproxy/chaos.py`: the controller's two TCP connections, the dashboard (29999) and the real-time feedback (30004), each through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/dobot python3 tools/faultproxy/chaos.py drivers/dobot/chaos.py

The substrate is `SimulatedController`, the one the suite uses: the dashboard's text protocol, a 1440-byte feedback packet every 8 ms, and an arm that follows a ServoJ target at the speed the simulation gives it. The
driver is pointed at two relays, which are what a cable, a switch or a busy controller is to it. **The simulator agrees with whoever wrote it** (F15): what this shows is the driver's own transaction budget, reading
cycle, staleness bound and stop path, and nothing about Dobot's firmware.

TCP delivers every byte or none, so the profiles are the stream ones: a delay, a jitter, a slow link, a stall, a blackhole and a reset. A feedback stream that is slow enough to back up is the case this subject is
for: the packets that arrive late are old, and nothing in a packet says how old it is on this clock.
"""

from __future__ import annotations

import json
import math

import skynet_rhal as rhal

from skynet_dobot import bridge as bridge_module, controller_sim
from skynet_dobot.bridge import DobotBridge
from skynet_dobot.controller_sim import SimulatedController

LIMITS = [[-6.0, 6.0], [-3.0, 3.0], [-2.5, 2.5], [-6.0, 6.0], [-6.0, 6.0], [-6.0, 6.0]]
#: Two far-apart poses, each inside every joint's range (radians): the arm is always on its way from one to the other, so a late reading is a wrong one.
POSES = ([-1.0] * 6, [1.0] * 6)
DEG = math.pi / 180.0


class Subject:
    name = "dobot (two TCP flows: the dashboard and the real-time feedback)"
    #: `reset` is not in the list, for the reason `drivers/onrobot/chaos.py` gives: a connection the controller dropped is made again within half a second while the controller is still there, so the link carries data
    #: throughout and "outage reported" has no outage to report (0 of 10 reads were not OK, which is the driver doing its job). What `selftest.py` holds is that a dropped connection is made again and the arm read and
    #: commanded again, that a controller which accepts and will not talk is retried at most twice a second, and that one which is not listening is reported as a line that could not be opened; a persistent outage is
    #: `blackhole`'s, and it is here.
    profiles = ["delay-50ms", "jitter-30ms", "slow-link", "stall-1500ms", "blackhole"]
    #: How fast the simulated arm follows a ServoJ target, in radians a second: the simulation's own figure (the maker publishes none for a ServoJ), the lag the check turns into an age.
    speed = controller_sim.MAX_SPEED_DEG_S * DEG
    stale_s = bridge_module._STALE_S

    def __init__(self) -> None:
        self.sim: SimulatedController | None = None
        self.bridge = DobotBridge()
        self.flows: list = []

    def start(self) -> None:
        self.sim = SimulatedController("CR5", time_scale=1.0, enabled=True).start()
        self.flows = [("tcp", ("127.0.0.1", self.sim.dashboard_port)), ("tcp", ("127.0.0.1", self.sim.feedback_port))]
        self.bridge = DobotBridge()

    def stop(self) -> None:
        if self.sim is not None:
            self.sim.stop()
            self.sim = None

    def config(self, addresses) -> str:
        dashboard, feedback = (address[1] for address in addresses)
        # A transaction of 250 ms: the relay's profiles delay each direction by up to 50 ms, and a round trip of a hundred is no reason to call a command lost.
        return json.dumps({"host": "127.0.0.1", "port": dashboard, "feedback_port": feedback, "model": "CR5", "stop_mode": "stop", "simulator": True, "limits": LIMITS, "payload_kg": 0.5, "rate_hz": 50.0,
                           "timeout_ms": 200, "open_timeout_ms": 3000})

    def open(self, config: str) -> int:
        return self.bridge.open(config)

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad), state.seq

    def target(self, index: int):
        return POSES[index & 1]

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
        return [v * DEG for v in self.sim.joints_deg] if self.sim is not None else [0.0] * 6


def subject() -> Subject:
    return Subject()
