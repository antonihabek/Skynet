"""skynet-ros-industrial — a robot arm behind a ROS-Industrial `simple_message` server, as one Skynet body.

`DRY-COVERAGE-PLAN.md` Phase 12 item 12.E.6. ROS-Industrial publish `simple_message`, the protocol their controller-side servers speak (MotoROS on Yaskawa Motoman, `abb_driver`'s RAPID
server, `fanuc_driver`, Stäubli's and others), as source (`industrial_core`, BSD-3-Clause) that builds without ROS. So this driver's wire was read out of the maker's files, its bytes are held to the
maker's library by `layout_check.py`, and its live block runs against a server built from that library — the plan's F118 ladder, rung 3 (an independent implementation of the protocol; not
any robot maker's controller).

| Module | What it is |
|---|---|
| :mod:`~skynet_ros_industrial.frames` | The envelope and the five message bodies. Pure, every constant cited |
| :mod:`~skynet_ros_industrial.controller` | A simulated controller with a server on it, written here — the dry substrate |
| :mod:`~skynet_ros_industrial.bridge` | The RHAL driver |
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
