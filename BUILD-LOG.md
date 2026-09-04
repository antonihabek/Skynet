# Build log — 2026-09-04 onward

A running record of this build session, kept beside the two canonical logs rather than instead of
them: [`PROGRESS.md`](PROGRESS.md) is the OS repository's session log and the *Progress log* in
[`docs/skynet-build-plan.md`](docs/skynet-build-plan.md) §33 is the Hub's. This file is the working
record — what was audited, what was found, what was changed, and what is verified — and its findings
are folded into those two when a round closes.

**Nothing in this session is pushed.** Commits are held on `main` locally until asked for.

---

## Audit — where the plan actually stood

Both repositories clean and level with `origin/main`. The plan is **not** finished; nearly everything
that can be finished without hardware, a GPU or an owner's decision is.

| | Verified today |
|---|---|
| Skynet OS | `cargo test --workspace` — **1,362 passed**, 4 ignored, exit 0, on Windows |
| Skynet Hub | 96 routes under `/api/skynet/v1`, 22 tracked migrations, 8 verifier scripts |

### Open, and blocked on the owner

| Item | What is actually blocked |
|---|---|
| Account erasure | The last unbuilt half of Phase 14's privacy line. Crosses namespaces, org membership, audit rows that must survive a deletion, and evaluations one person filed about another's work |
| Who may decide an abuse report | The states are already in the CHECK constraint; nothing says who may move them |
| Retention of sealed windows (§80) | They are charged for; deleting one the Hub has acknowledged is a decision about evidence |
| Collections | A product decision, deferred seven times |

### Open, and blocked on infrastructure that does not exist

| Item | Evidence |
|---|---|
| Phase 10 — a real worker | `lib/skynet/workers/in-process.ts` runs dataset-index and search-reindex only. Simulation, training and deployment queue and stay queued |
| Phase 11 — Spaces, GPU training, inference endpoints | The spec, the config and the job row exist; nothing executes one |
| Phase 12 — teleoperation | No module, no table, no route. Episodes and intervention counts exist; the operator session does not |

### Open, and blocked on hardware

The policy runtime's consumers (§25), real drivers (`ros2_control`, EtherCAT, CANopen, vendor SDKs),
capture backends (V4L2, GenICam), the safety MCU, TPM binding, a device key, §36.4's brownout input,
and OTA against real RAUC. §59.5 says this plainly and this log does not improve on it.

### Open, and buildable now — this session's queue

1. ~~README drift in the OS repository~~ — **done below**
2. ~~The Hub's base schema~~ — the two generated records were missing and are restored; a baseline migration was started and dropped as scaffolding
3. Phase 9: protected release channels, review approvals, required checks, source-to-release traceability
4. Phase 9: Git interoperability or an external-Git mirror
5. Phase 9: dataset completion — split/shard manifest, range access, quality reports, licence lineage
6. §31.3's second language edition, which is what the 1,055-case corpus exists for
9. ~~§25's control loop — the process eight rows of §73 were waiting on~~ — **done below**
7. `/skynet/os/editions` — §32.4 item 2, the only item of that list unbuilt
8. Six dead catalogue keys orphaned across 34 locale files

---

## 1. The README described a robot three features behind the one in the repository

`README.md` is the first thing a vendor reads and it carried four claims that the code had made
false. None of them was a small wording drift; each named a feature as absent that is built, tested
and in the table three paragraphs above it.

| It said | The code |
|---|---|
| *"What is **not** built is the fetch — §34's inbound half"* | `crates/skynetd-io/src/bundle.rs` fetches, resumes and hashes. `stage_image` has not reported `Unimplemented` for want of bytes since §75 |
| *"The **bundle** is still not fetched"* | Same module, said twice |
| *"The one class §34.4 names that still has nowhere to go is **telemetry**"* | `connectivity.rs:963` routes it to `/devices/{id}/telemetry`; §74's window is built every pass and all eight counters have a source |
| *"The half that is **not** here is the Hub's. Nothing serves that route yet"* | `PUT /devices/{id}/recordings/{sha256}` is served and has 27 live checks against the real bucket |

The fourth is the one worth being careful about, because the replacement is not *"it works now"*.
The Hub's half exists and the robot's half exists and **the two have never met**: each is exercised
against a double. That is a different sentence from either the one that was there or the one a
green tick would have written, and it is the one in the README now.

Two rows of the crate table gained what they were missing — §75's fetch under `skynetd-io`'s OTA
half, and §74's telemetry window beside §17's check-in.

**Verified:** `cargo test --workspace` — 1,362 passed, exit 0.

---

## 2. The Hub's test suite was red, and the log said it was green

Two files the build log's twenty-first session (fourth) describes as written — the ones its title
calls *"the repository now contains its own schema"* — were **not in the tree** and not in any
commit. The two generators were committed; their output was not.

