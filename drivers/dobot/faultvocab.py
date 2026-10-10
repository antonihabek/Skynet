"""The Dobot bridge's subject for `tools/fault-vocab/contract.py`: the controller's alarm list and its RobotMode, through the bridge's own read and write.

    PYTHONPATH=bindings/python:drivers/dobot python3 tools/fault-vocab/contract.py drivers/dobot/faultvocab.py

The substrate is `SimulatedController`, the one the suite uses, which holds an alarm until it is cleared, reports it in `GetErrorID()` and in the feedback's mode and error flag, and can be made to report any RobotMode
(`force_mode`: a value the simulator would not otherwise reach, among them values the document does not list). **The simulator agrees with whoever wrote it** (F15): what this holds is what the bridge does with an alarm
or a mode the controller reports, and nothing about Dobot's firmware. The alarm table is the maker's own, generated from `demo/files/alarmController.py` in their Python client (`drivers/fault-vocabulary.json`).
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_dobot import bridge as bridge_module, frames
from skynet_dobot.bridge import DobotBridge
from skynet_dobot.controller_sim import SimulatedController

LIMITS = [[-6.0, 6.0], [-3.0, 3.0], [-2.5, 2.5], [-6.0, 6.0], [-6.0, 6.0], [-6.0, 6.0]]
#: Two poses a long way apart, inside every joint's range: every command to take the arm "somewhere new" alternates between them.
POSES = ([-1.0] * 6, [1.0] * 6)


class Subject:
    driver = "dobot"
    name = "dobot (the bridge against its simulated controller)"
    #: A packet every 8 ms carries the mode and the error flag; the alarm list is read in the same cycle that first shows an alarm. A few cycles, and the margin, show a changed code.
    settle_s = 1.0

    def __init__(self) -> None:
        self.sim: SimulatedController | None = None
        self.bridge: DobotBridge | None = None
        self._pose = 0

    def start(self) -> None:
        self.stop()
        self.sim = SimulatedController("CR5", time_scale=20.0, enabled=True).start()
        self.bridge = DobotBridge()
        config = json.dumps({"host": "127.0.0.1", "port": self.sim.dashboard_port, "feedback_port": self.sim.feedback_port, "model": "CR5", "stop_mode": "stop", "simulator": True, "limits": LIMITS, "payload_kg": 0.5,
                             "rate_hz": 100.0, "timeout_ms": 120})
        rc = self.bridge.open(config)
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated controller (rc {rc})")

    def restart(self) -> None:
        self.start()

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None
        if self.sim is not None:
            self.sim.stop()
            self.sim = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        assert self.sim is not None
        self.clear(vocabulary)
        if vocabulary == "alarm_id":
            self.sim.raise_alarm(code)
        else:
            self.sim.force_mode = code

    def clear(self, vocabulary: str) -> None:
        """What a person does at the controller: clear the alarm, release the stop, enable the arm. The bridge never does (`clear_errors` and `enable` are motions a person asks for)."""
        assert self.sim is not None
        sim = self.sim
        with sim._lock:  # noqa: SLF001
            sim._alarms, sim._persistent, sim._estop, sim._collision = [], set(), False, False  # noqa: SLF001
        sim.force_mode = None
        sim.set_mode_state(powered=True, enabled=True)

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        out = rhal.State()
        rc = self.bridge.read_state(out, 0)
        return rc, out.flags, out.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        self._pose ^= 1
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(POSES[self._pose]), mask=[1] * 6)
        return self.bridge.write_command(command, 0)

    def expected_code(self, vocabulary: str, code: int) -> int:
        """For a mode: 9 and 11 are the mode itself (an alarm whose identifier has not been read, and a collision), and a mode the document does not list is that mode above any alarm identifier."""
        if code in (frames.MODE_ERROR, frames.MODE_COLLISION):
            return code
        return bridge_module.UNLISTED_MODE_BASE | code


def subject() -> Subject:
    return Subject()
