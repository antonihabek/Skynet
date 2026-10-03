"""Serve the Reachy Mini bridge to ``drivers/python-host``.

    python -m skynet_reachy_mini

Not run by hand: ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over stdin and stdout.

**Which daemon it opens** comes from the configuration the OS hands to the host's ``open()``: a ``host`` and a ``port``, the stop mode, and the limits and speeds the daemon does not give. With
``SKYNET_REACHY_SIM=1`` set it runs a simulated daemon in this process on loopback and points the bridge at it, which is how ``skynet driver check`` exercises this driver end to end with no robot and
no container. The simulator is opt-in through an environment variable rather than a fallback, for ``drivers/canopen``'s reason: a driver that silently substituted a simulated robot for a daemon it could not
reach would report a head at the setpoint it was given, on a robot that is not moving.
"""

from __future__ import annotations

import json
import os
import sys

import skynet_rhal as rhal

from .bridge import ReachyMiniBridge
from .daemon_sim import RANGES, SPEEDS, SimulatedDaemon

#: The roles a fixture asserts (§63.4: a role is a human's assertion, and the daemon carries none).
ROLES = {
    "head_x": "head.platform.slide#0", "head_y": "head.platform.slide#1", "head_z": "head.platform.slide#2",
    "head_roll": "head.platform.roll", "head_pitch": "head.platform.pitch", "head_yaw": "head.platform.yaw", "body_yaw": "torso.body.yaw",
    "antenna_right": "aux.antenna_right.rotate", "antenna_left": "aux.antenna_left.rotate",
}


class _SimulatedBridge(ReachyMiniBridge):
    """The bridge, with the loopback daemon's address and the limits and speeds a fixture asserts filled into whatever configuration arrives."""

    def __init__(self, daemon: SimulatedDaemon) -> None:
        super().__init__()
        self._sim = daemon

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError:
            return super().open(config)
        if not isinstance(settings, dict):
            return super().open(config)
        settings.setdefault("host", "127.0.0.1")
        settings.setdefault("port", self._sim.port)
        settings.setdefault("stop_mode", "hold")
        settings.setdefault("limits", {name: list(RANGES[name]) for name in ("head_x", "head_y", "head_z", "antenna_right", "antenna_left")})
        settings.setdefault("speeds", dict(SPEEDS))
        settings.setdefault("roles", dict(ROLES))
        return super().open(json.dumps(settings))


def main() -> int:
    bridge: ReachyMiniBridge = ReachyMiniBridge()
    if os.environ.get("SKYNET_REACHY_SIM", "").strip() == "1":
        daemon = SimulatedDaemon().start()
        bridge = _SimulatedBridge(daemon)
        print(
            "skynet-reachy-mini: SKYNET_REACHY_SIM=1, so this is a simulated daemon on loopback and not a robot. Nothing it reports is evidence about a machine, and the descriptor declares simulator: true.",
            file=sys.stderr, flush=True)
    return rhal.serve(bridge, ceiling="guest", name="reachy-mini", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
