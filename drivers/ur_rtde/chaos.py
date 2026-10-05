"""The UR bridge's subject for `tools/faultproxy/chaos.py`: its controller's three sockets, each through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/ur_rtde python3 tools/faultproxy/chaos.py drivers/ur_rtde/chaos.py

The substrate is `sim/controller.py`: a dashboard server, the secondary interface that takes the control program, and RTDE at 125 Hz with the input registers the program reads, and an arm that moves toward the setpoint at no more than
`joint_max_speed`. The bridge is pointed at three relays (`ports` in its configuration) and the relays are what a cable, a switch or a busy controller is to it. **The simulated controller agrees with whoever wrote it** (F15): what this
shows is the bridge's own timeouts, staleness and stop path, and nothing about URControl, which `make check-ursim-docker` is the half of.

`read()` gives the sequence as its fourth value. RTDE carries no age in the recipe the bridge reads, so a stream that stopped is reported the way §24 says a stalled producer is: the sequence of the last sample is held.
"""

from __future__ import annotations

import json
import os
import sys

import skynet_rhal as rhal

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "sim"))

from controller import SimulatedController  # noqa: E402
from skynet_ur.bridge import UrBridge  # noqa: E402

#: Two far-apart poses; the arm is always on its way from one to the other, so a late reading is a wrong one.
POSES = (
    [0.8, -1.8, 1.8, -1.6, -1.0, 0.8],
    [-0.8, -0.6, 0.6, -0.4, -2.0, -0.8],
)
#: How long a read may go on handing out a sample before the bridge says it is old. The recipe has no timestamp, so this is the bridge's own bound, in seconds.
STALE_S = 0.25


class Subject:
    name = "ur_rtde (three TCP flows: the dashboard, the secondary interface, RTDE)"
    profiles = ["delay-50ms", "jitter-30ms", "slow-link", "stall-1500ms", "blackhole", "reset"]

    def __init__(self) -> None:
        self.controller: SimulatedController | None = None
        self.bridge = UrBridge()
        self.flows: list = []
        self.speed = 2.0
        self.stale_s = STALE_S

    def start(self) -> None:
        self.controller = SimulatedController(joint_max_speed=self.speed).start()
        ports = self.controller.ports
        self.flows = [("tcp", ("127.0.0.1", ports[name])) for name in ("dashboard", "script", "rtde")]
        self.bridge = UrBridge()

    def stop(self) -> None:
        if self.controller is not None:
            self.controller.stop()
            self.controller = None

    def config(self, addresses) -> str:
        dashboard, script, rtde = (address[1] for address in addresses)
        return json.dumps({"host": "127.0.0.1", "ports": {"dashboard": dashboard, "script": script, "rtde": rtde}, "model": "UR5e", "simulator": True, "startup_timeout_s": 3.0})

    def open(self, config: str) -> int:
        return self.bridge.open(config)

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad), state.seq

    def target(self, index: int):
        return POSES[index & 1]

    def write(self, values) -> int:
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(values), mask=[1] * len(values))
        return self.bridge.write_command(command, 0)

    def stop_call(self) -> int:
        return self.bridge.emergency_stop()

    def release(self) -> None:
        self.bridge.release_stop()

    def close(self) -> None:
        self.bridge.close()

    def truth(self):
        return list(self.controller.position)


def subject() -> Subject:
    return Subject()
