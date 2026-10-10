"""The Rainbow bridge's subject for `tools/fault-vocab/contract.py`: the controller's message codes and the safety fields of its state packet, through the bridge's own read and write.

    PYTHONPATH=bindings/python:drivers/rainbow python3 tools/fault-vocab/contract.py drivers/rainbow/faultvocab.py

The substrate is `SimulatedController`, the one the suite uses: it says `error[code][N]` and `warn[code][N]` on the command port when it is told to and carries the packet's device error, software emergency stop, collision,
joint-word, SOS-state and mode fields as it is told to. **The simulator agrees with whoever wrote it** (F15): what this holds is what the bridge does with a code or a field the controller reports, and nothing about Rainbow
Robotics' firmware. The message table is the maker's own, generated from `rbpodo`'s `error_code.hpp` (`drivers/fault-vocabulary.json`, written by `sim/gen_vocab.py`).

Two things in how the bridge answers are why this subject is not the plainest one:

* The controller says an error once, so the bridge holds it until a person acknowledges it with the `clear_faults` motion (and the motion is refused while the packet still shows a cause). `clear` is that acknowledgement,
  made after the substrate's own cause is gone: what a person does at the pendant and then in the session.
* The controller says a warning after it has acknowledged the command it is about, so the bridge refuses the *next* command once (EINVAL, with the warning's text) and shows DEGRADED for two seconds. The contract asks that a
  degraded arm take commands, so after a warning `write` sends the command again when the first was refused for that reason, as a person who has read the refusal would. A refusal that goes on is still a refusal.
"""

from __future__ import annotations

import json
import time

import skynet_rhal as rhal

from skynet_rainbow import bridge as bridge_module
from skynet_rainbow.bridge import RainbowBridge
from skynet_rainbow.controller_sim import SimulatedController

LIMITS = [[-6.0, 6.0], [-3.0, 3.0], [-2.5, 2.5], [-6.0, 6.0], [-6.0, 6.0], [-6.0, 6.0]]
#: Two poses a long way apart, inside every joint's range: every command to take the arm "somewhere new" alternates between them.
POSES = ([-1.0] * 6, [1.0] * 6)
#: A joint word with nothing wrong: the document's RUN and INIT state bits.
JOINT_QUIET = 0b110
SOS_STATE_BIT = 12


class Subject:
    driver = "rainbow"
    name = "rainbow (the bridge against its simulated controller)"
    #: A packet every 10 ms carries the fields and the command port's lines are read between them: a few cycles, and the margin, show a changed code.
    settle_s = 1.0

    def __init__(self) -> None:
        self.sim: SimulatedController | None = None
        self.bridge: RainbowBridge | None = None
        self._pose = 0
        self._warned = False

    def start(self) -> None:
        self.stop()
        self.sim = SimulatedController("RB5-850", time_scale=20.0, simulation=True).start()
        self.bridge = RainbowBridge()
        config = json.dumps({"host": "127.0.0.1", "port": self.sim.command_port, "data_port": self.sim.data_port, "model": self.sim.model, "simulator": True, "limits": LIMITS, "rate_hz": 100, "timeout_ms": 120})
        rc = self.bridge.open(config)
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated controller (rc {rc})")
        # The controller's lines go to the connections it has when it says them, so the substrate waits until it has the command connection the bridge made.
        end = time.monotonic() + 5.0
        while time.monotonic() < end:
            rc, flags, _ = self.read()
            with self.sim._lock:  # noqa: SLF001
                connected = any(self.sim._is_command(c) for c in self.sim._connections)  # noqa: SLF001
            if rc == rhal.SKR_OK and not flags and connected:
                break
            time.sleep(0.02)
        self._warned = False

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
        sim = self.sim
        if vocabulary == "error_code":
            sim.raise_error(code)
        elif vocabulary == "warn_code":
            sim.raise_warn(code)
            self._warned = True
        elif vocabulary == "sos_flag":
            sim.sos = code
        elif vocabulary == "ems_flag":
            sim.ems = code
        elif vocabulary == "collision":
            sim.collision = code
        elif vocabulary == "joint_info":
            joint, bit = divmod(code, 256)
            if code:
                sim.joint_info[joint] = JOINT_QUIET | 1 << bit
        elif vocabulary == "sos_state":
            sim.chunk1 = code << SOS_STATE_BIT
        elif vocabulary == "mode":
            sim.mode_override = code
        else:
            raise SystemExit(f"faultvocab: no way to inject into {vocabulary!r}")

    def clear(self, vocabulary: str) -> None:
        """What a person does: remove the cause at the controller, then acknowledge what it said in the session (`clear_faults`, which the bridge refuses while the packet still shows a cause)."""
        assert self.sim is not None and self.bridge is not None
        sim = self.sim
        sim.sos = sim.ems = sim.collision = sim.chunk1 = 0
        sim.joint_info = [JOINT_QUIET] * 6
        sim.mode_override = None
        end = time.monotonic() + 3.0
        while True:
            rc, _ = self.bridge.extension(bridge_module.TOKEN, b"clear_faults")
            if rc == rhal.SKR_OK or time.monotonic() >= end:
                break
            time.sleep(0.02)
        self._warned = False

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        out = rhal.State()
        rc = self.bridge.read_state(out, 0)
        return rc, out.flags, out.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        rc = self._write_once()
        if rc == rhal.SKR_EINVAL and self._warned:
            self._warned = False
            rc = self._write_once()
        return rc

    def _write_once(self) -> int:
        assert self.bridge is not None
        self._pose ^= 1
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(POSES[self._pose]), mask=[1] * 6)
        return self.bridge.write_command(command, 0)

    def expected_code(self, vocabulary: str, code: int) -> int:
        """The word the bridge derives: a band above every code of the maker's table, and in it the value the controller reported."""
        if vocabulary == "sos_flag":
            return bridge_module.SOS_BAND | code
        if vocabulary == "ems_flag":
            return bridge_module.EMS_BAND | code
        if vocabulary == "collision":
            return bridge_module.COLLISION_BAND | code
        if vocabulary == "joint_info":
            return bridge_module.JOINT_BAND | code
        if vocabulary == "sos_state":
            return bridge_module.SOS_STATE
        if vocabulary == "mode":
            return bridge_module.MODE_BAND | code
        raise SystemExit(f"faultvocab: {vocabulary!r} has a verbatim word")


def subject() -> Subject:
    return Subject()
