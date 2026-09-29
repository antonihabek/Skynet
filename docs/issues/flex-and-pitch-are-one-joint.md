# Amendment — §5.1 has two words for a hinge, and §6 never joins them

**Status: proposed 2026-09-29**, by `DRY-COVERAGE-PLAN.md` Phase 10 (F177), found while building item
10.8. Not carried: §5.1's vocabulary is a closed set every edition spells identically
(`corpus/roles`), and §6's matching rule is a contract the Hub reads, so both are the owner's to
ratify and a maintainer's to carry to both copies.

## The gap

§5.1's axis set has `pitch` and it has `flex`. §67.2 uses `flex` for the SO-100/101's elbow and
wrist (`elbow_flex → right_arm.elbow.flex`), and every LeRobot policy that was recorded on one of
them carries those roles. The importer uses `pitch` for the same physical joint on every other arm:
since F166 it reads a joint's axis in the body's frame, and a horizontal hinge is a pitch, so a UR
imports as `right_arm.elbow.pitch`, `right_arm.wrist.pitch#0`. Both are right. They are two names for
one joint, and §6 binds a channel to a joint by **exact role key**:

```
✗ [2] right_arm.elbow.flex → (none)
    blocked — This body has no right_arm.elbow.flex and the policy requires it.
```

That is the transport plan for an SO-101 policy onto a Universal Robots arm whose roles the importer
suggested, accepted and got right. The fleet measures it on every arm the importer can name: before
item 10.8 the `transport` stage bound the menagerie's SO-101 and no other arm, the UR family among the
refused (`reference/reports/fleet.json`, 2026-09-28).

## Why this is not the importer's to fix

Renaming the importer's output to `flex` would move the mismatch to every humanoid, whose G1 policy
uses `elbow.pitch`, and to the quadrupeds §94 just gave `knee.pitch`. Two conventions are already in
use, each on the robots it was written for; the rule that joins them has to be §6's.

## Proposed answer: equivalence in the match, not a new canonical form

1. **`flex` and `pitch` are the same axis for a link that has one hinge.** §6's role match treats
   `X.L.flex` and `X.L.pitch` as equal when `L` is `elbow`, `wrist`, `knee`, `ankle`, `finger` or
   `knuckle` — the links whose motion is a single bend — and only there. A shoulder has a `pitch`
   and no `flex`; a hip has both an `abduct` and a `pitch`, and `flex` there stays a different word.
2. **The plan says which it did.** A binding made through the equivalence is `remap` with the reason
   *"Mapped by role right_arm.elbow.flex to `elbow_joint` (flex ≡ pitch on an elbow)."* so an
   integrator can see the assertion the match relied on.
3. **`parseRole` and the descriptor do not change.** No signature, digest or stored role moves; only
   the comparison does, in `resolve_transport` and its three other editions, with cases in
   `corpus/transport` for a bind, a refusal on a shoulder, and a refusal where both `flex` and
   `pitch` exist on the same link of one body.

## What item 10.8 does without it

`fleet/roles.py` asserts the LeRobot spelling on each arm family whose geometry a table can be
checked against, so a numbered arm binds today. That is a workaround at the wrong layer: it hides
the mismatch for the 88 descriptions it lists, changes what the importer's own suggestion is called on
the ones it could already name, and leaves every other integrator to write the same table.
