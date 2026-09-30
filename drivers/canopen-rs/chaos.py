"""The Rust CANopen port's subject for `tools/faultproxy/chaos.py`: the same bus, relay and simulated drives as `drivers/canopen/chaos.py`,
with the port's shared object in the bridge's place, called through its RHAL vtable.

    cargo build --release -p skynet-canopen-rs
    sh tools/seed-defects/in-netns.sh python3 tools/faultproxy/chaos.py drivers/canopen-rs/chaos.py

`CHAOS_DRIVER` names another shared object. **The drives agree with whoever wrote them** (F15); this shows the port's own waits,
staleness and stop path against a bus that loses, repeats, reorders and delays frames.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "tools", "faultproxy"))
sys.path.insert(0, os.path.join(ROOT, "drivers", "canopen"))
sys.path.insert(0, os.path.join(ROOT, "bindings", "python"))

from rhal_ctypes import Driver  # noqa: E402

_spec = importlib.util.spec_from_file_location("canopen_chaos", os.path.join(ROOT, "drivers", "canopen", "chaos.py"))
_base = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_base)


class Subject(_base.Subject):
    name = "canopen-rs (a CAN bus through a relay, the Rust port)"

    def __init__(self) -> None:
        super().__init__()
        self.driver = Driver(os.environ.get("CHAOS_DRIVER", os.path.join(ROOT, "target", "release", "libskynet_canopen_rs.so")))

    def config(self, addresses) -> str:
        roles = ["right_arm.shoulder.pitch", "right_arm.elbow.flex", "right_arm.wrist.roll"]
        return json.dumps(
            {
                "interface": _base.DRIVER_SIDE,
                "joints": [{"node": n, "name": f"J{n}", "role": roles[(n - 1) % 3]} for n in _base.NODES],
            }
        )

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        return self.driver.read(len(_base.NODES))

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
