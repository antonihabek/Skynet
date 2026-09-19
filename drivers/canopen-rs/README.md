# skynet-canopen-rs

CiA 402 drives on a CANopen bus, in Rust — a systems port of `drivers/canopen`'s Python driver
(Phase 6, item 6.6 in `DRY-COVERAGE-PLAN.md`). One shared object, one exported symbol.

```bash
make check   # the crate's tests with the kernel-bus half required, then the sim half of SCTS
```

`make check` runs under `drivers/canopen/vcan.sh`, which uses the machine's `vcan0` or makes one in
an unprivileged network namespace — no root. Where it can do neither, it fails rather than passing a
measurement it did not take.

## What holds it to the Python driver

The Python driver is the executable specification, and the port is measured against it in two
layers, weakest claim first:

| Layer | Holds | Against |
|---|---|---|
| `tests/vectors.rs` | every frame, statusword, controlword, unit conversion and the descriptor's JSON rendering, bit for bit | `tests/vectors.json`, generated from `frames.py` and `cia402.py` by `tests/gen_vectors.py` |
| `tests/live_vcan.rs` | open, the descriptor, reads, commands that move the drives, the refusals, the stop and its latch, close and reopen, a faulted drive, a drive without units, an axis that misses every SYNC | the Python driver's own `SimulatedDrive`s in another process (`python3 -m skynet_canopen.drives`), over a kernel CAN bus |

Plus `skynet driver check` on the built `.so` (the sim half of SCTS, 26 passed), and the
descriptor byte-identical with the Python bridge's for the same drives — asserted inside
`live_vcan.rs`, not eyeballed. The live test is known to catch the defect it exists for: with the
read reduced to one empty look after the SYNC — F71's shape — its first read fails with
`SKR_ETIMEDOUT`.

## What writing it found

F73 in `DRY-COVERAGE-PLAN.md`: four paths the Python driver had never run, each fixed there first
with a check that fails without the fix, because it is the specification. A zero motor-side gear
figure was divided by. A command refused on its second axis had already sent the first. A second
`open` stacked a second body on the first and kept its latch. Two joints could name one node. And
the worst, found by the check for the second: a setpoint past RPDO2's 32 bits was stored before its
frame failed to pack, after which the quick-stop frame — which carries each axis's stored target —
could not be packed either, and `emergency_stop` raised instead of stopping.

## What the port knowingly differs on

Four differences from the Python driver, each documented where it happens:

* The configuration is checked whole before the bus is opened, and a value of the wrong JSON type is
  refused rather than read for its truthiness — a `simulator` of `"false"` caps a tier.
* A TPDO2 that is not six bytes is counted as a frame this driver does not speak; Python raises.
* A bus error is `SKR_EIO`; Python raises `OSError` out of whichever entry point met it.
* The stop sends on a descriptor duplicated at open, outside the body's lock, reading each axis's
  target from an atomic — so `emergency_stop` takes no lock it could wait on, and `write_command`
  re-checks the latch under the lock, as `franka-rs` does.

## What the port does not claim

A language that permits the Host ceiling, not an approach to it. `read_state` sends a SYNC and waits
up to the SYNC window behind the body's mutex, which is the Python structure translated and the
nature of a CANopen read; a thread that owns the SYNC cycle and a ring the RHAL reads is the
follow-up this port enables. And a virtual bus is still not a bus: no arbitration, no bit timing, no
bus-off, so nothing here says whether eight axes fit in a millisecond at 1 Mbit — the one row the
plan's table keeps under *not dry-verifiable* for this driver.
