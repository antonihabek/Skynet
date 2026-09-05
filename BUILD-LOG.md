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

---

## 10. Four decisions, taken by the owner, recorded where they were open

The four items this session's audit filed as *blocked on the owner* were put to them and answered.
They are recorded in the plan's *Still not built* table and, for the last one, in §80 — not in this
file, because a decision that lives only in a session log is a decision nobody will find.

**None of the four is built yet.** What changed is that none of them is blocked.

### Account erasure — GitHub's model

The owner's answer was *"it should be like on GitHub"*, and GitHub's model is more specific than it
sounds, so it is written out rather than referred to:

| | |
|---|---|
| Their repositories | Deleted, public and private alike |
| What they wrote in **somebody else's** place | Kept, reattributed to a placeholder — comments, discussions, and **evaluations**. An evaluation is a claim about another person's model; deleting it silently moves that person's leaderboard position |
| Forks of their work | Survive |
| A namespace with other members | **Refuses the deletion** until it is transferred or deleted. This is GitHub's organisation rule and it is the one that stops a company losing its repositories because a founder left |
| Audit rows | Kept, actor reattributed. They exist for other people's security, not the actor's convenience |

**The one case GitHub has never had to answer** is the one this platform has: a published version
that robots are running right now. Those bytes stay until nothing references them — which is not a
new rule, it is the reachability [reclaim.ts](lib/skynet/reclaim.ts) already computes from
`ARTIFACT_REFERRERS` rather than from `ref_count`. A rollback target that vanished because somebody
closed their account is a robot that cannot roll back.

### Who may decide a report — a platform staff role

The question had to be asked twice, and the first answer is worth recording because the
misunderstanding is the natural one: *"you mean like an account handle?"*

It is not about handles. A **namespace** here is the account or organisation that owns things —
`acme` in `acme/gripper` — and the gap was that **nobody in this system has authority over anybody
else's**. `permissions.ts` grants capabilities within a namespace; `org_members` is the only roster;
neither describes a person who can act on a stranger's repository. So a report of *unsafe* could be
filed, stored, and read by its reporter, and then nothing could happen to it. Not even by the owner
of the platform, who had no role in the code.

Decided: **a platform staff role, held by the owner**, is the only thing that may move a report to
`upheld` or `rejected`, and upholding is the only thing that may hide another namespace's content.
Nothing else gains cross-namespace power, and every decision is audit-logged with who made it.

### Retention of sealed windows — chosen here, at the owner's instruction

*"I don't know which is correct so choose it yourself."* The reasoning §80 already carried decides
it: the Hub's acknowledgement is **another party's word**, and §13 exists precisely for the day that
party is the one being asked questions. So acknowledgement alone never authorises a deletion.

* A window an **incident names is never reclaimed**.
* A routine window is reclaimed only when all three hold: a receipt exists, the receipt's digest
  **is** the window's digest, and the disk is under §28's floor. Oldest first.
* A reclaim writes a **tombstone** — digest, seal time, byte count, reason.

The tombstone is the part worth defending. It costs almost nothing, it is kept for ever, and it is
the difference between a robot that cannot account for a window and one that can say what it held
and when it let it go. Only bytes are ever freed.

Nothing charges a namespace for a stored window on the Hub side either. That stays open, and it is
an accounting item rather than an evidence one.

### Collections — yes

Curated cross-repo sets, deferred seven times. Wanted. Out of *Still not built* and into the queue.

**Mirrors re-pinned** after the plan and §80 edits.

---

## 11. Phase 9's release gates — the whole of a sentence nothing had implemented

Phase 9's second bullet is *"protected release channels, review approvals, required checks, release
policy, and source-to-release traceability"*. A grep across `lib/skynet`, `supabase/migrations` and
`app/api/skynet` for `protected`, `required_check`, `release_channel` and `approval` returned
**nothing**. The platform's entire release process was: hold `repo.publish`, publish, and watch
`latest_version_id` move to whichever semver was highest.

### What a channel is, and the one thing it must never become

A channel is a named moving pointer at one published version — `acme/gripper-policy@stable`. The
load-bearing rule is written into the migration header and the module both:

> **A robot never resolves a channel.**

§11 says no control decision may require a network round trip, which is why a desired-state document
carries a pinned digest. A channel is resolved by a **person**, at the moment they promote, and what
reaches the fleet is the digest that resolution produced. The alternative — a fleet following
`stable` — is a design where promoting a version puts forty robots in motion with no deployment and
no record beyond a pointer update.

So this is a publishing concept and nothing in the device path reads it.

### Three checks, and the fourth that was written and deleted

| `scan` | every loadable file has been scanned, and none executes on load |
| `signature` | at least one signature over *this* version's digest from a key still active |
| `evaluation` | at least one succeeded, verified, non-demo evaluation |

**`compatibility` was written and removed.** A version is compatible *with a robot*, never in the
abstract, so a repository-level compatibility check has to choose its own subject — and a check that
chooses what it is about always passes. It would have been the most reassuring badge on the page and
would have meant nothing.

`scan` calls the existing `hardwareSafetyGate` rather than defining a second answer to the same
question, and adds exactly one condition the deployment gate deliberately lacks: **the file must
have been scanned at all.** The gate is right that an `unknown` verdict should not block — refusing
every unrecognised format makes a gate people route around — but `unknown` covers two facts. Scanned
and unclassifiable is a measurement; never scanned is the absence of one. §74.3's rule, one
subsystem over.

### The counting is the difficulty, not the storing

`tallyApprovals` is pure and has seven tests, because four plausible rules each produce a green tick
that is wrong:

* **Latest verdict per reviewer.** Approving after requesting changes is the normal path and
  produces two rows for one person.
* **An approval is about a proposal *and* a base.** Approving 1.2.0 onto a base of 1.1.0 is not
  approval of merging it onto 1.3.0. A verdict whose captured versions have moved is **superseded**
  — shown in the timeline, never counted — because the attack here is not adversarial, it is a busy
  repository.
* **An author does not approve their own proposal.** Enforced, not assumed.
* **One `changes_requested` blocks three approvals.** Deliberately asymmetric: a threshold is a
  number somebody chose, an objection is a statement that something is wrong, and out-voting it
  would make the count the safety property instead of the reading.

### A promotion freezes its evidence

Every input to a check moves — a scanner is updated, a key is revoked, an evaluation is superseded.
All three are correct behaviours and all three would silently rewrite history if the promotion
record were a join. *"It was green when it was promoted"* and *"it is green now"* are different
facts and an incident review needs the first. Same rule as `deployments_at_report` one table over.

The release trace reports both, named so they cannot be confused: `reviews.tally` is **now**,
`promotions[].evidence` is **then**.

### Two constraints worth naming

**Protection cannot be a badge with nothing behind it.** A channel marked protected with no
approvals and no checks reads as a guarantee on every surface and enforces nothing. Refused by CHECK
and by `validatePolicy`, so the message can name the field.

**Promotion refuses `latest`.** The dry run accepts it like every other read path; the write does
not. Promotion is the act that tells everybody else which version to use, and resolving a moving
pointer inside it lets the operator and the server disagree about what was promoted — the one place
in this feature where that is unrecoverable, because the channel is what other people then follow.

### Built

| | |
|---|---|
| `20260904220000_skynet_release_channels.sql` | three tables, six indexes, three RLS policies |
| `lib/skynet/reviews.ts` | verdicts, and the tally that decides what one is worth |
| `lib/skynet/channels.ts` | the check resolvers, the policy verdict, promotion |
| `lib/skynet/release-trace.ts` | version → source → proposal → reviews → promotions |
| 11 routes | reviews (2), channels (5), promote (2), trace (1) |
| `skynet review / channels / promote / trace` | four CLI commands |
| `Repo.reviews/review/channels/channel/promotable/promote/trace` | seven SDK methods |

