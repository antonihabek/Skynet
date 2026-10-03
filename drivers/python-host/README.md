# skynet-python-host

A RHAL driver that runs a **Python** driver out of process. One shared object, exporting the one
symbol, forwarding every RHAL call to an interpreter it spawned.

```bash
make
make check          # builds it, then runs the sim half of SCTS against the LeRobot bridge's fake
```

## Why a sidecar and not embedded CPython

Embedding is the obvious design and it cannot honour §4's second rule:

> `emergency_stop()` must work from a signal handler and from a process that is dying.

Calling into CPython from a signal handler is not safe — the interpreter takes locks and allocates,
and a stop path that can deadlock is worse than one that is slow. Writing one line to a pipe *is*
async-signal-safe. So running the driver out of process is not a workaround here; it is the only
arrangement in which a Python driver can implement the entry point that matters most.

Two further properties fall out of it, both the right way round:

* **A garbage-collection pause stalls the interpreter, not the host.** The host owns its own timing
  and reports a late answer as `SKR_ETIMEDOUT` — which is true — rather than absorbing it.
* **A Python driver that crashes takes down a child process.** The host sees the closed pipe,
  reports `SKR_EIO`, and the OS still has a live stop path.

The cost is a pipe round trip per cycle, tens of microseconds. That is unaffordable on a 1 kHz
torque loop and irrelevant at Guest tier — which is exactly the tier a Python driver is capped at
(§31.3), so the cost lands where it does not matter.

## The stop path

`emergency_stop` sets a latch and writes `E\n`. The latch is a single store to a flag the command
path already reads, so **the stop takes effect whether or not the interpreter ever answers**. The
write is a best effort on top of it: it is what actually disables torque, and it is allowed to fail
without the stop failing. Nothing on that path allocates, takes a lock, or calls into an interpreter.

The reply is not waited for — waiting is what a stop path must never do — so it is drained before
the next exchange. That draining is load-bearing: without it the reply arrives where the answer to
the *next* request should be, and the stream sits one frame out for the rest of the session.

**A stop during an extension (F234).** The child is one thread reading one line at a time, so the
`E` was read when the call in progress returned. For `read_state` and `write_command` that is
microseconds. For an `extension` that is a streamed motion — `drivers/mavlink`'s velocity,
`drivers/ros2_base`'s, held for up to five seconds — it was the end of the motion the stop was
meant to end: the host's latch refused the *next* command and the one running ran out its
duration. A driver whose `emergency_stop` is safe to call from another thread while an extension
runs says so with a class attribute, `stop_during_extension = True`, and the child then reads its
requests on a thread of its own, acts on an `E` that arrives while an extension is in flight at
once, and still answers in the order it was asked: the extension's reply first, the stop's after,
which is the order the host drains in. A driver that does not say so is read as before, because
calling a stop that was written to be serialised from a second thread is not a change the
child can make for it. `make check-stop` holds both halves against a fixture child.

## Configuration

The RHAL hands `open()` a JSON string and this host has no JSON parser; adding one to a shim whose
job is to forward bytes would be the tail wagging the dog. So the split is:

| | |
|---|---|
| `SKYNET_PYTHON` | the interpreter to run (default `python3`, `python` on Windows) |
| `SKYNET_PYTHON_DRIVER` | the module to run with `-m` (default `skynet_lerobot`) |
| `PYTHONPATH` | where that module lives |

and **the config JSON is forwarded to the child verbatim**, which is what it is for: it configures
the *robot*, and the robot is the child's business.

## The protocol

Newline-delimited ASCII on the child's stdin and stdout, defined in
[`bindings/python/skynet_rhal.py`](../../bindings/python/skynet_rhal.py). Deliberately not JSON: the
only structured thing crossing the boundary is the descriptor, and it passes through opaquely.

**Everything the driver says to a human goes to stderr.** stdout is the wire, and a stray `print` on
it is a corrupted frame rather than a log line.

