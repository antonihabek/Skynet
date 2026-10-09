"""Serve the LEAP Hand bridge to ``drivers/python-host``.

    python -m skynet_leap

Not run by hand: ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over stdin and stdout.

**Which hand it opens** comes from the configuration the OS hands to the host's ``open()``: a serial ``device`` (or a gateway's ``host`` and ``port``), the variant, the current limit and gains, the stop mode, and
whether it is a simulation. With ``SKYNET_LEAP_SIM=1`` set it runs a simulated hand on a pseudo-terminal in this process and points the bridge at it, which is how ``skynet driver check`` exercises this driver end to
end with no hand and no container. The simulator is opt-in through an environment variable rather than a fallback, for ``drivers/canopen``'s reason: a driver that silently substituted a simulated hand for a port it
could not open would report fingers at the angles it was given, on a hand that is not moving.
"""

from __future__ import annotations

import json
import os
import sys

import skynet_rhal as rhal

from .bridge import LeapBridge
from .frames import JOINTS
from .hand_sim import SimulatedHand

#: The roles a fixture asserts (§63.4: a role is a human's assertion, and a hand carries none). §5.1's vocabulary standardises no names for the links of a finger, so these are this fixture's: the link is the finger's
#: joint, the axis is `abduct` for the side-to-side MCP and `flex` for the rest.
ROLES = {name: f"right_hand.{name.rsplit('_', 1)[0] if name.endswith(('side', 'forward')) else name}.{'abduct' if name.endswith('side') else 'flex'}" for name in JOINTS}


class _SimulatedBridge(LeapBridge):
    """The bridge, with the simulated hand's port and the variant, current, gains, stop mode and roles a fixture asserts filled into whatever configuration arrives."""

    def __init__(self, hand: SimulatedHand) -> None:
        super().__init__()
        self._sim = hand

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError:
            return super().open(config)
        if not isinstance(settings, dict):
            return super().open(config)
        if "host" not in settings:
            settings.setdefault("device", self._sim.path)
        settings.setdefault("variant", "full")
        settings.setdefault("stop_mode", "hold")
        settings.setdefault("current_ma", 350)
        settings.setdefault("gains", {"kp": 600, "ki": 0, "kd": 200})
        settings.setdefault("simulator", True)
        settings.setdefault("roles", ROLES)
        return super().open(json.dumps(settings))


def main() -> int:
    bridge: LeapBridge = LeapBridge()
    if os.environ.get("SKYNET_LEAP_SIM", "").strip() == "1":
        hand = SimulatedHand("full", time_scale=20.0).start()
        bridge = _SimulatedBridge(hand)
        print(
            "skynet-leap: SKYNET_LEAP_SIM=1, so this is a simulated hand on a pseudo-terminal and not a hand. Nothing it reports is evidence about a machine, and the descriptor declares simulator: true.",
            file=sys.stderr, flush=True)
    return rhal.serve(bridge, ceiling="guest", name="leap", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
