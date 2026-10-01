"""The EtherCAT bridge's subject for `tools/fault-vocab/contract.py`: a drive's CiA 402 state on the ring, through the bridge's own read and write.

    PYTHONPATH=bindings/python:drivers/ethercat:drivers/canopen python3 tools/fault-vocab/contract.py drivers/ethercat/faultvocab.py

The substrate is `EtherCatDrive` on a `LoopbackSegment`, the pair the suite uses. **The simulator agrees with whoever wrote it** (F15): what this
holds is what the bridge does with a state a drive reports, and nothing about any maker's drive. Only the statusword is covered: the drive's own
error code travels by the mailbox, which is acyclic (`drivers/fault-vocabulary.json`, not_covered).
"""

from __future__ import annotations

import skynet_rhal as rhal

import selftest as suite
from skynet_canopen import cia402
from skynet_ethercat.bridge import EtherCatBridge
from skynet_ethercat.segment import LoopbackSegment

STATES = {55: cia402.State.OPERATION_ENABLED, 15: cia402.State.FAULT_REACTION_ACTIVE, 8: cia402.State.FAULT}


class Subject:
    driver = "ethercat"
    name = "ethercat (the bridge against a simulated CiA 402 drive on a loopback segment)"
    #: The loopback segment answers inside the cycle, so a change is in the next read.
    settle_s = 0.2

    def __init__(self) -> None:
        self.bridge: EtherCatBridge | None = None
        self.ring: list = []

    def start(self) -> None:
        self.ring = suite.drives(1)
        self.bridge = EtherCatBridge()
        self.bridge._make_segment = lambda settings, expected: LoopbackSegment(self.ring)  # noqa: SLF001
        rc = self.bridge.open(suite.config(1))
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated ring (rc {rc})")

    def restart(self) -> None:
        self.stop()
        self.start()

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        drive = self.ring[0]
        if STATES[code] is cia402.State.FAULT:
            drive.latch_fault(0x2310)
        else:
            drive.state = STATES[code]

    def clear(self, vocabulary: str) -> None:
        """What a person at the drive does: reset it. The bridge never does (F26)."""
        drive = self.ring[0]
        drive.fault_code = 0
        drive.state = cia402.State.OPERATION_ENABLED

    def expected_code(self, vocabulary: str, code: int) -> int:
        """The drive's position on the ring plus one: the cyclic image carries the statusword and not the drive's own code."""
        return 1

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, state.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        return self.bridge.write_command(suite.command(rhal.MODE_POSITION, [0.0]), 0)


def subject() -> Subject:
    return Subject()
