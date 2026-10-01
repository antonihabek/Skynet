"""The Franka bridge's subject for `tools/fault-vocab/contract.py`: the 41 error flags and the robot mode, through the bridge's own read and write.

    PYTHONPATH=bindings/python:drivers/franka python3 tools/fault-vocab/contract.py drivers/franka/faultvocab.py

The substrate is `FakeControlBox`, the one the suite uses: a command port and a state stream at the box's own rate, with `errors` and `robot_mode`
set the way a reflex or a person's stop sets them. **The fake agrees with whoever wrote it** (F15): what this holds is what the bridge does with a
flag or a mode the controller reports, and nothing about a Franka's controller. The 41 error names and the 7 modes are Franka Robotics' own
(`drivers/fault-vocabulary.json`, generated from `libfranka-common`'s headers).
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_franka import protocol as fci
from skynet_franka.bridge import FrankaBridge
from skynet_franka.sim import FakeControlBox


class Subject:
    driver = "franka"
    name = "franka (the bridge against its fake Control box)"
    #: The box streams at 500 Hz here; a state change is in the next read, and the reader needs a moment to drain.
    settle_s = 0.5

    def __init__(self) -> None:
        self.box: FakeControlBox | None = None
        self.bridge: FrankaBridge | None = None

    def start(self) -> None:
        self.stop()
        self.box = FakeControlBox(hz=500.0)
        port = self.box.start()
        self.bridge = FrankaBridge()
        rc = self.bridge.open(json.dumps({"host": "127.0.0.1", "port": port}))
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the fake Control box (rc {rc})")

    def restart(self) -> None:
        self.start()

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None
        if self.box is not None:
            self.box.stop()
            self.box = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        assert self.box is not None
        if vocabulary == "errors":
            # The flag alone, with the robot mode as it was: a real arm is in Reflex when an error is set, and that is the `robot_mode`
            # vocabulary's row — what this holds is that a flag set in a benign mode is still a fault.
            self.box.errors[code - 1] = True
        else:
            self.box.robot_mode = code

    def clear(self, vocabulary: str) -> None:
        """What a person does: recover the arm. The driver never clears an error (F26's rule)."""
        assert self.box is not None
        self.box.clear_errors()

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, state.fault_code

    def write(self) -> int:
        assert self.bridge is not None and self.box is not None
        here = list(self.box.q)
        return self.bridge.write_command(rhal.Command(stamp_ns=0, seq=1, mode=rhal.MODE_POSITION, values=here, mask=[1] * fci.JOINTS), 0)


def subject() -> Subject:
    return Subject()
