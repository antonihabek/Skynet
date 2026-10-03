# Amendment — §5's base kinds have no word for water

**Status: ratified 2026-09-29 and carried as §97 on 2026-10-02** (`DRY-COVERAGE-PLAN.md` item 12.G.1), as proposed
below: two kinds, both mobile. `drivers/mavlink` drives a boat, and since 2026-10-03 a submarine too
(`DRY-COVERAGE-PLAN.md` item 12.E.4: ArduSub's own firmware, `base: submerged`). Proposed 2026-09-29, by `DRY-COVERAGE-PLAN.md` Phase 11 (F178), found while building
`drivers/mavlink`. It was not carried then: `BASE_KINDS` is a closed set every edition spells identically
(`crates/skynet-contracts/src/embodiment.rs`, `crates/skynet-urdf/src/descriptor.rs`, and the same list in
each other edition), so it is the owner's to ratify and a maintainer's to carry.

## The gap

§5's descriptor says what a body's base is, and the closed set is `fixed`, `wheeled`, `tracked`,
`legged`, `aerial` and `rail`. §87 reads it: an absent basis on any base but `fixed` is
`basis_undeclared`, and `power.rs` keeps `MOBILE_BASES` beside it for the reserve arithmetic.

A hull is none of the six. ArduPilot's own firmware builds a surface boat (`MAV_TYPE_SURFACE_BOAT`, a
rover with `FRAME_CLASS` 2) and a submarine (`MAV_TYPE_SUBMARINE`, ArduSub), both speak the same MAVLink
this tree now drives, and both ran against `drivers/mavlink`'s substrate on 2026-09-29. The driver
refuses them by name:

```
the vehicle reports MAV_TYPE 11 (a surface boat). §5's base kinds are fixed, wheeled, tracked, legged,
aerial and rail: none is a hull, and calling one `wheeled` would make a false statement to every guard
that reads it.
```

What the kind is read for today is one question, in `power.rs`: `base_is_mobile`. A mobile base owes §87.2
a safe-state figure and the basis it was arrived at; `fixed`, and a kind left out, do not. So a boat has two
ways to be wrong and no way to be right. Leave the kind out and it is read as bolted down, which owes §87
nothing — the mobile body escaping the mobile-body rule, in the direction that hurts. Say `wheeled` and it owes
§87 what it should, under a false name that every other reader of the kind takes as a fact about the
mechanism: a console, the Hub's compatibility engine, a policy declaring which bases it fits (§6's
`synthesize` is defined as *"base velocity from a legged policy onto a wheeled base"*).

## Why this is not the driver's to fix

A driver can declare only what §5 lets a body say. The vocabulary is the owner's, for the reason §5.1
gives roles: an extension nobody standardised must be expressible without being mistaken for one that
was.

## Proposed answer: two kinds, and what each means to the reserve

1. **`surface`** — a body on the water's surface: a boat, a hovercraft, a sailboat.
2. **`submerged`** — a body under it: a submarine, a remotely operated vehicle.
3. **Both are mobile.** `MOBILE_BASES` gains both, so §87.2's rule applies to them as it does to the other
   five. What a reserve figure means over water — a boat has no brake and stops as current and wind allow —
   is §87's to say when a driver first declares one; this amendment only makes the body ask the question.
4. **Nothing else changes.** No role, no guard and no signed field moves; the set gains two words, and
   the corpus gains a case for each in `corpus/embodiment`.

## What `drivers/mavlink` does without it

Refused a boat and a submarine at `open()`, by name and with the reason above, and pointed here. The
substrate proved the refusal against ArduPilot's own boat and submarine (`live.py boat`, `live.py sub`),
so the day the set gained the words, the two refusals were the two lines to change and there was a
firmware to run the change against: the boat on 2026-10-02, and the submarine on 2026-10-03, which needed
more than a word (ArduSub's own modes, sticks to move it, and a stop the operator names).
