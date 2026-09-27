"""Serve the LeRobot bridge to ``drivers/python-host``.

    python -m skynet_lerobot

Not run by hand — ``drivers/python-host`` spawns it and speaks the protocol in
:mod:`skynet_rhal` over stdin and stdout. Running it in a terminal is a legitimate way to poke at
the protocol, and the first thing it prints is the handshake.

**Which robot it opens** comes from the configuration the OS hands to the host's ``open()``, which is
forwarded here verbatim. With LeRobot installed that is a real robot; without it, and with
``SKYNET_LEROBOT_FAKE=1``, it is the fake from :mod:`skynet_lerobot.fake` — which is how
``skynet driver check`` exercises this driver end to end on a machine with no arm plugged into it.

The fake is opt-in through an environment variable rather than a fallback, because a driver that
silently substitutes a simulated body for a missing real one is a driver that will one day report a
successful grasp that never happened.
"""

from __future__ import annotations

import os
import sys

import skynet_rhal as rhal

from .bridge import LeRobotBridge, open_bridge
from .fake import FakeCameraArm


def main() -> int:
    fake = os.environ.get("SKYNET_LEROBOT_FAKE", "").strip().lower() in {"1", "true", "yes"}

    if fake:
        print(
            "skynet-lerobot: SKYNET_LEROBOT_FAKE is set, so this is the fake robot from "
            "skynet_lerobot.fake and not hardware. Nothing it reports is evidence about a machine.",
            file=sys.stderr,
            flush=True,
        )
        driver: object = LeRobotBridge(FakeCameraArm())
    else:
        # `open_bridge` needs the configuration, and the configuration arrives over the protocol
        # rather than on the command line. So the real path is a lazy driver: it constructs the
        # LeRobot robot the first time `open` is called, with the config it was given.
        driver = _LazyBridge()

    return rhal.serve(driver, ceiling="guest", name="lerobot-bridge", version="0.1.0")


class _LazyBridge:
    """A bridge that builds its robot from the configuration ``open`` supplies.

    Every method before ``open`` reports ``SKR_EPERM``, which is what the RHAL says about a body
    nobody has opened. A failure to construct the robot reports ``SKR_ENODEV`` with the reason on
    stderr, rather than an exception the host would see as a hung interpreter.
    """

    def __init__(self) -> None:
        self._inner: LeRobotBridge | None = None

    def open(self, config: str) -> int:
        try:
            self._inner = open_bridge(config)
        except Exception as error:  # noqa: BLE001
            print(f"skynet-lerobot: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_ENODEV
        return self._inner.open(config)

    def __getattr__(self, name: str):
        inner = self.__dict__.get("_inner")
        if inner is None:
            raise AttributeError(name)
        return getattr(inner, name)

    def close(self) -> int:
        return self._inner.close() if self._inner else rhal.SKR_OK

    def read_state(self, out, deadline_ns: int) -> int:
        return self._inner.read_state(out, deadline_ns) if self._inner else rhal.SKR_EPERM

    def write_command(self, cmd, deadline_ns: int) -> int:
        return self._inner.write_command(cmd, deadline_ns) if self._inner else rhal.SKR_EPERM

    def emergency_stop(self) -> int:
        return self._inner.emergency_stop() if self._inner else rhal.SKR_OK

    def release_stop(self) -> int:
        return self._inner.release_stop() if self._inner else rhal.SKR_OK

    def describe(self) -> str:
        if self._inner is None:
            raise RuntimeError("describe() before open()")
        return self._inner.describe()

    def read_sensor(self, name: str, out) -> int:
        return self._inner.read_sensor(name, out) if self._inner else rhal.SKR_EPERM


if __name__ == "__main__":
    raise SystemExit(main())