Two tests read them at module scope, so the suite could not have been passing:

```
FAIL  lib/__tests__/skynet-export-schema.test.ts
Error: ENOENT: no such file or directory, open '…\supabase\skynet-schema.json'
Test Files  1 failed | 69 passed (70)
      Tests  1665 passed (1665)
```

Regenerated with the repository's own scripts — both read-only against the live database, one over
PostgREST's OpenAPI document and one through `pg_dump --schema-only`:

* `supabase/skynet-schema.json` — 59 tables, 823 columns
* `supabase/schema/skynet.sql` — 58 tables, 88 indexes, 63 policies, 15 triggers, 16 functions

`npx vitest run` — **1,691 passed, 70 files**, which is the number the log had been claiming. Both
generators' `--check` modes confirm the files are current, so the round trip is stable rather than
merely written once.

**A baseline migration was started here and abandoned** on the owner's instruction, as scaffolding
rather than product. The gap it was aimed at is real and stays open: `supabase/migrations` holds 22
increments on a base schema that no file in the tree creates.

---

## 3. §25's command path had two ends and one of them

The item: §73's last column has eight rows saying some version of *nothing has run a cycle*, and all
eight are downstream of one absent process. `skynet-kernel` is the guards, `skynet-ring` the channel,
`skynet-rhal` the wire, `skynet-blackbox` the record — and nothing in the workspace opened a driver,
read the ring, asked the kernel and wrote a joint.

Built: **`crates/skynet-kernel-host`** and the `skynetk` binary, plus **`crates/skynet-dl`**, which is
the old `skynet-cli/src/dl.rs` extracted so the loader `unsafe` has one home and two callers.

The full argument is in [`PROGRESS.md`](PROGRESS.md)'s fifty-fourth session. The three decisions
worth repeating anywhere:

| | |
|---|---|
| **A hold is a mask of zeros** | §24 defines a zero mask entry as *hold this joint*. Zeros with a mask of ones slams a position-controlled arm to its origin and makes a torque-controlled one go limp |
| **The deceleration bound is the acceleration bound** | No deceleration figure exists anywhere, and a controlled stop needs a rate. `max_delta_per_cycle` is the jerk ceiling the rate limiter enforces going up; ramping down at it is the declared number read the other way, not a new one |
| **An escalation is recorded as one** | With no jerk ceiling there is no rate, so a Cat 2 becomes a Cat 0. That is the wrong trade for a robot holding a load and doing it silently is worse — the record carries demanded, performed, and the missing input |

### Verified

* `cargo test --workspace` — **1,377 passed** (1,362 before), `fmt --check` and
  `clippy --workspace --all-targets` clean
* `skynetk run --driver drivers/mock/mock_robot.dll --tier host --velocity-max 2.0 --jerk 0.1
  --watchdog-ms 50 --cycles 40 --hz 200` — Enforcing, four guards active, four naming what they
  lack, **zero overruns**
* `crates/skynet-kernel-host/tests/loop.rs` — three `#[ignore]`d cases that compile the mock robot
  and drive `skynetk` against it, wired into the `abi` CI job, so the 200 Hz run repeats on every
  push rather than having happened once. Two of them failed first time on `LoadLibraryW` error 32:
  three parallel tests were compiling the mock to one path
* corpus 77 files / 1,055 cases, golden path 48 commands, mirrors re-pinned after the §73 edit

### One bug it found in its own first run

`--jerk` was parsed, stored and wired nowhere. No test failed. What said so was §7.2's own output on
the first real run: `unavailable RateLimit — needs profile.max_delta_per_cycle`, printed on a run
where `--jerk 0.1` had just been passed. The jerk ceiling is the operator's and had been put on the
transport clamp, which has no such field.

### What it does not do

It is the ring's **consumer**. The producer — inference, observation assembly from real sensors, the
sandbox holding them — is unwritten, so the ring is empty every cycle and the kernel holds every
cycle. That is §33.1's own property rather than a defect, and it is now watchable instead of only
arguable. It also does not make a robot host-tier: `Tier::enforcement` still derives enforcement,
and `overruns` is the only number this loop can honestly contribute to that question.

---

## Round state

Nothing pushed. Both repositories have uncommitted work on `main`.

**Next:** the OS side's remaining buildable items are §31.3's second language edition and §80's
`people_detection` vocabulary gap. The Hub's are Phase 9's release gates, `/skynet/os/editions`, and
the six dead catalogue keys.

---

## 4. A skip reason that named a missing path, and the bug that found

`package.grants.gate` — one of §15's eight runtime tests — had been skipped for eleven sessions
because the mode gate was *"the Safety Kernel's, through a path §33.1 denies this daemon."* Item 3
built that path, so the reason had expired and nothing would have noticed: a skipped test does not
fail.

