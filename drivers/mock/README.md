# The mock robot

A body that answers the RHAL and has no hardware behind it.

Two audiences:

* **A driver author** writes and tests against this with no robot present, and — the part that
  matters more — tests the failure paths that cannot be provoked safely on a real machine. Being
  able to develop the dangerous cases without a robot in the room is the difference between a driver
  that has been exercised and one that has merely been run.
* **The conformance suite.** The sim half of SCTS needs something to run against, so a simulated body
  is not a lesser path — it is load-bearing infrastructure.

```bash
make check
```

```bash
make cpp-header-check
```

No dependencies beyond a C11 compiler. Someone evaluating the platform should be able to clone the
repository and reach a passing conformance run before deciding whether to invest an afternoon.

Without `make`:

```bash
gcc -std=c11 -Wall -Wextra -Wpedantic -I../../bindings/c -o selftest mock_robot.c selftest.c -lm && ./selftest
```

## Fault injection

Selected by substring in the config string passed to `open()`, because a mock robot that needed a
JSON parser to be useful would be a mock robot with a dependency.

| Config | What happens | Why it is worth being able to provoke |
|---|---|---|
| `""` | A well-behaved three-joint arm | |
| `fault=stall` | `seq` stops advancing | A stalled producer is what the kernel's watchdog guard reads. A repeat is not a fresh command. |
| `fault=nan` | Joint 1 reports NaN velocity | A dead encoder. NaN must be reported as NaN, never substituted with a plausible zero. |
| `fault=estop` | The estop flag is asserted | Commands are refused with `EPERM`, and the flag is visible in the state. |
| `fault=timeout` | `read_state` returns `ETIMEDOUT` | "The bus did not answer" and "the drive is faulted" lead to different investigations, and the black box records which. |
| `fault=degraded` | The degraded flag plus a vendor fault code | Running, but something is wrong. |

## What it demonstrates about writing a driver

* **The hot path allocates nothing.** Every array is caller-allocated and reused across cycles. A
  real driver allocates in `open()` and frees in `close()`, and never in between.
* **`emergency_stop` is callable from a signal handler.** No allocation, no locking, no blocking I/O,
  and no dependence on the rest of the driver still being consistent.
* **The mask means hold, not zero.** A zero in `cmd->mask` holds the joint at its setpoint. On a
  torque-controlled joint that is the difference between holding position and going limp, and it is
  where a `drop` binding from a transport plan meets the wire.
* **`describe()` refuses a buffer that is too small**, rather than truncating. Half a parsed body
  description is worse than a failed one.
* **The descriptor declares `"envelope": null`.** The mock knows its joints' design torque limits —
  they are right there in the source — and still declares no safety envelope, because a design value
  from a model is not a datasheet safety rating. Copying one into the other would auto-populate a
  safety limit nobody verified, and the Safety Kernel would then be enforcing a rumour.
* **An estop latches.** A robot that recovers itself from a safety stop is a robot that resumes
  moving while somebody is standing next to it working out what happened.

## The layout cross-check

`mock_robot.c` carries `_Static_assert`s on `offsetof` for every RHAL struct, and
`crates/skynet-rhal` asserts the same numbers from the Rust side. §31.2 says the RHAL C ABI stays
single-source no matter how many editions exist, "because two definitions of a struct layout is a
segfault" — and the way to keep that true rather than aspirational is to assert it from both sides,
so a change to either one fails a build.
