"""skynet-piper — an AgileX Piper arm over CAN, as one Skynet body.

`DRY-COVERAGE-PLAN.md` item 12.E.10 asks for the vcan CAN family — Piper, ARX, Allegro, ODrive, VESC, moteus. ODrive
was its first driver; this is the second, and the first *arm* in it. Piper is a good dry fit for ODrive's reason and a
better one: the maker publishes the whole protocol as readable source (``agilexrobotics/piper_sdk``, MIT), including
the codec that turns every frame to and from named fields, so a layout check can drive the maker's own decoder with
this driver's bytes and the maker's own encoder against this driver's decoder, in both directions, with nothing to
transcribe from a PDF.

What that leaves outside is one row: **a real arm.** No Piper was on any bus this driver was measured against; the
simulated arm answers the way the SDK documents the firmware answering, and the layouts are held to the maker's
codec — but a simulation agreeing with its driver is rung 4, and it stays alpha.

| Module | What it is |
|---|---|
| :mod:`~skynet_piper.protocol` | Identifiers, layouts, units, enumerations. Pure |
| :mod:`~skynet_piper.arm` | A simulated Piper — the dry substrate |
| :mod:`~skynet_piper.bridge` | The RHAL driver |

The bus underneath is the CANopen driver's (:mod:`skynet_canopen.bus`, imported rather than rewritten, with its
package staged beside this one): a frame is a frame whatever protocol runs above it.
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
