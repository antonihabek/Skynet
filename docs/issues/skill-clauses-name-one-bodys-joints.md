# Amendment — a skill's clauses name the joints of the body it was written on

**Status: proposed 2026-10-01**, by `DRY-COVERAGE-PLAN.md` Phase 12 (item 12.F.4), found while building item 12.F.3. Not carried:
the answer that would let one package serve several bodies changes §8's manifest and §54.2's context, which every edition spells
identically, so it is the owner's to ratify and a maintainer's to carry. What *is* built is the check that makes the problem visible
(`skill.context.on_body` in `skynet skill check --body`), and a rule for the library until this is decided (below).

## The gap

§54.2's context names a joint by what the body's descriptor calls it: `robot.state.joints.shoulder_lift`. §5.1 leaves those names
vendor-specific on purpose (*"Wire names stay vendor-specific; the semantics become shared"*), and shares the semantics through roles,
which the transport plan (§6, §37.1) uses to bind a skill's **action channels** to a body's joints. A skill's **clauses** are not
bound that way. They read the name as written, so a clause is refused on any body whose joints are called something else — with the
path named, which is §54.2's first rule doing its job, and is no help to the person holding the package.

The 24 skills in the Hub's `skynet-skills` read exactly five names, in 98 clauses: `shoulder_lift` (36), `shoulder_pan` (34),
`elbow_flex` (20), `wrist_flex` (4) and `wrist_roll` (4). Those are the SO-101's. Of the five bodies the manifests used to name in
`verified_on`, none of the other four has any of them:

| Body | Its joints |
|---|---|
| SO-101 follower | `shoulder_pan`, `shoulder_lift`, `elbow_flex`, `wrist_flex`, `wrist_roll`, `gripper` |
| Universal Robots UR5e | `shoulder_pan_joint`, `shoulder_lift_joint`, `elbow_joint`, `wrist_1_joint` … |
| Franka Research 3 | `panda_joint1` … `panda_joint7`, `panda_finger_joint1` |
| AgileX Piper | `joint1` … `joint6`, `gripper` |
| Unitree G1 (arm) | `right_shoulder_pitch_joint`, `right_elbow_joint`, `right_wrist_roll_joint` … |

So a skill that says it was verified on those four had a precondition on its first line that cannot resolve there. Item 12.F.5 emptied
`verified_on` for that reason; this is the rest of the reason.

**Measured, not argued** (`skynet skill check --body`, every skill against every body, 2026-10-01): 23 of the 24 skills name at least one
joint (the 24th, `shift-readiness`, names none). `skill.context.on_body` passes **23 of 23 on the SO-101 and 0 of 23 on each of the other
four**. The same 120 runs of `skill.transport.resolves` pass 23 of 23 on *every* body: the action space binds everywhere, because it
binds by role, and the clauses bind nowhere but the SO-101, because they bind by name. A package that clears the transport plan and is
refused at its first precondition is the shape of this finding.

## Why not simply map the names

The obvious fix is to let the install stage rewrite `robot.state.joints.<source name>` to the target body's joint through the role the
two share, as it already does for channels. That turns *unresolvable* into *resolves*, and what it then resolves to is a **number**:
`shoulder_lift` between −1.65 and −1.35 rad is the SO-101's fold. On a UR5e, whose `shoulder_lift_joint` is zero with the arm
horizontal, the same interval is an arm standing nearly vertical, and `park` would judge a pose that is not the park pose and call it
reached. A refusal that names the path is the safe failure; a clause that resolves and means something else is the failure this
project calls the worst kind, because it looks like success.

So the name is the visible half. The invariant half is that **a pose is a property of a body**, and a package that states one carries
the body's numbers whether or not it carries its names.

## What was done, and the rule until this is decided

* `skynet skill check --body <descriptor>` reports `skill.context.on_body`: every joint a clause names must be a joint of the body, and
  the failure lists the body's own. Without a body it says it did not measure. The Hub's `check.mjs` now runs the robot's gate against the
  body each manifest quotes as its `source_embodiment`, which is the only body its numbers were written for; it used to run every
  package against whichever body file sorts first (the AgileX Piper), which would have reported 23 packages failing on a body they
  never claimed.
* **Until this is ratified, a skill whose clauses name joints is a package for the body those names belong to.** A second body is a second
  package (or a second version line), written from that body's descriptor, with its own numbers; the library says so in each manifest's
  `source_embodiment` and in nothing else.

## Proposed answer

Two changes, either of which is useful alone:

1. **Per-body parameter tables in the package.** A skill ships `bodies/<robot_slug>.json` beside its manifest: for each body it has been
   written for, the joint names and the numbers its clauses and its action channels use. Admission picks the table whose `robot_slug`
   matches the robot's descriptor, and refuses the install with the slugs the package has when none does. Clauses are written once over
   *parameter names* (`park.shoulder_lift.min`), and the table binds them. This keeps the property above: a body's numbers come from a
   table somebody wrote for that body.
2. **A role-keyed branch for what is genuinely body-independent.** `robot.state.by_role.<chain>.<link>[.<axis>]` for a joint's reading,
   by the role it was suggested or asserted to have, so a clause such as *"the elbow is not at its limit"* — which means the same on
   every arm — is written once. Roles carry an optional axis and an index (`right_hand.finger.flex#2`), so the branch's keys would be the
   role with its dots as nesting and `#` kept; a descriptor with both `right_arm.elbow` and `right_arm.elbow.roll` makes one key both a
   leaf and a branch, and that case is refused at import rather than resolved.

## What it costs

(1) is a field on the package (§8) and a rule in admission; (2) is a branch in §54.2 and in `live_context`. Both are spelled by every
edition and pinned by the corpus (`skill`, `live`), and neither is needed by the skills in the library if each stays a package for its
own body, which is the cheaper rule and the one in force.

## What it does not settle

Whether the Hub's catalogue shows a skill once with several bodies or several times with one: that is the Hub's. And it does not touch
the thresholds' *provenance* — a taught angle is a measurement of a cell, not of a model number — which is the recorded-and-not-solved
part of 12.F.9.