The handshake carries the ABI version, the protocol version and the driver's **tier ceiling**. A
child announcing anything but `guest` is refused rather than believed — §31.3's third rule, *an
edition may not raise its own ceiling*, made mechanical.

### Two channels, because §89.1 needs two (protocol 2)

§89.1 lets a driver declare that a sensor may be read from a second thread while `read_state()` and
`write_command()` run, *without making them wait*. One pipe cannot carry that: two threads writing
requests into it interleave their bytes, and a mutex around it would make the control thread wait
for a render — the very thing the declaration says does not happen.

So a declared sensor gets a channel of its own. The child is spawned with a second pipe pair on
descriptors 3 and 4 (on Windows, two inheritable handles named in the environment), and after
`open()` the host asks `Y`; the child answers with the sensors its own descriptor declares. Those
reads go down the second pipe, every other call keeps the first, and the child serves the second on
a thread. A driver that declares nothing gets exactly the arrangement it had before.

`make check-concurrent` is where this is measured, against
[`fixtures/skynet_slow_camera.py`](fixtures/skynet_slow_camera.py): undeclared, declared honestly,
and declared falsely. The middle one **fails** with the second channel taken away, which is how it
was held to a control.

**The GIL is not fixed by any of this.** Two threads in one CPython process do not run bytecode at
the same time, so the declaration is honest for a read that spends its time inside a C extension
that releases the GIL — a render, a large numpy operation, a blocking device read — and is not
honest for one that does not. That is the vendor's call, which is §89.1's whole design.

**And no driver in this repository can make it**, for a reason one level below this host:
`skynet_lerobot`'s camera frames are a by-product of the `get_observation()` that reads the joints,
and `skynet_mujoco` renders the same `mjData` its step writes. Multiplexing the pipe removes the
host's share of the obstacle; the driver's share can only be removed inside the driver. See
`DRY-COVERAGE-PLAN.md` F46.

## Frames

`read_sensor` is implemented and frames cross as base64 on their own line, capped at 4 MiB. That cap
is a design statement rather than a limit anyone should meet: a 640×480 RGB frame is 900 KB per
cycle through a pipe, and the answer for a real camera is the runtime's own capture path (§47.2),
not a bigger buffer here. What legitimately crosses is a small depth patch, a tactile array, or a
test frame.

## Extension (`T`), with no protocol bump

`extension` is forwarded: `T <token> <base64>` on one line, `R <rc> <b64len>` plus the base64
answer on the next. The single-line request is load-bearing — a child from before `T` consumes it
as one unknown verb and answers a bare `R EPROTO`, which the host reads as "this child predates
extension" and reports as `ENOSYS`, with the stream still in sync. A driver without an `extension`
method answers `ENOSYS` the same way. Payloads are capped by the 64 KiB line discipline both
sides honour.

## Release (`R`), with no protocol bump

`release_stop` is forwarded the same way: `R` on one line, `R <rc>` back. A child from before
`R` consumes it as one unknown verb and answers a bare `R EPROTO`, which the host reads as
"this child predates release" and reports as `ENOSYS` — after clearing its own session latch,
which no child's answer can hold. A driver without a `release_stop` method answers `ENOSYS`
the same way. The OS treats `ENOSYS` from release as "no software release" and renews the
session instead; a child's stop state that only a new session returns is exactly what that
path is for.

`make check-extension` is where this is proven, twice: a child that implements it (echo,
refusals, an exploding driver, a too-small buffer) and a child from before it. The first
service-robot driver — `drivers/pudu`, whose only route that moves anything is `extension` under
`vendor.pudu.task` — is what needed the verb to exist.

## What is verified, and where

| | |
|---|---|
| Windows | Built and run end to end, by hand and by `make check` — **except** protocol 2's second channel, whose handle-passing has not been run there |
| Linux | Built and run in CI (`abi` job), the second channel included |
| macOS | Not exercised. The POSIX path is shared with Linux; the difference is untested |

The platform-specific part is four functions — spawn, write, read a line, terminate — and it is
short on purpose, because it is the part that can only be tested where it runs.
