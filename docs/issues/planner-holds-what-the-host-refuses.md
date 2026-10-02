# Gap — the planner says an unmapped joint is "held by the controller", and the kernel host refuses the plan

**Status: found 2026-10-02**, by `DRY-COVERAGE-PLAN.md` Phase 12 (item 12.F.7, F218). Not carried: the repair is a channel map in the
kernel host, which is a change to the loop every robot runs under, and the owner's to decide. What *is* built is the measurement
(`fleet/sweep.py`'s `skill` stage; `docs/skill-support.md`) and the words in `fleet/skills.py` that tell the host's refusal from a skill's
failure.

## The gap

Two sentences in the specification, and a third in the code, do not agree about a joint a plan does not drive.

* **§6.1 and the planner** (`skynet transport`, `skynet skill check`, `skynetd`'s install stage): a plan that binds fewer channels than the
  body has joints is fine, and says so — *"1 joint on this body is not driven by the policy and will be held by the controller"*
  (`unmapped_roles`). Measured on the Hub's own UR5e body with `park`: verdict `direct`, six channels bound, one joint (`wrist_2`) left to the
  controller.
* **§93.2**: *"A plan that binds a channel anywhere but its own joint is refused … because this host writes channel i to joint i and has no
  channel map."*
* **`skynet-kernel-host/src/plan.rs`**: refuses a plan whose channels are not the body's joints, in the body's order, and **all** of them —
  *"the plan binds 5 channel(s) and the body declares 6 joint(s). This loop commands every joint on every cycle, so a plan that leaves one out is
  refused."*

So the planner's answer to "does this skill reach this body?" is *yes* for a plan the loop will refuse the rings to (§93.2: *"The loop goes on
holding position: a robot whose commanding package does not fit its body still holds"*). Nothing before the robot says so: `skynet skill check`
passes `skill.transport.resolves`, the Hub's `check.mjs` is green on that line, and the install stage admits the package. The first anyone
learns of it is a skill that is installed, activated and never given the rings.

## Measured

`fleet/sweep.py --stage skill` asks every skill of every public body that reached the kernel loop, and drives the ones whose action space binds
through `skynetk --source`, with the package's own `bin/run`. See `docs/skill-support.md` for the figures. Two findings that need no table:

1. **Three of the 24 skills have a five-channel action space** (`calibration-pose-sweep`, `payload-droop-check`, `photo-turntable`: no gripper
   channel). Every one of them binds the SO-101 follower, the body their numbers were taught on (`skill.transport.resolves` passes), and the host
   refuses all three on it: the SO-101 has six joints. They cannot run on the body they were written for.
2. **Every other binding is on a body the host can drive only when the body's joint list is the skill's, in order.** In the fleet that is the SO-ARM100 and
   SO-ARM101 descriptions and nothing else; on every other arm the planner's `remap` is a plan nothing executes.

## Why it matters beyond the skills

A policy trained on one arm and run on another is the thing §6 is for. The planner does that work and the loop does not honour its output for any
body whose joints are not the policy's. The conformance corpus covers the planner and not the host, so no edition and no corpus case can see the gap.

## What would close it

Either of these alone changes what a person is told; the second changes what the robot does.

1. **Say it at check time.** `skynet skill check --body` gains a check — the host's acceptance of the plan, by the same function the host calls — so a package that
   binds and cannot run fails `skill.transport.runs` with the host's sentence, not a green line. (`plan.rs`'s structural test would move into
   `skynet-contracts`, beside the planner, and the host and the CLI would both call it.) This is a measurement and moves no behaviour; it would turn the Hub's
   `npm run check` red for the three five-channel skills on the SO-101, truthfully.
2. **Give the host a channel map.** The loop expands the N channels a policy writes into the J joints the driver takes: `values[j] = policy[map[j]]`,
   and `mask[j] = 0` for a joint no channel reaches — §25's zero mask, which already holds a joint where it is (`fleet`'s `hold` stage shows it standing a
   body up). Ceilings, modes and travel come from the plan for mapped joints and from the body's own ratings for the rest. The observation ring is the
   mirror. It is a change to `skynet-kernel-host` (`plan.rs`, `cycle.rs`), to §93.2's sentence about the channel map, and to nothing in the corpus; it
   is the piece of §6 that makes `remap` mean something at run time.

## What was done instead

Nothing in the loop. The stage reports `host_refused` with the host's two reasons (*reorders joints*, *leaves a joint out*) as their own category, so the figures do
not blame a skill for the host's limit, and `docs/skill-support.md` quotes the count.
