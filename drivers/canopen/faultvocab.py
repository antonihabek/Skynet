"""The CANopen bridge's subject for `tools/fault-vocab/contract.py`: a drive's emergency object and its CiA 402 state, through the bridge's read and write.

    PYTHONPATH=bindings/python:drivers/canopen python3 tools/fault-vocab/contract.py drivers/canopen/faultvocab.py

The substrate is `SimulatedDrive` on a `LoopbackBus`, the pair the suite uses. A drive is made to say what a real one says — an emergency object
with a code, a statusword in another state — and **the simulator agrees with whoever wrote it** (F15): what this holds is what the bridge does with
a code or a state a drive reports, and nothing about any maker's drive. The 35 emergency codes are CiA 301's, as CANopenNode lists them
(`drivers/fault-vocabulary.json`).
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_canopen.bus import LoopbackBus
from skynet_canopen.bridge import CanOpenBridge
from skynet_canopen.cia402 import State
from skynet_canopen.frames import Frame
from skynet_canopen.slave import SimulatedDrive

#: The statusword each listed code stands for, and the state a drive in it is in.
STATES = {55: State.OPERATION_ENABLED, 15: State.FAULT_REACTION_ACTIVE, 8: State.FAULT}
NODE = 1


class Subject:
    driver = "canopen"
    name = "canopen (the bridge against a simulated CiA 402 drive on a loopback bus)"
    #: The loopback bus delivers inside `recv`, so a change is in the next read.
    settle_s = 0.2

    def __init__(self) -> None:
        self.bridge: CanOpenBridge | None = None
        self.drive: SimulatedDrive | None = None
        self.bus: LoopbackBus | None = None

    def start(self) -> None:
        self.bus = LoopbackBus()
        master = self.bus.endpoint()
        self.drive = SimulatedDrive(self.bus.endpoint(), NODE)
        self.bus.attach(self.drive.poll)
        self.bridge = CanOpenBridge(master)
        config = json.dumps({"joints": [{"node": NODE, "name": "J1", "role": "right_arm.shoulder.pitch"}]})
        rc = self.bridge.open(config)
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated drive (rc {rc})")

    def restart(self) -> None:
        self.start()

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        assert self.drive is not None
        if vocabulary == "emcy_error_code":
            self._emcy(code)
        else:
            self.drive.state = STATES[code]
            self.drive.faulted = STATES[code] is State.FAULT

    def clear(self, vocabulary: str) -> None:
        """What a person at the drive does: reset it. The bridge never does (F26)."""
        assert self.drive is not None
        if vocabulary == "emcy_error_code":
            self._emcy(0)
        else:
            self.drive.state = State.OPERATION_ENABLED
            self.drive.faulted = False

    def _emcy(self, code: int) -> None:
        assert self.drive is not None
        self.drive.bus.send(Frame(0x080 + NODE, bytes([code & 0xFF, (code >> 8) & 0xFF, 0x01, 0, 0, 0, 0, 0])))

    def expected_code(self, vocabulary: str, code: int) -> int:
        """`node << 16 | code` for an emergency; for a state there is no code of the maker's, so the node alone."""
        return NODE << 16 | (code if vocabulary == "emcy_error_code" else 0)

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
