"""Serve the MAVLink bridge to ``drivers/python-host``.

    python -m skynet_mavlink

Not run by hand — ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over
stdin and stdout.

**Which vehicle it opens** comes from the configuration the OS hands to the host's ``open()``: a ``link``
(``tcp:127.0.0.1:5760``, ``udpin:0.0.0.0:14550``, ``serial:/dev/ttyACM0:921600``). With
``SKYNET_MAVLINK_SIM=1`` set it runs a simulated ArduPilot multirotor in this process on loopback and points the
bridge at it (``SKYNET_MAVLINK_SIM=px4``: a simulated PX4 multirotor), which is how ``skynet driver check``
exercises this driver end to end with no vehicle and no container.

The simulator is opt-in through an environment variable rather than a fallback, for ``drivers/canopen``'s
reason: a driver that silently substituted a simulated vehicle for one it could not reach would report a
pose that is not a machine's, and there would be nothing in the output to say so.
"""

from __future__ import annotations

import json
import os
import sys

import skynet_rhal as rhal

from .autopilot import SimulatedAutopilot
from .bridge import MavlinkBridge
from .frames import Autopilot


class _SimulatedBridge(MavlinkBridge):
    """The bridge, with the loopback vehicle's link filled into whatever configuration arrives."""

    def __init__(self, vehicle: SimulatedAutopilot) -> None:
        super().__init__()
        self._vehicle = vehicle

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError:
            return super().open(config)
        settings.setdefault("link", f"tcp:127.0.0.1:{self._vehicle.port}")
        return super().open(json.dumps(settings))


def main() -> int:
    bridge: MavlinkBridge = MavlinkBridge()
    simulated = os.environ.get("SKYNET_MAVLINK_SIM", "").strip()
    if simulated in ("1", "px4"):
        vehicle = SimulatedAutopilot(autopilot=Autopilot.PX4 if simulated == "px4" else Autopilot.ARDUPILOTMEGA).start()
        bridge = _SimulatedBridge(vehicle)
        print(
            f"skynet-mavlink: SKYNET_MAVLINK_SIM={simulated}, so this is a simulated {'PX4' if simulated == 'px4' else 'ArduPilot'} multirotor on "
            "loopback and not a vehicle. Nothing it reports is evidence about a machine, and the descriptor declares simulator: true.",
            file=sys.stderr,
            flush=True,
        )
    return rhal.serve(bridge, ceiling="guest", name="mavlink", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
