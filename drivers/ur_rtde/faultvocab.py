"""The UR bridge's subject for `tools/fault-vocab/contract.py`: the safety status a controller reports, through the bridge's own read and write.

    PYTHONPATH=bindings/python:drivers/ur_rtde python3 tools/fault-vocab/contract.py drivers/ur_rtde/faultvocab.py

The substrate is the scripted controller this driver's own suite runs against (`selftest.py`'s `ScriptedRtde`, `ScriptedDashboard` and a clock
the test owns), so a code is *injected* the way the controller would publish it: in the next RTDE data package. **A scripted controller agrees
with whoever scripted it** (F15), and so does this table: what it holds is what the bridge does with a number, not what URSim or a UR does
with a collision. The live half is `selftest.py` against URSim, and it can only ever see NORMAL.
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

import selftest as suite
from skynet_ur import bridge as bridge_module
from skynet_ur.bridge import UrBridge


class _Sockets:
    """`socket` as the bridge names it, so the control program goes somewhere this test can read."""

    @staticmethod
    def create_connection(address: tuple, timeout: float = 0.0) -> suite.ScriptedScript:
        return suite.ScriptedScript(address)


class Subject:
    driver = "ur_rtde"
    name = "ur_rtde (the bridge against a scripted controller and a clock the test owns)"
    #: The scripted controller answers at once; a real controller's RTDE stream is 125 Hz.
    settle_s = 0.2

    def __init__(self) -> None:
        self.bridge: UrBridge | None = None
        self.safety = 1
        self._saved: tuple | None = None

    def start(self) -> None:
        clock = suite.Clock()
        suite.ScriptedRtde.clock = clock
        self._saved = (bridge_module.Rtde, bridge_module.Dashboard, bridge_module.socket, bridge_module.time)
        bridge_module.Rtde, bridge_module.Dashboard = suite.ScriptedRtde, suite.ScriptedDashboard
        bridge_module.socket, bridge_module.time = _Sockets, clock
        self.restart()

    def restart(self) -> None:
        suite.ScriptedRtde.made.clear()
        suite.ScriptedRtde.fail = None
        suite.ScriptedDashboard.made.clear()
        suite.ScriptedDashboard.safety = "NORMAL"
        self.safety = 1
        suite.ScriptedRtde.first = suite.sample_of(suite.Q0, suite.QD0)
        self.bridge = UrBridge()
        rc = self.bridge.open(json.dumps({"host": "10.9.8.7", "model": "UR5e"}))
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the scripted controller (rc {rc})")

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
        if self._saved is not None:
            bridge_module.Rtde, bridge_module.Dashboard, bridge_module.socket, bridge_module.time = self._saved

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        self.safety = code

    def clear(self, vocabulary: str) -> None:
        self.safety = 1

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None and self.bridge._rtde is not None
        self.bridge._rtde.publish(suite.sample_of(suite.Q0, suite.QD0, safety=self.safety))
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, state.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        return self.bridge.write_command(
            rhal.Command(mode=rhal.MODE_POSITION, values=list(suite.Q0), mask=[1] * 6), 0
        )


def subject() -> Subject:
    return Subject()
