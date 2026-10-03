# Proposed amendments

**Four were filed by `DRY-COVERAGE-PLAN.md` Phase 10** from what the fleet sweep measured on every
public robot description, in the last four rows. Three were filed 2026-09-26 and the owner ratified
all three on 2026-09-28; the fourth was filed 2026-09-29 and is **proposed**. A fifth, filed the same
day by Phase 11 from what ArduPilot's boat and submarine showed, is **proposed** too. Each row says how far
it has been carried.

**Eleven of the fourteen were ratified and built before them.** The last two were filed 2026-09-20 and both
correct Part VII against the tree and against the vendors rather than settling a design question —
see the bottom two rows. They were carried 2026-09-21 on the owner's instruction to implement the
plan fully.

**The ratified nine** — `declared-versus-measured-limits.md` and
`who-checks-the-real-time-kernel.md` carried 2026-09-18 on the owner's instruction to complete
Phase 6, and `ethercat-reference-driver-is-not-soem.md` (F48) and `stop-latch-lifetime.md` (F76)
carried together 2026-09-19 on the instruction to finish every section of the plan — and each is
kept as the proposal it was
rather than deleted — the specification is the text that holds, and the value of a proposal
afterwards is the record of where the built form departed from it. Six departed substantially:
§85 rejected this directory's two-segment token rule, §88 found the blocker
`motion-action-space.md` named had dissolved rather than needing the bridge it proposed, §89's
MuJoCo driver took no lock where `sensor-reads-off-the-control-thread.md` sketched one, §5.3
folded `declared-versus-measured-limits.md`'s §44 half into itself (§44 holds no envelope field
list) and disambiguated `envelope.basis` from §87's safe-state `basis`, `who-checks` gained
the harness override the existing `--tier host` tests needed, and
`part-vii-says-none-of-it-is-built.md`'s "sixteen drivers" is not restated — the count went stale
between filing and carrying, which is F86's own argument about figures nothing checks. The rule
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
| [`ethercat-reference-driver-is-not-soem.md`](ethercat-reference-driver-is-not-soem.md) | §4, §62.1 | a sentence that told a vendor to build on a GPLv3 stack — **ratified as proposed**, and carried to §62.1's table too |
| [`declared-versus-measured-limits.md`](declared-versus-measured-limits.md) | §5, §44, §63.4 | keeping a cited design value from reading as a measured rating — **ratified as §5.3** (§44 carried there too: it holds no envelope field list) |
| [`who-checks-the-real-time-kernel.md`](who-checks-the-real-time-kernel.md) | §1, §23.5 | refusing `--tier host` on a machine that cannot back the claim — **ratified as proposed** |
| [`stop-latch-lifetime.md`](stop-latch-lifetime.md) | §4, the RHAL header | a stop that outlived its session, and a suite that never opened a second one — **ratified as proposed**, gated by `skynet driver check`'s second session |
| [`program-level-robots-are-a-narrower-class.md`](program-level-robots-are-a-narrower-class.md) | §59.2, §59.7, §61 | the tier ceiling on modern ABB, FANUC and KUKA controllers, which sell a 250 Hz external motion interface §59.2 did not know about — **ratified as proposed**, with conforming edits where §§84/88 restated the narrowed claim |
| [`part-vii-says-none-of-it-is-built.md`](part-vii-says-none-of-it-is-built.md) | §59.5, §61 | §59.5 and §73 contradicting each other about whether the kernel, the daemon, the drivers and the corpus exist — **ratified as proposed**, except the driver count, which had gone stale between filing and carrying and is not restated |
| [`the-plan-never-reaches-the-kernel.md`](the-plan-never-reaches-the-kernel.md) | §6.3, §33.1, §73 | a joint's rating reaching the guard on a robot: `skynetk.service` starts the loop with no plan, so the envelope guard holds nothing (F158) — **ratified as §93** |
| [`travel-has-two-ends.md`](travel-has-two-ends.md) | §6, §7.1, §7.3 | the narrow end of a joint's travel, which the kernel never held because a position ceiling is one magnitude (F159) — **ratified as §92** |
| [`four-legs-in-a-two-leg-vocabulary.md`](four-legs-in-a-two-leg-vocabulary.md) | §5.1, §43 | any role on any quadruped, and so any cross-body remapping onto one (F163) — **ratified as §94** |
| [`flex-and-pitch-are-one-joint.md`](flex-and-pitch-are-one-joint.md) | §5.1, §6 | an SO-101 policy binding to a Universal Robots arm whose roles the importer got right, because the vocabulary has `flex` and `pitch` for one hinge and §6 matches by exact key (F177) — **ratified 2026-09-29, carried as §98** (2026-10-02, item 12.G.1); item 10.8's role tables stay for the arms the importer cannot name at all |
| [`fault-flag-is-not-read.md`](fault-flag-is-not-read.md) | §24, §27 | a drive that has faulted and ignores commands leaving the operating state at `present`, because §27 reads `RHAL_DEGRADED` and `RHAL_ESTOP` and nothing reads `RHAL_FAULT` (F197) — **proposed**; the drivers were made to work inside §27 as written, and whether a faulted body is a stopped one is the owner's |
| [`no-base-kind-for-water.md`](no-base-kind-for-water.md) | §5, §87 | a boat or a submarine having a base kind at all: `BASE_KINDS` has no word for a hull, so ArduPilot's own boat and submarine were refused by name rather than called `wheeled` or left out, in opposite directions of wrong (F178) — **ratified 2026-09-29, carried as §97** (2026-10-02, item 12.G.1); the boat and the submarine are now driven |
| [`gripper-has-no-open-end.md`](gripper-has-no-open-end.md) | §5, §54.2 | `robot.state.gripper`, which every Hub skill that names it is refused for on every robot: the branch is defined as 0 closed to 1 open and nothing in the descriptor says which joint is the gripper or which end of its travel is open, so the daemon cannot fill it without a guess in the direction that hurts (item 12.F.2) — **proposed** |
| [`skill-clauses-name-one-bodys-joints.md`](skill-clauses-name-one-bodys-joints.md) | §5.1, §8, §54.2 | the 24 Hub skills read five joint names, all the SO-101's, so a clause is refused on every other body — and mapping the names would only turn the refusal into a pose judged against another body's numbers; a body-keyed parameter table, and a role-keyed branch for what is body-independent, are the proposed answers (item 12.F.4) — **proposed**; `skill.context.on_body` is the check, and one package per body is the rule until it is decided |
| [`planner-holds-what-the-host-refuses.md`](planner-holds-what-the-host-refuses.md) | §6.1, §93.2 | a skill whose plan the planner accepts (*"held by the controller"*) running at all: the kernel host writes channel *i* to joint *i* and refuses any plan that leaves a joint out, so the three five-channel skills cannot run on the SO-101 they were taught on, and no check before the robot says so (F218) — **proposed**; the measurement is built, the channel map is the owner's |
