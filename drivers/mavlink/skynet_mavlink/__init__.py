"""skynet-mavlink — an ArduPilot vehicle as one Skynet body, over MAVLink.

`DRY-COVERAGE-PLAN.md` Phase 11. The class this covers — multirotors, fixed wings and ground rovers — had no
driver in this tree. ArduPilot's flight code builds for x86 with its physics simulated, so this driver is
measured against the code that runs on the flight controller (F118's rung 1), and its codec against the
reference MAVLink implementation.

| Module | What it is |
|---|---|
| :mod:`~skynet_mavlink.frames` | The wire: framing, the CRC, eighteen messages, the enums. Pure, every constant held against the reference |
| :mod:`~skynet_mavlink.geometry` | NED/FRD to ENU/FLU (§44.4), and the arithmetic between them |
| :mod:`~skynet_mavlink.link` | TCP, UDP or serial, in the stdlib |
| :mod:`~skynet_mavlink.autopilot` | A simulated vehicle written here — the substrate for the refusals no firmware produces on demand |
| :mod:`~skynet_mavlink.bridge` | The RHAL driver |
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