`origin` on a trace is `direct`, `merged` or **`source_gone`** — the third being a version merged
from a repository since deleted or made private. A null would have collapsed that into "no source",
which is what a direct publish looks like.

### The eighth and ninth rows on a list that was seven

The client contract test's own argument is that *"a workflow with no client is a failure, not a
warning: the alternative is this list quietly acquiring an eighth exemption."* `release-channel` and
`review` were added **with** the feature rather than after it, so a channel API with no client would
be found the way `device` was.

### Verified

* `npx vitest run` — **1,726 passed**, 71 files (1,700 / 70 before)
* `npm run typecheck` — clean
* 24 new policy tests; the i18n error test caught all 23 new refusal keys, which is the test working
* 38 catalogue keys added, `i18n-catalog.mjs sync` run — **translation not run**, it needs the
  owner's approval per rule 17

### What this does not do, and it is the same gap this repository keeps finding

**The migration has not been applied, so not one of these three tables has ever held a row.** The
twenty-first session's finding was exactly this shape: the recordings receiver, the reports surface
and the incident chain were all written against a database where their tables did not exist, all
three were carefully reasoned and unit-tested, and the first real request found a column named in a
string literal and a whole collection missing from every document. Unit tests cover arithmetic;
the schema, the RLS and the order of the writes are what they cannot reach.

Also absent: any UI, and a `skynet-verify-channels.mjs` to drive the routes against a real database.
Both are next, and neither is worth writing before the tables exist.

### The panel, added after the log entry above

`components/skynet/ChannelsPanel.tsx`, and a `channels` tab on all five repository kinds — a
dataset with a `stable` channel is making the same claim a model is, and a repository whose
releases nobody may recommend is not a different kind of repository.

Two things on it are easy to render wrongly:

* **A channel with nothing on it says so.** `version_id` is null until the first promotion, which is
  the normal state of a channel declared before its first release. A blank cell would make a channel
  *waiting* for a release look like one whose release failed to load.
* **The evidence against a promotion is read from the frozen column**, never recomputed. A reader
  looking at last month's promotion is asking what was true last month, and a scanner update would
  otherwise rewrite the answer.

The panel ends on the sentence the whole feature turns on: *a channel is a publishing concept;
robots are never given one to resolve.*

58 catalogue keys total this round, synced. `npx vitest run` — **1,726**, typecheck clean.

**Still unapplied, still unrendered against real rows.** The panel has not been loaded in a browser
because the three tables do not exist yet in the database, and rendering it against an empty
`listChannels` would only prove the empty state.

### Applied, on the owner's instruction — and what applying it found

`psql` against the pooler, single transaction, `ON_ERROR_STOP`. Three `CREATE TABLE`, six
`CREATE INDEX`, three `ALTER TABLE … ENABLE ROW LEVEL SECURITY`, three policies, ten comments.

Then the part `skynet-verify-migrations.mjs` says in its own header it **cannot** do — *"policies,
indexes, triggers, grants, CHECK expressions and function bodies are not visible over REST"* — read
straight out of `pg_constraint` and `pg_policy`:

| | |
|---|---|
| three tables | RLS **enabled**, one policy and three indexes each |
| **eight CHECK constraints** | every expression exactly as written, including `required_checks <@ ARRAY['scan','signature','evaluation']` and the protection rule `protected = false OR required_approvals > 0 OR cardinality(required_checks) > 0` |
| **eleven foreign keys** | and the two that matter are `RESTRICT`: the version a channel points at, and the version a promotion names. A channel silently emptying because its version was removed is the failure that column exists to prevent |

**What this is and is not.** A CHECK constraint whose definition is right does refuse — Postgres has
no state where the expression is correct and unenforced — so this is real evidence about the schema.
It is **not** the end-to-end run: nothing has yet gone through `createChannel` → `promoteVersion` and
come back out. That is `scripts/skynet-verify-channels.mjs`'s job and it is not written.

### The finding: the Skynet schema holds no rows at all

```
 repos | versions | pull_requests | namespaces
     0 |        0 |             0 |          0
```

Fifty-eight sessions of "live checks against the production database", and the answer is that every
one of those scripts **builds its own fixtures and removes them in a `finally`**. That is the right
design and it is not the same sentence as the log has been writing. The database this platform runs
against is empty of Skynet data; what has been verified is that the code works against rows it
created seconds earlier, which is a much narrower claim than *"verified against production"* and
should be written that way from here on.

It also explains a first attempt at the behavioural probes that reported every constraint as failing
to refuse: `INSERT … SELECT … FROM skynet_repos LIMIT 1` inserts **nothing** when there are no repos,
raises no exception, and leaves the "did it refuse?" flag false. Seven probes, all reporting the same
alarming answer, and the giveaway was the *positive* control failing too — a probe suite where even
"this should be accepted" comes back false is measuring its own fixtures, not the thing under test.

---

## 12. §81.3's interlock, and the gate that had no caller

The item as §80 states it: *"Refusing to command a body that cannot warn. §81.3. A transition that
demands a tone and cannot produce one is a robot moving with a control missing from somebody's risk
assessment, and `skynetd` cannot decline it: §27's state is derived from conditions rather than
granted, so there is nothing to withhold. The rule belongs in the kernel's interlock."*

The kernel's interlock is `can_start`, and looking for it found something else first.

### `can_start` had no production caller

Four tests asked it. Nothing else in the workspace did. `skynetk` opened a driver, resolved limits
and turned a cycle without the start gate ever being consulted — so the kernel's own refusal rule
was correct, tested, and protecting nothing.

That is finding 107's shape exactly, one crate over: a rule written down is not a rule applied, and
the gap is invisible because the rule itself is fine.

### The third refusal

`WarningChannel` — three states, `no_std`, no allocator:

| `Ready` | declared, and no attempt has failed |
| `Failed` | declared, and the last attempt did not sound — a broken device |
| `Undeclared` | nothing declared on this robot |

`Ready` is deliberately the **weakest** claim available: nothing short of sounding a klaxon proves a
klaxon works, and a robot that has never been asked to warn sits here. Reporting anything stronger
would invent a measurement.

**The refusal is certified-only**, which is §81.3's own scoping and not a softening. The certified
tier's meaning is *the declared safety controls are present*; a certified robot moving without the
audible warning a risk assessment assumed makes that claim false. At host and guest §81.3's incident
and console line stand, and refusing every bench robot without a speaker is precisely the *"refuses
to start on every imperfection is a system that gets bypassed"* failure `can_start`'s own header
warns about.

`Undeclared` and `Failed` refuse separately and say so separately, because the fix differs: a
provisioning script that never finished, versus a speaker that is broken.

### What wiring it in does and does not do, stated exactly

`skynetk` now calls `can_start` before `Host::new`. **All three refusals are dormant in that
binary**, and it is worth writing down why rather than implying a gate went live:

* `transport_refused` is `false` — this binary builds its own clamp from the command line and has no
  plan that could fail to resolve. On a robot the plan arrives from §6.3's hand-off and can.
* enforcement is a pure function of tier (`Tier::enforcement`), so `resolve_limits` cannot produce
  the host-tier-resolved-to-advisory case. The existing test says this already: *"not reachable
  through `resolve_limits` … which is exactly when it is worth having"* — the gate defends against a
  limit set assembled some other way, which on a robot is the usual case.
