"""Serve the Rainbow bridge to ``drivers/python-host``.

    python -m skynet_rainbow

Not run by hand: ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over stdin and stdout.

**Which arm it opens** comes from the configuration the OS hands to the host's ``open()``: the control box's ``host`` (and the ports, if they are not the factory's), the model, the joint limits, the servo parameters, and
whether it is a simulation. With ``SKYNET_RAINBOW_SIM=1`` set it runs a simulated controller on two loopback ports in this process and points the bridge at it, which is how ``skynet driver check`` exercises this driver end
to end with no arm and no container. The simulator is opt-in through an environment variable rather than a fallback, for ``drivers/canopen``'s reason: a driver that silently substituted a simulated controller for an address
it could not reach would report joints at the angles it was given, on an arm that is not moving.
"""

from __future__ import annotations

import json
import os
import sys

import skynet_rhal as rhal

from .bridge import RainbowBridge
from .controller_sim import SimulatedController
from .frames import JOINTS

#: The roles a fixture asserts (§63.4: a role is a human's assertion, and an arm carries none). The names are the ones `drivers/ros_industrial`'s fixture uses for a six-axis arm, in the order an RB's joints are numbered (the base
#: yaw, the shoulder and elbow, then the three wrist axes): this fixture's assertion, not the maker's.
ROLES = dict(zip(JOINTS, ("right_arm.shoulder.yaw", "right_arm.shoulder.pitch", "right_arm.elbow.flex", "right_arm.wrist.pitch", "right_arm.wrist.yaw", "right_arm.wrist.roll")))
#: The fixture's joint ranges, in radians: a turn either way for the joints that have one, narrower for the elbow. Not any arm's manual; the operator's, as a real configuration's are.
LIMITS = [[-6.0, 6.0], [-3.0, 3.0], [-2.5, 2.5], [-6.0, 6.0], [-6.0, 6.0], [-6.0, 6.0]]


class _SimulatedBridge(RainbowBridge):
    """The bridge, with the simulated controller's address and the model, limits and roles a fixture asserts filled into whatever configuration arrives."""

    def __init__(self, controller: SimulatedController) -> None:
        super().__init__()
        self._sim = controller

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError:
            return super().open(config)
        if not isinstance(settings, dict):
            return super().open(config)
        settings.setdefault("host", "127.0.0.1")
        settings.setdefault("port", self._sim.command_port)
        settings.setdefault("data_port", self._sim.data_port)
        settings.setdefault("model", "RB5-850")
        settings.setdefault("limits", LIMITS)
        settings.setdefault("simulator", True)
        settings.setdefault("roles", ROLES)
        return super().open(json.dumps(settings))


def main() -> int:
    bridge: RainbowBridge = RainbowBridge()
    if os.environ.get("SKYNET_RAINBOW_SIM", "").strip() == "1":
        controller = SimulatedController("RB5-850", time_scale=20.0, simulation=True).start()
        bridge = _SimulatedBridge(controller)
        print(
            "skynet-rainbow: SKYNET_RAINBOW_SIM=1, so this is a simulated controller on loopback ports and not an arm. Nothing it reports is evidence about a machine, and the descriptor declares simulator: true.",
            file=sys.stderr, flush=True)
    return rhal.serve(bridge, ceiling="guest", name="rainbow", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
