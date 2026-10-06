"""Serve the moteus bridge to ``drivers/python-host``.

    python -m skynet_moteus

Not run by hand — ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over stdin and stdout.

**Which bus it opens** comes from the configuration the OS hands to the host's ``open()``. With ``SKYNET_MOTEUS_SIM=N`` set it is an in-process bus with ``N``
simulated controllers on it (ids 1 to N, each with its own travel of ±0.25 revolutions), which is how ``skynet driver check`` exercises this driver end to end on a
machine with no CAN interface — and on one with no CAN *kernel module*, which is most containers.

The simulator is opt-in through an environment variable rather than a fallback, for ``drivers/odrive``'s reason: a moteus driver that silently substituted
simulated controllers for a bus it could not open would report axes at the positions it was given, on a machine that is not moving, and there would be nothing in
the output to say so.
"""

from __future__ import annotations

import os
import sys

import skynet_rhal as rhal

from .bridge import MoteusBridge
from .bus import LoopbackBus
from .sim import SimulatedMoteus


def main() -> int:
    simulated = os.environ.get("SKYNET_MOTEUS_SIM", "").strip()
    bus = None
    if simulated:
        try:
            count = int(simulated)
        except ValueError:
            count = 0
        if not 1 <= count <= 16:
            print(f"skynet-moteus: SKYNET_MOTEUS_SIM={simulated!r} is not a number of simulated controllers (1 to 16).", file=sys.stderr, flush=True)
            return 1
        loopback = LoopbackBus()
        bus = loopback.endpoint()
        controllers = SimulatedMoteus(loopback.endpoint(), list(range(1, count + 1)), position_min=-0.25, position_max=0.25)
        loopback.attach(controllers.poll)
        print(
            f"skynet-moteus: SKYNET_MOTEUS_SIM={simulated}, so these are {count} simulated moteus controllers on an in-process bus and not hardware. "
            "Nothing they report is evidence about a machine, and the descriptor declares simulator: true.",
            file=sys.stderr,
            flush=True,
        )

    return rhal.serve(MoteusBridge(bus), ceiling="guest", name="mjbots-moteus", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
