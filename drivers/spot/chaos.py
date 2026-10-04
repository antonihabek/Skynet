"""The Spot driver's subject for `tools/faultproxy/chaos.py`: the robot's one TLS port (all eight gRPC services share it) through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/spot <python with bosdyn-client> tools/faultproxy/chaos.py drivers/spot/chaos.py

The substrate is `fake_spot`, the one the suite uses: the eight gRPC services over TLS, with the vendor's own client unmodified on this side. The relay carries bytes and does not look inside them, so the TLS session
runs end to end through it, and the driver is pointed at the relay's port with the test trust root. Spot's API is command-level (§59.2): `write_command` refuses every joint command, so the "command" the contract
sends is the one route that moves this body, a task under `vendor.spot.task` (stand and sit, in turn). The body does not move in the fake, so there is no freshness question. **The fake agrees with whoever wrote
it** (F15): what this shows is the driver's own timeouts and stop path against a link worse than a dock network's, and nothing about Boston Dynamics' firmware.
"""

from __future__ import annotations

import json
import os

import skynet_rhal as rhal

from skynet_spot import fake_spot
from skynet_spot.bridge import TOKEN, SpotDriver

CERTS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "certs")


class Subject:
    name = "spot (one TLS port: the eight gRPC services)"
    #: Not `reset`, for the reason `drivers/reachy_mini/chaos.py` gives: gRPC reconnects a channel the peer dropped, so a reset is a failed call and the next one is made on a new connection. A persistent outage is
    #: `blackhole`'s.
    profiles = ["delay-50ms", "jitter-30ms", "slow-link", "stall-1500ms", "blackhole"]
    #: The body does not move in the fake and a command-level driver reads no setpoint, so a late reading is not a wrong one.
    speed = 0.0
    #: The driver keeps no state between reads; the bound on a read is what the contract asks of it.
    stale_s = 2.0
    #: Spot's one route that moves it is a task whose call waits for the robot's answer: the motion's own state read (the gate, `_READ_TIMEOUT_S`, 1.5 s) and then the command (`_COMMAND_TIMEOUT_S`, 3 s). A quarter
    #: of a second is a control loop's bound for a joint command and this body has no joint command, so the motion declares what its own call is given, and the contract holds it to that.
    write_bound_s = 5.0

    def __init__(self) -> None:
        self.server = None
        self.world = None
        self.driver = SpotDriver()
        self.flows: list = []

    def start(self) -> None:
        self.server, self.world, port = fake_spot.serve(0)
        self.flows = [("tcp", ("127.0.0.1", port))]
        self.driver = SpotDriver()

    def stop(self) -> None:
        if self.server is not None:
            self.server.stop(grace=0).wait()
            self.world.halt()
            self.server = None

    def config(self, addresses) -> str:
        return json.dumps({"host": "127.0.0.1", "port": addresses[0][1], "username": "admin", "password": "password", "ca": os.path.join(CERTS, "ca.pem"), "simulator": True})

    def open(self, config: str) -> int:
        rc = self.driver.open(config)
        if rc == rhal.SKR_OK:
            # The robot's motors on, over the clean relay, as an operator would before asking it to stand.
            self.driver.extension(TOKEN, b"power_on")
        return rc

    def read(self):
        state = rhal.State()
        rc = self.driver.read_state(state, 0)
        return rc, state.flags, list(state.position_rad) if rc == rhal.SKR_OK else []

    def target(self, index: int):
        return [float(index & 1)]

    def write(self, values) -> int:
        code, _ = self.driver.extension(TOKEN, b"stand" if values[0] else b"sit")
        return code

    def stop_call(self) -> int:
        return self.driver.emergency_stop()

    def release(self) -> None:
        self.driver.release_stop()

    def close(self) -> None:
        self.driver.close()

    def truth(self):
        return []


def subject() -> Subject:
    return Subject()