* `--tier certified` is **deliberately not selectable** in `skynetk`, so the new refusal cannot fire
  here either.

The value is that the gate is now **on the path**. The warning channel is passed as `Undeclared`
because that is what is true — `skynetk` is a bench harness with no audio device and §81.1's
declaration lives in `state/audio.json`, which `skynetd` owns. Passing `Ready` to keep it quiet
would have been asserting a control this process has never had.

### Verified

* `cargo test --workspace` — **1,398 passed** (1,392 before), 0 failed. Kernel suite 101 → **107**
* `cargo fmt --check` clean, `cargo clippy --workspace --all-targets` — zero warnings
* `skynetk run --driver drivers/mock/mock_robot.dll --tier host --velocity-max 2.0 --jerk 0.1
  --watchdog-ms 50 --cycles 40 --hz 200` — 40 cycles, **zero overruns**, four guards naming what
  they lack. The interlock is on the path and the loop still makes its deadline

### §73's numbers, corrected

The table said *100,000 lines of Rust across seventeen crates with 1,376 tests* and *the Hub's own
suite is 1,499 tests*. Measured: **103,000 lines, eighteen crates, 1,392 tests** (before this item),
Hub **1,726**. The three crates past fifteen are named in the table now, since "eighteen" without
them is a number nobody can check.

---

## 13. §31.3's second edition — started, and honest about how far it got

§73's last column has carried one entry against §31 since the table was written: *"The second
edition, which is what the corpus is for."* `editions/python/` is the start of it.

**It is not yet an edition, and the runner says so on every line it prints.** §31.4: *"An edition
that has not passed the corpus is not an edition."* It passes **172 of 1,055 cases, in 3 of 31
corpora** — `vocabulary` (73), `semver` (61), `roles` (38) — and the other 883 are reported as *in
corpora with no handler*, counted rather than skipped.

That last property was designed before anything else. A runner that quietly passed three corpora and
printed "OK" would be the single most misleading artifact in this repository, and the temptation is
real precisely because the number is small.

### The discipline that makes any of it worth anything

**Every value is transcribed from `reference/typescript/`, never from `corpus/`.** A corpus exists to
catch a divergence between an edition and the reference; an edition generated *from* the corpus
agrees with it by construction and proves nothing. So the reference's source is read and the corpus
judges the transcription.

It paid twice on the first day.

**`reserved_capabilities` came out as eight members.** A first extraction pass used a regex that ran
past the end of the declaration. The real value is one token — `hri.warning` — and both the Rust and
TypeScript editions say so. A corpus-generated table would have had eight and been wrong in the same
direction as the tool that produced it, which is the failure mode that makes "generate it from the
expected output" so appealing and so useless.

**And the one the corpus caught on its first run: Python has arbitrary-precision integers and this
specification is IEEE-754.**

The `large-major` case compares `9007199254740993.0.0` with `9007199254740992.0.0`. Its note says in
advance what it is for: *"Components are read with `Number()`, so a value beyond an integer's exact
range rounds rather than failing to parse. Both editions must round it the same way."*

`int("9007199254740993")` is exact in Python. `Number(…)` rounds, and so does Rust's `f64`. So this
edition ordered the two versions **1** where the reference says **0** — and serialised both majors
identically, because canonical form renders the rounded value either way. A disagreement about
ordering that is invisible in the output is the worst shape a divergence can take, and nothing but a
byte-for-byte comparison finds it. Version components are read as doubles now, with the reasoning in
the function rather than in a commit message.

### What is built

| | |
|---|---|
| `canonical.py` | the serialisation rule, a deliberate **third** implementation beside `crates/skynet-json` and `reference/typescript/canonical.ts` |
| `vocabulary.py` | all 73 closed sets, in declaration order — order is part of the set, since `FAULT_RESPONSES` runs least to most severe |
| `roles.py` | §5's grammar, including the refusal: an unrecognised role parses to nothing, never to a nearby guess |
| `semver.py` | ordering and ranges, with `is_valid_range` kept apart from `satisfies` for §54.2's reason |
| `corpus.py` + `run_corpus.py` | the runner, and a dispatch keyed on corpus *and* file label — the Rust runner's own lesson about catch-all arms |

Two Python-specific traps are handled in `canonical.py` and written down there: **`bool` is a
subclass of `int`**, so a naive number branch renders `True` as `1`; and **`int` and `float` are
different types where the reference has one `number`**, so integrality decides the rendering and
never the Python type. An edition that let `1.0` render as `1.0` would diverge on the first velocity
ceiling anybody declared as a whole number.

No third-party dependencies, deliberately: an edition whose conformance run needs a package index
cannot be checked on the machine it is being reviewed on.

### Next, in dependency order

`predicate`, `digest`, `kinematics`, `watchdog`, then the four that carry the weight — `cycle` (§7's
eight guards), `transport` (§6), `admission` (§8), `certification` (§15). The foundation is the part
that was worth getting right; the rest is mechanical against a runner that will not lie about what
it has not done.

---

## 14. The release-channel feature had never held a row, and a live run found nothing wrong with it

Item 11 above ended on a sentence this repository has now written four times: *"the migration has not
been applied, so not one of these three tables has ever held a row"*. The migration was applied
later the same round, and what was still missing was the thing that had found the last three
defects of this shape — a probe that drives the real routes against the real database.

[`scripts/skynet-verify-channels.mjs`](../Robot%20Overflow/scripts/skynet-verify-channels.mjs) is
that, and it is the ninth verifier: **60 live checks**, every fixture removed in a `finally`.

The policy arithmetic is unit-tested and is deliberately not re-tested. What the probe covers is
everything the arithmetic sits on and no unit test can reach:

| | |
|---|---|
| the three CHECK constraints | an unknown check name, a protected channel with an empty policy, a review state that is not a verdict — refused through the route **and** straight at the table, because only the database's half cannot be bypassed |
| the unique index | `stable` cannot be declared twice |
| `ON DELETE RESTRICT` | the version a channel recommends cannot be deleted. The one constraint whose entire purpose is to refuse, and nothing that does not delete a row can see it |
| RLS, both ways | a public repository's channels are readable through the anon key; a private one's are not, and neither are the reviews on its proposals |
| the frozen evidence | read back out of `jsonb`, then the review it recorded is superseded and the evidence checked again — it does not move |
| the counting rules | approve, request changes, approve again: three rows kept, one verdict counted. Then the base moves underneath the approval and the tally reports **superseded**, not zero |

It passed on the second run. The three failures on the first were not in the feature.

### One was the probe's own, and it is worth writing down

`supabase.auth.signInWithPassword` stores the session **on the client instance**, and
`persistSession: false` keeps it off disk rather than out of memory. The probe signed in on the same
client it then used to ask *"what can the public see?"* — so two RLS checks were the owner reading
their own private repository, reported as a leak. The fix is a second anon-key client that never
signs in, and the same mistake would make a "public read works" check prove nothing at all.

### The other two were a real defect, and it was not in this feature

`origin` on a release trace came back `source_gone` — the value that means *"it came from somewhere
that is not there any more"* — for a version whose source was sitting in the table.

---

## 15. The query nineteen places in the Hub could not run, and the two layers that now say so

`skynet_repo_versions` and `skynet_repos` are joined **twice**: a version belongs to a repository,
and a repository points at its latest version (`skynet_repos.latest_version_id`). PostgREST will not
choose between them. `versions.select('…, repo:skynet_repos(…)')` is not a query that returns the
wrong row — it is a query that returns an **error and no rows**, decided while parsing the request,
before a table is read.

