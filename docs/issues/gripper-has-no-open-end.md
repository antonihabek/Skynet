# Amendment — `robot.state.gripper` has a definition and no source

**Status: proposed 2026-10-01**, by `DRY-COVERAGE-PLAN.md` Phase 12 (item 12.F.2, the half of it that is not code). Not
carried: it adds a field to the embodiment descriptor, which every edition spells identically, so it is the owner's to
ratify and a maintainer's to carry. The half of 12.F.2 that *was* code — `robot.guards.active` — is built (F204) and needs
no amendment, because the loop already writes the fact and the daemon only had to read it.

## The gap

§54.2's context has `robot.state.gripper`, and `skynet_contracts::live::LiveInputs::gripper` defines it as *"the gripper's
opening, 0 closed to 1 open, when the body has one"*. The Hub's skills read it as exactly that: `gripper-cycle-check` starts at
`gte 0.9` ("fully open") and ends at `lte 0.08` ("fully closed"), and `handoff-receive` wants `gte 0.8` before it moves. The
daemon fills the branch from nothing, on every robot, because nothing on this disk says what a gripper's opening is:

* **Which joint is the gripper.** The descriptor's `end_effectors[]` carries a *kind*, a *dof* and a grip force, and
  `crates/skynet-urdf/src/descriptor.rs` says why it is empty for every body imported from a URDF: *"URDF has no way to say
  'this is a gripper' — a hand is a joint and a link like any other"*. A body's `joints[]` has the finger, under a role
  (`…gripper.grip`) a person asserted, and no link from the end effector to it.
* **Which end of its travel is open.** A parallel gripper's joint runs from `lo` to `hi`, and a fraction needs to know whether
  `hi` is open (the Franka hand's finger runs from 0 closed to 0.04 m open) or closed (a hand whose joint closes it by
  growing, which is a convention each vendor chose for itself). Nothing in the descriptor, the role grammar or the limits
  says, and a guess in the wrong direction reports a closed hand as open.
* **What a hand with five fingers is.** `dexterous_hand` is a kind, `gripper.grip` is a channel, and "the opening" of the first
  is not a number between 0 and 1 that means the same thing as the second's.

So a daemon that wrote `robot.state.gripper` today would be inventing the one number a skill's first precondition exists to
check — *"the gripper is open, so there is somewhere for the workpiece to go and nothing already in it to crush"* — and a
guess in the wrong direction reads a closed gripper as open. That is the case §54.2's first rule exists to prevent, and the
branch is left absent for that reason (`runtime.rs`, `live_inputs`). Nineteen of the twenty-four skills in the Hub's
`skynet-skills` name this path (the plan's audit count) and so are refused, with the path named, on every robot.

## Why this is not the daemon's to fix

It is a fact about the *mechanism*, and the only author of it is whoever wrote the descriptor: a driver that knows it
(`drivers/mock`, a vendor's hand), or a person who asserted it on import. Computing it from joint limits and a convention would be
a second author of the same fact, wrong in the case that matters.

## Proposed answer

An end effector of kind `gripper` says which joint it is and where its open end is:

```json
"end_effectors": [{
  "name": "right_gripper", "kind": "gripper", "dof": 1,
  "joint": "gripper", "open_at": "max"
}]
```

* `joint` is a name in `joints[]`. A hand with several joints names the one whose travel is the opening, or none, and then there
  is no `robot.state.gripper` for it (absent, never an average).
* `open_at` is `"min"` or `"max"`: the end of **that joint's declared limits** at which the hand is open. The daemon's
  value is `(position - closed) / (open - closed)` clamped to `[0, 1]`, from the newest recorded cycle, and absent whenever the cycle is
  not a measurement or the joint has no limits.
* A body that declares neither has no `robot.state.gripper`, exactly as today; §54.2's refusal names the path, and a person
  adds the two fields.

`skynet urdf import` keeps saying it cannot know (`end_effectors` stays empty with its action note), and gains the two
keys in the note's text; nothing is inferred from a joint's name. A driver that builds its own descriptor
(`drivers/mock`, `drivers/mujoco`) can declare both, because it is the author.

## What it costs

A field on `end_effectors[]` in §5 and the descriptor's JSON schema, the same list in every edition and the corpus cases
(`embodiment`, and `live` for the new branch), a read in `live_inputs` beside the joints it already takes from the cycle, and
a reflection in `skynet skill check` so the path is addressable only on a body that declares it (item 12.F.3). Hub skills
need no change: they already read `0 closed to 1 open`.

## What it does not settle

Force, slip and "holding something": a gripper that closed on a workpiece reads `0.3`, not `0`, and that is a fact about the
workpiece. Perception of what is in the jaws is the recorded-and-not-solved part of 12.F.9.
