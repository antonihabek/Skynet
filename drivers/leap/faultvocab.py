"""The LEAP bridge's subject for `tools/fault-vocab/contract.py`: the Hardware_Error_Status byte the servos' e-manual defines, through the bridge's own read and write.

    PYTHONPATH=bindings/python:drivers/leap python3 tools/fault-vocab/contract.py drivers/leap/faultvocab.py

The substrate is `SimulatedHand`, the one the suite uses, which holds a hardware error the way the e-manual describes: in the error byte of one servo, until the servo is rebooted (the `clear_errors` motion, which is a
person's), and which switches that servo's torque off for the bits in its Shutdown register. **The simulator agrees with whoever wrote it** (F15): what this holds is what the bridge does with a byte a servo reports,
and nothing about ROBOTIS's firmware. The codes and their meanings are the e-manual's (`drivers/fault-vocabulary.json`). The fault word is `(servo + 1) << 16 | the error byte` of the lowest-numbered servo that holds one.
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_leap import frames
from skynet_leap.bridge import LeapBridge
from skynet_leap.hand_sim import SimulatedHand

#: Two poses a long way apart, in the middle of every joint's range: every command to take the hand "somewhere new" alternates between them.
POSES = ([lo + 0.25 * (hi - lo) for lo, hi in zip(frames.LIMITS_MIN, frames.LIMITS_MAX)], [lo + 0.75 * (hi - lo) for lo, hi in zip(frames.LIMITS_MIN, frames.LIMITS_MAX)])


class Subject:
    driver = "leap"
    name = "leap (the bridge against its simulated hand)"
    #: A reading every 20 ms carries the byte; a few of them, and the margin, show a changed code.
    settle_s = 0.4

    def __init__(self) -> None:
        self.hand: SimulatedHand | None = None
        self.bridge: LeapBridge | None = None
        self._pose = 0

    def start(self) -> None:
        self.stop()
        self.hand = SimulatedHand("full", time_scale=20.0).start()
        self.bridge = LeapBridge()
        config = json.dumps({"device": self.hand.path, "variant": "full", "stop_mode": "hold", "current_ma": 350, "gains": {"kp": 600, "ki": 0, "kd": 200}, "side_scale": 0.75, "simulator": True,
                             "rate_hz": 50.0, "timeout_ms": 100})
        rc = self.bridge.open(config)
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated hand (rc {rc})")

    def restart(self) -> None:
        self.start()

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None
        if self.hand is not None:
            self.hand.stop()
            self.hand = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        assert self.hand is not None
        self.clear(vocabulary)
        if code:
            self.hand.fault(0, code)

    def clear(self, vocabulary: str) -> None:
        """What a person does at the hand: reboot the servo and switch its torque back on. The bridge never does (the `clear_errors` and `enable_torque` motions are a person's)."""
        assert self.hand is not None
        self.hand.clear_faults()
        self.hand.set_register(0, 64, b"\x01")

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        out = rhal.State()
        rc = self.bridge.read_state(out, 0)
        return rc, out.flags, out.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        self._pose ^= 1
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(POSES[self._pose]), mask=[1] * 16)
        return self.bridge.write_command(command, 0)

    def expected_code(self, vocabulary: str, code: int) -> int:
        """Servo 0, counted from 1, and the error byte."""
        return 1 << 16 | code


def subject() -> Subject:
    return Subject()