Nineteen embeds in nine modules were written that way. Every caller destructured `{ data }` and
discarded the error, so each produced a different plausible wrong answer:

| Module | What a user would have seen |
|---|---|
| `versions.ts`, `artifacts.ts` | a version listing and an artifact read, empty |
| `deployments.ts` | *"deployment not found"* for a deployment that exists |
| `training.ts`, `spaces.ts` | runs and Spaces invisible |
| `evaluations.ts` (5) | leaderboards and evaluation reads, empty |
| `reports.ts` | **`410 gone`** when reporting an unsafe *version* — the platform telling somebody the thing they are reporting does not exist |
| `repos.ts` | the robots a repository supports, empty |
| `release-trace.ts` | `source_gone`, which is the one that started this |

**Ten of the nineteen carried `!inner`**, and that is why they read as correct in review: `!inner` is
a join modifier and looks exactly like the disambiguating hint the query needs.

None of it was visible. The Skynet schema holds **no rows** — 0 repos, 0 versions, 0 deployments,
0 training runs, 0 spaces — so these queries had never run against anything, and a suite that mocks
the client parses no select strings. `tsc` sees `string`.

### Two layers, failing at different times

* [`lib/__tests__/skynet-embed-ambiguity.test.ts`](../Robot%20Overflow/lib/__tests__/skynet-embed-ambiguity.test.ts)
  — offline, every commit. Builds the foreign-key graph from `supabase/schema` and
  `supabase/migrations`, extracts every select in `lib/skynet`, walks each embed tree and fails on
  any embed between a twice-joined pair that does not name its constraint. Four of its nine cases
  test **the detector**, including that `!inner` does not satisfy it: a check that cannot fail is
  the thing this whole feature set is written against.
* [`scripts/skynet-verify-embeds.mjs`](../Robot%20Overflow/scripts/skynet-verify-embeds.mjs) — the
  tenth verifier. Puts **all 291 selects** in front of a real PostgREST with `limit(1)`. An empty
  database is fine, because the refusal happens at parse time. It therefore also catches what a
  graph read from SQL cannot: a column that does not exist, a table renamed, a relationship never
  declared.

### The scanner lied twice before it was right, and both lies are now checks

Writing the extractor reproduced this document's own subject in miniature.

1. A fixed character window paired `sk('skynet_pairing_codes')` with the `.select('handle')` of the
   *next* query in the file. Four reported failures were the scanner's. Pairing is by statement now
   — the codebase omits semicolons, so a statement ends where a line stops being a continuation.
2. Then it read **only the first line** of a multi-line template literal, so evaluations' three
   largest queries were dropped and the file reported clean. `collectSelects` now returns
   `{ selects, unresolved }` and both the test and the verifier fail on a non-empty `unresolved`,
   because a scanner that silently skips what it cannot read produces the same green run as one that
   checked everything.

A floor of 250 selects is asserted for the same reason: the failure mode of a scanner is not
miscounting, it is finding nothing.

### Verified

* `npx vitest run` — **1,735 passed**, 72 files (1,726 / 71 before). `npm run typecheck` clean
* `node scripts/skynet-verify-channels.mjs` — **60 checks**, all pass
* `node scripts/skynet-verify-embeds.mjs` — 291 selects, 160 distinct, **every one parses**
* Ten live verifiers now, and the Hub's schema record is the thing that made the offline half
  possible: it was added two rounds ago and this is the second defect class it has caught

---

## 16. The second edition, from 3 corpora to 8 — and what each transcription held

§73's last column has carried *"the second edition, which is what the corpus is for"* against §31
since the table was written. `editions/python/` went from **172 of 1,055 cases in 3 corpora** to
**328 in 8**: `predicate`, `digest`, `kinematics`, `watchdog`, and both halves of `frames`, which
`flatten` needs.

The discipline is unchanged and it is the whole value of the exercise: **every value is transcribed
from `reference/typescript/`, never from `corpus/`.** An edition generated from the corpus agrees
with it by construction and proves nothing.

### Four ways Python is quietly not JavaScript

Each of these produces a *different answer*, not a different message, and each is written down in
the module that handles it.

| | |
|---|---|
| `predicate` | **`True == 1`** is true in Python and `true === 1` is false. Equality compares types before values, or a rule reading `eq(state.enabled, 1)` fires here and not in the reference |
| `predicate` | Comparison coerces strings the way **`Number()`** does, not `float()`: `0x1f` is 31, `1e3` is 1000, a blank string refuses, and `Infinity` parses and is *then* discarded as non-finite. The corpus carries a case for each of the five |
| `digest` | Paths sort by **UTF-16 code unit**. Python's `sorted` is by code point, and the two disagree above U+FFFF — a surrogate pair begins at 0xD800 and sorts *before* U+E000. A naive edition hashes the file list in a different order and produces a wrong digest rather than an error |
| `kinematics` | **`math.trunc` returns an arbitrary-precision `int`** where `Math.trunc` returns a double. The range reduction converts back immediately, so nothing downstream is handed an integer Python keeps exactly and a double could not |
| `watchdog` | A window declared as `True` is a finite `int`, nests inside every real window, and reports a chain as **sound** on the strength of a value that is not a duration |

### The one that proves the arithmetic

All fourteen `kinematics` cases pass, and they are the ones worth having: `irrational-angles`,
`many-revolutions` and `past-the-exact-reduction` compare **thirty digits of translation**. That is
a third hand-written sine, cosine and square root — Newton's iteration from a bit-pattern seed, a
fixed five times; fdlibm's minimax coefficients with fdlibm's own digits; Cody–Waite reduction with
its second step unconditional — agreeing bit for bit with the other two.

It is also why those three are written out at all rather than taken from a platform library: §7.4's
certified tier mirrors this kernel on a separate safety processor, and a redundant channel that
disagrees with the primary about a clamp is precisely the failure the redundancy exists to detect.

### The mistake this file had already warned about

`digest`'s separator between a path and its hash is a **NUL**, and the reference's own header
records that writing it by eye is how the TypeScript edition came to use a space and found out from
a digest that did not match. This edition then wrote a **raw NUL** — arithmetically correct, and
invisible in every editor, terminal and diff. `git` reported the file as binary, which is how it was
noticed at all.

It is `_NUL = chr(0)` now, declared and named. The reference's comment was asking for exactly that
and neither edition had done it.

### Verified

* `python editions/python/run_corpus.py` — **328 passed, 0 failed**, 727 in corpora with no handler
* Complete: `digest`, `frames`, `kinematics`, `predicate`, `roles`, `semver`, `vocabulary`,
  `watchdog`. The runner still prints the 23 it cannot do on every line of every run
* `cargo test --workspace` — 1,398 passed, unchanged: nothing outside `editions/` was touched

---

## 17. Where this session stops

Stopped on request, at a clean point rather than mid-transcription. Everything below is committed
locally on `main` in both repositories. **Nothing is pushed.**

### What this session built

| | |
|---|---|
| §31.3's second edition | 3 corpora → **8**; 172 of 1,055 cases → **328**. `predicate`, `digest`, `kinematics`, `watchdog`, and both halves of `frames` |
| Phase 9's release gates | committed, and given the live verifier the round before it had not got to — **60 checks** |
| A defect class in the Hub | **19 PostgREST embeds in 9 modules** the database refuses to parse, each discarded into a plausible wrong answer. Fixed, with two layers of coverage |
| §81.3's interlock | wired to a caller, with the three refusals that are dormant in `skynetk` named rather than implied |

### Left mid-way, deliberately, and exactly where

