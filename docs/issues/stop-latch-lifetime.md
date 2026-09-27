# Amendment — §4 says the stop must not block, and nothing says how long its latch lives

**Status: ratified and carried 2026-09-19**, on the owner's instruction to finish every section of
`DRY-COVERAGE-PLAN.md`, which left F76 as *"for the owner"*. Written and carried by the plan's
twentieth session, as proposed below; what carrying it found is at the end.

## The gap

§4's third property is that `emergency_stop` works from a failing process, and the header's second
rule says it never blocks. Neither says what a stop leaves behind. Every driver in this repository
latches — later commands are refused and the next state carries `SKR_FLAG_ESTOP` — and until the
nineteenth session they disagreed about how long:

| Driver | Latch after `close` and `open` |
|---|---|
| `drivers/franka`, `drivers/ur_rtde` (Python) | **kept** — every command of the next session refused, for the life of the process (F76) |
| `drivers/canopen` (Python) | **kept**, and the second session appended its axes to the first's — a six-joint body of a three-joint robot (F73) |
| `drivers/ethercat`, `drivers/franka-rs` | cleared |

The first two were fixed in the tree (`f314354`, `852b666`), so the tree agrees with itself. The
document still says nothing, and **nothing checked it**: `skynet driver check` opened every driver
once, so no second session had ever been asked for anything.

## The question

Whether a driver's software latch lives for the **session** (cleared by `open`), for the
**process**, or until an **explicit clear** a driver would have to expose.

## Proposed answer: the session — and the robot's stop is not the driver's to time

The latch is the session's. Three reasons, the last of which decides it.

1. **A process is already a session boundary.** The latch is state inside a shared object loaded
   into `skynetk`, and `skynetk.service` restarts on exit. A latch "for the process" is a latch for
   the session in every deployment this OS has, except the one where a caller reopens in-process —
   which is exactly the case the two defective drivers got wrong.
2. **An explicit clear is a second stop API**, and the RHAL has none. A driver-level reset would be
   a call nothing in the OS makes, reached by nobody, which is this repository's four-times defect
   arriving in the vendor interface.
3. **How long a robot stays stopped is §27's**, and §27 already says it: `SAFE` *"requires an
   explicit human action to leave"*, because *"a robot that recovers itself from a safety stop is a
   robot that resumes moving while somebody is standing next to it"*. A driver cannot see that
   human. A driver that kept its latch to stand in for §27 would be enforcing the OS's rule from the
   one layer that cannot observe whether it has been satisfied — and would make the OS's deliberate
   new session impossible to start.

The exception is F26's, and it is not a latch the driver chose: **a latch the hardware keeps** — a
rated stop circuit, a drive's latched fault — is reported in the state's flags and fault code, and
`open` never clears it, because it is the evidence of why the robot stopped. A CANopen driver that
reset every drive at open erased exactly that (F26).

## Proposed text — §4, the third property

> **Its latch is the session's:** every command is refused until `close`, and `open` begins a
> session in which no earlier stop is asserted. How long a *robot* stays stopped is not the
> driver's to decide — it is §27's `SAFE`, which a human leaves, and a driver cannot see that
> human. A latch the hardware keeps, a rated stop circuit or a drive's latched fault, is the
> exception: it is reported in the state's flags and fault code and `open` never clears it,
> because it is the evidence of why the robot stopped.

The header's second rule carries the same sentence, because a vendor reading only the header is
the reader §4 exists for.

## The gate

`skynet driver check` opens a **second session** after it has stopped and closed the first, and
asks it four things:

| Check | Filed under | Why there |
|---|---|---|
| it opens | `rhal.abi.version` | beside *"opens"* and *"closes"*: the lifecycle |
| its state reports the joint count the descriptor declares | `rhal.state.completeness` | §15's own definition — every descriptor joint in the state — and F73's appended axes fail it |
| its first state carries no stop | `safety.estop.latency` | the stop's family; a bench id, so it reports and does not move the award |
| it accepts a well-formed command | `rhal.command.rejection` | the first session's *"accepts a well-formed command"*, asked again — guest-required, so a driver whose stop outlives its session is not a guest-tier driver |

A body whose envelope declares a **rated** stop (`envelope.estop: true`) is asked only the first
two: a stop in its second session is the hardware reporting honestly.

## What carrying it found

* **F81.** The Rust UR port could not open a second session at once: URSim answers `IN_USE` for the
  RTDE registers until it has freed the closed session's — about 60 ms, measured. The Python driver
  had passed only because every `open` through the Python host starts an interpreter first. Both
  drivers now wait for their own registers, bounded, in `open`; both suites dropped the fifteen-
  second retry that had hidden it.
* **F82.** `skynet driver check` had exited 0 for a driver that reached **no tier**, since the
  command was written: `tier_rank("none")` ranked at guest. The gate above would have printed its
  refusal and passed every `make check` it ran in.
* **SOEM**, in the same paragraph: `ethercat-reference-driver-is-not-soem.md` was carried with this
  one, and §62.1's table, which repeated the parenthetical, with it.

## Amends

§4's third property, and `bindings/c/skynet_rhal.h`'s second rule. No figure, no contract, no
corpus case: `driver check`'s assertions are the CLI's, and §15's test list is unchanged.
