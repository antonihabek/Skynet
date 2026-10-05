# skynet-odrive — ODrive servo axes over CANSimple

`DRY-COVERAGE-PLAN.md` item 12.E.10 asks for the vcan CAN family — Piper, ARX, Allegro,
ODrive, VESC, moteus — and this is its first driver: one Skynet joint per ODrive axis, over
ODrive Robotics' CANSimple protocol, on one CAN bus. An ODrive v3 board carries two axes;
the configuration names nodes, so a body of several boards is several nodes.

```sh
make check         # the suite, then `skynet driver check` through the real ABI
make selftest      # the protocol, the simulated axis and every refusal, on loopback
make check-vcan    # the same driver on a kernel CAN bus, against axes in another process
make check-layout ODRIVE_FW=/path/to/ODrive   # this driver against ODrive's own sources
make chaos         # the link-chaos contract: the bridge on one vcan, the axes on another, a relay across
make faultvocab    # the fault-vocabulary contract: the maker's error words through the bridge's read and write
```

The driver is Python and runs under `drivers/python-host`; the bus underneath is the
CANopen driver's (`skynet_canopen.bus`, imported rather than rewritten, its package staged
beside this one), and the axes are simulated ones written here — first on an in-process
bus, then on a kernel `vcan`.

## The wire, and why this driver trusts it

CANSimple is arithmetic over eight-byte frames: an 11-bit identifier with the node id in
the upper six bits and the command in the lower five, a heartbeat carrying the axis error
and state, encoder estimates in turns, and a setpoint per control mode. ODrive publishes
the protocol's own CAN database (`tools/odrive-cansimple.dbc`), the firmware beside it
(`Firmware/communication/can`), and the enumerations as importable Python
(`tools/odrive/enums.py`) — so the layout claim is three of the maker's sources held
against each other and against this driver, not this driver's word for the wire:

* the thirty command ids come from the firmware's `CANSimple` enum;
* the payload layouts (offsets, widths, byte order, scales) and the state, mode and error
  value tables come from the DBC, parsed, with `enums.py` imported unmodified as the
  second opinion on every value;
* the six behaviours the driver leans on — zero-length requests, the 64-bit motor error,
  what the stop latches, what clear wipes, the float estimates, the position feed-forwards —
  are held as source text out of `can_simple.cpp`: if the firmware rewords them the check
  fails and a person re-reads the file.

`make check-layout` (96 checks) refuses to run without a checkout at the pinned commit
(`3a2e4dd1`, 2026-01-20): a verdict about another firmware would be a verdict about
nothing. It found the maker's own sources disagreeing twice (DRY-COVERAGE-PLAN.md F280):
the DBC models the motor error as 32 bits while the firmware sends 64 (and the value table
names bits past the first word), and the DBC's input-velocity units are swapped. Both are
pinned as errata the check fails on if they change — the driver follows the firmware.

## What it reads, and what it will not invent

**Health is the heartbeat.** Every heartbeat carries the axis error word, the axis state,
and whether the motor, encoder and controller hold errors of their own. A body whose axes
do not send one does not come up: without it an error would sit in the firmware unread
while the driver reported positions. The cyclic rates are the integrator's configuration
and the driver never assumes them — the estimates are *requested* every cycle, so an axis
with its cyclic estimates switched off still answers.

**Errors are refused, never cleared.** A nonzero axis error reads `FAULT` and `DEGRADED`
with the word verbatim, and every command is refused while it stands. Clearing is the
person's act at the drive (`CLEAR_ERRORS` is board-wide in the firmware, which is one more
reason no path of this driver sends it). The stop latches `AXIS_ERROR_ESTOP_REQUESTED` in
the firmware by design, so there is no software release — `release_stop` answers ENOSYS
and a new session returns the body (§4, the Spot precedent) — and `open` refuses while any
axis holds the error, so the session is renewed over a drive the person cleared
deliberately. The table is `drivers/fault-vocabulary.json` and `make faultvocab` holds
this driver to it (`tools/fault-vocab/`, `DRY-COVERAGE-PLAN.md` item 12.C.6): the maker's
13 axis error words and 6 undefined bits, each held to the flags, the word, refusal and
recovery. The subsystem words are read by the firmware into the axis word or the state,
and the heartbeat's flag bytes are correctly ignored — see the table's `not_covered` for
the firmware evidence.

**No rating is declared.** The wire publishes no travel, no velocity rating and no torque
rating: nothing on it says what motor is attached to an axis, and the firmware's limits
are integrator configuration the protocol cannot even read back. The joints are
`continuous` with no limits and the envelope is null, and the guards that need a figure
report unavailable.

**Effort is NaN.** `GET_IQ` reports amps, and amps become newton-metres through the motor's
torque constant, which is configuration and not on the wire (§24's rule, and a zero would
read as "this joint is unloaded" every cycle).

**Modes** are position, velocity and torque, per joint (§86.1): each axis is independent,
so a mixed command is one frame per joint. The input mode is always passthrough — the
kernel's setpoint reaches the controller unshaped.

## Where it stops

| Claim | Standing |
|---|---|
| **Guest tier** | Reached. 173 checks in its own suite (181 on `vcan`), 96 in the layout check, 32 through the real ABI |
| A real axis | None was on any bus this driver was measured against; the simulated axis answers the way the firmware answers, and the layouts are held to the maker's DBC — but a simulation agreeing with its driver is rung 4, and it stays alpha |
| Torque motion | The sim holds its shaft in torque mode: integrating torque needs an inertia the wire does not carry, and an invented one would be physics the tests could pass against and a robot would not have |
| Calibration, homing | The sim never calibrates and the wire reports no homing state; homing reads unreported |
| Bus load | Eight axes at high rate on a real bus is a question about arbitration this suite cannot answer (`vcan` has no bit timing); the row stays in the plan's *not dry-verifiable* column |

The vendor's firmware is read at the pinned commit and never vendored; the bus package it
shares with the CANopen driver is this repository's own. No account, no SDK install, no
binary beyond the toolchain: `make check` and `make check-vcan` run on any machine with a
C compiler, Python and (for the second) a kernel that will hand out a CAN socket.
