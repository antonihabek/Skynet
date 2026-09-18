# Proposed amendments

**One file here is proposed and not ratified**: `ethercat-reference-driver-is-not-soem.md`,
filed 2026-09-18 from `DRY-COVERAGE-PLAN.md` F48.
**The other seven have been ratified and built** — `declared-versus-measured-limits.md` and
`who-checks-the-real-time-kernel.md` carried 2026-09-18 on the owner's instruction to complete
Phase 6 — and each is kept as the proposal it was
rather than deleted — the specification is the text that holds, and the value of a proposal
afterwards is the record of where the built form departed from it. Five departed substantially:
§85 rejected this directory's two-segment token rule, §88 found the blocker
`motion-action-space.md` named had dissolved rather than needing the bridge it proposed, §89's
MuJoCo driver took no lock where `sensor-reads-off-the-control-thread.md` sketched one, §5.3
folded `declared-versus-measured-limits.md`'s §44 half into itself (§44 holds no envelope field
list) and disambiguated `envelope.basis` from §87's safe-state `basis`, and `who-checks` gained
the harness override the existing `--tier host` tests needed. The rule
below is why they were files here first, and it stands for the next one.

**Nothing in this directory is a change on its own.** `CONTRIBUTING.md`:

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
| [`motion-action-space.md`](motion-action-space.md) | §18, §59.2 | Phase 3 items 1–3 — **ratified as §84**; Phase 4 item 1 — **ratified as §88** |
| [`capability-token-vocabulary.md`](capability-token-vocabulary.md) | §5.0, §8 | Phase 4 item 3 — **ratified as §85** |
| [`phase-5-class-gates.md`](phase-5-class-gates.md) | §43, §53 | Phase 5, all four items — **ratified as §87** |
| [`per-joint-command-modes.md`](per-joint-command-modes.md) | §24, §7.1 | a mixed-mode body at all — **ratified as §86** |
| [`sensor-reads-off-the-control-thread.md`](sensor-reads-off-the-control-thread.md) | §24, §47.2, §7 | a sensor slower than the control period, without the period paying for it — **ratified as §89** |
| [`ethercat-reference-driver-is-not-soem.md`](ethercat-reference-driver-is-not-soem.md) | §4 | a sentence that tells a vendor to build on a GPLv3 stack — **proposed** |
| [`declared-versus-measured-limits.md`](declared-versus-measured-limits.md) | §5, §44, §63.4 | keeping a cited design value from reading as a measured rating — **ratified as §5.3** (§44 carried there too: it holds no envelope field list) |
| [`who-checks-the-real-time-kernel.md`](who-checks-the-real-time-kernel.md) | §1, §23.5 | refusing `--tier host` on a machine that cannot back the claim — **ratified as proposed** |
