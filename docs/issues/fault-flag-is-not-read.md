# Amendment — §24 gives a driver three flags, §27 reads two, and the third is the one that says the robot is not doing what it is told

**Status: proposed**, filed 2026-10-01 by `DRY-COVERAGE-PLAN.md` item 12.C.6 (F197). Nothing in the
specification has changed. The drivers have been made to work *inside* §27 as it is written, which is
the part that did not need a decision, and what remains is the one that does.

## The gap

§24's `flags` has four bits a driver can raise, and §27 and the daemon read these of them:

| Flag | Meaning, §24 | What reads it | What it does |
|---|---|---|---|
| `RHAL_ESTOP` | *an emergency stop is asserted* | `StopSighting`, `post.rs`, `run.rs` | `stop` condition; `STOPPING`, and never `stop_complete` from this bit alone |
| `RHAL_DEGRADED` | *running, but something is wrong* | `RhalSighting::of` (`skynetd-io/src/blackbox.rs:899`) | `rhal: degraded`; §27's `DEGRADED` row |
| `RHAL_FAULT` | *the drive has faulted; commands are being ignored* | **nothing** | recorded verbatim in the black box's `state_flags`, and that is all |
| `RHAL_BRAKED` | *holding brakes are engaged* | `robot.state.braked` | a precondition path; no state |

§27's `DEGRADED` row says *"a guard is unavailable, or the RHAL reports `RHAL_DEGRADED`"* and no row
mentions `RHAL_FAULT`. The daemon's own comment says a faulted drive *"is not a loss"*, which is true
and does not say what it is.

The consequence is not hypothetical, because §24 also says a fault code is *"vendor-specific,
surfaced verbatim"* — the OS is told never to interpret one — so **the flags are the whole of what a
driver can tell the OS**, and a driver that raises `FAULT` alone has told it nothing. Item 12.C.6's
contract (`tools/fault-vocab/`) was run against each driver that has a substrate here, **before** any
of them was changed, and these raised exactly that:

| Driver | A fault read as | What §27 saw | How known |
|---|---|---|---|
| `xarm` | `ESTOP` + `FAULT` | a stop — only because the controller's STOP state happens to set `ESTOP` | measured: all 60 codes injected failed a check |
| `franka`, `franka-rs` | `FAULT` (an error flag, or the Reflex mode) | `present`: a healthy arm | measured: 50 of 52 codes |
| `ethercat`, `ethercat-rs` | `FAULT` | `present` | measured: 2 of 3 states |
| `ur_rtde`, `ur_rtde-rs` | `DEGRADED` alone, for a protective stop, a safeguard stop, a violation and a fault | an impaired arm, **with commands still being taken** | measured: 30 of 72 checks |
| `mavlink` | `FAULT` (for `CRITICAL`, `EMERGENCY`, `FLIGHT_TERMINATION`), and nothing at all for `UNINIT`, `BOOT`, `CALIBRATING`, `POWEROFF` and any state the dialect does not define; a terminated flight still answered `land` | `present`, and commands still taken in a failsafe | measured: 27 of 76 checks against the bridge as it was before `33e596c` (2026-10-01, in the forty-second session; the driver was fixed in that commit) |
| `spot` | `FAULT` (for any fault, or a stopped robot), and `ESTOP` + `FAULT` for an estop slot saying "stopped" and nothing at all for one in any other state | `present`: a healthy robot — and an informational message, which the maker calls *no hardware problem*, read as a fault | measured: 60 of 143 checks (2026-10-01, in the forty-second session; the driver is fixed, the contract is green) |
| `fairino` | nothing: the controller's error number went into the fault word and no flag rode with it | `present`: a healthy arm, and commands still sent to a controller holding an error | measured in the driver's own suite, not through the contract (it has no table): 11 of its 181 checks failed against the driver as committed, four of them the new ones (F206) |

`drivers/canopen` and `drivers/ros2_control` already raised `DEGRADED` beside `FAULT`, which is the
only reason they were not on the list, and nothing in §24 or the header told their authors to.

## The question

Whether the OS reads `RHAL_FAULT`, and as what.

## Proposed answer

Two decisions, and the first does not need the second.

1. **`RhalSighting::of` reads `RHAL_FAULT` as `degraded`**, and §27's `DEGRADED` row says so: *"the RHAL
   reports `RHAL_DEGRADED` or `RHAL_FAULT`"*. A driver that follows the header's own wording —
   *"the drive has faulted"* — then reaches §27 without having to know that a different bit is the one
   the daemon reads. It is one line in `blackbox.rs`, it changes no corpus case (`RhalSighting` is
   the daemon's mapping of a record onto §27's inputs, not part of `derive`), and it makes the
   `DEGRADED` requirement the contract now holds drivers to redundant rather than wrong.
2. **Whether a faulted body is a stopped one is the owner's.** `RHAL_FAULT` means *commands are being
   ignored*. A kernel that goes on commanding a body that is ignoring it is writing into the void
   every cycle, and on a multi-axis body whose one axis has faulted the others are not. §27's
   `STOPPING` row has four causes and a fifth, `estop`, added because the chain is *the body's* word;
   a sixth, `fault`, would be the body's other word. Whether the Safety Kernel should command a stop
   on it is a safety decision and not a mapping, and it is left here because making it in a driver —
   reporting `ESTOP` for what is not an open chain — is the one workaround this phase refused: it
   puts a name on an event that has none (`StopCause::Estop`'s own documentation says why).

## What was done without it

Every driver that can be exercised here was made to raise `DEGRADED` beside `FAULT` and `ESTOP`
(`drivers/fault-vocabulary.json`, `tools/fault-vocab/contract.py`), which is within §24 as written,
and to **refuse commands while the controller is ignoring them** — a servo target written to a UR's
registers during a protective stop had been answered `SKR_OK`. If decision 1 is carried the first
half becomes belt and braces; the second half does not depend on it.

## Carrying it

Decision 1 is one edit in each copy of §27's table, one line in `crates/skynetd-io/src/blackbox.rs`,
a test beside `RhalSighting`'s, and `node tools/check-mirrors.mjs --pin ../RobotOverflow`. Decision 2
is a design question and has no implementation order until it has an answer.
