"""Serve the Franka bridge to `drivers/python-host` over stdin and stdout.

`SKYNET_FRANKA_SIM=1` starts the fake Control box in this process first and points the bridge at
it. That is opt-in rather than a fallback for the reason the LeRobot bridge gives about its own
fake: a driver that silently substitutes a simulated body for a missing real one will one day
report a motion that never happened.
"""

from __future__ import annotations

import os
import sys

import skynet_rhal as rhal

from .bridge import FrankaBridge
from .sim import FakeControlBox


def main() -> int:
    driver = FrankaBridge()
    box = None
    if os.environ.get("SKYNET_FRANKA_SIM"):
        hz = float(os.environ.get("SKYNET_FRANKA_SIM_HZ", "500"))
        box = FakeControlBox(hz=hz)
        port = box.start()
        # The bridge reads its host and port from the config JSON, so the fake's port is handed
        # over the same way an operator would hand over a real box's: through the environment,
        # which `skynet driver check --config` cannot reach.
        os.environ["SKYNET_FRANKA_PORT"] = str(port)
        print(f"skynet-franka: fake Control box on 127.0.0.1:{port} at {hz:g} Hz — this is not a "
              f"robot and nothing it reports is evidence about one.", file=sys.stderr, flush=True)
    try:
        return rhal.serve(driver, name="skynet-franka", version="0.1.0")
    finally:
        if box is not None:
            box.stop()


if __name__ == "__main__":
    sys.exit(main())
