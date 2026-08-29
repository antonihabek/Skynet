"""Serve the MuJoCo bridge to ``drivers/python-host``.

    python -m skynet_mujoco

Not run by hand — ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal`
over stdin and stdout. Running it in a terminal is a legitimate way to poke at the protocol, and the
first thing it prints is the handshake.

**Which scene it opens** comes from the configuration the OS hands to the host's ``open()``, which is
forwarded here verbatim, or from ``SKYNET_MUJOCO_SCENE`` when that configuration names none — which
is how ``skynet driver check`` exercises this driver, since it opens a driver with an empty
configuration.

There is no fallback scene. A simulator driver with no scene has no body, and inventing one would
describe a robot nobody asked for and then report conformance results about it.
"""

from __future__ import annotations

import sys

import skynet_rhal as rhal

from .bridge import MujocoBridge, TOOL_VERSION


def main() -> int:
    print(
        "skynet-mujoco: this is a simulator. Nothing it reports is evidence about a machine, the "
        "descriptor declares simulator: true, and the tier is guest whatever the numbers say.",
        file=sys.stderr,
        flush=True,
    )
    return rhal.serve(
        MujocoBridge(), ceiling="guest", name="mujoco-bridge", version=TOOL_VERSION
    )


if __name__ == "__main__":
    raise SystemExit(main())
