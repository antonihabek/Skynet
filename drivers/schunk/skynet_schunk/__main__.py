"""Serve the SCHUNK gripper bridge to ``drivers/python-host``.

    python -m skynet_schunk

Not run by hand: ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over stdin and stdout.

**Which module it opens** comes from the configuration the OS hands to the host's ``open()``: a ``host`` and a ``port``, the stop mode, the speed, and whether it is a simulation. With ``SKYNET_SCHUNK_SIM=1`` set it
runs a simulated module in this process on loopback and points the bridge at it, which is how ``skynet driver check`` exercises this driver end to end with no gripper and no container. The simulator is opt-in
through an environment variable rather than a fallback, for ``drivers/canopen``'s reason: a driver that silently substituted a simulated gripper for a module it could not reach would report a jaw at the setpoint it
was given, on a gripper that is not moving.
"""

from __future__ import annotations

import json
import os
import sys

import skynet_rhal as rhal

from .bridge import SchunkBridge
from .gripper_sim import SimulatedGripper

#: The role a fixture asserts (§63.4: a role is a human's assertion, and the module carries none).
ROLE = "right_hand.gripper.grip"


class _SimulatedBridge(SchunkBridge):
    """The bridge, with the loopback module's address and the stop mode, speed and role a fixture asserts filled into whatever configuration arrives."""

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
        settings.setdefault("host", "127.0.0.1")
        settings.setdefault("port", self._sim.port)
        settings.setdefault("stop_mode", "controlled")
        settings.setdefault("speed", 0.02)
        settings.setdefault("simulator", True)
        settings.setdefault("roles", {"jaw": ROLE})
        return super().open(json.dumps(settings))


def main() -> int:
    bridge: SchunkBridge = SchunkBridge()
    if os.environ.get("SKYNET_SCHUNK_SIM", "").strip() == "1":
        gripper = SimulatedGripper(time_scale=20.0).start()
        bridge = _SimulatedBridge(gripper)
        print(
            "skynet-schunk: SKYNET_SCHUNK_SIM=1, so this is a simulated module on loopback and not a gripper. Nothing it reports is evidence about a machine, and the descriptor declares simulator: true.",
            file=sys.stderr, flush=True)
    return rhal.serve(bridge, ceiling="guest", name="schunk", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
