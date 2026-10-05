"""The ODrive bridge's subject for `tools/faultproxy/chaos.py`: a kernel CAN bus with a fault-injecting relay across it.

    PYTHONPATH=bindings/python:drivers/canopen sh tools/seed-defects/in-netns.sh \\
        python3 tools/faultproxy/chaos.py drivers/odrive/chaos.py

Two `vcan` interfaces stand for one cable cut in two: the bridge is on `vcan0`, the simulated axes (`skynet_odrive.serve`, in a
process of their own) are on `vcan1`, and `tools/faultproxy`'s CAN relay carries every frame between them, through the engine.
The interfaces are made here if the namespace has none (`ip link add ... type vcan`, which an unprivileged user namespace can do).
**The axes agree with whoever wrote them** (F15): this shows the bridge's own waits, staleness and stop path against a bus that
loses, repeats, reorders and delays frames, and nothing about a real axis.

The stop the contract tries is the firmware-latching one: `emergency_stop` sends every axis its ESTOP frame and the driver holds
the latch in software, and there is no software release (`release_stop` is ENOSYS by design — §4, the Spot precedent). The only
profile that tries it on this transport is `blackhole`, which drops the stop's frames before they reach the axes, so what the
contract's recovery half measures is the session renewed over axes that never saw the stop — the re-open the OS does on ENOSYS —
and not a release clearing a latch this driver does not clear.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

import skynet_rhal as rhal

from skynet_canopen.bus import SocketCanBus
from skynet_odrive.bridge import ODriveBridge

DRIVER_SIDE = "vcan0"
SUBSTRATE_SIDE = "vcan1"
POSES = ([0.8, -0.8], [-0.8, 0.8])
NODES = (0, 1)


def _interfaces() -> None:
    for name in (DRIVER_SIDE, SUBSTRATE_SIDE):
        # `ip`, not /sys/class/net: an unprivileged network namespace shares the machine's sysfs, which lists the machine's devices.
        if subprocess.run(["ip", "link", "show", name], capture_output=True).returncode != 0:
            subprocess.run(["ip", "link", "add", "dev", name, "type", "vcan"], check=True, capture_output=True)
        subprocess.run(["ip", "link", "set", "up", name], check=True, capture_output=True)


class Subject:
    name = "odrive (a CAN bus through a relay)"
    profiles = ["delay-2ms", "jitter-2ms", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct"]
    speed = 0.0  # the axes are in another process: there is no truth to read, so no age to measure
    #: A cycle's estimates are asked for and awaited inside one window (50 ms by default), so a read that hears nothing for a whole
    #: cycle is the outage — two windows of it, and the blackhole's every read says so at once.
    stale_s = 0.1

    def __init__(self) -> None:
        self.flows = [("can", (DRIVER_SIDE, SUBSTRATE_SIDE))]
        self.process: subprocess.Popen | None = None
        self.bridge: ODriveBridge | None = None
        self.bus: SocketCanBus | None = None

    def start(self) -> None:
        _interfaces()
        here = os.path.dirname(os.path.abspath(__file__))
        environment = dict(os.environ)
        environment["PYTHONPATH"] = os.pathsep.join([
            here,
            os.path.join(here, "..", "canopen"),
            os.path.join(here, "..", "..", "bindings", "python"),
        ])
        self.process = subprocess.Popen(
            [sys.executable, "-m", "skynet_odrive.serve", SUBSTRATE_SIDE, *[str(n) for n in NODES]],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            env=environment,
        )
        assert self.process.stdout is not None
        if self.process.stdout.readline().strip() != "ready":
            raise SystemExit("chaos: the simulated axes did not come up")

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
        roles = ["right_arm.shoulder.pitch", "right_arm.elbow.flex"]
        return json.dumps({"joints": [{"node": n, "name": f"J{n}", "role": roles[i]} for i, n in enumerate(NODES)]})

    def open(self, config: str) -> int:
        self.bus = SocketCanBus(DRIVER_SIDE)
        self.bridge = ODriveBridge(self.bus)
        return self.bridge.open(config)

    def read(self):
        assert self.bridge is not None
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad)

    def target(self, index: int):
        return POSES[index & 1]

    def write(self, values) -> int:
        assert self.bridge is not None
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(values), mask=[1] * len(values))
        return self.bridge.write_command(command, 0)

    def stop_call(self) -> int:
        assert self.bridge is not None
        return self.bridge.emergency_stop()

    def release(self) -> None:
        assert self.bridge is not None
        self.bridge.release_stop()

    def close(self) -> None:
        if self.bridge is not None:
            self.bridge.close()

    def truth(self):
        return list(POSES[0])


def subject() -> Subject:
    return Subject()
