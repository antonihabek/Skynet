"""skynet-mavlink — an ArduPilot or PX4 vehicle as one Skynet body, over MAVLink.

`DRY-COVERAGE-PLAN.md` Phase 11 and item 12.E.1. The class this covers — multirotors, fixed wings and ground rovers — had no
driver in this tree. ArduPilot's and PX4's flight code both build for x86 with their physics simulated, so this driver is
measured against the code that runs on the flight controller (F118's rung 1), and its codec against the
reference MAVLink implementation.

| Module | What it is |
|---|---|
| :mod:`~skynet_mavlink.frames` | The wire: framing, the CRC, nineteen messages, the enums and both autopilots' mode tables. Pure, every constant held against the reference |
| :mod:`~skynet_mavlink.geometry` | NED/FRD to ENU/FLU (§44.4), and the arithmetic between them |
| :mod:`~skynet_mavlink.link` | TCP, UDP or serial, in the stdlib |
| :mod:`~skynet_mavlink.errors` | The two ways a bridge says no |
| :mod:`~skynet_mavlink.motions` | What differs between autopilots: modes, parameters, takeoff, goto and velocity |
| :mod:`~skynet_mavlink.autopilot` | A simulated vehicle written here — the substrate for the refusals no firmware produces on demand |
| :mod:`~skynet_mavlink.bridge` | The RHAL driver |
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
