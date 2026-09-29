"""Serve the xArm bridge to ``drivers/python-host``.

    python -m skynet_xarm

Not run by hand — ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over
stdin and stdout.

**Which controller it opens** comes from the configuration the OS hands to the host's ``open()``: a
``host``, and optionally ``ports``. With ``SKYNET_XARM_SIM=1`` set it runs a simulated controller in this
process on loopback and points the bridge at it, which is how ``skynet driver check`` exercises this
driver end to end with no robot and no container.

The simulator is opt-in through an environment variable rather than a fallback, for
``drivers/canopen``'s reason: a driver that silently substituted a simulated arm for a controller it
could not reach would report an arm at the setpoint it was given, on a robot that is not moving, and
there would be nothing in the output to say so.
"""

from __future__ import annotations

import json
import os
import sys

import skynet_rhal as rhal

from .bridge import XArmBridge
from .controller import SimulatedController


class _SimulatedBridge(XArmBridge):
    """The bridge, with the loopback controller's address filled into whatever configuration arrives."""

    def __init__(self, controller: SimulatedController) -> None:
        super().__init__()
        self._controller = controller

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError:
            return super().open(config)
        settings.setdefault("host", "127.0.0.1")
        settings.setdefault("ports", dict(self._controller.ports))
        return super().open(json.dumps(settings))


def main() -> int:
    bridge: XArmBridge = XArmBridge()
    if os.environ.get("SKYNET_XARM_SIM", "").strip() == "1":
        controller = SimulatedController().start()
        bridge = _SimulatedBridge(controller)
        print(
            "skynet-xarm: SKYNET_XARM_SIM=1, so this is a simulated xArm 6 controller on loopback and "
            "not hardware. Nothing it reports is evidence about a machine, and the descriptor "
            "declares simulator: true.",
            file=sys.stderr,
            flush=True,
        )
    return rhal.serve(bridge, ceiling="guest", name="xarm", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
