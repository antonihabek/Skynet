# Dry coverage plan — 2026-09-16 onward

Coverage work carried out **without a physical robot**, and the mechanisms that keep the
untested half visible rather than implied. Kept beside [`BUILD-LOG.md`](BUILD-LOG.md), which is
the session record; this file is the plan and its progress log.

> **The constraint that shapes everything below.** No step here is verified against hardware.
> That is a decision the owner made, not an omission, and the plan's first job is to make it a
> property of the build rather than a sentence in a README.

**Nothing here is pushed.** Commits are held on `main` locally until asked for.

---

## What "full coverage" is allowed to mean under this constraint

§1 refuses the strong reading of *universal* and §59 refuses it again at length. This plan adds a
third refusal, narrower than both, because it is the one that applies to a build with no robot in
the room:

**Every robot with a public control interface gets a conformant driver, exercised against a fake or
a simulator, and no robot is awarded a tier.**

The last clause is not pessimism. It falls out of §15's own arithmetic, and Phase 0.1 pins it as a
test rather than leaving it as an inference — see the finding below.

Three things stay outside the claim no matter how much of this plan lands:

| Outside | Why |
|---|---|
| Robots with no documented control interface | An access problem. §59.1 |
| Host or Certified on program-level controllers (Fanuc, KUKA, ABB, Spot) | The vendor controller owns the servo loop and is the safety-rated component. §59.2, "not fixable in software" |
| Task success after retargeting | A research result nobody has. §59.4, §61 |

---

## Findings that changed the plan before any code was written

### F1 — Dry evidence awards `none`, not `guest`. One test stands in the way.

The plan as first drafted assumed dry work would land every robot at Guest tier, and proposed a new
CI assertion to hold it there. Reading `crates/skynet-contracts/src/certification.rs` found that
assertion largely unnecessary and its premise wrong.

`SCTS_TESTS` is 36 tests: **17 `sim`, 19 `bench`, 0 `field`** — field conformance is a ratio on
`CertificationInput`, not a per-test suite. Of the 19 bench tests, 18 are required from `host` or
`certified`, so a dry run being `missing` on them caps the award at guest by construction. That much
the design already does, and `absence_is_not_a_pass` plus
`the_evidence_caps_the_award_rather_than_the_claim` already hold it.

**But exactly one bench test is required from `guest`:**

```
offline.control.independent    suite: bench    area: offline    required_from: guest
```

So a sim-only run is `missing` on a guest-required test, and `award_certification` returns
**`none`** — no tier at all. The dry ceiling is one step lower than the plan claimed, and the
difference is a single row of §15's table.

### F2 — The question F1 raises is a specification decision, not a patch.

`offline.control.independent` measures §11's invariant: no control decision requires a network round
trip. That does **not** need a body. It needs a real machine — real Linux, real systemd, a real
kernel turning a loop — which is exactly what `packaging/virtual-robot.sh` produces.

So there is a real question: is that test `bench` because it needs *hardware*, or because it needs
*a machine rather than a model*? A virtual robot is the second without the first.

**This plan does not answer it.** CONTRIBUTING.md is explicit that a specification change is an
issue a maintainer carries to both repositories, never a pull request against the pinned document —
and reclassifying a bench test to `sim` is precisely the move Phase 0.1b exists to prevent anyone
making quietly. Raised, not resolved — and **resolved as 4 under *Decisions*** below, which keeps it `bench`.

### F3 — Phase 0.4 (limit provenance) is already built. Twice.

The original plan asked for a marker distinguishing declared limits from measured ones, on the
grounds that a descriptor generated from a URDF or a simulator model would otherwise be treated as
sourced. Both halves already exist:

* `embodiment.limits.sourced` — a `sim` test, required from `host`: *"Every declared limit cites a
  source. A rating with no provenance fails, because the safety kernel would be enforcing a
  rumour."*
* CI enforces that `skynet-urdf` writes a literal `Json::Null` envelope and that nothing else can
  reach the field, with the failure message citing §63.4: a URDF effort limit is a design value from
  a CAD model, not a datasheet safety rating.

No code. Recorded here so it is not rebuilt.

### F3a — Extending the dry ceiling to the other seven editions is blocked on this machine.

`corpus/certification/award.json` has twenty cases and **none of them is the sim-only shape.**
`no-evidence` is an empty result set and `guest-complete` passes the bench test too, so nothing holds
the eight language editions to F1 — only the Rust test added in 0.1 does.

A corpus case is the right fix and it cannot be produced here. Two independent blockers:

* **Zig is not installed**, and `tools/edition-report.mjs --check` re-runs *every* edition to
  re-derive `reference/reports/editions.json`. Adding a case changes the figure, and the check would
  go red on something this machine cannot regenerate.
* **Node here is v26; the `corpus` job pins Node 24.** `ci.yml` is explicit that the case tables are
  read out of the engine the reference runs on and that the Unicode version differs between the two.
  Regenerating the corpus under the wrong one risks a diff nobody asked for.

Left undone deliberately rather than half-done. It is the first item in *Next* below.

> **Done, 2026-09-16, and both blockers above were wrong.** Zig 0.16.0, Node 24 and .NET 8 all
> install in minutes on a machine that is not this one, so the three cases exist and all eight
> editions were run against them. The claim that `edition-report.mjs --check` re-runs *every*
> edition is also wrong — it re-runs the four complete ones, and Zig is held by its own ratchet.
> What genuinely blocks a corpus case is F8: §73 states the case count exactly. Kept rather than
> rewritten, because a blocker that turned out to be a property of one laptop is worth being able
> to recognise the next time one is written down.

