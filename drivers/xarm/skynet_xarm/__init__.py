"""skynet-xarm — a UFACTORY xArm as one Skynet body, over the controller's own wire.

`DRY-COVERAGE-PLAN.md` Phase 10 item 10.5. UFACTORY publish their Python SDK's source (BSD-3-Clause)
and a simulator that runs the controller's own firmware, so this driver's wire was read out of
their files and is measured against their controller — the plan's F118 ladder, rung 1.

| Module | What it is |
|---|---|
| :mod:`~skynet_xarm.frames` | The register table and the report layouts. Pure, every constant cited |
| :mod:`~skynet_xarm.controller` | A simulated controller written here — the dry substrate |
| :mod:`~skynet_xarm.bridge` | The RHAL driver |
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