`editions/python/kinematics.py` gained the six geometry functions §7.1's two pose guards consume —
`margin`, `segment_distance_squared`, `capsule_in_world`, `ignores`, `adjacent`, `distance` — and
they are the groundwork for `safety.py`, which is **not written**. Those functions are exercised by
nothing but a smoke test at present; the corpus does not reach them until the guards call them.

That was the next item, and it is the largest remaining lever on the edition: `safety.ts` alone
unlocks `limits` (19 cases), `cycle` (35) and `fuzz/cycle` (40) — 94 of the 727 outstanding.

**One runner change has to come with it.** `fuzz` is one corpus spanning three files — `cycle`,
`observation` and `transport` — and the dispatch is keyed on the corpus, so implementing one of the
three would send the other two into a handler that raises, counting them as *failures* rather than
as *unimplemented*. The honest fix is a per-file `Unimplemented` signal the runner counts in the
same bucket it already counts a missing corpus in, and a `complete` list that refuses to name a
corpus with an unimplemented file in it. That is the first thing to write, before any of `safety.py`,
because it is the property that keeps the number on every run true.

### The order after that, unchanged

`observation.ts` (16 + 48 fuzz cases), then `transport` (40 + 60), then `admission` (34) and
`certification` (18). The four that §31.3 calls the ones that carry the weight.

### Verified at the stopping point

* `python editions/python/run_corpus.py` — **328 passed, 0 failed**, 727 in corpora with no handler
* `cargo test --workspace` — **1,398 passed**, 0 failed, `cargo fmt --check` clean
* `npx vitest run` — **1,735 passed**, 72 files; `npm run typecheck` clean
* `node scripts/skynet-verify-channels.mjs` — 60 checks; `skynet-verify-embeds.mjs` — 291 selects,
  every one parses
* `node tools/check-contract-mirrors.mjs "../Robot Overflow"` — every mirrored contract matches;
  `docs/skynet-os-architecture.md` byte-identical in both repositories

---

# Round two — 2026-09-05

Resumed from §17's stopping point, in the order it named. Baselines reproduced **before** anything
was changed, so every number below is a delta rather than a claim:

| | At §17 | Reproduced today |
|---|---|---|
| `cargo test --workspace` | 1,398 | **1,398 passed, 0 failed** |
| `npx vitest run` | 1,735 / 72 files | **1,735 passed, 72 files** |
| `python editions/python/run_corpus.py` | 328 / 1,055 | **328 passed, 727 unimplemented** |

---

## 18. The runner's fourth bucket, and the check that it counts

§17 named this first and said why: `fuzz` is **one corpus spanning three files** — `cycle`,
`observation` and `transport` — and the dispatch is keyed on the corpus. Implementing one of the
three would send the other two into a handler never written for them, where they would raise and be
counted as **failures**. 108 cases nobody has transcribed would have been reported as 108 wrong
answers, and the number would have got worse exactly as the edition made progress.

`Unimplemented` is the signal. A handler declines a **file** by name; the runner counts those cases
in the same bucket as a corpus with no handler at all, keeps the two numbers separate, and refuses
to call a corpus `complete` while it holds a declined file:

```
result   422 passed, 0 failed, 633 not implemented (525 in corpora with no handler,
                                                    108 in files a handler declined), 1055 total
complete cycle, digest, frames, kinematics, limits, predicate, roles, semver, vocabulary, watchdog
declined fuzz/observation, fuzz/transport
partial  fuzz
```

`partial` is the line that matters. All forty of `fuzz/cycle` pass, and `fuzz` is still **not**
complete — which is the property the next four items depend on.

### Declining is a property of a file, and the runner enforces that

A handler that declines *some* of a file's cases and answers others is a defect in the edition
rather than a gap in it, so the whole file is evaluated before anything is recorded and a mixed
result is reported as failures. A number that read an inconsistency as honest progress is the one
thing this runner exists to prevent.

### The check found the rule, by disagreeing with it

`self_check.py` runs the runner against **synthetic** corpora with known answers and a swapped
dispatch table — 15 checks, four of which test the *detector* rather than the happy path. One of
them was written asserting that every case of a partially-declined file fails, "including the ones
that answered correctly", and it failed against the code.

The code was right. A sibling case that produced the right bytes **did** produce them, and failing
it to punish the handler would be a false claim in the opposite direction — the same error as the
one being guarded against, pointing the other way. Nothing can be hidden either way, because the
declined case is already a failure and the run is red regardless. The rule is written down in the
module now because writing the check the wrong way round is what found it.

The last check deletes the declining file and asserts the same corpus **becomes** complete, so an
exclusion that happened for some other reason would show up.

---

## 19. §31.3's second edition: `safety.py`, and the guards the corpus cannot reach

The largest single lever §17 identified, and it landed as predicted: `limits` 19, `cycle` 35 and
`fuzz/cycle` 40 — **94 cases, 328 → 422 passed, 0 failed**, on the first run.

966 lines of `reference/typescript/safety.ts` transcribed. Every value read from the reference,
never from `corpus/`, which is the discipline that makes the exercise worth anything: an edition
generated from the corpus agrees with it by construction and proves nothing.

### Passing first time is a claim that has to be earned

A suite that goes green on the first run has told you nothing until you have shown it can go red.
Five mutations, each reverted:

| Mutation | Cases it broke |
|---|---|
| the sign dropped from the magnitude clamp | 16 |
| the rate limiter removed (it runs *after* clamping, because clamping makes step changes) | 13 |
| a `hold` forwards the clamped command anyway | 17 |
| the `no_collision_margin` notice not emitted | 1 |
| proximity scaling removed | 6 |

### The sixth mutation broke nothing, and that is the finding

`_finite_number` rejects `bool` by name, because `typeof true === 'number'` is **false** in
JavaScript and `isinstance(True, int)` is **true** in Python. An envelope declaring
`joint_torque_max_nm: true` clamps every joint to **1 Nm** in a naive transcription and is discarded
outright by the reference. Deleting that guard changed **no case's answer**: the corpus does not
reach it.

That is not a gap in the corpus, and the fix is not to add a case. Every edition must pass the
corpus, and **Rust cannot represent the input at all** — `joint_torque_max_nm` is an `Option<f64>`
and serde refuses `true` before any rule runs. These are properties of a *dynamically typed*
transcription, so they belong to this edition, and `self_check.py` gained eight of them (23 checks
total), with expectations read from `safety.ts` rather than from `safety.py`:

| Trap | What the reference does, and what a naive transcription does instead |
|---|---|
| a boolean limit | Discarded; 80 comes from the transport clamp. Naively: **1 Nm on every joint** |
| `self_collision_margin_m === undefined` | Strict. Present-and-null emits **no** notice and still clamps at zero clearance; `.get` collapses the two |
| `capability !== undefined` | Absent means the runtime did not say and the gate **does not evaluate**. Present-and-null means it said null, which is not a granted token — so the gate **trips** |
| `Object.entries` order | Array-index keys first, ascending, *then* insertion order. A channel named `"2"` clamps before one named `"10"`, and `"09"` is not an index at all |

Three further mutations confirm those checks fail when the guards are removed (1, 2 and 2 checks
respectively).

### Two more divergences, written down where they are handled

`Math.sign` returns `-0` for `-0` and `NaN` for `NaN`, neither of which `copysign` reproduces — it
maps `NaN` onto `±1` and would turn an unrepresentable command into a confident clamp. And the
widening check reaches `null > ceiling` when a profile field is present and null, which JavaScript
coerces to `0 > ceiling`; that is transcribed rather than tidied, because a sensible reading of the
specification is a divergence from it.

### Verified

