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
making quietly. Raised, not resolved. See *Open for the owner* below.

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
| Inference backend (§23.3) | ONNX Runtime, CPU | Runs in CI, deterministic. **Would be this workspace's first third-party dependency — see *Open for the owner*** |

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
an issue, not a branch. See *Open for the owner*.

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

## Open for the owner

Four decisions this plan deliberately does not make.

1. **Is `offline.control.independent` witnessable on a virtual robot?** (F1, F2.) It is the single
   test between dry evidence and a guest award, it needs a machine rather than a body, and
   `virtual-robot.sh` is a machine. A specification change either way, so: an issue carried to both
   repositories, never a patch here.
2. **ONNX Runtime as the inference backend breaks "no third-party Rust dependencies."** That rule is
   CI-enforced and its stated reason is that a vendor building a driver years from now should not
   need a package registry to have survived. An inference backend is not the kernel and not a
   driver, so the rule may not have been written with it in mind — but it is written, and Phase 1
   cannot start until this is settled.
3. **Phase 3 needs a descriptor field and new transport methods**, both in the pinned specification.
4. **How a dry-only award is rendered.** `none` is correct and is also what a robot with a broken
   driver gets. Whether an operator should be able to tell "no evidence yet" from "evidence, and it
   failed" is a surface question with the same shape as §80's `inert` decision — where folding a
   third state into an existing one was the tempting simplification and the wrong answer, because
   the two send a person to do different things.
5. **Which copy of §19's driver-activation rule survives** (F5) — `driver_may_activate`, or the
   inline check in `runtime.rs:519`.
6. **Whether `skynet blackbox` should report replay completeness** (F5) — `verdict_inputs_all_recorded`
   exists and is called by nothing.
7. **§73's line count** (F6). Two lines of headroom. The next addition needs the figure moved, which
   is a specification edit.

---

## Next

In order, and the first is the one that most needs a machine this one is not.

1. **The sim-only corpus case (F3a)** — holds all eight editions to F1 rather than only Rust. Needs
   Zig installed and Node 24 for the corpus job.
2. **Drain the 0.2 baseline.** Thirty-eight names, of which twelve are answered — the signer behind
   `test-signing`, `skynet-capture`'s producer side waiting on a capture backend, and the vendor ABI
   view — and twenty-six are untriaged debt in `tools/production-callers-allow.json`.
3. **Phase 1**, once the ONNX question in *Open for the owner* is settled.

---

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
