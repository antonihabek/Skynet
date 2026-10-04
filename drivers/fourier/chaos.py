"""The Fourier driver's subject for `tools/faultproxy/chaos.py`: RoCS's one TCP port (the HTTP calls and the command WebSocket both) through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/fourier python3 tools/faultproxy/chaos.py drivers/fourier/chaos.py

The substrate is `fake_rocs`, the one the suite uses: the limit list, a per-motor read, the stop route and the `move_joint` WebSocket, with motors that jump to the angle they are told. The driver's HTTP calls and
its WebSocket go to the same port, so one relay carries both. **The fake agrees with whoever wrote it** (F15): what this shows is the driver's own timeouts and stop path against a link that is worse than a LAN,
and nothing about Fourier's server.
"""

from __future__ import annotations

import json
import math
import threading

import skynet_rhal as rhal

from skynet_fourier.bridge import WATCHDOG_MS, FourierDriver
from skynet_fourier.fake_rocs import MOTORS, serve

#: Two poses a quarter of a radian apart on every motor (the fake's motors jump, so a gap this size is what a late reading is measured against), each inside every motor's range.
_START = [math.radians(start) for *_, start in MOTORS]
POSES = (_START, [value + 0.25 for value in _START])
#: The rate at which "how stale" is converted to seconds: one radian a second, so a reading a quarter of a radian behind is a quarter of a second old.
SPEED = 1.0


class Subject:
    name = "fourier (one TCP port: RoCS's HTTP calls and its command WebSocket)"
    #: Not `reset`, for the reason `drivers/reachy_mini/chaos.py` gives for an HTTP driver: a dropped HTTP connection is a failed call and the next call is a new one. (The WebSocket this driver holds is
    #: the exception, and it stays dead by design: a driver that reconnected mid-session would be deciding the robot is back. `recovers` allows a re-open.)
    profiles = ["delay-50ms", "jitter-30ms", "slow-link", "stall-1500ms", "blackhole"]
    speed = SPEED
    #: The driver has no state between reads; its watchdog window is the bound it states for a read.
    stale_s = WATCHDOG_MS / 1000.0

    def __init__(self) -> None:
        self.server = None
        self.state = None
        self.driver = FourierDriver()
        self.flows: list = []

    def start(self) -> None:
        self.server, self.state = serve(0)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.flows = [("tcp", ("127.0.0.1", self.server.server_port))]
        self.driver = FourierDriver()

    def stop(self) -> None:
        if self.server is not None:
            self.server.shutdown()
            self.server.server_close()
            self.server = None

    def config(self, addresses) -> str:
        return json.dumps({"host": "127.0.0.1", "port": addresses[0][1], "simulator": True})

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        state = rhal.State()
        rc = self.driver.read_state(state, 0)
        return rc, state.flags, list(state.position_rad) if rc == rhal.SKR_OK else []

    def target(self, index: int):
        return POSES[index & 1]

    def write(self, values) -> int:
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(values), mask=[1] * len(values))
        return self.driver.write_command(command, 0)

    def stop_call(self) -> int:
        return self.driver.emergency_stop()

    def release(self) -> None:
        self.driver.release_stop()

    def close(self) -> None:
        self.driver.close()

    def truth(self):
        with self.state.lock:
            return [math.radians(self.state.positions[(no, orientation)]) for no, orientation, *_ in MOTORS]


def subject() -> Subject:
    return Subject()
