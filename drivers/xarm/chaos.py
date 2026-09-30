"""The xArm bridge's subject for `tools/faultproxy/chaos.py`: its controller's three sockets, each through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/xarm python3 tools/faultproxy/chaos.py drivers/xarm/chaos.py

The substrate is `SimulatedController`, the one the suite uses: a control socket that takes `SET_STATE` and `MOVE_SERVOJ`, a fast
report stream at 100 Hz and a rich one at 10 Hz, and an arm that moves towards its last target at the speed ceiling. The driver
is pointed at three relays — `host` and `ports` in its configuration — and the relays are what a cable, a switch or a busy
controller is to it. **The simulator agrees with whoever wrote it** (F15): what this shows is the driver's own timeout, staleness
and stop path, and nothing about UFACTORY's firmware.
"""

from __future__ import annotations

import json
import math

import skynet_rhal as rhal

from skynet_xarm import bridge as bridge_module
from skynet_xarm.bridge import XArmBridge
from skynet_xarm.controller import SimulatedController

#: Two far-apart poses; the arm is always on its way from one to the other, so a late reading is a wrong one.
POSES = (
    [0.8, -0.8, -0.5, 0.8, 0.0, 0.0],
    [-0.8, 0.8, -1.5, -0.8, 0.0, 0.0],
)


class Subject:
    name = "xarm (three TCP flows: control, fast report, rich report)"
    profiles = ["delay-50ms", "jitter-30ms", "slow-link", "stall-1500ms", "blackhole", "reset"]

    def __init__(self) -> None:
        self.controller: SimulatedController | None = None
        self.bridge = XArmBridge()
        self.flows: list = []
        self.speed = 0.0
        self.stale_s = bridge_module._STALE_S

    def start(self) -> None:
        self.controller = SimulatedController().start()
        ports = self.controller.ports
        self.flows = [("tcp", ("127.0.0.1", ports[name])) for name in ("control", "fast", "rich")]
        self.speed = self.controller.joint_max_speed
        self.bridge = XArmBridge()

    def stop(self) -> None:
        if self.controller is not None:
            self.controller.stop()
            self.controller = None

    def config(self, addresses) -> str:
        control, fast, rich = (address[1] for address in addresses)
        return json.dumps(
            {
                "host": "127.0.0.1",
                "ports": {"control": control, "fast": fast, "rich": rich},
                "joints": [{"name": f"joint{i + 1}", "role": None} for i in range(self.controller.axis)],
            }
        )

    def open(self, config: str) -> int:
        return self.bridge.open(config)

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad)

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
        return list(self.controller.position[: self.controller.axis])


def subject() -> Subject:
    return Subject()
