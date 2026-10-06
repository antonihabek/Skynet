# skynet-piper — an AgileX Piper arm over CAN

`DRY-COVERAGE-PLAN.md` item 12.E.10 asks for the vcan CAN family — Piper, ARX, Allegro,
ODrive, VESC, moteus — and this is its second driver and the first arm in it: a Piper's six
revolute joints and, if the operator declares one, its gripper, as one Skynet body over the
maker's own CAN protocol on one bus, joint-level, in position.

```sh
make check         # the suite, then `skynet driver check` through the real ABI
make selftest      # the protocol, the simulated arm and every refusal, on loopback
make check-vcan    # the same driver on a kernel CAN bus, against an arm in another process
make check-layout PIPER_SDK=/path/to/piper_sdk PIPER_PYTHON=/path/to/venv/bin/python
                   # this driver's frames against AgileX's own SDK codec, in both directions
make chaos         # the link-chaos contract: the bridge on one vcan, the arm on another, a relay across
make faultvocab    # the fault-vocabulary contract: the maker's arm statuses, error bits and driver bits
```

The driver is Python and runs under `drivers/python-host`; the bus underneath is the
CANopen driver's (`skynet_canopen.bus`, imported rather than rewritten, its package staged
beside this one), and the arm is a simulated one written here — first on an in-process bus,
then on a kernel `vcan`.

## The wire, and why this driver trusts it

Eleven-bit identifiers, eight-byte frames, **big-endian** throughout (the reverse of the
other CAN drivers here), one megabit. The arm *broadcasts*: once powered it sends its
status (`0x2A1`), its six joint angles in pairs (`0x2A5`–`0x2A7`, signed 0.001°), the
gripper (`0x2A8`) and each driver's own report (`0x261`–`0x266`) cyclically, with no
request. A master answers with a mode (`0x151`), joint targets (`0x155`–`0x157`), the
gripper's target (`0x159`), motor enables (`0x471`) and, at open, a query for each joint's
limits (`0x472`, answered on `0x473`).

AgileX publishes the protocol as an SDK (`agilexrobotics/piper_sdk`, MIT) with a codec of
its own, so the layout claim is held to the maker's code rather than to this driver's
word: `make check-layout` (72 checks) imports the maker's `DecodeMessage` and
`EncodeMessage` unmodified at a pinned commit (`c9e8a281`, 2026-07-31) and holds every
frame this driver writes to the one the maker's code reads, and every frame the maker's
code writes to the one this driver reads, together with the identifiers, the units and the
ranges the maker publishes. It refuses to run without that checkout: a verdict about
another commit would be a verdict about nothing. It found one real divergence in this
driver's own encoder (the maker's `ArmMsgGripperCtrl` refuses a torque past 5000, and this
encoder did not), and one place the maker's own sources disagree (DRY-COVERAGE-PLAN.md
F284: the motor-enable message class documents `0xFF` as "all motors", the interface
documents `7` as "all", and `EnablePiper` is `EnableArm(7)`; this driver enables joints 1
to 6 by name and the gripper by its own number, which means the same on either reading).

## What it reads, and what it will not invent

