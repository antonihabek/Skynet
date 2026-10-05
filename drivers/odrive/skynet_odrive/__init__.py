"""skynet-odrive — ODrive servo axes over CANSimple, as one Skynet body.

`DRY-COVERAGE-PLAN.md` item 12.E.10 asks for the vcan CAN family — Piper, ARX, Allegro,
ODrive, VESC, moteus — and this is its first driver. ODrive is the best dry fit in that set
for the same reason CANopen was in Phase 2: almost all of CANSimple is arithmetic over
eight-byte frames, the maker publishes the protocol's own CAN database
(``tools/odrive-cansimple.dbc``) and the firmware it is generated beside, and ``vcan`` plus
a simulated axis is a substrate with no hardware in it.

What that leaves outside is one row: **a real axis.** No ODrive was on any bus this driver
was measured against; the simulated axis answers the way the firmware's
``Firmware/communication/can/can_simple.cpp`` answers, and the layouts are held to the
maker's own DBC — but a simulation agreeing with its driver is rung 4, and it stays alpha.

| Module | What it is |
|---|---|
| :mod:`~skynet_odrive.cansimple` | Identifiers, commands, payload layouts, units. Pure |
| :mod:`~skynet_odrive.axis` | A simulated ODrive axis — the dry substrate |
| :mod:`~skynet_odrive.bridge` | The RHAL driver |

The bus underneath is the CANopen driver's (:mod:`skynet_canopen.bus`, imported rather than
rewritten, with its package staged beside this one): a frame is a frame whatever profile
runs above it, and two implementations of SocketCAN setup would drift.
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
