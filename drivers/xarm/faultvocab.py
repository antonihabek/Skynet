"""The xArm bridge's subject for `tools/fault-vocab/contract.py`: the controller's error and warning codes, through the bridge's own read and write.

    PYTHONPATH=bindings/python:drivers/xarm python3 tools/fault-vocab/contract.py drivers/xarm/faultvocab.py

The substrate is `SimulatedController`, the one the suite uses, which raises an error the way the firmware does — the code, and the arm in its
STOP state — and holds a warning beside a running arm. **The simulator agrees with whoever wrote it** (F15): what this holds is what the bridge
does with a code the controller reports, and nothing about UFACTORY's firmware. The 41 error codes and 5 warning codes are UFACTORY's own table,
generated from their SDK (`drivers/fault-vocabulary.json`).
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_xarm import frames
from skynet_xarm.bridge import XArmBridge
from skynet_xarm.controller import SimulatedController

#: Where the arm is told to go: inside every joint's range on every model, and where this simulator starts.
HOME = [0.0] * 6


class Subject:
    driver = "xarm"
    name = "xarm (the bridge against its simulated controller)"
    #: The rich report, which carries the error and the warning, is 10 Hz; the fast one, which carries the state, is 100 Hz.
    settle_s = 1.5

    def __init__(self) -> None:
        self.controller: SimulatedController | None = None
        self.bridge: XArmBridge | None = None

    def start(self) -> None:
        self.stop()
        self.controller = SimulatedController().start()
        self.bridge = XArmBridge()
        config = json.dumps(
            {
                "host": "127.0.0.1",
                "ports": self.controller.ports,
                "joints": [{"name": f"joint{i + 1}", "role": None} for i in range(self.controller.axis)],
            }
        )
        rc = self.bridge.open(config)
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated controller (rc {rc})")

    def restart(self) -> None:
        self.start()

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None
        if self.controller is not None:
            self.controller.stop()
            self.controller = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        assert self.controller is not None
        if vocabulary == "error_code":
            self.controller.latch_error(code)
        else:
            self.controller.warn_code = code

    def clear(self, vocabulary: str) -> None:
        """What a person at the controller does: clean the error or the warning, then tell the arm to run again. The driver never does it."""
        controller = self.controller
        assert controller is not None
        if vocabulary == "error_code":
            controller.error_code = 0
            controller._set_state(bytes([frames.State.RUN]))
        else:
            controller.warn_code = 0

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, state.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        return self.bridge.write_command(rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(HOME), mask=[1] * 6), 0)


def subject() -> Subject:
    return Subject()