* `python editions/python/run_corpus.py` — **422 passed, 0 failed**, 633 not implemented
* `python editions/python/self_check.py` — **23 checks**, all pass
* `cargo test --workspace` — **1,398 passed**, unchanged: nothing outside `editions/` was touched
* Complete: `cycle`, `digest`, `frames`, `kinematics`, `limits`, `predicate`, `roles`, `semver`,
  `vocabulary`, `watchdog` — ten of thirty-one. `fuzz` is `partial` and says so

---

## 20. `observation.py`, and the two things the corpus could not tell apart

§25.1's frame assembly — **64 cases, 422 → 486 passed**, first run, zero failures. Then the same
discipline as §19: seven mutations, to show the suite could go red.

Four were caught (11, 13, 22 and 10 cases). **Three were not**, and two of those turned out to be
gaps in the corpus rather than in the transcription.

### The rounding the reference's own comment is about

`roundHalfAway` exists because §31.2 forbids taking a rounding from a platform library, and its
comment says why in the sharpest possible terms:

> two editions rounding a half-nanosecond differently is a one-nanosecond disagreement today and a
> **different chosen sample** the day two candidates are equidistant

Replacing it with Python's built-in `round` — which is half-to-**even** — broke **no case**. Nothing
in the corpus landed on a half. Including `sub-millisecond-latency`, whose note claims to pin
exactly this and whose 0.35 ms multiplies out to `350000.00000000006`.

### The budget that is one-sided on purpose

The staleness budget bounds how far *behind* the target a sample may be. A sample from *ahead* of it
is a clock fault rather than a stale one — visible in the signed age, and "reporting it as staleness
would send somebody to look at a capture pipeline that is working". Making the check two-sided broke
**no case**: every existing negative age was *inside* its budget, so the wrong reading agreed with
the right one everywhere.

### Both are now cases, because neither is a Python problem

Unlike §19's boolean trap, these are expressible in every edition — Rust holds `latency_ms` as an
`f64` and can represent a half — so they belong in the corpus, and the TypeScript changed first:

| New case | What it pins |
|---|---|
| `exactly-half-a-microsecond` | Instants 0, 500 ns and 2500 ns behind the target, so `age_us` and `worst_skew_us` land on **exact** halves. Half-away gives 0, 1, 3 and a skew of 3; half-to-even gives 0, 0, 2 and a skew of 2 |
| `future-sample-past-its-budget` | The only sample is 4 ms *after* the target with a 2 ms budget. Deliverable and not stale — a two-sided reading holds the frame |

**1,055 cases → 1,057.** Both mutations now fail. The Rust edition reproduces both new cases byte
for byte **without a change**: they pin behaviour that was already right in both editions and
protected in neither.

### The third uncaught mutation is a divergence in the reference

`roundHalfAway` is defined **twice** — `kinematics.ts` computes `Math.trunc(v ± 0.5)`, this file
tests the fraction — under one name and one stated contract. They disagree:

| input | `kinematics.ts` | `observation.ts` |
|---|---|---|
| `0.49999999999999994` | **1** | **0** |
| `-0.49999999999999994` | **−1** | **0** |
| `NaN`, `±Infinity` | `NaN`, `±Inf` | **0** |

`v + 0.5` rounds up to exactly `1.0` at the largest double below a half. Substituting the
`kinematics.ts` version into `observation.py` breaks nothing, which is the honest measurement:
**unreachable from any real latency, and still one function with two behaviours** — the shape of
defect §7.4's redundant channel exists to catch, in the module that argues for writing the rounding
out by hand in the first place.

Left alone deliberately. This edition transcribes each file's own version, because a transcription
that quietly unified them would be an edition disagreeing with the reference in order to be right.
Unifying them is a change to the TypeScript, the Rust kernel and the corpus in one commit, and it is
its own item rather than a drive-by inside a transcription round.

### Two more traps handled, both new to this module

* **`String(n)`.** A hold's detail interpolates the age with a template literal, and `String(13)` is
  `"13"` where `str(13.0)` is `"13.0"`. `_js_number_string` reproduces `Number::toString`, including
  its switch to exponential form outside `1e-7 … 1e21` — which `canonical.format_number`
  deliberately does *not* do, so the canonical writer is the wrong tool for this one job. Mutating
  it to `str` breaks **13 cases**.
* **`Number.isFinite`.** False for a boolean, where `math.isfinite` takes one as 0 or 1.

### Verified

* `python editions/python/run_corpus.py` — **488 passed, 0 failed**, 569 not implemented, of 1,057
* `python editions/python/self_check.py` — 23 checks
* `node tools/corpus-gen/generate.ts` — regenerating is a no-op after the two additions
* `cargo test -p skynet-corpus --test corpus` — **1,057 cases across 77 files, all reproduced byte
  for byte**
* `cargo test --workspace` — **1,398 passed, 0 failed**
* `node tools/check-contract-mirrors.mjs "../Robot Overflow"` — every mirrored contract matches
* Complete: eleven of thirty-one corpora. `fuzz` is `partial` with one file left

---

## 21. `transport.py` — and a defect a corpus is structurally unable to catch

§6's planner: how a policy's action space reaches a body, by name, by canonical role, through task
space, or not at all. **100 cases, 488 → 588 passed**, first run, zero failures. `fuzz` reached
`complete` — all three of its files written, so `partial` and `declined` are empty for the first
time since item 18 created them.

Ten mutations. **Four caught, six not**, and the six split cleanly into two kinds.

### The largest catch, worth stating on its own

Walking a warning's `subjects` as a Python `set` instead of in insertion order breaks **51 cases**
— *and that number is not stable*, which item 22 measures and which is the real finding. A
JavaScript `Set` iterates in insertion order and a Python `set` has *no promised order at all*, so
an edition that reached for the obvious type would produce entirely correct plans whose warnings are
shuffled, **differently between runs of the same input**. Two lists are built this way: the unmapped
roles, and the chains that need retargeting.

### Four of the six were gaps in the corpus

Every edition can express these, so the TypeScript changed first and the corpus grew 1,057 → 1,061:

| New case | The rule it pins, and what was producing identical bytes without it |
|---|---|
| `duplicate-role-first-wins` | A body declaring one role on two joints keeps the **first** — the reference's own comment says preferring the later one "would make the mapping depend on array order". Two joints rated 80 and 60 Nm, so the winner is visible in the binding's target *and* its limit |
| `scaled-to-an-exact-half` | A 128 Nm peak on an 80 Nm rating is a scale of exactly 0.625, so the warning's percentage lands on 62.5. `toFixed(0)` takes the **larger** integer on a tie and renders `63`; half-to-even renders `62`. Every other scale in the corpus was nowhere near a half |
| `sensor-declares-no-width` | `candidate.width ?? 0` makes an undeclared dimension a **zero** rather than a match, so an IMU with no width against a 640-wide requirement *downscales*. Reading the absence as "no constraint" reports the stream as direct and hands the policy a frame nobody checked |
| `zero-watchdog-is-no-watchdog` | A watchdog window of zero is not a window: no `min_control_hz`, no `rate_below_watchdog`. The `> 0` guard appears in two separate places and neither was covered |

### Two of the six are not the corpus's to hold

`allowRetarget` read as truthy rather than strictly `true`, and an absent `peak` conflated with a
null one. Rust's `allowRetarget` is an `Option<bool>` and cannot hold `1` or `"yes"` at all, and
serde reads an absent field and an explicit null into the same `None` — so a corpus every edition
must pass cannot carry either. They went to `self_check.py`, **23 checks → 27**, and both mutations
now fail two checks each.

