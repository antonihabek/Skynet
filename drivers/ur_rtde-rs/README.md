# skynet-ur-rtde-rs

A Universal Robots arm over RTDE, in Rust — a systems port of `drivers/ur_rtde`'s Python driver
(Phase 6, item 6.6 in `DRY-COVERAGE-PLAN.md`). One shared object, one exported symbol.

```bash
make test                               # the wire, the program, the descriptor. Needs nothing
SKYNET_UR_HOST=127.0.0.1 make check     # and the live test and SCTS, against URSim
```

URSim is Universal Robots' own controller software, in a container — the command is in the
`Makefile`'s header. Without `SKYNET_UR_HOST` the second half is skipped by name; with it, the live
test is required and a run that cannot reach the controller fails.

## What holds it to the Python driver

The Python driver is the executable specification, and it was itself measured against URSim. The
port is held to it in two layers:

| Layer | Holds | Against |
|---|---|---|
| `tests/vectors.rs` | every request the session writes, byte for byte; every reply it parses, including a text message mid-handshake and an `IN_USE` refusal; the setpoint package; the drain keeping the newest sample; the control program, character for character; the descriptor for every model, simulated and not, by value | `tests/vectors.json`, which `tests/gen_vectors.py` produces by driving the Python `Rtde` session through a scripted socket |
| `tests/live_ursim.rs` | the whole §24 surface through the real vtable: open, the descriptor, a commanded move with the other joints held, the refusals, a refused command that must not move the arm when the next one holds, the stop and its latch, the same driver opened again after it | URSim 5.26.1 |

Plus `skynet driver check` on the built `.so` against URSim: 26 passed, 0 failed. The live test is
known to catch what it is for: with `write_command` storing each joint as it is checked — F74's shape
— the holds after a refused command drive joint 0 to the refused value, 0.3 rad away, and the test
fails.

## What writing it found

Three defects in the Python driver, each fixed there first with a check that fails without the fix
(`DRY-COVERAGE-PLAN.md`):

* **F74** — a command refused on its second joint had already stored its first, and the next command
  that held that joint sent the stored value. On URSim that is the arm moving 0.3 rad on a "hold".
* **F75** — the output drain consumed a packet's header before its body had arrived, so a packet
  straddling the moment the socket ran dry desynchronised the stream for good: a robot that looks
  frozen.
* **F76** — the stop's latch outlived the session, so a driver opened again after a stop refused every
  command. The Franka driver had the same.

## What the port knowingly differs on

* A configuration value of the wrong JSON type is refused rather than read for its truthiness.
* The setpoint registers must come back typed as six doubles and an integer — the layout the stop
  path's stack-built package writes — and the output fields as the vectors and ints this driver reads.
* `pause` at close reads past the data packages still streaming to its own reply; Python's refused the
  first data package and `close` swallowed the error. Only the newest package of a drain is decoded.
* The stop writes the hold package and `stop` on sockets duplicated at open, and waits for no reply,
  as the RHAL requires (*"Never blocks"*); Python waited for the dashboard's reply line. It returns
  `SKR_OK` when either write reached the controller, and `SKR_EIO` when neither did.

## What the port does not claim

A language that permits the Host ceiling, not an approach to it: `read_state` waits up to one
controller period for a new sample behind the connection's mutex. And URSim is the controller's
software on a developer's machine, not a controller — it proves the protocol and the program, not
that a setpoint reaches a joint inside 8 ms. That row stays in the plan's *not dry-verifiable*
column, and the rated stop is still the emergency circuit, which no frame on any socket reaches.