**Now measured**, against a robot's own grants: one command exercising a granted capability is
admitted, one exercising an ungranted capability is refused and sends no channel, one naming no
capability is treated as neither — §7.2's distinction between *absent* and *denied*. A robot with no
grants is skipped rather than half-measured, because a check that only ever refuses would pass on a
kernel whose gate said no unconditionally.

**And on every robot that exists it still skips**, for a reason that is now a live property instead
of a missing component: `ROBOT_TIER` is guest, guest is advisory, and §7.4 forwards the command
whatever the guards said. A tier that cannot refuse has not passed a test about refusing, and
reporting one would turn guest observation into a safety claim.

### The bug

Writing that expectation failed against the kernel, and the kernel was right. Item 3's `decide`
zeroed the mask on **every** hold, advisory ones included — so a guest-tier robot would have withheld
a command the kernel said to forward, while the record beside it said `advisory: true`. Written
yesterday; no test in that crate could have caught it, because every fixture there resolves at host
tier, which is where the interesting decisions are.

Fixed, with two regression tests, and the module header now argues the rule rather than assuming it.

### Verified

`cargo test --workspace` — **1,385 passed**. fmt and clippy clean, golden path 48 commands, mirrors
re-pinned after a second §73 edit.

---

## 5. §13 gets cycles, and the sequence that lives in memory

The loop built the records and dropped them. `skynetk --blackbox <dir>` now writes each one into
§26's ring and seals the window on exit — **the first recording of control cycles this repository has
produced**: thirty cycles against the mock robot at 200 Hz, sealed into a 48 KB MCAP.

**The question that could not arise until there were two writers.** `skynetd` records events and gaps;
the host records cycles; both belong in one recording — and a `Ring` holds its next sequence *in
memory*, so two over one file encode two records into one slot and produce a recording that passes
every check a reader applies and is wrong in the interval that matters. §26.2 now states the rule —
one ring, one writer, an exclusive lock, a refusal that names the holder — records three rejected
alternatives, and puts the real fix (the sequence as an atomic in a mapped header) in §80.

**What the format refused, and the fix that was not the obvious one.** The first run wrote nothing:
`Shape { field: "desired", expected: 3, found: 0 }`. Padding with zero would have satisfied it and
been wrong — a recorded `desired` of 0.0 says the policy asked for the zero position, and on an idle
cycle nothing asked for anything. The fill is `NaN`, per §74.3's rule that zero and unmeasured are
different facts.

### Verified

`cargo test --workspace` — **1,388 passed**; `--ignored` 3 passed; fmt and clippy clean; mirrors
re-pinned after §26.2 and §80.

---

## 6. Phase 9 asks for seven workflows to have a client. One had none.

Phase 9's last bullet is *"CLI, SDK, and API contract tests covering repository, artifact, release,
pull-request, evaluation, device, and deployment workflows"*. The contract test added in the
twenty-first session proves every path either client calls **exists**. It says nothing about whether
a workflow has a client at all, which is the half the sentence is actually about.

Reading it mechanically found one gap, and only one: **device**.

`GET /devices/fleet` and `GET /devices/{id}` have been served, RLS-checked and rendered on the web
since the ninth session, and **no CLI command and no SDK class addressed either.** It is an easy gap
to explain — a robot pairs itself, and §10 keeps the device credential on the machine — and it was
still a gap, because nothing about that stops a fleet operator wanting to see what their robots are
running.

### Built

| | |
|---|---|
| `skynet fleet [--namespace ns]` | Every robot in a namespace and what it last said. A robot that has never reported is **listed**, because that row is usually the one an operator is looking for |
| `skynet device <id>` | One robot: told to run, says it is running, and the gap. §10's three digests, with an absent one rendered as *unmeasured* rather than as a blank |
| `skynet.Device`, `skynet.fleet()` | The same two reads from Python |

Deliberately no third command. Neither client can write a device's state, and a test asserts it: a
client that could post a check-in would turn every field on a fleet page from something a machine
measured into something anybody could assert.

### Verified

* `lib/__tests__/skynet-client-contract.test.ts` — nine new cases, one per workflow plus two for the
  asymmetry that would be easiest to reintroduce. A workflow with no client is now a **failure**
  rather than a sentence in a plan
* `scripts/skynet-verify-fleet.mjs` — **18 live checks** against a running server and the production
  database: the fleet answers, a member sees their own and not somebody else's, an unreported robot
  is listed with a blank state rather than omitted, neither route is public, and the fixtures are
  removed in a `finally`
* The last four of those **run the binary**, with the fixture token in `SKYNET_TOKEN`. Driving the
  routes proves the routes; a command that resolves to a live route and prints `undefined` is
  exactly what a path-matching contract test cannot see
* `npx vitest run` — **1,700 passed**, 70 files. `npm run typecheck` clean

---