The `allowRetarget` one is worth reading twice: the *grant* is an identity test and the *message
branch* is a truthy test, so a truthy non-boolean refuses retargeting and then blames a missing
solver on a body that has one. Transcribed as it stands, and pinned, because it is the reference's
sentence rather than this edition's opinion of it.

### The defect none of this would ever have found

`duplicate-role-first-wins` was the first case in the corpus with exactly **one** unmapped joint,
and it printed:

> `1 joint on this body **are** not driven by the policy and will be held by the controller.`

The plural was on the noun and not on the verb. **Seventeen existing cases carried it**, every
edition reproduced it byte for byte, and the corpus was silent — because all three editions had the
*same* bug. A corpus detects editions disagreeing. It is structurally unable to detect them agreeing
about something wrong, and no amount of adding cases changes that; only reading the output does.

Fixed in the order the mirror requires: `reference/typescript/embodiment.ts` first, since it is the
specification; then `crates/skynet-contracts/src/transport.rs`, which has to produce the same bytes
or the corpus test fails; then this edition. Corpus regenerated, and the Hub resynced — `embodiment`
is a mirrored contract, so the change was one the Hub owed.

### Verified

* `python editions/python/run_corpus.py` — **592 passed, 0 failed**, 469 not implemented, of 1,061
* `python editions/python/self_check.py` — **27 checks**
* `cargo test -p skynet-corpus --test corpus` — **1,061 cases across 77 files, byte for byte**
* `cargo test --workspace` — **1,398 passed, 0 failed**
* `npx vitest run` (Hub) — **1,735 passed**, 72 files, with the resynced contract
* `node tools/check-contract-mirrors.mjs "../Robot Overflow"` — every mirrored contract matches
* `node tools/corpus-gen/generate.ts` — regenerating is a no-op
* **Thirteen of thirty-one corpora complete.** 328 → 592 cases over three rounds

### Round state

Three commits on `main` in the OS repository and one in the Hub, **none pushed**.

| | At §17 | Now |
|---|---|---|
| Python edition | 328 / 1,055 in 8 corpora | **592 / 1,061 in 13** |
| Corpus | 1,055 cases | **1,061** — six added, each closing a mutation the suite survived |
| Editions holding a shared defect | 3 | 0, for the one that was found |

---

## 22. `admission.py` and `certification.py` — a bug in the Rust edition, and a question that is not mine

The last two of the four §31.3 calls *the ones that carry the weight*. **52 cases, 592 → 644
passed**, both first run, zero failures. Sixteen mutations between them; three survivors became
corpus cases, and two of those found something larger than a gap.

### The Rust edition certified a robot on a stale pass

`certification/award.json :: duplicate-results-keep-the-last` reports one test twice — a pass, and
then the failing re-run that replaced it. The reference builds a `Map` from the results, so a later
entry overwrites an earlier one and the **failing** result governs: award refused.

`crates/skynet-contracts/src/certification.rs` used `input.results.iter().find(..)`. **First wins.**
It awarded `guest` on the stale pass.

```
expected: {"awarded":"none", "failures":["rhal.abi.version"], …}
actual:   {"awarded":"guest","failures":[],                   …}
```

That is the same answer on every input where a test is reported once, which is every input the
corpus had held for as long as it has existed. A vendor re-running a failed conformance test and
submitting both results would have been certified — by the edition that runs **on the robot**, which
is the one §12's kernel gate reads. Fixed, with the reason and the case named at the line.

### A version comparison the editions answer differently, and why it is still open

§8.2's `min_os_version` gate splits both versions on `.` and compares the segments as numbers, and
the two editions disagree about a segment that is not one:

| | robot on `1.2.0-rc1`, package needs `1.2.1` |
|---|---|
| reference, and the Hub that mirrors it | **admits** — `Number("0-rc1")` is `NaN`, `NaN < 0` is false, so the robot reads as *newer* |
| `skynet-contracts`, on the robot | **refuses** — an unparseable segment falls back to `0`, so the robot reads as *older* |

The Hub tells an operator the package will install and the robot refuses it: the §17 disagreement
this whole corpus exists to prevent, invisible because no case carried a malformed version.

**Not fixed, and that is the decision.** Making the Rust edition agree would ship the reference's
answer to the robot, and the reference **fails open**: a robot whose version has any non-numeric
segment satisfies *every* `min_os_version`, which is precisely what that field exists to prevent.
Making the reference agree is a change to the specification, which the Hub's mirrored `package.ts`
inherits. Neither is a transcription's call, so it is written into **§80** as an owner's decision
with both consequences stated — and `os-version-with-an-empty-segment` pins the neighbouring case
they *do* agree on (`Number("")` and a failed Rust parse are both `0`), so the agreement cannot rot
while the disagreement is open.

### The other case: §49.1 is checked before the operator's denials

`admission/install.json :: reserved-and-denied` requests the warning channel *and* has it withheld
by the profile. The reference checks the reserved token first on purpose — a reserved token is not
something an operator declined, it is something nobody can grant — so `capability_reserved` is
reported before `capability_denied`. No case had a token that was both, so the ordering held by
luck; moving the block now fails.

### A hazard in the mutation harness, and every figure re-measured

One mutation reported a failure against code that was no longer on disk. A mutation whose
replacement has the **same byte length** as the original can leave a stale `__pycache__` after the
revert: CPython keys the cache on `(mtime, size)`, and a revert inside the same second changes
neither. The reordering mutations are exactly the same-length kind.

Every mutation figure across all five modules was re-measured with the cache cleared before each
run — **34 of 34 caught**, baseline zero — and one figure changed meaningfully.

### The number that is not a number

Item 21 recorded the `set`-ordering mutation as breaking 51 cases. It does not break 51 cases. Six
identical runs of the identical mutated code:

```
41, 44, 48, 52, 58, 69
```

CPython randomises string hashing per process, so a `set` there does not merely order the warnings
wrongly — it orders them **differently on every start**. An edition that reached for the obvious
type would produce entirely correct plans whose warnings shuffle between runs, and a suite that
happened to pass once would fail the next time nobody changed anything. The variance is a better
statement of the hazard than any single figure, and item 21's is annotated rather than silently
replaced.

### Verified

* `python editions/python/run_corpus.py` — **648 passed, 0 failed**, 417 not implemented, of 1,065
* `python editions/python/self_check.py` — **29 checks**
* `cargo test -p skynet-corpus --test corpus` — **1,065 cases across 77 files, byte for byte**
* `cargo test --workspace` — **1,398 passed, 0 failed**
* 34 of 34 mutations caught, cache cleared before each
* `docs/skynet-os-architecture.md` byte-identical in both repositories after the §80 edit
* **Fifteen of thirty-one corpora complete**, and all four §31.3 calls the load-bearing ones

### Round state

| | At §17 | Now |
|---|---|---|
| Python edition | 328 / 1,055 in 8 corpora | **648 / 1,065 in 15** |
| Corpus | 1,055 cases | **1,065** — ten added, every one closing a mutation the suite survived |
| Defects found in another edition | 0 | **2** — one fixed, one referred to §80 |

Four commits on `main` in the OS repository and one in the Hub. **Nothing pushed.**

---

## 23. `skill`, `opstate` and `identity` — and the most dangerous line nothing was testing

Three modules, **163 cases, 644 → 815 passed**, all three first run, zero failures. Eighteen
corpora complete. Thirty-nine mutations across the three; **thirty caught**. Of the nine survivors:
four became corpus cases, three went to `self_check`, one is §80's new open question — and one was
a badly built mutation of my own that changed nothing, which a correctly built version of it caught
immediately.

