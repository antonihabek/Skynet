"""The Rust Franka port's subject for `tools/faultproxy/chaos.py`: the same fake Control box and the same kernel link emulation as `drivers/franka/chaos.py`, with the port's shared object in the bridge's place, called
through its RHAL vtable.

    cargo build --release -p skynet-franka-rs
    sh tools/seed-defects/in-netns.sh python3 tools/faultproxy/chaos.py drivers/franka-rs/chaos.py

`CHAOS_DRIVER` names another shared object. **The fake agrees with whoever wrote it** (F15); this shows the port's own waits, staleness and stop path against a link that delays, loses, repeats, reorders, corrupts and
blackholes the FCI's TCP and UDP traffic, and nothing about Franka's Control box.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "tools", "faultproxy"))
sys.path.insert(0, os.path.join(ROOT, "drivers", "franka"))
sys.path.insert(0, os.path.join(ROOT, "bindings", "python"))

from rhal_ctypes import Driver  # noqa: E402

_spec = importlib.util.spec_from_file_location("franka_chaos", os.path.join(ROOT, "drivers", "franka", "chaos.py"))
_base = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_base)


class Subject(_base.Subject):
    name = "franka-rs (the FCI's TCP command port and UDP state stream, the kernel's link emulation on the loopback; the Rust port)"

    def __init__(self) -> None:
        super().__init__()
        self.driver = Driver(os.environ.get("CHAOS_DRIVER", os.path.join(ROOT, "target", "release", "libskynet_franka_rs.so")))

    def config(self, addresses) -> str:
        return json.dumps({"host": "127.0.0.1", "port": self.port, "simulator": True})

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        return self.driver.read(7, with_seq=True)

    def write(self, values) -> int:
        return self.driver.write(values)

    def stop_call(self) -> int:
        return self.driver.emergency_stop()

    def release(self) -> None:
        self.driver.release_stop()

    def close(self) -> None:
        self.driver.close()


def subject() -> Subject:
    return Subject()
