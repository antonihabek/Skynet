"""The Piper bridge's subject for `tools/fault-vocab/contract.py`: the arm's status, error word and driver bytes, through the bridge's read and write.

    PYTHONPATH=bindings/python:drivers/canopen:drivers/piper python3 tools/fault-vocab/contract.py drivers/piper/faultvocab.py

The substrate is `SimulatedPiper` on a `LoopbackBus`, the pair the suite uses. A code is staged on the arm the way the suite stages one —
assigned, the way the firmware would report it — and the next broadcast carries it; **the simulator agrees with whoever wrote it** (F15):
what this holds is what the bridge does with a status, a word or a byte an arm reports, and nothing about any maker's arm. The tables are
generated from AgileX's own SDK (`drivers/piper/vocab_gen.py`, into `drivers/fault-vocabulary.json`) and not retyped.

The fault word is composed, not copied, so four of the six vocabularies are `derived` and say how in `expected_code`.
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_canopen.bus import LoopbackBus
from skynet_piper import protocol as p
from skynet_piper.arm import SimulatedPiper
from skynet_piper.bridge import PiperBridge

#: The joint whose driver the per-driver vocabularies stage. Any joint would do; the suite covers all six.
JOINT = 1
GOOD = [0.2, 0.5, -0.5, 0.1, 0.1, 0.1, 0.03]


class Subject:
    driver = "piper"
    name = "piper (the bridge against a simulated arm on a loopback bus)"
    #: The loopback bus delivers inside `recv`, so a staged code is in the next read.
    settle_s = 0.2

    def __init__(self) -> None:
        self.bridge: PiperBridge | None = None
        self.arm: SimulatedPiper | None = None
        self.bus: LoopbackBus | None = None

    def start(self) -> None:
        self.bus = LoopbackBus()
        master = self.bus.endpoint()
        self.arm = SimulatedPiper(self.bus.endpoint())
        self.bus.attach(self.arm.poll)
        self.bridge = PiperBridge(master)
        rc = self.bridge.open(json.dumps({"speed_percent": 30, "gripper": True, "gripper_torque_nm": 1.0}))
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated arm (rc {rc})")

    def restart(self) -> None:
        self.start()

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        arm = self.arm
        assert arm is not None
        if vocabulary == "arm_status":
            arm.arm_status = code
        elif vocabulary == "error_word":
            arm.err_code = code
        elif vocabulary == "driver_status":
            arm.driver_flags[JOINT] = code
        elif vocabulary == "driver_enable":
            arm.enabled[JOINT] = bool(code)
        elif vocabulary == "gripper_status":
            arm.gripper_flags = code
        elif vocabulary == "gripper_enable":
            arm.enabled[6] = bool(code)
        elif vocabulary == "ctrl_mode":
            arm.ctrl_mode = code
        else:
            raise SystemExit(f"faultvocab: no way to stage the `{vocabulary}` vocabulary")

    def clear(self, vocabulary: str) -> None:
        """What a person at the arm does: clear it. The bridge never does."""
        arm = self.arm
        assert arm is not None
        if vocabulary == "arm_status":
            arm.arm_status = p.ARM_STATUS_NORMAL
        elif vocabulary == "error_word":
            arm.err_code = 0
        elif vocabulary == "driver_status":
            arm.driver_flags[JOINT] = 0
        elif vocabulary == "driver_enable":
            arm.enabled[JOINT] = True
        elif vocabulary == "gripper_status":
            arm.gripper_flags = 0
        elif vocabulary == "gripper_enable":
            arm.enabled[6] = True
        elif vocabulary == "ctrl_mode":
            arm.ctrl_mode = p.CTRL_CAN

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, state.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        return self.bridge.write_command(rhal.Command(mode=rhal.MODE_POSITION, values=list(GOOD), mask=[1] * len(GOOD)), 0)

    def expected_code(self, vocabulary: str, code: int) -> int:
        """The fault word the bridge composes for a code, for the vocabularies whose word is derived."""
        if vocabulary == "arm_status":
            return code << 16
        if vocabulary == "driver_status":
            return code << 24
        if vocabulary in ("driver_enable", "gripper_enable"):
            return 0 if code else (p.DRIVER_ENABLED << 24)
        if vocabulary == "gripper_status":
            return p.FAULT_GRIPPER if code else 0
        if vocabulary == "ctrl_mode":
            return 0 if code == p.CTRL_CAN else p.FAULT_NOT_CAN_CONTROL
        raise SystemExit(f"faultvocab: no derived word for the `{vocabulary}` vocabulary")


def subject() -> Subject:
    return Subject()