### The default this module's own header calls out by name

`opstate.py` — §27's machine — carries this about its inputs:

> Every field is optional and every default is the least capable reading. A caller who has not
> observed something has not established it, and a state machine that read "nobody told me about the
> kernel" as "the kernel is live" would be **the single most dangerous line in this repository**.

Defaulting `kernel` to `live` broke **no case**. Twenty-five cases reach that rule and every one of
them names the kernel explicitly; the only case that leaves it out — `observed-nothing` — leaves out
*everything*, and is refused four rules earlier by the RHAL default. So the one default the file
singles out was the one nothing reached on its own.

`a-kernel-nobody-observed-is-down` observes every condition **except** the kernel. An edition
defaulting to `live` there reports a robot as `running` on the strength of a field nobody filled in.

### Two more gaps in the same run

| New case | What it pins |
|---|---|
| `a-key-the-vendor-signed-with-no-certificate-to-check-it` | `attested` takes **both** halves. Every existing case had the certificate trusted *or* the key absent, so an edition requiring only the hardware binding agreed everywhere |
| `a-credential-that-declares-no-scopes-at-all` | An empty scope list is not an absent one: absent is an opaque bearer token (a warning), empty is a credential that declared it carries nothing (no finding). Truthiness reads the first for the second |

Plus `root-absent-is-a-root-invocation` for `skill`: the test is `root === false`, not a truthy one,
so an invocation that never said acquires its own leases. Read loosely, an unlabelled invocation
skips `acquiring` entirely and runs **holding nothing** — §25.2 bypassed by omission. Every other
lifecycle case sets `root` explicitly, so both readings agreed on all of them.

**1,066 → 1,069 cases.** The Rust edition reproduces all four new ones byte for byte.

### A third divergence between the editions, and it is left open too

Transcribing `identity.py` raised a question about `String.length`, and probing it confirmed one:

| | eight astral characters typed as a pairing code |
|---|---|
| reference, and the Hub | `too_long` — *"A pairing code is 8 characters and this is **16**"* |
| `skynet-contracts`, on the robot | `not_in_alphabet` — *"Character **1** of the 8…"* |

`.length` counts UTF-16 code units and `chars().count()` counts code points. Both editions refuse,
so nothing unsafe is shipping either way — what is wrong is that §64 requires a refusal to name its
fix, and these name two different fixes for one typo. The reference's sentence is also **false as
English**: somebody who typed eight characters is told they typed sixteen.

So this is the rare divergence where the *reference* is the edition that should change, which makes
it a specification edit the Hub's mirrored `identity.ts` inherits. Recorded in **§80**, and the
probe case was **withdrawn** rather than left red: pinning either answer would settle it by whichever
edition was easier to edit.

### What went into `self_check` instead, and why each had to

Five strict-`true` tests that Rust's `Option<bool>` cannot express, so no corpus every edition must
pass can reach them — **31 checks → 34**:

* `hold === true` decides which clauses are re-checked every cycle, which is the whole of §54.2's
  second rule: *"battery dropping below reserve mid-skill should stop the skill, not be discovered
  at the end."*
* `stop_settled === true` decides whether a robot mid-ramp is reported `safe`.
* `credential === true` decides whether a robot is treated as paired.

### The transcription traps worth naming

* **`Math.round` is half toward +∞** — neither of Python's roundings. It rounds a *timeout in
  milliseconds*, so the disagreement is a skill stopped a millisecond apart and, at the boundary, a
  different verdict. Mutating it breaks a case.
* **An array is an object to JavaScript**, so a clause whose predicate is a list reaches the
  evaluator and is refused as malformed, rather than being called unenforceable. Two different
  reports, and the reader is not the place that decides which.
* **`String.length` and `Array.sort` both work in UTF-16**, which is the same disagreement
  `digest.py` documents, reached from two more directions.
* **`String.prototype.trim` is a smaller set than Python's `strip()`** — Python also removes
  `\x1c`–`\x1f`, so a namespace of one of those is empty to Python and non-empty to the reference.

### Verified

* `python editions/python/run_corpus.py` — **815 passed, 0 failed**, 254 not implemented, of 1,069
* `python editions/python/self_check.py` — **34 checks**
* `cargo test -p skynet-corpus --test corpus` — **1,069 cases across 77 files, byte for byte**
* `cargo test --workspace` — **1,398 passed, 0 failed**
* `node tools/corpus-gen/generate.ts` — regenerating is a no-op
* `docs/skynet-os-architecture.md` byte-identical in both repositories
* **Eighteen of thirty-one corpora complete.** Thirteen remain, 254 cases

---

## 24. `transfer`, `power` and `ota` — the boundaries nothing stood on

Three modules, **87 cases, 815 → 902 passed**, all three first run. Twenty-one corpora complete.
Twenty-five mutations; **twenty-three caught**, and both survivors were the same shape: a comparison
whose boundary no case sat exactly on.

### `partial >= size`, and the case that was never the equals

Every fetch case in the corpus had a partial well under the declared size or well over it. The
boundary — a partial that is **exactly** the whole length — was untested, and it is the interesting
one: either the transfer finished and nothing recorded it, or these are somebody else's bytes of the
same length. Both are answered by discarding and starting again. An edition testing `>` alone
resumes from the end of a file and sends nothing at all.

`a-partial-exactly-the-size-of-the-image` is that case.

### Two gated components, and the order nobody pinned

`gated_components` is reported twice over: as a sentence an operator reads before booking bench
time, and as a fault's `subjects`. It is sorted. No bundle in the corpus declared **more than one**
gated change, so an edition emitting them in declaration order agreed about every verdict and
disagreed about every list. `several-gated-changes-at-once` declares three out of order, one of them
a name this edition does not recognise — because §12's rule is that an unrecognised component is
gated too.

**1,069 → 1,071 cases**, and Rust reproduces both without a change.

### The mutation that broke twenty-one cases

Treating a temperature with no declared threshold as `critical` rather than `nominal`. The rule
exists because 85 °C is right for a CPU package and fifty degrees wrong for a harmonic drive, so a
body that has not said what hot is gets a **fault** and nothing shed — and the corpus holds it
hard, which is what a rule that decides whether a robot gives up its own sensors deserves.

### The four traps this batch added

* **`Array.prototype.includes` is `SameValueZero`**, so `[1, 2].includes(true)` is false where
  `True in [1, 2]` is true. It guards two things: §36.4's brownout override — *the one rule that
  ignores the gauge and stops the robot immediately* — and §12's compatible-board list.
* **`Math.trunc` truncates toward zero** and Python's `//` floors. Every use here is clamped at zero
  afterwards, so the two agree on every input that reaches the file, which is exactly the kind of
  agreement that stops being true when somebody moves the clamp.
* **`new Set(…)` then `.sort()`** is insertion order then UTF-16 order; a Python `set` is neither.
* **`String(600000)`** is `"600000"` and `str(600000.0)` is `"600000.0"`, in a sentence the corpus
  compares byte for byte.

### Verified

* `python editions/python/run_corpus.py` — **904 passed, 0 failed**, 167 not implemented, of 1,071
* `cargo test -p skynet-corpus --test corpus` — **1,071 cases across 77 files, byte for byte**
* 25 of 25 mutations caught, cache cleared before each
* **Twenty-one of thirty-one corpora complete.** Ten remain, 167 cases: `cell` 26, `checkin` 23,
  `blackbox` 18, `calibration` 18, `provenance` 17, `telemetry` 16, `live` 15, `readiness` 12,
  `birth` 11, `intent` 11
