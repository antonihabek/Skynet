"""The ROS-Industrial bridge's subject for `tools/faultproxy/chaos.py`: its controller's two servers, each through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/ros_industrial python3 tools/faultproxy/chaos.py drivers/ros_industrial/chaos.py

The substrate is `SimulatedController`, the one the suite uses: a motion server that takes trajectory points and a stop, and a state server that pushes feedback at 50 Hz and a status at
10 Hz, over an arm that moves towards each queued point at the speed its velocity ratio allows. The driver is pointed at two relays — `host` and `ports` in its configuration — and the relays
are what a cable, a switch or a busy controller is to it. **The simulator agrees with whoever wrote it** (F15): what this shows is the driver's own timeout, staleness and stop path, and
nothing about any robot maker's controller.
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_ros_industrial import bridge as bridge_module
from skynet_ros_industrial.bridge import RosIndustrialBridge
from skynet_ros_industrial.controller import (
    SIX_JOINT_LIMITS,
    SIX_JOINT_MAX_SPEED,
    SimulatedController,
)

#: Two far-apart poses, each inside every joint's range; the arm is always on its way from one to the other, so a late reading is a wrong one. Each is reached by a stream of points the
#: driver's own speed arithmetic allows (the writes below step towards it).
POSES = (
    [0.8, -0.8, -0.5, 0.8, 0.0, 0.0],
    [-0.8, 0.8, -1.5, -0.8, 0.0, 0.0],
)


class Subject:
    name = "ros_industrial (two TCP flows: the motion server and the state server)"
    profiles = ["delay-50ms", "jitter-30ms", "slow-link", "stall-1500ms", "blackhole", "reset"]

    def __init__(self) -> None:
        self.controller: SimulatedController | None = None
        self.bridge = RosIndustrialBridge()
        self.flows: list = []
        self.speed = 0.0
        self.stale_s = bridge_module._STALE_S
        self._held: list[float] = [0.0] * 6

    def start(self) -> None:
        self.controller = SimulatedController().start()
        ports = self.controller.ports
        self.flows = [("tcp", ("127.0.0.1", ports[name])) for name in ("motion", "state")]
        # The arm moves at the speed ratio the driver sends: its configured ceiling times the ratio, and a write is limited to one `point_duration_s` of that.
        self.speed = SIX_JOINT_MAX_SPEED * 0.5
        self.bridge = RosIndustrialBridge()
        self._held = [0.0] * 6

    def stop(self) -> None:
        if self.controller is not None:
            self.controller.stop()
            self.controller = None

    def config(self, addresses) -> str:
        motion, state = (address[1] for address in addresses)
        return json.dumps({
            "host": "127.0.0.1",
            "ports": {"motion": motion, "state": state},
            "simulator": True,
            "rate_hz": 10,
            "point_duration_s": 0.5,
            "joints": [{"name": f"joint_{i + 1}", "role": None, "min_rad": SIX_JOINT_LIMITS[i][0], "max_rad": SIX_JOINT_LIMITS[i][1], "max_speed_rad_s": SIX_JOINT_MAX_SPEED}
                       for i in range(6)],
        })

    def open(self, config: str) -> int:
        self._held = [0.0] * 6
        return self.bridge.open(config)

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad)

    def target(self, index: int):
        return POSES[index & 1]

    def write(self, values) -> int:
        # One step towards the pose, no further than the driver's own speed arithmetic allows in one point's duration: a step past that is the driver's refusal and not the link's.
        step = SIX_JOINT_MAX_SPEED * 0.5 * 0.9
        held = self.bridge._held or self._held
        goal = [max(h - step, min(h + step, v)) for h, v in zip(held, values)]
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=goal, mask=[1] * len(goal))
        return self.bridge.write_command(command, 0)

    def stop_call(self) -> int:
        return self.bridge.emergency_stop()

    def release(self) -> None:
        self.bridge.release_stop()

    def close(self) -> None:
        self.bridge.close()

    def truth(self):
        return self.controller.positions() if self.controller is not None else [0.0] * 6


def subject() -> Subject:
    return Subject()
