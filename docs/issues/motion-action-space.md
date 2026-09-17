# Proposed amendment — §84, a motion action space

**Status: ratified 2026-09-17 as §84 of the specification, and built.** Kept as the proposal it was, for
the reason `DRY-COVERAGE-PLAN.md` keeps its superseded findings; §84 is the text that holds, and it
records where the built form departs from this one — a fourth warning, `motion_parameter_unused`; a
fail-closed reading of `required`; and a match with a missing parameter blocked even on an optional
motion. The second half of the Phase 4 paragraph at the end is **§88, ratified 2026-09-17 and
built** — and the answer it got is not the one this file expected. It names the second blocker as a
missing channel from the supervisor to the process holding the driver; §88.1 finds that the channel
was never needed, because a skill over a program-level body is a policy and the ring a policy
already writes is the route. What was actually missing is the **admission rule** for a request
arriving on one.

It is the specification half of `DRY-COVERAGE-PLAN.md` Phase 3, whose *Decisions* 6 deferral expired
when Phase 1's gate was met. The implementation half — the corpus cases, then the editions, then a
mock program-level controller driver — waits on this, except for the driver, which is built:
`drivers/pendant`.

---

## 84. Amendment to §18 and §59.2 — a motion action space

§59.2 records the largest gap in this design in one sentence:

> **Transport has almost nothing to map.** There are no joint channels to bind; the action space is
> a set of named motions with parameters.

It is a true description of most of the world's installed industrial robots, and the design has no
way to *write it down*. `EmbodimentDescriptor` describes a body as joints and end effectors;
`SourceEmbodiment` describes a policy as indexed action channels; `resolveTransport` binds one to
the other. A Fanuc, KUKA, ABB or older Universal Robots controller offers none of those, so the
honest result for such a body today is a descriptor with an empty `joints` array and a transport
plan in which every channel is `blocked` — arithmetically correct, and it tells an operator nothing,
because *"this policy cannot drive this robot"* and *"this robot is not driven that way at all"* are
different sentences and only the second has a remedy.

This amendment adds the second sentence. It changes nothing about the first.

### 84.1 The body declares its motions

`EmbodimentDescriptor` gains **`motions`**, a list of the named motions the controller accepts:

```json
"motions": [
  { "name": "move_l",
    "motion": "move_linear",
    "parameters": [
      { "name": "pose",  "unit": "m",   "required": true },
      { "name": "speed", "unit": "m/s", "required": false, "min": 0.01, "max": 0.25 }
    ] },
  { "name": "close_gripper", "motion": "grasp", "parameters": [] }
]
```

* `name` is the controller's own spelling, and it is what a command carries.
* `motion` is the **canonical** motion this one is, from a closed vocabulary — `move_joint`,
  `move_linear`, `move_circular`, `grasp`, `release`, `home`, `wait`, `tool_change`, `io_set` — or
  absent where the vendor's motion is not one of them. §5.1's argument one level up: a name nobody
  standardised must be expressible without being mistaken for one that was, and a vocabulary is what
  lets `move_l` on one controller and `LIN` on another be the same motion.
* `parameters` is what the controller requires, and `required` is the field that earns the list. A
  motion whose name matches and whose required parameter a policy does not supply is a command the
  controller rejects at run time — exactly the class of failure transport exists to move to install
  time.
* `min` and `max` are the controller's declared bounds, **optional in the sense every rating in §5.2
  is optional**: absent means nobody declared one, never "unbounded". A consumer that substituted a
  plausible bound would be inventing a limit, which §5.2 forbids in the strongest terms this
  document uses.

**An empty `motions` is the default and means a body that is not commanded this way.** The field is
additive: every descriptor written before this amendment reads as a joint-commanded body, which is
what it is.

### 84.2 The policy declares the motions it emits

`SourceEmbodiment` gains **`motions`**, the same shape from the other side:

