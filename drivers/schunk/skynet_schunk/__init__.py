"""skynet-schunk — a SCHUNK EGU, EGK or EZU gripper as one Skynet body, over its module's HTTP interface.

`DRY-COVERAGE-PLAN.md` item 12.E.9. SCHUNK publish a Python client and a protocol simulator (`schunk_mechatronic_gripper`, GPL-3.0), so this driver's wire was read out of the maker's own files, its bytes are held to
the maker's own client by `layout_check.py`, and its live block runs against the maker's own simulator: the plan's F118 ladder, rung 2 (the maker's simulator), and not a gripper.

| Module | What it is |
|---|---|
| :mod:`~skynet_schunk.frames` | The routes, the two 16-byte frames, the bits and the parameters. Pure, every constant cited |
| :mod:`~skynet_schunk.gripper_sim` | A simulated module written here — the dry substrate |
| :mod:`~skynet_schunk.bridge` | The RHAL driver |
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
