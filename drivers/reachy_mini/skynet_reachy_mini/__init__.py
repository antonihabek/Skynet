"""skynet-reachy-mini — a Reachy Mini as one Skynet body, over its daemon's own REST interface.

`DRY-COVERAGE-PLAN.md` item 12.E.12. Pollen Robotics publish the daemon (`reachy_mini`, Apache-2.0) with a MuJoCo simulator behind it, so this driver's routes were read out of the maker's own source
and its live block runs against the maker's own daemon in simulation: the plan's F118 ladder, rung 2 (the maker's simulator), and not a robot.

| Module | What it is |
|---|---|
| :mod:`~skynet_reachy_mini.frames` | The routes and the shapes the daemon's models give them. Pure, every constant cited |
| :mod:`~skynet_reachy_mini.daemon_sim` | A simulated daemon written here — the dry substrate |
| :mod:`~skynet_reachy_mini.bridge` | The RHAL driver |
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
