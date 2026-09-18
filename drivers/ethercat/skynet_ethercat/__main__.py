"""Serve the EtherCAT bridge to `drivers/python-host` over stdin and stdout."""

from __future__ import annotations

import sys

import skynet_rhal as rhal

from .bridge import EtherCatBridge


def main() -> int:
    return rhal.serve(EtherCatBridge(), name="skynet-ethercat", version="0.1.0")


if __name__ == "__main__":
    sys.exit(main())