## 7. The counters report, and the one figure the reader refuses to print

`skynetk blackbox <dir>` reads a ring back and prints §74's five counters off it. Two hundred cycles
against the mock robot: `cycles 200, interventions 0, guard trips 0, clamps 0, estops 0` — five
counters that have reported *unmeasured* for the life of this repository, reporting.

**The first version also printed `field conformance 1.0000`, and that line was the problem.** Nothing
commanded that robot: §25's ring was empty every cycle, so the kernel held every cycle, so it
intervened in none, and §15's ratio comes out perfect by arithmetic. On any page that rendered it,
that reads as a robot which did everything right. It did nothing at all.

The reader now counts cycles that carried a command — `command_seq` is in every record — and over a
window where none did, says so and prints no figure. A test covers the other branch: twenty cycles
commanding ten times the channel ceiling give 20 interventions and a conformance of **zero**, which
is §15's figure with a numerator and a denominator that both came from a machine.

### Verified

`cargo test --workspace` — **1,389 passed**, fmt and clippy clean, golden path 48 commands, mirrors
re-pinned.

---

## 8. §26.2 was stated and half-enforced

The rule — one ring, one writer — was written in item 5 and applied only to the writer that was being
written. `skynet-kernel-host` had a lock; `skynetd`'s recorder, which has been writing the ring for
fifty-three sessions, had none.

`crates/skynet-lock` is the single implementation, with the same justification `skynet-dl` has: two
callers, and neither may depend on the other. The two refuse differently and that is the design —
`skynetd` starts **without a recorder** and says so (§33.1: a supervisor that cannot record is not a
robot that cannot stop), and `skynetk` refuses to run, because a control loop whose cycles go nowhere
is the one case where the recording is the point.

A stale lock is named rather than stolen, and the message says it is safe to delete — otherwise it is
a robot nobody can restart.

**Verified:** `cargo test --workspace` — **1,392 passed**, fmt and clippy clean, golden path 48
commands.

---

## 9. The baseline migration is withdrawn from the plan

At the owner's direction, and the plan now records it as withdrawn rather than open.

It had been added to *Still not built* by the twenty-first session (fourth) as *"a baseline for the
marketplace schema and the Skynet schema in dependency order"*. It was attempted early in this
session — `pg_dump` of `public` and `skynet_private` behind a generated prerequisites file, applied
to a throwaway Postgres container and dumped back for a byte-for-byte round trip — and dropped before
it landed, on the grounds that it is scaffolding rather than product: a file that exists so that a
database nobody is standing up could be.

**The judgement holds, and the distinction it draws is worth keeping.** What the tree needed was not
a way to *build* the schema but a record of the schema it *has*, so code can be held to it — which is
what `supabase/skynet-schema.json` and `supabase/schema/skynet.sql` are, and what the `follows`
defect happened for want of. That those two were described in the log and present in no commit is the
finding that mattered.

A fresh environment still cannot be built from `supabase/` alone. That is a **stated property** of
the repository now rather than an open task in it.

Nothing baseline-related remains in either tree: no `supabase/baseline/`, no `skynet-baseline-*.mjs`.

---

## Round state — stopping here

Nothing pushed. Both repositories have their work committed locally on `main`.

| | Start of session | Now |
|---|---|---|
| Skynet OS | 1,362 tests | **1,392**, plus 3 driver tests the `abi` job runs |
| Skynet Hub | 1,665 — **suite red** | **1,700**, typecheck clean |
| Live verifier scripts | 8 | **9**, 18 new live checks |
| Crates | 15 | **18** — `skynet-dl`, `skynet-kernel-host`, `skynet-lock` |

### What was built

1. **The control loop (§25)** — the process §3's stack diagram draws and the repository did not
   have. Opens a real RHAL driver, turns a cycle at a declared rate, reads the ring, asks the eight
   guards, writes the verdict, records the cycle. 200 Hz against the mock robot, zero overruns.
2. **§13's ring, with cycles in it** — the first recording of control cycles this repository has
   produced, sealed into a real MCAP; and `skynetk blackbox` reads it back and reports §74's five
   counters, which have said *unmeasured* for the life of the repository.
3. **`package.grants.gate`** — moved from skipped to measured, because its skip reason named a path
   that item 1 built.
4. **`skynet fleet` / `skynet device` / `skynet.fleet()`** — the one Phase 9 workflow with no client.

### What was found

Six things the logs recorded as fine and were not: a red Hub suite, a `main` that would have failed
CI's fmt job, a `--jerk` flag wired nowhere, advisory enforcement in a day-old file, an empty
`desired` whose obvious fix would have been a lie, and a workflow with no client. Each is written up
where it was found.

### Two decisions recorded rather than assumed

§26.2 — one ring, one writer — and the refusal to print a field-conformance figure over a window
nothing commanded.
