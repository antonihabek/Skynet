"""skynet-canopen — CiA 402 drives on a CANopen bus, as one Skynet body.

`DRY-COVERAGE-PLAN.md` calls this *"the best dry fit in the set"*, and the reason is that almost all
of CANopen is arithmetic over eight-byte frames: the state machine, the object dictionary, the
expedited SDO encoding and the PDO layouts can all be exercised exactly, against a simulated drive,
with no hardware and — here — with no kernel CAN module either.

What that leaves outside is one row: **bus bandwidth at a real joint count and a real rate.** Eight
axes at 1 kHz on a 1 Mbit bus is a question about arbitration and bit timing, and nothing in this
package can answer it. It stays in the plan's *not dry-verifiable* column.

| Module | What it is |
|---|---|
| :mod:`~skynet_canopen.frames` | COB-IDs, expedited SDO, the PDO layouts. Pure |
| :mod:`~skynet_canopen.cia402` | The drive profile: states, commands, the object dictionary, units. Pure |
| :mod:`~skynet_canopen.bus` | SocketCAN, or an in-process bus that needs no kernel |
| :mod:`~skynet_canopen.slave` | A simulated CiA 402 drive — the dry substrate |
| :mod:`~skynet_canopen.bridge` | The RHAL driver |
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
