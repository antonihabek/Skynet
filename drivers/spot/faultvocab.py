"""The Spot driver's subject for `tools/fault-vocab/contract.py`: what the robot's state says is wrong, through the driver's own read and motions.

    PYTHONPATH=bindings/python:drivers/spot ~/spot-venv/bin/python tools/fault-vocab/contract.py drivers/spot/faultvocab.py

The substrate is `fake_spot`, the gRPC services the suite uses, served over loopback TLS and driven by the driver exactly as it drives a robot — which
is Boston Dynamics' own `bosdyn-client`, unmodified (`make faultvocab` refuses to run without it). **The fake agrees with whoever wrote it** (F15):
what this holds is what the driver makes of a severity, a cause or an estop state the robot reports, and nothing about when a real Spot reports one.

This body is moved by motions (`vendor.spot.task`) and its `write_command` refuses every joint command, always (§59.2); "a command" here is the motion
that brings the body up or takes it somewhere new (`stand`), and the vocabularies' `keeps` are the motions a person most wants when something is wrong.
"""

from __future__ import annotations

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "bindings", "python"))
sys.path.insert(0, HERE)

try:
    import bosdyn.client  # noqa: F401
    import grpc  # noqa: F401
except ImportError:
    print("::error::the Spot contract needs the project environment: bosdyn-client and grpcio are not importable. See drivers/spot/README.md.", flush=True)
    raise SystemExit(1)

import skynet_rhal as rhal

from skynet_spot import TOKEN, SpotDriver
from skynet_spot.fake_spot import STAND, serve

CERTS = os.path.join(HERE, "certs")
#: The maker's own number for a fault, as the fake reports it: arbitrary, and not zero (§24's healthy).
FAULT_CODE = 4242


class Subject:
    driver = "spot"
    name = "spot (the driver against the fake gRPC services, with the vendor's own client)"
    #: The driver reads the robot's state on every call and caches nothing, so a changed state is in the next read.
    settle_s = 0.5

    def __init__(self) -> None:
        self.server = None
        self.world = None
        self.driver_ = None

    def start(self) -> None:
        self.stop()
        self.server, self.world, port = serve(0)
        self.driver_ = SpotDriver()
        config = {"host": "127.0.0.1", "port": port, "username": "admin", "password": "password", "ca": f"{CERTS}/ca.pem"}
        rc = self.driver_.open(json.dumps(config))
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the driver did not open against the fake robot (rc {rc})")
        for request in ("power_on", "stand"):
            code, text = self.driver_.extension(TOKEN, request.encode())
            if code != rhal.SKR_OK:
                raise SystemExit(f"faultvocab: `{request}` was refused at the start (rc {code}: {text.decode(errors='replace')})")

    def restart(self) -> None:
        self.start()

    def stop(self) -> None:
        if self.driver_ is not None:
            self.driver_.close()
            self.driver_ = None
        if self.server is not None:
            self.server.stop(grace=1).wait()
            self.server = None
        if self.world is not None:
            self.world.halt()
            self.world = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        world = self.world
        assert world is not None
        if vocabulary == "system_fault_severity":
            world.inject_fault("injected", severity=code, code=FAULT_CODE)
        elif vocabulary == "behavior_fault_cause":
            world.inject_behavior_fault("injected", cause=code)
        elif vocabulary == "estop_state":
            world.hw_estop_state = code
        elif vocabulary == "motor_power_state":
            world.motor_power_override = code
        else:
            raise SystemExit(f"faultvocab: no way to inject {vocabulary}")

    def clear(self, vocabulary: str) -> None:
        """What a person does about each: the robot's own report goes away. The driver never clears one (`ClearBehaviorFault` is an operator's act)."""
        world = self.world
        assert world is not None
        world.clear_faults()
        with world._lock:  # noqa: SLF001 — the fake's own world, which a test reaches into by design
            world.behavior_faults.clear()
            world.hw_estop_state = None
            world.motor_power_override = None
        self.restore()

    def restore(self) -> None:
        """What the world is put back to after a motion: motors on and standing, as `start` left it."""
        world = self.world
        assert world is not None
        with world._lock:  # noqa: SLF001
            world.powered = True
            for name in world.joints:
                world.joints[name] = (STAND[name.rsplit(".", 1)[1]], 0.0)
            world.body_z = 0.55

    def read(self) -> tuple[int, int, int]:
        assert self.driver_ is not None
        state = rhal.State()
        rc = self.driver_.read_state(state, 0)
        return rc, state.flags, state.fault_code

    def write(self) -> int:
        """The motion that takes the body somewhere new."""
        return self._ask("stand")

    def motion(self, name: str) -> int:
        return self._ask(name)

    def _ask(self, request: str) -> int:
        assert self.driver_ is not None
        return self.driver_.extension(TOKEN, request.encode())[0]

    def expected_code(self, vocabulary: str, code: int) -> int:
        """A system fault's own `code`; a behaviour fault's cause under the top bit; a motor power state under the next one down."""
        if vocabulary == "system_fault_severity":
            return FAULT_CODE
        if vocabulary == "behavior_fault_cause":
            return 0x80000000 | code
        if vocabulary == "motor_power_state":
            return 0x40000000 | code
        raise SystemExit(f"faultvocab: {vocabulary} has no fault word to expect")


def subject() -> Subject:
    return Subject()
