"""skynet-ur — a Universal Robots arm over RTDE, verified against URSim.

| Module | What it is |
|---|---|
| :mod:`~skynet_ur.rtde` | The protocol: framing, recipes, the two rules measurement taught |
| :mod:`~skynet_ur.dashboard` | Port 29999 — power, brakes, safety status |
| :mod:`~skynet_ur.control` | The URScript loop that turns registers into `servoj` |
| :mod:`~skynet_ur.bridge` | The RHAL driver |
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
