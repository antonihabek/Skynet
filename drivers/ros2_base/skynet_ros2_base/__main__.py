"""Serve the ROS 2 base driver to ``drivers/python-host``.

    python -m skynet_ros2_base

Not run by hand — ``drivers/python-host`` spawns it and speaks the protocol in :mod:`skynet_rhal` over stdin and stdout. It needs a ROS 2 environment sourced before it starts
(``. /opt/ros/jazzy/setup.sh``), because the driver imports ``rclpy``; the configuration the OS hands to ``open()`` names the base's two topics and the speeds it may be driven at.

With ``SKYNET_ROS2_BASE_SIM=1`` set it runs a simulated base in this process and connects to that instead of to ROS (``SKYNET_ROS2_BASE_SIM=twist``: one whose controller takes plain
``Twist`` rather than ``TwistStamped``), which is how ``skynet driver check`` exercises this driver end to end on a machine with no ROS and no robot.

The simulator is opt-in through an environment variable rather than a fallback, for ``drivers/canopen``'s reason: a driver that silently substituted a simulated base for one it could not
reach would report a pose that is not a machine's, and there would be nothing in the output to say so.
"""

from __future__ import annotations

import os
import sys

import skynet_rhal as rhal

from .bridge import Ros2BaseDriver, Settings
from .sim import SimulatedBase
from .transport import TWIST, TWIST_STAMPED, RclpyTransport


def _ros(settings: Settings) -> RclpyTransport:
    return RclpyTransport(settings.command_topic, settings.odometry_topic, domain_id=settings.domain_id, frame_id=settings.frame_id)


def main() -> int:
    make = _ros
    simulated = os.environ.get("SKYNET_ROS2_BASE_SIM", "").strip()
    if simulated in ("1", "twist"):
        kind = TWIST if simulated == "twist" else TWIST_STAMPED

        def make(settings: Settings) -> SimulatedBase:
            return SimulatedBase(command_type=kind, holonomic=settings.holonomic)

        print(
            f"skynet-ros2-base: SKYNET_ROS2_BASE_SIM={simulated}, so this is a simulated base ({kind}) in this process and not a robot. "
            "Nothing it reports is evidence about a machine, and the descriptor declares simulator: true unless the configuration says otherwise.",
            file=sys.stderr,
            flush=True,
        )
    return rhal.serve(Ros2BaseDriver(make), ceiling="guest", name="ros2-base", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())
