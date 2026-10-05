# skynet-franka-rs

A Franka Robotics arm over the FCI, in Rust — a systems port of `drivers/franka`'s Python
driver (Phase 6, item 6.6 in `DRY-COVERAGE-PLAN.md`). One shared object, one exported symbol.

```bash
make check   # the crate's tests, then the sim half of SCTS against the release .so
```

## What holds it to the Python driver

The Python driver is the executable specification, and the port is measured against it in three
layers, weakest claim first:

| Layer | Holds | Against |
|---|---|---|
| `tests/layout.rs` | every offset and size | `drivers/franka/layout.json` — Franka's own headers, compiled |
| `tests/vectors.rs` | every pack/unpack function, byte for byte | `tests/vectors.json` — eleven wire messages generated from `protocol.py` |
| `tests/live_fake.rs` | open, state, commands, motion switching, refusals, estop, close | the Python fake Control box, over real sockets |

Plus `skynet driver check` on the built `.so` (the sim half of SCTS, 31 passed — five of them a second session after a stop, §4), and the
descriptor byte-identical with the Python bridge's for the same slug — asserted inside
`live_fake.rs`, not eyeballed.

## What a bad link does

`make chaos` runs the link-chaos contract (`tools/faultproxy/chaos.py`) against the release `.so`: the same fake Control box as `drivers/franka`, and the kernel's own link emulation on the loopback of a
network namespace they share, because the FCI's state stream goes to a UDP port the driver announces in `Connect` and no relay can stand in front of it. **56 checks, 0 failed**, the first time it was run. The
contract found a defect in the Python driver that the port did not have (F279: a silent state stream read as a successful read of the last state, where the port answers `SKR_EIO`, and its
`the_newest_state_in_the_buffer_is_the_one_read_and_a_silent_stream_is_not_read_as_fresh` test holds it to).

## What the port knowingly differs on

Four deliberate differences from the Python driver, each documented where it happens:

* The stop path sends on a `TcpStream` cloned at open and held outside the connection lock, so
  `emergency_stop` takes no lock at all. Python sends on the shared socket under the GIL, which
  is a lock it does not name.
* `write_command` re-checks the estop latch under the connection lock, closing a race the Python
  driver has.
* A malformed `port` is `SKR_EINVAL`. Python's `int()` raises there — a traceback out of `open`
  rather than a refusal.
* Per-joint `modes` (§86.1) are unread, as the Python driver does not read them: the descriptor
  declares no `per_joint_modes` and the FCI runs one motion generator in one mode.

## What the port does not claim

This is 6.6's whole point stated as a negative: the driver is in a language that permits the
Host ceiling, and it does not approach it. `read_state` blocks on the socket with a timeout,
`write_command` transacts on the calling thread, and the connection sits behind a mutex — the
Python driver's structure, translated. A background thread with a lock-free ring, and a
wait-free `read_state` over it, is the follow-up this port enables; claiming it here would be
claiming timing behaviour no machine in this phase can measure.

What still needs a Control box is unchanged from the Python driver's README: that the robot
sends what these structs say when it says it, that `Move` is answered the way the driver
expects, and that a 1 kHz loop holds. And the hardware stop is still a button on the box this
driver cannot press.

## What it makes of the maker's codes

The same as [`drivers/franka`](../franka/README.md#what-it-makes-of-the-makers-codes): a test in `tests/scripted.rs` reads `drivers/fault-vocabulary.json` — the table the Python driver is held to, so the two cannot be given different ones — and holds this port to every code in it and to a code in none (DRY-COVERAGE-PLAN.md item 12.C.6).
