"""The Robotiq bridge's subject for `tools/fault-vocab/contract.py`: the gripper's own fault codes and the optional controller's, through the bridge's own read and write.

    PYTHONPATH=bindings/python:drivers/robotiq python3 tools/fault-vocab/contract.py drivers/robotiq/faultvocab.py

The substrate is `SimulatedGripper`, the one the suite uses, which holds a fault the way the maker's own model does: in the status block's fault byte, across commands, until rACT falls. **The simulator agrees with
whoever wrote it** (F15): what this holds is what the bridge does with a code the gripper reports, and nothing about Robotiq's firmware. The codes, their meanings and their severities are the maker's own, generated
from `fault_status.hpp` (`drivers/fault-vocabulary.json`); `live.py` stages the same bytes in the maker's own register model.
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_robotiq.bridge import RobotiqBridge
from skynet_robotiq.gripper_sim import SimulatedGripper

#: Two openings inside the stroke: every command to take the fingers "somewhere new" alternates between them.
POSES = (0.02, 0.06)


class Subject:
    driver = "robotiq"
    name = "robotiq (the bridge against its simulated gripper)"
    #: An exchange every 20 ms carries the fault byte; a few of them, and the margin, show a changed code.
    settle_s = 0.4

    def __init__(self) -> None:
        self.gripper: SimulatedGripper | None = None
        self.bridge: RobotiqBridge | None = None
        self._pose = 0

    def start(self) -> None:
        self.stop()
        self.gripper = SimulatedGripper(time_scale=20.0, activated=True).start()
        self.bridge = RobotiqBridge()
        config = json.dumps({"device": self.gripper.path, "model": "2F-85", "stop_mode": "halt", "speed": 0.1, "effort": 0.5, "simulator": True, "rate_hz": 50.0,
                             "roles": {"jaw": "right_hand.gripper.grip"}})
        rc = self.bridge.open(config)
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated gripper (rc {rc})")

    def restart(self) -> None:
        self.start()

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None
        if self.gripper is not None:
            self.gripper.stop()
            self.gripper = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        if vocabulary == "gripper_fault" and code == 0x09:
            return ("the gripper raises 0x09 itself, when it has heard nothing for a second, and takes it down as the next request arrives (the maker: \"No communication during at least 1 second\"): it cannot be held "
                    "while the driver is exchanging with it every period, so the simulated gripper clears it at once and a held 0x09 would measure nothing")
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        assert self.gripper is not None
        if vocabulary == "gripper_fault":
            self.gripper.set_fault(gripper=code)
        else:
            self.gripper.set_fault(controller=code)

    def clear(self, vocabulary: str) -> None:
        """What a person does at the gripper: reset it. The driver never does (the `recover` motion is a person's)."""
        assert self.gripper is not None
        self.gripper.set_fault(0, 0)

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        out = rhal.State()
        rc = self.bridge.read_state(out, 0)
        return rc, out.flags, out.fault_code

    def write(self) -> int:
        assert self.bridge is not None
        self._pose ^= 1
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[POSES[self._pose]], mask=[1])
        return self.bridge.write_command(command, 0)

    def expected_code(self, vocabulary: str, code: int) -> int:
        """The word is the whole fault byte: the gripper's own code is its low nibble, and the controller's the high one."""
        return code if vocabulary == "gripper_fault" else code << 4


def subject() -> Subject:
    return Subject()
