"""Serve the Tello bridge to ``drivers/python-host``.

    python -m skynet_tello

Not run by hand: ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over stdin and stdout.

**Which drone it opens** comes from the configuration the OS hands to the host's ``open()``: the drone's ``host``, the stop mode and whether it is a simulation. With ``SKYNET_TELLO_SIM=1`` set it runs a simulated Tello on loopback
ports in this process and points the bridge at it, which is how ``skynet driver check`` exercises this driver end to end with no drone. The simulator is opt-in through an environment variable rather than a fallback, for
``drivers/canopen``'s reason: a driver that silently substituted a simulated drone for an address it could not reach would report a height on a drone that is not flying.
"""

from __future__ import annotations

import json
import os
import socket
import sys

import skynet_rhal as rhal

from .bridge import TelloBridge
from .drone_sim import DroneSimulator


def _free_udp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


class _SimulatedBridge(TelloBridge):
    """The bridge, with the simulated drone's address filled into whatever configuration arrives."""

    def __init__(self, drone: DroneSimulator) -> None:
        super().__init__()
        self._sim = drone

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError:
            return super().open(config)
        if not isinstance(settings, dict):
            return super().open(config)
        settings.setdefault("host", "127.0.0.1")
        settings.setdefault("port", self._sim.command_port)
        settings.setdefault("state_port", self._sim.state_port)
        settings.setdefault("simulator", True)
        settings.setdefault("stop_mode", "hover")
        return super().open(json.dumps(settings))


def main() -> int:
    bridge: TelloBridge = TelloBridge()
    if os.environ.get("SKYNET_TELLO_SIM", "").strip() == "1":
        drone = DroneSimulator(time_scale=20.0, state_port=_free_udp_port()).start()
        bridge = _SimulatedBridge(drone)
        print("skynet-tello: SKYNET_TELLO_SIM=1, so this is a simulated Tello on loopback ports and not a drone. Nothing it reports is evidence about a machine, and the descriptor declares simulator: true.",
              file=sys.stderr, flush=True)
    return rhal.serve(bridge, ceiling="guest", name="tello", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
