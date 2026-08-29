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

## Frames

`read_sensor` is implemented and frames cross as base64 on their own line, capped at 4 MiB. That cap
is a design statement rather than a limit anyone should meet: a 640×480 RGB frame is 900 KB per
cycle through a pipe, and the answer for a real camera is the runtime's own capture path (§47.2),
not a bigger buffer here. What legitimately crosses is a small depth patch, a tactile array, or a
test frame.

## What is verified, and where

| | |
|---|---|
| Windows | Built and run end to end, by hand and by `make check` |
| Linux | Built and run in CI (`abi` job) |
| macOS | Not exercised. The POSIX path is shared with Linux; the difference is untested |

The platform-specific part is four functions — spawn, write, read a line, terminate — and it is
short on purpose, because it is the part that can only be tested where it runs.
