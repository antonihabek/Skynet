"""Serve the Piper bridge to ``drivers/python-host``.

    python -m skynet_piper

Not run by hand — ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over stdin and stdout.

**Which bus it opens** comes from the configuration the OS hands to the host's ``open()``. With ``SKYNET_PIPER_SIM=1``
set it is an in-process bus with one simulated arm on it, which is how ``skynet driver check`` exercises this driver end
to end on a machine with no CAN interface — and on one with no CAN *kernel module*, which is most containers.
``SKYNET_PIPER_SIM=0`` is the same arm without a gripper.

The simulator is opt-in through an environment variable rather than a fallback, for ``drivers/odrive``'s reason: a Piper
driver that silently substituted a simulated arm for a bus it could not open would report an arm at the angles it was
given, on a machine that is not moving, and there would be nothing in the output to say so.
"""

from __future__ import annotations

import os
import sys

import skynet_rhal as rhal
from skynet_canopen.bus import LoopbackBus

from .arm import SimulatedPiper
from .bridge import PiperBridge


def main() -> int:
    simulated = os.environ.get("SKYNET_PIPER_SIM", "").strip()
    bus = None
    if simulated:
        if simulated not in ("0", "1"):
            print(
                f"skynet-piper: SKYNET_PIPER_SIM={simulated!r} is not 1 (an arm with its gripper) or 0 (without).",
                file=sys.stderr,
                flush=True,
            )
            return 1
        loopback = LoopbackBus()
        bus = loopback.endpoint()
        arm = SimulatedPiper(loopback.endpoint(), gripper=simulated == "1")
        loopback.attach(arm.poll)
        print(
            f"skynet-piper: SKYNET_PIPER_SIM={simulated}, so this is a simulated Piper on an in-process bus and not hardware. "
            "Nothing it reports is evidence about a machine, and the descriptor declares simulator: true.",
            file=sys.stderr,
            flush=True,
        )

    return rhal.serve(PiperBridge(bus), ceiling="guest", name="agilex-piper", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
