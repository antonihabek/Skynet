"""The OnRobot bridge's subject for `tools/fault-vocab/contract.py`: the status bits and the error code the maker's library names, through the bridge's own read and write.

    PYTHONPATH=bindings/python:drivers/onrobot python3 tools/fault-vocab/contract.py drivers/onrobot/faultvocab.py

The substrate is `SimulatedGripper`, the one the suite uses, which holds a status bit the way the registers hold it: in the status word, across commands, until a person deals with the cause (`heal`). **The simulator
agrees with whoever wrote it** (F15): what this holds is what the bridge does with a bit a gripper reports, and nothing about OnRobot's firmware. The bits are the ones the maker's library decodes
(`onrobot_tool_api/diagnostics.hpp`): an RG's two safety switches (pushed, triggered) and its safety DC error, its three-bit error code, and a 2FG's not-calibrated and linear-sensor-error bits. The library gives
no table of what an error code means, so the table says so. The fault word is the status word's fault bits as the maker lays them out, which is what the black box needs to look a bit up.

An RG vocabulary runs against a simulated RG2 and a 2FG one against a simulated 2FG7: the registers differ by family, and the subject opens the one the vocabulary is about.
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_onrobot import frames
from skynet_onrobot.bridge import OnRobotBridge
from skynet_onrobot.gripper_sim import SimulatedGripper

#: Two widths a long way apart, inside each family's stroke (metres): every command to take the gripper "somewhere new" alternates between them.
RG_POSES = (0.03, 0.09)
FG_POSES = (0.02, 0.06)
RG_VOCABULARIES = ("rg_safety_bits", "rg_error_code")
FG_VOCABULARIES = ("fg_status_bits",)


class Subject:
    driver = "onrobot"
    name = "onrobot (the bridge against its simulated gripper)"
    #: A reading every 20 ms carries the status word; a few of them, and the margin, show a changed bit.
    settle_s = 0.4

    def __init__(self) -> None:
        self.gripper: SimulatedGripper | None = None
        self.bridge: OnRobotBridge | None = None
        self.family = "rg"
        self._pose = 0

    def _open(self, family: str) -> None:
        self.stop()
        self.family = family
        model = "RG2" if family == "rg" else "2FG7"
        self.gripper = SimulatedGripper(model, time_scale=20.0).start()
        self.bridge = OnRobotBridge()
        settings = {"host": "127.0.0.1", "port": self.gripper.port, "model": model, "stop_mode": "stop", "simulator": True, "rate_hz": 50.0, "roles": {"jaw": "right_hand.gripper.grip"}}
        if family == "rg":
            settings.update({"force_n": 20.0, "limits": [0.0, 0.11]})
        else:
            settings.update({"force_n": 30.0, "speed": 50, "supply_power_w": 48})
        rc = self.bridge.open(json.dumps(settings))
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated {model} (rc {rc})")

    def start(self) -> None:
        self._open("rg")

    def restart(self) -> None:
        self._open(self.family)

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None
        if self.gripper is not None:
            self.gripper.stop()
            self.gripper = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        wanted = "fg" if vocabulary in FG_VOCABULARIES else "rg"
        if wanted != self.family or self.gripper is None:
            self._open(wanted)
        assert self.gripper is not None
        self.gripper.heal()
        if vocabulary == "rg_error_code":
            self.gripper.set_error(code=code)
        else:
            self.gripper.set_error(flags=code)

    def clear(self, vocabulary: str) -> None:
        """What a person does at the gripper: deal with the cause. The bridge never does (the maker's library has no call that clears one)."""
        assert self.gripper is not None
        self.gripper.heal()

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        out = rhal.State()
        rc = self.bridge.read_state(out, 0)
        return rc, out.flags, out.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        self._pose ^= 1
        poses = RG_POSES if self.family == "rg" else FG_POSES
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[poses[self._pose]], mask=[1])
        return self.bridge.write_command(command, 0)

    def expected_code(self, vocabulary: str, code: int) -> int:
        """The status word's fault bits as the maker lays them out: an RG's safety bits are bits 2 to 6 and its error code bits 7 to 9; a 2FG's not-calibrated and sensor-error bits are 3 and 4."""
        if vocabulary == "rg_error_code":
            return (code & frames.RG_ERROR_MASK) << frames.RG_ERROR_SHIFT
        return code


def subject() -> Subject:
    return Subject()
