"""The CANopen bridge's subject for `tools/faultproxy/chaos.py`: a kernel CAN bus with a fault-injecting relay across it.

    sh tools/seed-defects/in-netns.sh env PYTHONPATH=bindings/python:drivers/canopen \\
        python3 tools/faultproxy/chaos.py drivers/canopen/chaos.py

Two `vcan` interfaces stand for one cable cut in two: the bridge is on `vcan0`, the simulated drives (`skynet_canopen.drives`, in a
process of their own) are on `vcan1`, and `tools/faultproxy`'s CAN relay carries every frame between them, through the engine.
The interfaces are made here if the namespace has none (`ip link add ... type vcan`, which an unprivileged user namespace can do).
**The drives agree with whoever wrote them** (F15): this shows the bridge's own waits, staleness and stop path against a bus that
loses, repeats, reorders and delays frames, and nothing about a real drive.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

import skynet_rhal as rhal

from skynet_canopen.bridge import CanOpenBridge
from skynet_canopen.bus import SocketCanBus

DRIVER_SIDE = "vcan0"
SUBSTRATE_SIDE = "vcan1"
POSES = ([0.8, -0.8, 0.5], [-0.8, 0.8, -0.5])
NODES = (1, 2, 3)


def _interfaces() -> None:
    for name in (DRIVER_SIDE, SUBSTRATE_SIDE):
        # `ip`, not /sys/class/net: an unprivileged network namespace shares the machine's sysfs, which lists the machine's devices.
        if subprocess.run(["ip", "link", "show", name], capture_output=True).returncode != 0:
            subprocess.run(["ip", "link", "add", "dev", name, "type", "vcan"], check=True, capture_output=True)
        subprocess.run(["ip", "link", "set", "up", name], check=True, capture_output=True)


class Subject:
    name = "canopen (a CAN bus through a relay)"
    profiles = ["delay-2ms", "jitter-2ms", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct"]
    speed = 0.0  # the drives are in another process: there is no truth to read, so no age to measure
    stale_s = 0.1

    def __init__(self) -> None:
        self.flows = [("can", (DRIVER_SIDE, SUBSTRATE_SIDE))]
        self.process: subprocess.Popen | None = None
        self.bridge: CanOpenBridge | None = None
        self.bus: SocketCanBus | None = None

    def start(self) -> None:
        _interfaces()
        here = os.path.dirname(os.path.abspath(__file__))
        environment = dict(os.environ)
        environment["PYTHONPATH"] = os.pathsep.join([here, os.path.join(here, "..", "..", "bindings", "python")])
        self.process = subprocess.Popen(
            [sys.executable, "-m", "skynet_canopen.drives", SUBSTRATE_SIDE, *[str(n) for n in NODES]],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            env=environment,
        )
        assert self.process.stdout is not None
        if self.process.stdout.readline().strip() != "ready":
            raise SystemExit("chaos: the simulated drives did not come up")

    def stop(self) -> None:
        if self.process is not None:
            assert self.process.stdin is not None
            self.process.stdin.close()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
            self.process = None

    def config(self, addresses) -> str:
        roles = ["right_arm.shoulder.pitch", "right_arm.elbow.flex", "right_arm.wrist.roll"]
        return json.dumps({"joints": [{"node": n, "name": f"J{n}", "role": roles[(n - 1) % 3]} for n in NODES]})

    def open(self, config: str) -> int:
        self.bus = SocketCanBus(DRIVER_SIDE)
        self.bridge = CanOpenBridge(self.bus)
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
        if self.bridge is not None:
            self.bridge.close()

    def truth(self):
        return list(POSES[0])


def subject() -> Subject:
    return Subject()
