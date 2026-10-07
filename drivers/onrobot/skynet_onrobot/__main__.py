"""Serve the OnRobot gripper bridge to ``drivers/python-host``.

    python -m skynet_onrobot

Not run by hand: ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over stdin and stdout.

**Which gripper it opens** comes from the configuration the OS hands to the host's ``open()``: a Compute Box (``host`` and ``port``) or a serial ``device``, the model, the force (and a 2FG's speed and supply budget),
the stop mode, and whether it is a simulation. With ``SKYNET_ONROBOT_SIM`` set (to ``1`` for an RG2, or to a model name) it runs a simulated gripper in this process, on a TCP port, and points the bridge at it, which is how
``skynet driver check`` exercises this driver end to end with no gripper and no container. The simulator is opt-in through an environment variable rather than a fallback, for ``drivers/canopen``'s reason: a driver that
silently substituted a simulated gripper for a Compute Box it could not reach would report a jaw at the width it was given, on a gripper that is not moving.
"""

from __future__ import annotations

import json
import os
import sys

import skynet_rhal as rhal

from .bridge import OnRobotBridge
from .frames import MODELS
from .gripper_sim import SimulatedGripper

#: The roles a fixture asserts (§63.4: a role is a human's assertion, and a gripper carries none).
ROLES = {"jaw": "right_hand.gripper.grip"}


class _SimulatedBridge(OnRobotBridge):
    """The bridge, with the simulated gripper's address and the model, stop mode, force, speed, budget, limits and roles a fixture asserts filled into whatever configuration arrives."""

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
        model = self._sim.model
        if "device" not in settings:
            settings.setdefault("host", "127.0.0.1")
            settings.setdefault("port", self._sim.port)
        settings.setdefault("model", model.name)
        settings.setdefault("stop_mode", "stop")
        settings.setdefault("simulator", True)
        settings.setdefault("force_n", 20.0 if model.family == "rg" else max(30.0, model.min_force_n or 0.0))
        if model.family == "fg":
            settings.setdefault("speed", 50)
            settings.setdefault("supply_power_w", 48)
        else:
            settings.setdefault("limits", [0.0, float(model.max_width_mm or 0.0) / 1000.0])
        settings.setdefault("roles", ROLES)
        return super().open(json.dumps(settings))


def main() -> int:
    bridge: OnRobotBridge = OnRobotBridge()
    choice = os.environ.get("SKYNET_ONROBOT_SIM", "").strip()
    if choice:
        model = choice if choice in MODELS else "RG2"
        gripper = SimulatedGripper(model, time_scale=20.0).start()
        bridge = _SimulatedBridge(gripper)
        print(
            f"skynet-onrobot: SKYNET_ONROBOT_SIM is set, so this is a simulated {model} on a TCP port and not a gripper. Nothing it reports is evidence about a machine, and the descriptor declares simulator: true.",
            file=sys.stderr, flush=True)
    return rhal.serve(bridge, ceiling="guest", name="onrobot", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
