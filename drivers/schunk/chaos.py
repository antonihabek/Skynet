"""The SCHUNK bridge's subject for `tools/faultproxy/chaos.py`: its module's one TCP port, through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/schunk python3 tools/faultproxy/chaos.py drivers/schunk/chaos.py

The substrate is `SimulatedGripper`, the one the suite uses: the module's two routes over stdlib HTTP, a 16-byte input and output frame, and a jaw that moves at the speed it is told. The driver's three connections to
the module (the command's, the state poller's and the stop's) go through the one relay, which is what a cable, a switch or a busy controller is to it. **The simulator agrees with whoever wrote it** (F15): what this
shows is the driver's own timeout, staleness and stop path, and nothing about the maker's firmware or its simulator.
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_schunk import bridge as bridge_module
from skynet_schunk.bridge import SchunkBridge
from skynet_schunk.gripper_sim import SimulatedGripper

#: Two far-apart positions, each inside the travel (metres): the jaw is always on its way from one to the other, so a late reading is a wrong one.
POSES = ([0.03], [0.09])
SPEED = 0.02


class Subject:
    name = "schunk (one TCP port: the module's web interface)"
    #: `reset` is not here, for Reachy Mini's reason (`drivers/reachy_mini/chaos.py`): this driver is HTTP, and a connection the module dropped is made again on the next request while the module is still there,
    #: so the link carries data throughout; what `selftest.py` holds is DEGRADED for the staleness bound after one. A persistent outage is `blackhole`'s, and it is here.
    profiles = ["delay-50ms", "jitter-30ms", "slow-link", "stall-1500ms", "blackhole"]

    def __init__(self) -> None:
        self.gripper: SimulatedGripper | None = None
        self.bridge = SchunkBridge()
        self.flows: list = []
        self.speed = SPEED
        self.stale_s = bridge_module._STALE_S

    def start(self) -> None:
        self.gripper = SimulatedGripper().start()
        self.flows = [("tcp", ("127.0.0.1", self.gripper.port))]
        self.bridge = SchunkBridge()

    def stop(self) -> None:
        if self.gripper is not None:
            self.gripper.stop()
            self.gripper = None

    def config(self, addresses) -> str:
        return json.dumps({"host": "127.0.0.1", "port": addresses[0][1], "stop_mode": "controlled", "speed": SPEED, "simulator": True, "rate_hz": 20.0})

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
        return [self.gripper.position_um / 1e6] if self.gripper is not None else [0.0]


def subject() -> Subject:
    return Subject()
