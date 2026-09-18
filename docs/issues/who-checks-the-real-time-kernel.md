# Proposed amendment — §1 says Host runs on PREEMPT_RT, and nothing says who checks

**Status: ratified and carried 2026-09-18**, on the owner's instruction to complete Phase 6.
Written 2026-09-18 from `DRY-COVERAGE-PLAN.md` F57, by Phase 6's first session; carried as
proposed, with one addition the carrying found necessary: `SKYNETK_UNCHECKED_TIER=1`, the
explicit harness override the existing `--tier host` suites run under, without which the
refusal would have turned CI's enforcement coverage red on its stock runners.

## The gap

§1's table says Host *runs on* PREEMPT_RT. `control.realtime` is one of §85's thirty-eight
capability tokens, so a package may require it and a body may declare it, and
`embodiment.capabilities.declared` — `sim`, required from host — asks that every declared token
be one the driver can actually service. **Nothing in the tree reads whether the running kernel
is PREEMPT_RT.** No file matches `/sys/kernel/realtime`, `CONFIG_PREEMPT` or `cyclictest`, and
`install.sh`'s host check tests for Linux, `systemctl` and systemd as PID 1 while its failure
message *mentions* §23.5 asking for PREEMPT_RT without checking for it.

So a robot on a stock kernel can declare `control.realtime`, pass the sim test that is supposed
to police the declaration, and present as Host-ready. This is §61's "vendors do not publish
these" arriving from the inside: the platform's own machine cannot contradict its own
declaration.

`mlockall` is the same gap one row down. §23.5's list is *"RT core, `SCHED_FIFO` for the kernel
thread, `mlockall(MCL_CURRENT|MCL_FUTURE)`, no swap"*; `skynetk.service` sets
`LimitMEMLOCK=infinity`, which raises the limit and locks nothing, and Phase 6 item 6.2 makes
the call and reports it. Reporting is not checking: a stock kernel, an unlocked address space
and four gigabytes of swap currently start `--tier host` and run it, with the startup lines as
the only witness.

What Phase 6's first session built, deliberately short of the fix: `install.sh --check` reports
swap and the RT kernel as machine readings (not postconditions — install writes neither, so
failing on them would blame the installation for the machine), and `skynetk` prints its lock
and RT status at startup. F57's sentence stands over both: *the fix is not another advisory.*

## The question

Whether the check belongs to `install.sh` (refuse the installation), to `skynetk` (refuse the
tier at start), or to the check-in (report it and let a Hub refuse the award).

The recommendation is `skynetk`'s, for the precedent F57 names: §31.3 derives an edition's
ceiling and §35.3 derives the simulator cap, and neither asks permission — the tier the loop
*runs* at is derived from what the machine is. `--tier host` on a kernel that is not
PREEMPT_RT, with an address space `mlockall` could not lock, or with swap active, should be a
refusal at start that names what it read. `--tier`'s own help already frames it that way: *a
tier is a claim and the default should not make one* — and a claim the machine cannot back is
what a refusal is for.

That changes one sentence this repository has carried since `skynetk` was written: *"Nothing
here decides a tier."* True today, and true in the sense the header means it — the binary
resolves limits, it does not award anything. What the refusal decides is narrower: whether the
claim may be made on this machine. The header's second sentence already concedes the shape:
*"Passing `--tier host` on a general-purpose scheduler does not make the clamp real."* After
this section it does not start, either.

The other two keep their share. `install.sh --check` reports both readings, as it does now, so
a bench robot says early what it is. The check-in carries them, so a Hub refusing an award has
the machine's own measurements rather than the vendor's word. Neither refuses: the installer
does not own the machine's kernel, and a Hub refusing an award weeks later is an audit, not a
gate.

## Proposed text

In §23.5, after the list: *the tier the loop runs at is derived from what the machine is. A
loop asked for Host on a kernel that is not PREEMPT_RT, with memory it could not lock, or with
swap active, refuses to start and names what it read. The readings are reported by the
installer and the check-in; the refusal belongs to the loop, because it is the loop that would
run the claim.*

In §1's table, on the Host row: *the kernel requirement is checked by the loop at start, not
declared by the descriptor.*

## What this unblocks, in order

Phase 6 items 6.4 and 6.5: the refusal in `skynetk`, refused-at-start tests with a control on
each of the three readings, `ros2_control` and `unitree` to Host-ready on a PREEMPT_RT virtual
robot — the harness that can show a refusal on a stock kernel and a start on an RT one, and no
bench evidence either way.

## Why this is an issue and not a patch

`CONTRIBUTING.md`: *"If you think the specification is wrong, open an issue rather than a pull
request against it — a maintainer carries the change to both copies."* Who checks is a design
decision, not §73's figure maintenance (`DRY-COVERAGE-PLAN.md` *Decisions* 1 and 10), and
*Decisions* 13 and 14 each ratified a named amendment rather than a class of them. So it waits
for the owner.

## Amends

§23.5 (the refusal), §1 (one clause on the Host row). §31.3 is precedent, cited, unchanged. No
corpus case: a refusal at start is a behaviour, and behaviours are tested where they run.
