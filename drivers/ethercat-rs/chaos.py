"""The Rust EtherCAT port's subject for `tools/faultproxy/chaos.py`: two veth pairs with a fault-injecting relay joining them.

    cargo build --release -p skynet-ethercat-rs
    sh tools/seed-defects/in-netns.sh python3 tools/faultproxy/chaos.py drivers/ethercat-rs/chaos.py

The master (the port's shared object, through its RHAL vtable) is on `fpa0`; the ring — the Python driver's slave stack,
`python3 -m skynet_ethercat.wire`, which is the substrate `make check` uses — is on `fpb1`; and the relay is bound to `fpa1` and
`fpb0`, the two ends between them, carrying every EtherCAT frame across through the engine. A raw relay does not hear its own
transmissions, so a frame crosses once. **The ring agrees with whoever wrote it** (F15): this shows the port's own frame
deadline, working-counter assertion and stop path against a link that loses, repeats, reorders and delays frames.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "tools", "faultproxy"))

from rhal_ctypes import Driver  # noqa: E402

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
    name = "ethercat-rs (raw Ethernet frames through a relay, the Rust port)"
    profiles = ["delay-2ms", "jitter-2ms", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct", "truncate-5pct", "corrupt-5pct"]
    speed = 0.0  # the slave stack is in another process: no truth to read, so no age to measure
    stale_s = 0.1

    def __init__(self) -> None:
        self.flows = [("raw", (RELAY_DRIVER_SIDE, RELAY_SUBSTRATE_SIDE))]
        self.process: subprocess.Popen | None = None
        self.driver = Driver(os.environ.get("CHAOS_DRIVER", os.path.join(ROOT, "target", "release", "libskynet_ethercat_rs.so")))

    def start(self) -> None:
        _interfaces()
        environment = dict(os.environ)
        environment["PYTHONPATH"] = os.pathsep.join(
            [os.path.join(ROOT, "drivers", "ethercat"), os.path.join(ROOT, "drivers", "canopen"), os.path.join(ROOT, "bindings", "python")]
        )
        self.process = subprocess.Popen(
            [sys.executable, "-m", "skynet_ethercat.wire", "--interface", RING, "--drives", str(JOINTS)],
            stdout=subprocess.PIPE,
            text=True,
            env=environment,
        )
        assert self.process.stdout is not None
        if not self.process.stdout.readline().startswith("serving"):
            raise SystemExit("chaos: the slave stack did not start serving")

    def stop(self) -> None:
        if self.process is not None:
            self.process.kill()
            self.process.wait()
            self.process = None

    def config(self, addresses) -> str:
        roles = ["right_arm.shoulder.pitch", "right_arm.elbow.flex", "right_arm.wrist.roll"]
        return json.dumps({"interface": MASTER, "joints": [{"name": f"J{n + 1}", "role": roles[n]} for n in range(JOINTS)]})

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        return self.driver.read(JOINTS)

    def target(self, index: int):
        return POSES[index & 1]

    def write(self, values) -> int:
        return self.driver.write(values)

    def stop_call(self) -> int:
        return self.driver.emergency_stop()

    def release(self) -> None:
        self.driver.release_stop()

    def close(self) -> None:
        self.driver.close()

    def truth(self):
        return list(POSES[0])


def subject() -> Subject:
    return Subject()
