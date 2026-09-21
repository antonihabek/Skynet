"""Run the Spot driver as a `python-host` child: `python -m skynet_spot`."""

from __future__ import annotations

import skynet_rhal as rhal
from skynet_spot.bridge import SpotDriver


def main() -> int:
    return rhal.serve(SpotDriver(), ceiling="guest", name="skynet-spot", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
