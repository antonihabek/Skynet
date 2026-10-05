"""The ODrive bridge's subject for `tools/fault-vocab/contract.py`: the axis error word in the heartbeat, through the bridge's read and write.

    PYTHONPATH=bindings/python:drivers/canopen python3 tools/fault-vocab/contract.py drivers/odrive/faultvocab.py

The substrate is `SimulatedAxis` on a `LoopbackBus`, the pair the suite uses. An error word is staged on the axis the way the suite
stages one — assigned, the way the firmware latches it — and the heartbeat carries it; **the simulator agrees with whoever wrote it**
(F15): what this holds is what the bridge does with a word an axis reports, and nothing about any maker's drive. The 13 words are
the DBC's `Axis_Error` table (`drivers/fault-vocabulary.json`), generated from it and not retyped.
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_canopen.bus import LoopbackBus
from skynet_odrive.axis import SimulatedAxis
from skynet_odrive.bridge import ODriveBridge

NODE = 0


class Subject:
    driver = "odrive"
    name = "odrive (the bridge against a simulated axis on a loopback bus)"
    #: The loopback bus delivers inside `recv`, so a staged word is in the next read.
    settle_s = 0.2

    def __init__(self) -> None:
        self.bridge: ODriveBridge | None = None
        self.axis: SimulatedAxis | None = None
        self.bus: LoopbackBus | None = None

    def start(self) -> None:
        self.bus = LoopbackBus()
        master = self.bus.endpoint()
        self.axis = SimulatedAxis(self.bus.endpoint(), NODE)
        self.bus.attach(self.axis.poll)
        self.bridge = ODriveBridge(master)
        config = json.dumps({"joints": [{"node": NODE, "name": "J1", "role": "right_arm.shoulder.pitch"}]})
        rc = self.bridge.open(config)
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated axis (rc {rc})")

    def restart(self) -> None:
        self.start()

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        assert self.axis is not None
        self.axis.axis_error = code

    def clear(self, vocabulary: str) -> None:
        """What a person at the drive does: clear it. The bridge never does (CLEAR_ERRORS is board-wide)."""
        assert self.axis is not None
        self.axis.axis_error = 0

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, state.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        return self.bridge.write_command(rhal.Command(mode=rhal.MODE_POSITION, values=[0.0], mask=[1]), 0)


def subject() -> Subject:
    return Subject()
