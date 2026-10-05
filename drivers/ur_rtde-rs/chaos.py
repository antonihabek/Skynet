"""The Rust UR port's subject for `tools/faultproxy/chaos.py`: the same controller, relays and arm as `drivers/ur_rtde/chaos.py`, with the port's shared object in the bridge's place, called through its RHAL vtable.

    cargo build --release -p skynet-ur-rtde-rs
    sh tools/seed-defects/in-netns.sh python3 tools/faultproxy/chaos.py drivers/ur_rtde-rs/chaos.py

`CHAOS_DRIVER` names another shared object. **The controller agrees with whoever wrote it** (F15); this shows the port's own waits, staleness and stop path against a link that delays, stalls, blackholes and resets.
"""

from __future__ import annotations

import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "tools", "faultproxy"))
sys.path.insert(0, os.path.join(ROOT, "drivers", "ur_rtde"))
sys.path.insert(0, os.path.join(ROOT, "bindings", "python"))

from rhal_ctypes import Driver  # noqa: E402

_spec = importlib.util.spec_from_file_location("ur_rtde_chaos", os.path.join(ROOT, "drivers", "ur_rtde", "chaos.py"))
_base = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_base)


class Subject(_base.Subject):
    name = "ur_rtde-rs (three TCP flows: the dashboard, the secondary interface, RTDE; the Rust port)"

    def __init__(self) -> None:
        super().__init__()
        self.driver = Driver(os.environ.get("CHAOS_DRIVER", os.path.join(ROOT, "target", "release", "libskynet_ur_rtde_rs.so")))

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        return self.driver.read(6, with_seq=True)

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
