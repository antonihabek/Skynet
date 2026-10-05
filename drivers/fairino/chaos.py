"""The Fairino driver's subject for `tools/faultproxy/chaos.py`: its controller's three TCP sockets, each through a fault-injecting relay.

    make -C drivers/fairino chaos            # builds both shared objects, then runs this in a network namespace of its own

The substrate is `fake_controller.hpp`'s `FakeServer`, the one the suite runs, loaded as a shared object (`chaos_fake.cpp`) so that this process can ask the plant where
the arm is. Its three listeners are an XML-RPC server (every command verb), the instant-command port (where the vendor's own STOP frame arrives) and the CNDE state stream.
The driver is pointed at a relay for each — `xmlrpc_port`, `command_port` and `state_port` in its configuration — and the relays are what a cable, a switch or a busy
controller is to it. **The fake agrees with whoever wrote it** (F15): what this shows is the driver's own timeouts, staleness and stop path, and nothing about FAIR INNOVATION's
controller.

`CHAOS_DRIVER` and `CHAOS_FAKE` name other shared objects. The configuration leaves `rpc_timeout_ms` and `open_timeout_ms` at the driver's defaults on purpose: the contract is
about what the driver does as shipped.
"""

from __future__ import annotations

import ctypes
import json
import math
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "tools", "faultproxy"))

from rhal_ctypes import Driver  # noqa: E402

#: Where the fake listens. Not the vendor's 20003, 8080 and 20005: a developer running this outside a network namespace may have a controller (or the cross-check's) on those.
XMLRPC_PORT, COMMAND_PORT, STATE_PORT = 33003, 33080, 33005

DEGREES = math.pi / 180.0

#: Two far-apart poses, each inside every joint's soft limits (the fake's: base ±175°, shoulder -265/+85, elbow ±150, wrist 1 -265/+85, wrist 2 ±175, wrist 3 ±90); the arm is
#: always on its way from one to the other, so a late reading is a wrong one.
POSES = (
    [degrees * DEGREES for degrees in (30.0, -120.0, 60.0, -120.0, 30.0, 30.0)],
    [degrees * DEGREES for degrees in (-30.0, -60.0, 120.0, -60.0, -30.0, -30.0)],
)

#: The plant moves a joint at no more than this (`VELOCITY_MAX_DEG_S`), in the unit `read()` and `truth()` report.
SPEED_RAD_S = 180.0 * DEGREES


class Subject:
    name = "fairino (three TCP flows: XML-RPC, the instant-command port, the CNDE state stream)"
    flows: list = []
    profiles = ["delay-50ms", "jitter-30ms", "slow-link", "stall-1500ms", "blackhole", "reset"]
    speed = SPEED_RAD_S
    #: The longest `read_state` waits for a frame newer than the last it returned (`bound_ns`'s cap); after that it returns the old one under the old sequence, which is how
    #: it says the stream has stopped.
    stale_s = 0.2

    def __init__(self) -> None:
        self.driver = Driver(os.environ.get("CHAOS_DRIVER", os.path.join(ROOT, "drivers", "fairino", "libskynet_fairino.so")))
        fake_path = os.environ.get("CHAOS_FAKE", os.path.join(ROOT, "drivers", "fairino", "libfairino_fake.so"))
        if not os.path.exists(fake_path):
            raise SystemExit(f"chaos: no fake controller at {fake_path}. Build it first: make -C drivers/fairino chaos-libs")
        self._fake = ctypes.CDLL(fake_path)
        self._fake.fairino_fake_start.restype = ctypes.c_void_p
        self._fake.fairino_fake_start.argtypes = [ctypes.c_uint, ctypes.c_uint, ctypes.c_uint]
        self._fake.fairino_fake_positions_deg.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_double)]
        self._fake.fairino_fake_stop.argtypes = [ctypes.c_void_p]
        self._handle = None

    def start(self) -> None:
        self._handle = self._fake.fairino_fake_start(XMLRPC_PORT, COMMAND_PORT, STATE_PORT)
        if not self._handle:
            raise SystemExit(f"chaos: the fake controller could not bind {XMLRPC_PORT}, {COMMAND_PORT} and {STATE_PORT}. Something holds one of them: run this through `sh tools/seed-defects/in-netns.sh` (`make chaos` does).")
        self.flows = [("tcp", ("127.0.0.1", port)) for port in (XMLRPC_PORT, COMMAND_PORT, STATE_PORT)]

    def stop(self) -> None:
        if self._handle:
            self._fake.fairino_fake_stop(self._handle)
            self._handle = None

    def config(self, addresses) -> str:
        xmlrpc, command, state = (address[1] for address in addresses)
        return json.dumps({"host": "127.0.0.1", "xmlrpc_port": xmlrpc, "command_port": command, "state_port": state, "simulator": True})

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        return self.driver.read(6, with_seq=True)

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
        positions = (ctypes.c_double * 6)()
        self._fake.fairino_fake_positions_deg(self._handle, positions)
        return [value * DEGREES for value in positions]


def subject() -> Subject:
    return Subject()