**Health is the status frame and the drivers' reports together.** The arm status (sixteen
values), the error word (a joint's angle-limit and communication bits) and every driver's
status byte (supply voltage, motor and driver temperature, over-current, collision
protection, driver error, stall protection, and *driver enabled*) are all on the wire. A
body that does not send them does not come up. The fault word is the maker's own, composed
and not translated: the error word in the low sixteen bits, the arm status in bits 16 to
23, and the union of the six drivers' fault bits in the top byte, where `0x40` means *a
driver is disabled* — a disabled Piper joint is a limp one. Two bits of it are this
driver's, both in the byte the maker lists as reserved: bit 15, set while the arm is in a
control mode other than CAN control and so ignoring this master, and bit 14, set while the
gripper's status byte holds a fault. The tables are generated from the SDK's own
enumerations (`vocab_gen.py`, into `drivers/fault-vocabulary.json`) and `make faultvocab`
holds this driver to them (`tools/fault-vocab/`, item 12.C.6): seven vocabularies, each
code injected through the driver's own read and write and held to the flags, the fault
word, the refusal and the recovery, with the values past each table escalating and never
reading as healthy.

**Faults are refused, never cleared.** While any of that stands every command is refused
whole, and the driver sends no frame that clears one. The arm's clear is the person's act
at the arm.

**The stop is a stop, and its release is not the SDK's.** The stop sends the arm's fast
stop (`0x150`, `emergency_stop` 1) and latches. The maker's *resume* is the same frame with
`2`, which in the same SDK is `ResetPiper`: *the arm loses power and falls*. A software
release that dropped a loaded arm is not a release, so `release_stop` answers ENOSYS,
nothing in this driver encodes that byte (the suite asserts that no session ever sent
one), and `open` refuses while the arm reports the stop. The session is renewed over an arm
a person reset deliberately, with its weight supported (§4, the ODrive and Spot
precedent). Closing a session sends nothing: a Piper that is disabled loses power and
falls, one that is put in standby is no longer held, and one that is simply no longer
spoken to holds the last target it was given.

**Travel is the narrower of two tables.** The maker publishes each joint's range
(`C_PiperParamManager`) and the arm reports the limits it holds (`0x473`, asked at open).
The descriptor declares the intersection, never the wider (F181), and a setpoint outside
it is refused whole, never clamped. An arm found outside its range at open is refused
(F246).

**A command that is not a finite number is refused whole** (F282), before anything is built
or stored, whatever mode or mask it arrives with: NaN passes every comparison a clamp is
made of.

**Velocity and effort are NaN.** The drivers report *motor* speed and current, and the gear
ratio and torque constant that make them a joint's are not on the wire (§24).

**The gripper is the operator's to declare.** `gripper` and `gripper_torque_nm` have no
default: an arm sold without a hand has no seventh joint, and a hand's grip force is the
integrator's risk assessment (§63.4). So has `speed_percent`: a joint move is "to this
angle at this percentage of the arm's own maximum speed", and how fast an arm may move
among people is not this driver's to decide.

**Modes.** Position only. The wire's joint-level verb is *move to this angle*; there is no
velocity or torque command in the CAN control mode this driver speaks (the MIT mode would
carry them, with gains the operator chooses, and is not built). A command in another mode
is refused.

## Where it stops

| Claim | Standing |
|---|---|
| **Guest tier** | Reached. 330 checks in its own suite (341 on `vcan`), 72 in the layout check, 40 in the link-chaos run, 280 in the fault-vocabulary contract, and the sim half of SCTS through the real ABI |
| Seeded defects | 169 of 169 noticed at rung 4 (100%), with 5 equivalent mutants left out for the reason written on each and 4 of 4 controls surviving as they must (`node tools/seed-defects/seed-defects.mjs --driver piper`). The first campaign noticed 144 of 171 and found 27 checks nobody had written: the wire's identifiers pinned as literals and not as the constants the rest of the suite is written in, both ends of a limit report, the signedness of the slow report's two single-byte fields, the maker's gripper-torque bound, the whole travel table, §5.1's chain and axis grammar, the stop latch apart from the arm's own confirmation of it, every report going stale, a session's limits, stop and backlog not outliving it, and the *reason* an open is refused for (a stop and a missing report are two different next actions) |
| A real arm | None was on any bus this driver was measured against, and AgileX publishes no simulator. The simulated arm broadcasts the way the protocol document and the SDK say, and the layouts are held to the maker's own codec — but a simulation agreeing with its driver is rung 4, and the driver stays alpha |
| The joints' response | The simulated joints move to a target at a speed this repository chose: gravity, payload, gains, a stalled joint and what the speed percentage is in rad/s are unseen |
| What the stop does to a load | The driver sends the fast stop; whether the firmware holds a loaded arm or lets it fall was not measured |
| The MIT mode | Not built: velocity, torque and the per-joint gains are the operator's to choose and this driver has none to offer |
| Bus load | An arm broadcasting at its full rate on a real controller is a question about arbitration this suite cannot answer (`vcan` has no bit timing) |

The SDK is read at the pinned commit and never vendored; the bus package it shares with
the CANopen driver is this repository's own. `make check`, `make check-vcan`, `make chaos`
and `make faultvocab` need nothing beyond Python and (for the kernel-bus ones) a kernel that
will hand out a CAN socket; `make check-layout` needs the SDK checkout and Python's
`python-can`, which is the SDK's one dependency.