### F4 — Most of Phase 0.1's proposed machinery already exists as an invariant.

`ci.yml`'s `invariants` job already greps both halves of the §35.3 simulator cap — in
`certification.rs` and in `skynet-cli`'s tier walk — so a simulated body being capped at guest is
already a check rather than a comment. What it does **not** cover is a real driver with no hardware
behind it, which sets no `simulator` flag and is not lying when it doesn't. That gap is real and
narrow, and F1 is what actually closes it.

---

### F5 — The production-caller check found two on its first run.

Both are the historical shape exactly, and neither is a crash:

* **`driver_may_activate`** (`skynetd-io/src/admit.rs`) — §19's rule that only a `verified`
  signature admits a driver. `runtime.rs:519` enforces it **inline** and is the live copy; this
  spelling is called by nothing. Not a safety hole, because the rule *is* enforced — but it is §78's
  shape, where one fact with several spellings wants a mechanism rather than care. Which copy
  survives is the owner's call.
* **`verdict_inputs_all_recorded`** (`skynet-blackbox/src/assemble.rs`) — §26.4. The black box can
  read a sealed window back and replay its cycles through the kernel, and never reports whether the
  window recorded every field a replay evaluates. A replay of an incomplete window is presented
  exactly like a replay of a complete one.

### F6 — §73's line count has two lines of headroom, and 0.1 spent most of it.

`tools/scale-report.mjs --check` holds §73's stated size to the tree, rounded to the nearest
thousand. The tree was at 136,427 lines of Rust — 73 lines below the 136,500 boundary where the
figure rounds to 137,000.

0.1's first draft added 103 lines and broke the check. The fix would have been a one-word edit to
§73, which is in the pinned specification and therefore an issue rather than a patch — so the tests
were rewritten to carry their argument by reference to this document instead of restating it, landing
at 71 added lines and **136,498 total.**

**Two lines of headroom.** The next person to add Rust will trip this, and the honest resolution then
is the §73 edit rather than more compression.

### F7 — `main` was red on three counts when Phase 0 was written, and the gate did not say so.

The 2026-09-16 entry below records the full pre-PR gate passing. It did pass. CI on the same commit
(run 60, `682d9b0`) failed three jobs, and none of the three is reachable by the gate
`CONTRIBUTING.md` asks for:

| Job | Failure | Why the gate missed it |
|---|---|---|
| `rust` | `clippy::byte_char_slices` on `skynet-ring-shm/src/pass.rs:187` | The lint landed in clippy 1.98. The machine that ran the gate had stable at 1.94 |
| `invariants` | `TS4104` on `tools/unicode-case.ts:643` | The oracle type-check is a CI step; the gate has no TypeScript in it |
| `corpus` | the Go edition's fusion self-check | The gate has no edition in it either — `editions/README.md` is a separate instruction, conditioned on *"if you changed an edition"*, and nobody had |

All three are fixed. The general lesson is the first row: **a pre-PR gate is only as current as the
toolchain under it**, and a clippy release can turn a green tree red with no commit in between.

The Go one is worth its own line, because the check was right and the fact was wrong. Go does not
contract `x*y + z` on amd64 at *any* `GOAMD64` level — v1, v2, v3 and v4 all emit `MULSD` then
`ADDSD` — so `fusingTargets` had been claiming a fusion the compiler has no rule for, and the scan
refused the target rather than reporting a listing it could not see into. The row survives because
an explicit `math.FMA` still reaches an amd64 binary as `VFMADD231SD`, which is a single rounding by
definition and exactly what §31.2 forbids; `fusionprobe.Intrinsic` is now its positive control, and
on that target the scan is a check for the call rather than redundant cover for the other five.

### F8 — F3a named two blockers, and neither was the binding one.

Zig and Node 24 were real on the machine that wrote the plan and are not properties of the work.
Both installed here in under a minute, along with .NET 8, so **all eight editions and the corpus
generator ran on one machine for the first time.**

What actually blocks a corpus case is one line of the pinned specification. §73 states the corpus
size *exactly* — `a corpus of 80 files and 1,199 cases` — and `scale-report.mjs --check` holds the
document to a measurement of the tree, refusing with *"correct the document, not this check."* So
**every corpus case anyone adds, ever, requires an edit to `docs/skynet-os-architecture.md`.** F6
predicted this arriving through Rust lines; it arrives through the corpus as well, and more often,
because cases are added more freely than crates.

Also worth recording against F3a's own text: `edition-report.mjs --check` re-runs the **four
complete** editions, not every edition. Zig is held by its own `--min-passed` ratchet in CI and is
not on that file at all — which is the refusal inside `edition-report.mjs` working as designed.

### F9 — 0.2's own check had two defects, and the second one lies.

Both found by opening a file a finding named and not seeing the function there.

* **Wrong line, 18 times out of 38.** `strip()` removed comments and string literals *including
  their newlines*, and every line number is counted off the stripped text, so each block comment
  above a declaration shifted it upward by its own height. `upload_target` was reported at
  `upload.rs:545` and is declared at 742.
* **Blind to 500 lines, and one caller.** `strip()` had no case for a character literal, so `'"'` at
  `upload.rs:230` opened a string the scan closed at the next quote far below, swallowing the call
  site at `upload.rs:389` whole. `upload_target` **has** a production caller and was allow-listed as
  though it did not, with a reason written to fit.

The second is the one that matters. A false positive on this check is an argument somebody has to
write down; **a false negative is an answer nobody goes and checks** — and the remedy the check
offers for a false positive is to add a name to a list, which is how a real caller got denied in
writing. Five other files spell `'"'`; none happened to hide another sole caller, so the count moved
by exactly one.

