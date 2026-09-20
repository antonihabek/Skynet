"""Run the Fourier driver as a `python-host` child: `python -m skynet_fourier`."""

from __future__ import annotations

import skynet_rhal as rhal
from skynet_fourier.bridge import FourierDriver


def main() -> int:
    return rhal.serve(FourierDriver(), ceiling="guest", name="skynet-fourier",
                      version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
