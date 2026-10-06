"""skynet-moteus — mjbots moteus controllers over CAN-FD, as one Skynet body.

`DRY-COVERAGE-PLAN.md` item 12.E.10 asks for the vcan CAN family — Piper, ARX, Allegro, ODrive, VESC, moteus. ODrive and Piper were the first two; this is the
third, and the first that needs a CAN-FD bus: a moteus's command and its query do not fit eight bytes, and the frame that asks for an answer has an identifier
no 11-bit one holds.

moteus is a good dry fit for ODrive's and Piper's reason, and a better one in two respects. The maker publishes the protocol, the register map, the fault
vocabulary *and the firmware* (`mjbots/moteus`, Apache-2.0), including a Python client that encodes and parses every frame, so a layout check can drive the
maker's own codec with this driver's bytes and hold its enumerations to the firmware's headers, with nothing transcribed from a PDF. And the controller reports
its own travel over a diagnostic channel (`conf get servopos.position_min`), which no CAN servo drive in this family does: the declared limits are the controller's
own and not the operator's typing.

What that leaves outside is one row: **a real controller.** No moteus was on any bus this driver was measured against; the simulated controllers answer the way the
documentation and the firmware's mode machine say, and the wire is held to the maker's codec — but a simulation agreeing with its driver is rung 4, and it stays alpha.

| Module | What it is |
|---|---|
| :mod:`~skynet_moteus.protocol` | Subframes, registers, quantities, modes, fault codes. Pure |
| :mod:`~skynet_moteus.bus` | CAN-FD frames; the CANopen driver's loopback carries them, and a socket bus speaks them |
| :mod:`~skynet_moteus.sim` | Simulated controllers — the dry substrate |
| :mod:`~skynet_moteus.bridge` | The RHAL driver |
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
