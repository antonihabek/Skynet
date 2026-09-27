# Amendment — §6 and §7.1 bound a joint's travel with one number, and travel has two ends

**Status: proposed 2026-09-26**, by `DRY-COVERAGE-PLAN.md` Phase 9 (F140). Not carried: it changes
§7.1's envelope guard, the transport clamp every edition resolves, and the corpus, so it is the
owner's to ratify and a maintainer's to carry to both copies (`CONTRIBUTING.md`).

## The gap

§7.1's envelope guard checks *"per-joint position, velocity, torque vs. rating and transport
clamp"*. The rating reaches it as one magnitude per channel: the planner's `rating_for` hands a
position channel `max(|position_min_rad|, |position_max_rad|)`, and the kernel clamps
`|value| > ceiling` to `±ceiling`. The planner's own comment says *"the kernel holds both ends"*. It
does not: nothing in `crates/skynet-kernel` reads a travel's two ends — `kinematics.rs` leaves travel
to *"the envelope guard's business"*, and the envelope guard has one number.

So on any joint whose travel is not symmetric about zero, every command between the narrow end and
the negative of the wide end is outside the joint's travel and passes the kernel untouched:

| Joint | Travel (rad) | Ceiling handed to the kernel | Passes, and is outside travel |
|---|---|---|---|
| Franka Panda joint 4 | −3.0718 … −0.0698 | 3.0718 | anything in (−0.0698, 3.0718] |
| Unitree Go2 calf | −2.7227 … −0.8378 | 2.7227 | anything in (−0.8378, 2.7227] |
| a humanoid knee | 0 … 2.5 | 2.5 | anything in [−2.5, 0) — hyperextension |

## Measured

The fleet sweep (`fleet/sweep.py`, `reference/reports/fleet.json`, stage `travel`) asks every
asymmetric joint of every position-commanded body for a tenth of its travel past its **narrow** end,
through `skynetk --source` with the plan's ceilings in force and no rate limit, so a clamp can only
be the envelope guard's. The report carries the count on every body; on the first twelve run, the
Go2's eight asymmetric joints were asked 2,392 times and clamped 0 times, Apollo's twenty-two 6,600
times and clamped 0. The clamps the stage does see on some bodies are coincidence — a narrow-end
request that also exceeds the wide end's magnitude — and each of those is clamped to the wide
magnitude, which is still outside the travel.

At guest tier enforcement is advisory (§7.4), so no command on any robot today is clamped anyway;
what is wrong today is the **record**: a window in which a policy drove a knee backwards is sealed
and replayed as a window in which the envelope guard found nothing. At host tier it is an unenforced
limit.

## Proposed answer: a position channel carries its travel, both ends

1. **§6's clamp.** `TransportClamp.per_channel` keeps its magnitude for velocity and torque channels.
   A position channel additionally carries `travel: [min, max]` from the joint's
   `position_min_rad` / `position_max_rad`, keyed by joint name like `joint_modes`. A joint that
   declares one end and not the other carries no travel, and says so — the §7.2 rule, per channel.
2. **§7.1's envelope guard.** For a channel whose `joint_modes` entry is `position` and which has a
   travel, the clamp is `min(max(value, min), max)`; a clamp at either end is `Clamped` with both
   figures, exactly as the magnitude clamp is recorded. The magnitude check stays for every other
   channel, and for a position channel with no travel.
3. **The composition rule.** §7.3's "limits compose by minimum" becomes, for travel, *intersection*:
   the profile, the cell and the plan can each narrow a travel and none can widen it. An empty
   intersection is refused at resolution time, with both ranges named, rather than clamping every
   command to a point.
4. **The record.** §26's cycle record already carries each clamp's `from` and `to`; nothing is added
   to the ring format.

## Implementation order

The oracle first (`reference/typescript/safety.ts`, `embodiment.ts`), then the corpus — new cases in
`corpus/limits` and `corpus/transport` for an asymmetric joint at each end, an intersection, an empty
intersection, and a position channel with one end declared — then every edition reproducing it
byte for byte, then `skynet-kernel`, `skynet-kernel-host` (`plan_of` carries the travel it already
has), and last the fleet sweep's `travel` stage, which should turn from `fail` to `pass` on every
body with no change to the sweep.

## What it does not change

The tier (§59.2's ceilings stand), the rate limit, and the watchdog. A continuous joint has no
travel and gets none. The magnitude ceiling does not go away: it is what a velocity or torque channel
is, and a position channel with no declared travel keeps the protection it has today.
