"""Serve the CANopen bridge to ``drivers/python-host``.

    python -m skynet_canopen

Not run by hand — ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal`
over stdin and stdout.

**Which bus it opens** comes from the configuration the OS hands to the host's ``open()``. With
``SKYNET_CANOPEN_SIM=<n>`` set it is an in-process bus with *n* simulated drives on it, which is how
``skynet driver check`` exercises this driver end to end on a machine with no CAN interface —
and how it is exercised on a machine with no CAN *kernel module*, which is most containers.

The simulator is opt-in through an environment variable rather than a fallback, for
``drivers/lerobot``'s reason and with more force here: a CANopen driver that silently substituted a
simulated drive for a bus it could not open would report an axis at the setpoint it was given, on a
robot that is not moving, and there would be nothing in the output to say so.
"""

from __future__ import annotations

import os
import sys

import skynet_rhal as rhal

from .bridge import CanOpenBridge
from .bus import LoopbackBus
from .slave import SimulatedDrive


def main() -> int:
    simulated = os.environ.get("SKYNET_CANOPEN_SIM", "").strip()
    bus = None
    if simulated:
        try:
            count = int(simulated)
        except ValueError:
            print(
                f"skynet-canopen: SKYNET_CANOPEN_SIM={simulated!r} is not a number of drives.",
                file=sys.stderr,
                flush=True,
            )
            return 1
        loopback = LoopbackBus()
        bus = loopback.endpoint()
        for node in range(1, count + 1):
            drive = SimulatedDrive(loopback.endpoint(), node)
            loopback.attach(drive.poll)
        print(
            f"skynet-canopen: SKYNET_CANOPEN_SIM={count}, so this is {count} simulated CiA 402 "
            "drive(s) on an in-process bus and not hardware. Nothing it reports is evidence about "
            "a machine, and the descriptor declares simulator: true.",
            file=sys.stderr,
            flush=True,
        )

    return rhal.serve(
        CanOpenBridge(bus), ceiling="guest", name="canopen-cia402", version="0.1.0"
    )


if __name__ == "__main__":
    raise SystemExit(main())
