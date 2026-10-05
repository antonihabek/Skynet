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

Plus `skynet driver check` on the built `.so` against URSim: 31 passed, 0 failed — five of them a second session after a stop (§4), which is what found F81. The live test is
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

## What it makes of the maker's codes

The same as [`drivers/ur_rtde`](../ur_rtde/README.md#what-it-makes-of-the-makers-codes): a test in `tests/scripted.rs` reads `drivers/fault-vocabulary.json` — the table the Python driver is held to, so the two cannot be given different ones — and holds this port to every code in it and to a code in none (DRY-COVERAGE-PLAN.md item 12.C.6).

## When the link is worse than a cable

```bash
make chaos        # the same controller, relays and arm as drivers/ur_rtde, with this port's shared object called through its RHAL vtable
```

`DRY-COVERAGE-PLAN.md` item 12.C.4. The port's stop was already fire-and-forget (the latch, then the hold package and the dashboard's `stop`, each written once on a socket duplicated at open, no reply read, no lock
waited on), and it passed the contract's stop check as it stood. What the run found, and what was done: the same sentence on every period (188 and 97 identical lines in one run, F266), which is said once per run of the same
cause now; a stream the controller closed read as a protocol it could not parse (`EPROTO`), where it is a link that has gone (`EIO`, `RtdeError::Closed`: while a session is being brought up it is still the controller
refusing); and, because they are the same defect as the Python driver's and the contract could not reach them, a bound on a setpoint's send (20 ms, shared with the stop path's duplicate of the socket, which a
controller that has stopped reading and filled the buffer would otherwise have held for the socket's five seconds), a session that refuses setpoints by name once a send did not finish (F272), a bound on the pause a close
waits for, and `ports` in the configuration so that a relay can stand between the port and a controller (`bridge.py`'s `DEFAULT_PORTS`, with the same refusals).