### F10 — §36.1's operator notification is `Transition::warning`, four defects later.

Found by draining the baseline, and the clearest vindication of 0.2 so far.

A power state derives three facts. `power_to_json` serialises `requires_safe_state` and
`stops_immediately`, and the CLI renders both. **`notifies_operator` is in neither the document the
daemon serves nor the daemon itself** — nothing under `crates/skynetd-io/src` or
`crates/skynetd/src` matches on `PowerState::Low`, `Reserve` or `Critical` at all. The row that says
an operator is told is computed, tested, and told to nobody.

The good half, because it changes what the finding means: §36.1's *withholding of new task leases*
**is** enforced, by `power_refusal_from` on the reconcile pass (`runtime.rs:1761`). A robot below its
reserve stops taking work. What it does not do is say so to a person.

Closing it is a wire change — a field on the power document, the TypeScript oracle, the corpus's 28
power cases and four editions — so it was raised rather than done. **Decided as 2 under *Decisions*:** wired.

---

## Phase 0 — the dry-mode contract

Buys no coverage. Everything after it is unsafe without it.

| | Item | State |
|---|---|---|
| 0.1 | Pin what dry evidence awards, as a test | **Done** — 2 tests in `certification.rs`, positive control verified |
| 0.1b | CI invariant: the bench suite cannot shrink | **Done, as the same test** — see below |
| 0.2 | CI check: every capability has a production caller | **Done** — `tools/check-production-callers.mjs`, wired into `ci.yml` |
| 0.3 | `virtual-robot.sh` as the harness of record | **Done by this document** — it already runs on every push; this is the record saying it is the one |
| 0.4 | Limit provenance | **Already built — F3** |

**0.1b did not become a `ci.yml` grep, deliberately.** The Rust test runs in the blocking `rust` job
and names the test that moved when it fails, so a grep over the same source line would be a second
record of a fact the first one already holds — which this codebase argues against for the reason §80
gives about the partial's length: a second record of a fact disagrees exactly once, after the failure
that matters. What a Rust test *cannot* reach is the other seven editions, and that is F3a.

**0.2's justification** is four incidents in this repository's own history, all the same shape — a
path that existed and was never reached by anything but a test:

* `Transition::warning` reached the daemon's observe step and was dropped; nothing had ever made a
  sound.
* `can_start` had four test callers and no production caller; `skynetk` turned a cycle without
  consulting the start gate.
* `EventCode::GrantChanged` had exactly one producer, so the ring recorded every grant a Hub made
  and none an operator made standing at the robot.
* `available.proximity_reading` was hard `false` in two files, each carrying the note *"the day a
  producer exists, this is the line that changes"*.

A dry build multiplies this failure mode, because a fake is easier to satisfy than a robot.

---

## Phase 1 — close the loop, dry

| Work | Dry substrate | Note |
|---|---|---|
| Capture backend | Synthetic — PNG sequence or MuJoCo render — behind the trait V4L2 and GenICam will share | Deterministic, so frames are replayable and corpus-able. V4L2 cannot be dry-verified at all |
| Inference backend (§23.3) | ONNX Runtime, CPU | Runs in CI, deterministic. This workspace's first third-party dependency, **allowed by the owner on 2026-09-16** under the narrow terms in *Decisions* 5 |

**Gate:** `skynetk` offers the frame pool, `skynetp` maps it, an ONNX policy closes the loop against
the MuJoCo driver for N cycles with zero overruns, then the sealed window is read back and replayed
through the kernel to identical verdicts. `skynet-blackbox` already does read → assemble → replay,
so the last step is a proof rather than a smoke test.

---

## Phase 2 — drivers, and how far each goes dry

