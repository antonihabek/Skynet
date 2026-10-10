"""The Rainbow bridge's subject for `tools/faultproxy/chaos.py`: the control box's two TCP connections, the command port (5000) and the data port (5001), each through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/rainbow python3 tools/faultproxy/chaos.py drivers/rainbow/chaos.py

The substrate is `SimulatedController`, the one the suite uses: the command port's text lines with their acknowledgements, a 580-byte state packet for every `reqdata` on the data port, and an arm that follows a
`move_servo_j` target at the speed the simulation gives it. The driver is pointed at two relays, which are what a cable, a switch or a busy control box is to it. **The simulator agrees with whoever wrote it** (F15):
what this shows is the driver's own transaction budget, reading cycle, staleness bound and stop path, and nothing about Rainbow Robotics' firmware.

TCP delivers every byte or none, so the profiles are the stream ones: a delay, a jitter, a slow link, a stall, a blackhole and a reset. The data port is a request and a reply, so a slow link makes its packets late and
nothing in a packet says how old it is on this clock; the driver's own bound on the time a reply took is what the check turns into an age.
"""

from __future__ import annotations

import json
import math

import skynet_rhal as rhal

from skynet_rainbow import bridge as bridge_module, controller_sim
from skynet_rainbow.bridge import RainbowBridge
from skynet_rainbow.controller_sim import SimulatedController

LIMITS = [[-6.0, 6.0], [-3.0, 3.0], [-2.5, 2.5], [-6.0, 6.0], [-6.0, 6.0], [-6.0, 6.0]]
#: Two far-apart poses, each inside every joint's range (radians): the arm is always on its way from one to the other, so a late reading is a wrong one.
POSES = ([-1.0] * 6, [1.0] * 6)
DEG = math.pi / 180.0


class Subject:
    name = "rainbow (two TCP flows: the command port and the data port)"
    #: `reset` is not in the list, for the reason `drivers/dobot/chaos.py` gives: a connection the controller dropped is made again within half a second while the control box is still there, so the link carries data
    #: throughout and "outage reported" has no outage to report. What `selftest.py` holds is that a dropped connection is made again and the arm read and commanded again, that a control box which accepts and will not talk
    #: is retried at most twice a second, and that one which is not listening is reported as a port that could not be opened; a persistent outage is `blackhole`'s, and it is here.
    profiles = ["delay-50ms", "jitter-30ms", "slow-link", "stall-1500ms", "blackhole"]
    #: How fast the simulated arm follows a servo target, in radians a second: the simulation's own figure (the maker publishes none), the lag the check turns into an age.
    speed = controller_sim.MAX_SPEED_DEG_S * DEG
    stale_s = bridge_module._STALE_S

    def __init__(self) -> None:
        self.sim: SimulatedController | None = None
        self.bridge = RainbowBridge()
        self.flows: list = []

    def start(self) -> None:
        self.sim = SimulatedController("RB5-850", time_scale=1.0, simulation=True).start()
        self.flows = [("tcp", ("127.0.0.1", self.sim.command_port)), ("tcp", ("127.0.0.1", self.sim.data_port))]
        self.bridge = RainbowBridge()

    def stop(self) -> None:
        if self.sim is not None:
            self.sim.stop()
            self.sim = None

    def config(self, addresses) -> str:
        command, data = (address[1] for address in addresses)
        # A transaction of 250 ms: the relay's profiles delay each direction by up to 50 ms, and a round trip of a hundred is no reason to call a command lost.
        return json.dumps({"host": "127.0.0.1", "port": command, "data_port": data, "model": "RB5-850", "simulator": True, "limits": LIMITS, "rate_hz": 50.0, "timeout_ms": 250, "open_timeout_ms": 3000})

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
