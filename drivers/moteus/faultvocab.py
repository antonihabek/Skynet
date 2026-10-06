"""The moteus bridge's subject for `tools/fault-vocab/contract.py`: the mode and fault registers, through the bridge's read and write.

    PYTHONPATH=bindings/python:drivers/canopen:drivers/moteus python3 tools/fault-vocab/contract.py drivers/moteus/faultvocab.py

The substrate is `SimulatedMoteus` on a `LoopbackBus`, the pair the suite uses. A code is staged on the controller the way the firmware latches it — mode 1 and a
number in the fault register, or a limiting number beside mode 10, or a mode written over the controller's own — and the query carries it; **the simulator agrees with
whoever wrote it** (F15): what this holds is what the bridge does with a code a controller reports, and nothing about any maker's controller. The code tables are
generated from the firmware (`vocab_gen.py`, `drivers/fault-vocabulary.json`).

`clear` is what a person at the robot does, and for a moteus that is two acts: stop the controller (`moteus_tool --stop`, the one thing that clears a fault) and take it
back up (a position command that holds where it is), because a stopped controller has its driver off and a bridge that is not going to re-enable it on its own reads that as
somebody else's doing, for ever. The driver is *meant* not to resume (see `skynet_moteus.bridge`); the substrate is what a person with the maker's tool is.
"""

from __future__ import annotations

import json
import math

import skynet_rhal as rhal

from skynet_moteus import protocol as p
from skynet_moteus.bridge import MoteusBridge
from skynet_moteus.bus import LoopbackBus
from skynet_moteus.sim import SimulatedMoteus

ID = 1


class Subject:
    driver = "moteus"
    name = "moteus (the bridge against a simulated controller on a loopback bus)"
    #: The loopback bus delivers inside `recv`, so a staged code is in the next read.
    settle_s = 0.2

    def __init__(self) -> None:
        self.bridge: MoteusBridge | None = None
        self.sim: SimulatedMoteus | None = None
        self.bus: LoopbackBus | None = None

    def start(self) -> None:
        self.bus = LoopbackBus()
        master = self.bus.endpoint()
        self.sim = SimulatedMoteus(self.bus.endpoint(), [ID], position_min=-0.25, position_max=0.25)
        self.bus.attach(self.sim.poll)
        self.bridge = MoteusBridge(master)
        # A watchdog far longer than a check: simulated time is the polls, a contract run reads for a couple of seconds a code, and the controller must not time out under it.
        config = json.dumps({"joints": [{"id": ID, "name": "J1", "role": "right_arm.shoulder.pitch"}], "stop_mode": "zero_velocity", "watchdog_ms": 10000})
        rc = self.bridge.open(config)
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated controller (rc {rc})")

    def restart(self) -> None:
        self.start()

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None

    @property
    def controller(self):
        assert self.sim is not None
        return self.sim.controllers[ID]

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        controller = self.controller
        if vocabulary == "fault":
            if code == 0:
                self._person_clears()
            else:
                controller.inject_fault(code)
        elif vocabulary == "limiting":
            controller.limiting = code
        elif vocabulary == "mode":
            controller.mode = code
            controller.fault = 0
            if code in p.PREPARING_MODES:
                controller._prepare_left = 10**9  # the preparing phase would otherwise end in a few cycles; the code is staged for as long as it is read
            if code == p.MODE_POSITION:
                self._person_clears()
        else:
            raise AssertionError(vocabulary)

    def clear(self, vocabulary: str) -> None:
        self._person_clears()

    def _person_clears(self) -> None:
        """Stop, then take it back up: the controller leaves every state staged above through a stop, and a stopped one is not holding anything."""
        controller = self.controller
        controller.limiting = 0
        controller._prepare_left = 0
        controller.handle(p.stop_command(p.MODE_STOPPED), False, 0)
        controller.handle(p.position_command(math.nan, 0.0, 0.0, 1.0, 1.0, math.nan, 5.0), False, 0)
        controller.step(10)

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, state.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        return self.bridge.write_command(rhal.Command(mode=rhal.MODE_POSITION, values=[0.0], mask=[1]), 0)

    def expected_code(self, vocabulary: str, code: int) -> int:
        joint = 1 << 16
        if vocabulary == "fault":
            return joint | (p.MODE_FAULT << 8) | code
        if vocabulary == "limiting":
            return joint | (p.MODE_POSITION << 8) | code
        if vocabulary == "mode":
            return joint | (code << 8)
        raise AssertionError(vocabulary)


def subject() -> Subject:
    return Subject()
