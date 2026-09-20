"""Run the Pudu driver as a `python-host` child: `python -m skynet_pudu`."""

from __future__ import annotations

import skynet_rhal as rhal
from skynet_pudu.bridge import PuduDriver


def main() -> int:
    return rhal.serve(PuduDriver(), ceiling="guest", name="skynet-pudu", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