```json
"motions": [ { "name": "move_l", "motion": "move_linear",
               "parameters": ["pose", "speed"], "optional": false } ]
```

A policy declaring motions is a policy whose action space is motions. A policy declaring action
channels is one whose action space is joints. **Declaring both is refused**, and that refusal is the
point of this section rather than an edge case: they are two architectures, not two spellings, and a
package that claims both has not decided which machine it is for.

### 84.3 Transport over motions

`resolveTransport` returns a **motion binding** per declared motion, using §18's existing methods
with two of the six unavailable:

| Method | When |
|---|---|
| `direct` | the body declares a motion of the same `name`, and every parameter it requires is one the policy supplies |
| `remap` | no name matches, and exactly one body motion carries the same canonical `motion` |
| `drop` | no match, and the policy marked the motion optional |
| `blocked` | no match and it is required; or a name matched and a required parameter is missing |

**`retarget` and `synthesize` are not available over motions, and that is a rule rather than an
omission.** Retargeting means solving IK against a body's kinematics, and a program-level controller
exposes none: it owns the servo loop, which is §59.2's whole point. Synthesising means producing a
command whose *meaning* transfers — and a motion is a program the controller will execute, so a
runtime that invented one would be authoring a trajectory for a machine whose safety case belongs to
somebody else. A plan reporting `retarget` over a motion would claim an ability no component in this
design has.

A body and a policy that do not agree on the **kind** of action space produce one warning,
`action_space_mismatch`, and no bindings at all. Per-channel `blocked` results in that case would
describe a negotiation that never happened.

### 84.4 What this does not change

* **Tier.** §59.2's ruling stands: these robots are Guest tier permanently, because the vendor
  controller owns the servo loop and is the safety-rated component. Nothing here lets a
  motion-commanded body reach Host, and the Safety Kernel still never sees the commands.
* **The kernel.** No guard reads a motion. §7's guards are over joint commands and a motion is not
  one; a robot commanded this way is commanded past them, which §59.2 already says and this section
  does not soften.
* **§15.** No test is added. The suite is 36 tests and this does not make it 37: what `motions`
  needs is `embodiment.descriptor.valid`, which already requires a descriptor to parse against the
  schema this section extends.

---

## What ratifying it costs, in order

`DRY-COVERAGE-PLAN.md` Phase 3 has the order, and the order is the lesson §47.6 taught:
**the corpus cases before the editions.**

1. `reference/typescript/embodiment.ts` — the types and `resolveTransport`'s motion arm. The oracle
   is the definition; everything below reproduces it.
2. `tools/corpus-gen/generate.ts` — cases into `corpus/transport/motion.json`. At minimum: a direct
   match, a remap through the canonical vocabulary, a required parameter missing, an optional motion
   dropped, a required one blocked, both action spaces declared at once, and the kind mismatch in
   both directions.
3. `corpus/transport/motion.json`, regenerated. **Every case added moves §73's stated corpus count**
   (F8), so the figure moves with it in both copies, and the three complete-edition ratchets in
   `ci.yml`, `editions/cpp/Makefile` and `editions/go/Makefile` move too.
4. `crates/skynet-contracts` — `embodiment.rs`, `transport.rs`, `json.rs`.
5. `editions/python`, `editions/cpp`, `editions/go` — the other three complete editions. The four
   partial ones are held by their own floors and are unaffected.
6. `skynet transport` renders motion bindings; `skynet driver check` measures `motions` under
   `embodiment.descriptor.valid`.

**Phase 4's first item waits on this too**, and on a second decision beside it: §33.1 keeps
`skynetd` off the path that holds a driver, so `execute({ skill, arguments })` has to reach
`skynetk` and there is no channel from the supervisor to it. That is a decision of the same weight
as this one and belongs in the same issue.

`drivers/pendant` is already there to test it against: a driver that declares motions and no joints,
refuses `write_command` with `SKR_EPERM`, and executes a motion through `extension()`.
