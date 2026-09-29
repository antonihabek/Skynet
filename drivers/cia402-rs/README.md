# skynet-cia402-rs — CiA 402's drive profile, shared by the two Rust drivers that speak it

Not a driver and not a robot: a library. `drivers/canopen-rs` (CiA 402 over CANopen) and
`drivers/ethercat-rs` (CiA 402 over EtherCAT, whose CoE is CANopen's application layer on another
transport) both decode a statusword, build a controlword and convert increments to SI units with this
crate, because two implementations of one state machine disagree exactly once, after the failure that
matters (§80). `drivers/models.json` records it as `kind: library`, so it carries no release stage — a
stage is a claim about a robot, and this commands none.

It is a port of `drivers/canopen/skynet_canopen/cia402.py`, which is the executable specification, and
only the master's half: a master reads a statusword and never predicts one, so the transition table
that the Python module needs for its simulated drive is not here. Pure and safe (`#![forbid(unsafe_code)]`),
with no I/O and no clock.

```bash
make check      # this crate's own unit tests
```

`make check` is what CI needs to hold the crate on its own; the property that matters is held from the
other side. `drivers/canopen-rs/tests/vectors.rs` compares every state's statusword, every command's
controlword and the unit conversions, across rounding boundaries, with what the Python module produces,
so a change here that the Python profile disagrees with fails there.

## What it does not show

* Any vendor's drive. The profile is CiA 402's own, and a drive's quirks (a state it takes seconds to
  leave, F174; objects it does not expose, `0x608F` and `0x6091`) are the drivers' to meet, against
  whatever counterpart they have — `docs/robot-support.md` says which.
* Anything about timing. Nothing in this crate has a clock.
