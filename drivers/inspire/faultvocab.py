"""The Inspire bridge's subject for `tools/fault-vocab/contract.py`: the error and status bytes the maker's manuals define, through the bridge's own read and write.

    PYTHONPATH=bindings/python:drivers/inspire python3 tools/fault-vocab/contract.py drivers/inspire/faultvocab.py

The substrate is `SimulatedHand`, the one the suite uses, which holds an error the way the manuals describe: in the error byte of one actuator, across commands, until CLEAR_ERROR clears what is clearable (or, for
over-temperature, until the actuator cools). **The simulator agrees with whoever wrote it** (F15): what this holds is what the bridge does with a byte a hand reports, and nothing about Inspire's firmware. The
codes and their meanings are the manuals', generated from their text by `vocab_gen.py` (`drivers/fault-vocabulary.json`). The fault word is the lowest-numbered joint that holds a fault, its status byte and its error
byte (the joint is counted from 1), which is what the black box needs to look the byte up.
"""

from __future__ import annotations

import json
import math

import skynet_rhal as rhal

from skynet_inspire import frames
from skynet_inspire.bridge import InspireBridge
from skynet_inspire.hand_sim import SimulatedHand

#: Two poses a long way apart, in the middle of every joint's range: every command to take the hand "somewhere new" alternates between them.
POSES = ([0.25] * 6, [0.75] * 6)


def _pose(fraction: float) -> list[float]:
    profile = frames.PROFILES["RH56"]
    return [math.radians(low + (high - low) * fraction) for low, high in profile.angle_deg]


class Subject:
    driver = "inspire"
    name = "inspire (the bridge against its simulated hand)"
    #: A reading every 20 ms carries both bytes; a few of them, and the margin, show a changed code.
    settle_s = 0.4

    def __init__(self) -> None:
        self.hand: SimulatedHand | None = None
        self.bridge: InspireBridge | None = None
        self._pose = 0

    def start(self) -> None:
        self.stop()
        self.hand = SimulatedHand(time_scale=20.0).start()
        self.bridge = InspireBridge()
        config = json.dumps({"device": self.hand.path, "protocol": "private", "model": "RH56", "stop_mode": "hold", "speed": 0.5, "force_g": 300, "simulator": True, "rate_hz": 50.0})
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
        self.hand.heal()
        if vocabulary == "error_byte":
            if code:
                self.hand.set_fault(0, error=code)
        else:
            self.hand.hold_status(0, code)

    def clear(self, vocabulary: str) -> None:
        """What a person does at the hand: clear it (or let it cool). The bridge never does (the `clear_errors` motion is a person's)."""
        assert self.hand is not None
        self.hand.heal()

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        out = rhal.State()
        rc = self.bridge.read_state(out, 0)
        return rc, out.flags, out.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        self._pose ^= 1
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=_pose(0.25 if self._pose else 0.75), mask=[1] * 6)
        return self.bridge.write_command(command, 0)

    def expected_code(self, vocabulary: str, code: int) -> int:
        """Joint 1 (counted from 1), the status byte the hand shows and the error byte: the injected byte is the error byte for `error_byte` and the status byte for `status_byte`."""
        assert self.hand is not None
        status = self.hand.statuses[0]
        error = code if vocabulary == "error_byte" else 0
        return 1 << 16 | status << 8 | error


def subject() -> Subject:
    return Subject()
