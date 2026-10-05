"""Serve the ODrive bridge to ``drivers/python-host``.

    python -m skynet_odrive

Not run by hand — ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal`
over stdin and stdout.

**Which bus it opens** comes from the configuration the OS hands to the host's ``open()``. With
``SKYNET_ODRIVE_SIM=<n>`` set it is an in-process bus with *n* simulated axes on it, which is how
``skynet driver check`` exercises this driver end to end on a machine with no CAN interface —
and how it is exercised on a machine with no CAN *kernel module*, which is most containers.

The simulator is opt-in through an environment variable rather than a fallback, for
``drivers/lerobot``'s reason and with more force here: an ODrive driver that silently substituted
a simulated axis for a bus it could not open would report a shaft at the setpoint it was given,
on a motor that is not moving, and there would be nothing in the output to say so.
"""

from __future__ import annotations

import os
import sys

import skynet_rhal as rhal
from skynet_canopen.bus import LoopbackBus

from .axis import SimulatedAxis
from .bridge import ODriveBridge


def main() -> int:
    simulated = os.environ.get("SKYNET_ODRIVE_SIM", "").strip()
    bus = None
    if simulated:
        try:
            count = int(simulated)
        except ValueError:
            print(
                f"skynet-odrive: SKYNET_ODRIVE_SIM={simulated!r} is not a number of axes.",
                file=sys.stderr,
                flush=True,
            )
            return 1
        loopback = LoopbackBus()
        bus = loopback.endpoint()
        for node in range(count):
            axis = SimulatedAxis(loopback.endpoint(), node)
            loopback.attach(axis.poll)
        print(
            f"skynet-odrive: SKYNET_ODRIVE_SIM={count}, so this is {count} simulated ODrive "
            "axis/axes on an in-process bus and not hardware. Nothing it reports is evidence "
            "about "
            "a machine, and the descriptor declares simulator: true.",
            file=sys.stderr,
            flush=True,
        )

    return rhal.serve(
        ODriveBridge(bus), ceiling="guest", name="odrive-cansimple", version="0.1.0"
    )


if __name__ == "__main__":
    raise SystemExit(main())
