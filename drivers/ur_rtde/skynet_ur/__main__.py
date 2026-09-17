"""Serve the UR bridge to ``drivers/python-host``.

    python -m skynet_ur

Not run by hand — ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal`
over stdin and stdout.

**Which robot it opens** comes from the configuration the OS hands to the host's ``open()``:
``{"host": "192.168.56.101"}``, or ``127.0.0.1`` for a URSim container. There is no default and no
fake. A UR driver with a built-in simulator would be a driver that comes up when the arm is
unplugged, and the first thing anybody would learn from it is that the robot is fine.
"""

from __future__ import annotations

import skynet_rhal as rhal

from .bridge import UrBridge


def main() -> int:
    return rhal.serve(UrBridge(), ceiling="guest", name="ur-rtde", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
