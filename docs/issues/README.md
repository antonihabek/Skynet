# Proposed amendments

**Everything in this directory is an issue, not a change.** `CONTRIBUTING.md`:

> If you think the specification is wrong, open an issue rather than a pull request against it — a
> maintainer carries the change to both copies.

Each file is one amendment written out in the form it would take, so that ratifying it is one edit
in each repository plus `node tools/check-mirrors.mjs --pin ../RobotOverflow`, followed by the
implementation order the file names.

They exist because of a finding rather than a preference. `DRY-COVERAGE-PLAN.md` F29: the plan's
later phases are gated on **design decisions**, not on a robot — which is the opposite of the
constraint the plan is named for.

| File | Amends | Unblocks |
|---|---|---|
| [`motion-action-space.md`](motion-action-space.md) | §18, §59.2 | Phase 3 items 1–3; Phase 4 item 1 — **ratified as §84** |
| [`capability-token-vocabulary.md`](capability-token-vocabulary.md) | §5.0, §8 | Phase 4 item 3 — **ratified as §85** |
| [`phase-5-class-gates.md`](phase-5-class-gates.md) | §43, §53 | Phase 5, all four items |
| [`per-joint-command-modes.md`](per-joint-command-modes.md) | §24, §7.1 | a mixed-mode body at all — **ratified as §86** |
