"""The Python EtherCAT master's subject for `tools/faultproxy/chaos.py`: two veth pairs with a fault-injecting relay joining them.

    sh tools/seed-defects/in-netns.sh env PYTHONPATH=bindings/python:drivers/ethercat:drivers/canopen \\
        python3 tools/faultproxy/chaos.py drivers/ethercat/chaos.py

The master (`skynet_ethercat.bridge.EtherCatBridge`, in this process) is on `fpa0`; the ring — the slave stack, `python3 -m skynet_ethercat.wire`, which is the substrate `make check` uses — is on `fpb1`; and the relay is
bound to `fpa1` and `fpb0`, the two ends between them, carrying every EtherCAT frame across through the engine. A raw relay does not hear its own transmissions, so a frame crosses once. **The ring agrees with whoever
wrote it** (F15): this shows the master's own frame deadline, working-counter assertion and stop path against a link that loses, repeats, reorders and delays frames, and nothing about a real slave. The Rust port's subject
(`drivers/ethercat-rs/chaos.py`) is this one with the port's shared object in the master's place.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

import skynet_rhal as rhal

from skynet_ethercat.bridge import EtherCatBridge

MASTER = "fpa0"
RELAY_DRIVER_SIDE = "fpa1"
RELAY_SUBSTRATE_SIDE = "fpb0"
RING = "fpb1"
JOINTS = 3
POSES = ([0.8, -0.8, 0.5], [-0.8, 0.8, -0.5])


def _has(name: str) -> bool:
    return subprocess.run(["ip", "link", "show", name], capture_output=True).returncode == 0


def _interfaces() -> None:
    for first, second in ((MASTER, RELAY_DRIVER_SIDE), (RELAY_SUBSTRATE_SIDE, RING)):
        if not _has(first):
            subprocess.run(["ip", "link", "add", first, "type", "veth", "peer", "name", second], check=True, capture_output=True)
        for name in (first, second):
            subprocess.run(["ip", "link", "set", "up", name], check=True, capture_output=True)


class Subject:
    name = "ethercat (raw Ethernet frames through a relay)"
    profiles = ["delay-2ms", "jitter-2ms", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct", "truncate-5pct", "corrupt-5pct"]
    speed = 0.0  # the slave stack is in another process: no truth to read, so no age to measure
    stale_s = 0.1

    def __init__(self) -> None:
        self.flows = [("raw", (RELAY_DRIVER_SIDE, RELAY_SUBSTRATE_SIDE))]
        self.process: subprocess.Popen | None = None
        self.bridge = EtherCatBridge()

    def start(self) -> None:
        _interfaces()
        here = os.path.dirname(os.path.abspath(__file__))
        environment = dict(os.environ)
        environment["PYTHONPATH"] = os.pathsep.join([here, os.path.join(here, "..", "canopen"), os.path.join(here, "..", "..", "bindings", "python")])
        self.process = subprocess.Popen(
            [sys.executable, "-m", "skynet_ethercat.wire", "--interface", RING, "--drives", str(JOINTS)],
            stdout=subprocess.PIPE,
            text=True,
            env=environment,
        )
        assert self.process.stdout is not None
        if not self.process.stdout.readline().startswith("serving"):
            raise SystemExit("chaos: the slave stack did not start serving")
        self.bridge = EtherCatBridge()

    def stop(self) -> None:
        if self.process is not None:
            self.process.kill()
            self.process.wait()
            self.process = None

    def config(self, addresses) -> str:
        roles = ["right_arm.shoulder.pitch", "right_arm.elbow.flex", "right_arm.wrist.roll"]
        return json.dumps({"interface": MASTER, "joints": [{"name": f"J{n + 1}", "role": roles[n]} for n in range(JOINTS)]})

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
        return list(POSES[0])


def subject() -> Subject:
    return Subject()
