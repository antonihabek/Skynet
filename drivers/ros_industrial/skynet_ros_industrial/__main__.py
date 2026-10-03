"""Serve the ROS-Industrial bridge to ``drivers/python-host``.

    python -m skynet_ros_industrial

Not run by hand — ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over stdin and stdout.

**Which controller it opens** comes from the configuration the OS hands to the host's ``open()``: a ``host``, optionally ``ports``, and the joints. With ``SKYNET_ROSI_SIM=1`` set it runs a
simulated controller in this process on loopback and points the bridge at it, which is how ``skynet driver check`` exercises this driver end to end with no robot and no container.

The simulator is opt-in through an environment variable rather than a fallback, for ``drivers/canopen``'s reason: a driver that silently substituted a simulated arm for a controller it could not
reach would report an arm at the setpoint it was given, on a robot that is not moving, and there would be nothing in the output to say so.
"""

from __future__ import annotations

import json
import os
import sys

import skynet_rhal as rhal

from .bridge import RosIndustrialBridge
from .controller import SimulatedController

#: The roles a fixture asserts for a six-joint arm (§63.4: a role is a human's assertion, and the protocol carries none).
ROLES = ("right_arm.shoulder.yaw", "right_arm.shoulder.pitch", "right_arm.elbow.flex", "right_arm.wrist.pitch", "right_arm.wrist.yaw", "right_arm.wrist.roll")


class _SimulatedBridge(RosIndustrialBridge):
    """The bridge, with the loopback controller's address and the six joints a fixture asserts filled into whatever configuration arrives."""

    def __init__(self, controller: SimulatedController) -> None:
        super().__init__()
        self._controller = controller

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError:
            return super().open(config)
        if not isinstance(settings, dict):
            return super().open(config)
        settings.setdefault("host", "127.0.0.1")
        settings.setdefault("ports", dict(self._controller.ports))
        settings.setdefault("simulator", True)
        if "joints" not in settings:
            settings["joints"] = [
                {"name": f"joint_{i + 1}", "role": ROLES[i], "min_rad": low, "max_rad": high, "max_speed_rad_s": self._controller.max_speed}
                for i, (low, high) in enumerate(self._controller.limits)
            ]
        return super().open(json.dumps(settings))


def main() -> int:
    bridge: RosIndustrialBridge = RosIndustrialBridge()
    if os.environ.get("SKYNET_ROSI_SIM", "").strip() == "1":
        controller = SimulatedController().start()
        bridge = _SimulatedBridge(controller)
        print(
            "skynet-ros-industrial: SKYNET_ROSI_SIM=1, so this is a simulated six-joint controller on loopback and not hardware. Nothing it reports is evidence about a machine, and the "
            "descriptor declares simulator: true.",
            file=sys.stderr,
            flush=True,
        )
    return rhal.serve(bridge, ceiling="guest", name="ros-industrial", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
