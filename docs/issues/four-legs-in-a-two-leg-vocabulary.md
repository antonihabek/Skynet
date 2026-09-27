# Amendment — §5.1 says its chains cover quadrupeds, and no quadruped gets a role

**Status: proposed 2026-09-26**, by `DRY-COVERAGE-PLAN.md` Phase 10 (F163). Not carried: §5.1's
vocabulary is a closed set every edition spells identically (`corpus/roles`), so a convention for it
is the owner's to ratify and a maintainer's to carry to both copies.

## The gap

§5.1 has two leg chains, `left_leg` and `right_leg`, and §43's governance row says the twelve
chains *"cover humanoids, arms, quadrupeds and wheeled bases"*. A quadruped's four legs are
expressible only through the index — `left_leg.hip.abduct#0` and `left_leg.hip.abduct#1` — and
nothing says which index is the front leg. Two vendors' quadrupeds that both used the grammar could
therefore disagree about which of their legs is which, and §6's role match would bind a policy's
front-left hip to a rear-left hip without a warning, because the roles would be equal strings.

In practice nobody reaches that far. `crates/skynet-urdf`'s suggester recognises the words for a
side, and a quadruped's joints name their leg by a corner — Unitree's `FL_hip_joint`, ANYbotics'
`LF_HAA`, Boston Dynamics' `fl.hx` — so every quadruped imports with **no roles**, and every
cross-body transport onto or off a quadruped is by exact joint name or not at all. The fleet sweep
(`reference/reports/fleet.json`) measures it on every public quadruped description: none carries a
single suggested role.

## Proposed answer: a convention for the index, not a new chain

1. **The index on a leg chain counts from the front.** `left_leg.*#0` is the front-left leg,
   `left_leg.*#1` the one behind it, and so on for a hexapod. A biped's legs carry no index, which
   is what every humanoid descriptor already writes, so no existing descriptor changes meaning.
2. **Hip axes.** The sideways swing of a quadruped hip is `abduct`, already in the axis set; the
   fore-aft swing is `pitch`; a knee is `knee.pitch`. Written down so two importers make the same
   choice.
3. **The suggester** learns the corner spellings in use — `FL/FR/RL/RR`, `LF/RF/LH/RH`,
   `fl/fr/hl/hr`, `front_left`/`rear_right` — as side plus index, and labels every such suggestion
   `named` only when both came from the name.
4. **The corpus** gains the corner forms under `corpus/roles` so every edition parses a front and a
   rear leg the same way; `parseRole` itself does not change, because the grammar already admits it.

## What it does not change

The grammar, the chain set and every descriptor written so far. It closes a convention gap, and it
is the one thing standing between the marketplace's second-largest class and §6's remapping.
