# skynet-ethercat-rs

CiA 402 drives on an EtherCAT segment, in Rust — a systems port of `drivers/ethercat`'s Python
driver (Phase 6, item 6.6 in `DRY-COVERAGE-PLAN.md`), the last of the four. One shared object, one
exported symbol.

```bash
make test    # the frames against vectors from frames.py, and the unit tests. Needs nothing
make check   # and the live test and SCTS against the Python slave stack, over a veth pair
```

`make check` runs `check.sh`, which makes `ecat0`/`ecat1` in an unprivileged network namespace (no
root, no NIC), serves the Python driver's simulated drives on `ecat1` with
`python3 -m skynet_ethercat.wire`, and runs this master on `ecat0` over `AF_PACKET` — the arrangement
SOEM is cross-checked in, with this port where SOEM is.

## What holds it to the Python driver

| Layer | Holds | Against |
|---|---|---|
| `tests/vectors.rs` | frames packed and unpacked, byte for byte | `tests/vectors.json`, from `frames.py` by `tests/gen_vectors.py` |
| `tests/live_wire.rs` | the real vtable: scan, configure and Init → Op; CoE reads; the descriptor, equal as a value to the Python bridge's; the CiA 402 walk and the drives reaching their targets; the refusals; F77's refused command; the stop and its latch; reopen; a faulted drive; a drive without units; a configuration that miscounts the ring | the Python slave stack, in another process, over a real interface |

Plus SCTS against the built `.so`: 31 passed, 0 failed — five of them a second session after a stop (§4). The live test is known to catch what it is
for: with `write_command` writing each drive's output as it is checked — F77's shape — the holds
after a refused command carry the first drive to the refused 0.9 rad, and the test fails.

The CiA 402 profile is `drivers/cia402-rs`, shared with `canopen-rs` exactly as the Python driver
imports the CANopen package's: two implementations of one state machine disagree exactly once.

## What the port knowingly differs on

* **No in-process segment.** The Python driver brings simulated drives up in its own process and
  derives `simulator` from the segment's type; the port speaks only to an interface, so it defaults
  to `simulator: true` and a real integration says `false`, as every other driver here does.
* A configuration value of the wrong JSON type is refused, and so is a body larger than one `LRW`
  carries (123 drives).
* A frame the ring does not return within 50 ms is `SKR_EIO` — a deadline for the whole wait —
  where Python's per-read socket timeout raised out of `read_state`.
* `emergency_stop` sends one quick-stop `LRW` built on the stack, on a socket duplicated at open,
  with index 0 (which no exchange of the master's uses) and waits for no answer; the next read keeps
  every controlword at quick stop and asserts the working counter. Python cycled once, waiting,
  inside the stop.
* `describe` before `open` is `SKR_EPERM`; Python raised.

## What the port does not claim

A language that permits the Certified ceiling, not an approach to it: the two properties that
justify it — distributed clocks and STO over FSoE — are the two this repository has never verified
and cannot, and a veth pair has no propagation delay to measure. `simulator: true` caps this row at
guest regardless.