| Driver | Dry substrate | Dry-verifiable | Not dry-verifiable |
|---|---|---|---|
| `ros2_control` bridge | Fake `hardware_interface` | Adapter, resource-manager mapping, descriptor generation | Jitter under load |
| CANopen / CiA402 | Linux `vcan` + CiA402 slave sim | Nearly all of it — best dry fit in the set | Bus bandwidth at real joint × rate |
| EtherCAT / SOEM | Software slave stack | CiA402 state machine, frame encoding | **Distributed clocks and FSoE STO — the whole basis of its Certified ceiling** |
| UR RTDE | URSim (UR's own controller, containerised) | Almost everything, against real vendor software | Servo timing |
| Unitree SDK2 | SDK sim | Protocol, low-level mode mapping | Joint access at rate |
| `libfranka` / FCI | Protocol fake from public docs | Encoding, state machine | 1 kHz torque, which is its entire claim |

EtherCAT is the honest warning in this table: everything is dry-verifiable **except** the two
properties that justify its ceiling. A green CI on that driver must not be allowed to imply
otherwise.

---

## Phase 3 — motion-primitive action space

Highest coverage-per-effort item, and the most dry-verifiable, because it is contract work and the
corpus plus eight editions plus the TypeScript oracle is exactly that machinery.

**Requires a specification change** (a descriptor field and new transport methods), so it starts as
an issue, not a branch — and **Decisions* 6 defers it behind Phase 1's gate rather than opening one now.

Order matters, and the repo has already paid for the lesson: **write the corpus cases before the
editions.** Doing it that way on §47.6 found the specified rule and the reference disagreeing about
which released slot a publish takes — a bug that would have cost a reader its frame on nearly every
cycle while presenting as a slow policy.

1. Descriptor: action-space kind, named motions with parameters
2. Transport bindings over motions rather than joint channels
3. Corpus cases, then the editions
4. A mock program-level controller driver — the teach-pendant analogue of `drivers/mock`

Ends fully dry-verified, because there was never a clamping claim to verify.

---

## Phase 4 — skills coverage

1. Route `execute({ skill, arguments })` to a command-level driver. §59.2 makes this the only path
   that applies to the largest installed base.
2. Widen the live precondition context per driver. Every unresolvable path is a body the skill
   cannot reach, so this is directly measurable: how many bodies a fixed skill set admits on,
   before and after.
3. Capability token vocabulary and `extensions`. Pure contract, pure corpus.

`skynet skill check` stays the workstation gate it already is.

---

## Phase 5 — the class-gating open items

Each unlocks a robot class rather than a feature. All contract-shaped, all dry-verifiable.

| Item | Gates | §ers |
|---|---|---|
| Multiple robots on one compute | Cells — one Jetson, two arms | §43 |
| Safe-state discovery for mobile robots | AMRs at Host; §36.2's reserve budget depends on it | §36.2, §43 |
| `on_command_loss` per chain | Humanoids with mixed drivetrains | §53 |
| Hub-brokered console relay | A robot in a customer's building with nobody on site | §43 |

---

## What stays open, and must stay visible

Not caveats — surfaces that have to keep saying this until somebody stands next to a machine.

* **All 19 bench tests.** `missing`, never `skipped`, never absent.
* **OTA against real RAUC on a real A/B device.** The updater is a trait with a recording
  implementation, which is what makes rollback testable and is not the same as having watched one.
* **A signature on a conformance result.** §19 forbids a robot signing, so §12's kernel gate reads
  unsigned evidence. §80 says the Hub-countersigning shape is a *different* claim from what §15 asks
  for and must not be built as though it were the same. Undesigned.
* **Task success after retargeting.** Measure it via §56; never promise it.
* **`proximity` is inert on every robot.** Every declared input resolves and nothing fills
  `cycle.nearest_person_m`. Perception is a package, not the OS (§58.3), so this waits on somebody
  shipping the package — which means even a Host-tier robot under this plan does not enforce
  distance to a person.

**The standing rule.** A dry run may report `pass` only for a check that executed. Everything else
is a named absence. §15's own sentence is the one to hold the work to: a conformance runner that
reported `pass` for a test it did not perform would be the most damaging component this platform
could ship.

---

## Decisions, and who made them

The owner answered item 5 and delegated the rest on 2026-09-16. What follows is what each was
decided as and why, kept in place of the questions so the next reader sees the ruling rather than
re-opening it. Where a decision produced work, it is in *Next*.

1. **The §73 corpus figure edit stands.** §73's own paragraph says every figure in it is *measured,
   and checked*, and `scale-report --check` refuses with *"correct the document, not this check."* A
   count of what is in the tree is a derived fact, not a design statement, so bringing it up to date
   is maintenance. It was done in both copies and re-pinned with `--pin`, which refuses unless the
   two agree, so the mechanism that the rule exists to protect did its job. See 10 for the general
   case.

2. **`notifies_operator` gets a surface.** Its two siblings are serialised and rendered and this one
   is not, and the argument that `requires_safe_state` already covers it does not survive §36.1
   having written three rows rather than two. A robot that has stopped taking work and a robot that
   has stopped taking work *and owes somebody a phone call* are different situations. Wired, oracle
   first, then the corpus, then the editions.

3. **Both functions whose comments name absent callers are deleted.** `check_code` is a third
   spelling of a check the CLI and the daemon each already make directly; `tier_profile_to_json`
   renders a model for a surface nobody built. Deleting is the answer that leaves no claim behind:
   building a surface to justify a function is the tail wagging the dog, and a comment asserting a
   caller is the same defect as a figure asserting a measurement.

4. **`offline.control.independent` stays a `bench` test.** F2 asked whether it is `bench` because it
   needs *hardware* or because it needs *a machine rather than a model*, and the answer is the
   second — but that is an argument for letting a virtual robot **produce bench evidence** for it,
   not for reclassifying it as `sim`. Reclassifying would let a run against a pure fake claim it,
   which is the opposite of what the distinction is for. No specification change; the work is in
   *Next* instead, and it is the one item that would move the dry ceiling off `none`.

5. **ONNX Runtime is allowed, narrowly. — the owner's call.** The no-third-party-dependency rule
   protects a stated thing: a vendor building a driver years from now should not need a package
   registry to have survived. An inference backend is neither the kernel nor a driver, so the
   carve-out is written to that shape rather than as a general relaxation: the dependency lives in
   one crate, behind a feature that is off by default, the safety kernel stays `no_std` with no
   allocator and no dependency, and `ci.yml`'s check is narrowed to permit exactly that package and
   nothing else rather than switched off.

6. **Phase 3's specification fields wait for Phase 1.** Motion primitives are real design, not a
   count, and designing new transport surface ahead of a loop that has never closed is how a
   specification acquires fields nobody needs. Revisit when Phase 1's gate is met.

7. **"No evidence" and "evidence, and it failed" are rendered apart.** §80's `inert` precedent is
   the one on point: folding a third state into an existing one was the tempting simplification and
   the wrong answer, because the two send a person to do different things. The verdict already
   carries the distinction — `missing` non-empty with `failures` empty is one, `failures` non-empty
   is the other — so this is a rendering fix and not a wire change.

8. **§19's driver-activation rule gets one named spelling, and the accessor goes.** The enforcement
   point is the inline check in `runtime.rs`; `driver_may_activate` reads the same fact off a report
   that `main.rs` builds to *print* rather than to gate. Neither copy survives as it stands: the
   predicate moves onto `SignatureState` in the contracts crate, where it is one named fact, and the
   accessor that duplicated it is deleted.

9. **`skynet blackbox` reports replay completeness.** Same class as 2, and the plainest instance of
   it in the tree: the black box can already tell whether a sealed window recorded every field a
   replay evaluates, and presents a replay of a partial window exactly like a replay of a whole one.
   A replay is evidence, and evidence that will not say how complete it is has the shape §15 calls
   the most damaging thing this platform could ship.

10. **Figures in §73 are maintained, not negotiated.** The general rule behind 1: they are
    measurements of the tree, they go stale whenever the tree changes, and updating them in both
    copies and re-pinning is the maintenance that keeps the check meaningful. **Compressing code to
    avoid moving a figure is the thing not to do** — F6 records 0.1 doing exactly that, rewriting
    tests to save lines, and the tests were not improved by it. A design change to §73 is still an
    issue; the numbers in it are not.

## Next

Everything below is decided and unblocked. In order.

1. **The small ones, together** — *Decisions* 3, 7, 8 and 9. Delete `check_code` and
   `tier_profile_to_json`; name §19's activation rule once on `SignatureState` and drop the
   accessor; make `skynet blackbox` say whether the window it replayed was complete; render "no
   evidence" apart from "evidence, and it failed". Four small changes, one concern each, all inside
   the reference edition.

2. **`notifies_operator`** (*Decisions* 2). Corpus first, as Phase 3 argues and §47.6 proved: the
   TypeScript oracle, then the 28 power cases, then the four complete editions, then the CLI.

3. **Phase 1** (*Decisions* 5), of which the capture backend is **done** — `Source`, `Synthetic`
   and `pump` in `skynet-capture`, and F11 is what writing it cost. Three steps left, in order:
   1. **Wire `pump` into `skynet-kernel-host`'s `step_observing`.** The one allow-list entry with a
      date attached to its removal. It needs a dependency edge that crate does not have and a
      lifetime for the distributor and the source, which is why it is a step rather than a line.
   2. **The inference backend** (§23.3), now permitted. One crate, a feature off by default,
      `ci.yml`'s `Cargo.lock` check narrowed to that package rather than switched off, and the
      safety kernel untouched — the terms in *Decisions* 5, which are narrower than "yes".
   3. **The gate the plan already states**: `skynetk` offers the frame pool, `skynetp` maps it, a
      policy closes the loop against the MuJoCo driver for N cycles with no overruns, and the sealed
      window is read back and replayed to identical verdicts.

4. **Bench evidence from a virtual robot** (*Decisions* 4). The one item that would move the dry
   ceiling off `none`, and the only one here that changes what this plan is allowed to claim.
   `packaging/virtual-robot.sh` already produces the machine; what does not exist is a conformance
   run against it that is recorded as `bench` for `offline.control.independent` and for nothing else
   it has not earned.

5. **A check on doc comments that name a caller** (F9). Two of the twenty-three reasons had to be
   written against the function's own documentation. The production-caller check already knows which
   functions have no caller; refusing a doc comment on one of them that says *"for the CLI"* or
   *"called by"* is a small addition to a check that has the data in front of it.

Phases 2 and 5 are unchanged and unstarted. Phase 3 waits on Phase 1 (*Decisions* 6).

## Progress log

Newest last. Each entry: what was audited, what was found, what changed, what is verified — and
what is not.

### 2026-09-16 — Phase 0 opened, and closed

**Audited.** `CONTRIBUTING.md`, `BUILD-LOG.md`, `docs/README.md`, `docs/mirror-digests.json`,
`reference/typescript/mirror-digests.json`, §1, §5, §6, §7.1, §15, §54, §56.2, §58–§62, §66–§67, §80,
`.github/workflows/ci.yml`, `crates/skynet-contracts/src/certification.rs`,
`corpus/certification/award.json`.

**Found.** F1–F6 above, F1–F4 before any code was written. Two cancelled planned work — 0.4 was
already built twice, and most of 0.1's proposed machinery was already an invariant — and one made this
plan's central claim wrong in the direction that matters: the dry ceiling is `none`, not `guest`, and
a single row of §15's table is why.

**Changed.**

| File | What |
|---|---|
| `crates/skynet-contracts/src/certification.rs` | +71. Two tests pinning the dry ceiling and the suite split it is computed from. No behaviour change, so the corpus is untouched |
| `tools/check-production-callers.mjs` | New. 0.2's check |
| `tools/production-callers-allow.json` | New. Its baseline — 38 names, 12 answered, 26 debt |
| `.github/workflows/ci.yml` | One step in the `invariants` job |
| `reference/reports/scale.json` | Regenerated, and **required**: `scale-report.mjs --check` holds the pinned file to what a run produces, so 71 lines of Rust have to move the pin. Reverting it fails the check — confirmed by reverting it |
| `DRY-COVERAGE-PLAN.md` | This file |

**Verified.** The full pre-PR gate from `CONTRIBUTING.md`, on the final state:

```
cargo fmt --all --check                                      exit 0
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets exit 0, 0 warnings
cargo test --workspace                          1,769 passed, 0 failed
node tools/check-mirrors.mjs                    the specification matches its pin
node tools/check-contract-mirrors.mjs           24 mirrored contracts match
node tools/check-golden-path.mjs                exit 0
node tools/check-rhal-flags.mjs                 exit 0
node tools/scale-report.mjs --check             exit 0  (§73 matches the tree)
node tools/check-production-callers.mjs         940 checked, 38 allow-listed
node --check console.js / fixture-serve.mjs     exit 0
```

Both new checks were held to a **positive control**, per `CONTRIBUTING.md`'s rule that a check which
has never failed has proved nothing:

* Reclassifying `offline.control.independent` from `bench` to `sim` fails both 0.1 tests. Reverted.
* 0.2 fails on each of its three refusals independently — a new unreached `pub fn`, an allow-listed
  name that has gained a caller, and an entry with an empty reason. All three reverted.

Writing 0.2 also found a bug in 0.2: `#[cfg(test)] mod tests;` puts a whole *file* behind the
attribute and the first version scanned each file alone, so `skynetd-io/src/tests.rs` — nine thousand
lines of tests — was read as production surface. Fixing it *raised* the finding count from 35 to 38,
because five functions reached only from that file had been counted as reached.

**Not verified, and not claimed.** No robot, virtual or physical, was brought up: `virtual-robot.sh`
needs Linux with systemd as PID 1 and this is Windows. So `skynetd-io`'s end-to-end systemd run and
the `#[ignore]`d driver-loading suites did not execute here — CI runs both. The three tests added
prove an arithmetic property of the award function, not that any robot reports it.

### 2026-09-16 (second session) — Next 1 and 2 done, on a machine that could run all of it

Carried out on Linux with every edition toolchain present, which is the difference between this
entry and the one above. Branch `claude/magical-darwin-igdkl1`, rebased onto `main` at `682d9b0`.
**Nothing pushed**, in either repository.

**Audited.** CI runs 55–60 on `main` and the logs of all three failing jobs; `CONTRIBUTING.md`'s
gate against what CI actually runs; `certification.rs` and the TypeScript oracle's `conformance.ts`;
`corpus/certification/award.json`; `tools/corpus-gen/generate.ts`; `scale-report.mjs` and
`edition-report.mjs` and what each of them pins; `docs/README.md`'s mirror procedure and both copies
of the specification; `editions/go/cmd/selfcheck` and `internal/fusionprobe`; `strip()` in
`check-production-callers.mjs`; and the declaration and call sites of all 23 untriaged names.

**Found.** F7–F10. In order of how much they change: `main` was **red the whole time** and the gate
could not have said so (F7); 0.2's check reported the wrong line 18 times in 38 and was blind to a
real caller it had already denied in writing (F9); §36.1's operator notification is built on both
sides and joined by nothing (F10); and F3a's two blockers were properties of a laptop rather than of
the work, while the real one is a single exact figure in the pinned specification (F8).

**Changed.** Six commits here, one in the Hub.

| File | What |
|---|---|
| `crates/skynet-ring-shm/src/pass.rs` | `[b'R']` → `*b"R"`. clippy 1.98's `byte_char_slices`, the first of three reasons `main` was red |
| `tools/unicode-case.ts` | `rows()` takes `readonly string[]`; it reads `.length` and `.slice` and nothing else. The second |
| `editions/go/internal/fusionprobe/probe.go` | A second control, `Intrinsic`, calling `math.FMA`. The third |
| `editions/go/cmd/selfcheck/main.go` | `fusingTargets` re-measured and its comment corrected; the finding message names both remedies, because the listing cannot say which route produced the instruction |
| `tools/corpus-gen/generate.ts` | +3 cases. `sim-only-awards-none`, `sim-only-plus-the-one-bench-test`, `sim-only-claiming-host-still-reads-guest` |
| `corpus/certification/award.json` | Regenerated. 20 cases → 23, purely additive, no existing case perturbed |
| `ci.yml`, `editions/{cpp,go}/Makefile` | The three complete-edition ratchets, 1199 → 1202. The four partial editions' floors are unchanged, which is what they are separate for |
| `reference/reports/{editions,scale}.json` | Regenerated. Rust is still 136,498 lines, so F6's two-line headroom is untouched — nothing here added Rust |
| `docs/skynet-os-architecture.md` | **The pinned specification.** One figure, in both repositories, re-pinned with `--pin`. Ratified as *Decisions* 1 |
| `docs/status.md` | 1,199 → 1,202, twice |
| `tools/check-production-callers.mjs` | Removed spans keep their newlines; character literals are consumed as units |
| `tools/production-callers-allow.json` | 38 entries → 37. `upload_target` removed because it was never unreached; the 23 baseline entries replaced with arguments |

**Verified.** The whole gate, on the final state, at CI's own toolchain — stable moved 1.94 → 1.98
here first, which is what made F7's first row reproducible rather than theoretical:

```
cargo fmt --all --check                          exit 0
RUSTFLAGS="-D warnings" cargo clippy             exit 0, 0 warnings
cargo test --workspace                           1,857 passed, 0 failed
npx tsc -p .                                     exit 0
node tools/check-mirrors.mjs                     the specification matches its pin
node tools/check-contract-mirrors.mjs            every mirrored contract matches
node tools/check-golden-path.mjs                 exit 0
node tools/check-rhal-flags.mjs                  exit 0
node tools/check-production-callers.mjs          942 checked, 37 allow-listed
node tools/scale-report.mjs --check              exit 0  (§73 matches the tree)
node tools/edition-report.mjs --check            4 editions, 1202 cases each
node tools/corpus-gen/generate.ts                regenerates the committed corpus byte for byte
```

And **all eight editions**, which is the thing the machine above could not do:

```
rust    1202/1202      python  1202/1202      cpp   1202/1202      go     1202/1202
c        354 passed    zig      315 passed    java   315 passed    csharp  315 passed
```

Every check that changed was held to a positive control, per `CONTRIBUTING.md`:

* **clippy** — reproduced at 1.98.1 before the fix, clean after.
* **tsc** — TS4104 reproduced before, clean after.
* **the fusion scan** — a planted `math.FMA` under `internal/` is refused on all six targets; a
  planted `x*y + z` on five and **not on amd64**, which is the same measurement arriving from the
  other direction. Both reverted.
* **the corpus cases** — the Python edition mutated to treat an absent `offline.control.independent`
  as satisfied, which is the defect these cases exist for, fails `sim-only-awards-none` at byte 12
  — where `none` and `guest` differ — while `sim-only-plus-the-one-bench-test` keeps passing because
  its bench result is present. The pair isolating the row rather than the suite. Reverted.
* **the line-number fix** — 18 of 38 reported lines did not open their own declaration before, 0
  after, with the same 38 findings both times.
* **the character-literal fix** — the ratchet itself was the control: with the scan able to see,
  `upload_target` gained a production caller and the check failed until the name came off the list.

**Not verified, and not claimed.** No robot, virtual or physical. `packaging/virtual-robot.sh` needs
systemd as PID 1 and PID 1 here is a container supervisor, so §33.1's units never started, the
`#[ignore]`d driver-loading suites did not run, and `skynetd-io`'s end-to-end systemd run did not
either — CI runs all three and they are green there. The three corpus cases prove that four
independent implementations compute the same award from the same evidence; they say nothing about
any robot reporting it, and F1's ceiling is still arithmetic rather than a measurement of a machine.

**Still true, and worth repeating because this entry is mostly good news.** All 19 bench tests are
`missing`. `proximity` is inert on every robot. Nothing has been signed by a Hub. The dry ceiling is
`none`, and three more editions now agree about it — which is a stronger statement that the number
is right and not a weaker one that it is high enough.

### 2026-09-16 (third session) — the decisions taken, and Phase 1's capture half

The owner answered *Open for the owner* 5 — **ONNX allowed, narrowly** — and delegated the other
nine. That section is now *Decisions, and who made them*. Six more commits here, one in the Hub,
**nothing pushed**.

**Changed.**

| Decision | What happened |
|---|---|
| 3 | `check_code` deleted. **`tier_profile_to_json` kept** — see below |
| 7 | The `no tier` headline says which of the two causes it is, extracted so the four shapes are testable |
| 8 | §19's activation predicate moved onto `SignatureState::admits`; the report accessor stays for the `NotRun` arm it alone carries |
| 9 | `skynet blackbox` reports whether the window it replayed was complete |
| 2 | `notifies_operator` reaches the power document and then a surface — oracle, 28 corpus cases, four editions, CLI |
| 5 | Phase 1's **capture half** built. The inference half is unstarted |
| 1, 10 | §73's Rust figure moved 136,000 → 137,000 in both copies, as maintenance rather than as a negotiation |

**Two decisions were wrong when made, and reading the code is what corrected them.**

*Decision 3 said delete both.* `tier_profile_to_json` is **the only place in the workspace that
renders `SAFETY_CLAIM_RULE` into an output** — the kernel declares the sentence, CI checks its text
is intact, and nothing else emits it. Deleting it would have left CONTRIBUTING.md's rule with
nothing implementing it and no check able to notice, which is a safety claim quietly weakened by a
tidy-up. Its comment named a surface that does not exist; the comment was the defect.

*Decision 8 said fold two copies into one.* They are not two copies. `driver_may_activate` also
refuses a report that never ran, which `SignatureState` cannot express and which is not the same
fact. The **predicate** was the duplicate and is now written once; the structural duplication —
`runtime.rs` deciding activation without ever building a `Report` — is still there and is a larger
refactor than §78's remedy asked for.

### F11 — the first capture backend found the release rule wrong, in the worse direction.

`Delivered` told a caller to release `displaced` **and** `not_recorded`, and said that was
*"everything this call took ownership of and nothing it still holds"*. It is wrong about the second.
`LatestSlot::publish` always accepts, so `not_recorded` is the frame that was **just published** —
handed back only to report that the recorder refused it, while the slot holds that very buffer as
the call returns. A backend following the documentation hands a live buffer back to the device.

The two failure directions are not symmetric and this is the bad one. A pool that runs dry stops
capture and looks like a driver fault, which somebody investigates; a buffer freed while a consumer
reads it is a policy acting on a frame something else is writing, on a zero-copy path with nothing
to catch it, three hours in.

The fact underneath is that §47.4 gives one frame to two consumers, so a buffer has up to two
holders and **no lend-and-return scheme can say when it is free**. A reference count answers it,
which is what `dmabuf` does with an fd refcount and what V4L2 with a GPU and an encoder on one
buffer does underneath. `Source::retain` now says so in the trait.

Worth recording as a coverage finding rather than only as a bug: this crate had seventeen tests, a
`no_std` discipline, and a doc comment for every public item — and the defect was in **what the
documentation told the first caller to do**, which nothing that existed could have caught, because
there was no caller. It is the exact argument 0.2's check makes, arriving one level up.

**Verified.** The full gate at CI's toolchain, and all eight editions on the corpus:

```
cargo fmt --all --check / clippy -D warnings / cargo test    1,866 passed, 0 failed
npx tsc -p .                                                 exit 0
check-mirrors / check-contract-mirrors / golden-path         exit 0
check-rhal-flags / check-production-callers                  944 checked, 33 allow-listed
scale-report --check                                         §73 matches the tree
rust python cpp go   1202/1202      c 354   zig 315   java 315   csharp 315
Cargo.lock                                                   24 packages, all workspace members
```

Controls, each failing the suite and each reverted: `Untrusted` made to admit a driver; releasing
the refused frame; forgetting the recorder's retain; leaking the displaced frame. The corpus's
power cases were re-derived from the oracle rather than edited, and the four complete editions were
each brought to them separately.

**Not verified, and not claimed.** Still no robot, virtual or physical — PID 1 here is a container
supervisor, so `virtual-robot.sh` did not run and neither did the `#[ignore]`d driver-loading
suites. The capture backend has never seen a camera and does not claim to: what it demonstrates is
the §47.4 discipline, which is a claim about frames and not about pixels. **No policy has closed a
loop**, so Phase 1's gate is not met and nothing here moves the dry ceiling off `none`.


### 2026-09-16 (fourth session) — `invariants` was still red, and Phase 1 resumed

Carried out on Windows 11 with WSL2 Ubuntu 26.04 beside it — **systemd is PID 1 in that WSL**,
which is the first machine in this log that has it. Rust 1.98.1 on Windows (the CI toolchain), 1.93.1
in WSL, Node 26, Go, .NET, JDK 25; no Zig and no Node 24. **Nothing pushed.** *In progress — this entry
is updated as the session goes.*

**Audited.** Every step of `ci.yml`'s `invariants` job, run locally one by one — the shell steps under
WSL's GNU tools, the Node steps on Windows — rather than the pre-PR gate, because F7's lesson is that
the gate is not what CI runs. Then the `rust` job's Linux half in WSL, clippy at 1.98 against
`aarch64-unknown-linux-gnu` from Windows (which type-checks every `cfg(target_os = "linux")` path at
CI's lint level), and the kernel's `thumbv7em-none-eabihf` build.

### F12 — F7 fixed the first red step in `invariants`, and four more were behind it.

F7 recorded `main` red on three counts and all three fixed. The `invariants` job's first failure was
the oracle type-check, and **a GitHub job stops at its first failing step** — so everything behind it
had been unobserved for as long as anything in front of it was red. Run step by step, four more failed
on the commit F7 called green:

| Step | Failing since | What it was |
|---|---|---|
| No string literal has a hole punched in it | 2026-09-04 | **Real.** 38 literals in 16 files, two of them the Safety Kernel's own `can_start` refusals |
| No secret reaches a command line | 2026-09-02 | **Real.** `bundle` and `upload` each spawned their own curl with their own copy of the credential rule |
| A check-in carries no history | 2026-09-03 | **Stale check.** `delta_` matched `max_delta_per_cycle`, a jerk *ceiling* |
| An intervention count is absent rather than zero | 2026-09-03 | **Stale check.** It required a field aed1306 deliberately removed, and its zero test named a field the episode no longer writes, so that half could not fail at all |

The mechanism is the finding, more than the four: a job that stops at its first red step reports one
defect per CI run, and "fixed" becomes "revealed the next one". Every `run:` step in `invariants` and
`corpus` now carries `if: ${{ !cancelled() }}` — each is an independent claim, so each runs whatever
the one before it did, and the job still fails if any did.

Nothing here could see CI itself: the repository is private and `gh` is not installed on this
machine. "Failing since" is the date the offending line entered the tree against the date the check
did, not an observed run.

### F13 — an episode's intervention count forgot a hole as soon as the next pass was counted.

Found while rewriting F12's fourth check. A pass nothing could count set the invocation's count to
`None`, and the next counted pass revived it with `get_or_insert(0)` — so an invocation with an
unwatched interval in the middle reported the passes after it as its whole. The existing test put its
hole on the **last** pass, which is the one ordering the defect cannot reach. §74.3's rule is that
zero and unmeasured are opposite facts; a partial sum is worse than either, because it is a number
quietly smaller than the truth in the field that means the truth.

The same line cast the ring's `u64` count to the episode's `u32` with `as`, so 2³² interventions
arrived as **zero** — the one value a marketplace reads as a perfect run — and two large counts would
have panicked the daemon under the release profile's overflow checks.

**Changed so far.**

| Commit | What |
|---|---|
| `c04f57a` | 38 string literals given their `\` continuations back. No corpus bytes move; none is pinned by an edition |
| `7635741` | `connectivity::curl` is the only spawner; `presentable` is the credential rule, asked before any disk work and again inside `curl`. One test reaches the refusal from all three modules |
| `e043eb6` | The two stale checks measure the code again; `!cancelled()` on every `invariants` and `corpus` step; `episode()`'s doc loses the sentence aed1306 said it had retracted |
| `8b59c0b` | F13. `InterventionCount` — no pass yet, counted, unmeasured — with unmeasured terminal; saturating on both casts |

**Verified so far.** On Windows at 1.98.1: `cargo fmt --check`, clippy `-D warnings` for the host
**and** for `aarch64-unknown-linux-gnu`, `cargo test --workspace` (1,778 before any change here —
Windows compiles fewer tests than Linux), the kernel, ring and capture for `thumbv7em-none-eabihf`,
`tsc -p .`, and all eight Node checks. In WSL at 1.93.1: `cargo test --workspace` 1,868 passed and 0
failed, clippy clean, and every shell step of `invariants` green.

Controls, each failing its check or test and each reverted: dropping the fetch's early credential
refusal; dropping the push's; a `delta_ms` field on the guard report; a literal zero intervention
count; an `unwrap_or(0)` on it; and the previous `skill.rs` against both F13 tests.
