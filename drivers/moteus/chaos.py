"""The moteus bridge's subject for `tools/faultproxy/chaos.py`: a kernel CAN-FD bus with a fault-injecting relay across it.

    PYTHONPATH=bindings/python:drivers/canopen sh tools/seed-defects/in-netns.sh \\
        python3 tools/faultproxy/chaos.py drivers/moteus/chaos.py

Two `vcan` interfaces stand for one cable cut in two: the bridge is on `vcan0`, the simulated controllers (`skynet_moteus.serve`, in a process of their own) are on
`vcan1`, and `tools/faultproxy`'s CAN relay — in its CAN-FD mode, because a moteus's frames are 64 bytes and 29-bit — carries every frame between them, through the engine.
The interfaces are made here if the namespace has none, with the MTU an FD frame needs (`ip link set ... mtu 72`). **The controllers agree with whoever wrote them**
(F15): this shows the bridge's own waits, staleness and stop path against a bus that loses, repeats, reorders and delays frames — controllers that *answer*, so a lost frame
is a missing reply, which is a different thing for a driver to get wrong than Piper's broadcast — and nothing about a real controller.

**The watchdog is long on purpose.** A moteus that is not commanded for its watchdog falls back to the behaviour its owner configured and stays there until a *stop* — and
this driver never writes the stop that leaves it, because that is a person's act. So an outage longer than the controller's own watchdog ends in a latched `timeout` that no
read and no re-open clears, by design (the suite shows it). That is the controller's contract and the person's recovery, and it is not what this run measures: it measures what
the *driver* does on a link that is worse than the one it was written on, so the controllers here are given ten seconds (`watchdog_ms`), longer than any profile's outage,
and the driver's own staleness, latch and recovery are what is left to read.

The stop the contract tries is `zero_velocity`, which the driver can honestly release (the firmware switches straight back to position). The profile that tries it on this
transport is `blackhole`, which drops the stop's frames before they reach the controllers, so what the recovery half measures is a release over controllers that never saw the
stop: the latch is the driver's own, and it must clear without an axis the stop did not reach being treated as one that has to be brought back.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

import skynet_rhal as rhal

from skynet_moteus.bridge import MoteusBridge
from skynet_moteus.bus import SocketCanFdBus

DRIVER_SIDE = "vcan0"
SUBSTRATE_SIDE = "vcan1"
#: Two poses inside both joints' travel (±0.25 revolutions is ±1.57 rad).
POSES = ([0.3, -0.3], [-0.3, 0.3])


def _interfaces() -> None:
    for name in (DRIVER_SIDE, SUBSTRATE_SIDE):
        # `ip`, not /sys/class/net: an unprivileged network namespace shares the machine's sysfs, which lists the machine's devices.
        if subprocess.run(["ip", "link", "show", name], capture_output=True).returncode != 0:
            subprocess.run(["ip", "link", "add", "dev", name, "type", "vcan"], check=True, capture_output=True)
        subprocess.run(["ip", "link", "set", name, "mtu", "72"], check=True, capture_output=True)
        subprocess.run(["ip", "link", "set", "up", name], check=True, capture_output=True)


class Subject:
    name = "moteus (a CAN-FD bus through a relay)"
    profiles = ["delay-2ms", "jitter-2ms", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct"]
    speed = 0.0  # the controllers are in another process: there is no truth to read, so no age to measure
    #: A read waits for every controller's reply inside one window (50 ms by default), so a read that hears nothing for a whole cycle is the outage — two windows of it,
    #: and the blackhole's every read says so at once.
    stale_s = 0.1

    def __init__(self) -> None:
        self.flows = [("canfd", (DRIVER_SIDE, SUBSTRATE_SIDE))]
        self.process: subprocess.Popen | None = None
        self.bridge: MoteusBridge | None = None
        self.bus: SocketCanFdBus | None = None

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
            [sys.executable, "-m", "skynet_moteus.serve", SUBSTRATE_SIDE, "--ids", "1", "2"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            env=environment,
        )
        assert self.process.stdout is not None
        if self.process.stdout.readline().strip() != "ready":
            raise SystemExit("chaos: the simulated controllers did not come up")

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
        return json.dumps({"joints": [{"id": 1, "name": "J1"}, {"id": 2, "name": "J2"}], "stop_mode": "zero_velocity", "watchdog_ms": 10000})

    def open(self, config: str) -> int:
        self.bus = SocketCanFdBus(DRIVER_SIDE)
        self.bridge = MoteusBridge(self.bus)
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
