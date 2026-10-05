"""The Rust EtherCAT port's subject for `tools/faultproxy/chaos.py`: the same two veth pairs, relay and ring as `drivers/ethercat/chaos.py`, with the port's shared object in the master's place, called through its RHAL vtable.

    cargo build --release -p skynet-ethercat-rs
    sh tools/seed-defects/in-netns.sh python3 tools/faultproxy/chaos.py drivers/ethercat-rs/chaos.py

`CHAOS_DRIVER` names another shared object. The ring — the Python driver's slave stack, `python3 -m skynet_ethercat.wire`, which is the substrate `make check` uses — is on `fpb1`, the master on `fpa0`, and the relay
between them carries every EtherCAT frame across through the engine. **The ring agrees with whoever wrote it** (F15): this shows the port's own frame deadline, working-counter assertion and stop path against a link
that loses, repeats, reorders and delays frames.
"""

from __future__ import annotations

import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "tools", "faultproxy"))
sys.path.insert(0, os.path.join(ROOT, "drivers", "ethercat"))
sys.path.insert(0, os.path.join(ROOT, "drivers", "canopen"))
sys.path.insert(0, os.path.join(ROOT, "bindings", "python"))

from rhal_ctypes import Driver  # noqa: E402

_spec = importlib.util.spec_from_file_location("ethercat_chaos", os.path.join(ROOT, "drivers", "ethercat", "chaos.py"))
_base = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_base)


class Subject(_base.Subject):
    name = "ethercat-rs (raw Ethernet frames through a relay, the Rust port)"

    def __init__(self) -> None:
        super().__init__()
        self.driver = Driver(os.environ.get("CHAOS_DRIVER", os.path.join(ROOT, "target", "release", "libskynet_ethercat_rs.so")))

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        return self.driver.read(_base.JOINTS)

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
