"""Serve the Robotiq gripper bridge to ``drivers/python-host``.

    python -m skynet_robotiq

Not run by hand: ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over stdin and stdout.

**Which gripper it opens** comes from the configuration the OS hands to the host's ``open()``: a serial ``device`` (or a ``host`` and ``port``), the model, the speed and effort, the stop mode, and whether it is a simulation.
With ``SKYNET_ROBOTIQ_SIM=1`` set it runs a simulated gripper on a pseudo-terminal in this process and points the bridge at it, which is how ``skynet driver check`` exercises this driver end to end with no gripper and no
container. The simulator is opt-in through an environment variable rather than a fallback, for ``drivers/canopen``'s reason: a driver that silently substituted a simulated gripper for a port it could not open would
report fingers at the opening it was given, on a gripper that is not moving.
"""

from __future__ import annotations

import json
import os
import sys

import skynet_rhal as rhal

from .bridge import RobotiqBridge
from .gripper_sim import SimulatedGripper

#: The role a fixture asserts (§63.4: a role is a human's assertion, and the gripper carries none).
ROLE = "right_hand.gripper.grip"


class _SimulatedBridge(RobotiqBridge):
    """The bridge, with the simulated gripper's port and the model, stop mode, speed, effort and role a fixture asserts filled into whatever configuration arrives."""

    def __init__(self, gripper: SimulatedGripper) -> None:
        super().__init__()
        self._sim = gripper

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError:
            return super().open(config)
        if not isinstance(settings, dict):
            return super().open(config)
        if "host" not in settings:
            settings.setdefault("device", self._sim.path)
        settings.setdefault("model", "2F-85")
        settings.setdefault("stop_mode", "halt")
        settings.setdefault("speed", 0.1)
        settings.setdefault("effort", 0.5)
        settings.setdefault("simulator", True)
        settings.setdefault("roles", {"jaw": ROLE})
        return super().open(json.dumps(settings))


def main() -> int:
    bridge: RobotiqBridge = RobotiqBridge()
    if os.environ.get("SKYNET_ROBOTIQ_SIM", "").strip() == "1":
        gripper = SimulatedGripper(time_scale=20.0, activated=True).start()
        bridge = _SimulatedBridge(gripper)
        print(
            "skynet-robotiq: SKYNET_ROBOTIQ_SIM=1, so this is a simulated gripper on a pseudo-terminal and not a gripper. Nothing it reports is evidence about a machine, and the descriptor declares simulator: true.",
            file=sys.stderr, flush=True)
    return rhal.serve(bridge, ceiling="guest", name="robotiq", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
