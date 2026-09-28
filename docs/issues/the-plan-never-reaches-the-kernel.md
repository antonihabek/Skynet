# Amendment — §6.3 hands a joint's ceiling to the kernel through the plan, and on a robot nothing carries the plan

**Status: ratified 2026-09-28 as §93 of the specification, and built.** Filed 2026-09-26 by
`DRY-COVERAGE-PLAN.md` Phase 10 (F158). Kept as the proposal it was. The built form departs from it
in two places, and §93 names each: `GET /limits` reads the loop's live report rather than the
sealed window's recording, and a wider plan waits on the earlier policies' processes exiting rather
than on a ring release. It adds one thing the proposal did not name: a window carries every set it
let go of, so each stretch replays against its own.

## The gap

§6.3: the planner *"decides what is possible and under what ceilings, and hands the ceilings to the
Safety Kernel — which is the only component that can enforce them"*. On a robot:

* `skynetd` resolves the plan for each commanding package (§37.1) and composes the limits from it
  (`skynetd-io::limits`, served at `GET /limits`, printed by `skynet limits --package`). Those are
  the ceilings an operator reads.
* `skynetd` starts the package with `SKYNET_RING_SOCKET=<root>/run/policy.sock`, and the package's
  `skynetp` writes into the ring `skynetk` offers there. Commands reach the guards.
* `skynetk` — the process that runs the guards — is started by `skynetk.service` with the driver,
  the tier, the rate, the black box and the socket, and **no plan and no ceiling**. It resolves its
  limits from its own command line (`main.rs`), where the per-channel table is empty unless
  `--velocity-max` is given, and the unit gives none.

§73's row for the embodiment descriptor says the same (*"no policy runtime exists to carry a command
as far as a guard"*); carrying this amendment corrects that row in both copies. `skynetd`'s own
note was corrected in the code by the session that filed this, because it is text a robot serves
to its operator.

So the envelope guard runs against an empty table on every robot, while the operator's page shows the
plan's ceilings, and `skynetd`'s own note on that page says *"Nothing enforces them here: … no policy
runtime exists to carry a command as far as a guard"* — true when written, and not since `skynetp`
and the §90.2 policy host were built. The guard also reports itself **active**: §7.2's
*"guards that cannot run say so"* is not reached, because `partition_guards` gives `envelope` no
input that can be missing.

## Measured

`fleet/sweep.py` closed the loop on every public robot description with a policy asking each joint
for a quarter of its travel past the upper end (`reference/reports/fleet.json`, stage `loop`).
Before `--source` existed, seven of seven bodies tried ran three hundred such cycles with **no clamp
and no guard trip** — the only interventions were the watchdog after the policy exited — and each
window replayed as clean. With `--source` the same runs clamp on every cycle (Go2: 2,336 clamps in
350 cycles), and the replay reproduces every verdict.

## Built, interim: `skynetk run --source <policy.json>`

The policy's source embodiment, resolved against the body's own descriptor by the same pure function
`skynet transport` and `skynetd` call (`resolve_transport`). Channels, each joint's ceiling in its own
mode's units, and the modes come from the plan; `--velocity-max` becomes the whole-body figure §86.3
applies by mode, rather than a per-channel number that compared radians with radians per second on a
position channel (F160). A refused plan refuses the run; a plan that binds channel *i* anywhere but
joint *i* is refused, because this host has no channel map. Tested in
`crates/skynet-kernel-host/tests/loop.rs`.

It closes the gap for a bench and for the sweep. It does not close it on a robot, because the unit
file still passes nothing.

## The question

How the commanding package's plan reaches `skynetk` without putting `skynetd` on the actuator path
(§33.1) and without two processes holding two answers.

## Proposed answer: `skynetd` writes the resolved plan's source beside the socket; `skynetk` reads it

1. **What crosses.** Not the plan: the package's **source embodiment**, which `skynetd` already has
   from the manifest. `skynetk` resolves it against the descriptor it already holds with the
   function `--source` already calls. One planner, two callers, the same inputs — the arrangement
   `skynetd-io::limits` and the kernel's own `resolve_limits` already have.
2. **Where.** `<root>/run/source.json`, written by rename so a reader never sees half a file, removed
   when no commanding package is active. `run/` because it is a rendezvous, as `policy.sock` is.
3. **When `skynetk` reads it.** At start, and when a policy takes the ring. A policy that takes the
   ring with no file present runs against the body's own ratings — a plan over every joint by exact
   name, which is what a policy trained on this body would resolve to — and `skynetk` prints that it
   did. A file whose plan is refused refuses the policy: the ring is not offered.
4. **§33.4's asymmetry.** A narrower plan applies on the next cycle; a wider one waits for the policy
   to release the ring, because widening a limit under a running policy is the thing §33.4 exists to
   prevent.
5. **`GET /limits`** stops saying nothing enforces the figures, and says instead which source
   `skynetk` last read, from the recording's `limits.json` beside the sealed window — the only place
   the loop's own figures are written down.

## What it does not change

§33.1: `skynetd` writes a file and never touches the ring, the driver or a cycle. The tier: guest
stays advisory (§7.4); what this changes at guest is the record, which stops calling an unbounded
window clean. The corpus: `resolve_transport` and `resolve_limits` are unchanged; only who calls them
with what.
