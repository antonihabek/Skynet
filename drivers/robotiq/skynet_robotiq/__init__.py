"""skynet-robotiq — a Robotiq 2F-85 or Hand-E gripper as one Skynet body, over Modbus RTU.

`DRY-COVERAGE-PLAN.md` item 12.E.8. Robotiq publish a C++ SDK (`github.com/Robotiq/grippers`, BSD-3-Clause) with a fake gripper in it, so this driver's wire was read out of the maker's own headers, its bytes are
held to the maker's own client by `layout_check.py`, and its live block runs against the maker's own fake gripper served on a pseudo-terminal: the plan's F118 ladder, rung 2 (the maker's simulator), and not a gripper.

| Module | What it is |
|---|---|
| :mod:`~skynet_robotiq.frames` | The command and status blocks, the Modbus RTU frames, the faults, the profiles and the unit conversions. Pure, every constant cited |
| :mod:`~skynet_robotiq.link` | The line: a serial port or a TCP socket carrying the same frames |
| :mod:`~skynet_robotiq.gripper_sim` | A simulated gripper written here, on a pseudo-terminal — the dry substrate |
| :mod:`~skynet_robotiq.bridge` | The RHAL driver |
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
