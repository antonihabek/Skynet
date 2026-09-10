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

---

## 25. `birth`, `intent` and `readiness` — logged after the fact

Committed as `versions` before this session and never written up, so the record is completed here
rather than left as a commit message. Three modules, **34 cases, 904 → 938 passed**, twenty-four
corpora complete. Nothing in them needed a decision; they are the three smallest files left and they
went through first run.

---

## 26. `checkin` — the document §17's whole rule rests on, and a defect it found in Rust

`checkin` is three files and **23 cases**, and it is the one corpus where the three files are three
different ways for an edition to be wrong rather than three parts of one answer. `claim` is the
field §17's rule lives on — *the platform never claims a robot is running something it has not
heard from* — `health` is §33.6's age rule, and `document` is the bytes that actually cross to the
Hub. An edition can reproduce the first two and assemble them differently, which is the case only
the third catches.

**938 → 961, first run, no case needing a second look.** Twenty-five corpora complete.

### Then the mutation run, which is where the round actually started

Nineteen mutations, **fifteen caught**. The four survivors were not noise, and they split cleanly
into two kinds:

| Survivor | Kind |
|---|---|
| `deferred === true` becomes truthy | The corpus cannot express it — `deferred?: boolean` |
| An unrecognised readiness word defaults to `ready` | Same — `readiness?: ReadinessState` |
| `pending` emitted unsorted | **A corpus gap.** Every input is well-typed |
| A digest survives a slot nobody read | **A corpus gap**, and a defect |

The first two are the class §31.4 leaves to `self_check.py`: a case carrying a `1` where the
reference declares a boolean is a case no typed edition can represent, and Rust decodes it at the
boundary before the branch is reachable. Four checks were added there, each with the positive
control beside it so a check that passes by finding nothing shows up as one.

The other two became corpus cases, which is the order §31.4 asks for — **a new case lands in the
corpus before the fix lands in any edition**.

### `pending` was sorted and nothing had ever proved it

`blocking` is sorted, and it has been pinned since the first check-in case. `pending` is a **second**
list of the same facts, sorted separately in the builder — and no document case had ever carried
more than one entry, so an edition emitting it in declaration order agreed about every claim in the
corpus and disagreed about the bytes of every real check-in from a robot with two things left to do.

The reason it has to be sorted rather than merely stable is §34.4: check-ins are **collapsed**, and
two documents differing only in the order of a list are two documents a queue cannot tell are
duplicates. `two-stages-left-and-the-order-nobody-pinned` declares two out of order. Rust already
sorted, and reproduces it without a change.

### The digest on a slot nobody read — a divergence, and the reference was right

A measure carries a `state` and a `digest`, and both are well-typed independently. `{ state:
'unmeasured', digest: 'sha256:aaaa' }` is a document that says nobody read the slot and carries a
reading of it in the next field, and **no case had ever put the two together** — so what a consumer
sees was unpinned, and the two editions had quietly answered it differently for as long as the field
has existed:

* `reference/typescript/shape.ts` gates the digest on the state. `unmeasured` emits `null`.
* `crates/skynet-contracts/src/identity.rs` emitted whatever was in the field.

This is not a coin toss, and the argument is the query rather than the tidiness. §32.1 puts a fleet
column on *"which robots are running image `sha256:…`"* — the question an incident is run through —
and Rust's answer puts a robot whose image slot was **never read** into that result. The prose half
of the same document already says `an unmeasured OS image (the slot could not be read)`, so passing
the digest through has one report contradicting itself, in the machine-readable half, on the side
where nobody is reading English to catch it.

`a-digest-on-a-slot-nobody-read` is the case. It went in first, the Rust corpus test went red on it
alone, and `measure_to_json` now gates the digest on the state with the reasoning in its doc comment
and the case named there. **1,073 cases, both editions, byte for byte.**

### One thing deliberately not done

`shapeMeasure` and Rust also disagree about an *unrecognised* state word, and the Python edition
normalises it to `unmeasured` at the boundary as Rust does. No case pins it and none was added: an
unrecognised `MeasureState` is not a well-typed input either, so it belongs with the first two
survivors rather than with these — and `_measure_block` says so where it makes the choice.

### A stale mirror pin, found by running the check rather than by reading it

`docs/skynet-os-architecture.md` was byte-identical in both repositories and pinned at digest
`1a5d70bb…` while both trees held `c44f772b…`. The §80 rows added in `b39c6d5` and `a78595b` were
mirrored correctly and the pin was never moved, so `check-mirrors.mjs` had been reporting the
specification as changed-without-repinning since. Re-pinned from the two agreeing copies with
`--pin`, which moves no file. Nothing in the Hub was touched.

### Verified

* `python3 editions/python/run_corpus.py` — **963 passed, 0 failed**, 110 not implemented, of 1,073
* `python3 editions/python/self_check.py` — **38 checks**, four of them new
* Both new self-checks confirmed against the mutation they exist for: 2 of 38 and 3 of 38 red
* 17 of 19 mutations caught by the corpus; the two survivors are the two `self_check.py` covers
* `cargo test --workspace` — **1,431 passed**, 0 failed, 7 ignored (four need a C compiler)
* `cargo fmt --check` and `cargo clippy --workspace --all-targets` clean
* `tsc -p .` clean; the generator reproduces all 77 files byte-identically
* `check-golden-path.mjs`, `check-contract-mirrors.mjs`, `check-mirrors.mjs` all clean

**Twenty-five of thirty-one corpora complete. Six remain, 110 cases:** `cell` 26, `blackbox` 18,
`calibration` 18, `provenance` 17, `telemetry` 16, `live` 15.

---

## 27. `cell`, and this edition running in no CI job at all

`cell` is §55.2's descriptor and **26 cases in two files**, split because the document has two
consumers that could be got right independently: `validate` is what an operator sees when they save
it, `context` is what a precondition and the workspace guard read out of it afterwards.

**963 → 989, first run.** Twenty-six corpora complete.

### Nineteen mutations, and three kinds of survivor

Five survived, and sorting them was most of the round:

| Survivor | What it turned out to be |
|---|---|
| A non-finite box corner | **The corpus structurally cannot carry it** — and this repository already wrote that down |
| An unknown shape kind decoding as a sphere | A **no-op mutation**: `cylinder` never enters the sphere branch. The sharper version is caught |
| A shadowing fact winning | **Redundant by construction**: the filter and the overwrite order are two guards, and the filter is caught |
| `volume: null` on a fixture | A real gap. Well-typed |
| The stop after a duplicate fixture | A real gap. Well-typed |

Two of those are worth the paragraph they cost, because both look like corpus gaps and neither is.
A mutation that survives is not a finding until it has been shown to change an answer, and two of
these five changed nothing — one because the branch it edited is unreachable for the only input
that exercises it, one because a second guard already covers it. Re-mutating sharper is what
separated them.

### The one the generator cannot express, and the note that already said so

`tools/corpus-gen/generate.ts` has carried this since the frame-bindings rule was written:

> The non-finite coefficient rule has **no case here, and cannot have one.** This generator writes
> every input through `JSON.stringify`, which renders infinity as `null`, so the frozen input would
> be a *different* body from the one the expectation was computed from — worse than no case at all,
> because it would fail an edition for reading the file correctly.

Exactly the same applies to a box corner, so the check went to `self_check.py` with a coordinate of
`true` beside it — which JSON *can* carry and no typed edition can produce. Both matter because
Python collapses them by accident: `isinstance(x, float)` is true of `NaN` and of both infinities,
and `isinstance(True, int)` is true of a boolean, so the obvious coordinate check admits an infinite
box and a corner at `true`.

### The two that were real, and what each is about

`a-fixture-named-and-not-measured` pins a `volume` written out and left `null`. §55.2 permits it —
*a fixture may be named without being measured* — and the check that follows is truthy for exactly
this document. An edition testing for the key reports an operator who was careful enough to write
the field as one who measured it wrongly.

`a-duplicate-fixture-measured-wrongly` pins that the walk **stops** at the duplicate. The second
`jig-a` is not the fixture any precondition will reach, so its geometry is not a second thing to
tell the operator about — an edition that fell through would have somebody fixing the volume on the
copy that is about to be deleted. Rust reproduces both without a change.

### Then the finding that was not in any transcription

**The Python edition was in no CI job.** The `corpus` job's own comment said so without meaning to:

> Every edition sits the same exam. **One row today.**

That was written when it was true. Eight rounds later a second edition existed, at 991 of 1,075
cases, and it ran nowhere but on the machine of whoever last touched it — so a change to
`reference/typescript/`, to the corpus, or to a shared module would have broken it silently. §31.4
asks for differential checking on every commit and this was half of it.

Both editions run now. The Python row is not a smaller copy of the Rust one, and the difference is
what makes it safe to gate on: the runner passes when every case it *answers* is byte-identical and
reports the rest as not implemented, so the step goes red on a divergence and stays green on a gap.

### `--min-passed`, because every other check here has a floor of zero

One regression the runner's own design cannot see: an edition whose handler was deleted answers
nothing, fails nothing, and exits 0 — the cases move from *passed* into *not implemented*, which is
the bucket that exists to read as honest progress. The ratchet is a number in the workflow rather
than a default in the runner, because it is a fact about history rather than about the code, and
lowering it is then a decision somebody makes in a diff. Verified both ways: `--min-passed 991`
green, `--min-passed 992` red with the sentence explaining why nothing above it went red.

### One refactor, and the corpus caught it immediately

`js_trim` and `utf16_key` were about to have a third copy, so they moved to
`editions/python/skynet_os/js.py` — the four JavaScript expressions Python has no equivalent for,
in one place, because a divergence helper that exists twice is one that will eventually disagree
with itself. The move dropped `utf16_at` on the way past and eleven `identity` cases went red on
the next run. That is the corpus doing the job it exists for on a change that touched no rule.

### Verified

* `python3 editions/python/run_corpus.py --min-passed 991` — **991 passed, 0 failed**, 84 not
  implemented, of 1,075
* `python3 editions/python/self_check.py` — **41 checks**, three of them new
* 16 of 19 cell mutations caught by the corpus; of the three survivors, one is covered by
  `self_check.py` (confirmed red on the mutation) and two were shown to change no answer
* `cargo test --workspace` — **1,431 passed**, 0 failed, 7 ignored
* `cargo fmt --check` clean; `tsc -p .` clean; the generator reproduces all 77 files
* `check-golden-path.mjs` and `check-mirrors.mjs` clean; the CI workflow parses

**Twenty-six of thirty-one corpora complete. Five remain, 84 cases:** `blackbox` 18, `calibration`
18, `provenance` 17, `telemetry` 16, `live` 15.

---

## 28. `blackbox` — the corpus whose subject is bytes, and a case that never reached its own rule

`blackbox` is §26's ring record, the ring header, the six MCAP channel schemas and a whole sealed
file. **18 cases, 991 → 1,009, first run** — including the SHA-256 of a 19,805-byte MCAP file
written by a third independent implementation of the framing, and all six schema documents
byte-for-byte.

That last part is worth stating plainly, because §31.2 puts these on the short list of things that
stay single-source across editions on the grounds that *a black box sealed by one edition must be
readable by every other*. Until this round that was two implementations agreeing. It is three now,
and the third was written from the specification without ever opening the corpus.

### Twenty-two mutations, nineteen caught, and the first survivor is the round

`frame-astral-stream-name` exists to pin the truncation rule. Its note says a cut inside a
multi-byte sequence *"would put invalid UTF-8 in the ring and every reader would have to guess"*,
and its name is nine **two-byte** characters in a sixteen-byte field.

Sixteen is even. The boundary falls exactly between the eighth character and the ninth, the back-off
loop looks at a lead byte, and it exits having done nothing. **Deleting the loop entirely broke no
case.** The one case written for this rule does not reach it, and has not since it was added.

`frame-a-stream-name-cut-inside-a-character` is six **three-byte** characters. The boundary lands
inside the sixth, the cut walks back to fifteen, and the field holds `e38182` five times and a NUL.
Without the loop it would hold sixteen bytes ending in a bare `e3` — invalid UTF-8 in a recording
every later reader has to interpret.

This is the second time this session a case's *note* has been the thing that was true and its
*input* the thing that was not. It is a different failure from a missing case: nobody adds a case
for a rule that already has one.

### Two more boundaries nothing had sat on

**`cycle-every-guard-tripped`.** `MAX_TRIPPED` is 8, the guard table is 8 bytes at offsets 104–111,
and `capability` is at 112. No case had gone past **one** tripped guard, so neither the clamp on the
count nor the slice on the table had ever been exercised at the edge they exist for. The record now
reads `08`, then `0102030405060708`, then `ffff` — which says in one line that the count field, the
table and the field after it are the sizes the format claims.

**`three-messages-on-one-channel`.** The existing seal case writes one message to channel 6 and one
to channel 1, so the per-channel sequence is 1 on both and an edition writing a constant produces
the same file. A reader uses that number to detect a message it did not receive, so a constant is a
recording that can never report a hole in itself. The third message is also **out of order in
time** — 3000 ns then 2000 ns, which is what writing from a ring looks like — which pins the
summary's `message_start_time` as the minimum rather than the first one seen.

**1,075 → 1,078.** Rust reproduces all three without a change; each pins behaviour that was already
correct in both editions and unprotected in both.

### The last survivor is a question about the format, and is not closed with a case

`encodeCycle` checks seven arrays against the ring's shape — `desired`, `command`, `mask`,
`clamped`, `position`, `velocity`, `effort` — and **throws** when one does not match:

> `black box: desired holds 3 values, and the ring's shape says 2`

`tripped` is the eighth array in the same function and is **silently truncated** to eight instead.

A ninth guard is not something a producer emits, so the clamp is defensive rather than reachable.
But that is the whole difficulty: a corpus case carrying one would freeze *truncation* as the
specified answer for a malformed cycle, when the answer the other seven arrays give to the same
question is a refusal. The two are not obviously reconcilable in the encoder's favour either — a
record is written on the control path, and a throw there is a cycle that goes unrecorded.

So it is written down rather than settled by whichever behaviour was easier to transcribe. It is not
an edition disagreeing with another; it is one function answering the same question two ways, which
is the shape §7.4's redundant channel exists to catch, and the second instance of it this repository
has found in the reference (the first being `roundHalfAway`, defined twice in `kinematics.ts` and
`observation.ts`).

### Verified

* `python3 editions/python/run_corpus.py --min-passed 1012` — **1,012 passed, 0 failed**, 66 not
  implemented, of 1,078
* `cargo test -p skynet-corpus --test corpus` — 1,078 cases, byte for byte
* 21 of 22 mutations caught after the three new cases, up from 19; the survivor is the item above
* `cargo test --workspace` — **1,431 passed**, 0 failed, 7 ignored
* `python3 editions/python/self_check.py` — 41 checks; `tsc -p .` clean; the generator reproduces
  all 77 files
* The CI ratchet raised to 1,012

**Twenty-seven of thirty-one corpora complete. Four remain, 66 cases:** `calibration` 18,
`provenance` 17, `telemetry` 16, `live` 15.

---

## 29. `calibration` — nine of nineteen, and a function the corpus was measuring against itself

`calibration` is §45: whether a stored measurement is still believed, and which guards lose their
input when it is not. **18 cases, 1,012 → 1,030, first run.**

Then the mutation run, which is the **weakest of the project so far: nine of nineteen caught.** Ten
survivors, eight of them real. That number is not about this transcription — it is about how the
corpus was written, and the pattern is one sentence: every case sits comfortably inside or
comfortably outside its rule, and nothing sits **on** one.

**1,078 → 1,091.** Thirteen new cases; Rust reproduces every one without a change.

### `setsFor` was measured by nothing

`calibration/events` has two cases whose `applicable` field is written as `setsFor(…)` — the
generator calls the function and freezes its **output into the input**. So the function that decides
which parameter sets a body can hold, the one that keeps a bolted-down arm from reporting `balance`
unavailable for want of an IMU it will never have, had no case: two editions could disagree about it
and the corpus would be comparing them on an answer one of them had already supplied.

`corpus/README.md` diagnosed this shape three rounds before it happened:

> a corpus grows a blind spot wherever an *input* field is missing, not only where an
> implementation is.

This is that, pointed at a function rather than at a field, and it is worth distinguishing from an
ordinary gap: a missing case is visible to anyone reading the file list, and a case that *calls the
function it is meant to test* looks like coverage.

`corpus/calibration/sets.json` is seven cases against `setsFor` directly, and four reach branches
nothing had: `sensing.depth` alone, `sensing.force` alone, an IMU on a fixed base, and a declared
capability matching none of the sensing rules. `crates/skynet-corpus` gained the matching branch;
both editions agreed on all seven first time, which is the outcome to want — the function was
correct in both and simply unchecked.

### Four boundaries nobody had sat on

| New case | Why it matters |
|---|---|
| `a-residual-exactly-at-its-threshold` | 1.0 against 1.0 is **valid**. A method reporting exactly the error it declared acceptable has met its own standard, and `>=` fails every calibration that hit its target precisely |
| `an-age-exactly-at-the-limit` | 3,600 s against 3,600 s. On a fleet recalibrating on a schedule, `>` and `>=` disagree about **every robot on the day its policy comes due** — which is the day they are all checked |
| `a-robot-that-got-colder` | Every temperature case ran warmer. Without `Math.abs`, −18 °C reads as inside a 5 °C budget: a bias measured in a heated cell is trusted in a cold store, which is where it drifts most |
| `a-policy-with-no-timestamp` | §45's rule that a missing reading never extends validity, for the **taken** reading. `temperature-unreadable` pins it for the current one and nothing pinned the other half |

Plus `an-empty-method` — §45.3's check is falsy rather than a null test, and no record had carried
`""`, which is the shape of every form somebody tabbed past — and `an-age-on-a-rounding-tie`, at
half a second, where `toFixed` takes the larger and Python's formatting takes the even one.

### Two notes that claimed what their own inputs did not reach

`temperature-delta`'s note says *"the delta is the rule and the direction does not matter"* and its
input only goes one direction. `arm-without-an-imu`'s note says *"seven sets exist, four apply"* and
five apply.

That is the **third and fourth** instance this session of a note asserting a property its input does
not exercise, after `frame-astral-stream-name` and the `pending` ordering. It is a distinct failure
from a missing case and worth naming as one: nobody adds a case for a rule that already has a case
claiming to cover it, so these are the gaps with the longest half-life. The count is now the only
one of the four that was wrong as English rather than as coverage, and it is corrected.

### One consolidation, and it was a latent fix

`toFixed` existed **twice** in this edition. `certification.py` implemented it through `Decimal`;
`transport.py` implemented it as `floor(x + 0.5)`, with a comment arguing that its domain — a
percentage in [0, 100] — made the shortcut safe.

They are not the same rule. At `0.49999999999999994` the sum rounds up to exactly `1.0` in binary
and the shortcut answers `1`, where the nearest integer to the value itself is `0`. That is the
*identical input* on which `roundHalfAway` is defined twice and two ways in `reference/typescript`,
recorded in this repository since the fourth round — the same trap, caught the second time by
noticing the duplication rather than by hitting the value.

There is one `to_fixed` now, in `js.py`, on the `Decimal` rule. The corpus was green before and
after: nothing reachable moved, which is what makes this the right moment for it rather than the
morning after somebody finds the input.

### Verified

* `python3 editions/python/run_corpus.py --min-passed 1043` — **1,043 passed, 0 failed**, 48 not
  implemented, of 1,091
* `cargo test -p skynet-corpus --test corpus` — 1,091 cases, byte for byte, including the seven
  `sets` cases neither edition had been measured on
* 16 of 19 mutations caught, up from 9; of the three survivors one is a partial edit that is caught
  when applied to every call site, and two are equivalent by construction (no non-expired validity
  carries a `reason` or a `checked`)
* `cargo test --workspace` — **1,431 passed**, 0 failed, 7 ignored; fmt and clippy clean
* `python3 editions/python/self_check.py` — 41 checks; CI ratchet raised to 1,043

**Twenty-eight of thirty-one corpora complete. Three remain, 48 cases:** `provenance` 17,
`telemetry` 16, `live` 15.

---

## 30. `telemetry` — eight of nineteen, and nine boundaries

`telemetry` is §74: counts over a self-describing half-open window, and what a set of them covers.
**16 cases, 1,043 → 1,059, first run.**

Then **eight of nineteen** mutations caught, which is worse than the round before it. Nine new cases
later it is seventeen, and both remaining survivors are no-ops — one edits a line the `continue`
above it makes unreachable for the differing case, the other removes a guard `field_conformance`
repeats two lines down.

**1,091 → 1,100.** Rust reproduces all nine without a change.

### The nine, and the one sentence they share

| New case | The boundary |
|---|---|
| `a-clock-that-stepped-back-part-of-a-second` | Every skew in the corpus was a whole number of seconds, so `Math.trunc` and a floor agreed |
| `a-counter-that-went-backwards` | The `< 0` arm of a three-way test |
| `a-duration-that-is-negative` | `elapsed_ms: 0` had a case; a negative one did not |
| `a-window-of-no-wall-duration-at-all` | `to === from` is **not** inverted |
| `a-clock-corrected-by-one-millisecond` | And `to === from - 1` is |
| `a-skew-of-exactly-one-minute` | Strictly greater; every skew was zero or an hour |
| `a-skew-of-exactly-minus-one-minute` | And the same value the other way |
| `a-window-wholly-inside-another` | Every overlap ran forwards |
| `windows-reported-out-of-order` | Every case arrived sorted |

The first is the one the reference had already argued about. Its own comment says truncation was
chosen over rounding because *"`Math.round` is half toward positive infinity and the Rust edition's
helper is half away from zero, and the two disagree on exactly the negative halves a backwards clock
produces"* — and then no case carried a skew that was not a whole number of seconds, so the
disagreement it was written to prevent was invisible. −90.5 s truncates to −90 and floors to −91.

None of the nine is a Python trap and none is a defect in any edition. They are nine places where
two correct editions were being compared on inputs that could not tell them apart.

### The pattern across two rounds, which is the finding

`calibration` caught 9 of 19 and `telemetry` 8 of 19 — the two weakest runs of the project, back to
back — and **fifteen of the twenty-two cases added across them are boundaries**: a value exactly at
a threshold, a comparison's other side, a sort's other order, an absolute value's other sign.

That is a property of how this corpus was written rather than of these two modules. A case is
written to demonstrate a rule, so it is given an input that demonstrates it clearly — an age well
past its limit, a skew of exactly an hour, windows that tile in order. Every one of those choices is
the right one for a reader and the wrong one for a differential test, and the two purposes have been
pulling in opposite directions since the first case was written.

It is worth stating as a rule for the corpus rather than as an observation about two files: **a case
that demonstrates a rule and a case that pins it are different cases, and a corpus needs both.** The
first is what somebody reads to understand the specification; the second is what stops two editions
drifting. Fifteen of the twenty-two are the second kind, and none of them existed.

### Verified

* `python3 editions/python/run_corpus.py --min-passed 1068` — **1,068 passed, 0 failed**, 32 not
  implemented, of 1,100
* `cargo test -p skynet-corpus --test corpus` — 1,100 cases, byte for byte
* 17 of 19 mutations caught, up from 8; both survivors shown equivalent by construction
* `cargo test --workspace` — **1,431 passed**, 0 failed, 7 ignored
* `python3 editions/python/self_check.py` — 41 checks; CI ratchet raised to 1,068

**Twenty-nine of thirty-one corpora complete. Two remain, 32 cases:** `provenance` 17, `live` 15.

---

## 31. `provenance` — the first case to fail an edition for reading its input correctly

`provenance` is §56: what an on-robot evaluation result is worth, given who signed it. **17 cases,
1,068 → 1,084 on the first run, and one failure** — which is the first time a transcription in this
edition has gone red on a case rather than on a mutation of one.

### The case, and why it was the corpus rather than the edition

`provenance/scores :: two-robots-that-computed-nothing` carries an input of
`{"on_robot": null, "on_hub": null}` and an expectation computed from `NaN`:

> "The robot rolled this run up to **NaN** and the Hub to **NaN**…"

JSON cannot spell `NaN`, and the generator writes every input through `JSON.stringify`. The frozen
input is therefore a different document from the one its expectation was computed against — which is
precisely the hazard `tools/corpus-gen/generate.ts` names, at the frame-bindings rule, and **refuses
to create there**:

> The non-finite coefficient rule has **no case here, and cannot have one.** This generator writes
> every input through `JSON.stringify`, which renders infinity as `null`, so the frozen input would
> be a *different* body from the one the expectation was computed from — worse than no case at all,
> because it would fail an edition for reading the file correctly.

That sentence was written about a rule with no case. This is the same shape with a case, and it did
exactly what the sentence predicts.

### It is a real convention, stated in one adapter and applied silently in another

`crates/skynet-corpus/src/blackbox.rs` says it in its own header — *"`null` is how the oracle spells
NaN, which is how §24 spells 'this is not measured'"* — and the provenance dispatch does the same
thing with `unwrap_or(f64::NAN)` and no comment. So the two editions agreed, the convention was
never written where a reader of the corpus would find it, and a third edition rediscovered it from a
failing case.

The reading is **forced rather than chosen**, and that is what makes the convention sound: the field
is typed `number` in the reference, not `number | null`, so no caller could have supplied a null and
the only thing it can be is a value the format cannot write.

It is also emphatically **not a blanket rule**, which is why a `?? NaN` on every null would be
wrong:

* the reference's type **admits** null — `Frame.worstSkewUs: number | null`,
  `CalibrationRecord.method?: string | null` — and the null is the value;
* the reference's type is a **bare `number`** — `scoresAgree(onRobot: number, onHub: number)`,
  `Cycle.basePose: number[]` — and the null is a non-finite value.

An edition picking one reading for all nulls is wrong on half of them in opposite directions:
reading the first kind as NaN discards a declared absence, and reading the second kind as an absence
answers a different question from the one the expectation was computed against.

`corpus/README.md` carries this now, in the section describing the format, with both halves and the
generator's refusal quoted beside them. It is a property of the format and belonged there rather
than in one adapter's doc comment.

### Then the mutation run: ten of seventeen, and four more cases

| New case | The gap |
|---|---|
| `a-segment-recorded-as-empty` | `""` is a segment somebody wrote down as empty, not a missing one. §56.4's check is against null and undefined and nothing else, and a falsy test reports `not_replayable` against a run whose recording exists |
| `a-simulator-whose-birth-certificate-attests-nothing` | Every simulator case was otherwise perfect, so the branch that **skips** §50's two checks for a declared simulator was doing nothing observable. An edition running them anyway puts `birth_unattested` in front of a publisher who cannot act on it |
| `fewer-seeds-than-the-benchmark-supplies` | Every seed case was three against three, so `seeds_not_supplied`'s two counts were equal and an edition reporting them the wrong way round said the same sentence — while the ordering rule, *expected then supplied*, is the reason they are a pair |
| `a-difference-of-exactly-the-tolerance` | `1e-9` apart is **agreement**, because the comparison is `<=`. Nothing sat on the boundary, so `<` and `<=` were the same function |

**1,100 → 1,104.** Rust reproduces all four without a change.

And `a-difference-just-outside-the-tolerance`'s note read *"The boundary, frozen"* — at `1e-8`, which
is ten times the tolerance. That is the **fifth** instance this session of a note claiming a property
its own input does not reach, after `frame-astral-stream-name`, `temperature-delta`, the `pending`
ordering and `arm-without-an-imu`'s count. The note now says what the case is, and the boundary has
a case of its own.

Fifteen of seventeen mutations caught, up from ten. Both survivors are no-ops: `device_unknown`
already blocks `field` through the reasons list, so dropping `device_known` from the attribution
test changes nothing, and `abs(NaN − NaN) <= 1e-9` is false however the guard above it is written.

### Verified

* `python3 editions/python/run_corpus.py --min-passed 1089` — **1,089 passed, 0 failed**, 15 not
  implemented, of 1,104
* `cargo test -p skynet-corpus --test corpus` — 1,104 cases, byte for byte
* `cargo test --workspace` — **1,431 passed**, 0 failed, 7 ignored
* `python3 editions/python/self_check.py` — 41 checks; CI ratchet raised to 1,089

**Thirty of thirty-one corpora complete. One remains, 15 cases:** `live`.

---

## 32. `live`, and a corpus that tested one side of its own rule — the edition is complete

`live` is §54.2's precondition context: the seven branches a skill's preconditions may read, and the
rule that decides every field in them. **15 cases, 1,089 → 1,104, first run.**

**That is 31 of 31 corpora and 1,104 of 1,104 cases.** With the five cases this round then added,
the Python edition reproduces **1,109 of 1,109** and the runner's *not implemented* line reads zero
for the first time since it was written.

### Thirteen of twenty, and the seven survivors were one shape

`live.ts`'s header states the rule the whole file exists for, in bold, as its first section:

> **The rule that decides every field: absent is not null.** A fact this robot does not know must be
> **absent** from the context, never present as `null` and never present as a plausible zero.

**Zero of the fifteen cases carried an explicit null.** Every optional field in every case was either
missing entirely or held a real value — so an edition that published `null` for a
declared-and-unmeasured gripper reproduced all fifteen and broke the rule the module is named for.

That is the sharpest instance of this session's pattern, and it is worth separating from the others:
the earlier gaps were boundaries nobody had sat on, which is an oversight. This is a corpus for a
module about a **distinction** that exercises only one side of it, which is closer to a category
error — and it survived because the side it tests is the side a reader needs to see.

`declared-and-not-measured` carries four at once: a gripper the body has and cannot read, a battery
sensor that answered nothing, a pose the driver did not report, a state nobody observed. Every one
omits its path, exactly as an absent key does — and that identity is the case. An edition publishing
`null` gives a predicate something to compare against, and `robot.state.battery_percent > 20` is
**false** on a null and **unresolvable** on an absence. Only the second refuses the skill, which is
§54.2's first rule in one comparison.

Four more, each a branch nothing had reached: an `age_s` declared and null; a tier of `''`, which
**is** published because the test is `typeof === 'string'` and an empty string is a value somebody
supplied; a joint with no name at all, beside the dotted one that had a case; and a frame fault
whose subjects arrive unsorted and duplicated, where the one existing fault case carries a single
subject and the sort was therefore free.

**Eighteen of twenty caught.** The last two are one trap and it cannot be written: `battery_percent`
and `age_s` are typed `number | null` and tested with `Number.isFinite`, so the two readings differ
only on a **non-finite number** — which `JSON.stringify` writes as `null`, and which
`corpus/README.md` now says means *null* in a field whose type admits one. Three checks in
`self_check.py` instead, both directions, and the reason they earn their place: a battery published
as `NaN` makes every comparison against it false, so a robot whose sensor has failed reads as *"the
battery is low"* rather than producing a refusal.

### §73 needs an amendment this session cannot make

§73's last column has said this against the Languages row since the table was written:

> **Built** — one edition, held to a corpus of 77 files and 1,055 cases | *The second edition, which
> is what the corpus is for*

That entry is now satisfied at **L2**. §73 is a **mirrored** document — `docs/README.md`: *"A shared
document is edited in both repositories or in neither. It never diverges."* — and this session was
instructed not to touch the Hub repository, so the edit is not made rather than made on one side.

The amendment, when somebody makes it in both: the row's last column becomes L3 and L4, which is
what is actually missing, and §42 is already explicit that neither is planned and that *almost
everyone asking for their language wants L1*. The mirror pin stays where it is until then, and
`check-mirrors.mjs` stays green because nothing diverged.

### What the seven rounds produced, separated from the transcription

The transcription was the cheap half. Seven modules passed their corpora on the first run and not
one mutation run found a defect in this edition. What the rounds found was in the corpus and in the
other two editions:

| | |
|---|---|
| Cases added | **38** (1,071 → 1,109) |
| Defects fixed in the Rust edition | **1** — a digest published from a slot nobody read |
| Defects found in this edition | **0** |
| Case notes claiming what their own input does not reach | **5** |
| Functions no case measured | **1** — `setsFor`, whose output was frozen as an input |
| Format conventions undocumented | **1** — `null` in a numeric field, now in `corpus/README.md` |
| Duplicated implementations of one rule | **2** — `toFixed`, and the JS helpers into `js.py` |
| CI jobs this edition was in | **0 → 1**, with a ratchet |

Thirty-one of the thirty-eight cases are boundaries: a value exactly at a threshold, a comparison's
other side, a sort's other order, an absence's other spelling. Seven are coverage of a function
nothing measured. None of the thirty-eight is a Python trap, and none pins behaviour that was wrong
in any edition — every one pins behaviour that was already right in both and protected in neither.

### Verified

* `python3 editions/python/run_corpus.py --min-passed 1109` — **1,109 passed, 0 failed, 0 not
  implemented**, 31 of 31 corpora complete
* `cargo test -p skynet-corpus --test corpus` — 1,109 cases, byte for byte
* `cargo test --workspace` — **1,431 passed**, 0 failed, 7 ignored; fmt and clippy clean
* `python3 editions/python/self_check.py` — **44 checks**, three of them new, both new-trap
  mutations confirmed red
* 18 of 20 `live` mutations caught by the corpus; the two survivors are the `self_check.py` trap,
  confirmed red on both
* `check-golden-path.mjs`, `check-mirrors.mjs`, `check-contract-mirrors.mjs` all clean
* CI ratchet raised to 1,109

---

## 33. §80's retention of sealed windows — decided 2026-09-04, built

The §80 row read *"decided 2026-09-04, **unbuilt**"*, and `grep -rn "reclaim\|tombstone" crates/`
returned nothing. §79.4 says plainly what the state was:

> **The sealed file is not deleted.** … Retention of sealed windows is a real question and this
> amendment does not answer it; it is listed in §80 rather than settled in passing.

`crates/skynetd-io/src/retention.rs` is that answer, and the owner's rule went in unchanged: a
window an incident names is never reclaimed; a routine window may be reclaimed only when a receipt
exists, the receipt's digest **is** the window's digest, and the disk is under the floor, oldest
first; a reclaim writes a tombstone.

### The measurement it waits on, and why that is a parameter rather than a blocker

**Nothing on this robot measures free disk.** `crate::checkin` says so in its own header — §33.6's
`storage_headroom` *"needs a `statvfs` nothing here calls"* — and `capacity` and `api` repeat it.
So the third condition of §80's rule has no input.

That did not stop the item, and the shape it took is the repository's own: `plan()` takes
`free_bytes` and `floor_bytes` as arguments, for §45's reason and the kernel's — *the clock and the
conditions are supplied*. The policy is decided in a test rather than on a machine whose disk
happens to be full, and the missing measurement is a `Free::Unmeasured` the caller passes today.

The syscall was deliberately **not** written here. `skynetd-io` carries `#![forbid(unsafe_code)]`,
and this workspace's precedent for a necessary `unsafe` is a crate of its own with the argument in
its header — `skynet-dl` exists exactly because *"the loader `unsafe`, extracted so it has one home
and two callers"*. Putting a hand-rolled `statvfs` binding into the daemon would spend that property
on a status field, and a `struct statvfs` whose layout differs between glibc and musl is not a thing
to get wrong quietly. It is now a gap that blocks **two** named things instead of one, which is a
better place for it to sit than in a doc comment.

### Three conditions, tested against each other

The rule is a conjunction, and every interesting failure is one term being sufficient alone. A suite
that checked *"a cited window is kept"* and *"an uncollected window is kept"* separately would pass
against an implementation that reclaimed whenever **either** held — so the fourteen tests are
arranged as interactions:

* the receipt alone and the floor alone, in one test, because either alone is the bug;
* a cited window at zero free disk, where freeing it is the only way to reach the floor, and it
  stays — the pass reports it **could not** reach the floor rather than reaching it;
* a cited window that is also the **oldest**, so an implementation stopping at the first ineligible
  candidate rather than skipping it frees nothing;
* the floor boundary, where exactly at the floor is at it.

### The citation is recorded, not inferred

Every incident this daemon raises seals with `Reason::Fault`, so `reason == "fault"` answers *"does
an incident name this window"* today. It answers it by **inference**, and it is one new incident
path away from being wrong: a raise that sealed for `Estop` or `Operator` would produce a window an
incident names and the reason does not, and the reclaim would free it. Silent, irreversible, and
landing on exactly the window somebody is about to ask about.

So `runtime.rs` writes `<name>.mcap.cited.json` at the raise, by the process that just put the
window's name into the incident, and the citation carries the **digest**: §13 names a window for a
monotonic instant and a reboot restarts that clock, so a citation believed on its filename would
protect whichever window took the name next and leave the one it was about looking routine.

### The scan hashes, and that is the expensive decision on purpose

`upload::sealed` reads manifests, because it is cheap enough for a status route and nothing it feeds
destroys anything. `retention::scan` hashes every window, because the manifest's own doc comment
says what it is — *"a cache in the sense that it can be rebuilt, and it is not a second source of
truth"* — and a window deleted because a sidecar carried another window's digest is the worst
outcome this module can produce. There is a test for exactly that: a manifest and a receipt that
agree with each other and not with the file, and the window survives.

It costs one pass over a directory that is only read when the disk is already under the floor, and
`run` refuses to scan at all without a measurement — so the state the daemon is in today costs one
`matches!`.

### Tombstone first, then the bytes

A crash between the two leaves a tombstone for a window that still exists: visible, recoverable, and
wrong in the direction that loses nothing. The reverse leaves freed bytes nothing accounts for,
which is the state §80 exists to make impossible. It is a `?` rather than best effort, and a test
forces the failure by putting a directory where the log wants a file.

`reclaimed.jsonl` is **not** the audit log, and the reason is the subject rather than the shape.
§38.3's log is *"small, append-only, and kept for the life of the machine"* — right in every
respect — and every entry carries an actor and a previous value. A reclaim has neither: it is not a
decision somebody made, it is a consequence of a disk. Writing one there would put a digest into a
`detail` string, where it is prose rather than a field, which is the mistake §10's attestation spent
a session undoing.

### The mutation run found a rule written twice

Twelve mutations, eight caught. Two of the four survivors were the same finding: `plan` had an early
return for a disk already above the floor, and the loop's first test is `free + freed >= floor`,
which on the first iteration is exactly `free >= floor`. The early exit produced an identical plan
through a **second copy of the rule**, and breaking the copy broke no test. It is removed, the floor
is expressed once, and the boundary test now reaches the surviving copy.

The other two were a missing ordering test — `read_dir` has no order worth relying on, and the
fixture happened to create files in sorted order — and a bad mutation of mine that left the `?` in
place. **Eleven of eleven caught** after the two new tests.

### What is on the board that was not

`GET /blackbox` carries a `retention` block beside §79's `collection`, because they are two halves
of one question: whether the Hub has a window, and whether this robot still does. It reports
`free_bytes: null` with a sentence saying why, which is §33.6's treatment one level up — a route
that omitted the field would leave an operator unable to tell a robot with plenty of room from one
whose disk nothing can see. It does **not** scan.

`DEFAULT_FLOOR_BYTES` is one gigabyte and is a placeholder with the reasoning attached rather than a
decided number: enough for a preallocated ring plus an OTA slot on the smallest machine §28's table
admits. It sits in code rather than in §38's configuration deliberately — a floor an operator can
lower to zero is a floor that authorises deleting everything, and that is the owner's decision.

### What this leaves open, precisely

| Open | Blocked on |
|---|---|
| A `statvfs`, and with it §33.6's `storage_headroom` and §80's third condition | A crate to hold the `unsafe`, and the struct layout across libcs |
| The floor as a number | The owner. One gigabyte is a placeholder that says so |

§80's row can move from *"decided, unbuilt"* to *"built, waiting on a disk measurement"* — but §80
is in `docs/skynet-os-architecture.md`, which is **mirrored**, and this session was told not to
touch the Hub repository. The edit is not made rather than made on one side, and this is the note
that says so.

### Verified

* `cargo test --workspace` — **1,445 passed**, 0 failed, 7 ignored, up 14
* 11 of 11 mutations of the policy caught, after two tests the first run asked for
* `cargo fmt --check` and `cargo clippy --workspace --all-targets` clean
* `python3 editions/python/run_corpus.py --min-passed 1109` — 1,109 of 1,109, unchanged
* `check-golden-path.mjs`, `check-mirrors.mjs`, `check-contract-mirrors.mjs` clean
* Nothing in the Hub repository was touched; `git status` there is empty

---

## 34. §26.2's multi-writer ring: the design pass, and a conflict between two arguments in this repository

§80 lists *"a ring two processes can write"* and §26.2 names the fix:

> **The fix, and it is open.** The sequence belongs in a mapped header as an atomic, and writers
> claim a slot by fetch-add — which is what `skynet-ring-shm` already does for §25's command ring,
> in the same repository, for the same reason. **It wants a design pass** and a Linux mapping.

This is that design pass, and the first thing it found is that the repository already argues the
other way. `Ring::open`, in `crates/skynet-blackbox/src/ring.rs`:

> Recovery is a scan, and it is a scan on purpose. **The alternative is a "next sequence" field in
> the header, updated on every append** — which turns one positioned write per record into two, and
> makes the header the thing most likely to be torn by a power cut. The scan is `O(capacity)`,
> happens once at start, and is the only way to be right after a crash: the highest sequence number
> actually on disk is the truth, whatever any counter claims.

§26.2 proposes, by name, the thing `Ring::open` rejects by name. Neither cites the other, and both
are right — which is the shape of a divergence rather than a disagreement, and it is the second one
this session has found between two correct arguments in one codebase.

### They are two different counters, and saying so resolves it

* `Ring::open`'s subject is **crash recovery**. After a power cut a counter is ahead of or behind
  the disk and there is no way to tell which, so the disk is the truth.
* §26.2's subject is **concurrent claiming**. Two live writers must agree about who gets which slot,
  and two in-memory counters cannot.

A claim counter that is *initialised from the scan* and never consulted after a restart satisfies
both: recovery stays a scan, so `Ring::open`'s rule is untouched, and the claim is a fetch-add, so
§26.2's property holds while both writers are up.

### Which settles the placement, and it is not the header

Three consequences fall out, and each removes a reason this item looked expensive:

1. **Not in the CRC'd header.** `encode_header` computes a CRC over the whole header; a field
   mutated on every append invalidates it immediately, and `Ring::open`'s "torn by a power cut"
   objection lands squarely on it.
2. **Not durable at all.** It is rebuilt from the scan at every open, so it need not survive a
   restart — which means it is not part of the on-disk format.
3. **Therefore the format does not change.** `FORMAT_VERSION` stays 2, `corpus/blackbox/header`
   stays as it is, and neither edition moves. §31.2 lists the black box format as single-source
   across editions, and this was the reason the item read as a format break.

The shape is a small `MAP_SHARED` mapping beside the ring holding one `AtomicU64`, created by
whichever writer opens first, seeded from `Ring::next_sequence()`, and claimed by `fetch_add`. The
directory lock stays and changes meaning: from *one writer* to *one creator of the claim mapping*.

### What that leaves, stated as a cost rather than as a plan

| | |
|---|---|
| A fourth `unsafe` crate | `skynet-ring-shm`'s header says it is *"the only `unsafe` in the workspace outside the ABI and the loader"*, and CI asserts every other crate is safe. A file mapping is a fourth, and it needs the same treatment: as small as a crate can be, with the argument in its header |
| A specification amendment | §26.2's last paragraph and §80's row both describe an open item whose fix is *"in a mapped header"*. Both are in `docs/skynet-os-architecture.md`, which is **mirrored** |
| A tear question the command ring does not have | `skynet-ring` runs a seqlock because a *reader* may race a writer mid-slot. The black box has no concurrent reader — sealing takes the lock — so a claimed slot is written by exactly one writer and read by nobody until the process that claimed it has finished. That is a smaller problem than §25's and worth stating so nobody imports the seqlock with the mapping |

### Not built here, and why that is the honest stopping point

The design is settled and the code is not, for one reason that is not about effort: the amendment
this makes to §26.2 and §80 belongs in `docs/skynet-os-architecture.md`, and that file is edited **in
both repositories or in neither** (`docs/README.md`). This session was told not to touch the Hub
repository, so building the mechanism while leaving the specification saying *"the sequence belongs
in a mapped header"* would ship code that contradicts the document on the one point the document is
currently open about — which is worse than the interim state §26.2 already calls safe rather than
merely tolerable.

The interim remains what it was: `skynet-lock` refuses the second writer by name, `skynetd` starts
without a recorder and says so, `skynetk` refuses to run. Failing loudly beats corrupting evidence.

**What a session that can touch both repositories should do**, in order: amend §26.2's last
paragraph and §80's row to the counter described above, mirror and re-pin, then write the mapping
crate and move the lock's meaning. The corpus and both editions are not involved, which is the part
of this that was not previously known.

---

## 35. §26.2 amended in both repositories, `skynet-claim` written, and the allow-list that had never run

The owner authorised editing the mirrored specification, so §34's design pass became an amendment
rather than a note. Three things happened; the third was not planned.

### The amendment, in both copies

§26.2's closing paragraph said *"the sequence belongs in a mapped header as an atomic"* and *"it
wants a design pass"*. It now carries the design pass instead: the two arguments that conflicted
(§26.2's and `Ring::open`'s), why both hold, and the conclusion that the counter is **seeded from the
scan and never consulted after a restart** — which puts it outside the header, outside the CRC, and
outside the on-disk format entirely.

The consequence worth repeating is the one that shrank the item: **`FORMAT_VERSION` stays at 2, the
conformance corpus is untouched, and no language edition moves.** §31.2 keeps the black box format
single-source across editions precisely so a change here is expensive, and this is not one.

§80's row moves from an open question to *"designed 2026-09-05"*, naming the placement the design
pass overturned so the next reader does not re-derive it.

Edited in `Skynet`, copied to `Robot Overflow`, byte-equality verified, and re-pinned with
`--pin` — which is the rule `docs/README.md` states and the first time this session has exercised
the whole of it.

### `crates/skynet-claim` — written and tested, deliberately not wired

Five `extern "C"` declarations, one `mmap`, one `AtomicU64::from_ptr`, and six tests. It is the
**fourth** `unsafe` crate in the workspace and takes the same treatment as the third: as small as a
crate can be, with the argument in its header, so a reviewer asking whether this is memory-safe
reads one short file.

The tests are arranged around the property rather than the API. `two_mappings_of_one_file_never_
hand_out_the_same_number` is §26.2 in one assertion — two `Claim`s are two independent mappings, the
same relationship two *processes* have, and a counter held per-mapping would give each of them
1, 2, 3 and encode two records into one slot. `threads_racing_the_counter_take_every_number_exactly_
once` runs sixteen claimants over four mappings and asserts the union is exactly `1..=1000`: unique
**and** dense, because a black box's numbering has no holes it did not record as gaps.

One test exists to make a hazard meet the reader rather than be inferred:
`seeding_again_resets_it_which_is_why_the_caller_holds_the_lock`. `Claim::seed` is unconditional —
it has to be, because a file left by a previous run holds a sequence that was true for a ring
`Recorder::open` may since have recreated at a different shape — so a second seed while a writer is
claiming resets the counter under it. That is exactly why §26.2 narrows the directory lock's meaning
to *one creator*, and the test is where somebody meets it.

**What is not done**, precisely: nothing calls it. `skynet-lock` still refuses the second writer
rather than serialising creation and letting both attach, and neither `skynetd` nor
`skynet-kernel-host` claims through it. The interim §26.2 describes is therefore unchanged and still
safe — the daemon starts without a recorder and says so, `skynetk` refuses to run. Stopped here on
request, at a crate boundary rather than mid-wiring.

### And the check that had never run

Adding a crate to the workspace ran the `unsafe` allow-list check locally for the first time, and it
was **already failing on three crates that were not mine**:

| Crate | State |
|---|---|
| `skynet-capture` | No `unsafe` anywhere, no `forbid(unsafe_code)` |
| `skynet-corpus` | The same |
| `skynet-dl` | **27 `unsafe` occurrences**, and not on the allow-list |

The check's own comment explains why this is the interesting kind of failure. It was rewritten once
already, and the rewrite's stated purpose was to make it enumerate the directory rather than a list:

> The previous version listed the crates that must forbid it, which is the same check with the
> default pointing the wrong way: a new crate joined the workspace unwatched.

The default now points the right way and three crates still joined unwatched — because **nothing in
this repository has been pushed**, so the workflow has never executed. A check whose logic is
correct and whose runs are zero is worth exactly as much as the one it replaced, and that is the
finding rather than the three crates.

Fixed: the two with no `unsafe` gained the attribute, and `skynet-dl` joined the allow-list, where
§73's own words already put it — *"the loader `unsafe`, extracted so it has one home and two
callers"*. The allow-list is five now, each with its reason written beside it, and `skynet-dl`'s
records that it arrived late and why nothing caught it.

`skynet-ring-shm`'s header said it was *"the only `unsafe` in the workspace outside the ABI and the
loader"*. It is not, since this round; the sentence now names both mappings.

### Verified

* `cargo test --workspace` — **1,451 passed**, 0 failed, 7 ignored, up 6
* The allow-list check's own shell reproduced locally: passes, for the first time
* `cargo fmt --check` and `cargo clippy --workspace --all-targets` clean
* `python3 editions/python/run_corpus.py --min-passed 1109` — 1,109 of 1,109, unchanged
* `python3 editions/python/self_check.py` — 44 checks
* `check-mirrors.mjs` — all shared documents match, re-pinned; `check-contract-mirrors.mjs` and
  `check-golden-path.mjs` clean

---

# Round three — 2026-09-05

## 36. The audit, and a suite that was red for a file nobody deleted on purpose

Asked for a list of what is left before writing any code. Both plans read end to end, both trees
checked against them rather than against their own logs.

### What the logs had right, and the two things they did not

The *Still not built* tables are accurate on the Hub and one round stale on the OS. Two corrections:

* **`/skynet/os/editions` is no longer blocked.** It has been deferred nine times with one reason —
  *"still one edition, and a page listing the matrix would be listing editions that do not exist"* —
  and §32 of round two finished the Python edition at 1,109 of 1,109 cases. The reason expired and
  nothing noticed, because the deferral is written in the table rather than derived from the tree.
* **§73's table still says "one edition"**, and its test and crate counts are a round behind.

### The Hub's suite was red, again, and for the same class of reason

`supabase/schema/skynet.sql` — 6,712 lines, committed in the twenty-first session precisely because
its absence had made the suite red while the log reported it green — was **deleted in the working
tree**. `lib/__tests__/skynet-export-schema.test.ts` fails on it: not a test that reads the file for
content, one that refuses to run at all without it.

Restored from `HEAD`. 26 checks pass. Nothing else in the tree was touched, and the working tree's
other 148 modified files are the owner's marketplace work, unrelated to Skynet.

### Five tests that have never executed, and the machine that could run them

`skynet-claim` was written last session with six tests. **Five of them are `#[cfg(target_os =
"linux")]`**, and every `cargo test --workspace` this repository has ever logged ran on Windows:

```
cargo test -p skynet-claim          →  1 passed
```

That one is `the_path_is_beside_the_ring`. The five that establish the crate's actual properties —
that two mappings never hand out the same number, that sixteen threads over four mappings take
`1..=1000` exactly once, that a second seed resets the counter under a writer — had produced no
evidence of any kind. The CI job that would have run them has never executed either, for the reason
§35 already found: nothing in this repository has been pushed.

There is a WSL Ubuntu 24.04 on this machine with cargo 1.96.1 in it, which nothing has used.

| `cargo test --workspace` | passed | failed | ignored |
|---|---|---|---|
| on Windows | 1,413 | 0 | 7 |
| under WSL Linux | **1,451** | 0 | 7 |

**Thirty-eight tests in this workspace had never executed anywhere**, and they are not evenly spread:

| Crate | Windows | Linux | What only Linux runs |
|---|---|---|---|
| `skynet-claim` | 1 | **6** | Every property of §26.2's counter |
| `skynet-ring-shm` | 3 | **10** | §25's `memfd` mapping |
| `skynetd-io` | 537 | **563** | Unix sockets (§33.7's whole control path), symlinks (§37's A/B swap), cgroups, `/sys` |

That is the platform the OS targets, and the two crates carrying this workspace's memory-safety
argument had been tested with their subject compiled out. The previous session's logged figure of
1,451 was right about the number and wrong about where it came from: it is the Linux number, and no
Linux run had happened.

Nothing here is a defect — every one of the 38 passes. The finding is that a repository whose
`unsafe` allow-list check had also never run (§35) was reporting a test count for a platform it was
not tested on.

---

## 37. §26.2 wired, and the asymmetry the design pass could not have found

§35 left `skynet-claim` written, tested and **called by nothing**, at a crate boundary rather than
mid-wiring. This is the wiring, and it turned one open item into a smaller one.

### One join, in one crate, because there are two callers

`skynet-lock::writership` is the narrowing §26.2 specified: the directory lock stops meaning *one
writer* and starts meaning *one creator*. `Writership::join` returns a **role** —

* **Creator** — took the lock, creates or opens the ring, seeds the counter from `Ring::open`'s
  scan, holds the lock for its life, and is the only party that may seal.
* **Attacher** — maps the counter the creator seeded, claims sequences from it, and **never creates
  a ring**.

That last clause is the one that needed care. Both recorders already sealed-and-replaced a ring
whose shape did not match the body, which is right for a robot whose descriptor changed between
boots and catastrophic for a process that arrived second: it would seal and delete a window the
other writer was still filling.

`Ring::stage_claimed` is the append that takes a sequence from outside. It raises the ring's own
counter and never lowers it, so `next_sequence()` stays a floor rather than becoming a claim about
what the other writer has taken — and a recorder with a claim reports the counter's value instead,
because that is the only number that knows.

### The asymmetry, which is the finding

The two writers cannot take the same rule about shape, and the reason is arithmetic rather than
policy. A record's size is a function of the joint and channel counts:

| | Writes | Attaching to a ring of another shape |
|---|---|---|
| `skynet-kernel-host` | cycles — a value per joint, per channel | **refused**, by name, with both shapes in the message |
| `skynetd` | events and gaps — neither | **accepted**; it writes into whatever the creator sized |

That asymmetry is what makes one recording possible at all on a machine where the loop knows the
body and the supervisor does not. It is not visible from the design: §26.2 is a document about
sequence numbers, and the shape question only appears when two recorders are pointed at one
directory.

### A zero is not a sequence

`offset_of` is `(sequence - 1) % capacity`. A counter read before its creator seeded it hands out
zero, which underflows to the ring's **last slot** — on top of the oldest record, silently, in a
release build. `RingError::Unseeded` refuses it, and an attacher waits a bounded two seconds for the
seed rather than writing at zero: a creator preallocating 64 MiB has begun a step it will finish,
and one that never seeds is named in the refusal along with the fact that its lock file is safe to
delete.

### What this leaves, written into §80 rather than around

On a robot `skynetd` starts first and opens a ring shaped for **zero** joints and zero channels,
so a control loop starting afterwards is refused: its cycles do not fit. The ordering that works is
the loop first; the ordering a robot boots in is the daemon first. Closing it means the daemon
sizing its ring for the body — a joint count it has and a channel count it does not, because
channels belong to a policy runtime that is unwritten. §26.3 says so and §80 carries it as a new
row, replacing the one this session closed.

### The check that had never run, again, and this time it could not have passed

§35 fixed the `unsafe` allow-list and reported its shell reproduced locally. Running it after adding
a crate showed it **fails on `skynet-cli`**:

```
grep: crates/skynet-cli/src/lib.rs: No such file or directory
NO unsafe in allow-listed skynet-cli
```

`skynet-cli` is a binary-only crate. Its root is `src/main.rs`, its argument for `unsafe` is in that
file's header, and the check looks for `lib.rs` — so it reported the CLI as an allow-listed crate
containing no `unsafe` when what was true is that it was reading a file that does not exist. A check
that cannot pass is worth exactly what one that cannot fail is. Both loops now resolve a crate's
root as `lib.rs` **or** `main.rs`, and the whole check passes locally.

### Verified

| | Before | After |
|---|---|---|
| `cargo test --workspace`, Windows | 1,413 | **1,421** |
| `cargo test --workspace`, WSL Linux | 1,451 | **1,462** |

* `python3 editions/python/run_corpus.py` — **1,109 of 1,109**, unchanged: `FORMAT_VERSION` stays 2
  and no language edition moves, which is what §26.2's design pass predicted and this confirms
* `python3 editions/python/self_check.py` — 44 checks
* `cargo fmt --check` and `cargo clippy --workspace --all-targets` clean — the clippy run found two
  dead-code warnings in `skynet-claim`'s Windows build, where a test helper for five Linux-only
  tests is compiled for nobody; gated rather than allowed
* `check-mirrors.mjs --pin` — §26.3 and §80 edited in both repositories, byte-equal, re-pinned;
  `check-contract-mirrors.mjs` and `check-golden-path.mjs` clean
* The `unsafe` allow-list check's own shell — passes, and for the first time it is a run that could
  have failed

### Two processes, which is what the mapping is for

Everything above is two `Writership`s inside one process, and that is a weaker claim than it looks:
two mappings in one address space could be satisfied by anything the two happen to share. §26.2 is
about `skynetd` and `skynet-kernel-host`, which share nothing but a file.

`two_processes_over_one_directory_take_every_number_once` re-executes the test binary as a child
with an environment variable that makes it the attacher, and claims 400 numbers in each **while the
other is running**. The union is `1..=800`, each exactly once. It failed on its first run for a
reason worth keeping: the child is a test binary, prints a harness summary of its own, and an
unmarked parse read numbers out of *"8 filtered out"*.

That is the first time §26.2's property has been shown across a process boundary. What still has not
happened is the two real binaries — `skynetd` and `skynetk` — against one black box directory, which
needs the shape question above answered first.

### And the seven tests marked `ignored`

`cargo test --workspace -- --ignored` under WSL: **7 passed, 0 failed**. They are the ABI and driver
tests that need a C compiler — *"run explicitly with --ignored (the abi CI job does)"* — and the abi
CI job has never run either, for §35's reason. A fresh C driver scaffold builds, loads, and reaches
guest tier; the loop turns against it; the jerk ceiling reaches the rate-limit guard; a channel list
that disagrees with the body is refused.

---

## 38. One syscall nothing called, and the three fields that were waiting for it

§33 built §80's retention and stopped at the same wall the check-in had been standing at since it
was written: **nothing on this robot measured free disk.** `crate::checkin`'s header said so in one
sentence — *"needs a `statvfs` nothing here calls"* — `capacity` repeated it, and `retention`'s
third condition, *the disk is under the floor*, was a parameter no caller could supply. Three
features, one absent measurement, and §33 filed it as *"a gap that blocks two named things instead
of one, which is a better place for it to sit than in a doc comment"*.

`crates/skynet-statfs` is the measurement. It is the **fifth** `unsafe` crate and takes the
treatment the other four take: one question, one call, the argument in the header.

### `statvfs` was the obvious call and is the wrong one

`struct statvfs` is **libc's**, and its layout is glibc's or musl's rather than the kernel's. §33
named that hazard while declining to write the syscall — *"a `struct statvfs` whose layout differs
between glibc and musl is not a thing to get wrong quietly"* — and the way past it is `statfs`,
whose structure belongs to the kernel and is, on 64-bit Linux, fifteen `__kernel_long_t` in a row:
the same 120 bytes on x86-64 and on aarch64, which takes it from `asm-generic`.

So there is **no `struct` in this crate at all**. The buffer is `[u64; 15]` and the fields are named
indices into it. A layout that is a struct definition can be subtly wrong; a layout that is an index
is one a reviewer counts.

### Checked rather than trusted, because of what the number authorises

Every reading is validated before it is returned: the block size is a power of two between 512 and
1 MiB, the total is non-zero, the available count does not exceed the total. A reading that fails
any of them is `SpaceError::Implausible`, and every caller turns that into **unmeasured**.

That matters more here than it would anywhere else in this repository. This number decides whether a
**sealed recording may be deleted**. The failure to design against is not "no answer" — it is a
confident wrong one, and an offset one field out gives exactly that: a block size that is really an
inode count, or an available figure larger than the disk.

`plausible()` is public, and the test exercises *it* rather than a restatement of it. The first
draft of that test had its own copy of the rule, which is a check that passes whatever either half
says.

### Verified against `df`, which is how an FFI reading should be checked once

```
statfs: 1006768979968 of 1081101176832 bytes available (93.1% free)
df -B1:  1006771200000 of 1081101176832
```

The **total agrees to the byte** — that is the number that says the structure is being read at the
right offsets. The available counts differ by about two megabytes, because the two readings were
seconds apart on a live filesystem.

### What now has a source

| Field | Was | Is |
|---|---|---|
| §33.6 `storage_headroom` | absent at every check-in ever made | the fraction free on the black box's volume |
| §80's floor | `Free::Unmeasured`, so nothing was ever reclaimed | a real byte count, or unmeasured — never zero |
| §8's `disk_mb` in capacity | `null`, *"needs statvfs"* | megabytes available where packages install |

`disk_mb` changes what an admission decision **means**, and the crate header says so rather than
leaving it to be discovered: it is a reading rather than a property, so a package admitted today can
be refused tomorrow on a robot whose disk filled. That is the right answer and it is a new way for
an install to fail.

Two status sentences had to change with them. `GET /health`'s `unmeasured` note said storage
headroom needs a `statvfs` this daemon does not call; on a robot that now measures it, that sentence
is a status document lying about itself, so it is chosen from what is actually absent. `GET
/capacity`'s note is the same edit.

### Two tests that were asserting the absence

`health_reports_what_nobody_has_measured_as_unmeasured` and
`the_capacity_is_read_or_refused_and_never_estimated` both asserted `null`, and both would now fail
on Linux and pass on Windows — the exact platform-dependent test this round has been finding
elsewhere. Rewritten to the rule that was underneath the assertion all along: **never zero unless it
is true**. A machine that can measure reports a fraction or a megabyte count; one that cannot
reports null; neither reports a full disk on a fixture with room on it.

### And the allow-list check, which a comment could satisfy

Adding a fifth `unsafe` crate ran §37's fixed check again, and it **passed when it should have
failed**. `skynet-statfs` is not on the allow-list yet contains `unsafe`, and the loop looked for
the string `forbid(unsafe_code)` anywhere in the crate's root file — where this crate's own header
explains that *`skynetd-io` carries `#![forbid(unsafe_code)]`*, which is why the syscall lives in a
crate of its own. A sentence about another crate's attribute stood in for the attribute.

Anchored now: `^#!\[forbid\(unsafe_code\)\]`. Sixteen crates match it, `skynet-cli` matched only by
prose and is allow-listed anyway, and `skynet-statfs` joined the list with its reason beside it.

Twice in one round the check that guards this workspace's memory-safety argument has been found not
to work, and both times it was found by adding a crate rather than by reading it.

### Verified

| | §37 | Now |
|---|---|---|
| `cargo test --workspace`, Windows | 1,421 | **1,425** |
| `cargo test --workspace`, WSL Linux | 1,462 | **1,468** |

* `cargo fmt --check` clean; `cargo clippy --workspace --all-targets` — **zero** warnings
* The `unsafe` allow-list check's own shell, anchored version — passes
* `check-mirrors.mjs` — §80's retention row rewritten in both repositories, re-pinned;
  `check-golden-path.mjs` clean

---

## 39. `/skynet/os/editions`, deferred nine times, and the two things it could not honestly say

§32.4's second item has carried one reason in every *Still not built* table since the tenth session:
*"needed the moment there is more than one"*, and there was one. §32 of round two finished the
Python edition at 1,109 of 1,109. **The reason expired and nothing noticed**, because a deferral
written in a table is not derived from the tree — which is the finding, and it is the same shape as
§35's check that had never run.

### A page about this repository, built out of this repository

The page renders two things, and neither is typed into it.

**The matrix** is §31.3 — nine languages, each with a tier ceiling that comes from the language
rather than from a flag — and `tools/edition-report.mjs` **parses it out of**
`docs/skynet-os-architecture.md`. That document is already mirrored into the Hub byte-identically
and pinned, which is what makes it a source to read rather than a table to copy. The parse is
strict: a table that has moved or changed shape stops the tool rather than producing a page with six
rows where the specification has nine.

**The measurements** are the output of running the editions. `run_corpus.py --json` and a new
`corpus-report` binary print the same fields under the same names, and the generator refuses to
write a report from a runner that reported a failure or an unimplemented case — §31.4's rule is that
an edition which has not reproduced the corpus is not an edition, and a report file is not the place
to record that one half is.

The file crosses as `reference/reports/editions.json` → `lib/skynet/os/reports/editions.json`, which
is a **third** kind of mirror beside the contracts and the wire fixtures, with a third reason: a
contract is mirrored so two implementations cannot drift, a fixture so a producer and a consumer can
be tested against each other, and a **report** because the Hub renders numbers about this repository
and §32.2's rule does not stop at the robot.

### The two things the first render said that were not true

**"1109-case corpus"**, and the specification's own markdown printed back at the reader —
``` `no_std`, no GC ``` and *"skynetd-equivalent fleet services \*off\* the robot"*, backticks and
asterisks included. The numbers now go through the catalogue as `{cases, number}`, and
`lib/skynet/spec-inline.tsx` renders the two constructs a quoted span carries. It is deliberately
not a markdown library: two constructs, no links, no raw HTML, because everything a fuller parser
would add is a way for text taken from a document to become markup on a page.

**"TypeScript — no code written."** That was the serious one. The Hub is written in TypeScript, the
reference edition *is* TypeScript, and `tools/corpus-gen` produces the corpus by *"running curated
inputs through the TypeScript oracle"*. Measuring it against the corpus would be measuring it
against its own output and would print 1,109 of 1,109 while meaning nothing.

So the join has **three** states rather than two, and each is a different sentence:

| State | What it means |
|---|---|
| measured | The edition exists and reproduced every case |
| oracle | The edition exists and the corpus comes from it, so it has no score |
| absent | Nothing has been written — not *failed*, not *not yet run* |

A blank cell would have been all three at once. The count in the header moved from *2 of 9* to
**3 of 9** with it, because the oracle exists and a page that left it out of the numerator would be
understating the tree to make a rule look tidier.

### And a `FORMATTING_ERROR`, caught this time by looking

Adding `{cases, number}` to the page description without passing `cases` to `generateMetadata`
produced next-intl's `FORMATTING_ERROR` in the dev log and a description that never reached the
page. That is exactly the defect the twenty-first session found in eight places at once and wrote up
as *"in the dev log the whole time"*. It survived here for about four minutes, because the browser
console was read rather than the page merely looked at.

The description was also 183 characters. SEO V3 asks for 140–160; it is 146.

### Verified

* `npx vitest run` — **1,755 passed**, including four new checks on the join and the mirror test
  extended to a third kind of file
* The page rendered and read back: 3 of 9, `1,109` grouped, the matrix's nine rows, `no_std` as
  code, *off* as emphasis, canonical `https://robotoverflow.com/en/skynet/os/editions/`
* `node tools/edition-report.mjs --check` — the file on disk is what a run produces
* `check-contract-mirrors.mjs` — every contract, fixture and report matches; re-pinned in both trees
* 38 new catalogue keys, `i18n-catalog.mjs sync` run, audit clean at 5,912 keys across 35 locales.
  **Not translated**: `translate-with-openai.mjs` needs the owner's approval in the conversation
  that runs it, so the new strings fall back to English until then

**One thing this session did not do, and did not cause.** `npm run typecheck` is red on
`components/listings/NewListingForm.tsx` and `EditListingForm.tsx` — 60 JSX parse errors between
them. Both are **untracked** files in the owner's own marketplace work, unrelated to Skynet, and
nothing here touches them. Every other file in the tree typechecks.

---

## 40. Who may decide a report — the governance question, built

The twentieth session shipped abuse reporting and stopped at a sentence it wrote into its own
migration rather than answering:

> Taking content down requires knowing **who may decide**, and this platform has no concept of a
> moderator. Inventing one in a migration would be choosing the platform's governance model as a
> side effect of adding a table.

The owner decided it on 2026-09-04. This is that decision as code: a **platform staff role**, held
by them; the only thing that may move a report to `upheld` or `rejected`; upholding is the only
thing that may hide another namespace's content; nobody else gains cross-namespace power; and every
decision is audit-logged with who made it.

### The role is not a capability, and that is the whole argument

`permissions.ts` answers *what may this person do in this namespace*, and every capability it grants
is bounded by one. This role is the opposite by construction — it acts **on** other people's
namespaces. Folding it into `Capability` would put a cross-namespace power into the type every
namespace check reads, where the first mistake anybody made with it would be a silent widening.

So `staff.ts` holds it, it has no capability name, `permissions.ts` is untouched, and `org_members`
is still the only roster. There is also **no route that grants the role**: a self-service grant path
is how a compromised session becomes permanent.

### Withholding is deliberately not `visibility`

A repository already has one, and it is the wrong mechanism: the owner can set it back, and a
takedown the party taken down can undo is not a takedown. `withheld_at` / `withheld_by` /
`withheld_report_id` are written by one code path — upholding — and `withholdRepo` is **not
exported**, which is the owner's rule expressed as a call graph.

Three consequences, each of which had an obvious wrong alternative:

* **Upholding hides a repository and nothing else.** A report can name a version, a namespace, a
  discussion, a comment or an evaluation, and upholding one of those records the decision and hides
  nothing — because each needs a withholding rule of its own that this would be guessing at. A
  version is immutable and pinned by deployments; a namespace holds other people's memberships; an
  evaluation removed silently moves somebody else's leaderboard position, which is the same argument
  the account-erasure decision already turned on. The API returns `withheld` so nobody believes they
  took something down when they did not.
* **The namespace finds out through its audit log.** The pages 404 and the listings drop the row, so
  an entry with no actor would be a takedown nobody could ask anybody about. `report.upheld`,
  `report.rejected`, `repo.withheld` and `repo.restored` are the **only** report rows that carry an
  actor: filing and withdrawing stay anonymised, because that log is readable by namespace members
  and the reported party usually owns the namespace the subject is in.
* **It is reversible, by a separate act.** `restoreRepo` rather than re-deciding, because *"it was
  withheld and then restored"* and *"the report changed its mind"* are different histories. A
  takedown nobody can undo is one that has to be right the first time, and this is one person
  reading a queue.

### The queue shows no reporter, and that is in the type

`QueuedReport` has no `reporter_user_id` and the route cannot return one. A moderator who can see
who filed what can be lobbied about it. The row stays attributable in the table where an
investigator can reach it; it is not on the surface somebody decides from.

Oldest first, which is the opposite of every other listing here — a queue sorted newest-first is a
queue whose bottom is never read — with one thing outranking age: `unsafe` and `malware`, because
§11's desired state may be running the subject on machines while the report waits.

### A page, not only a route

The eighteenth session's finding applied before it had to be found again. A report queue that exists
as an endpoint is a queue nobody reads, and an unread queue is worse than none — it is a commitment
the platform has made and is not keeping. `/skynet/settings/reports` is it, reachable by everybody
and refusing legibly: *that* there is a moderation queue is not a secret, and **who is on it** is
what the roster keeps private.

### The 500 this caused, in the four minutes it existed

`listRepos` gained `.is('withheld_at', null)` — the right shape, exact counts, no page slot spent on
a row nobody sees — and every model listing on the site went to a **500**, because PostgREST refuses
to parse a filter on a column the database does not have.

That is this repository's own recorded defect class: nineteen embeds the database could not parse,
each discarded into a plausible wrong answer. Shipping a twentieth to save a `filter` would have
been knowing better and doing it anyway, so the filter is in JavaScript with the cost written beside
it — `total` counts withheld rows — and the query-side version named as what it becomes.

**And then the columns turned out to be live.** A direct probe with the service role finds
`skynet_platform_staff.user_id`, `skynet_repos.withheld_at` and `skynet_reports.resolved_by` all
present. This session did not apply the migration and had no Supabase MCP to have done it with, so
the honest statement is what was measured rather than how it got there. The JavaScript filter stays:
it is correct either way, and *"it worked on the machine where the column happened to exist"* is the
whole family of failure this note is about.

### Four more embeds the database refuses to parse, found by refreshing a record

`skynet-verify-migrations.mjs` reported the new migration as **PARTIAL**, and so was the release
channels one from last round — both because `supabase/schema/skynet.sql` was a session stale, and
five of the nine checks are made against that file rather than against the database.

Refreshing it moved both to **applied — 24 of 24, 0 partial**, and it did something better: the
embed-ambiguity test could finally see the release-channels tables, and immediately found **four
ambiguous embeds** in `release-trace.ts` and `reviews.ts`. `skynet_channel_promotions` reaches
`skynet_release_channels` twice and `skynet_pull_request_reviews` reaches `profiles` twice; an
unqualified embed there is PGRST201, discarded into a plausible empty answer. Each now names its
constraint.

A stale record of the schema had been hiding a defect in code written against it, which is the same
shape as §36's stale deferral and §38's allow-list: **a check is only as current as the thing it
reads.**

### Verified

* `npx vitest run` — **1,762 passed**, 75 files
* `node scripts/skynet-verify-migrations.mjs` — **24 applied, 0 partial, 0 not applied**
* `node scripts/skynet-verify-embeds.mjs` — 295 selects across 42 modules, every one parses against
  the live schema
* `npx tsc --noEmit` — clean for every file this session touched
* The queue page rendered signed out and refused legibly; `/en/skynet/models/` recovered from the
  500 and serves again
* 32 new catalogue keys, `i18n-catalog.mjs sync` run, **not translated** — that needs the owner's
  approval in the conversation that runs it

### Where this round stops

Stopped on request. Nothing is committed and **nothing is pushed**, in either repository.

| Built this round | |
|---|---|
| §26.2's multi-writer ring | `skynet-claim` wired into both recorders, two roles, and the property shown across two processes |
| §33.6 and §80's missing measurement | `skynet-statfs`, and the three fields that were waiting for it |
| `/skynet/os/editions` | §32.4 item 2, deferred nine times, with its numbers generated rather than typed |
| Who may decide a report | the platform staff role, the decision path, the queue, and withholding |

| Still open, unchanged | |
|---|---|
| Account erasure | Decided 2026-09-04, unbuilt |
| Collections | Decided 2026-09-04, unbuilt |
| §26.3's shape question | The daemon opens a ring for no body, so a loop starting after it is refused |
| Two edition disagreements | A malformed `min_os_version`, and the length of a pairing code — both need the owner |
| Phases 10–12 | A real worker, Spaces, GPU training, inference endpoints, teleoperation |

---

# Round four — 2026-09-05

The owner's instruction for this round was two sentences: audit first and say what is left, then
**decide the OS questions myself** and build. So the two entries in the previous round's *Still
open* table that read *"both need the owner"* are settled here, by me, with the reasoning written
down — because a decision recorded without its argument is indistinguishable from a coin toss, and
the next person to touch `min_os_version` deserves to know why it reads the way it does.

## 41. The two edition disagreements, decided — and a third nobody had found

### What the audit found before any of this

Both suites were green and stayed green: `cargo test --workspace` at **1,425**, the Hub's
`vitest` at **1,762** over 75 files, the Python edition at **1,109 of 1,109**, every contract
mirror matching, both copies of the specification byte-identical. §73's own table was the thing
that was wrong, and §39 is a fair warning about that.

### `min_os_version`: neither edition's answer

§80 framed this as a choice between two readings, and said the Rust one was conservative and
therefore probably right. Reading both, it is a choice between **three**, and the third is better
than either.

The reference mapped `Number` over the dotted segments, so `"0-rc1"` was `NaN`; `NaN !== 0` is true
and `NaN < 0` is false, so the robot read as *newer* than the minimum and the package was
**admitted**. Any robot with one non-numeric segment satisfied every `min_os_version` there has ever
been. Rust fell the whole segment back to `0`, which is fail-closed.

**What decided it is that the reference was already inconsistent with itself.** `Number("")` is
**0**, not NaN — and `os-version-with-an-empty-segment` has pinned that since the corpus was
written. So "an unreadable segment is worth zero" was *already* the reference's rule for the only
malformed case anybody had thought about; NaN was an exception to it that nobody chose. Adopting
the Rust reading is therefore not a new policy, it is the rule the reference already had, applied
consistently.

The Rust reading is also not quite right. `"1.2.10-rc1"` falls to `1.2.0`, is refused a package
needing `1.2.5`, and says so:

> Requires Skynet OS 1.2.5 or later; this robot runs 1.2.10-rc1.

A refusal naming a version that looks **newer** than the one it demands. §64 makes that a defect
rather than a cosmetic problem — an operator cannot act on it — and it is the same failure as the
pairing-code sentence below, reached from the other end.

So: **a segment is worth its leading run of ASCII digits, or zero.** A candidate compares as the
release it is a candidate for, and anything genuinely unreadable is still the oldest version there
is. Two fail-*open* cases went with it that neither edition had noticed: `"Infinity"` parsed to
infinity in **both** — the rare malformed version they agreed on, and agreed to admit against every
minimum simultaneously — and `"0x10"` was 16 to the reference.

Written into the specification as **§8.4**, in §8 where a rule about admission belongs, rather than
left in §80's amendments table where nobody looks for one. Five corpus cases in
`corpus/admission/install.json`, and a Rust unit test that names each superseded behaviour beside
the value it used to produce.

### The pairing code: code points, and the reference is the edition that changed

The reference counted UTF-16 code units, so eight characters above U+FFFF were `too_long` — *"A
pairing code is 8 characters and this is 16"*, told to somebody who typed eight. Rust counts code
points and refuses one rule later, at `not_in_alphabet`. Both refuse, so nothing unsafe was
shipping; what was wrong is that the reference's sentence is **false as English**.

Decided: code points. The position in `not_in_alphabet` was a UTF-16 index too, and indexing a
string that way hands back half a surrogate pair — a character in no alphabet and not one anybody
typed — so it moved with the length.

### And the third divergence, which was in the same twelve lines and was worse

Found by reading the function rather than by looking for it. `toUpperCase()` on **one** code point
can return more than one: the sharp s becomes `SS`, the fi ligature becomes `FI`. The reference
folded the *result*; Rust folds each character. So:

| | reference | `skynet-contracts` |
|---|---|---|
| the fi ligature, upper-cased | `FI` | `FI` |
| after folding | `FI` — `I` is **not** in the alphabet | `F1` — both **are** |
| verdict | `not_in_alphabet` | **a valid code** |

One edition **refused** a string the other accepted as a pairing code. Every other divergence this
repository has found was two spellings of one refusal; this one was a refusal against an
acceptance, in the direction that matters — the robot is the half that decides whether pairing
proceeds, and the robot was the half saying yes.

It had been invisible for the same reason as its neighbour: the only non-ASCII pairing case in the
corpus was eight Greek letters, and Greek is inside the basic plane, so it passed under both the
wrong length arithmetic and the wrong fold. Three cases now pin it — a ligature, eight astral
characters, and the sharp s, which the two editions always agreed about and which is pinned so the
agreement cannot rot while its neighbour is being changed.

Written into the specification as **§10.1**.

### What was actually changed

| | |
|---|---|
| `reference/typescript/package.ts` | `segmentValue`, and `compareVersions` reading it |
| `crates/skynet-contracts/src/admission.rs` | the same, in saturating integer arithmetic — the one way an overflow here could fail *open* is by wrapping into a small number |
| `editions/python/skynet_os/admission.py` | the same; `_js_number` and its two regexes deleted, having existed only to reproduce a coercion that is now gone |
| `reference/typescript/identity.ts` | code points for length and position; folding per upper-cased character |
| `editions/python/skynet_os/identity.py` | the same; `_utf16_length` and `_utf16_at` dropped from the module |
| `tools/corpus-gen/generate.ts` | eight new cases, expectations generated from the reference |
| `lib/skynet/os/{package,identity}.ts` | resynced, both digests rewritten by `--sync` |

Rust needed no change to `identity.rs`: it was right about both.

### Verified

* `cargo run -p skynet-corpus --bin corpus-report` — **1,116 of 1,116**, 78 files
* `python editions/python/run_corpus.py` — **1,116 of 1,116**, every corpus complete
* `cargo test --workspace` — **1,426 passed**, 0 failed
* `npx vitest run` in the Hub — **1,762 passed**, 75 files, with the resynced mirrors
* `node tools/check-contract-mirrors.mjs "../Robot Overflow"` — every contract matches

---

## 42. §73 said eighteen crates and one edition, and the fix is not to retype it

§73 opens with an amendment about its own table going stale — *"nobody updates a summary while they
are building the thing it summarises"* — and then closes with a paragraph of typed figures. Those
figures were wrong: **eighteen crates** against twenty, **one edition** against two, **1,055 corpus
cases** against 1,116, **1,392 tests** against 1,426.

The section had diagnosed the disease and caught it again inside itself. That is the point at which
correcting the numbers is the wrong repair.

`tools/scale-report.mjs` measures the tree — Rust lines and crates, the reference, the Python
edition, drivers, the corpus, the Hub's mirror — pins the result in `reference/reports/scale.json`,
and `--check` holds **§73's own sentence** to it. It runs in CI.

### The precision is the design

A check that failed whenever somebody added a test is a check people switch off, and §73 itself says
*"numbers go stale; the shape does not"*. So:

* **Exact** for anything somebody could recount in a second — crates, corpus files, corpus cases.
  Each of those was wrong, and each is wrong in a way a reader would catch and lose confidence over.
* **To the nearest thousand** for line counts, matching the precision the prose states them to. A
  hundred new lines does not fail a build; eight thousand does.
* **Test counts are deliberately excluded.** The suites print them on every run. A specification is
  not where anybody should read them, and putting them under a check would make every commit that
  adds a test a documentation commit.

### It caught two errors in the edit that introduced it

Which is the only reason to believe it works. `13,741` rounds to **14,000** and I had written
13,000; and the pattern for the Hub's mirror did not match because the sentence wrapped between
`the` and `Hub's`. The first is a real error in the document. The second is a defect in the check —
where a paragraph wraps is not a fact about the system — so it now collapses whitespace before
matching.

### And a third, which would have failed every CI run

The first version wrote `measured: true` into the pinned file when a Hub checkout was beside this
one. CI has a single checkout, so a fresh run there would produce `measured: false`, the pin and the
run would differ, and the job would fail for a reason with nothing to do with §73. Verified by
copying the tree to a directory with no Hub beside it and running the check — it failed, exactly as
described.

**Whether this run could see the Hub is a fact about the machine it ran on, and it must not reach
the pinned file.** The file carries the number; the console says where it came from. Re-verified in
both configurations, and then verified that the check can still *fail* — a doctored crate count in a
throwaway copy produced `crates: §73 says nineteen, measured 20`.

A check that cannot fail is the thing §36 and §38 both found. It seemed worth not shipping a fourth.

### One mistake worth recording because it cost real work

`node tools/check-mirrors.mjs --sync ../BotMarket` copies **from** BotMarket, and its header says
so. I ran it after editing the specification in *this* repository and it did exactly what it
promises: overwrote four finished sections with the Hub's older copy. All four had to be re-applied.

The shared documents have a direction — **the Hub is the authority, and this repository syncs from
it** — and the way to make an edit is to make it there. Recorded here rather than quietly redone,
because the next person will read `--sync` as "make these match" and lose the same hour.

### Verified

* `node tools/scale-report.mjs --check "../Robot Overflow"` — all eight figures match
* the same with no argument, as CI runs it — passes, and says the mirror was not measured
* `node tools/check-mirrors.mjs` — all shared documents match, both copies byte-identical

---

## 43. The policy runtime, and the ring that had a consumer and no producer

The largest unbuilt thing in §73's table, and the one eight of its rows named without naming: the
last column said some version of *the ring has no action channels* eight times, and every one of
them was downstream of the same absence. `skynet-kernel-host` reads §25's command ring. Nothing
wrote to it. Its own header had said so since it was built — *"Nothing here loads a model, assembles
an observation frame or produces a desired command; this is the consumer of the ring, never the
producer."*

`crates/skynet-policy-host` is the producer. **13 tests, and the last two are the ones worth having.**

### The design turns on a distinction §25 had never made

A policy has three things it can be doing, and modelling what a runtime returns as `Option<Command>`
spells two of them the same way:

| | published | the sequence | what §7.1's watchdog sees |
|---|---|---|---|
| a command | a command | advances | a live producer |
| a **hold** | a command with an all-zero mask | **advances** | a live producer asking for nothing |
| **silence** | nothing at all | **does not advance** | a **stalled producer**, and a controlled stop |

A hold is *I am here and I want nothing*, and it has to satisfy the watchdog — a robot that stopped
every time a policy waited would be unusable. Silence is a policy that crashed, and it must not.
§24 had already decided the consequence (*"the watchdog treats a repeat as a stalled producer"*);
what was missing was the rule on the producer's side, and it is one sentence: **the sequence counts
commands that reached the ring, and nothing else.** Not cycles. Not ticks.

Advancing it on a cycle that published nothing would tell the kernel a producer is alive when it is
not, which is the one lie this process can tell that the architecture cannot survive. It also makes
publishing nothing the *cheapest* path, which is deliberate — a runtime in trouble should not have
to succeed at anything in order to fail safely.

Written into the specification as **§25.3**, because it is a property of the protocol rather than of
this implementation, and the next edition to write a producer needs it.

Two smaller rules fell out of the same place, and both are tested:

* **A hold masks every channel out rather than commanding zero.** On a torque-controlled joint a
  zero *value* goes limp and a zero *mask* holds position, which is §24's reason for a mask
  existing. Getting this backwards is a robot that collapses when its policy pauses.
* **A command past the lease is refused, not clamped.** Narrowing it would be this process forming
  an opinion about what is safe; §7's guards are the only place that opinion may live. The check is
  over the **mask** rather than the values, because §24 asks a producer to keep one full-width
  buffer and reuse it — so a stale float above the lease is debris, and refusing it would refuse the
  thing the specification asks for. Both directions are pinned.

### A guard has tripped on a policy's command

§73's interface row has said, in every revision since the guard-trip column got its source, that
what it lacks is *"a robot that trips a guard"*. It could not have had one: tripping a guard needs a
command, and every command in this repository's history was written by hand in a test.

`a_policy_that_overruns_its_envelope_trips_a_guard` is the first one that is not. A trajectory asks
a joint for 5.0 radians against a per-channel ceiling of 1.0; the runtime publishes it unnarrowed;
the envelope guard clamps it to 1.0; `Cycle::intervened()` becomes true; the body goes to the
ceiling and the record keeps the 5.0 that was asked for, which is what makes the trip investigable
months later. That boolean is the numerator §15's `field_conformance` is a ratio of — still not
*reported* by anything, because that needs a window and a robot, but no longer a ratio with nothing
to count.

### And the whole path, end to end, in one process

`the_whole_control_path_in_one_process` puts this producer and `skynet-kernel-host`'s consumer on
one ring: a backend produces an intent, the runtime publishes it, the kernel reads the sequence this
crate assigned, the eight guards evaluate, the driver moves. Then a second turn, which says
something the kernel host's own suite structurally cannot — a finished trajectory **holds**, the
sequence still advances, the joint stays where the trajectory left it rather than returning to zero,
and the robot is not stopped for having finished successfully.

The kernel host links this crate as a **dev**-dependency and never a dependency. Two processes that
meet at a shared mapping must not link each other's code, or neither half can be replaced — which is
the property a vendor writing one of them needs.

### Three things it deliberately does not do, and one that was removed

**It does not do inference.** §23.3 puts CUDA, PyTorch and ONNX Runtime above the kernel; a neural
network is a `Backend` nobody here has written, and the trait exists so that vacancy is named. What
ships is `Hold` — a real policy, and the right one for a robot with a lease and no behaviour loaded
— and `Trajectory`, open-loop, which is what a homing sequence and a calibration sweep are.

**No observation reaches it.** §25.1's assembler is built, corpus-pinned and reproduced in three
editions, and it is wired to nothing, because the transport that would carry a frame from the
process holding the RHAL to the sandboxed one does not exist. So `Backend::decide` has **nowhere to
put a frame**, rather than taking one and being passed `None`. A parameter nothing can fill is a
worse lie than a missing one.

**No two processes have shared a ring.** `skynetk` creates its ring privately and says so; `skynetp`
does the same. Neither offers a descriptor and neither adopts one.

Which is what was removed. A draft of `skynetp` had `--ring-fd`, adopting a mapping another process
passed over — and nothing in this system passes one over. That is half a handshake, and §79.7 is
this repository's own record of what half a route costs: the address answered, the sender backed off
politely on §34.4's curve, no bytes moved, and the failure was indistinguishable from patience. The
option went, and `skynet-ring-shm` went with it rather than sitting in the manifest doing nothing.

Both gaps are now rows in §80 with the shape of their answer written down — a second mapping the
other way for observations, which fits a 4 KiB slot for proprioception and does **not** fit camera
frames; and a descriptor handshake at both ends.

### §26.3's missing number now exists

§80 recorded the daemon's ring-shape question as blocked on a channel count, *"because channels are
the policy runtime's and that runtime is unwritten"*. `Commanding::channels` is that number,
declared when a lease is granted, which is before either ring is opened.

It is **still open**, and the reason has changed and shrunk: the number exists and nothing carries
it to the daemon. That is a lease record `skynetd` can read rather than a design question, and it is
the first thing worth building on top of the runtime.

### The scale check, four hours old, earned its place twice

Adding a crate made §73 wrong again immediately — *"crates: §73 says twenty, measured 21"* — which
is exactly the drift it exists to catch. Then it caught a defect in itself: `\w+` does not match the
hyphen in "twenty-one", so it reported the figure as **absent** rather than wrong. A number word
with a hyphen in it is still a number word; the pattern is `[\w-]+` now.

### Verified

* `cargo test --workspace` — **1,439 passed**, 0 failed, **0 warnings** across the workspace build
* `skynetp run --hz 200 --cycles 10 --channels 2 --backend trajectory --setpoints "0:…;3:…;6:…"` —
  3 commanded, 7 held, 0 refused, which is what three rows over ten turns should be
* `cargo run -p skynet-corpus --bin corpus-report` — 1,116 of 1,116, unaffected
* `python editions/python/run_corpus.py` — 1,116 of 1,116, unaffected
* `npx vitest run` in the Hub — **1,762 passed**; `npx tsc --noEmit` — clean
* `check-mirrors`, `check-contract-mirrors`, `check-golden-path`, `scale-report --check`,
  `edition-report --check` — all pass, with the editions report resynced to 1,116

### What §73 now says, and what it still does not

The §25 row moved from *"the half that fills it does not"* exist to **built on both ends**. The
interface row's *"a robot that trips a guard"* became *"a **robot** that trips one"*, which is a
much smaller gap. The skills row's *"nothing on this edition can run a policy"* became a sentence
about a missing wire between §54.4's lifecycle and a runtime that exists.

None of it has run on a robot, and the table still says so.

---

## 44. The loop closes: `skynet-obs`, and the arrow §25 never specified

§43 built the producer and immediately opened the largest gap in this document, because §25 had
never said how a policy learns anything. Its diagram has an arrow going down — *observation frame →
assembler → inference* — and its prose specifies only the arrow going up: *"the ring is the only
channel **from the policy to the kernel**"*, and the policy *"cannot reach the RHAL at all"*.

So §25.1's assembler — built, corpus-pinned, reproduced in three editions, the most carefully
specified thing in Part II — was **wired to nothing**, and every backend was open-loop, because
there was no way for an observation to reach a process that could use one.

`crates/skynet-obs` is that channel. **9 tests of its own, and 22 in the policy host.**

### A separate format, and the magic is the argument

It is the mirror of `skynet-ring`: single-producer, single-consumer, lock-free, one page per slot,
the same seqlock word for word. About a hundred and fifty lines of real duplication, and it buys one
thing:

**A consumer built for commands that opened an observation mapping would read joint positions as
commanded setpoints, under a header that validated, with the drives live.** With `SKYO`/`BSV1` here
and `SKYR`/`ING1` there, `Ring::open` refuses at the first word. The alternative — one crate, one
header, a `kind` field — puts that check one field further in, on a value a caller passes, in the
crate that is on the safety path. This way the magic and the type system agree, and `skynet-ring`
was not touched to get it. A test pins the constants the two formats genuinely share.

### Two decisions the tests made for me

Both were written as assertions about what *should* happen and both failed, which is the useful kind
of failing test.

**A joint above the declared count reads as `NaN`, not zero.** The consumer reads only the joints a
slot declares — 384 loads per cycle for a six-joint arm would be most of the copy — so what a caller
sees above the count is whatever the default put there. Zero is a *measurement*: a claim that joint
seven of a three-joint body is at its origin, under the fresh stamp of a real observation. The
producer already wrote `NaN` above the count for the same reason; the default now agrees.

**`Observation` has no `PartialEq`, and that is deliberate.** The first consequence of the above was
a test comparing two reads of one slot and getting *not equal*, because `NaN != NaN`. A derived
comparison here is false exactly when it is most likely to be trusted — a test asserting nothing
changed — so the derive is gone and the doc says which fields to compare.

### What the runtime does with a reading, and what it refuses to

§25.1: *"A staleness budget per stream, from the policy's declared observation requirements.
Exceeding it is a `hold`, not a silently older frame."* Enforced by the **runtime**, not by each
backend: a reading past its budget means the backend is **never asked**, so it cannot decide
otherwise, and the hold is counted separately from a hold the policy chose. "Why did the robot stop
moving" has two very different answers and they should not be one number.

An undeclared budget is not a licence. §25.1's own note says an absent budget *"means nothing here
can say whether the one chosen is good enough"* — so the reading is passed through and the turn
counts it as `unbudgeted`, which is the count of cycles on which nothing in this system could vouch
for the body a policy acted on.

And `Body` has four variants rather than being an `Option`, for the same reason `Intent` has three:
**open-loop by configuration** and **a feed that has died** are different situations. `Detached` is
a robot running a homing sequence. `Empty` is a policy that started before the driver. Collapsing
them would lose the distinction on exactly the robot where it matters.

### The first closed loop in this repository

`Approach` is the smallest backend that reads what it is given: `position + gain × (target −
position)`. Not a control design — no velocity term, no feed-forward, no model — and that is not
what it is for. It is the first backend whose command on cycle *n* is a function of what the robot
did on cycle *n − 1*.

It refuses to guess. A joint whose measured position is `NaN` is **masked out** rather than
commanded: `NaN × gain` would put a `NaN` on the wire, and commanding from an assumed zero would
drive a joint to its origin *because nothing measured it*, which is the worst available response to
missing data. Masking holds it, which is §24's meaning of a zero mask.

Two tests carry the result, and the second is the one worth having:

* `the_body_reaches_the_policy_and_the_policy_moves_the_body` — the whole diagram, running. Driver
  read, state across the observation ring, decision, command across the command ring, eight guards,
  driver written. 0 → 0.5 → 0.75 → 0.875 → 0.9375, and every number comes out of the driver rather
  than out of the test.
* `a_clamped_command_comes_back_as_the_body_the_robot_actually_reached` — a guard clamps 1.0 to a
  ceiling of 0.25, and the next observation carries **0.25**, where the robot is, not 1.0, what it
  was told. A policy reading the second would never learn it is being clamped. This is the case that
  makes the ring worth building rather than passing commands back.

### The publisher publishes on failed reads too

`Host::step_observing` publishes the body whatever the cycle did, including cycles where the driver
read **failed**. The fallback stamp and a zero sequence go out, and a policy watching the sequence
stop advancing learns exactly what the black box learns. Skipping the publish would leave the policy
reading a stale slot with no way to tell it from a body that is not moving — which is the same
Hold-versus-Silent confusion §25.3 exists to prevent, arriving from the other direction.

`step` still exists and delegates with no producer, so every existing caller is unchanged and an
open-loop deployment stays one.

### Verified

* `cargo test --workspace` — **1,457 passed**, 0 failed, **0 warnings**
* `cargo run -p skynet-corpus --bin corpus-report` — 1,116 of 1,116, unaffected
* `python editions/python/run_corpus.py` — 1,116 of 1,116, unaffected
* `npx vitest run` in the Hub — **1,762 passed**
* `check-mirrors`, `check-contract-mirrors`, `check-golden-path`, `scale-report --check` — all pass

### What this does not do

**Vision.** The ring carries the body, not the cameras. A 4 KiB slot holds 128 joints three times
over and does not hold one image; a ring of megabyte slots is a copy per frame at capture rate.
Frames need a buffer pool with handles and a lifetime rule, and beginning that as a side effect of
this would have produced a bad version of it. So a policy can close the loop on where its joints are
and not on what is in front of them, and §54's skills cannot yet be what they will be.

**Two processes still have not shared either ring.** Both hosts create theirs privately and say so.

**No backend does inference.** §23.3's runtime is still a named vacancy in the `Backend` trait.

---

### Where round four stops

Stopped on request. Nothing is committed and **nothing is pushed**, in either repository.

| Built this round | |
|---|---|
| §80's two edition disagreements | Decided by me on the owner's instruction, specified as §8.4 and §10.1, built in three editions, eight corpus cases — plus a third divergence found in the same function, where one edition accepted a pairing code the other refused |
| §73's stale figures | Measured by `tools/scale-report.mjs`, pinned, and checked in CI. It caught its own author three times |
| The policy runtime | `skynet-policy-host`. §25's command ring has both ends; a guard has tripped on a policy's command |
| The observation path | `skynet-obs`. The loop closes: a command that depends on what the robot did, and a clamp the policy can see |

| Still open | |
|---|---|
| Vision | A buffer pool with handles. The one thing between this and a real manipulation policy |
| Two processes sharing a ring | Neither host offers a descriptor. Removed from `skynetp` rather than shipped as half a handshake |
| Inference | No backend loads a model |
| §26.3's shape question | `Commanding::channels` is the number it waited on; nothing carries it to the daemon |
| Account erasure, Collections | Hub-side, decided 2026-09-04, unbuilt |
| Phases 10–12 | A real worker, Spaces, GPU training, inference endpoints, teleoperation |

---

## 45. §26.3's shape question, and the lease record it turned out not to need

The item at the top of the previous round's *Still open* table, and the one §80 called *"the first
thing worth building on top of the runtime"*:

> `Commanding::channels` is the number it waited on; nothing carries it to the daemon.

On a robot `skynetd` starts first (§27), so `skynetd` **creates** the recording. It created one
shaped for zero joints and zero channels — the honest description of a supervisor that commands
nothing, and the wrong thing to write into a file a second writer has to share. A control loop
starting afterwards was refused, because a cycle carries a value per joint and per channel and does
not fit a record sized for no body. So the ordering that worked was the loop first, and the ordering
a robot actually boots in is the daemon first.

`skynet-kernel-host`'s refusal has said what would fix it since it was written, in its last clause:
*"Point this loop at another directory, or **give the other writer this body's shape**."*

### Both halves of the stated blocker were wrong

§80 asked for a lease record. A lease record would have worked and would have been wrong in a way
worth keeping, because the shape of the mistake is the useful part:

* **A lease is granted when a policy starts. The daemon needs the number when *it* starts**, which
  is earlier. A record written at the first lease and read at every later boot leaves the first boot
  after installing a policy still refusing the loop.
* **And it would size every later ring from whatever ran last** — configuration inferred from
  history, which is what §28's layout header argues against in as many words: *"a record kept beside
  the thing it describes is a record that can disagree with it."*

The second half was wrong too. **Nothing has to carry the number, because the daemon already
computes it.** §37.1's `transport` stage resolves a plan for every package this robot installs, and
a plan's `bindings` are *"every channel, in the policy's output order"* — the same count
`Commanding::channels` holds, from the same declaration, one process earlier. The daemon has been
computing it at install and throwing it away since §37.1's transport stage was built.

| | Where it already was |
|---|---|
| joints | the embodiment descriptor, which `embodiment::load` already reads |
| channels | the transport plan, which `admit::transport_plan` already resolves |

Both on disk before the first pass reconciles anything. `skynetd-io`'s `shape::supervising` is
sixty lines that read two things the daemon had and joins them.

### It refuses to guess in five ways, and that is most of the design

| | The ring |
|---|---|
| No descriptor | events only |
| A body, nothing installed declaring an action space | events only |
| One commanding package, plan resolves | **the body's shape** |
| One commanding package, plan `refused` | events only — a loop that will never run |
| Two commanding packages | events only, both named |
| A manifest that does not decode | events only, named |

**Two commanding packages is a refusal rather than a choice.** §25 has one command ring, one loop
and one lease, so which policy will command is not a fact the daemon holds, and picking one costs
the other its recording. The line names both, so an operator can act on it.

**Every events-only outcome is zero joints *and* zero channels**, never the body's joints. A record
sized for a per-joint array no writer fills spends §13's window on zeroes.

`allow_retarget` is `false`, which is not a default but §6.1's rule: retargeting is *"a decision
somebody made, not a fallback that happens quietly"*, and there is no somebody at boot. `stage` reads
a flag for it because a person typed the command; the reconcile loop passes `false` for the same
reason this does.

### The defect the Linux run found, in code written this session

The five tests that need an activated package are `cfg(unix)` — activation is §37.1's atomic symlink
swap and Windows has no such thing. Run on Linux under WSL rather than left for CI, they failed:
every commanding robot read as `NoPolicy`.

The fixture was missing `digest`, which `manifest_from_json` requires. That was a bad fixture. What
it exposed was worse and was mine: `supervising` read the decode failure with

```rust
let Ok(manifest) = manifest_from_json(&document) else { continue };
```

— which reads *a manifest nobody can parse* as *a package that declares no action space*. `json.rs`
warns about exactly this where it decodes the same block:

> a block that does not decode is an **error** rather than an absence: an action space nobody could
> parse and an action space nobody declared are **opposite facts**

And `tests.rs` already asserts the state is reachable: *"`manifest_from_json` refuses, so admission
does not run at all."* A robot with a policy whose manifest had gone bad would have had its ring
sized events-only, its loop refused, and nothing anywhere saying why.

So there is a `Supervising::Unreadable`, it is **fatal to the whole answer rather than to one
package** — the unreadable manifest may be the one declaring the action space, so a count taken
without it cannot be trusted — and it names the package and the reason.

**Fixing the fixture alone would have made the tests pass and left the defect.** That is the second
time in three rounds that running a thing rather than reasoning about it found the difference.

### Two stale headers, corrected before they became the next session's finding

`skynetd-io/src/blackbox.rs` stated as fact that *"there is no control loop yet: the policy runtime,
the SPSC ring and observation assembly (§25) are unwritten"* — false since round four — and that the
ring's shape is always zero/zero, false as of this item. `skynet-kernel-host/src/recorder.rs` quoted
the same sentence. Both now say what is true, and the kernel host's says which process creates the
ring and why that is not its decision.

### Verified

Mutation-checked in both directions, because a test that passes against the old behaviour is not
evidence:

| Mutation | Caught by |
|---|---|
| `shape()` always events-only — the behaviour before this item | `one_policy_sizes_the_ring_for_the_body`, `the_ring_the_daemon_creates_is_one_the_loop_can_attach_to` |
| restore the `else { continue }` that swallowed a bad manifest | `a_manifest_that_does_not_decode_is_not_a_package_that_commands_nothing` |

* `cargo test --workspace` — **1,459 passed** on Windows, **1,508** on Linux, 0 failed
* 8 new tests in `shape::tests`, 6 of them `cfg(unix)`, all run and passing under WSL Ubuntu 24.04
* `cargo fmt` / `cargo clippy` — clean **for the files this item touched**; see below
* corpus 1,116 of 1,116 in both editions, `check-contract-mirrors`, `check-golden-path`, and
  `check-mirrors` all pass; the Hub's suite is unaffected
* `scale-report --check` failed on this item's own line count and was right to: §73 said 109,000
  lines of Rust and the tree holds 109,977. §73 says 110,000 and the report is re-pinned

### What this does not close, and one thing it did not touch

**A loop nothing declared.** `skynetk` run by hand against a driver names no package and defaults to
one channel per joint, so a robot with a body and no commanding package still meets the shape
refusal. That is the honest answer rather than a gap — nothing on that robot declares a policy, and
a daemon inventing one would be sizing evidence for a loop that exists in an operator's terminal.

**The two writers still have not shared a ring in one boot.** This makes the daemon-first ordering
*possible*; it has not been run end to end with two processes, because neither host publishes a
descriptor. That remains §80's own row and is unchanged by this.

**`cargo fmt` and `cargo clippy` are red in the tree, and not from this item.** Ten formatting diffs
and four clippy warnings sit in round four's uncommitted work — `skynet-contracts/src/admission.rs`,
`skynet-kernel-host/src/run.rs`, and the new `skynet-obs` and `skynet-policy-host` crates. §44's
entry records that round as *"fmt and clippy clean"*, and it is not now. CI sets
`RUSTFLAGS: -D warnings`, so those four warnings would fail the build. They are left alone rather
than fixed in passing: they are another round's in-flight code, and `assertions_on_constants` and
`large_enum_variant` want a decision from whoever wrote them, not a drive-by.

# Round five — 2026-09-06

## 46. The console has been reachable and unaddressable since §34.2 was written

Asked for the foundations of an operator-facing desktop application, and the first question was
which part of it cannot be a web page. The answer turned out to be a hole in the specification
rather than a feature request.

§32.2 gives the console its reason for existing — *"when the network is down the Hub is unreachable,
and that is exactly the moment somebody needs to see the robot."* §34.2 holds that line properly:
`Lan::bind` **refuses a routable address**, so the console is link-local and unreachable from off the
robot's network. Both are right. Together they leave the console reachable and **unaddressable**,
because to open it you need an address, and a link-local IPv6 address carries a zone index —
`fe80::a1b2:c3d4:e5f6:0708%en0` — that nobody types or reads off a chassis.

Every part of Part I assumes an operator already knows where the robot is. On a bench with three, or
on the first boot of a robot whose network configuration is the thing that is broken, they do not.

`mdns`, `_tcp` and `dns-sd` had **zero occurrences in either repository**. Not a gap in an
implementation — a missing subsystem underneath the one surface the specification promises will work
when everything else has failed.

### `skynet-mdns`, and the two rules that decide what a robot may say

The twenty-third crate, and the first that does not run on a robot: DNS-SD wire format, RFC 6763's
TXT convention, the definition of a Skynet advertisement, and a one-shot browse. `std::net` and
nothing else, `forbid(unsafe_code)`, 66 tests.

The design work was almost entirely in deciding what goes in the record, and both rules removed
something the first draft had.

**Rule one — a multicast record is published at `Anonymous`.** §39.1's table gives exactly two routes
to nobody in particular and puts everything else behind a credential. An advertisement is readable by
every host on the link. So a key in a TXT record is that key published at `Anonymous`, whatever the
route that also serves it requires.

The first draft advertised the **device identifier**, because correlating a robot on a bench with a
device in a Hub fleet is exactly what an operator wants. `GET /identity` is `Observer`. Advertising it
would have been a crate outside the daemon quietly overruling the daemon's own authorisation table —
the silent widening `permissions.ts` has a whole header about on the Hub side. It is not there.

**Rule two — a discovery record carries identity, not state.** mDNS records are cached by other hosts
and by the resolver for the TTL the responder set, so a record announced at boot is still being served
after the robot has been paired, upgraded or unplugged. Anything that *changes* is wrong to advertise.

That leaves the instance name, `txtvers`, `api`, SRV and the addresses. **The OS version is
deliberately absent and is the one worth naming**, because it passes rule one — it is anonymous on
`GET /status` — and is genuinely useful in a fleet list. It fails rule two: it changes on every OTA,
and a list showing a robot on last week's image is a list that will be trusted and be wrong.

A test asserts the record holds exactly two keys, and a second asserts the encoded bytes contain no
`id=`, `tier`, `state`, `paired`, `os_version` or `guard`. A key added later without an argument for
why it is safe at `Anonymous` **and** stable over a TTL fails there first.

### The unicast reply, which is a constraint and was nearly recorded as a preference

The textbook browser binds 5353, joins the group, and sees unsolicited announcements. That is the
better design. **It requires `SO_REUSEPORT`, which `std::net` does not expose**, and the port is held
permanently by `mDNSResponder` on macOS and by Avahi on most Linux — so a plain bind fails on two of
the three platforms this has to run on. Reaching the option needs `unsafe` and a raw `setsockopt`, or
a dependency; this crate has ruled out both.

So the query sets RFC 6762 §5.4's QU bit and listens on an ephemeral port, which is a mode of the
protocol rather than a workaround. What it costs is written down rather than discovered later:
unsolicited announcements are not seen, so **there is no watch mode** — a watch on this transport
would silently miss arrivals, which is worse to offer than no watch.

### The parser, which reads packets from anyone on the link

Two bounds on name compression, and building the test for the second one is what showed the first is
not sufficient:

* **a pointer must target an offset strictly before its own** — RFC 1035's own rule;
* **and jumps are capped at 64.**

A pointer that targets another pointer directly *does* decrease monotonically, because the cursor
becomes the target. Interleaving a **label** with each jump breaks that: reading the label moves the
cursor forward, so the next pointer sits above the one before it and every jump is still strictly
backwards. `a_name_that_only_grows_by_jumping_is_capped` builds that ladder inside a TXT record's
RDATA and enters it from a second record's name. Without the counter it is unbounded, and the
counter's doc comment claimed exactly that before there was a test proving it.

A companion test runs the same ladder one block shorter and asserts it still decodes, because a cap
that refuses a legal name is its own defect.

Also: nothing allocates from a declared record count — a hostile twelve-byte header can claim 65,535
records — and the TXT parser is deliberately *lenient* where the wire parser refuses, because RFC 6763
§6.4 says to skip a malformed entry and refusing the record instead would let any host on the LAN
erase a robot from an operator's list by appending one junk byte.

### One half, and it says which half

`advertisement()` — what a robot answers a browse *with* — is defined beside the parser that reads it,
so the two halves are one definition rather than §78's two spellings. **Nothing calls it.** Binding the
responder socket is `skynetd`'s and Unix-only.

§79 is the record of what half a route costs when nobody writes it down: an address that answered, a
sender that backed off on §34.4's curve, no bytes moved, and *"the failure was indistinguishable from
patience."* So this was built the other way round: the browse returns an **error** rather than an empty
list when it could not ask at all, and reports three counters beside the robot list — sockets asked on,
datagrams malformed, well-formed answers about other services. On a laptop with a VPN filtering
multicast, "no robots answered" and "this machine could not ask" are indistinguishable from a result
set, and only one is a statement about robots.

And a link-local IPv6 address with no scope is refused rather than handed back as a URL that will fail
to connect — reproducing the unaddressable-console bug one layer in would have been a poor joke.

### §82 and §83, written because the decision would otherwise be made twice

Part XI is new. §82 is the design above. §83 records what was decided in discussion about the
application itself, and the part worth keeping is the scope line, which came from the owner correcting
me: installing a skill **is** a Hub fetch followed by a robot install, so splitting the Hub out into
its own application makes an operator do half a job in a browser and half in a console, copying digests
between them.

> **If it ends in a robot doing something, it is in the application. If it ends in a person reading
> something, it is on the web.**

And the constraint that decides whether the thing survives contact: **it is a shell, not a client.** It
embeds the console the robot already serves and owns only what a browser cannot do — discovery, USB,
multi-gigabyte sealed windows, a tray stop, several robots at once. Re-rendering §32.2's six panels
natively would be a second implementation of the same data, which is §78's shape and §73's own
figures-wrong-three-times shape. The tray stop is a convenience and §83.2 says so in bold, because the
moment it is presented as a safety control the application has become the failure this document is
arranged against.

### Two things this found that were not the task

**`scale-report --check` caught its author for the fourth time**, which is now the mechanism working
rather than a pattern: §73 said twenty-two crates and 110,000 lines against a tree holding 23 and
112,000. Corrected and re-pinned.

**A CRLF round trip, caught by the pin tool's own guard.** Editing the specification with a Python
script on Windows rewrote it with CRLF line endings, and `.gitattributes` pins these documents to LF
*on every platform* precisely because the mirrors are decided by exact bytes. `check-mirrors --pin`
refused, with the argument already in its header: *"silently hashing bytes other than the ones on the
disk is how a pin stops being a statement about a file."* Both copies converted back to LF, byte
compared, and pinned. The guard was written two rounds ago against a hypothetical and caught a real
one on its first opportunity.

### Verified

* `cargo test --workspace` — **1,525 passed**, 0 failed (1,459 before; the 66 are this crate's)
* `cargo fmt -p skynet-mdns --check` clean, `cargo clippy -p skynet-mdns --all-targets` **0 warnings**
* `check-mirrors`, `check-contract-mirrors`, `check-golden-path`, `scale-report --check` — all pass
* The specification is mirrored into BotMarket and re-pinned

### What this does not do

**No robot advertises**, so a browse on a real network finds nothing. That is the honest state and it
is one Unix-only socket away, in `skynetd`.

**Nothing of the application exists** beyond §83 and the one crate underneath it. No Tauri workspace,
no window, no USB. That was the scope asked for — foundations, so the idea is not lost — and growing
it further before there is a robot on a bench would be building a fleet tool with no fleet.

**`cargo fmt` and `cargo clippy` are still red across the tree**, unchanged from §45: ten formatting
diffs and four warnings in round four's uncommitted `skynet-contracts`, `skynet-kernel-host`,
`skynet-obs` and `skynet-policy-host`. Still not touched, for §45's reason — `assertions_on_constants`
and `large_enum_variant` want a decision from whoever wrote them. **CI sets `RUSTFLAGS: -D warnings`,
so the workspace build fails until they are dealt with.** It is the shortest item on this list and the
only one blocking a green build.

Nothing is committed and nothing is pushed, in either repository.

## 47. Skynet Console: a window that opens, and a bug I reported that was never there

§46 built the discovery library and no application. This is the application — and then the owner
moved it and renamed it, both for reasons better than the ones I had written down.

### What exists

A Tauri workspace: a native window, a two-pane interface, and the scan wired to §82's browse. It
builds, opens, runs a real query on the link and renders the answer. Verified by running it.

Three files of interface, **no build step and no npm**, which is §32.2's discipline applied to a
different machine: there it is because the page must render on a robot with no internet; here it is
because this is the window an operator opens when a robot is misbehaving, and a toolchain between
the source and the thing that runs is one more component that can be the reason it does not open.

### The screen that matters is the empty one

A scan finds nothing, because §82.4's responder is unwritten. That is the state this application
will be in for as long as it takes to write the daemon's half, so the empty state is the design:

> **This is expected right now.** No robot advertises itself yet. The responder is the daemon's half
> and is not written, so an empty result is a true report rather than a fault — there is nothing on
> any network for this scan to find.

Beside it, what was actually heard: sockets asked on, other services, unreadable datagrams. And a
**failure to ask is a different screen from a failure to find** — on a laptop with a VPN filtering
multicast those are indistinguishable from a result set, and only one is a statement about robots.

`about()` reports what the build cannot do as **data from the binary** rather than as text in the
window, so a capability cannot be claimed by an interface nobody updated. Four absences are listed
rather than omitted, and the stop is labelled a convenience in the same breath it is promised.

### The bug I reported, which was a screenshot

I told the owner the "Not in this build" text overflowed the window and the Scan button had been
pushed off the right edge, and spent three rounds fixing it: `minmax(0, 1fr)` tracks, a grid row
instead of a flexbox, overflow guards at three levels.

**There was no overflow.** This environment has no desktop composition, so `CopyFromScreen` returns
black and `PrintWindow` is the only capture that works — and the shell calling it was DPI-unaware,
so `GetWindowRect` came back virtualised at 1/1.5 and the bitmap held the top-left **67%** of a
window rendering at `devicePixelRatio` 1.5. Every "clipped" edge was the edge of the crop.

What settled it was measuring instead of looking: a temporary command through which the window
reported its own geometry to stdout.

```
PROBE vw=1240 sw=1240 over=0 main=1240 list=260 detail=980 dpr=1.5 OVER:none
```

`scrollWidth == clientWidth`, and no element's right edge past the viewport. The layout had been
correct since the first build. Marking the shell `PER_MONITOR_AWARE_V2` produced a 1882×1286
capture with the Scan button exactly where it always was.

Two things worth keeping from being wrong for an hour. **A capture is an instrument and needs
calibrating like one** — I trusted three screenshots to say something they could not say, and the
one experiment that could distinguish the hypotheses took five minutes. And a **freshness marker**
is the cheap first move: changing a visible string proved the embedded assets were live and
eliminated the entire "stale build" branch I had been reasoning down.

The CSS changes are kept. They fix nothing that was broken, and `minmax(0, 1fr)` on a grid track is
still more robust than relying on a child's `min-width` — but the entry says plainly that they were
written against a fault that did not exist.

### Moved, and renamed

Both by the owner, and the first reverses what §83.3 said when I wrote it a session ago.

**It lives in BotMarket now, not in `skynet-os`.** §72.1 splits the repositories so the OS can be
open-sourced without a history-scrubbing rewrite — it begins private and *becomes public*. Skynet
Console is a commercial product surface rather than part of the runtime a vendor compiles against,
so putting it in the repository that is going to be published would mean either publishing it or
performing exactly the surgery §72.1 exists to avoid. And it had to move before it grew, which is
§72.3's own argument: moving a window and a scan is minutes; moving that plus a bundler, a signing
pipeline and a release matrix is a week.

**`skynet-mdns` stays in `skynet-os`,** and that half matters as much. §82's responder belongs to
`skynetd`, both halves must be one definition rather than two spellings (§78), and a vendor reading
the OS needs to see what a robot advertises. So the application consumes it by path from a sibling
checkout, which is what `check-mirrors` already assumes for the shared documents, and that path
becomes an ordinary published dependency the day the OS goes public. The cost is stated in §83.3a:
**building Skynet Console needs both checkouts.**

**Skynet Console, not Skynet Station.** I had argued against it — §32.2 already calls the
robot-served page *the on-robot console* — and the owner chose it anyway, so the answer is not to
re-argue but to make the ambiguity unable to do harm. §83.0 fixes the vocabulary: *the on-robot
console* is the robot's page, *Skynet Console* is the application, and neither is ever bare "the
console" where both are in play. `main.rs` carries the same paragraph, positioned where somebody
would be about to write the second implementation of those six panels.

**Downloaded from the Hub**, which §83.3b now specifies: a signed artefact per platform, a published
digest beside it, and a version compatibility statement against `skynetd`'s local API — which is
§8.2's `min_os_version` problem in a new place and deserves answering rather than discovering. None
of it is built; the download surface, the release pipeline and the certificates are all outstanding,
and until they exist the application is a local `cargo build`.

### Verified

* `cargo build` / `cargo test` in `desktop/` — **5 passed**, 0 failed; `cargo fmt --check` clean,
  `cargo clippy --all-targets` **0 warnings**
* The window opens, scans, and renders the result. Confirmed from a DPI-correct capture, not assumed
* `cargo test --workspace` in `skynet-os` — **1,525 passed**, 0 failed, unchanged by the move
* `check-mirrors`, `check-contract-mirrors`, `check-golden-path`, `scale-report --check` — all pass
* The specification is mirrored and re-pinned; `skynet-os`'s stale `exclude = ["desktop"]` removed

### What it is not

**Not native widgets.** Tauri gives a native window, native menus and the platform's own webview —
WebView2 on Windows, WKWebView on macOS — in one binary with no bundled runtime. It is not WinUI or
SwiftUI, and §83.3's argument for that is unchanged and is not mainly about effort: the application's
core job is to display the robot's own HTML console, so a webview is mandatory in any design. Two
native shells would each still embed one, and there would be two of everything else.

**Not tested on macOS.** It is `std::net` and Tauri, both of which support it, and *supports* is not
*ran*. No macOS machine has been near this.

**Nothing is signed, and there is no bundle.** `cargo tauri build` has not been run; the icons are
placeholders generated by a script.

Nothing is committed and nothing is pushed, in either repository.

## 48. Two native applications, and the compromise that going native deleted

The owner's preference, stated plainly: *truly native for both operating systems*, of the kind
`MADOS` has — WinUI 3 and C# on Windows, SwiftUI and Swift on macOS, no shared UI layer. §47's Tauri
application is superseded.

I had argued against this in §83.3 and the argument was wrong in a way worth keeping, because it is
a specific category error rather than a matter of taste.

### The argument I made, and why it does not hold

> §83.2's whole design is to embed the page the robot serves, so a webview is mandatory in every
> possible design. Once one is in the process, building the chrome around it in the same technology
> is coherent, and two native shells would each still embed one with two of everything else.

Every clause is true. The conclusion does not follow. **A webview is required for one control, not
for the application** — `WebView2` and `WKWebView` are native controls that a native application
hosts, exactly as it hosts a list or a table. I had turned "this app must be able to display a web
page" into "this app must be a web page", which are different claims.

### What going native removed, which is the part I did not anticipate

§82.3 records a real compromise in `skynet-mdns`: it cannot hold port 5353, because `std::net`
exposes no `SO_REUSEPORT` and the port belongs to the system resolver on both platforms. So it
browses from an ephemeral port with RFC 6762 §5.4's QU bit, **never sees an unsolicited
announcement**, and therefore §82.3 states there is deliberately no watch mode — *"a watch built on
this transport would silently miss arrivals, and a watch that misses arrivals is worse to offer than
no watch"*.

Both native platforms expose DNS-SD **through the resolver that does own 5353**:

| | |
|---|---|
| Windows | `DeviceInformation.CreateWatcher` with a DNS-SD selector |
| macOS | `NWBrowser` with a Bonjour descriptor |

So both applications **subscribe** rather than scan. A robot that boots appears on its own; one that
is unplugged disappears; nothing needs pressing again. Going native did not work around §82.3's
compromise — it removed the condition that forced it, and the Rust browser's limitation turns out to
be a property of *not being the system resolver* rather than of the protocol.

`skynet-mdns` keeps its actual job: §82's **responder** half, which runs on the robot.

### What was built

| | |
|---|---|
| `desktop/windows` | WinUI 3, C#, .NET 9, XAML, MVVM. **Builds, runs, watches** |
| `desktop/macos` | SwiftUI, Swift package, `NWBrowser`. **Written, never compiled** |
| `desktop/shared` | §82's contract as data |

Both carry the same information architecture as §47's window, because that part was right: four
states kept apart — *never started*, *watching and nothing answered*, *watching and here they are*,
*could not watch* — and the fourth never folded into the second, since on a machine where the
resolver is disabled "no robots" and "I never managed to look" are indistinguishable from a list and
only one is a statement about robots.

### The four failures the Windows build took to reach a window

Recorded because three of them are invisible from the source and cost an hour each to somebody who
has not met them.

| | |
|---|---|
| `MVVMTK0045` | `[ObservableProperty]` over a **field** generates code CsWinRT cannot marshal into the WinRT projection XAML binds through. Over a **partial property** it needs a generator that did not run in this configuration. Resolved by writing the properties out over `SetProperty` — a dependency fewer, and each one raises the derived properties the interface binds, which a generator would not have known to do |
| `CS1503` in `MainWindow.g.cs` | A `Window` is not a `FrameworkElement`, and `x:Bind` with a converter generates code that takes one. The UI moved to a `ConsolePage`, which is the idiomatic shape regardless |
| `APPCRASH` in `combase.dll`, `0x80004005` | A hand-written `<ApplicationManifest>` **replaces** the SDK-generated manifest, which carries the registration-free COM entries that make WinUI's activatable classes resolvable in an unpackaged app. The only reason I had written one was PerMonitorV2 DPI awareness, which WinUI 3 already opts into |
| *"This application could not be started"* | An unpackaged WinUI 3 app needs the App SDK runtime installed unless `WindowsAppSDKSelfContained=true`. An operator downloading a bench tool from the Hub should not have to install a framework first |

**The third one was found by logging, not by reasoning**, and that is the same lesson as §47's. The
crash reported only `combase.dll 0x80004005` — a WinRT activation failure with no managed stack.
Four lines writing to a file from `App`'s constructor produced the actual exception on the next run:

```
Cannot find a resource with the given key: Show.
   at ConsolePage_obj1_Bindings.LookupConverter(String key)
```

`x:Bind` resolves a `{StaticResource}` converter through the **page's** dictionary when it
initialises bindings, and mine were declared one level down on the root `Grid`. The markup compiles
cleanly either way, which is what makes it worth a comment in the file.

### One contract, three implementations, one check

§82 now has **three** spellings — the Rust responder, the C# browser and the Swift browser — which
is precisely §78's shape, and this time the mechanism was built with the second and third rather
than after them.

`desktop/shared/skynet-service.json` states the contract as data.
`scripts/check-skynet-service.mjs` verifies the chain **specification → contract → three
implementations**: that §82 itself names every advertised key, and that each build's constants match
the file. Each application also re-checks it **at launch** and shows the result in its status bar,
because a developer running one locally has not necessarily run the script. The Windows window's
footer reads *"2 advertised keys, 6 refused, checked against the shared contract"*.

Mutation-checked: changing `keyApi` to `apipath` in the Swift constants fails the script with
`desktop/macos: keyApi does not match the contract`.

It is a build check rather than a review item because of the failure signature, and this is the
sentence the script prints: **a browser looking for the wrong service name finds nothing, and finding
nothing is the expected result today (§82.4)** — so a divergence would stay invisible for exactly as
long as it took somebody to notice a robot they could see was missing from a list.

### What replaces the shared Rust core, honestly

The Tauri build linked the OS crates directly. Two native applications give that up, and the
accounting is in §83.3a: discovery is **not** a loss (each platform's resolver is better for a
browser), the CLI is invoked as a subprocess either way, and **black box reading and signature
verification are a real loss, deferred rather than solved.** When §83.2's black box viewer is built,
both applications need `skynet-blackbox` and `skynet-ed25519` — through a C ABI both can call, or
reimplemented twice. That decision is not taken here, because taking it early on no evidence is how
a bad FFI boundary gets designed.

### Verified

* `dotnet build -p:Platform=x64` — **succeeds**, `TreatWarningsAsErrors`, 0 warnings
* The Windows window **opens and watches**: `WinUIDesktopWin32WindowClass`, native chrome, the Watch
  button flips to Stop, the empty state renders, the contract check passes in the footer
* `node scripts/check-skynet-service.mjs` — 6 checks, and it fails under mutation
* `check-mirrors` re-pinned; `cargo test --workspace` in `skynet-os` — **1,525 passed**, unaffected

### What is not true

**The macOS application has never been compiled.** There is no Swift toolchain on this machine. It
is written against `NWBrowser` and SwiftUI APIs that support what it asks for, its symbols were
cross-checked by hand, and *supports* is not *compiles*. Expect real errors on the first build. It
is listed in `desktop/README.md` as unverified rather than as done.

**Window sizing on a high-DPI display is unverified.** `AppWindow.Resize` takes physical pixels and
this environment has no desktop composition, so the captures cannot be fully trusted about geometry
— §47 is the record of what happens when I trust them anyway. The size is now a fraction of the work
area, which needs no DPI arithmetic and cannot be wrong about units, but it has not been seen on a
real desktop.

**§47's Tauri application is gone** from the tree, kept in this session's scratchpad rather than
deleted outright since it was never committed. Its information architecture survives in both native
builds; its implementation does not.

Nothing is committed and nothing is pushed, in either repository.

# Round six — 2026-09-06

Discovery's other half, and three defects only a running machine could show. Two of them were in
code written this round; the third had been in `skynetd` since §26.2 landed and would have left
every robot unable to seal a black box window from its second boot onward.

## 49. The tree would have failed CI before anything was built

`cargo fmt --all --check` had diffs in `skynet-contracts/src/admission.rs`, and `cargo clippy
--workspace --all-targets` reported **four** warnings in round four's in-flight code. CI sets
`RUSTFLAGS: -D warnings`, so the workspace on disk was one push away from a red build. The
forty-fifth session named them and left them deliberately — *"another round's in-flight code, and
`assertions_on_constants` and `large_enum_variant` want a decision from whoever wrote them"* — so
this round took the decisions rather than the suppressions:

| Warning | What it was told |
|---|---|
| `large_enum_variant` on `Intent` | **Allowed, with the reason in the file.** Boxing it puts an allocation per control cycle on §25's path, in a loop whose whole property is a deadline it makes every time. 640 bytes of fixed arrays is what `MAX_CHANNELS` costs, and the value lives on the stack |
| `needless_range_loop` in `run.rs` | Fixed. It indexed three arrays off one range and only the first needed the index |
| `assertions_on_constants` ×2 in `skynet-obs` | **Moved into `const` blocks**, which is the better answer rather than the quieter one: both operands are compile-time constants, so a slot that no longer fits its page now fails to *build* instead of failing a test somebody has to run |

## 50. §82's responder — the half that makes the other half worth having

`skynet-mdns` shipped in round five with the wire format, the TXT convention, `advertisement()` —
*the records a robot answers a browse with* — and **nothing that called it**. §82.4 said so plainly,
and §79 is this repository's record of what half a route costs.

Two modules, and the split is §82.4's own:

* **`skynet_mdns::responder`** — what to answer and when. Takes bytes and a clock reading, returns
  bytes and a destination; opens nothing. Probing, defending, announcing, the goodbye, RFC 6762 §6's
  20–120 ms delay for a shared record, and the conflict policy.
* **`skynetd_io::discovery`** — the socket. Binds 5353 on both families, joins the groups, polls
  non-blocking. A `Link` trait behind it, for the reason `launch::Launcher` is one: a link that
  refuses every send, a datagram from a robot that is not there, and a query arriving mid-probe are
  not states a real socket reaches on purpose.

`skynetd --lan <addr:port> [--advertise <label>]` starts it. **No `--lan`, no responder** — an
advertisement is an invitation to open a page, and a robot answering a browse while serving nothing
on the LAN sends an operator to a closed port and looks like a network fault. It advertises the
console's own address and never enumerates interfaces: the address to advertise is not a matter of
discovery, it is the one address §34.2 bound the console to.

### The conflict rule, and why it is not RFC 6762's

The RFC renames on conflict — `arm-01 (2)`. This does not. The label is what an operator reads off a
list before opening a console whose largest control is a **stop**, and two robots answering to one
name is a stop pressed on the wrong machine. So the second robot refuses, names the address of the
first, and says the fix. It is `skynet-lock`'s answer to a second writer, one subsystem over: the
holder is named, and nothing is stolen.

### Then two daemons on one link said otherwise

Both defects below passed every unit test in the crate, and no single-responder test could have
found either. They were found by running two `skynetd` processes in two containers on one bridge —
the cheapest two-machine link there is — and asking a third for a browse.

**The incumbent yielded.** A responder that treats every claim on its name as a conflict hands the
name to whoever booted last: the newcomer probes, the incumbent reads the probe as a rival and falls
silent, the newcomer hears nothing and claims. The browse then pointed at the robot that arrived
second, and the one that had been on the bench all afternoon was gone from the list — the exact
hazard the module header says it exists to prevent, implemented backwards. An established responder
**defends** (RFC 6762 §8.1's own rule); only one still probing refuses. `Rival::Established` and
`Rival::Probe` are that distinction, and the `defended` counter exists so a robot can say it
happened.

**And the probes went out together.** The first implementation queued three probes 250 ms apart and
sent whatever was overdue. `skynetd` preallocates a 67 MB black box before its first poll, so by the
time it polled, the whole schedule was in the past: three datagrams in one burst and the name
claimed in the same instant. **A responder that sends its probes together has not probed** — the
250 ms is not pacing, it is the window in which somebody else gets to say the name is taken, and a
robot that closes it wins every conflict against a robot that is already there. Probes are now paced
from the last one that actually *went*, one per pass, and the claim completes an interval after the
last of them rather than in the same breath.

A third thing the run showed: the daemon printed its claim **once**, at start-up. A robot that loses
its name at four in the afternoon because somebody powered on a second one has stopped being
findable, and the journal would have gone on saying it was advertising. It prints on every change
now, defences included.

## 51. The browse had no caller either

Nothing in the workspace depended on `skynet-mdns`. §83.3's desktop applications browse through
their platform's own resolver, so the Rust browse had lost its consumer without anybody writing that
down. `skynet discover` is the caller: the one command that answers the question every other command
in this CLI assumes, and the 49th the golden path counts. It prints the URL a browser can open, and
beside an empty list it prints how many sockets it managed to ask on, how many datagrams were
malformed and how many were about some other service — because "no robots answered" and "this
machine could not ask" are indistinguishable from a result set, and only one of them is about
robots.

## 52. A thread, and the measurement that justified it

§33.2's rule is one thread and one loop, and what it is against is a scheduler in front of a
reconcile and an accept. The responder is not that: it owns its sockets and its state and shares
nothing with the loop. What forced it out of the loop was a **deadline**, measured rather than
assumed — at `--interval 1000` a robot took **six seconds** to take its own name instead of 750 ms,
because the poll cadence is bounded by how long a reconcile pass takes, and a pass stages packages,
drains a queue and uploads a window. A browse waits two seconds. A responder polled between passes
answers a browse that has stopped listening.

It ticks every 20 ms, which is §6's own floor for an answer to a shared record.

## 53. The robot could not seal a window after its first restart

The end-to-end script had been aborting mid-way — `set -e`, and a non-zero exit from `skynet
blackbox seal`. The message was exact:

> this process attached to a black box another process created, and the creator is the one that
> seals it (§26.2)

§26.2's lock is right; the rule built around a *stale* one was wrong, for a reason about how a daemon
stops rather than about locks. **`skynetd` is stopped by a signal and installs no handler**, so
`Drop` never runs and the lock file is left behind on *every* restart. The fifty-eighth session
argued that a stale lock must be named rather than stolen — *"on the day the first one was slow
rather than dead"* — and that reasoning is sound; what it missed is that a stale lock is not the
exception here. It is the state every robot is in from its second boot. The next daemon attaches, an
attacher may not seal, and the black box goes on recording into a ring nobody can ever close.

The sentence is sharpened rather than reversed: **a lock is never taken from a process that
exists**, and existence is asked of the kernel. The note now carries the holder's pid *and its start
time* — field 22 of `/proc/<pid>/stat` — because a pid alone is not an identity: numbers are reused
within hours and reset at a reboot, which is exactly the case in question.

The first attempt compared `/proc/<pid>/comm` with the `who` in the note, and it was wrong in the
dangerous direction: `who` is a label a caller picks, `comm` is the executable's name, and where they
differ — every test in this crate, and any program embedding a recorder — a live holder reads as
gone. Six tests turned red on Linux and passed on Windows, which is what said so. `(pid, start
time)` cannot be fooled by a caller's choice of label.

A reclaim is **reported**: `Writership::reclaimed_from` carries the dead holder's note up to
`skynetd`, which prints it. A directory that changed hands is a fact an investigator reading that
recording is entitled to, and a silent takeover is what the original rule was right to fear.

## 54. A grant made at the robot, and the record that was not kept

With the seal fixed, the script ran to the end for the first time — and one check still failed, one
that had been failing long enough that nobody had seen the sections after it. `grants add` answered
`granted`; `grants remove`, moments later, answered `not_held`.

The audit log said what had happened. The API had written `capability.grant` for
`openrobot/holder:sensing.camera.rgb`, and `grants.json` held only `manipulation.grasp` — the one the
desired state names. **The reconciler had converged the operator's grant away on the next pass**,
which is §33.3 working exactly as specified, and it had left **no audit row at all**.

Two writers, one file, and the asymmetry is the defect: a grant made at the robot is permanent
evidence, and the taking of it went to the ring alone, which holds minutes. Eleven months later the
log answers *when did this robot start letting that package move an arm* and cannot answer when it
stopped. `audit_grant_changes` is the fix — every applied grant or revocation the reconciler
performs, attributed to **"desired state (§11)"** rather than to a person, because that is the honest
answer to *who did this* and it cannot be mistaken for a credential label.

**What is not fixed, because it is a decision rather than a defect:** the console offers a grant
button that says `granted` about a permission that will not survive the next pass. Converging is
correct; advertising the grant as done is not. The three shapes are to refuse it, to write it into
the desired state, or to say plainly that it lasts until the next pass — and choosing between them
is the owner's call, not this round's. It is written into the e2e as the behaviour it currently is.

## Verified

Everything below ran on this machine; the two-machine parts ran between containers on one bridge.

| | |
|---|---|
| `cargo test --workspace` | **1,557 passed**, 0 failed (1,525 at the start of the round) |
| `cargo fmt --all --check` | clean, from a tree that was not |
| `cargo clippy --workspace --all-targets` | **zero warnings**, from four |
| `crates/skynetd-io/e2e.sh` | **all checks passed** — the first complete run, including the seal that had been aborting it and §82's new stage |
| `skynet discover` against a live `skynetd` | finds the robot, and the URL it prints serves **200 text/html** |
| two robots, one label, one link | the incumbent keeps the name and logs `1 defended`; the second refuses, names `172.21.0.2` and states the fix; a third machine's browse resolves the label to the **first** robot |
| a daemon with no `--lan` | advertises nothing, and says nothing about mDNS |
| a second daemon on one machine | reports that 5353 is held, and its console is unaffected |
| corpus | 1,116 of 1,116 in both editions |
| `check-mirrors`, `check-golden-path`, `scale-report --check`, `check-skynet-service` | pass — the golden path counts **49** commands and §73 says 115,000 lines |

## What this round leaves open

* **The goodbye has no caller.** `farewell()` builds RFC 6762 §10.1's withdrawal and `skynetd` never
  sends one: it is stopped by a signal, and installing a handler needs `unsafe`. A robot switched off
  stays in a browser's cache until the 120-second TTL expires. Named in §82.4 rather than left to be
  found.
* **A grant made at the robot does not survive a pass**, and the console does not say so. §54 above.
* **The browse still cannot hold 5353**, so there is no watch mode and an unsolicited announcement is
  never seen. That is a property of the operator's machine, where Avahi and `mDNSResponder` already
  hold the port — the responder holds it on the robot, where it is free.

# Round seven — 2026-09-06

## 55. §25's hand-off: two processes, one ring

The row §80 has carried since the policy runtime was built:

> **No two processes have shared a ring.** §25 has the runtime create the ring and hand the
> descriptor to the sandboxed policy. `skynetk` creates its ring in its own address space and says
> so on every run; `skynetp` does the same; neither offers a descriptor and neither adopts one.

Both ends of the mechanism already existed and had never met. `skynet-ring-shm` creates a sealed
`memfd`, maps it, and its `fd()` carries a comment describing the send that nobody had written —
*"a caller sending it over a socket with `SCM_RIGHTS`, which duplicates it into the receiver, needs
nothing else"*. `Mapping::adopt` takes a descriptor and maps it writable or read-only. What was
missing was the thirty lines between them.

### A socket, not inheritance, and the reason is §33.5

A child inherits an open descriptor across `exec`, which needs no control messages and is what this
crate's own cross-process test uses. It is not available here: §33.5 starts a policy as a **systemd
unit**, so the process that creates the ring is not the process that starts the one which will write
it — the daemon starts that, and §33.1 keeps the daemon off the control path entirely. Two processes
started independently can only meet at a name in the filesystem.

`skynet_ring_shm::pass` is `sendmsg`/`recvmsg` with one `SCM_RIGHTS` control message, and it stays in
that crate for the reason the crate's header already gives: it is the one file a reviewer reads when
they ask whether the command path is memory-safe, and it is short. `struct msghdr` is declared as a
`repr(C)` struct with real field types rather than a hand-counted byte array — `socklen_t` is 32 bits
and the pointer after it is 64 on an LP64 target, and writing that hole in by hand is how a struct
is right on one architecture and wrong on the next. One byte of payload rides with the descriptor,
because an implementation may discard ancillary data attached to an empty message.

### The two halves

| | |
|---|---|
| `skynet_kernel_host::handoff::Port` | creates the mapping, binds the socket, and **offers** the descriptor to whatever connects. Non-blocking and level-triggered: a control loop that stopped to wait for a producer would miss its deadline because nothing was commanding it, which is exactly the state §7.6 calls normal |
| `skynetp --ring-socket <path>` | connects, receives, and maps it **writable** — while the runtime's own mapping carries no `PROT_WRITE`, so *the policy is the only writer* is enforced by the page tables rather than agreed between two programs |

`skynetk run --policy-socket <path>` is the flag that turns it on. Without it the ring stays private
and the binary says so, which is the honest state of a robot running no policy.

**Neither side falls back.** A policy told to join a ring and quietly publishing into its own memory
is a producer that looks like it is commanding and is not; a runtime asked for a shared ring and
running with a private one is a robot that looks commandable and is not. Both refuse and name what
is missing.

### What it did on a real machine

Two processes, a mock RHAL driver, and a trajectory backend:

```
skynetk run --driver libmock_robot.so --tier host --velocity-max 2.0 --jerk 0.1 \
            --watchdog-ms 500 --cycles 300 --hz 100 --policy-socket /tmp/policy.sock
skynetp run --hz 100 --cycles 250 --channels 2 --backend trajectory \
            --setpoints "0:0.001,0.001;100:0.002,0.002" --ring-socket /tmp/policy.sock
```

```
policy     took the ring (1 so far)
cycles          300
interventions   199
idle            101 (the ring was empty)
read failures   0
write failures  0
stops           0
overruns        0 — cycles that missed the 10.00 ms period
```

101 idle cycles before the policy connected, then 199 commanded ones from another process, through
the eight guards, to a driver. The interventions are the rate limiter clamping every step, which is
the kernel doing its job and being visible about it.

The first run of that pair had `stops 299` and `write failures 298`, and it was **not** a defect in
the hand-off: without `--jerk` the rate-limit guard has no input, reports itself unavailable, and the
kernel refuses to command. A guard with nothing to enforce refusing the command is the fail-closed
behaviour §7 is for, and seeing it happen on a real command from a real second process is worth more
than the passing run.

### Made permanent

`crates/skynet-policy-host/tests/handoff.rs` spawns the **real `skynetp` binary** and acts as the
runtime, so what is asserted is a mapping made by one process and written by another rather than a
protocol exercised in one. Two cases: a policy that fills the ring, and a policy pointed at a socket
nobody is listening on, which must refuse rather than publish into itself.

Three headers said the descriptor was never handed over — `skynetk`'s, `skynetp`'s, and §73's IPC
row — and all three are now the other sentence. The one that was hardest to write is `skynetp`'s,
because it had to keep the argument for *removing* `--ring-fd` while explaining why the option is
right this time: half a handshake is worth less than none, and what changed is that the other half
exists.

### What it does not close

**The observation ring has no hand-off.** A policy in its own process can command and cannot see:
`skynet-obs` carries the body the other way and both hosts still create theirs privately. That is
the next thing §73's row is waiting on, and it is the same shape as this one.

**It is not a sandbox.** §8's permissions and §40's cgroup limits are the daemon's to apply to the
unit it launches. This hand-off changes nothing about them — what it changes is that the two
processes exist at all.

## 56. The other direction, and the loop closing across the boundary

§55 left the observation ring behind, and §73's row said so: *"a policy in its own process can
command and cannot see."* That is not half a feature, it is the wrong half — a policy that can
command a body it cannot observe is open-loop by construction, and every interesting thing §54's
skills will do is closed-loop.

### One message, two descriptors

`SCM_RIGHTS` carries a vector, so both rings cross in **one** control message: a policy receives both
or neither. Two messages would make the failure reachable where a policy has the commands and not
the body, which is precisely the blind commanding above. `sys::Control` grew from one `int` to
`MAX_FDS`, `cmsg_len(n)` computes what the message claims, and the size assertion is now
`CMSG_SPACE(sizeof(int) * MAX_FDS)` computed from the word size rather than written out — the test
exists because a buffer a byte short is `EINVAL` and a byte over is a claim about data that is not
there.

The directions are opposite and both are enforced by the page tables rather than agreed:

| | the runtime | the policy |
|---|---|---|
| command ring | read-only | **writes** |
| observation ring | **writes** | read-only |

### A mapping that holds no ring

`Mapping::create` writes a §25 command-ring header and validates it, which is exactly what the
observation ring must not have — §25 gives it different magic on purpose, so that *"a consumer built
for commands which opened one is refused at the first word rather than reading joint positions as
setpoints with the drives live."* `create_raw` / `adopt_raw` map the memory and have no opinion about
what is in it, and `words()` became public so `skynet-obs` can lay its own ring over one. The
alternative was a second `mmap` in the crate that owns the other protocol, which is the thing this
crate exists to prevent there being.

### Two callers that did not exist

**`Host::step_observing` had none.** It has taken an observation producer since `skynet-obs` was
written, and `skynetk` called `Host::step`, which passes `None` — so the producer API had been
built, tested and never once called by a binary. It is called now, whenever a ring was handed over,
and not otherwise: a robot with nobody to read the body does not publish it.

**`Approach` had none either.** The one backend whose command on cycle *n* is a function of what the
body did on cycle *n − 1* was written with the observation ring and could not be selected from a
command line, because until the ring crossed a process boundary there was nothing outside a test for
it to close on. `--backend approach --target <values> --gain <fraction>` is that selection, and it
**refuses to run without `--ring-socket`**: a closed-loop backend with no body holds on every cycle,
and a policy holding because it was never given anything looks exactly like one that decided to.

### What it did

```
skynetk run --driver libmock_robot.so --tier host --velocity-max 2.0 --jerk 0.5 \
            --watchdog-ms 500 --cycles 400 --hz 100 --policy-socket /tmp/policy.sock
skynetp run --hz 100 --cycles 300 --channels 2 --backend approach \
            --target 0.4,-0.3 --gain 0.05 --ring-socket /tmp/policy.sock
```

**300 turns, 300 commanded, 0 held** — a `Hold` is what `Approach` produces when it cannot see the
body, so the count is the assertion: every cycle it read a body published by the other process. On
the runtime's side, 400 cycles, 101 idle before the policy connected, 299 interventions from the
rate limiter, no write failures, no stops, no overruns.

And the arithmetic is a test rather than a demonstration.
`a_command_from_another_process_is_a_function_of_the_body_this_one_published` publishes a joint at
0.0, spawns the real `skynetp` with `--target 1.0 --gain 0.5`, and asserts the command that comes
back is **0.5**; then publishes 0.5 and asserts the next one is **0.75**. Half the remaining
distance, computed in another process, from a number this one wrote.

## 57. Clippy on Windows is not the gate

The tree was clean under `cargo clippy` on this machine and had **two warnings on Linux** — a
redundant closure and a needless `mut`, both in code that is `cfg(unix)` and therefore invisible to
the lint on the platform it was written on. CI runs on Linux with `-D warnings`, so "clippy is
clean" had quietly become a statement about half the workspace. Both are fixed, and the lesson is
the check rather than the warnings: **a lint result is only as broad as the target it ran on**, and
this workspace has `cfg(unix)` in five crates now.

## Verified

| | |
|---|---|
| `cargo test --workspace` | **1,557** on Windows, **1,621** on Linux, 0 failed |
| `cargo clippy --workspace --all-targets` | zero warnings **on both** |
| `cargo fmt --all --check` | clean |
| `crates/skynetd-io/e2e.sh` | all checks passed |
| two processes, closed loop | 300 commanded, 0 held, 0 write failures, 0 overruns |
| corpus | 1,116 of 1,116 in both editions |
| `check-mirrors`, `check-golden-path`, `scale-report --check` | pass — §73 now says 116,000 lines |

## What is still open after this

* **A sandbox.** The policy is another process; it is not yet confined by §8's permissions or §40's
  cgroup limits. Those are the daemon's to apply to a unit it launches, and nothing here changes
  them.
* **The daemon does not start either half.** `skynetd` launches packages, and wiring §54.4's skill
  lifecycle to a `skynetk`/`skynetp` pair is the next thing this makes possible rather than
  something it did.
* **Vision.** The observation ring carries the body, not the cameras. A 4 KiB slot does not hold an
  image, and a buffer pool with handles and a lifetime rule is a larger design that this does not
  start.

## 58. How the two halves find each other on a robot

§55 and §56 built a hand-off that works when somebody types two commands. A robot has nobody typing,
and §33.1 says exactly why that is not a detail:

> The kernel and the RHAL driver survive a `skynetd` restart. They are separate processes with
> separate lifetimes; **the kernel is started by systemd before `skynetd`** and does not depend on
> it.

So neither half starts the other. The runtime is a systemd unit at boot; the policy is a package the
daemon launches when a desired state — or §54.4's skill lifecycle — says to. The only thing they can
share is a name in the filesystem, and the only party that knows both the layout and which package
is allowed to command is `skynetd`.

**`Layout::policy_socket`** is that name: `<root>/run/policy.sock`, in `/run` rather than `/data`
because it is a rendezvous rather than state — it exists while something is listening and means
nothing across a reboot.

**The environment, not the command line.** A package declares its own `runtime.args` (§8), and a
supervisor appending to them would be the daemon editing somebody else's command line. So `Unit`
gained an `env`, `systemd-run` gets `--setenv=` before the program — after it, a `--setenv` is an
argument the package receives and does nothing, which the test asserts by position — and `skynetp`
reads `SKYNET_RING_SOCKET` and `SKYNET_RING_CHANNELS` when its flags are absent. **A flag always
wins**: an operator running it by hand is the one case where the environment may describe a
different robot than the one they mean.

### Who is told, and the reason it is not decided here

`shape::rendezvous` answers it, and it defers to `shape::supervising` — the same function that
already decides which package sized the black box:

| | |
|---|---|
| the commanding package | the socket path, and the channel count §6.2's plan resolved |
| every other package | **nothing.** The address of a command ring is not something every unit on a robot needs to know, and §8's rule is that a package gets what it declared and no more |
| two commanding packages | **nothing, to either.** The daemon refuses to choose between them for the recording's shape, and handing the ring to whichever the loop iterated first would be the silent arbitration §25.2 describes as two policies driving one elbow |

The channel count travels with the path because a policy cannot infer it. §6.2's plan is resolved
against *this* body, and a producer that guessed would have a mask of the wrong width.

That check moved out of `parse` into `check` on the way, and the move is the point: validation that
only ran over the command line would hold an operator to a rule and let a policy the daemon started
past it.

### Run

```
skynetk run --driver libmock_robot.so --policy-socket /tmp/policy.sock ...
SKYNET_RING_SOCKET=/tmp/policy.sock SKYNET_RING_CHANNELS=2 \
  skynetp run --hz 100 --cycles 150 --backend approach --target 0.2,0.1 --gain 0.05
```

**150 turns, 150 commanded, 0 held.** No ring flags at all: a policy that found the runtime the way
a package the daemon launched would, and closed the loop on the body it was given. Four
cross-process tests hold it — the ring, the refusal with no runtime listening, the arithmetic of the
closed loop, and this one — plus three in `shape` for who is told and who is not.

## What round seven leaves

* **The daemon does not launch the runtime.** `skynetk` is a systemd unit at boot per §33.1, and
  this repository ships no unit file. That is packaging rather than code, and it is the next thing
  between here and a robot that boots into a commanded loop.
* **A skill still does not start a policy.** §54.4's `Effect::Start` starts a *package*, and a
  policy package that speaks §25 is now startable — what is missing is a package that does, which
  is a vendor's to write and this repository's to demonstrate.
* **The sandbox.** Two processes, no confinement between them yet.

## 59. The sandbox at launch — §8.3 stops being a record and starts being true of the process

The item is the tail of §80's ring row: *"What is still one process is a sandbox: §8's permissions
and the cgroup limits §40 resolves are the daemon's to apply to the unit it launches."* Audited
before building, because the sentence names two mechanisms and they turned out to be in different
states:

* **§40's half was already shipped.** `Limits::from_manifest` reads the declared budget into
  `MemoryMax`/`CPUQuota`/`TasksMax`, `Cpus` subtracts the isolated RT core and refuses an empty
  allowance, and `runtime.rs`'s activate path wires both into every `Unit`. Nothing to build.
* **§8.3's half had never started.** Every unit — policy included — launched with the ambient
  privilege of the daemon's systemd context. Admission records `isolation: "sandboxed"` and the
  kernel's `mode_gate` enforces the grants, but no line of code confined the process itself.

Built in `crates/skynetd-io/src/launch.rs`:

* `confinement_properties(kind)` — pure, so it is asserted on any machine: seven directives for
  `policy`/`service`/`skill` (`NoNewPrivileges`, `ProtectSystem=yes`, `ProtectKernelTunables`,
  `ProtectKernelModules`, `ProtectControlGroups`, `RestrictSUIDSGID`, empty
  `CapabilityBoundingSet`), none for `driver`. The driver's exemption is §8.3's own; its
  compensating controls are §19's signature gate and the limits/cpuset it still carries.
* Wired into `SystemdLauncher::command_line`, after the cgroup properties — so the argv systemd
  sees is the argv the tests read, and a sandbox only systemd ever saw would be one nobody read.
* Three tests: policy confined + driver native on the real argv (limits intact in both, proving
  the sandbox is not a second budget); all non-driver kinds identical; and the restraint pinned —
  no `PrivateTmp`/`ProtectHome`/`strict`, no device policy, no syscall/address filters.

The omissions are the design, not a shortfall of the session, and each names its unblocker.
`PrivateTmp` and `ProtectHome`/`strict` would fence stores under `/tmp` (which `e2e.sh` uses) or
a home directory (which a development machine uses) — a sandbox that breaks the layout it
confines is a sandbox operators remove. `DevicePolicy` waits on §40.3's accelerator allow-list:
with no declared device set, `closed` refuses every GPU policy rather than the second one.
Syscall and address-family filters wait on a measured allow-list; §64 makes a wrong filter a
defect rather than caution. The module header says all of this, so nobody mistakes the
systemd confinement for §23.4's OCI sandbox, which stays the designed end state.

`conformance.rs`'s `safety.policy.isolation` stays **skipped**, with its reason rewritten: the
old reason ("nothing on this edition sandboxes a package") is now false, but the check as
specified — a forbidden hold performed and refused — still has no machine probe, and the
launcher asserting its own argv would be an edition property, which is what
`safety.kernel.parity` is skipped for being. The reason names the probe that would execute it.

Two corrections to the record above, found while auditing:

* "Round seven" says this repository ships no unit file. It now ships two:
  `packaging/systemd/skynetd.service` and `packaging/systemd/skynetk.service`, with the §33.1
  ordering (`After=`/`Before=`, neither `Requires=` the other) and the §40.2 core pinning. That
  bullet is closed; the old text stands as what was true when written.
* The spec document is untouched by this session. §80's sandbox sentence is now half-stale, and
  fixing it means editing `docs/skynet-os-architecture.md`, which mirrors BotMarket byte for
  byte (`docs/README.md`: edited in both repositories or in neither). BotMarket is not present
  on this machine, so the annotation waits for a pass that has both trees — noted here rather
  than diverged there.

### Run

```
cargo test --workspace — 1,628 passed, 0 failed (baseline before the change: 1,625; the +3 are
the new confinement tests, confirmed by stash-and-rerun rather than by arithmetic)
cargo fmt --check --all — clean
cargo clippy --workspace --all-targets — clean
```

`node tools/check-*.mjs` not run: no `node` on this machine. The change touches no mirrors,
no corpus, and no golden-path surface, so there is nothing for them to re-pin — stated, not
assumed.

## What round eight leaves

* **Device confinement (§40.3).** The accelerator allow-list at admission first, then
  `DeviceAllow` derived from it. This order or the sandbox refuses the first GPU policy.
* **A probe-based isolation check.** Something that reads a launched unit's actual confinement
  (e.g. `systemctl show` on a `skynet:*` unit) rather than the argv that requested it — that is
  what flips `safety.policy.isolation` from skipped to executed.
* **Vision.** Unchanged: the observation ring carries the body, not the cameras; frames need a
  buffer pool with handles and a lifetime rule.
* **A skill starting a policy.** Unchanged: `Effect::Start` starts a package; the demo package
  that speaks §25 is still the missing piece.

## 60. A skill starts its unit — through the daemon, on the ordinary path

Round seven's skill bullet had two halves and only named one. The named half — a demo policy
package that speaks §25 — is a vendor artifact and still missing. The unnamed half — whether
the daemon, given such a package, actually starts it — had never been exercised past the
contract boundary: the skill suite proves the lifecycle *emits* `Effect::Start`, and nothing
proved `Daemon::perform` *performs* it through the same `start()` a desired state uses. A
lifecycle whose effects nobody performs is a second launcher waiting to happen, so this session
proved the wiring rather than building another path.

The test is `a_skill_entering_running_starts_its_unit_and_leaving_it_stops_it`, through the
real `Fixture` daemon with a `RecordingLauncher` — no new harness, the one the API suite
already uses:

* `POST /skills` invokes `acme/pick` (202, `idle` — the existing acceptance, reused as setup).
* Two `advance_skills` move `idle → preconditions → acquiring` with nothing started: a unit
  started before `running` would be a skill running that never cleared acquiring.
* The third advance enters `running` and the daemon performs the `Start` on the emitting pass —
  `launcher.calls` holds `start acme/pick`, via `start()` with its budget, cpuset, rendezvous
  env and (as of §59) confinement, because there is no second launcher to perform it differently.
* The unit is marked exited, the observation is re-taken with `daemon.observe` rather than
  assumed — the lifecycle reads the process table the supervisor changes, which is why
  `advance_skills` runs after the reconciler in `main.rs` — and two advances later
  (`verifying → terminal`) `launcher.calls` holds `stop acme/pick`.

Passed on the first run, which is evidence about the code rather than the test: the path was
already correct, it was only unwitnessed.

### Audited and deliberately not built

* **Daemon runtime launch.** Re-checked against the tree rather than the old bullets: `skynetk`
  is a systemd unit at boot (`packaging/systemd/skynetk.service`, §33.1 ordering, §40.2 pinning),
  the daemon never starts it by design, and the rendezvous (`SKYNET_RING_SOCKET`,
  `SKYNET_RING_CHANNELS` to the one commanding package, nothing to two) is covered by
  `shape`'s tests. There is no gap here any more — the "daemon does not launch the runtime"
  bullet died when the unit files landed, alongside the §59 correction.
* **Vision buffer pool.** `skynet-obs`'s header states the design (handles plus a lifetime rule)
  and the reason it is not started: no capture backend exists (V4L2/GenICam are hardware-blocked)
  and no consumer exists, so a pool written now would fix frame sizes, rates and handle
  widths against nothing — the unmeasured allow-list mistake §64 exists to prevent, in a new
  crate. The unblocker is the first capture backend with a measured frame contract, not another
  session of guessing at it.
* **Isolation probe.** The parser half without the wiring would be half a route (§79.7's
  `skynetp --ring-fd` lesson), and the wiring half needs three things this machine has none
  of: a systemd bus to read `systemctl show` from, the CLI surface to carry the observation
  into `conformance::run` (which is pure today), and a robot to show the executed outcome on.
  It stays open with its unblocker named: a systemd robot to capture the first real `show`
  fixture from.
* **Device confinement.** Still blocked where capacity.rs says it is: `vram_mb` stays `None`
  because accelerator memory needs a vendor runtime this daemon does not link. No admission
  number, no allow-list, no `DeviceAllow` — the order from round eight stands.

### Run

```
cargo test --workspace — 1,629 passed, 0 failed (1,628 before; +1, the skill-unit test)
cargo fmt --check --all — clean
cargo clippy --workspace --all-targets — clean (0 warnings)
```

## What round nine leaves

* **The demo skill-policy package.** The daemon side is now witnessed end to end; what is
  missing is a real `.sky` skill package whose entrypoint speaks §25 (the `skynetp`-in-a-blob
  pattern), exercised in `e2e.sh`'s systemd-gated section on a machine that has systemd.
* **A probe-based isolation check**, unblocked by a systemd robot to capture the first `show`
  fixture from.
* **Vision**, unblocked by the first capture backend with a measured frame contract.
* **Device confinement**, unblocked by a vendor runtime that reports accelerator memory.

## 61. The reconciler killed every skill on its second pass — and the demo that proved it

Round nine's first item is built: `e2e.sh` §9 installs a real skill package (`acme/wave`,
`skynetp-in-a-blob` entrypoint, bounded 60-second `hold`), activates it through desired state,
invokes it over the socket, and watches the daemon start it. Building it found a real bug
underneath the demo, which is why the demo took the session rather than the hour.

### The bug

`Daemon::perform` starts a skill's unit through the ordinary `start()`, and on the next pass
the level-triggered reconciler stops it: desired state never names the package, so
`!should_run` is true for ever, and `StopPackage` is emitted. Every skill on a looping daemon
lives exactly one reconcile interval. The contract suite could not see it — emission and
performance are tested, the plan's view of a skill-started unit was not — and neither could
anything else, because no test had ever driven all three in one room.

Red proof, at the plan boundary where it belongs:
`a_unit_backing_a_live_skill_is_not_stopped` failed with the signature in full —
`[StageOsImage {...}, StopPackage { id: "acme/pick" }]`.

### The fix

`Actual` gains `live_skills: BTreeSet<String>` — packages with a live (non-terminal)
invocation, read off the daemon's own skill table in `Daemon::observe`, the same discipline
as every other field. The plan's stop rule spares the set; the set empty again, a leftover
unit is reaped rather than supervised, which is what collects the unit when the daemon died
mid-skill and no `Stop` effect will ever come. Two writers, one story — finding 111's rule,
applied to the exact pair it was written about.

Deliberately not a signature change: the plan stays a function of desired and actual, and
the eleven spread-literal tests in the `skynetd` suite needed no touch at all — one
exhaustive literal took one line. The spare is also why `Daemon::observe` fills the set
rather than `observe::observe`: the free function takes store, launcher and slots, and an
observer inventing membership from nothing would be the supervisor trusting its own record.

Covered twice: the plan test above (spared while live, reaped when over) and
`reconcile_spares_a_unit_while_its_skill_is_live`, which drives the fixture daemon in loop
order — reconcile, advance, three times — with the skill in desired state as `installed`,
and asserts the pass holds no stop. The desired entry matters: without it the pass collects
the activation (§37.3) before the skill starts, and the test would prove nothing. The e2e
carries the same entry for the same reason.

### What the demo proves live, and what it honestly cannot

`e2e.sh` §9 passes end to end on a systemd machine: stages, activates (desired alone does
not start it), invokes (202), reaches running, the unit's trace file appears
(`$ROOT/skill-ran`, touched by the unit's own process — presence means the daemon really
started this package, not that it reported doing so), lands in terminal with `robot_stopped`,
and the episode is queued. The non-systemd half mirrors §3's contract: loud refusal in the
log, the trace file absent, terminal anyway.

Steady running is not asserted, and the reason is the design: this robot has no kernel, so
the daemon's operating state is `safe`, and `robot_stopped` aborts every running skill
within two passes. A skill that stayed running here would be the interlock failing. The
spare clause therefore has no live witness — on a kernel-less robot the interlock fires on
the same passes the reconciler would — and the log states the division plainly: the suite
carries the arbitration, e2e carries start-performed, interlock-named, episode-queued. The
steady-running witness waits on a kernel in the loop (skynetk plus the mock driver, §58's
machine, inside e2e's systemd section).

### The stash experiment that corrected itself

The new e2e was run with the fix stashed, expecting the reconciler kill to fail it. It did
fail — and the identical failure with the fix restored showed the failure was the interlock
both times, not the kill. Red without green is a story, not a proof: the experiment's value
was revealing the confound (and the reframed assertions), not reproducing the bug. The
reproduction stands one layer down, where no kernel confuses the question: the plan test's
red run above.

### Run

```
cargo test --workspace — 1,631 passed, 0 failed (1,629 before; +2: the plan spare test and
the daemon spare test)
cargo fmt --check --all — clean
cargo clippy --workspace --all-targets — clean
sh crates/skynetd-io/e2e.sh — all checks passed (systemd branch: unit ran, robot_stopped
named, episode queued)
```

## What round ten leaves

* **A kernel in the e2e loop.** skynetk plus the mock driver inside the systemd section, so a
  skill can hold `running` on a live machine and the spare clause gets its live witness. Until
  then the suite carries the arbitration and e2e carries everything around it.
* **A probe-based isolation check**, unblocked by a systemd robot to capture the first `show`
  fixture from.
* **Vision**, unblocked by the first capture backend with a measured frame contract.
* **Device confinement**, unblocked by a vendor runtime that reports accelerator memory.

## 62. The sandbox, read back off a real unit

§59 built the confinement at the argv layer and noted, honestly, that no systemd machine had
ever been asked what landed. This machine is one — `systemd-run --user` works here, and §3
starts a real policy unit — so `e2e.sh` §3 now reads the unit back with `systemctl --user
show` and asserts ten properties: the six confinement directives, the empty capability set
(line-exact, because a substring match would pass `cap_anything`), the declared 64 MB budget
arrived as `MemoryMax=67108864` (bytes, so this asserts the budget became the limit rather
than that a limit exists), the fixed `TasksMax=128`, and a non-empty `AllowedCPUs` whose
value stays deliberately unpinned — the core set is the machine's, and pinning it would fail
the check on every other machine. What is asserted about the cpuset is the failure mode:
an omitted cpuset is every core including the kernel's (§40.2), so presence is the property.

No Rust changed in this session. The mechanism was probed first with a throwaway transient
unit (empty `CapabilityBoundingSet=` accepted and reported empty, `MemoryMax=64M` reported
as bytes), then pinned against the daemon's own unit. All ten pass; the full e2e is green.

Two audit notes from the same pass, for the record rather than the build:

* The operating-state inputs are unwired territory. Nothing outside contract tests ever
  reports `RhalState::Present`, `KernelState::Live`, or homing complete, so
  `refresh_operating_state` derives from post plus defaults — RHAL lost means `safe` on
  every robot, always. That is why no skill can hold `running` on any current machine, and
  why round ten's "kernel in the e2e loop" item is misstated as written: a running skynetk
  alone changes nothing observable, because the daemon's conditions still say the kernel is
  down. The item should read "kernel liveness into the daemon's conditions, then skynetk in
  the e2e loop" — which crosses §33.1 (the kernel/daemon separation) and is a design task,
  not a gap to fill in passing. It is restated that way below.
* The isolation probe from round ten is half-delivered by the above: the live evidence it
  wanted (confinement lands on real units) now exists in e2e. What remains is the
  conformance flip — `safety.policy.isolation` from skipped to executed — which needs the
  observation carried into `conformance::run` (pure today) plus a robot to show the executed
  outcome on. Smaller than it was; still open.

### Run

```
cargo test --workspace — 1,631 passed, 0 failed (no Rust changed; confirms the suite)
sh crates/skynetd-io/e2e.sh — all checks passed, including the ten new unit-property reads
```

## What round eleven leaves

* **Kernel liveness into the daemon's conditions, then skynetk in the e2e loop.** Restated
  (see above): without the first half, the second changes nothing observable. Design work —
  how the daemon may know the kernel is live without violating §33.1.
* **The conformance flip for isolation.** Carry a unit-confinement observation into
  `conformance::run` and execute `safety.policy.isolation` against it; e2e already proves
  the underlying fact.
* **Vision**, unblocked by the first capture backend with a measured frame contract.
* **Device confinement**, unblocked by a vendor runtime that reports accelerator memory.

# Round twelve — 2026-09-07

## 63. §47.6's pool: the specification and its reference did not agree, and no test could see it

The audit that opened this round listed §47.6 as *specified 2026-09-06, awaiting the editions*:
a TypeScript reference in the Hub, six unit tests beside it, and no Rust, no Python and no
corpus. Filling that in looked like transcription. It was not, and the reason is worth the
section.

### What the corpus was written against

§31.4's third rule is *a new case lands in the corpus before the fix lands in any edition*,
and it is the rule that decides what this round found. The ten cases in `corpus/media/pool.json`
were written and **generated against the unfixed reference**, so the first thing this repository
recorded about §47.6 was what the reference actually did rather than what the section said it
did. Three of the ten came back wrong, in the expected bytes, where a diff could be read:

| Case | What the reference produced |
|---|---|
| `four-frames-into-three-slots` | `slot 0` four times — generations 1, 2, 3, 4. Slots 1 and 2 never written |
| `the-reader-that-looks-then-takes` | the reader's `acquire` **refused `stale_handle`**, in a three-slot pool with two slots free |
| `released-slots-go-oldest-first` | the second publish had already destroyed the first frame, before any reader existed |

§47.6 says `publish` *"reuses the oldest released slot"*. The reference took
`findIndex(slot => !slot.acquired)` — the **lowest-index** slot nobody is currently holding.
Those two rules agree while a pool holds at most one frame and diverge on every publish after
that, and the six unit tests written beside the reference all held pools of capacity 1, 2 or 4
into which exactly one frame was ever published. Not one of them could see it.

### The cost, which lands on the reader

The wasted slots are not the problem. A policy holds a frame by calling `latest()` and then
`acquire()` on what it named, and there is no lock across those two calls — there cannot be
one, because §47.4 forbids capture waiting on a reader. So capture publishes in between, and
under the lowest-index rule that publish lands on **the slot `latest()` just named**. The
acquire is refused `stale_handle`, on nearly every cycle, at any frame rate above the reader's
turnaround. A four-slot pool absorbs none of the jitter it was declared to absorb, and the
symptom is a policy that appears not to keep up.

§47.4 already has a one-slot latest-wins channel. A §47.6 that degenerates into one is not a
weaker §47.6 — it is a duplicate of the section above it, and the section that defers to it
loses its reason to exist.

### The rule, stated so an edition cannot read it two ways

Among the slots **not currently acquired**: an **empty** slot wins outright, because there is
nothing in it to destroy and because that is what makes a declared capacity mean what it says —
a pool fills before it overwrites. Otherwise the slot holding the **lowest sequence**, ties to
the lower index. Oldest is by the **frame** and never by the slot's position: the two stop
agreeing the first time a reader holds a frame and lets it go, which is the state a pool exists
for. Written into §47.6a, then fixed in the reference, then regenerated — and the three cases
turned over in the diff.

### Three editions, and the mutation run in both directions

`reference/typescript/media.ts`, `crates/skynet-contracts/src/media.rs` and
`editions/python/skynet_os/media.py`. Both ported editions reproduced all ten cases on their
first run, which is the outcome that proves nothing on its own — so the selection rule was
reverted in each and the corpus re-run:

```
rust    reverted to lowest-index free slot  →  1,124 passed, 3 failed
python  reverted to lowest-index free slot  →  1,124 passed, 3 failed
```

Exactly the three cases written for it, and nothing else. A corpus that had been written after
the editions would have frozen the defect into all three and called it agreement.

### Where the module lives, which was also wrong

The reference was in the **Hub**, at `lib/skynet/os/media.ts`. `check-contract-mirrors.mjs`
keeps the list of modules the Hub holds its own copy of, and the test it applies is *both sides
compute this and neither is authoritative*. Nothing on the Hub owns a camera, publishes a frame
or takes a loan on one — so the Hub's copy was a module nothing imported, which that file's own
header calls the mistake the list exists to avoid.

So `media` is **reference-only**, like `cell`, `blackbox` and `observation`: it lives in
`reference/typescript/` here, the corpus holds three editions to it, and the Hub keeps no copy.
The orphan and its six tests were deleted from the Hub as part of this change. What replaced
them is ten cases three editions reproduce byte for byte.

### The corpus grew a shape it did not have

Every family in `corpus/` until now was one input through one pure function. A pool's whole
subject is what the fourth publish does given what the first three did, so a `media` case is a
**script**: a capacity, a list of operations, and the outcome of every one in order, with `from`
naming an earlier step's handle so `latest` → `acquire` reads as the thing a reader does.

Nothing in it exposes the pool's slots. The handles do that already — a handle carries the slot
it was issued against — so the selection rule is pinned in the expected bytes without any
edition having to agree about how a slot is stored.

### Four stale numbers, found by the checks that exist for it

`scale-report --check` and `edition-report --check` both failed on the first run of this round,
before anything was built: §73 still said 78 corpus files and 1,116 cases from two days earlier.
`README.md` was worse — **1,055 cases and 77 files**, which is two sessions stale, in the table a
vendor reads first. All corrected, and the two reports regenerated and re-synced to the Hub.

### Run

```
cargo test --workspace          — 1,641 passed, 0 failed  (1,631 before; +10 in skynet-contracts)
cargo run -p skynet-corpus      — 1,127 of 1,127, 0 unimplemented
python3 editions/python/run_corpus.py — 1,127 passed, 0 failed, 0 not implemented
node tools/check-mirrors.mjs          — all shared documents match
node tools/check-contract-mirrors.mjs — every contract the Hub mirrors matches the reference
node tools/check-golden-path.mjs      — every link resolves
node tools/scale-report.mjs --check   — §73 matches the tree
node tools/edition-report.mjs --check — 2 editions, 1,127 cases each
```

## What round twelve leaves

* **The mapping half of §47.6.** A read-only mapping of the named slot in the policy's address
  space is per-OS code no contract can express. The loan's rules are now decided; what maps is
  not written.
* **`detects_people` (§5), specified 2026-09-06 and still unbuilt.** The seventh input to
  `resolveLimits()` and the only one with no path, so `proximity` resolves to a ceiling whose
  guard is permanently unavailable. Next item.
* **Kernel liveness into the daemon's conditions, then skynetk in the e2e loop.** Unchanged
  from round eleven. Design work within §33.1.
* **The conformance flip for isolation.** Unchanged: carry a unit-confinement observation into
  `conformance::run`.
* **Vision**, still waiting on the first capture backend and the inference backend §23.3 leaves
  vacant. One item shorter than it was.
* **Device confinement**, unblocked by a vendor runtime that reports accelerator memory.

# Round thirteen — 2026-09-07

## 64. §5's `detects_people` — and the hole that opened behind it

`resolveLimits` takes seven `available` inputs. Six had a path. The seventh —
`people_detection` — had none, and §80 had carried it as an open row for four sessions with a
precise diagnosis: *"it is not a missing delivery, it is that no body can declare a person
detector"*. §5's nine modalities say what a sensor **produces**, and detection is what somebody
does with the output; a camera is not a person detector, a camera plus a model somebody
validated is. So `skynetd` wrote `people_detection: false` on every robot, with a comment
saying why, and `proximity` was a guard the platform could not run however a body was
described.

### The shape, which is a boolean and deliberately not a vocabulary

`detects_people?: boolean` on the sensor, declared **per sensor and by the vendor**, because
only the party that built the sensing pipeline knows whether their depth stream is one a person
is measured in. This is why it did not need §30's *role vocabulary governance* first: there is
no vocabulary in it, nothing to version, and no existing descriptor breaks against an optional
boolean.

Three states, and the third is the reason it is optional rather than defaulted:

| | |
|---|---|
| absent | undeclared — nobody considered it |
| `false` | declared absent — considered, and this stream does not do it |
| `true` | this stream reports person distance |

The first two are **one fact downstream and two in the descriptor**, and that is worth stating
because it decides what the corpus can and cannot hold an edition to. Both make the guard
unavailable, so no expected value separates them: an edition whose parser folded them together
would pass every case in the file. `corpus/embodiment/detection.json` says so in its own header
rather than implying a coverage it has not got.

### Nine cases, and three ways to be wrong about a boolean

`hasPeopleDetection` is one line. The **parse** is where editions diverge, and each edition
refuses a malformed declaration by a different mechanism:

| Edition | Mechanism | Its way of being wrong |
|---|---|---|
| reference | `=== true` | — |
| Rust | `Json::as_bool()` returning `None` | `.is_some()`, which admits `Some(false)` |
| Python | `is True` | `== True` admits `1`; truthiness admits `"no"` |

Python's is the sharpest, because `1 == True` is true and `1 is True` is not. So there is a case
for the number one, and it exists precisely so the paragraph about it is checked rather than
claimed. Every mutation is caught and each catches only what it should:

```
python  `is True` → `== True`     1,135 passed, 1 failed   (the number-one case)
python  `is True` → truthiness    1,134 passed, 2 failed   (that, and the string "yes")
rust    `== Some(true)` → `.is_some()`   1,135 of 1,136    (declared-absent-on-every-sensor)
```

Two cases pin that the **modality is not consulted**: a depth sensor declaring it, and a
force-torque sensor declaring it. The second looks wrong and is the right answer, because the
alternative is a list of modalities a detector may live behind — which is a vocabulary, which
needs the governance this design avoided, and whose wrong entry would silently disable a guard
on a robot whose vendor had declared one.

### The daemon, and the hole this opened

`skynetd` derives it through `has_people_detection` rather than three lines of its own, so a Hub
reading the same descriptor cannot answer differently. `embodiment` is a mirrored contract, so
the change landed on both sides in one `--sync`.

And then the honest half. **`proximity` acts on `cycle.nearest_person_m`, and nothing on this
edition produces that number.** No capture backend, no perception component, no path from a
declaration to a distance. So a body that declares a detector moves the guard out of
*unavailable* and into *active*, where it will never trip — and *active* is what every surface in
this system renders as protection. That is a worse failure than the one this closed.

It is the same shape `available.imu` has always had, one step wider, because that field at least
has a RHAL reading behind it. Two things follow, and only one of them is a fix:

* `skynetd`'s limits document now names **`cycle.nearest_person_m`** in `unmeasured` on every
  robot, replacing the `available.people_detection` entry that this change made false. The
  inertness is stated rather than inferred from the absence of a trip.
* §80 gained a row rather than an answer: **does a guard whose inputs resolve and whose producer
  does not exist deserve a third state?** §7.2 partitions guards into active and unavailable, and
  this is neither. It is a change to a partition the console, the check-in, the device page and
  `skynet limits` all draw, so it is a decision and not a field.

`a_declared_detector_activates_a_guard_nothing_can_trip` is the test that holds it: the guard
resolves active, is no longer reported unavailable, and the document names the reading that is
not coming. Reverting the daemon's derivation fails exactly that test.

### Run

```
cargo test --workspace          — 1,645 passed, 0 failed  (1,641 before; +3 contracts, +1 skynetd-io)
cargo run -p skynet-corpus      — 1,136 of 1,136, 0 unimplemented
python3 editions/python/run_corpus.py — 1,136 passed, 0 failed
Hub: npx vitest run — 1,853 passed, 81 files; npm run typecheck — clean
mirrors, contract mirrors, golden path, scale (§73 matches), edition report — all green
```

## What round thirteen leaves

* **A guard active with no producer.** New, and the sharpest thing on this list because it is a
  reporting failure rather than a missing feature. §80 has it; it needs deciding.
* **The mapping half of §47.6**, and the capture backend behind it. Unchanged.
* **Kernel liveness into the daemon's conditions, then skynetk in the e2e loop.** Design work
  within §33.1. Unchanged.
* **The conformance flip for isolation.** Unchanged.
* **Device confinement**, unblocked by a vendor runtime that reports accelerator memory.

# Round fourteen — 2026-09-07

## 65. `safety.policy.isolation`: the argv is not the sandbox

Round eleven left this as one of four items, restated from round ten and half-delivered: §59
had built the confinement, `e2e.sh` had read ten properties back off a real unit, and the
conformance runner still reported the test **skipped** with a reason naming what would execute
it. This is that.

### Why two rounds, and why the first one could not finish it

§59's confinement is seven `--property=` flags on `systemd-run` for every non-driver package,
and the suite that covers it reads them back off `command_line`. That proves what was
**requested**. §15 asks what a package is actually held to, and the two are different questions
in every way that matters: **a directive systemd rejected, dropped, or does not support on this
kernel produces exactly the same argv.**

It is also the wrong *kind* of result. A check whose subject is this repository's own constant
rather than this machine's state is an edition property — which is precisely what
`safety.kernel.parity` two lines above it is skipped for being. A robot reporting one would be
reporting on its compiler. So the old skip reason named the probe rather than hand-waving, and
this round wrote it.

### What the daemon now does

`Launcher::confinement(id)` — a new trait method, beside `observe` and asking a different
question of a different vocabulary — runs `systemctl show <unit> -p <directives>` and returns
`Key=Value` pairs. `confinement_shortfall(kind, observed)` names every directive that did not
land. Both halves are pure and tested on a machine with no systemd, which is most of them.

Three decisions in twenty lines, each with an obvious wrong alternative:

* **The comparison is on the whole `Key=Value` line, never the key.** `CapabilityBoundingSet=`
  is asked for as the *empty* set, so a check that found the key and stopped would pass a unit
  carrying `CapabilityBoundingSet=cap_sys_admin` — the exact failure the directive exists to
  prevent. `e2e.sh` learned this in round eleven; the daemon inherits it, with a test that sets
  that value and asserts the shortfall names it.
* **A directive systemd did not report counts as not landed.** Fail-closed: an unreported
  directive is one this code cannot show was applied, and a sandbox nobody can read is not a
  sandbox anybody should certify.
* **`--property=` is derived from `confinement_properties`, not typed beside it.** This is the
  bug that would otherwise have been written later: a directive added to the launcher and
  forgotten in the query comes back unreported, the shortfall counts it as not landed, and the
  robot fails its own isolation test naming a directive that *was* applied.
  `confinement_property_names()` removes the possibility rather than testing for it — and there
  is a test that the two lists are the same length, so a name read and never compared is caught
  too.

### Which unit, and the three skips

The route picks the first **running non-driver** package, by id. Which one hardly matters —
`command_line` builds the same confinement for every policy, service and skill — and taking one
keeps this a bounded read on a `GET`. *Non-driver* matters a great deal: §8.3 makes a driver
native by design, so reading one and reporting its absent confinement as a failure would be
this robot failing itself for obeying the specification.

Skipped, never passed, in three cases, each naming what is missing: no sandboxed unit is
running; the launcher records rather than launches (`--dry-run`, or no systemd); systemd would
not describe the unit. `RecordingLauncher::confinement` returns an error saying so rather than
an empty list — an empty list would make `--dry-run` **fail** a safety conformance test on a
machine it never touched.

`conformance::run` stays a function over facts: the observation is read in `api.rs`, where the
I/O belongs, and passed in. A conformance runner that shelled out mid-check would be one nobody
could test without a systemd.

### What this session could and could not watch

The verdict function is unit-tested in all four states — pass, fail naming the directive, and
both skips. `e2e.sh` now calls `skynet conformance` and asserts **the pass** on a machine with
systemd and **the skip** on one without.

**This container is one without.** `systemd-run` is installed, `/run/systemd/system` does not
exist, and PID 1 is `process_api`, so `systemctl --user is-system-running` answers `offline`.
The e2e took its second branch and asserted the skip, correctly. So: the pass path has not been
watched returning `pass` off a real unit **in this session**. Round eleven's machine had a user
systemd and this one does not; the assertion is in the file and will run on the next machine
that has one. Saying it is demonstrated would be exactly the "which we do not" this runner's own
header warns about.

### Run

```
cargo test --workspace  — 1,655 passed, 0 failed  (1,645 before; +10 in skynetd-io)
cargo clippy --workspace — clean
sh crates/skynetd-io/e2e.sh — all checks passed (systemd launch step did not run here)
node tools/scale-report.mjs --check — §73 matches the tree
```

## What round fourteen leaves

* **A package that attempts the forbidden hold.** Confinement landing is the mechanism; a probe
  performing the hold and being stopped is the demonstration, and it needs a package built to
  try. The test's own detail says so rather than implying the second from the first.
* **The pass, watched on a real systemd.** In the file, not yet in a transcript.
* **A guard active with no producer** (§80, raised in round thirteen). Still the sharpest open
  item, and a decision rather than a field.
* **The mapping half of §47.6**, and the capture backend behind it.
* **Kernel liveness into the daemon's conditions, then skynetk in the e2e loop.** Design work
  within §33.1.
* **Device confinement**, unblocked by a vendor runtime that reports accelerator memory.

# Round fifteen — 2026-09-07

## 66. §27's `kernel` condition stops being a default

Round eleven's audit restated this item and explained why it had been misstated:

> Nothing outside contract tests ever reports `RhalState::Present`, `KernelState::Live`, or
> homing complete, so `refresh_operating_state` derives from post plus defaults — RHAL lost
> means `safe` on every robot, always.

So a running `skynetk` changed nothing observable, and *"skynetk in the e2e loop"* was the
second half of a task whose first half did not exist. This is the first half: one of the seven
defaults becomes a reading.

### The source, and the one that looks obvious and is wrong

`skynetd` already asks systemd about units, the kernel is a unit, and `ActiveState=active` is
one call away. It is the wrong source, and the reason is the case that matters:

> **A hung kernel is an active unit.**

`active` says a process exists, which is exactly what a robot with a wedged control loop also
has. Putting §27's most consequential condition on the one signal that cannot separate the
dangerous case from the healthy one would be worse than leaving it a default, because a default
is at least honest about knowing nothing.

A control loop that is turning **writes a cycle every pass** (§26). That record is a side effect
of the kernel doing its job, it is in a file this daemon already holds, and a stopped loop
cannot produce it. So `KernelState::Live` means *cycles recorded since the last look*, and
nothing else.

### Why this does not cross §33.1

§33.1's line is around the **command path**: `skynetd` must never be between a policy and a
joint. Reading a file the kernel wrote is not that, and it is not asking the kernel anything
either. Two properties keep it that way:

* **It fails closed, in the only direction that is safe.** Cycles seen is `Live`; silence, an
  unreadable ring, and no ring at all are all `Down`. A wrong answer here can hold a robot that
  could have been commanding and can never release one that should not be.
* **The daemon cannot forge it.** §26.2 gives the supervisor the event channel and the control
  loop the four cycle channels, so `skynetd` is structurally incapable of writing the record it
  reads. A liveness signal a process can manufacture about itself is not one, and this is a
  property of the format rather than of anybody's restraint.

### Three answers, because §27 has two and a console needs three

`KernelSighting` is `Unobservable(why)`, `Silent`, or `Turning { cycles }`. §27 maps the first
two to `down` and is right to. `GET /state` has to tell them apart:

| | |
|---|---|
| `Unobservable` | the recording has no action channels, so it *cannot* hold a cycle (§74.3's `commands`). Silence from it says nothing about a kernel |
| `Silent` | the ring could have held one and did not. The loop is not turning |
| `Turning` | cycles were recorded in this interval |

Every robot on this edition is in the first state, because no commanding package is resolved and
the supervisor's ring has no action channels. A console showing *"kernel: down"* about that robot
would send somebody to restart a kernel that is not the problem — so `observed` is `false` there
and `true` for the other two, and the reading travels with its reason and its source.

### The cursor, which is what makes it an interval

`daemon.kernel_sequence` is advanced every pass whether or not anything was counted, so a robot
cannot report a kernel live on the strength of cycles from ten minutes ago. It is deliberately
**separate** from `skills_sequence` even though both are cursors over the same ring: sharing one
would make each consume the other's evidence, and a pass that counted interventions for an
episode would leave the kernel observation nothing to see.

### What it changes, and what it does not

It removes **one of seven** blockers. A robot with a turning kernel still derives `safe` with
`rhal_lost`, because the RHAL, the guard set, the stop and the homing state are all still
defaults. That is not a disappointing result, it is the honest size of the step, and the entry in
§73 says so.

What it *does* change is that the next one is now a mapping rather than a channel. A cycle record
already carries `rhal_result`, `state_flags` and `fault_code` (§24). Turning those into
`Present`/`Degraded`/`Lost` is a decision about §24's semantics — which errno is degraded and
which is lost — and it is a decision, so it is written down here rather than made in passing.

### Run

```
cargo test --workspace  — 1,658 passed, 0 failed  (1,655 before)
cargo clippy --workspace — clean
sh crates/skynetd-io/e2e.sh — all checks passed
node tools/scale-report.mjs --check — §73 matches the tree
```

Four stale sentences corrected on the way: the start-up banner, `GET /limits`' `partial` note,
`warn_for_transition`'s header and two rows of §73 all said *seven of eight* or *one of eight*.

## What round fifteen leaves

* **`RhalState` from the cycle record.** A mapping decision over §24's `rhal_result`,
  `state_flags` and `fault_code`, not a missing channel. Next.
* **`skynetk` in the e2e loop**, which now *would* change something observable: a turning kernel
  flips a condition a route reports.
* **A package that attempts the forbidden hold** (§15's isolation, round fourteen).
* **A guard active with no producer** (§80, round thirteen). Still a decision.
* **The mapping half of §47.6**, and the capture backend behind it.
* **Device confinement**, unblocked by a vendor runtime that reports accelerator memory.

# Round sixteen — 2026-09-07

## 67. The cycle record could not say the read had failed

Round fifteen ended by naming the next condition — `RhalState` off the cycle record — and called
it *"a mapping decision over §24's `rhal_result`, `state_flags` and `fault_code`, not a missing
channel"*. Reading those fields to write the mapping found that the channel is missing after all,
in a way that is worse than a gap.

### What a failed read actually records

`Host::step` builds its `Observed` with `map_or` defaults when `sample` fails:

```rust
let (sample, read_error) = match body.sample(now_ns) { … };
let observed = Observed {
    stamp_ns: sample.map_or(now_ns, |s| s.stamp_ns),
    seq:      sample.map_or(0, |s| s.seq),
    flags:    sample.map_or(0, |s| s.flags),
    fault_code: sample.map_or(0, |s| s.fault_code),
    …
};
```

Zero flags. Zero fault code. Zero sequence. The **loop's own clock** as the acquisition stamp,
which §24 is explicit must be an acquisition time. And the joint arrays are copied out of
`body.state()`, which on a failed read still holds *the previous cycle's* positions.

Every one of those is a legal value on a healthy robot. So a recording of a driver that had
stopped answering was byte-comparable with a recording of one that had not, and a replay of it
showed a robot **holding still**. The loop knew — `summary.read_failures` counts them — and that
counter lives in the process's memory and dies with it. The black box is the thing that outlives
the robot, and it was the thing that could not say.

That is why this had to land before the RHAL mapping and not after. A mapping is only as good as
its input, and the input said "healthy" on the cycles that matter.

### Four bytes that were already there

`read_result` sits at offset 172, which was **reserved padding** — `CYCLE_FIXED` is 176 and the
fields ended at 172. So nothing moved, no offset changed, and the ring's `FORMAT_VERSION` stayed
2. Which raises the question the change actually turns on: **what does a zero at 172 mean in a
recording written last week?**

### The version byte that had never been read

The answer is the **per-record schema byte**, and finding it is the second half of this round.
`slot[5]` has existed since the format was written, with a comment stating exactly why:

> The record schema version, per record rather than per file: a ring outlives a daemon upgrade,
> and a reader must be able to tell which layout a given slot is in.

It was written as `1` and **read by nothing** — not by the decoder, not by replay, not by any
edition. Meanwhile `FORMAT_VERSION` (the ring *header's*, which is properly checked) had gone to
2 for the frame record's `streams_absent`, and `README.md` reported "Record format 2" about a
byte that said 1. Three claims, two of them wrong, and no check between them.

It says `2` now and the decoder reads it:

| Schema | Bytes 172..176 | The read's outcome |
|---|---|---|
| 1 | reserved padding | **unrecorded** — nothing is known |
| 2 | `read_result` | `0` is success; negative is the `errno` |

`ReadOutcome::Unrecorded` is the variant that matters. A reader that ignored the byte would read
every older recording as a robot whose every read succeeded — the exact failure the field exists
to prevent — so absence reads as *unrecorded* and never as *fine*.

### The code, not the sentence

`Actuator::sample` returned `Result<Sample, String>`, and a string is not something a black box
can be replayed against: a recording is read months later by somebody who does not have this
build's error strings, and a `-110` is a timeout in any language. `ReadFailure` carries both, and
`ReadFailure::from_driver` maps each `DriverError` onto the `errno` that is *true* of it rather
than onto a placeholder — a call that returned a code carries that code, a vtable with no
`read_state` is `-ENOSYS` (the answer a missing `write_command` already gets), and a shared object
that would not load is `-ENODEV`. A driver that returned zero and called it an error becomes
`-EIO`, because the alternative is a record saying the read worked on the cycle the driver said it
did not.

### Three editions, and the corpus as arbiter

The Rust was written first this time and the reference second — the wrong order by §31.4's third
rule, and worth saying rather than hiding. What preserved the rule's purpose is that the two were
written **independently against the decision** (offset 172, schema byte 2) and the corpus judged
them: had either guessed differently, `blackbox/records.json` would have said so. Both reproduce,
as does Python, and the mutation run confirms the cases bite:

```
rust    schema byte back to 1              1,125 of 1,137   (12 records)
python  the i32 at 172 not written           1,136 of 1,137   (the new case)
```

`cycle-the-read-failed` is the new case, and it is the record this field was added for: a cycle
whose `read_state` returned `-ETIMEDOUT`, with every other body field a fill value.

### Run

```
cargo test --workspace  — 1,659 passed, 0 failed  (1,658 before)
cargo clippy --workspace — clean
cargo run -p skynet-corpus      — 1,137 of 1,137
python3 editions/python/run_corpus.py — 1,137 passed
sh crates/skynetd-io/e2e.sh — all checks passed
mirrors, contract mirrors, golden path, scale, edition report — all green
```

## What round sixteen leaves

* **`RhalState` from the record**, now that the record can carry the fact it needs. Next, and it
  is the mapping this round set out to write.
* **Replay does not mark the fill values.** A record whose read failed still carries the previous
  cycle's positions, and now also carries the fact that they are not measurements. Making a
  replay say so is a separate change; `state_seq` and `state_stamp_ns` are integers with no
  `NaN`, so it cannot be done by writing a sentinel.
* **A package that attempts the forbidden hold** (§15's isolation).
* **A guard active with no producer** (§80). Still a decision.
* **The mapping half of §47.6**, and the capture backend behind it.
* **Device confinement**, unblocked by a vendor runtime that reports accelerator memory.

# Round seventeen — 2026-09-07

## 68. §27's `rhal` condition, and the one flag that is not a loss

The mapping round sixteen cleared the way for. Three of §27's eight conditions are observed now.

### A state, not a count

`kernel` is *cycles recorded since the last look* — an interval question, because "is the loop
turning" is about a period. `rhal` is not. §27's `rhal` is a **state**, so what decides it is the
**newest** cycle in the interval and not an aggregate over it: a driver that missed one read in
two hundred is not lost, and one that has answered nothing for a second is, however many good
reads preceded it.

Both are read from the same records over the same interval, and that pairing is deliberate. A
robot whose kernel is turning while its driver has stopped answering has to be able to report
exactly that; two readings taken at two moments could show a pair that was never true at any
instant.

### The mapping, which is five inputs onto three states

| What the newest record says | State |
|---|---|
| no cycle at all | `Lost` — nothing is reading the body |
| `read_result` negative | `Lost` — the driver did not answer this cycle |
| `read_result` unrecorded (schema 1) | `Lost` — an older recording cannot say, and this fails closed |
| `RHAL_DEGRADED` in the flags | `Degraded` — §24: *"running, but something is wrong"* |
| otherwise | `Present` |

Four of the five are mechanical. The fifth is a decision.

### `RHAL_FAULT` is not `Lost`, and that is the whole of the design

§24 defines it as *"the drive has faulted; commands are being ignored"*. The obvious reading is
that a robot whose drive is ignoring commands is a robot the kernel has lost, and it is wrong for
a reason §27 states about itself. `rhal_lost` is the row that means **nothing else can be known** —
no observed joint state, no estop reading, no guard that can evaluate — and it is first in
`derive` precisely because everything below it becomes unanswerable.

A faulted drive still **answers reads**. That is how anybody knows about the fault at all: the
fault code is in the record because the read succeeded. Mapping it to `Lost` would tell an
operator the robot cannot be observed at exactly the moment it is telling them something, and
would discard the fault code on the way. So a fault is `Present` with the vendor's code carried
verbatim, and what a fault *does* belongs to §7's guards.

### Run

```
cargo test --workspace  — 1,660 passed, 0 failed  (1,659 before)
cargo clippy --workspace — clean
sh crates/skynetd-io/e2e.sh — all checks passed
mirrors, contract mirrors, golden path, scale — all green
```

Three stale counts corrected again — the banner, `GET /limits`' `partial` note and two rows of
§73 — which is the third round running that a condition moving has left a count wrong somewhere.
They are all derived from one number and none of them derives it.

## What round seventeen leaves

* **`skynetk` in the e2e loop**, which now changes two things a route reports rather than none.
  The last of the three items round eleven raised, and it is no longer blocked on design.
* **The five remaining conditions** — homing, stop, guard_hold, guards, policy. Unlike the two
  that moved, none of these has evidence in the ring today: a cycle record carries what the guards
  *did*, not which were available, and homing is a driver-side fact nothing records. Each needs
  its own producer before it needs a mapping.
* **A count that nothing derives.** Four surfaces state how many of §27's eight conditions are
  observed and every one is a typed number. Small, and this round is the third time it has gone
  stale.
* **A package that attempts the forbidden hold** (§15's isolation).
* **A guard active with no producer** (§80). Still a decision.
* **The mapping half of §47.6**, and the capture backend behind it.
* **Replay does not mark the fill values** of a failed read (§26.4).

# Round eighteen — 2026-09-07

Resumed on the OS, in the order round seventeen left. The first item was not on that list: the
workspace did not build.

## 69. `cargo test --workspace` had not compiled on this machine, and no CI job would have said so

Round seventeen's numbers open with `cargo test --workspace — 1,660 passed`. Reproducing that
before changing anything — this repository's own rule — produced instead:

```
error[E0425]: cannot find function `install_skill` in this scope
    --> crates\skynetd-io\src\tests.rs:6754:5
error: could not compile `skynetd-io` (lib test) due to 2 previous errors
```

`install_skill` is `#[cfg(unix)]`, because §37.1's activation needs an atomic symlink swap and
`Store::activate` refuses to fake one. Two of its four callers — the skill-starts-a-unit test from
round nine and the reconciler-spares-it test from round ten — were not. On Linux the helper and the
callers are both present and nothing fails. On Windows the callers survive and the helper does not.

### The gap is the CI file, not the two attributes

All four jobs are `runs-on: ubuntu-latest`. So the workspace has never once been compiled by CI on
the platform it is developed on, and the two attributes were only ever going to be found by
somebody running the suite here.

§57 recorded the mirror image of this in round six — clippy was clean on Windows and had two
warnings on Linux, both in `cfg(unix)` code — and drew the rule: *"a lint result is only as broad
as the target it ran on."* The complement was left open and this is what it cost. A test **build**
is only as broad as the target it ran on too, and it is the sharper half, because a lint warns and
a missing `cfg` does not build at all.

Added a `windows` job: clippy `--all-targets` and `cargo test --workspace`. `--all-targets`
deliberately, because that is the exact scope the break lived in — `cargo build` was fine and
`cargo test` was not. It does **not** repeat the daemon end-to-end run or the corpus: neither is
platform-specific, so repeating them would double a cost for no coverage. What is Windows-only is
*which code exists*.

**1,590 passed, 0 failed** on Windows against 1,660 on Linux, and the difference is the
`cfg(unix)` tests, which is the number being right rather than a gap.

## 70. Four sentences stated one number by hand, and one of them had never been right

Round seventeen's third item: *"Four surfaces state how many of §27's eight conditions are observed
and every one is a typed number. Small, and this round is the third time it has gone stale."*

It was worse than stale. The three **rendered** strings disagreed with each other on the same pass:

| Where | Said | True |
|---|---|---|
| `GET /state`'s `partial` | three of eight | three |
| `GET /status`'s `safe_state_note` | *"the self-test and **eight** unobserved conditions"* | five |
| the start-up banner | three of eight | **one**, at the moment it prints |

The middle one counted §27's *total* as the remainder, so it described a daemon that had looked at
nine things. An operator reading the console header was told the machine had observed nothing while
the same daemon's state route said three — and §32.2's rule is that the console must never show a
value it did not receive. A value the daemon contradicts one route later is worse than one it never
sent.

The third is the one only a run would find. The banner prints **before the recorder is opened** —
`observe_state(&mut daemon, None)`, deliberately, so the first look reports unobservable rather than
down — so at the moment those words reach a terminal exactly one condition has been observed. It
claimed three every boot.

Two documents were wrong as well: `README.md` said seven unobserved (stale by two rounds) and
`skynet-kernel-host`'s header said all eight were.

### `crates/skynetd-io/src/observed.rs`

One array of eight booleans, built from the same two sightings §27's machine is derived from, and
three renderers over it — the tally, the complement, and `GET /state`'s phrasing of the complement.
Nothing outside it holds a number. The names are §27's and fixed; the observability is per-pass and
is not, because `kernel` and `rhal` are observed only when the recording could have held a cycle.

The test that matters asserts the **relationship** rather than the number: whatever `observed`
holds, both routes count it the same way. A condition gaining a producer moves all three sentences
together and the test needs no editing — which is the property the old arrangement lacked and the
only reason the module exists.

### And the documents, because a check that stops at code stops halfway

`tools/scale-report.mjs` already holds §73's closing paragraph to the tree. It now measures the
condition count too, by parsing `Observed::of`'s array — the `false` entries are the unobserved
ones — and checks **every occurrence** across `docs/skynet-os-architecture.md` and `README.md`
rather than the first. Every occurrence, because the way it went wrong was somebody correcting the
row they were editing and leaving the other three. Verified by breaking each file in turn.

The parse is anchored on `pub fn of(` and not on the field name, which the first version was: the
struct declares `seen: [bool; 8]` and an unanchored search finds that first, then reports one entry
against eight conditions. It says so where it throws.

## 71. §27's machine had no terminal that could ask it

Writing §72's e2e section needed the state route, and there was no way to reach it. `GET /state`
is served, the on-robot console renders it, and **no CLI command hits it** — so the one question §27
exists to answer had no terminal. An operator on a bench reads a terminal.

`skynet state`, which is 50 commands rather than 49. Not folded into `skynet status`, because
`api.rs` says why where it serves the two routes: status is *"the one line a console header wants"*
and state is *"the same fact with the observation gaps spelled out"*.

Two things it got wrong first, both found by running it:

**A tick that meant two things.** The mark says whether anybody *looked*. The first draft drew a
green ✓ beside `kernel` on a robot whose own detail read *"no control cycle was recorded"* — §48.5's
conflation in one character. An observation that found something wrong is now marked as a finding
and the state is printed rather than left implied.

**A pattern that added three CLI commands.** `matches!(state, Some("live") | Some("present") | …)`
put `live`, `present` and `passed` into the set of things `skynet <x>` may be, and the command count
went to 52. `print_power` documents this trap two functions below and its fix — a slice and
`contains` — and this is the second time the trap has been stepped in. §73 now states the command
count and `check-golden-path.mjs` holds it there, since that script is the only thing that counts
them and a second counter elsewhere would be §78's two spellings again.

## 72. `skynetk` in the e2e loop — round seventeen's first item

### It could not be run here at all, and the script did not say so

`sh crates/skynetd-io/e2e.sh` on this machine stops before its first check with `no skynetd at
…/target/debug/skynetd`. That reads as a build failure and is not one: the binary is there with
`.exe` on the end. The run genuinely cannot happen — `skynetd` refuses to serve on Windows and says
so on start-up, §23.5 — but a check that fails for a reason unrelated to its subject is worse than
one that does not run. The script now names the platform and points at WSL.

Everything below ran under **WSL Ubuntu 24.04, which has systemd**, so the launch and isolation
steps are exercised rather than skipped.

### Section 10, and why it is a robot of its own

Every robot above it has **no embodiment descriptor**, so `shape::supervising` answers `NoBody`
before reading a manifest and the ring is events-only — which is why every check above sees "no
control loop". Sizing a ring for a body needs a descriptor *and* a package that declares an action
space, and giving the robot above one would change what §27 derives underneath forty checks written
about a robot with nothing commanding it.

So: the mock compiled as a shared object, a three-joint descriptor matching the body
`drivers/mock` presents, and a commanding package whose `source_embodiment` binds three channels to
J1/J2/J3 by exact joint name. The daemon starts first and sizes the ring — *"sized for the body: 3
joints and 3 channels, from openrobot/reach's transport plan (direct)"* — and `skynetk` attaches to
the ring the daemon created. **961 cycles, written by a different process, counted by the daemon.**
That is §26.2 on one recording rather than in a suite.

### Two things the run corrected about assertions written from reading

**`observed` is true before the loop starts.** A ring with three action channels *can* hold a
cycle, so silence from it is evidence rather than an absence: §27's `kernel` is observed and
`down`, and `rhal` is observed and `lost`. On every other robot in the script the same silence is
`Unobservable`. Same reading, opposite meaning, and the ring's shape is what separates them. The
first draft asserted one observed condition before the loop and was wrong about the design.

**Polling for the count sampled the wrong instant.** Because `observed` is true immediately, a poll
waiting for *"three of §27's eight conditions"* was satisfied before the loop had written anything,
and every assertion after it sampled a robot whose kernel had not started. The document it produced
reported a **present RHAL beside a silent kernel** — a pair §27 says can never be true of one
instant, and the exact failure round seventeen gave as the reason the two are read together. The
poll waits for the state now, and there is a check asserting the two readings agree about the
interval they share.

### The assertion worth having

The robot stays `safe`. It began in `safe` with `rhal_lost`, and §27 makes `SAFE` **sticky**: it
takes a named human clearance to leave, never a condition improving. A robot that quietly began
commanding because a driver started answering would be the worst defect this machine could have,
and now something fails if it does.

## Run

```
cargo test --workspace  — 1,595 passed, 0 failed, 7 ignored (Windows)
cargo clippy --workspace --all-targets — clean;  cargo fmt --all --check — clean
sh crates/skynetd-io/e2e.sh (WSL, systemd) — all checks passed, 14 of them new
python editions/python/run_corpus.py — 1,137 of 1,137, 0 unimplemented
check-golden-path — 50 commands;  scale-report --check — §73 matches the tree
check-mirrors, check-contract-mirrors — all shared documents match
```

§73 gained two corrections it did not have to be told about: 120,000 lines and 50 CLI commands are
now checked figures rather than typed ones.

The Hub's copy of `docs/skynet-build-plan.md` held a whole session log — 2026-09-07's
twenty-second, on Collections — that had never been mirrored back. Synced from the Hub, which is
the direction the rule asks for its own document.

## What round eighteen leaves

* **The five remaining conditions** — homing, stop, guard_hold, guards, policy. Unchanged and
  unchanged in reason: each needs a producer before it needs a mapping. What is different is that
  the day one arrives, `observed.rs` is the only file that states how many there are.
* **A package that attempts the forbidden hold** (§15's isolation).
* **A guard active with no producer** (§80). Still a decision.
* **The mapping half of §47.6**, and the capture backend behind it.
* **Replay does not mark the fill values** of a failed read (§26.4).
* **`farewell()` has no caller** (§82.4): a robot that is switched off stays in a browser's cache
  until the 120-second TTL expires, because the daemon has no shutdown path.

# Round nineteen — 2026-09-07

Two more of round seventeen's list, and one item taken off it because it turns out to be a decision.

## 73. §26.4's other half: replay marks what was never measured

§26.4 built `read_result` and closed by naming what it deliberately did not do:

> A record whose read failed still carries the previous cycle's positions; what it now also carries
> is the fact that they are not measurements. Replay marking them is a separate change.

### The half that must not change, and it is the interesting half

The obvious build treats a failed read as an unevaluable cycle and skips it. That is wrong twice.

A cycle whose read failed replays **exactly**: the kernel's verdict is recomputed from the recorded
desired values, the recorded timing and the resolved limits, and the read supplies none of them. So
skipping it would report a window as non-deterministic on the strength of something determinism does
not depend on. And the robot really did produce that verdict from that fill — reproducing it is the
recording being faithful, not the replay being fooled.

So `Replay::deterministic()` is untouched and `Replay::observations_measured()` is new beside it.
A window can reproduce perfectly and have been measured on none of its cycles, and a consumer that
reported the first without the second hands an investigator a green tick over a window nobody read.

### Three states, and the third is not a tidy-up

`Measured`, `Fill(errno)`, `Unrecorded`. Folding a schema-1 recording into `Measured` is the exact
reading §26.4 exists to prevent — *"a reader that ignored the byte would read every older recording
as a robot whose every read succeeded"* — and folding it into `Fill` would invent a fault in a
window that may have had none.

Counts rather than a list, because the arithmetic decides it: a driver that stops answering for one
second at 1 kHz produces a thousand of them, and a list somebody has to scroll past is a worse
answer than a number they can act on. What is actionable is beside it — the distinct `errno`s,
bounded by the vocabulary rather than by the window, because a timeout and a faulted drive lead to
different investigations.

`observation_note()` is the one sentence every consumer owes a reader, rendered once rather than
spelled three ways (§78). §15's `telemetry.replay.deterministic` appends it to **both** its pass and
its failure.

### One fixture bug worth recording

The first version of the test asserted three cycles agreed and got three divergences. The fixture
was wrong, not the code: `desired` is `f32` and `command` is `f64`, so a recorded `0.1` compares
against a recomputed `0.10000000149011612`. Changed to `0.25`, which is exact in both. Worth writing
down because the failure looks like a replay defect and is a property of the record's own widths.

## 74. §15's isolation reaches the kernel, which is the rung below the manager

§15's `safety.policy.isolation` has been walking down a ladder one rung at a time, and it had one
left. §59 asserted the **argv** — what was requested. §62 read the unit back with `systemctl show` —
what the **manager holds**. Both are records of a request. What a process is actually held to is the
kernel's, and a directive the manager accepted, stored and reports happily can fail to be applied by
the kernel underneath it, with every layer above going on saying it landed.

So the daemon now reads `/proc/<MainPID>/status` on the unit's own process and compares the two
facts a kernel exposes per process — `NoNewPrivs` and the capability bounding set — against what the
unit claims. **A disagreement between the layers is its own failure**, and it is the most
interesting result this test can produce: a robot whose service manager and whose kernel do not
agree about a sandbox is a robot whose sandbox nobody can describe.

### Two rather than seven, said rather than implied

`NoNewPrivs` and `CapBnd` are the two directives in `confinement_properties` the kernel exposes
per-process and unambiguously. The five `Protect*` directives are mount namespaces, and what a
namespace contains is not a line in `status` — verifying those from inside means attempting a write
and reporting what happened, which is a probe rather than a reading. Checking two and saying two
beats checking two and implying seven.

`CapBnd` is compared as a **mask parsed from hex** and not against the literal
`0000000000000000`, because a string comparison is a check a future kernel could break by printing
a different width.

### Three outcomes, and the skip is the one with the argument in it

* **Fail** when the layers disagree, naming both readings.
* **Skip** when `/proc` will not describe the process — fail-closed the other way would report a
  sandbox defect where the finding is that nothing could be measured, and passing would be the
  manager's record standing in for the kernel's, which is the substitution this rung exists to stop.
* **Pass** only when both agree, and the sentence says the kernel was read *from /proc rather than
  from the manager's record of what it was asked for*.

### And the e2e asserts the rung, not just the word

A pass that had only reached the manager's record still prints "pass". So the script greps for the
kernel clause separately — otherwise the check goes quiet on exactly the machine where `/proc`
stopped answering. Verified on a real confined unit under WSL's systemd: `NoNewPrivs: 1`,
`CapBnd: 0000000000000000`, read off the running process.

### What it still does not establish, restated more precisely

An observation is not an attempt. The demonstration §15 wants — a package that *tried* the forbidden
hold and was stopped — needs two things, and the second was not obvious: a package built to try, and
a manager running **as root**. On a rootless user manager most of these directives forbid something
the user could not do anyway, so a refusal there would prove nothing about the sandbox. That is now
in §73 rather than in somebody's head.

## 75. `farewell()` — taken off the buildable list, because it is a decision

§82.4's goodbye has no caller and the reason is *"the daemon has no shutdown path — systemd stops it
with a signal, and installing a handler needs `unsafe`, which this crate forbids."* Three ways
across that, and **every one crosses a boundary this repository drew deliberately**:

| | What it costs |
|---|---|
| a signal handler | widening the `unsafe` exemption list, which CI enforces as an invariant over five crates |
| an admin `POST /shutdown` the unit's `ExecStop=` calls | a new route that stops the supervisor — a security surface §39.1's table governs |
| a separate `skynetd goodbye` process | it cannot know whether the name was **held**, and `farewell()`'s guard exists because withdrawing a name you never held tells every cache on the link to drop the records of whichever robot does own it |

A clean shutdown would also be worth a black box event, and that vocabulary is closed and CI-checked
— a fourth boundary.

§82.4 already accepts the TTL as the answer for a robot that is switched off, and §83.3's desktop
applications watch through a platform resolver that notices a robot going away without a goodbye. So
this is not urgent, it is not blocked on work, and picking one of three boundaries to cross is not a
call to make quietly. It moves from *buildable* to *open, and needs a decision*.

## Run

```
cargo test --workspace  — 1,602 passed, 0 failed, 7 ignored (Windows)
cargo clippy --workspace --all-targets — clean;  cargo fmt --all --check — clean
sh crates/skynetd-io/e2e.sh (WSL, systemd) — all checks passed
python editions/python/run_corpus.py — 1,137 of 1,137
check-golden-path — 50 commands;  scale-report --check — §73 matches the tree
check-mirrors, check-contract-mirrors — all shared documents match
```

**`cargo fmt --all` also reformatted three files nothing in these rounds touched** —
`skynet-contracts/src/media.rs`, `skynet-corpus/src/lib.rs` and `skynetd-io/src/limits.rs`, all
struct literals rustfmt breaks across lines. So `cargo fmt --all --check` was red at HEAD as well,
and it is the same cause as §69: **nothing has been pushed**, so the `rust` job has never run on any
of this work. Two of CI's four checks were failing on a tree whose log recorded them as clean, and
neither could have been noticed locally without running them. They pass now.

## What round nineteen leaves

* **The five remaining conditions** — homing, stop, guard_hold, guards, policy. Each needs a
  producer before it needs a mapping.
* **A package that attempts the forbidden hold**, which now has a second prerequisite written down:
  a root manager.
* **A guard active with no producer** (§80). Still a decision.
* **The mapping half of §47.6**, and the capture backend behind it.
* **`farewell()`'s caller** (§82.4). Moved to *needs a decision*; the three options and what each
  costs are in §75 above.

# Round twenty — 2026-09-07

Three decisions taken by the owner, and the first of them built. §27 now decides from four of its
eight conditions.

## 76. The three decisions, recorded where they were open

| Open since | Decided |
|---|---|
| §27's `stop`, once the producer was found (§77 below) | **`stopping`, fail-closed.** Its own cause, and never a completed stop |
| §80's guard active with no producer | **A third state.** `active` / `unavailable` / a new one for *inputs resolve and no reading is coming* |
| §82.4's `farewell()` caller | **A shutdown command**, called from the unit's `ExecStop=` |

The second and third are not built yet and are named here so the queue is the decision list rather
than a memory of a conversation.

## 77. §27's `stop`: the producer was already in the record

The five remaining conditions have been listed as *"each needs its own producer before it needs a
mapping"* since round seventeen. That was right about four of them and wrong about one:
`SKR_FLAG_ESTOP` is §24's first state flag, `Cycle::state_flags` has carried it since the format was
written, and it is in **the same record, on the same newest cycle, over the same interval** the RHAL
is read from. Nothing had to be produced. It had to be read.

### The vocabulary question, which is why this was a decision and not a mapping

§27's `stop` wants a **cause**, and the four that existed are the kernel's own decisions: it
evaluated a guard, or a person asked, and it commanded a stop. An open hardware chain is none of
those — it is the body reporting a fact that happened whether or not any software agreed.

The tempting fold is onto `operator`, and it is wrong. §24 gives the chain **one bit**: a robot can
say the chain is open and cannot say what opened it, and a light curtain, a door interlock and a
safety relay noticing itself fail all open the same chain as the button on the chassis. `operator`
means *a person asked*, so that mapping would put a name on an event that has none — §64's own rule
that a message misdescribing what happened is a defect rather than a wording choice.

So `estop` is a fifth cause, in a list three editions had closed at four, with `stopping_estop`
beside it. The reference edition, the Rust contract, the Python edition, the closed-set vocabulary
and three corpus cases.

### It never reports the stop as finished, and that is the whole of the trade

§27 pairs a stop with `stop_settled`, and **no edition can observe it for a hardware stop**. The bit
says the chain is open; it says nothing about whether the body has come to rest. A daemon setting
`stop_settled` here would be claiming to have watched a robot stop moving.

So it stays false, the machine derives `stopping`, and the consequence is written down rather than
discovered: a robot whose chain has been open for an hour goes on reporting `stopping`. That is the
strongest thing this evidence supports, and it is fail-closed in the only direction that is safe —
it can hold a robot and can never release one. Two corpus cases pin **both** halves, so the absence
is a reading rather than a rule the machine hard-codes: a caller who genuinely watched the body come
to rest supplies `stop_settled`, and §27 answers `stop_complete` for `estop` exactly as it does for
the other four.

### A fill value is not a reading of the chain, in either direction

A cycle whose `read_state` failed carries the **previous** cycle's flags (§26.4). Reporting `Clear`
off one would be §26.4's own failure a field further along; reporting `Asserted` off one would raise
a stop nobody observed. It is `Unobservable`, and §27 is already holding such a robot through
`rhal`, which reads the same record and answers `Lost` wherever this answers `Unobservable`.

### Three things the run corrected that reading could not

**The test that used `"estop"` as its example of a made-up cause.** `unreadable_fields_fall_back...`
fed the parser `"stop":"estop"` precisely because the vocabulary did not contain it — and then the
vocabulary grew to contain it, so the test began asserting that a real cause parsed to nothing.
Replaced with `"emergency"`, and the lesson kept in the comment: a fixture whose whole point is that
a string is *not* in a closed set has to be reread whenever the set changes, because nothing else
will notice.

**The corpus caught an incomplete transcription, which is what it is for.** The Python edition
declares its vocabularies by hand in `vocabulary.py`, transcribed from the reference rather than
generated from the corpus — deliberately, since an edition generated from the corpus agrees with it
by construction and proves nothing. Updating `opstate.py` and not `vocabulary.py` failed two cases
immediately.

**And the e2e assertion was wrong about §27's own rule.** It asserted that an open chain moves the
robot to `stopping`. It does not: that robot was already `safe` — nothing had read its body when the
daemon started — and §27 makes `SAFE` **sticky**, needing a named human clearance to leave. A stop
arriving at a robot that is already safe cannot make it less safe. The corrected check asserts
exactly that, which is a stronger statement than the one it replaced.

### Run against a real driver, not a flag a test wrote

`drivers/mock` latches its estop from the first read when opened with `fault=estop`, so the e2e
drives `skynetk` against a driver whose chain is genuinely open and reads the result back through
`skynetd` on the socket:

```
✓ stop   the emergency-stop chain is open on the newest cycle. §24 gives it one bit, so what
         opened it is not knowable from here — a light curtain and a door interlock open the
         same chain as the button
✓ and the robot stays safe: a stop cannot make a safe robot less safe (§27)
✓ and never as a completed stop — nothing watched the body come to rest (§27)
```

### The count moved itself

Four surfaces state how many of §27's conditions are observed, and none of them was edited by hand:
`observed.rs` derives it, and round eighteen's `scale-report --check` caught all four documents in
one run — §73's `§27` row, §73's `Audio` row, §81's closing paragraph and the README. That is the
mechanism doing exactly the job it was built for two rounds ago, on its first real change.

`skynet state`'s printer needed one fix the state words could not give it: `stop` has no state word
— an open chain is a boolean — so a mark derived only from `state` drew a green tick beside a robot
its own detail said had been stopped by hardware. The same class of error the printer already had to
fix for `kernel`, and the second time in two rounds, which is why the finding is now read off the
block rather than off a vocabulary.

## Run

```
cargo test --workspace  — 1,604 passed, 0 failed, 7 ignored (Windows)
cargo clippy --workspace --all-targets — clean;  cargo fmt --all --check — clean
sh crates/skynetd-io/e2e.sh (WSL, systemd) — all checks passed
python editions/python/run_corpus.py — 1,140 of 1,140, both editions
check-golden-path — 50 commands;  scale-report --check — §73 matches the tree
check-mirrors, check-contract-mirrors — all shared documents match
```

## What round twenty leaves

* **§80's third guard state**, decided and not yet built.
* **§82.4's shutdown command**, decided and not yet built — and it brings a second question with it:
  a clean shutdown is worth a black box event, and that vocabulary is closed and CI-checked.
* **The four remaining conditions** — homing, guard_hold, guards, policy. `stop` is off the list;
  these four have no producer in any record, and `homing` is the one worth restating: it is a
  driver-side fact §24 gives no flag for, so it needs an ABI change rather than a reader.
* **A package that attempts the forbidden hold**, which needs a root manager as well as a package.
* **The mapping half of §47.6**, and the capture backend behind it.

# Round twenty-one — 2026-09-07

§80's guard-with-no-producer, decided and built. The second of the three.

## 78. `inert` — the third guard state

§7.2 partitioned guards into **active** and **unavailable**, and §80 recorded the case those two
could not describe: a body that declares a person detector resolves `proximity` *active*, every
input the resolver can see is present, and nothing on any robot fills `cycle.nearest_person_m`. The
guard is reported as running and will never trip — and *active* is what every surface in the system
renders as **protection**.

### Why not `unavailable`, which was the tempting fold

The two mean different things to different people. `unavailable` is *an input this robot does not
have*, and its remedy is to fit a sensor or write a profile. An inert guard has both and wants a
**producer in the operating system**. A fleet operator sent to check wiring that is fine has been
told the wrong thing, and §64 makes that a defect rather than a wording choice.

### The input, which is not the one that already existed

`available.people_detection` is the **body's** claim, out of §5's descriptor: this machine has a
sensor that detects people. The new `available.proximity_reading` is the **operating system's**:
something running here fills that field every cycle. A robot can have the first without the second
and every robot that exists does — which is exactly what §7.2's two states could not express.

Checked **after** the missing-input test, and the order is the design: a robot with no detector is
`unavailable`, not inert. Two corpus cases pin both sides of that ordering.

### `skynetd` sets it to a constant `false`, and that is the honest value

Nothing in this operating system fills `cycle.nearest_person_m`: §25.1's assembler has no vision
path, §47.6's mapping half is unwritten, and no backend infers on one. It is a **field** rather than
a branch in the resolver so that the day a producer exists, that is the line that changes.

### What it touched, and what the corpus caught on the way

Three editions, the wire, both repositories:

| | |
|---|---|
| `reference/typescript/safety.ts` | the partition, the third bucket, the new flag |
| `crates/skynet-kernel/src/lib.rs` | `InertGuard`, `ResolvedLimits::inert_guards` |
| `editions/python/skynet_os/safety.py` | the same, transcribed |
| `checkin.ts` / `checkin.rs` / `checkin.py` | `GuardReport.inert`, carried across §17 rather than recomputed on the Hub (§78) |
| `shape.ts` / `safety.rs` / `safety.py` | the shaped documents the corpus compares |
| `reference/wire/checkin.json` | re-frozen, and resynced to the Hub |
| the Hub's `lib/skynet/os/` | `checkin.ts`, `safety.ts`, the wire fixture, and two new tests |

The corpus caught **three** incomplete transcriptions in a row, each one a real gap rather than a
formality: the Python partition, the Python check-in shape, and the Rust `ResolvedLimits` shape.
Every one of them would have shipped an edition that agreed about the guard and disagreed about the
document describing it. §31.4's whole argument, three times in one afternoon.

### One mistake worth writing down

Syncing the Hub's copies by `cp` overwrote its **import specifiers**, which are the one legitimate
difference the mirror check normalises — the reference is standalone and uses explicit `.ts`
extensions, the Hub resolves through its bundler and takes `SafetyEnvelope` from its own object
model. The check said "4 line(s) differ" and a plain diff of the two files said nothing, because the
difference is the thing the comparison rewrites. Restored by hand; the lesson is that `--sync` is
the tool and `cp` is not.

### And the test the decision was raised by

`a_declared_detector_activates_a_guard_nothing_can_trip` asserted **active** — correctly, under a
partition that could not express this robot. It is now
`a_declared_detector_with_no_producer_resolves_inert` and asserts all three: not active, not
unavailable, and inert with the reading named. The `unmeasured` list it also checked stays, because
the partition tells a console how to draw the guard and that list tells a person which reading is
absent.

## Run

```
cargo test --workspace  — 1,604 passed, 0 failed, 7 ignored (Windows)
cargo clippy --workspace --all-targets — clean;  cargo fmt --all --check — clean
python editions/python/run_corpus.py — 1,142 of 1,142, both editions
npx tsc -p . — clean
Hub: npx vitest run — 1,891 passed, 84 files;  npm run typecheck — clean
check-golden-path — 50 commands;  scale-report --check — §73 matches the tree
check-mirrors, check-contract-mirrors — all shared documents and contracts match
```

## What round twenty-one leaves

* **§82.4's shutdown command**, decided and not yet built — the last of the three. It brings a
  second question with it: a clean shutdown is worth a black box event, and that vocabulary is
  closed and CI-checked.
* **The four remaining conditions** — homing, guard_hold, guards, policy. None has a producer in
  any record; `homing` is a driver-side fact §24 gives no flag for, so it needs an ABI change
  rather than a reader.
* **A package that attempts the forbidden hold**, which needs a root manager as well as a package.
* **The mapping half of §47.6**, and the capture backend behind it — which is also what would flip
  `proximity_reading` to true and make §78's new state stop applying to every robot.

# Round twenty-two — 2026-09-07

§82.4's goodbye, which had no caller for eleven rounds. The last of the three decisions.

## 79. `POST /shutdown` — the daemon can be asked to stop

`farewell()` has built RFC 6762 §10.1's withdrawal since the responder was written and `skynetd`
never sent one, so a robot switched off cleanly stayed in every browser's cache on the link until
the 120-second TTL expired. The reason it stayed open is worth restating, because it is not that
nobody got to it: **all three ways across cross a boundary this repository drew on purpose.**

| | What it costs |
|---|---|
| a signal handler | widening the `unsafe` allow-list CI enforces over six crates |
| a separate `skynetd goodbye` | it cannot know whether the name was ever **held**, and `farewell()`'s guard exists because withdrawing a name you never held tells every cache on the link to drop the records of whichever robot does own it |
| a route | a credential that can reach the socket can stop the supervisor |

The owner chose the route. What it costs is now written in three places — the route's own header,
§39.1's table entry, and §82.4 — rather than discovered by whoever finds out first.

### Admin, by an entry rather than by the fall-through

`auth`'s table already defaults an unrecognised route to `Role::Admin`, so this would have been
admin without a line. It has one anyway, carrying the argument §39.1 makes about `unpair`: a robot
whose supervisor is not running applies no desired state, launches nothing and answers nobody, and
only systemd can start it again. That is the destructive half of an admin action.

It is emphatically **not** §39.1's `POST /stop`, which is anonymous on purpose because a stop must
never wait for a credential. A test asserts an operator gets 403 and an anonymous caller gets 401,
and that neither leaves a request behind — because the confusion between the two routes is one an
operator can make under pressure, and the one that is open must stay the one that stops the body.

### The route stops nothing, and that is the design

It records **who asked**; the loop performs it. It has to: the responder is another thread, the
recording is the loop's to seal, and the handler is running inside a connection the caller is still
waiting on. So the answer goes back first and the daemon exits at the top of its next turn — §33.3's
shape, where the API states an intent and the loop converges on it.

One `AtomicBool` each way carries the request to the responder and the acknowledgement back. `std`
has that without the `unsafe` a signal handler needs, which is the boundary that kept this shut.

### The order, and the two bounds

**The name first**, because it is the only part with a deadline: every millisecond spent sealing is
a millisecond an operator's list is wrong. The wait for the acknowledgement is **bounded at one
second**, after which the daemon exits anyway and says the name was not withdrawn — a lost goodbye
is the case the TTL exists for and is not a reason to keep a robot running. A daemon started without
`--lan` runs no responder and does not wait at all.

**Then §13's window**, sealed with `Reason::Shutdown` — which already existed, and whose doc is this
case in one line: *"the robot is shutting down and the window would otherwise be lost."* That is
why the second question this decision brought with it answered itself: **no new black box event was
needed.** Sealing writes §26's `sealed` record, so a clean stop is *in* the recording rather than
being the moment it goes quiet, and the closed, CI-checked event vocabulary stayed closed.

The exit code is **zero**, deliberately: `ExecStop=` runs this and systemd reads the daemon's own
exit, so a non-zero code would put a robot that stopped exactly as intended into `failed` on every
fleet dashboard that reads unit state.

### The unit file

`ExecStop=-/usr/bin/skynet shutdown …`, prefixed with `-` so a failure never blocks a stop — a robot
that cannot reach its own socket has already lost its supervisor, and refusing to stop would turn a
lost goodbye into a hung unit. `TimeoutStopSec=15s` is the ceiling past which systemd sends the
signal, which is the behaviour that existed before this, so the worst case is unchanged.

### Two things only a run found

**The Windows suite cannot see the daemon's loop.** `cargo test --workspace` passed on Windows with
`seal()`'s arguments reversed and its return type wrong, because `skynetd`'s main loop is Linux-only
code Windows never type-checks. Round eighteen added a Windows job for the mirror-image failure —
code that exists only on Windows — and this is the other half of the same asymmetry: **the two
platforms compile different subsets, and each is blind to the other's.** CI's `rust` job on ubuntu
would have caught this one, which is the arrangement working; what caught it here was running the
e2e under WSL rather than trusting a green suite.

**A credential added after the daemon starts does not exist to it.** The daemon reads its
credentials once, at start, so the first version of the e2e step asked for a shutdown with a token
the running process had never heard of and got a 403. The script's own §5 already does this in the
right order; the new step now does too.

### Run for real, on a machine with systemd

```
✓ the shutdown is accepted, and says it stops the supervisor not the robot
✓ and the daemon exits on its own, with nothing signalled (§82.4)
✓ it sealed the window, naming the reason
✓ and said so plainly
✓ and the sealed window is on the robot
✓ and it withdrew its mDNS name
```

Nothing in that step sends a signal, so a process still running afterwards would be a shutdown path
that does not work rather than one that is slow. The withdrawal itself is asserted only where a
responder bound 5353 — on a developer machine Avahi holds it, and there is then no name to give up.

## Run

```
Windows:  cargo test --workspace — 1,606 passed, 0 failed;  clippy clean;  fmt clean
Linux:    cargo test --workspace — 1,676 passed, 0 failed;  clippy clean;  fmt clean
          sh crates/skynetd-io/e2e.sh (systemd) — all checks passed
python editions/python/run_corpus.py — 1,142 of 1,142, both editions
check-golden-path — 51 commands;  scale-report --check — §73 matches the tree
check-mirrors, check-contract-mirrors — all shared documents and contracts match
```

## Where this leaves the OS

All three of the owner's decisions are built. What remains is, in every case, waiting on something
that is not a decision and not code anybody can write today:

* **The four remaining §27 conditions** — homing, guard_hold, guards, policy. None has a producer in
  any record. `homing` is the sharpest: it is a driver-side fact §24 gives no flag for, so it needs
  an **ABI change** rather than a reader, and the ABI is the one thing in this system a vendor
  compiles against.
* **A package that attempts the forbidden hold** (§15). Needs a package built to try *and* a
  manager running as root — on a rootless user manager the directives forbid things the user could
  not do anyway, so a refusal proves nothing.
* **The mapping half of §47.6 and a capture backend.** Per-OS code plus hardware, and it is also
  what would flip `proximity_reading` to true and stop §80's new `inert` state applying to every
  robot that exists.
* **Everything §73's last column already names as hardware**: real drivers, the safety MCU, a TPM,
  §36.4's brownout input, RAUC, a physical joint.

§73's own summary of itself is now true without qualification: every subsystem that can be built
without hardware has been.

---

# Round twenty-three — 2026-09-07

## Audit — what is left, asked of the plan rather than of the last log

No code this round. The instruction was to establish what remains for the **OS**, the **Hub** and
the **Console** before building anything, on the suspicion that the plan might be finished. It is
not finished, and the more useful result is that **round twenty-two's closing paragraph was wrong
about three items** — it filed them as waiting on hardware when they are waiting on a reader.

Everything below was checked against the tree on this machine rather than read out of a log.

### Baseline, measured today (Linux, this container)

```
cargo test --workspace                    1,676 passed, 0 failed
python3 editions/python/run_corpus.py     1,142 of 1,142, 33 corpora complete
node tools/check-mirrors.mjs              all shared documents match
node tools/check-contract-mirrors.mjs     every mirrored contract matches
node tools/check-golden-path.mjs          4 documents, 51 commands
node tools/scale-report.mjs --check       ok  (Hub mirror unmeasured: no checkout at the path it looks for)
node tools/edition-report.mjs --check     FAILS — see defect 1
node scripts/check-skynet-service.mjs     6 checks, no divergence          (Hub repo)
```

Both working trees are clean and level with `origin/main`.

### Toolchains present, and what that decides

`cargo` 1.94, `node` 22, `python3` 3.11, `gcc` 13, `systemd` 255, `psql` 16. **No `dotnet`, no
`swift`, no Supabase credentials, no `node_modules`.** So the OS is fully buildable and testable
here; the Hub is testable for units, types and lint once `npm install` runs and **not** against a
database; the Console cannot be compiled at all, and an uncompiled native edit is an unverified
one.

---

## Defects the audit found — all three are stale numbers, and all three are live

**1. `reference/reports/editions.json` says 1,137 cases. Both editions pass 1,142.**
The five cases the last commit added to the corpus never reached the generated report.
`tools/edition-report.mjs --check` catches it and **CI does not run that tool** — the Python job
gates on `--min-passed 1109`, a floor which 1,137 and 1,142 both clear. `check-contract-mirrors`
passes because the Hub's mirror is byte-identical to the stale file, so the drift is *mirrored
faithfully*: `/skynet/os/editions` renders "1,137 of 1,137" to the public today.

**2. `tools/edition-report.mjs --check` is absent from CI.** `ci.yml` runs four of the five check
tools in `tools/`; this is the fifth, and it is the only one that could have caught defect 1. Every
neighbouring check passed honestly while the figure was wrong: `check-contract-mirrors` compares
the Hub's copy to the reference's and both were stale together, and the corpus job's
`--min-passed 1109` is a floor that 1,137 and 1,142 both clear. A ratchet cannot catch a stale
exact figure; only re-deriving it can.

*(An earlier draft of this paragraph also named `check-contract-mirrors` as absent. It is not —
`ci.yml` line 273 — and the correction is left visible because the mistake is the one this section
is about: a figure read off the wrong grep rather than off the file.)*

**3. `scale-report --check` cannot see the Hub.** It reports *"the Hub's mirror was not measured:
no checkout beside this one"* while the Hub checkout is at `../RobotOverflow`. The path it looks
for is stale, so the one figure in §73 that spans both repositories is unchecked wherever the
directory is not named what it used to be.

None of these is a defect in a robot. All three are the failure mode this repository has documented
five times: a number stated by hand in more places than it is derived in.

---

## OS — what is left

### A. Buildable now, and round twenty-two said otherwise

**Three of §27's four unobserved conditions have a producer already in the tree.** Round
twenty-two's closing paragraph — *"None has a producer in any record"* — is true of `homing` and
false of the other three. `crates/skynetd-io/src/observed.rs` fixes all four to `false` with a
one-line reason each, and two of the reasons look at the wrong source:

| Condition | What `observed.rs` says | What is actually available |
|---|---|---|
| `guard_hold` | *"the kernel's"* | `Cycle.response` is `Option<FaultResponse>` and `FaultResponse::Hold` is one of five variants, **recorded per cycle** in §13's ring. This is the same shape as `stop`: a state read off the newest cycle |
| `guards` (`guards_unavailable`) | *"a cycle records what guards did, not which were available"* | Correct about the ring and it is not the source. `crate::limits` **resolves the guard partition on this daemon** (§7.6, `GET /limits`) — active, unavailable and, since §80, `inert`. "A guard is unavailable" is a value this process already computes and renders |
| `policy` | *"§25.2's lease table is the kernel's"* | `Cycle.policy: u16` names the commanding policy on every recorded cycle, and `shape::supervising` already resolves which package holds the commanding plan. `Ready` is derivable the way `kernel` was |
| `homing` | *"driver-side, and nothing records it"* | **True and unchanged.** §24 gives no flag for it, so it needs an **ABI change**, which is the one thing in this system a vendor compiles against |

Whether all three should be built is a judgement about evidence, not a lookup — `guards` in
particular changes what `DEGRADED` means on a robot that has never run a cycle — but they are code
somebody can write today, which is the thing round twenty-two denied.

### B. Buildable now, unambiguously

* the three defects above;
* a **third language edition** (§73, §31.3). Two editions can agree because the second was
  transcribed from the first; the corpus proves its worth when an edition is written by somebody
  who did not write the reference. 1,142 cases, 80 files, and the C edition is the one §31.3's
  matrix puts at the certified ceiling.

### C. Needs an owner's decision before any code

* **A Hub countersignature on a conformance result** (§57.1, §80). §19 forbids a robot holding a
  signing key, so the runner cannot sign; the Hub countersigning what a device credential delivered
  is a *different claim* from the one §15 asks for. Open since the eleventh session.
* **`farewell()` beyond `POST /shutdown`** — taken off the buildable list in round nineteen as a
  decision, and the decision has not been made.

### D. Needs hardware or a second machine

The §73 last column, unchanged: a real RHAL driver under bench supervision, a physical joint under
the kernel, the safety MCU and the certified tier's notified body, a TPM for
`security.identity.attested`, §36.4's brownout input, RAUC and an OTA image actually applied, a
capture backend (V4L2/GenICam) and §47.6's mapping half, a robot that trips a guard, a robot that
sounds §49's tone, a field-conformance figure measured over a window something commanded, and a
package built to attempt §15's forbidden hold **under a root systemd manager** (a rootless user
manager forbids things the user could not do anyway, so a refusal there proves nothing).

### E. Open questions, explicitly undecided (§30, §43, §53, §61)

Retargeting fidelity, role-vocabulary governance, kernel formal verification, field-conformance
privacy, multi-robot on one compute, safe-state discovery for mobile bases, package-level A/B for
drivers, a Hub-brokered console relay, fleet-wide config convergence, fleet-scale calibration drift,
`on_command_loss` per chain, entitlement for the OS itself, audio beyond warning and anomaly.

---

## Hub — what is left

`skynet-build-plan.md` §33's twenty-fifth session says the Hub-side list *"is now empty except what
needs a robot"*. Measured against §31's phases that is true of Phases 3–9 and **not** of 10, 11 and
12, which the same log filed a fortnight ago as *"blocked on infrastructure that does not exist"*
and has not restated since. Both sentences are in the document and they do not agree.

Verified in the tree: 95 route files under `/api/skynet/v1`, 28 migrations, 53 Skynet unit-test
files, 18 verifier scripts.

### A. Already built, and the log says otherwise

**`POST /repos/{handle}/{name}/restore` exists.** Session twenty-five records the restore surface as
*"recorded for the owner rather than built"*; the route is in the tree, staff-gated, and was added
in an earlier commit than the log entry that says it is absent. The item is closed and the log
needs the correction.

### B. Genuinely unbuilt, and not blocked on a robot

| Plan | What is missing |
|---|---|
| **Phase 10** — real execution | `IN_PROCESS_JOB_KINDS` is `['dataset_index', 'search_reindex']`. `evaluation`, `simulation`, `training` and `deployment` are queued and stay queued. No reference worker image, no sandboxed workspace, no attestation, no quarantine for unverifiable runs |
| **Phase 11** — Spaces and managed compute | `spaces.ts` resolves a `SimulationSpec` and nothing runs one. `training.ts` says so in its own header: *"the web tier orchestrates; it never trains"*. No inference endpoints exist as a surface at all |
| **Phase 12** — teleoperation | The only trace of it in the schema is five `teleop_*` values in `skynet_episodes.source`. No operator session, no transport, no time sync, no intervention identity, no consent or retention path. The ingestion half exists; the operating half was never started |
| **Phase 12** — rollout | No canary, no drift detection. `deployments` has desired state, check-in and rollback |
| **Phase 9** — datasets | No preview, no sample/trajectory/video viewer, no split/shard manifest, no range or stream access, no quality report, no consent/licence lineage |
| **Phase 9** — Git | No git interoperability and no external-Git mirror. `diff.ts` compares manifests and digests and states, correctly, that it never claims to compare content |

Phases 10 and 11 need containers, a GPU and a signing pipeline that this environment does not have —
but the *code* is writable and reviewable without them, and the plan's own Phase 8 gate says the
opposite of what queuing a job proves.

### C. Blocked here, not in principle

* The **marketplace-side listing-transactions migration** (`20260905200100_…`) is still unapplied,
  and this container has no database credentials, so it cannot be applied or verified here.
* Every `skynet-verify-*.mjs` script (18 of them) needs a live database and a running server.

### D. Needs a robot

A field-conformance figure that is not null, a robot that produces a run bundle, and robot-signed
conformance — which §19 makes a rule rather than a gap.

---

## Console — what is left

`desktop/` in the Hub repository, per §83.3c. 1,148 lines of C#/XAML and 746 of Swift. §83.5 fixes
the first version at five items and **two of them exist**: discovery and the window.

| §83.5 | State |
|---|---|
| Discovery | **Built**, both platforms, over each OS's own DNS-SD resolver. `check-skynet-service.mjs` holds all three implementations to one contract — 6 checks, green today |
| The window | **Built** on Windows. The macOS package is written and **has never been compiled**, on any machine |
| Pairing | Not started (§10, §50) |
| The on-robot console in a webview | Not started — no `WebView2`, no `WKWebView` |
| Black box download and replay | Not started |

Beyond the five: no USB path to a robot that has never been on a network, no tray stop (and when it
exists it is a convenience, **never** an emergency stop), nothing signed, no installer, no
Authenticode or notarisation, no published digest, no version-compatibility statement against
`skynetd`'s local API, and **no download surface on the Hub** (§83.3d) — there is no Console page
under `/skynet` at all.

One stale claim, and it is in a document a new reader starts from: `desktop/README.md` says
*"nothing advertises yet, so on a real network both applications find nothing"*. §82.4's responder
shipped on 2026-09-06 and `skynetd --lan` answers a browse; the sixty-ninth session ran it between
two daemons on one link. The sentence was true when it was written and is now the opposite of true.

**Nothing here is buildable in this container.** No `dotnet`, no `swift`. What can be worked on is
`desktop/shared`, the check script, the README, and the Hub-side download surface.

---

## What this round changes about the next one

The audit produced three defects, one closed-but-logged-open Hub item, one stale README claim, and
one correction to round twenty-two's closing paragraph. Nothing is committed and nothing is pushed.

## 80. The three defects, closed — and one of them was a published number

The audit's own list, worked in order.

**`reference/reports/editions.json` now says 1,142.** Regenerated by running both editions, which
is the only way that file is ever written — the generator refuses to compose one from a claim. The
Hub's mirror was resynced in the same pass and `check-contract-mirrors` re-pinned both trees, so
`/skynet/os/editions` stops rendering a figure five cases behind the corpus.

**`tools/edition-report.mjs --check` runs in CI**, at the end of the `corpus` job — the only job
where both editions have already been built. The comment beside it names the failure rather than the
rule, because the rule is obvious and the failure was not: every neighbouring check passed honestly
while the number was wrong. `check-contract-mirrors` compares the Hub's copy to the reference's and
both were stale together; `--min-passed 1109` is a ratchet, and a ratchet cannot catch a stale exact
figure because 1,137 clears it exactly as 1,142 does. Only re-deriving it can.

**The Hub's checkout has one spelling.** `scale-report.mjs` looked for two directory names where
`check-mirrors.mjs` and `check-contract-mirrors.mjs` looked for seven, so on this machine — Hub at
`../RobotOverflow` — two of the three found it and the third reported *"the Hub's mirror was not
measured: no checkout beside this one"* and exited zero. §73's one figure spanning both repositories
was unchecked and nothing said so. `tools/hub-path.mjs` is now the single list; all three import it,
and `scale-report --check` measures the mirror at 8,452 lines against §73's stated 8,000.

Three tool messages named `BotMarket` in text an operator reads, including two that told somebody to
run `--sync ../BotMarket` — a directory no clone has produced in months. Those are the current name
now. The historical prose explaining *why* the list has seven spellings is left alone: it is the
record of a rename, not an instruction.

## 81. §27's `guard_hold` — and the sentence that was true about the subject and false about the evidence

Round twenty-two closed by filing all four remaining §27 conditions as *"waiting on something that
is not a decision and not code anybody can write today"*, on the grounds that `observed.rs` gave
each a reason. Reading the reasons against the record format, one of them did not hold.

`guard_hold` was filed as **"the kernel's"**. So are `kernel`, `rhal` and `stop`, and all three were
closed in the previous rounds — because whose fact it is was never the question. What decides
whether a supervisor may observe a condition is **whether the kernel wrote it down**, and §26's
cycle record carries `response: Option<FaultResponse>` beside the flags this daemon already reads.

So `HoldSighting`, off the same newest cycle as `rhal` and `stop`, and two things had to be got
right that the field name does not suggest.

### An advisory hold is not a hold

§7.4 lets a tier that cannot promise a clamp run **advisory**: the guards evaluate, the verdict is
recorded, and the command is forwarded anyway. `limits::ROBOT_TIER` is guest, so *every robot on
this edition* is one — which means `Some(FaultResponse::Hold)` in a record here almost always
describes a hold that was demanded and never applied.

Mapping that onto §27's condition would report `HOLD` for a robot that is moving. That is the exact
failure §7.4 and §42 exist to prevent, and this repository has already made it once, inside `decide`
itself, where the `Hold` arm zeroed the mask on every verdict including advisory ones — *"a
guest-tier robot withheld a command the kernel had said to forward"*.

So `Advisory` is a **third state**, not a collapse into either neighbour. It is reported as an
observation, because a guard fired and somebody should see that; §27's condition stays false,
because nothing was withheld. `GET /state` carries `held` and `advisory` side by side for the same
reason: a console given only `held` could not tell a robot whose guards are firing and being
overruled from one no guard has touched, and those are opposite facts about the same still robot.

### A failed body read does not stop this one

`StopSighting` refuses a cycle whose `read_state` failed, and is right to: §26.4's fill puts the
*previous* cycle's flags in that record, so reading them would report a fill as an observation.

The guard verdict in the same record is **not a fill**. `decide` runs on every cycle, failed read or
not, and what it produces is what the kernel did with the command. So this reads a record the other
two refuse, and the split is the point — `rhal` and `stop` are facts about the **body**, and this is
a fact about the **kernel**. What the verdict was computed *from* on such a cycle is a fair question
and a different one: the guards saw stale positions, and the command was still withheld. A robot
that is not moving is the fact an operator needs before a philosophical one about the quality of the
inputs that stopped it.

### What it does not do, which is most of what it changes

Nothing about a robot's **state**. `guard_hold` is rule 7 of `derive`; above it sit the self-test
(4), homing (5) and the kernel (6), and `homing` defaults to `Pending` on every robot here because
§24 gives the ABI no flag for it. So this edition goes on reporting `unhomed` while carrying a
perfectly good guard-hold observation, and the route test asserts that ordering rather than leaving
somebody to discover it.

`homing` is now the only one of §27's eight that no reader can close. `guards` and `policy` remain,
and both have candidate sources in this process — the audit above names them.

### The count moved in four documents, and the mechanism found all four

`scale-report --check` parses the literal `false` entries out of `Observed::of` and holds four
sentences to the result. Changing one entry failed the check in §73's `§27` row, §73's `Audio` row,
§81's closing paragraph and the README simultaneously. That is the third round in which this figure
would otherwise have gone stale somewhere, and the first in which nobody had to remember it.

### Two surfaces gained something, and one deliberately did not

`skynet state`'s per-condition mark reads `flagged()`, which knew one key — `asserted`, the stop's.
A `guard_hold` block has `held` and `advisory` and no `state`, so the CLI would have drawn a green
tick beside a robot whose command was being withheld: the same error the printer's own comment
records having fixed once for `kernel`. It reads all three keys now, `advisory` included, because a
guard firing into a tier that will not act on it is precisely something to look at.

§32.2's console gained a **Guard holding** row, and only for the advisory case's sake: an enforced
hold already shows in the state line as `hold · guard_hold`, and an advisory one had no surface
anywhere except the word `guard_hold` in a list of conditions somebody looked at. The row is omitted
entirely when the condition was not observed rather than drawn as a reassuring "no".

The per-condition detail blocks for `kernel`, `rhal` and `stop` are still not rendered on that page
and this did not add a fourth. Adding one would have made the panel inconsistent about three
conditions in order to be complete about a fourth.

## Run

```
cargo test --workspace                      1,680 passed, 0 failed  (1,676 before)
cargo clippy --workspace --all-targets      clean
cargo fmt --all --check                     clean
sh crates/skynetd-io/e2e.sh                 all checks passed
python3 editions/python/run_corpus.py       1,142 of 1,142, both editions
node tools/edition-report.mjs --check       matches
node tools/scale-report.mjs --check         §73 matches the tree, Hub mirror measured
node tools/check-mirrors.mjs                all shared documents match, re-pinned
node tools/check-contract-mirrors.mjs       every mirrored contract matches
node tools/check-golden-path.mjs            4 documents, 51 commands
Hub:  npx vitest run                        1,895 passed, 84 files
      npm run typecheck                     clean
      node scripts/check-skynet-service.mjs 6 checks, no divergence
```

The e2e ran against a real daemon and a real control loop in another process, and the guard-hold
reading is taken off cycles that loop wrote: *"and no guard is holding the command"* is the
negative asserted on a running machine rather than in a fixture. Its systemd launch step did not
run — this container has the binaries but no manager (`systemctl`: *"System has not been booted
with systemd as init system"*), which is an environment limitation and not a skip the suite chose.

## What round twenty-three leaves

* **`guards` and `policy`**, the two §27 conditions the audit found candidate sources for.
  `guards_unavailable` is the sharper of the two: `crate::limits` already resolves the guard
  partition on this daemon, so the reading exists and what has to be decided is whether a partition
  computed from a descriptor is an *observation* in §27's sense on a robot that has never run a
  cycle. That is a question about evidence, not a mapping.
* **`homing`** — an ABI change, and the ABI is the one thing a vendor compiles against.
* Everything the audit filed under hardware, an owner's decision, or the Hub's Phases 10–12.

Nothing is committed and nothing is pushed.

---

# Round twenty-four — 2026-09-07

## 82. §27's `guards`, and the second source the field's own documentation named

The audit's other candidate, and it went the same way as `guard_hold`: the reason for leaving it
out was a true sentence about the wrong place.

`observed.rs` had it as *"a cycle records what guards did, not which were available"*. Correct about
§13's ring, and §7.6's partition does not live there — it is resolved **on this daemon**, by
`resolve_limits`, served at `GET /limits`, rendered by `skynet limits` and the on-robot console, and
carried across §17 to the device page. *Which guards are unavailable* was being computed, drawn in
two places and reported to the Hub every check-in, while §27 took it at a default.

So `GuardsSighting`, and it is the first of §27's conditions that comes off no ring at all.

### The second list, which the condition documents and nothing read

`skynet-contracts` annotates `guards_unavailable` as *"§27's `DEGRADED` row: a guard is unavailable.
**§45.5 is the usual source**"* — and §45.5 is calibration expiry, whose guard list is
`calibration::Status::unavailable`. That is a **different list** from `resolve_limits`'s, produced
by a different module, already consumed by `skynet calibration` and by POST's own
`calibration_expiry` check, and it had never met §27.

Either list alone is right about half the robots. A body that declares no IMU loses `balance`
through the resolver with every calibration in date; a body with a flawless descriptor and a camera
extrinsic that expired forty days ago loses `proximity` through §45.5 and nothing in the resolver
notices. The *usual* source is the one that was missing.

So the condition is the union, merged rather than concatenated — a guard that loses both its
declaration and its calibration is one problem, not two — and each entry says which source took it
out, because *fit the sensor* and *recalibrate* send an operator to different parts of the building.
Where both are true the calibration wins the label: it is the one they can act on today.

### Resolved, not per-cycle, and said so on every surface

What this observes is which guards **can** run given what the body declares and what a calibration
still believes. It is not a reading of which guards the kernel had inputs for on the last cycle, and
those differ — §80's `inert` state exists because of exactly that gap. A declared sensor that fails
at runtime is still invisible here, and that is a per-cycle fact belonging with the ring readings.
Claiming it would be the overreach the type is arranged against, so `GET /state`'s `source` string
says what it is and what it is not rather than leaving a reader to assume the stronger reading.

### A robot nobody described is not a robot with broken guards

With no descriptor `resolve_limits` finds nothing declared and reports nearly every guard
unavailable. Arithmetically right, and as an answer to §27 it would tell an operator their guards
are broken when what is missing is the document describing the robot. So `Unobservable` is a
variant, decided on a new `described` flag rather than on `envelope.is_some()` — a descriptor with
no `envelope` block is a body somebody described without ceilings, and a robot with no descriptor is
a different thing that leaves the same field `None`.

The suite asserts the distinction the hard way: it first checks that the resolver *does* report
guards unavailable for an undescribed body, so the variant is proven necessary rather than
decorative.

### Surfaces

`GET /state` gains a `guards` block — the boolean, the observation, the sentence, and the guards as
data with their source, because §45.5's requirement is that an unavailable guard say *why* and a
console given only a sentence cannot group them.

`skynet state`'s `flagged()` reads a fourth key. The on-robot console gains a **Guard set** row
beside the guard-hold one, and the pairing is the point: *is a guard acting right now* and *can they
act at all* are different questions, and an operator told "no guard is holding" about a robot whose
proximity guard has no calibration has been told something true and reassuring about a robot that is
not protected.

A test helper named `guards` in `limits.rs`'s suite was renamed to `partition`. It predated the
module function and would have shadowed it — a helper shadowing the thing under test is how a suite
ends up proving something about itself.

### The count, again

Six of eight now, and `scale-report --check` failed in four documents plus the line count, which had
crossed a thousand-line boundary in the same pass. Second round running that this mechanism has
caught every stale copy without anybody remembering where they were.

## Run

```
cargo test --workspace                      1,682 passed, 0 failed  (1,680 before)
cargo clippy --workspace --all-targets      clean
cargo fmt --all --check                     clean
sh crates/skynetd-io/e2e.sh                 all checks passed — six observed conditions on a
                                            running daemon, and the partition naming what each
                                            guard wants
python3 editions/python/run_corpus.py       1,142 of 1,142, both editions
node tools/edition-report.mjs --check       matches
node tools/scale-report.mjs --check         §73 matches the tree
node tools/check-mirrors.mjs                all shared documents match, re-pinned
node tools/check-contract-mirrors.mjs       every mirrored contract matches
node tools/check-golden-path.mjs            4 documents, 51 commands
```

The e2e assertion is the **positive** one — the mock body declares no IMU and no person detector, so
the check is that guards are unavailable and the robot says which and why, rather than a hopeful
"all available" that would pass on a robot that had resolved nothing.

## What round twenty-four leaves

* **`policy`** — the last of §27's eight with a candidate source in this process. `Cycle.policy`
  names the commanding policy on every recorded cycle and `shape::supervising` already resolves
  which package holds the commanding plan. What has to be decided is whether *a policy is
  commanding* and *a lease is granted* are the same fact, because §25.2's lease table is the
  kernel's and the ring shows the consequence rather than the grant.
* **`homing`** — an ABI change, and the ABI is the one thing a vendor compiles against. It is now
  the only one of the eight with no path at all.
* A **runtime** guard availability reading, which is what would close the gap this round's type
  documents: a declared sensor that fails mid-run is still invisible.

Nothing is pushed.

---

# Round twenty-five — 2026-09-07

## 83. §27's `policy` — the only condition whose honest answer is *partly* unobservable

The last of §27's eight with a source in this process, and the first where the answer is not simply
*yes it is readable* or *no it is not*.

`PolicyState` has three values. Two of them this daemon can establish **exactly**:

* **`None`** — *nothing to run.* Nothing installed declares an action space, or nothing describes
  the body. This process holds the store; that is not a question for anybody else.
* **`Held`** — *something to run that is not ready to*, in four distinct ways: the plan is refused,
  the manifest cannot be read, two packages both command (§80 leaves both without a ring for the
  same reason), or the plan resolves and the unit is not running — a policy whose process does not
  exist holds no lease.

The third is `Ready` — *lease granted and plan resolved* — and it is not this daemon's on any
input. It resolves the plan itself and it launched the unit, so it has both halves of the
*necessary*; §25.2's lease is the Safety Kernel's table and no amount of care here reads it. So that
one case reports itself **unobserved**, is taken at `held`, and `ready` is a value `PolicySighting`
cannot produce. The unit check is not a §33.1 crossing for a reason worth stating: the daemon
started the unit, so whether the unit is running is its own action's result rather than a question
to the kernel.

### And it closed a defect that had been live since §27's machine was written

The condition sat at `Conditions::default()`, which is `PolicyState::None`. §27's rule 8 splits
`no_policy` from `policy_not_ready` deliberately — *"there is nothing to run"* and *"there is
something and it has not got a lease"* are different sentences and the second is the one with a
remedy, because §25.2's arbitration is where an operator goes to find out who else is holding the
arm.

So a robot with a commanding package installed **and activated** was reporting *there is nothing to
run*. That is not a cautious reading of an unobserved condition. It is a false statement about a
store this process holds, made by the process that holds it, and the cautious reading was available
the whole time: `Held` is weaker than `Ready` and is the only one of the three that is true.

It was latent because `homing` is rule 5 and stops every robot on this edition three rules earlier,
which is why the suite drives §27's machine directly — `post` passed, `homing` not required,
`kernel` live, `rhal` present — to reach rule 8 and assert both sentences.

### Clippy caught the shape before a person did

`refresh_operating_state` reached eight arguments, six of which read as "a sighting". They are a
struct now — `observed::Sightings` — and the reason is not only ergonomics: **§27's conditions are
only comparable when they are readings of one instant**, which `Recorder::chain` already says where
the ring is scanned. A struct assembled once per pass says that; six parameters invite a caller to
gather them at six different moments. `Daemon::sightings()` is the other half — two routes and the
banner take the same six from one place rather than each naming six fields.

### The count check needed English

Four sentences reached **one** unobserved condition, and one is singular. Rewording them made every
pattern in `scale-report --check` miss — and the check said so, reporting *"no document states how
many of §27's conditions are unobserved any more, so this check is guarding nothing"* rather than
passing. That `found` is counted separately from the failures is what turned a grammar change from
a silent hole into a message. The patterns take `is` or `are` now, and carry a note that the next
boundary is `none`.

## Run

```
cargo test --workspace                      1,684 passed, 0 failed  (1,682 before)
cargo clippy --workspace --all-targets      clean, no warnings
cargo fmt --all --check                     clean
sh crates/skynetd-io/e2e.sh                 all checks passed — seven observed conditions on a
                                            running daemon, and the policy condition naming the
                                            installed package instead of reporting nothing to run
python3 editions/python/run_corpus.py       1,142 of 1,142, both editions
every check tool                            exit 0
```

## Where this leaves §27

**Seven of eight.** `homing` is the last, and it is the only one where the reason first written for
all eight survives contact: §24 gives the ABI no flag for it, so it is not written down anywhere
this process — or any process — could read. Closing it is an ABI change, and the ABI is the one
thing in this system a vendor compiles against.

Seven of eight does not buy a robot that commands. `homing` is rule 5 of `derive`, so this edition
still resolves `unhomed` however good the six readings below it are. What changed over these three
rounds is that the state rests on readings, and every condition a console shows beside it is a
measurement rather than a default dressed as one.

Nothing is pushed.

---

# Round twenty-six — 2026-09-07

## 84. §27's `homing` — the eighth, and the one whose reason was wrong rather than stale

The last of §27's eight conditions, and it did not close the way the other seven did.

Each of those closed by finding that the evidence was **already written down** somewhere this
process holds — §13's ring for four, §7.6's partition for the fifth, the store and the units for
the sixth and seventh. `homing` was not written down anywhere, and this repository had drawn a
conclusion from that and stopped:

> §24 gives the ABI no flag for it, so it is not written down anywhere at all and needs a vendor to
> recompile rather than a daemon to look somewhere else.

The first clause is true. **The second does not follow from it, and nobody rechecked it for
months.** §24's `flags` is a `u16` carrying four flags — `ESTOP`, `FAULT`, `DEGRADED`, `BRAKED` —
and **twelve spare bits**. A field added to a bitwise-or word moves no offset, changes no struct,
bumps no `SKYNET_RHAL_ABI_VERSION`, and breaks no driver ever compiled against v1: a driver that
has never heard of the field leaves it zero, and zero is a value that can mean *this driver does
not answer*. The whole of "the ABI break" was a two-line `#define` block.

What a vendor recompiles for is **answering**. That is a much smaller claim than the one that had
been standing in for it, and it is the same claim already recorded against every other row in §73
— no vendor has written a driver.

### Four states, and the argument for the fourth

```
0  SKR_HOMING_UNREPORTED    this driver does not answer the question
1  SKR_HOMING_NOT_REQUIRED  absolute encoders; there is nothing to home
2  SKR_HOMING_PENDING       joint positions are not yet trustworthy
3  SKR_HOMING_COMPLETE      homed, and the positions mean what they say
```

A single `SKR_FLAG_HOMED` bit was the obvious encoding and it is the wrong one. A clear bit would
mean *this robot is not homed* **and** *this driver has never heard of homing* at the same time,
and those two send an operator to different parts of the building — one to the homing routine, one
to the vendor. §7.2's rule is that a measurement nobody can attribute is not one. Two bits carry on
the wire the `Option` the type carries in memory, and the cost of the distinction is one sentence
while the cost of losing it is telling somebody to home a machine that has no homing routine.

`not_required` is the **driver's** to declare and not the descriptor's, which was the other
candidate. Whether a body needs homing is a property of the encoders actually fitted to the machine
in front of the driver; a descriptor is a vendor claim written once and shipped, and the two
disagree the first time somebody retrofits an absolute encoder — with the descriptor being the half
that is wrong.

### Reading it is the reading the other four already do

`HomingSighting` is off §13's ring, off the newest cycle, and it refuses a cycle whose `read_state`
failed for **`StopSighting`'s** reason rather than `HoldSighting`'s: §26.4 fills a failed cycle's
flags from the previous cycle, so the field in such a record is a copy. Reading `complete` off a
fill would release a robot on evidence nobody gathered, which is the one direction this must never
fail in. It parts company with `guard_hold` deliberately — a guard verdict on a failed cycle is
what the kernel *did*, and this is a claim about what the driver *saw*.

`Unreported` counts as **observed**, which is the one judgement in the type worth arguing. This
daemon looked, the ring answered, the loop is turning, and what came back is a driver declining to
say. That is a measurement of the driver. Counting it unobserved would put every robot on a v1
driver back where the condition started and throw away the distinction the field was added to make.

The bits reach `skynetd-io` as a **number out of a record it already holds**, restated rather than
imported: §33.1 forbids that crate an edge to `skynet-rhal` and CI asserts the edge is absent. The
variant carries §27's own `HomingState` rather than a second spelling of the enum, so only the wire
numbers are duplicated.

### What it moved, which is the first time closing a condition moved anything

Seven of eight bought no change of state, because `homing` is **rule 5** of `derive` and sits above
the kernel (6), the guard hold (7) and the policy (8). Every robot on this edition resolved
`unhomed` however good the readings below it were — four conditions being read and reflected in
nothing.

A body reporting `not_required` now passes rule 5 and stops at **rule 8**. That is the honest
blocker: §25.2's lease is the Safety Kernel's table and `PolicySighting` cannot produce `Ready` on
any input. The robot's reason changes from *"nobody has homed this machine"* — false, about a
machine with no homing routine — to *"there is nothing here holding a lease"*, which is true and
has a remedy.

### The driver set answers three different ways, on purpose

| Driver | Reports | Why |
|---|---|---|
| `drivers/mock` | `not_required` | `ctx->position` **is** the joint position; there is no zero to lose |
| `drivers/mujoco` | `not_required` | positions come out of `data.qpos`, same argument |
| `drivers/lerobot` | **nothing** | and this is the interesting one |

The LeRobot bridge is the one place in the driver set where the answer is genuinely unknown. Its
robot classes expose no homing or referencing state: a follower arm may be at its calibrated zero
or may have been power-cycled with the leader somewhere else, and nothing in the observation says
which. `complete` would be inventing it, and `not_required` would be worse — these are relative
encoders. So it leaves the field zero, which is exactly the state the fourth value exists to
express, and the robot is held.

## 85. A fourth spelling, and the mechanism that arrived with it

§24's flag values are now written in four places: the C header, `skynet-rhal`, the Python binding
and `skynetd-io`'s restatement. The fourth **cannot be removed** — an import there would put the
supervisor one `use` away from the actuator path — so the duplication is load-bearing and
permanent, which is §78's shape exactly and §78's rule is that the remedy is a mechanism.

Three mechanisms already held the ABI's **layout** (`layout_matches_the_header`,
`assert_abi_layout`, `header_check.cpp`). Nothing held the flag **values**, and the failure
signature is why that matters: a wrong mask does not crash, it reads a different field. A homing
field overlapping `SKR_FLAG_BRAKED` would put *ready to command* on the console of a robot whose
brakes are on, with both halves looking correct in isolation.

`tools/check-rhal-flags.mjs` parses all four and holds them to the header. It **found a real
asymmetry on its first run**, and the fix was to teach the check rather than to change the code:
the Rust view spells the four states as `enum Homing` rather than four constants, and that is
*better* than C — the zero has no variant because it is `Option::None`, so a Rust driver author
cannot treat "the driver did not answer" as a state. The check reads the discriminants and shifts
them, and records `HOMING_UNREPORTED`'s absence there as correct, with the reason. A parser that
had forced four flat constants into the crate would have made the code worse to satisfy a tool.

The overlap assertion derives the flag union **from the header** rather than listing today's four,
because the flag it would catch is the one somebody adds next — a hardcoded list goes stale in
exactly the commit that introduces the collision, and then passes.

Mutation-checked, three ways: a drifted Python shift, a drifted Rust enum discriminant, and a fifth
C flag moved onto bit 4. All three fail; restoring each passes.

### Two documents were wrong before this and the check said so

`scale-report --check` failed on four sentences at once — §73's two rows, §81.5 and the README —
which is what it is for. The count reached the boundary its own header predicted in writing
(*"this comment is here because the next boundary is `none`"*), and `NUMBER_WORDS` already had
`none: 0`, so the grammar held. The Rust line count moved 122,000 → 123,000 in the same pass.

## Run

```
cargo test --workspace                      1,621 passed, 0 failed  (1,618 before; Windows host,
                                            so the unix-gated tests are not in this number)
cargo clippy --workspace --all-targets      clean, no warnings
cargo fmt --all --check                     clean
node tools/check-rhal-flags.mjs             10 constants agree across 4 definitions
node tools/scale-report.mjs --check         §73 matches the tree
node tools/check-mirrors.mjs                all shared documents match, re-pinned
node tools/check-contract-mirrors.mjs       every mirrored contract matches
node tools/check-golden-path.mjs            4 documents, 51 commands
```

`crates/skynetd-io/e2e.sh` and the Python corpus runner were **not** run this round: no POSIX
daemon and no `python3` corpus host on this machine. Nothing in this round touches a contract, an
edition or the corpus — §24's ABI is single-source by §31.2 and is deliberately not corpus material
— so the editions are unaffected by construction rather than by assumption.

## What round twenty-six leaves

* **All eight of §27's conditions are readings.** There is no ninth, so this sequence is finished
  rather than advanced.
* **Rule 8 is the wall now**, and it is a real one: §25.2's lease is the kernel's table. A robot
  reaching a commanding state — and therefore §49's tone ever sounding — needs that lease readable
  or granted, which is the next thing in this area worth designing.
* No robot has run any of this. The mock and MuJoCo answers are correct answers from simulated
  bodies; a driver on real hardware reporting a real homing state is still a vendor's to write.

Nothing is pushed.

---

# Round twenty-seven — 2026-09-07

## 86. §36.4's brownout override — the same wrong reason, one section over

Round twenty-six's finding was that §73 had inferred *"needs a vendor to recompile"* from *"§24 has
no flag"*, and that the inference was invalid. Looking for the same shape elsewhere found it
immediately, in §36.5:

> **§36.4's override has no input on this edition.** `RHAL_DEGRADED` and its fault code come from
> the RHAL, and §33.1 keeps `skynetd` off that path — the daemon does not depend on the RHAL crate
> and CI asserts the edge is absent.

Every clause of that is true. The conclusion does not follow, and this is now the **seventh** time
in this repository that the same invalid step has been taken: *what decides whether a supervisor may
observe a fact is not whose fact it is, but whether it was written down somewhere this process
already holds.* §26's cycle record carries `state_flags` and `fault_code` verbatim, and
`RhalSighting` — §27's `rhal` condition — had been reading **both of them off that record** since
before the paragraph above was written. §36.4's override was one struct field away from an input.

§33.1 is untouched: no RHAL dependency, no actuator path, a number out of a file the daemon holds.

### What it turns on

The contract half was complete and tested the whole time. `assess` matches the vendor's code against
the descriptor's `brownout_fault_codes` — never interpreting it, which is what keeps §24's
*"vendor-specific"* rule intact — and produces `critical`/`brownout` past a healthy gauge, or
`brownout_undeclarable` when the list is empty. Both are reachable now, and neither had ever been
produced by any robot.

### `BusReading`, and the third state the contract cannot hold

`PowerObservation::rhal_degraded` is a `bool`, so *the bus was read and is healthy* and *nothing has
read the bus* both arrive at §36's rules as `false`. The old module header was right that the
difference matters, and it is kept rather than lost to the plumbing: `BusReading` has three
variants, `GET /power` carries `bus_observed` beside the state, and the note is a reading instead of
a constant sentence claiming no robot could ever report a brownout.

The contract is deliberately **not** widened to an `Option`. It is mirrored into three editions and
pinned by the corpus (§31.4, reference-first), and the fact that would justify that ripple — *this
robot's ring holds no control cycles* — is a property of one daemon rather than of §36's rules. The
party that knows says so, on its own surface. That also keeps this round inside one repository,
which is the constraint the `detects_people` attempt discovered the hard way.

**A faulted or lost RHAL is `Unobserved`, not `Healthy`.** The one mapping decision here. §36.4 asks
whether the bus is sagging, and a driver that has stopped answering has not said that it is not;
reporting the reassuring half of a fact nobody has is the failure this module is arranged against.

The gap that remains is **fail-open** and is now named rather than implied: §26.2 gives a
supervisor's ring the event channel and a control loop the cycle channels, so a robot running no
control loop records no cycle for this to read, and reports `normal` while its bus sags. Same shape
as §27's `kernel` reading, and the route says so.

## 87. The half that is genuinely §33.1's, checked rather than assumed

§36.1's `reserve` and `critical` rows ask for a **controlled stop**, and the audit listed it beside
the brownout as though the two would close together. They do not, and reading §36.1 is what settles
it: a controlled stop is a ramp to a park, sit or dock pose — *motion* — and §36.1 says in its own
words that *"a policy dropped mid-motion is the failure a controlled stop exists to avoid"*.

A supervisor stopping the commanding unit would be doing precisely the thing the row exists to
prevent, dressed as compliance with it. So this half stays open, and §73's row now says which half
is which instead of pairing a correctable claim with an uncorrectable one.

## Run

```
cargo test --workspace                      1,623 passed, 0 failed  (1,621 before)
cargo clippy --workspace --all-targets      clean, no warnings
cargo fmt --all --check                     clean
node tools/scale-report.mjs --check         §73 matches the tree
node tools/check-rhal-flags.mjs             10 constants agree across 4 definitions
node tools/check-mirrors.mjs                all shared documents match, re-pinned
node tools/check-golden-path.mjs            4 documents, 51 commands
```

Three new tests: the route in all three bus states, `brownout_undeclarable` on a body that declares
a pack and no codes, and the mapping — including that a lost RHAL carrying a fault code takes
neither the flag nor the number.

## What round twenty-seven leaves

The pattern is now worth stating as a rule rather than as an anecdote: **before recording that a
fact is unobservable because of §33.1, check whether §26 already writes it down.** Seven of the
things this repository filed under "the kernel's" were in the ring the whole time. The ones that are
genuinely not are the ones requiring the daemon to *act* on an actuator — asserting a stop, ramping
to a park pose — rather than to know something.

Nothing is pushed.

---

# Round twenty-eight — 2026-09-07

## 88. §80's `inert` was decided on a constant, and the constant was the bug

Round twenty-six's rule — *before recording that a fact is unobservable because of §33.1, check
whether §26 already writes it down* — found its third target the same day, and this one is different
from the first two: nothing here claimed the fact was unobservable. It just never asked.

§80 raised `inert` on 2026-09-07 for a guard *"whose every declared input resolves and whose
per-cycle reading never arrives"*, and built it against `available.proximity_reading` — hard `false`
in **two** files, the daemon and the kernel host, each carrying the note *"the day a producer
exists, this is the line that changes."*

That note is the whole problem. It is correct today, it is a promise about a future edit, and the
failure mode is silent in the worst direction: **the day a producer existed, a robot would go on
drawing a working proximity guard as inert, and nothing anywhere would say so.** A console renders
that partition. §7.2's rule is that a measurement nobody can attribute is not one, and a constant is
not a measurement at all.

`cycle.nearest_person_m` is a field in every cycle record. The daemon holds the ring.

## 89. An interval, not an instant — the one reading in the system that is counted

Every other ring reading here is a **state**: `rhal`, `stop`, `guard_hold` and `homing` are all
questions about *now*, so the newest cycle decides them.

*Does a producer exist* is not a state, and taking it off the newest cycle would have been the
obvious mistake. A detector at 30 Hz under a 1 kHz control loop leaves `NaN` in roughly thirty-two
cycles out of thirty-three — so a newest-cycle reading would report a **working** detector as absent
on almost every pass, and flicker between active and inert at the sensor's beat frequency.

So `ProximitySighting` counts, like `Recorder::tally`: how many cycles in the interval, and how many
carried a number. `Counted { cycles: 4000, filled: 120 }` is a healthy 30 Hz sensor. The count is
carried rather than a boolean because **0 of 4,000 and 0 of 0 are different problems** and send an
operator to different places.

Two things are counted in neither, and both are §26.4 one field along: a cycle whose body read
failed carries the previous cycle's values, so a `NaN` in a fill is not evidence that nothing
produced a reading and a number in one is not evidence that something did.

The counting is a **pure function over an iterator of cycles**, for the reason `KernelSighting::
from_tally` is pure: only a control loop can put a cycle in a ring (§26.2), so a rule reachable only
through a disk would be one this crate's own suite could never exercise.

## 90. Half of a gap the resolver had already named

`GuardsSighting`'s header has said since it was written:

> The gap that remains is a declared sensor that fails at runtime: the descriptor goes on declaring
> it and nothing here notices. **That is a per-cycle fact and belongs with the ring readings.**

It does, and it is there now. A detector that was filling `cycle.nearest_person_m` and stops moves
`proximity` from **active to inert within one interval** — a console stops drawing protection the
robot no longer has, with nobody editing a descriptor. That is the direction that matters.

It is **half**, and the header now says which half and why: that is the only sensor whose per-cycle
reading appears in a record at all. An IMU that fails is still invisible, because §26 carries the
base pose and twist a driver reported rather than a per-sensor liveness, and `balance` resolves off
the descriptor's `imu` declaration alone. Claiming otherwise would be the overreach the type exists
to prevent.

Fail-closed throughout: an unreadable ring, an events-only ring and an interval with no cycles all
resolve the guard **inert** rather than active. The alternative was available and is a robot
claiming to watch for people on the strength of a file it could not open.

## Run

```
cargo test --workspace                      1,627 passed, 0 failed  (1,623 before)
cargo clippy --workspace --all-targets      clean, no warnings
cargo fmt --all --check                     clean
node tools/scale-report.mjs --check         §73 matches the tree (Rust 123k → 124k)
node tools/check-rhal-flags.mjs             10 constants agree across 4 definitions
node tools/check-mirrors.mjs                all shared documents match, re-pinned
node tools/check-golden-path.mjs            4 documents, 51 commands
```

Four new tests: the counting and both §26.4 skips as a pure function; an events-only ring saying
nothing; a detector that stops filling moving the guard active → inert with no other change to the
body; and an unobserved reading leaving it inert rather than active.

## What round twenty-eight leaves

The kernel host's own `proximity_reading: false` stays, and it is **not** the same kind of constant:
that process knows its own backends, so it is reporting a fact about itself rather than inferring
one from an absence. The daemon is the one that had to look.

Three of §33.1's "unobservable" claims have now turned out to be facts already in the ring. The ones
that remain are the ones requiring the daemon to **act** on an actuator rather than to know
something — §36.1's controlled stop, §48.2's estop assert-and-observe — and those are correctly
where they are.

Nothing is pushed.

---

# Round twenty-nine — 2026-09-07

## 91. §80's "a loop nothing declared", and two sentences that had been run together

The last of §80's open rows with a source in this repository, and it closed the way the others did:
by rereading the reason rather than by building something large.

> **A loop nothing declared.** `skynetk` run by hand against a driver names no package, so a robot
> with a body and no commanding package still meets the shape refusal. That is the honest answer
> rather than a gap: nothing on that robot declares a policy, and the daemon inventing one would be
> sizing evidence for a loop that exists only in an operator's terminal.

Two sentences, and only the first is true:

1. **The daemon must not invent a loop.** Correct, and it is why nothing is inferred from whichever
   driver a bench operator happens to be pointing at.
2. **Therefore a bench loop cannot be recorded.** Does not follow. An operator saying *I am about to
   run a loop this wide* is not the daemon inventing anything.

`state/bench.json` is that declaration, and it is not a new idea in this system — it is §34.3's rule
applied for the **fourth** time. `power.json` names which of two batteries is the traction pack
because no measurement can. `brownout_fault_codes` names which vendor codes mean sag because §24
forbids the OS interpreting one. Bearers are declared for the same reason. *The parts a machine can
measure are measured, the parts it cannot are declared, and every surface says which is which.*

The joints stay **measured**, off the descriptor. Only the width is a person's word.

### Weaker than a plan, in three ways, each of which would otherwise hide something

A transport plan is two signed documents; a declaration is somebody's word. So:

* **A commanding package always wins.** A bench file left on a robot that later installs a policy
  changes nothing — otherwise the ring would be sized by whoever wrote a file last.
* **It cannot resolve `Refused`, `Ambiguous` or `Unreadable`.** Those are robots with a problem an
  operator needs to see, and papering over `Ambiguous` would hide precisely the case §80 leaves both
  packages unringed for: a robot where nobody knows which policy would command the arm.
* **It is not a policy.** §27's condition still reads `no_policy`, because §25.2's lease is granted
  to a policy the Safety Kernel knows about and a loop started by hand holds none and never will.
  The file changes the shape of a recording and deliberately nothing else.

### Every malformed value falls back, and the zero is the one worth arguing

Eight cases are pinned, and the interesting one is `{"channels": 0}`. Reading it as *the default*
would be defensible and wrong: zero channels is events-only, which is what the robot already does,
so nobody would ever write it deliberately — and a reader that accepted it would let a typo silently
disarm a bench session. It is refused, like a negative, a fraction, a value above `u16`, a string,
an absent field and a file that is not JSON. The fallback is safe by construction: events-only was
correct before this existed.

The operator line says **declared** rather than resolved, so a window recorded under it is never
read as evidence about a policy.

## 92. Two items checked and closed as correct rather than built

The audit that opened this session listed both, and reading them settles both:

* **§78's two spellings** of a package's requirements. Not a gap — §78.2 is a **decision**, and it
  is fully implemented: the union on read, the publish refusal in one direction,
  `embodiment_undeclared` as the seventeenth rejection code in all three editions, and corpus cases
  in `admission/install.json` and `vocabulary/closed-sets.json`. The duplication is load-bearing
  because `embodiment.capabilities` is normative and `required_capabilities` is cited by digests in
  published benchmark results.
* **§48.2's five `elsewhere` checks.** Also correct. The tempting one was
  `describe_matches_descriptor` — the ring records a joint count, so why not compare it? Because the
  ring's shape is **sized from the descriptor** by this same daemon, so the comparison is circular.
  The other four need the daemon to *act* on an actuator: load a driver, call `describe()`, assert a
  stop, read joints at boot. That is the line §33.1 actually draws, and it holds.

## Run

```
cargo test --workspace                      1,631 passed, 0 failed  (1,627 before)
cargo clippy --workspace --all-targets      clean, no warnings
cargo fmt --all --check                     clean
node tools/scale-report.mjs --check         §73 matches the tree
node tools/check-rhal-flags.mjs             10 constants agree across 4 definitions
node tools/check-mirrors.mjs                all shared documents match, re-pinned
node tools/check-golden-path.mjs            4 documents, 51 commands
```

## What round twenty-nine leaves

The rule these four rounds found is worth keeping: **before recording that a fact is unobservable,
separate *whose fact it is* from *where the evidence lives*, and separate *the daemon must not
guess* from *therefore nobody can say*.** Four rows closed on the first distinction and one on the
second, and none of them needed a new subsystem.

What is genuinely left in the OS is short, and none of it is a reasoning error:

* **The vision mapping half and an inference backend** (§80, §23.3) — real design work, and §80 says
  in its own words that starting it as a side effect of something else produces a bad version of it.
* **§79's two halves joined to each other** — each is exercised against a double; joining them needs
  a running Hub beside a running robot.
* **A third edition** — excluded by the owner from this pass.
* **Hardware** — the certified tier, a physical joint, a TPM, a vendor driver, an applied OTA.

Nothing is pushed.

---

# Round thirty — 2026-09-07

## 93. Run on a Linux machine, and the e2e found what four unit suites could not

The four rounds above were verified on a Windows host, where every `#[cfg(unix)]` test is compiled
out and `e2e.sh` cannot run at all. A WSL2 Ubuntu with a Rust toolchain was sitting on the same
machine, which makes the honest verification available rather than deferred — so it was run.

**1,702 tests rather than 1,631**, and the seventy-one difference is entirely the POSIX-gated ones:
symlink activation, Unix sockets, `SCM_RIGHTS`, the cross-process handoff. All pass.

`e2e.sh` — a real `skynetd`, a real `skynetk` against the mock driver, two processes sharing one
recording — reported **one failing check**, and it is the useful kind:

```
✗ and §27 now counts seven observed conditions
  expected 'seven of §27's eight conditions'
```

The script's own expectation was the stale thing. The daemon under it was reporting:

```
✓ homing
  eight of §27's eight conditions were observed here
```

So `homing` is observed **on a running robot**, off a driver that answers, through the ring — which
is the verification round twenty-six could describe and not perform. Both counts are now asserted
and the script says why they are two lines rather than one: a driver that had *not* been updated
would leave §24's field zero and the count would still be eight, because `unreported` is an
observation. So the second line asserts the **answer** — the mock's `not_required` — and the first
asserts the count.

## 94. A tick with nothing beside it, which §32.2 forbids

The e2e's rendering showed the defect the unit tests could not, because no unit test renders:

```
✓ rhal        present  the newest cycle read the body and reported it present
✓ homing
```

`HomingSighting::detail()` existed, was tested, and reached no surface: `GET /state` carries a block
per condition — `stop`, `guard_hold`, `guards` — and `homing` had none. A console showing that a
condition **was observed** while showing nothing about **what was observed** is exactly §32.2's
rule failing, and it would have shipped as a green tick.

The block carries `state`, `observed`, `detail`, `source`, and one field that is not on its
neighbours: **`reported`**, the boolean separating *this driver does not answer* from *this robot is
not homed*. That distinction is the reason §24's field is two bits, and a console that had to grep
the prose for it would eventually stop trying.

The CLI needed one more thing. Its `good` vocabulary — the list deciding a green tick from a warning
mark — was `["live", "present", "passed", "none"]`, so `not_required` drew a **warning** on a
perfectly healthy bench robot. `not_required` and `complete` join it; `pending` deliberately does
not, and it is the row an operator most needs marked, because it covers both *nobody has homed this
machine* and *this driver does not report homing*. Which of the two is in the detail beside it.

## Run — on Linux this time

```
cargo test --workspace                      1,702 passed, 0 failed   (1,631 on the Windows host)
cargo clippy --workspace --all-targets      clean, no warnings
cargo fmt --all --check                     clean
sh crates/skynetd-io/e2e.sh                 all checks passed — eight observed conditions on a
                                            running daemon, and the homing state naming the
                                            driver's answer
python3 editions/python/run_corpus.py       1,142 of 1,142, both editions
node tools/check-rhal-flags.mjs             10 constants agree across 4 definitions
node tools/scale-report.mjs --check         §73 matches the tree
node tools/check-mirrors.mjs                all shared documents match
node tools/check-contract-mirrors.mjs       every mirrored contract matches
node tools/check-golden-path.mjs            4 documents, 51 commands
node tools/edition-report.mjs --check       matches
```

## What round thirty leaves

The lesson is one this log keeps relearning from a different direction: **a suite that cannot render
cannot see a rendering defect.** Four rounds of unit tests, a mutation-checked parity tool and a
green clippy did not catch a condition reaching a console with no sentence attached. One run of the
thing an operator actually looks at did, immediately.

The verification environment is no longer a limitation to note in a log — it is a `wsl -e` away, and
every round from here should use it.

Nothing is pushed.

---

# Round thirty-one — §47.6's mapping half, and the number that did not have to cross

The last in-repo OS item. `skynet-frames` existed at the end of round thirty as a crate with twelve
passing tests and **no caller**, which is precisely §79.7's shape — a module nothing imports. This
round gives it the route.

## What landed

**`crates/skynet-frames`** — the Linux mapping half of §47.6. Two shared regions per pool: pixels
mapped read-only into the reader, and a loans region of one `u64` per slot mapped writable. It
carries no copy of the selection rule; that rule is `skynet-contracts`' `FramePool`, reproduced by
three editions and pinned by `corpus/media/pool.json`, and §47.6a is the record of what a second
implementation of it costs even when it looks equivalent. `#![forbid(unsafe_code)]` — the one
`mmap` is `skynet-ring-shm`'s.

**`Reader::adopt_declared`** — the pool describes its own shape, so its geometry never crosses out
of band. The alternative was four lines: `SKYNET_RING_CHANNELS` already crosses beside the rings and
a `SKYNET_FRAME_POOL=<slots>x<bytes>` beside it would have worked. It would also have been a second
copy of a number the region already states, and the failure when the two disagree is *not an error*
— it is a reader addressing the wrong offset for every slot past the first and finding plausible
pixels there. So a receiver maps 64 bytes of a duplicate descriptor, checks the magic and version,
reads the geometry out of the header, and only then maps the region. It consumes both descriptors
on every path including the refusals, and a test asserts that by calling `dup` on them afterwards
and requiring it to fail.

**`pass::MAX_FDS` two → four**, and `pass::close`. The pool crosses in the *same* `SCM_RIGHTS`
message as the two rings — pixels third, loans fourth — for the reason the observation ring crosses
with the commands: a policy handed some of them and not others is a program running on a world it
half has, and both halves of that failure are silent.

**The count is the announcement.** Two descriptors is a runtime with no camera; four is one with a
pool. Not two-plus-optional, and never three — a policy handed pixels and no loans region can read a
frame and cannot stop the writer taking the slot out from under it, which is the one property the
pool exists to provide. `adopt_from` refuses any other count by name.

**`skynetk --frame-pool <stream>:<slots>x<bytes>`** creates the region and hands it over, and
refuses without `--policy-socket` — a pool nobody receives is a mapping a process talks to itself
through. It does **not** publish into it: this build has no capture backend, so the pool stays empty
for the whole run and the summary *says so* rather than implying it (§34.3). The region and the
hand-off are the operating system's; the pixels are a vendor driver's.

**Two integration tests start `skynetp` as a real child** and read its stdout: one where a pool
crosses and the policy names the slot the frame is in, one where none does and the policy says
nothing about frames. The second exists because "four means a pool" asserted and "two means no pool"
assumed is exactly the pair where the assumption fails quietly — a policy that printed a frame line
on every robot would be claiming a camera that is not there.

## Documented

§47.6c is new: the mapping half, the loan's narrow property, the geometry that stays in the header,
and the count as the announcement. §73's media row and scale paragraph updated (24 crates, 126,000
lines), §80's *what a policy can see* row now says the mapping is built and names what is still open
— **where the pixels come from** and **what infers on them**, both a vendor's rather than this
system's. Both mirrors edited and re-pinned.

## Run

```
cargo test --workspace                      1,718 passed, 0 failed   (1,702 last round)
cargo clippy --workspace --all-targets      clean, no warnings
cargo fmt --all --check                     clean
python3 editions/python/run_corpus.py       1,142 of 1,142, both editions
python3 editions/python/self_check.py       44 checks
node tools/scale-report.mjs --check         §73 matches the tree
node tools/check-mirrors.mjs                all shared documents match
sh crates/skynetd-io/e2e.sh                 3 check(s) failed — NOT YET DIAGNOSED
```

## What round thirty-one leaves

**`e2e.sh` reports three failing checks and I have not diagnosed them.** That is the honest state.
The first run of it this round reported *eleven*, and every one of those was my own harness error:
I ran the script from `crates/skynetd-io`, and its mock-driver compile step uses a path relative to
the repository root, so `cc` could not find `drivers/mock/mock_robot.c` and ten checks cascaded off
one missing `.so`. Re-run from the root, eleven became three. Whether those three are a regression
from this round or were already there is exactly what has not been established — do not assume
either. **This is the first thing the next round picks up.**

Worth keeping beside it: the script fails informatively for a missing compiler and does not fail
informatively for the wrong working directory. That is a gap in a check whose whole job is to be the
thing an operator actually runs.

The OS is otherwise closed apart from the third language edition, which the owner excluded from this
pass. Next area is the Console (§83.5).

Nothing is pushed.

# Round thirty-two — the three failing checks, and the second supervisor

Round thirty-one left one thing: *"`e2e.sh` reports three failing checks and I have not diagnosed
them. Do not assume either."* Good instruction. It was not three, it was not a regression, and it
was not in the daemon.

## What the number actually was

The first run this round reported **thirteen**, and they were one cluster: every §59 confinement
read-back in section 3, plus §15's `safety.policy.isolation` and the two assertions about what its
pass says. Round thirty-one's three were the same cluster in a luckier moment.

Each failure dumped this:

```
AllowedCPUs=
MemoryMax=infinity
TasksMax=37039
CapabilityBoundingSet=cap_chown cap_dac_override … cap_sys_admin … cap_checkpoint_restore
ProtectKernelTunables=no
ProtectSystem=no
NoNewPrivileges=no
```

Every directive present, every one of them a refusal. It reads as a sandbox that landed nothing.
It is **`systemctl show`'s output for a unit it has never heard of**: the manager's own defaults,
printed for whatever properties you name, with **exit status 0**.

Reproduced by hand in the same environment, against the same store: `NoNewPrivileges=yes`,
`ProtectSystem=yes`, `MemoryMax=67108864`, `TasksMax=128`, `CapabilityBoundingSet=` empty,
`AllowedCPUs=0-15`. The confinement lands. There was no unit to read it off.

## Why there was no unit

The package started and was stopped again about half a second later, over and over — start, stop,
start, on a level-triggered loop. Nothing in `skynetd` was doing it: `GET /plan` never contained a
`StopPackage`, no skill existed to emit `Effect::Stop`, and nobody called `POST /stop`.

Two units, identical but for their names, settled it. `probe-plain.service` stayed up indefinitely;
`skynet:openrobot:probe.service` was stopped 1.7 s after starting, every time. At `log-level debug`
the manager showed the shape of it — a `GetAll` on the unit, then
`Trying to enqueue job skynet:openrobot:probe.service/stop/replace`. That is not systemd's
housekeeping. That is `observe()` followed by `stop()`.

```
$ ps -eo pid,etime,args | grep skynetd
3030  03:30:57  /tmp/sk-target/debug/skynetd --root /tmp/skynet-e2e/kernel --user --interval 500
```

**A `skynetd` abandoned by an aborted run three and a half hours earlier.** §33's unit names —
`skynet:<ns>:<name>.service` — live in **one** user manager, so a second supervisor observes the
first one's units, finds them absent from its own desired state, and stops them. Two daemons on one
machine fight over every package on it. A robot has one `skynetd`, so this is not a defect in the
daemon; it is a defect in a script that starts five background processes and cleans up one.

The trap read `trap 'kill "$DAEMON" …' EXIT`. `$MDNS`, `$KDAEMON`, `$KLOOP` and `$KSTOP` were killed
only by the lines that follow them, which an early exit never reaches. Every aborted run since that
trap was written has left a supervisor on the machine, and it outlives the checkout that made it.

## Fixed in three places, and only one of them is the script

**1. `confinement()` was reading an answer that was not about a unit.** A new pure
`confinement_from_show` asks for `LoadState` alongside the directives — in the *same* call, because
two calls could straddle the moment a transient unit is collected — and returns `Err` unless it is
`loaded`. `not-found` gets its own sentence, because it is the ordinary one: `--collect` reaps a
transient unit when it goes inactive, so a policy that stopped between being listed and being read
is a thing that happens, and it says nothing either way about the confinement it ran under.

That matters beyond this script. `policy_isolation` already treats an unreadable confinement as a
**skip** with the reason, and a shortfall as a **fail**. So before this, a robot whose policy had
stopped would fail its own `safety.policy.isolation` and name seven directives that were in fact
applied — the failure `confinement_property_names` documents one cause of, arriving by another
route. `applied()` had the honest version of this all along (`MainPID` of 0 → *"It stopped between
being listed and being read"*); the rung above it did not.

Three tests, including one that keeps the old misreading demonstrable: the real not-found dump,
parsed the old way, still produces a shortfall of exactly seven, so nobody can quietly undo this.

**2. The script kills what it starts.** `track` records every background pid and `kill_tracked` is
the trap, with a `pkill -f "skynetd --root $ROOT"` after it for a process whose pid this shell never
learned — `$!` is only ever the last one.

**3. The script refuses to run beside another supervisor**, naming the pid and the root rather than
saying something is wrong. A run against a poisoned machine that reports ten confinement failures is
worse than one that does not start.

## What the read-back does when there is no unit

One failure, not ten, and it says what happened: *"the unit was started and is not there to read"*.
Ten "directive missing" lines would be ten false sentences. It also waits for the unit the way
`start_daemon` waits for an answer rather than for a socket file — the comment claiming *"it runs
for the whole script, so there is no race"* was a belief about a transient unit, and `--collect`
does not share it.

## Run

```
sh crates/skynetd-io/e2e.sh          98 checks, 0 failed        (was 13 failed / 85 passed)
cargo test --workspace               1,721 passed, 0 failed     (1,718 last round)
cargo clippy --workspace --all-targets   clean
cargo fmt --all --check              clean
python3 editions/python/run_corpus.py    1,142 of 1,142, both editions
python3 editions/python/self_check.py    44 checks
node tools/scale-report.mjs --check      §73 matches the tree
node tools/check-mirrors.mjs             all shared documents match
node tools/check-rhal-flags.mjs          green
```

The three mechanisms were verified rather than assumed: a clean run leaves no `skynetd`; a run
started beside a stray refuses with exit 1 naming its pid; and a run interrupted mid-way leaves
zero daemons where it used to leave one for ever.

No contract, edition or corpus change. `reference/reports/scale.json` regenerated for the 152 added
lines of Rust; §73's rounded figure is unchanged, so no document moved.

## What round thirty-two leaves

The e2e is green, and the reason it was not is a class of bug worth naming rather than a one-off:
**a check that reads a tool's output without reading whether the tool was describing anything.**
It is the same shape as round thirty-one's wrong-working-directory note, and both cost a session.

Next is the Console (§83.5), per the owner.

Nothing is pushed.

---

# Round thirty-three — the audit before the round, and the audit's own first draft was wrong

Asked to continue "as the plan says", and asked first to establish whether the plan is finished.
No code this round. Every item below was checked against the tree and the machine rather than
against the prose that claims it — which is the discipline round thirty-two's failure was an
argument for, and which **the first draft of this entry failed at in its own first item.**

Three surfaces, three logs: this file is the OS, `skynet-build-plan.md` is the Hub and the
Console. The audit is in one piece because the answer to "is it finished" is different on each,
and the differences are the useful part.

## The correction, first, because it is the finding

The first draft listed as open: *"A policy package that speaks §25 — `skynetp` appears nowhere in
`skynetd-io`, there is no manifest in the tree, and §54.4's lifecycle has never started a real
commanding loop end to end."*

**All three clauses are false, and they were false when they were written.** `e2e.sh` §9 has
installed a real skill package since round nine: `acme/wave`, the `skynetp`-in-a-blob pattern
(`exec "$BIN/skynetp" run --hz 10 --cycles 600 --backend hold`), staged through `skynet install`,
activated through desired state, invoked over the socket, watched into `running` and out through
`terminal`. It is among the 98 checks that pass. Building it is also what found §61's bug — the
reconciler stopping every skill on its second pass.

The draft was assembled from §73's Skills row, which says *"nothing joins them, so an invocation
still starts nothing"*, and from round seven's bullet, which said the same. Both are stale, and
**reading a stale row and repeating it is not an audit** — it is the thing an audit is for. The
grep that settled it took one command. Recorded rather than quietly deleted, because this
repository has now produced the same failure from three directions in three rounds: round
thirty-one read a tool's output without reading whether the tool was describing anything, round
thirty-two the same, and this round read a document without reading the tree it describes.

## The state it starts from

HEAD is `4c55900`, working tree clean. `e2e.sh` 98 checks, 0 failed. `cargo test --workspace`
1,721 passed. `node tools/scale-report.mjs --check` — §73 matches the tree at all eight figures.
`node scripts/check-skynet-service.mjs` (BotMarket) — one discovery contract, 6 checks, no
divergence across the Rust responder and both browsers.

`node tools/check-mirrors.mjs` — **one document had diverged**, which is a finding rather than a
status line.

## A — buildable here: no hardware, no vendor, no owner decision

**A1. Three documents behind the tree — all three closed this round.** The failure §73's own
header is written against, and after the correction above it was the largest item on the list
rather than the smallest.

* `docs/skynet-build-plan.md` had **diverged** from BotMarket's copy: Hub sessions twenty-six
  through twenty-nine — the Console pairing pane, the Hub download surface, black box download,
  the sitemap entry, the remainder audit — existed only in the other repository. 180 lines,
  purely additive, nothing removed, so `--sync` was the right direction and was checked to be
  before it was run.
* §73's **Skills row** claimed a missing wire that round eight witnessed and round nine
  demonstrated end to end. Rewritten: the middle column now carries the package and the lifecycle
  it is driven through, and the last column keeps the stale sentence visible with the reason a
  stale *"still missing"* is the one kind of wrongness a "what is missing" column cannot catch by
  being read — it costs a reader nothing to believe, and the next session plans around a hole
  that is not there. What replaces it is the two things that **are** missing: no physical joint has
  ever produced the post-state §54.3 judges, and `intervention_count` is attributed only while
  one invocation is in `running`.
* §80's **ring row** ended *"What is still one process is a sandbox"*. False since §59, and
  round thirty-two's e2e reads the directives back off a running manager to prove it. §59's own
  section in this log recorded why it had not been fixed there — *"fixing it means editing
  `docs/skynet-os-architecture.md`, which mirrors BotMarket byte for byte … BotMarket is not
  present on this machine, so the annotation waits for a pass that has both trees."* **This is
  that pass.** The row now carries the audit that preceded the build (§40's half already
  shipped, §8.3's half never started), the four omissions with their unblockers, and the fact
  that the confinement is read back rather than asserted.

Both documents were edited in **both** repositories, which is the rule, and `docs/mirror-digests.json`
re-pinned from the two agreeing copies with `--pin` rather than `--sync` — the distinction the
tool spends a paragraph on, and this is the round it mattered: `--sync` here would have silently
reverted the edit that had just been made in both trees.

    node tools/check-mirrors.mjs             all shared documents match
    node tools/check-contract-mirrors.mjs    every contract the Hub mirrors matches

**A2. §79's two halves, joined to each other.** The robot half (`skynetd-io::upload` — the probe,
the `PUT`, the receipt, with `skynet-contracts::upload` deciding whether to send) and the Hub half
(`GET|PUT /devices/{id}/recordings/{sha256}`) are both built, and each is exercised against a
double. `e2e.sh` contains no upload at all. Round twenty-nine put this on its short list and it is
still there: joining them needs a running Hub beside a running robot, which is two processes on
this machine rather than a piece of hardware.

## B — blocked on hardware or a vendor, and correctly so

* **Where the pixels come from.** No capture backend fills §47.6's pool; `skynetk --frame-pool`
  offers a region that stays empty for the whole run and says so (§34.3). A vendor driver's gap.
* **What infers on them.** §80 says in its own words that starting this as a side effect of
  something else produces a bad version of it.
* **A physical joint, a TPM (`security.identity.attested`), a vendor RHAL driver, an OTA image
  this Hub has applied, the certified tier's MCU and its notified body.**
* **Three of §15's eight runtime tests**, which name what they wait for. Five execute.
* **Device confinement** — `vram_mb` stays `None` because accelerator memory needs a vendor
  runtime this daemon does not link, so there is no admission number and no `DeviceAllow`.

## C — the owner's call, and neither is an agent's to take

* **A third language edition.** Excluded by the owner from the last pass. Two editions can agree
  because the second was transcribed from the first, which this one was; the corpus only earns its
  keep when an edition is written by somebody who did not.
* **A signature on a conformance result.** §19 forbids a robot holding a signing key, so §12's
  kernel gate reads evidence nobody signed. A Hub countersignature is a **different claim** and
  §80 says so; an operator attestation from Skynet Console (§83.4) is a **third** again.

## D — designed-open on purpose

§30, §43, §53 and §58.3, unchanged and not stale: retargeting fidelity, role-vocabulary
governance, kernel formal verification, field-conformance privacy, multi-robot coordination, two
robots on one compute, safe-state discovery for mobile robots, driver A/B, console over the
internet, fleet config convergence, fleet-scale calibration drift, `on_command_loss` per chain,
entitlement for the OS itself. Perception and motion planning are out of scope by decision rather
than by absence (§58.3).

## The Hub and the Console, in one paragraph each

**Hub.** Finished apart from four things, none of them effort: the **restore surface**
(`restoreRepo` is complete in lib with its own audit row and no route calls it — route placement
is a product call and it is a second cross-namespace staff power); the **countersignature** (C,
above); **robot-dependent** items; and `20260905200100_marketplace_listing_transactions.sql`,
which is marketplace-side and left with the owner's marketplace work. Thirteen of thirteen
endpoints, 26 migrations applied, 0 partial, every verify script green as of session twenty-five.

**Console.** The Windows application builds, runs, discovers, and opens four panes — the robot's
own console in `WebView2`, the discovery record, pairing, and the black box index with seal and
download — over 78 tests. Everything left is blocked, and on six different things: **replay**
waits on §83.3a's FFI decision; **USB/serial** has no route on the robot to talk to; the **tray
stop** has no tray package in the offline NuGet cache, only runtime-verifiable failures on a
machine where the application cannot run, and a product question about which robot a global hotkey
stops; the **CLI subprocess** has no robot to point at; **signing and the installer** need
certificates; **macOS** is written, never compiled, and now four panes behind Windows.

## What this round leaves

The plan is not finished, and after the correction what an agent can build here was **two items**.
A1 is closed — the mirror is synced, both stale rows are rewritten in both trees, and the pins
agree. **A2 is the whole of what is left that does not wait on a person, a vendor or a machine.**

Worth carrying forward, because it is the round's actual lesson rather than its output: the audit
was asked for because the plan *might be finished*, and the first draft answered from the plan.
Three of the four things it called open were closed in the tree, and the one that was genuinely
open — A2 — was the one the documents said least about. **A summary is evidence about when
somebody last wrote it down, and nothing else.**

Nothing is pushed.

---

# Round thirty-four — §79's two halves meet, and both of them were wrong about a trailing slash

Round thirty-three left one buildable item: *"the robot half and the Hub half are each exercised
against a double, and neither has met the other."* This is that, and it found two defects — neither
of which either suite could have seen, and both of which are the same sentence about a receiver
that canonicalises a path.

## The environment, because it decided the shape

`skynetd` serves on a Unix socket and launches through systemd, so the daemon proper runs in WSL.
The Hub runs on Windows. **WSL cannot reach the Windows host on this machine** — TCP and ICMP to
the host address are both blocked, and opening them is a firewall change that is not an agent's to
make. So the crossing runs entirely on Windows, through `skynetd once`.

That is a supported mode rather than a workaround, and the code says so where it matters: §33 calls
single passes *"a legitimate way to operate a robot"*, and `report_upload` sits in the `once` path
deliberately — *"a robot operated entirely through single passes should not be one whose sealed
recordings are never collected."* One slice per pass, which is what makes the loop below a resume
path rather than a single transfer. What it is **not** is the supervised loop, and the log and the
specification both say which.

## What crosses

`scripts/skynet-verify-recordings-robot.mjs`, in BotMarket beside the receiver's own verifier,
because that is where the service-role client and the cleanup convention already live. It reports
**unchecked** rather than passing when it cannot run — no OS checkout, no binaries, no mock driver,
no curl, no service key each get their own sentence, since *"cannot run"* and *"cannot run because
the OS is not checked out beside this repository"* send a reader to two different places.

1. **`skynetk` turns the loop** against the mock RHAL driver at 200 Hz for 400 cycles, recording
   into §13's ring and sealing on exit. A real MCAP window of real cycles, not a file of random
   bytes with a plausible name — asserted as MCAP, because a sealer that wrote nothing readable
   would still produce a file this script could upload and check a digest of.
2. **One unpaired `skynetd once`**, which reshapes: the daemon opens its ring at the shape §26.3
   derives from what this robot is, cannot reuse the loop's, and seals it rather than discarding
   it. So there are **two** windows, which is better than one — it exercises §79's rule 2, *one
   subject per pass, oldest first*, and it is what a robot's first supervised boot after a bench
   session actually does.
3. **The Hub end**: a namespace, a device and a credential. Pairing is not exercised and the script
   says why — §10's pairing runs over the daemon's Unix socket, which does not exist here, so the
   identity is placed as the two files pairing leaves behind. The subject is the upload.
4. **`skynetd once` in a loop** until §79 goes `Idle`, which is precisely *every sealed window here
   has a receipt*.

50 checks. Both windows complete, a receipt beside each on the robot naming the digest the recorder
sealed, a recording row and an artifact each on the Hub, the namespace charged for both, and the
object in the bucket **byte-identical** to the file on the robot's disk — with the two digests
computed independently by two implementations that share no code. Then a further pass, which must
say nothing, because §79.7's whole lesson is that half a route presents from the sending side as a
window that never stops being pending.

## And then the sentence §79.7 has been making since it was written

With the transport proven, the fault path costs one more section and closes the claim §73 has
carried unproven: *`skynet_incidents.artifact_id`, NULL on every row the platform could hold since
the fleet schema was written, is filled by the digest the incident named.* True on both sides,
against doubles on both sides — this repository's incident went to a sink it wrote, and the
Hub's join was exercised by rows a script inserted.

A real fault was needed and §38.1 supplies one that needs no hardware: **a profile whose
signature was made over other bytes.** A genuine Ed25519 key, genuinely in the trust store, and a
genuine signature — over a different message. That is precisely the state §38.1's attacker
leaves behind, and it is deliberately the only signature state that raises: `absent` is a profile
nobody signed and `untrusted` is a key nobody added, both administrative gaps, and raising for
either would train an operator to close incidents without reading them.

`raise_incident` did the rest with nothing faked: sealed the window, cited it so §80's reclaim
can never take it, queued the document with the recording's identity inside it (§77.2). The
drain sent the alarm, the next passes sent the window, and the Hub's `attachToIncidents` matched
`body->recording->>digest` and filled the column. Asserted not merely as *filled* but as filled **by
the right window** — the artifact's own `sha256` against the digest inside the incident —
because a join on a digest that matched any window this device had sent would look identical and be
wrong.

One thing noticed and deliberately not changed: **a robot refusing a profile prints nothing.** The
alarm is the incident; stderr says nothing about ceilings that were not applied. `raise_profile_incident`
is loud only when the *queue* fails, which is defensible — the operator surface is
`GET /outbound` and the console — and it is a judgement rather than a defect, so it is recorded
here rather than edited. What the script asserts locally instead is the thing that is true and
observable: §13 sealed a window whose **file name carries the reason**.

## What crossed beside the window

It also asserts what crossed **beside** the window, because that part was free. Every pass builds
§17's check-in and drains §34.4's queue, so the same run put a document through
`POST /devices/{id}/checkin` — through `post_json`, the sibling that already carries
`--location` because this same canonicalisation caught it once before. That route had only ever met
a double too. What is asserted is the half neither double can vouch for: the document the real
producer built arrived, was stored whole, and §27's state on the Hub's lifted column is the one
inside the stored document rather than a value the Hub decided.

## An absence, asserted, because it is correct rather than missing

§74's telemetry does **not** cross in this harness, and the script says so in a check rather than by
being silent about it. An unasserted absence is one a later reader takes for a gap.

`Counting` lives in memory on the `Daemon`, so every `skynetd once` opens a window and closes none —
which `report_telemetry`'s own comment already says about a *single* pass. What is worth writing
down is that repeated single passes do not accumulate one either, because each is a new process, and
that this is **right**: §74.2's window carries both clocks, and a monotonic interval does not survive
a process boundary. A window whose bounds came from two different processes would be a fabricated
duration — precisely the thing §74.3 spends a whole column keeping out of the table.

So this is **not** the same case as §79's upload, and the difference is worth stating because the
surface argument looks identical. Bytes on a disk are a fact that outlives a process; an interval
measured by a monotonic clock is not. `report_upload` belongs in the `once` path for a reason that
does not transfer to `report_telemetry`, and nothing here should be "fixed" to make it.

If that check ever fails, telemetry has learned to span passes, and this paragraph is the first
thing that needs rewriting.

## The first defect: curl appends a filename to a URL that ends in a slash

`--upload-file` has a documented behaviour that is a footgun here and nowhere else in this daemon:
*if the URL ends with a slash, curl appends the local file's name.* The local file is the scratch
slice, so the push went to `…/recordings/sha256%3A<hex>/slice`.

It would be an obscure curiosity if a slash-terminated URL were unusual. It is the **normal** answer
from the Hub this robot pairs with: `trailingSlash` canonicalisation, `308` to the spelling §17
prints. And `advance` reuses `probed.effective_url` — which is right, and is what keeps a redirect
from costing a re-sent window — so the push was handed exactly the shape curl will not accept.

What it produced is §79.7's signature: the probe answered, the push 404ed, the window stayed
pending, and every surface said the robot was trying.

`upload_target` is the fix, in `curl_push` rather than in `advance`, because this is a property of
curl and not of §79 — an uploader written against a different client should not inherit a
workaround for a client it does not use. It trims the slash and lets `--location` follow the
redirect back to it, which costs one round trip per slice and does not cost a re-sent body: curl
sends `Expect: 100-continue` for a `PUT` of this size and the 308 arrives before the body does.

One test pins the pure half, and pins the assumption underneath it too — that the address this
daemon constructs never ends in a slash to begin with, so the trim is never doing work nobody
expects.

## The second defect, found while looking at the first, and larger

Every pass printed this and I nearly read past it:

```
skynetd: the desired state could not be pulled: the Hub answered HTTP 308 to
http://localhost:3000/api/skynet/v1/devices/<id>/desired
```

`connectivity::get_json` had **no `--location`**. Its sibling `post_json` has carried one, and a
paragraph explaining it, since a canonicalising Hub was found to be swallowing *"every check-in,
every incident, every telemetry window and every episode this edition has ever produced"*. The
`GET` was never given the same flag.

Its two callers are §11's desired state and §12's release list. So a robot paired to this Hub was
one that **could not be told what to run and could not be told there was an update** — the whole of
reconciliation, reported honestly and uselessly as *the Hub answered HTTP 308*.

The thing that hid it is worth more than the fix: `get_json`'s own doc comment said *"there is no
body, so there is no file. **That is the whole of the difference between the two.**"* Two functions
side by side, described as differing in one way, differing in two. The sentence now says *the whole
of the intended difference*, and names the one that was not.

## Both fixes proven load-bearing, by reverting them

Not by arithmetic. `upload_target` reverted → 7 checks fail with the exact 404 on the exact invented
path. `--location` reverted → the desired-state check fails and nothing else does. Both restored,
both re-run green.

## Run

```
cargo test --workspace                      1,722 passed, 0 failed   (1,721 last round)
cargo clippy --workspace --all-targets      clean
cargo fmt --all --check                     clean
sh crates/skynetd-io/e2e.sh                 98 checks, 0 failed
python3 editions/python/run_corpus.py       1,142 of 1,142, both editions
python3 editions/python/self_check.py       44 checks
node tools/scale-report.mjs --check         §73 matches the tree
node tools/check-mirrors.mjs                all shared documents match
node tools/check-contract-mirrors.mjs       every contract the Hub mirrors matches
node tools/check-rhal-flags.mjs             green
node scripts/skynet-verify-recordings-robot.mjs   50 checks, 0 failed   (new, BotMarket)
```

`reference/reports/scale.json` regenerated; §73's rounded figures are unchanged, so no document
moved for it.

## Documented

§73's black box row and §79.6 both said the two halves had never met, and both are rewritten in
both trees with the pins taken from the copies that agree. §79.6's amendment is the one worth
reading: it is the second retraction that section has carried, and the first one — *"the Hub half
is not built here"* — is why the sentence was phrased so it could be retracted rather than deleted.

§73's row also gains the incident half, because that is the sharper of the two claims: the row
has said `skynet_incidents.artifact_id` *is filled by the digest the incident named* since §79.7
was written, and until this round both sides said so about a double. It now says which fault
produced it and that the column was filled by the right window rather than by a window.

## What round thirty-four leaves

The gap that closed was between two halves. The gap that remains inside it is narrower and named:
the daemon ran **single passes**, not the supervised loop, and **§10's pairing did not run**. Both
wait on the same thing — a Linux host that can reach the Hub — and neither is a missing mechanism.

Everything else on round thirty-three's list is where it was: a vendor's, a machine's, or the
owner's. What this round adds to that list is a rule rather than an item:

> **Two components that have only ever met doubles have not met.** Both suites here were green,
> both had been green for weeks, and between them sat a robot that could not upload a window and
> could not read its own desired state. The doubles were not wrong about the protocol; they were
> silent about the one thing a real receiver had an opinion on.

Nothing is pushed.

---

# Round thirty-five — a black box nobody could read, and a command §41.3 has specified since Part IV

Asked to finish the OS and to make the Console usable, with the features an operator would actually
want. The audit two rounds ago said what was left was a vendor's, a machine's or the owner's. That
was true about the *list* and it was answering the wrong question, which this round found by asking
a different one: **not what is unbuilt, but what would somebody standing in front of a stopped robot
reach for.** Three things came out of that, and two of them were specified and missing rather than
open.

## `skynet doctor`, which §41.3 has described since Part IV

> *"One command producing one bundle … It is the first thing to ask for in a support conversation,
> and it exists so that the answer to 'what is wrong with this robot' is a file rather than an
> interview."*

Specified, quoted in the golden path, and **not a command**. `skynet doctor` printed *"`doctor` is
not a skynet command"*, which made that paragraph false in the way that matters: the answer *was* an
interview, conducted one `skynet` invocation at a time by somebody who already knew which ones to
run.

It is an **aggregator** and holds no second opinion about anything. Every section is a route on
§33.7, so a `doctor` that derived its own view of §7.2's guard partition would be a second
implementation of it — §78's shape, which this repository has paid for once. Twelve sections: the
nine §41.3 lists, plus §27's state, §33.6's health and the window's shape, because the two questions
a support conversation opens with are *what state is this robot in* and *is its supervisor healthy*,
and a bundle that sent somebody to a second tool for those would not be the file §41.3 describes.

**The rule it is built on is that a route it could not read is recorded as unread, with the
reason.** A robot answering 403 for want of a credential and 503 because its recorder is not running
is telling you something, and a bundle that omitted those sections — or wrote them as empty objects
— would erase the two most interesting facts in it. So a run with no credential produces a bundle:
one section read, eleven refusals, each naming the role it wanted. That is a readable document
rather than a failed run, and it is asserted in `e2e.sh` rather than described.

**Two things it got wrong, both found by running it against a robot rather than by reading the
routes.** The summary counted a `guards` array with a `state` field, which `GET /limits` does not
have, and printed **"0 of 0 active"** for every robot — a sentence that reads as a measurement and
is an artefact of looking for a key that is not there. §7.6's document names three lists separately
(`active_guards`, `inert_guards`, `unavailable_guards`) and §80 spends a row on why the last two are
not the same absence, so the summary now names them separately too. The queue depth was the same
mistake one route over. Both are the shape round thirty-four's defects had: correct-looking output
derived from a key nobody checked existed.

It runs over `--host`, which is what makes it collectable: the person who needs the bundle is
usually not sitting at the machine, and §34.2 binds the console link-local for exactly the moment
the Hub is unreachable.

## `GET /blackbox/events`, without which §41.3's list had an unanswerable item

§41.3 asks for *"the last N black box events"* and nothing could produce them. `GET /blackbox`
reports a window's **shape** — how many records, over what interval, with what continuity — and a
shape does not answer *what happened just before it stopped*.

**Gaps are in the list, in their place, and that is the whole design of it.** A [`Body::Gap`] is not
a [`Body::Event`] in the type system and omitting it would be the obvious reading of "events". It
would also present a continuous list over a discontinuous recording: two events either side of four
hundred dropped records read as consecutive, which is precisely the misreading §26 annotates a gap
to prevent. `held` is reported beside `returned` for a related reason — *"the ring holds 12,000 and
you are looking at 50"* and *"the ring holds 50"* are different robots.

Observer, with the index, rather than operator with the download. §39.1's line is between *what the
recording is* and *what the robot was told to do*: the file carries every commanded position, and
this carries lease grants, stops, seals and gaps. The route sits **above** the download arm in both
the router and the authorisation table, and if it ever stops doing so `events` is read as a file
name and silently needs the wider role — so the ordering is asserted in both places.

It is also a panel on the robot's own console, which is §83.2 working as designed: the page gained
it, and Skynet Console gained it by embedding the page, with no second rendering and no line of
markup of its own.

## Reading a sealed recording, which nothing had ever done

The strangest gap of the three. §13's seal has written MCAP **since the sixth session** and no code
anywhere had ever read one back. That is not a missing convenience: §13's whole argument is that a
recording turns *"the robot did something strange on Tuesday"* into evidence, and evidence nobody
can open is an anecdote with a checksum. Every question about a window was a question for the robot
that produced it — which works until the robot is the thing that is broken, or until the window is a
file somebody emailed.

`skynet-blackbox::read` is the reader and `skynet blackbox inspect <file>` is the command, and it is
the one robot command that talks to no robot. It stops short of replay deliberately: reassembling
cycles out of the four messages each is written as, and evaluating them against resolved limits, is
the harder and more valuable thing, and building it as a side effect of *"open the file"* would
produce a bad version of both. What it prints is the file's own account of itself — provenance,
whether it is complete, and the events — and it says in as many words that it has not replayed
anything.

Three properties worth keeping:

* **Nothing trusts the file.** Every length is checked against what is left before it is used. A
  recording is exactly the kind of file that arrives from somewhere else, and every failure is a
  named error rather than a panic.
* **Truncated and damaged are different answers.** A file that begins correctly and does not end
  correctly is a transfer that stopped, which §79 resumes; a record whose declared length runs past
  the end is a file to replace. Calling both "bad file" would hide the one with a remedy.
* **Unknown opcodes are skipped by length rather than refused**, and counted. That is what lets a
  reader from one edition open a file from a later one, and *"I understood four of nine thousand
  records"* is a different sentence from *"the recording is nearly empty"*.

Two defects, both found by pointing it at a real 200-cycle recording rather than at a fixture: every
topic was listed **twice**, because MCAP repeats the channels in the summary section and the reader
pushed both; and an empty metadata value printed as a blank where the seal writes every key whether
or not the robot had something to put in it, so `robot_id` on an unpaired machine rendered as a
value nobody noticed was missing. Both are pinned.

## Documented

§41.3 says it is built and what the bundle actually contains. §73's black box row carries the reader,
the events route and the bundle; its interface row carries the panel and the reason it needed no
Console work. §83.5 is amended — four of its five first-version items exist, and the fifth turned out
to be two questions of which only the second needs §83.3a's decision. §73's figures moved: 127,000
lines and 54 CLI commands, both re-checked mechanically.

## Run

```
cargo test --workspace                      1,728 passed, 0 failed   (1,722 last round)
cargo clippy --workspace --all-targets      clean
cargo fmt --all --check                     clean
sh crates/skynetd-io/e2e.sh                 119 checks, 0 failed     (98 last round)
python3 editions/python/run_corpus.py       1,142 of 1,142, both editions
python3 editions/python/self_check.py       44 checks
node tools/scale-report.mjs --check         §73 matches the tree
node tools/check-mirrors.mjs                all shared documents match
node tools/check-golden-path.mjs            every command is one of the 54
node tools/check-rhal-flags.mjs             green
```

## What round thirty-five leaves

The console fixture gained a recorder, which is a smaller note with a sharper edge: it had none, so
its black box panel could only ever render *unavailable*, and **nobody had ever looked at the
interesting half of that panel**. A fixture that can produce only one state is a fixture in which
the other states have never been seen. The same applies to the events panel it now feeds.

What is left of the OS is where it was — a capture backend and inference (a vendor's), a third
edition and the countersignature (the owner's), and hardware — with one addition that is now the
sharpest unbuilt thing rather than a deferred one: **replaying a window against the limits that were
in force.** It has a reader under it for the first time, so it is no longer blocked on anything but
the work.

Nothing is pushed.

---

# Round thirty-six — a window that could be read and not replayed, and two defects that only a real recording had

Asked to establish first whether the plan is finished and then to continue building. The audit is
below in one paragraph because its useful part is a correction; the round's substance is what the
correction opened.

## The audit, and the three things the logs were wrong about

Round thirty-three's own lesson — *"a summary is evidence about when somebody last wrote it down,
and nothing else"* — applied to round thirty-five's summary, and it caught three items.

* **`cargo test --workspace` did not build on Windows.** `skynet-ring-shm` states, in a comment over
  the block that exists for it, *"a stub rather than a compile error, so `cargo test --workspace`
  works on a machine that is not the robot."* `Mapping::words` has that stub; `bytes_slice`, added
  with §47.6c's mapping half in round thirty-one, does not, and `skynet-frames` calls it from ten
  ungated places. So the workspace failed to compile on the host this repository is developed on,
  and **CI's `windows` job — which exists because of a break of exactly this shape, and says so in
  fourteen lines of comment — was red.** The log said clippy and fmt were clean; both were run on
  Linux.
* **`cargo fmt --all --check` was not clean either.** Fourteen hunks across five files, all of them
  round thirty-five's own new code. Both toolchains here agree about it, so this was not a version
  difference.
* **Replay was not blocked, and most of it was already written.** Both logs called it *the sharpest
  unbuilt thing* and §83.5 called it the one item still waiting on §83.3a's FFI decision.
  `crates/skynet-blackbox/src/replay.rs` has evaluated recorded cycles against `ResolvedLimits`
  since round twenty-something, and its own doc comment says it takes records and name tables
  rather than a `Ring` **"so that a recording read from a sealed file — which has the names and not
  the ring — replays through the same function."** `read.rs` arrived last round. Nothing joined
  them, and no FFI decision stood between them.

## What the file could not say, which is why nothing had joined them

Reassembly is the inverse of §26's *one cycle becomes four messages*, and doing it found that the
inverse **did not exist**. Three of `replay()`'s inputs reached the ring and stopped there:

| Field | Where it lives | What its absence would have done |
|---|---|---|
| `capability` | `/sk/desired` | §7.2's mode gate treats *no capability* and *the runtime did not say* differently. Every replayed cycle would have claimed the second |
| `nearest_person_m` | `/sk/state` | The proximity guard's whole input. Every window replays as a robot with nobody near it |
| `read_result` | `/sk/state` | §26.4's fact about whether a cycle's body fields are measurements at all |

Two of those are inputs the Safety Kernel **evaluates**. A replay missing them does not fail — it
recomputes a different verdict and presents the difference as the kernel disagreeing with itself,
which is the worst available failure for a tool whose entire purpose is to say whether the kernel
still agrees with itself.

So they are in the schema, in all three editions, and `corpus/blackbox/schemas.json` and
`seal.json` are regenerated. An older recording is **not** thereby unreadable and **not** silently
wrong either: the missing keys become the values the ring already uses for *not known* —
`NO_CAPABILITY`, `NaN`, §26.4's `Unrecorded` — and `Assembled` counts the cycles that lacked each,
so a caller says *this window predates the field* instead of publishing a comparison that rests on
a guess. `Absent::verdict_inputs_all_recorded` deliberately excludes `read_result`, because that
one changes what a reader is told and no verdict is computed from it.

**The wire fixture caught the consequence before a person did.** The schemas are embedded in every
sealed file, so adding three fields moved every window's size and digest, and
`reference/wire/incident.json` — which the Hub keeps a copy of — stopped matching. Re-frozen and
resynced, and the Hub's 1,948 tests still pass.

## The join is by sequence, and the obvious version of it is wrong on the file that matters

MCAP numbers messages per channel, so the *n*th `/sk/state` and the *n*th `/sk/desired` are one
cycle. Reading that as *the same index in each list* is the obvious join and it fails on precisely
the file this tool exists for: a transfer cut between two of a cycle's four messages leaves the
channels one apart for the whole rest of the window, and every cycle after the cut is then
assembled out of two different ones — each field plausible, the whole thing wrong, nothing saying
so. Matched on the sequence itself, that cut is one reported `Incomplete`. The four are then
required to agree about `mono_ns`, because a cycle whose messages disagree about when they happened
is not a cycle.

That was written as the positional join first, and the test that catches it was already written and
failing, which is the only reason the sequence version exists.

## Then it was pointed at a real recording, and both halves of it were wrong

Round thirty-five's own finding, again: *"found by pointing it at a real 200-cycle recording rather
than at a fixture."* Two processes on Linux — `skynetk` against the mock driver, `skynetp`
publishing a trajectory into the ring it was handed over `SCM_RIGHTS` — sealed a 300-cycle window.
Replaying it reported **300 of 300 cycles diverged**. Every unit test in the repository was green.

**One: `replay()` evaluated cycles the robot had never asked the kernel about.** §25's loop returns
before `evaluate_cycle` when the policy has posted nothing — *"with nothing posted there is nothing
to evaluate and nothing to send"* — and records the cycle with a mask of zeros and an all-`NaN`
`desired`. There is no verdict in those to reproduce. Replaying them anyway trips the watchdog on a
sequence that never advanced, so the 167 cycles before the policy connected came back as 167
divergences.

The first fix for that was also wrong, and the real recording said so too. Reading *nothing posted*
as *`desired` is empty* collapsed the same window to **two** evaluated cycles, because a policy
publishing a **hold** masks every channel out and records an all-`NaN` `desired` as well — and the
kernel was asked about those and did answer. §24's sequence is what separates them: zero is *no
command has ever arrived*, a value a real command cannot carry. With that, `unposted` is 167 and
the loop's own summary line says `idle 167`. Two numbers produced by two processes that share no
code.

`previous` follows the same rule for the same reason: the loop clears it every cycle and refills it
from the decision's mask, so a held or idle cycle leaves the next one with **no predecessor**, and
a replay that carried the last command forward would give the rate-limit guard a comparison the
robot never made.

**Two: `GET /limits` cannot replay anything, and that is not a small gap.** `evaluate_cycle` reads
`watchdog_ms` off the declared **envelope**, not off the resolved set, and `limits_to_json` does not
serialise an envelope — a console renders ceilings. So the guard that decides nearly every cycle on
a bench had no window to measure staleness against, and two recorded `stop_cat2` verdicts came back
as `none`: **a replay saying a robot kept going where the recording says it stopped.** That is the
one direction this must never be wrong in.

`skynetk` now writes the limits it ran under **beside every window it seals**, as the same document
`GET /limits` returns plus an `envelope` key. That closes a hole larger than the field: nothing else
on the machine knows those limits — `skynetd` resolves its own from a descriptor and whatever plan
is in force, which on a bench is a different robot's answer to a different question — so a window
sealed here and carried away could be inspected and never replayed.

## What it refuses, and where the refusal lives

A limits document still cannot supply §7.1's separation thresholds or §8.2's capability grants.
The first draft refused in the reader, and **that refused every real document**: `mode_gate` is
active on an ordinary robot and does nothing at all on a cycle that exercised no capability.
Whether an absence matters is a fact about the *window*, not about the document, so
`limits_from_json` reports (`Unreconstructed`) and the replay decides — refusing when the window
engaged the guard, when the recording is too old to say (*cannot tell* is not *did not happen*),
and when this tool has no rule for that guard at all. The last is separate from the second because
conflating them produced a real defect: the watchdog arrived with no rule and was reported as
*"this recording is too old to say"* about a recording sealed minutes earlier.

`skynet blackbox replay` takes the sibling by default, or a `GET /limits` body, or a whole `skynet
doctor` bundle — and a bundle whose `limits` section is a refusal passes the robot's own sentence
back, which is what §41.3's design is for. It is the CLI's second documented exit-code exception,
after `driver check`: **0** reproduced, **2** ran and did not, **1** could not run. Skynet Console
shells out to it, and a pane that rendered a divergence as *the tool failed* would bury the finding
under a tooling complaint.

## Proven three ways, on real windows

| | |
|---|---|
| The right limits | 133 of 133 evaluated cycles reproduced; `unposted` 167 against the loop's own `idle 167` |
| The envelope removed | refused, naming `envelope.watchdog_ms` and the cycles that engaged it — and without the refusal it had produced 133 false divergences |
| The wrong tier | 115 of 115 diverged, naming `advisory` and the commands a stop had zeroed being forwarded at 2.7 rad/s, which is what *the tier could only advise* means |

All three are in `e2e.sh` §11, against a compiled driver, two processes and a real seal.

## Run

```
cargo test --workspace           Windows 1,652 · Linux 1,739   (1,641 / 1,728 last round)
cargo clippy --workspace --all-targets    clean on both
cargo fmt --all --check                   clean on both      (was not, at the start of this round)
sh crates/skynetd-io/e2e.sh               129 checks, 0 failed   (119 last round)
python editions/python/run_corpus.py      1,142 of 1,142, both editions
python editions/python/self_check.py      44 checks
node tools/scale-report.mjs --check       §73 matches the tree
node tools/check-mirrors.mjs              all shared documents match
node tools/check-contract-mirrors.mjs     every contract the Hub mirrors matches
node tools/check-golden-path.mjs          every command is one of the 55
desktop/tests (BotMarket)                 137 passed  (113 last round)
Hub vitest (BotMarket)                    1,948 passed
```

## Documented

§26's channel table carries the two new facts and the reassembly rule; §73's black box row carries
the reader, the reassembler, the command and both defects, and its figures move to 129,000 lines
and 55 commands; §83.5's second question is answered. The spec is mirrored into BotMarket and
re-pinned. The README's three stale rows are corrected.

## And on the other side of the repository split

Two Console items closed in the same pass, both recorded in `skynet-build-plan.md`: the **replay
pane**, which is this round's command run as a subprocess, and §83.2's **tray stop and global
hotkey** — deferred for three sessions on a reason that was false when it was written, since the
session that wrote *"a machine where this application cannot be run"* had run it. Exercised rather
than compile-checked: a second process is refused `Ctrl+Alt+.` with 1409 while the Console holds it,
which is the proof that matters, and the Console says so out loud when another process holds it
first. §83.5's first-version list is now entirely built.

The Hub's **restore route** was found built and never once called — no unit test, no probe — and
now has nine live checks against a real server and database as a fifth phase of
`skynet-verify-decisions.mjs`, because restore is the reverse of the act that script already
performs and the pair is the thing worth testing.

## What round thirty-six leaves

Unchanged and still not an agent's: a capture backend and inference (a vendor's), a third edition
and the countersignature (the owner's), hardware, and the Console's USB/serial, signing and macOS.

What it adds is a rule, and it is the third round in a row to earn one:

> **A component that has only ever met a fixture has not been tested.** The reassembler had a
> round-trip test that passed, against a fixture built by the same hand that built the reader. The
> first real recording it opened found two defects in the code under it, one of which said a robot
> kept moving through a stop. The fixture was not wrong; it was silent about the two things a real
> window had an opinion on — a policy that connects late, and a guard whose input is not where the
> ceilings are.

Nothing is committed. Nothing is pushed.

---

# Round thirty-seven — the audit, run on a machine that is not the one the last four rounds ran on

Asked to establish what is left before writing anything, because the plan *might be finished*. No
code this round. Every item below was checked against **this** tree and **this** machine rather
than against the prose that claims it, which is round thirty-three's rule and the one three rounds
running have earned.

The machine matters more than usual this time, and it is the round's largest finding.

## What was run, and what it said

| | |
|---|---|
| `cargo test --workspace` | **1,739 passed**, exit 0, on Linux |
| `sh crates/skynetd-io/e2e.sh` | **117 checks, 0 failed** — every one that does not need a systemd user manager, which this container has no PID 1 for. The script says so itself rather than passing quietly |
| `node tools/scale-report.mjs --check` | §73 matches the tree at all eight figures |
| `node tools/check-mirrors.mjs` | all three shared documents match |
| `node tools/check-contract-mirrors.mjs` | all eighteen contracts match |
| `node tools/check-golden-path.mjs` | every link resolves, every make target exists, every command is one of the 55 |
| `node tools/check-rhal-flags.mjs` | every flag accounted for across the four bindings |
| Hub: `npx tsc --noEmit` | **exit 0** |
| Hub: `npx vitest run` | **1,963 passed**, 93 files, exit 0 |
| Hub: routes, migrations | **95** routes under `/api/skynet/v1`, **29** migrations (27 Skynet) |

## The finding: the environment blocker is gone, and nobody has noticed yet

Round thirty-four wrote down the reason §10's pairing and the supervised loop had not met a real
Hub, and it was a fact about a particular machine rather than about the code:

> *"`skynetd` serves on a Unix socket and launches through systemd, so the daemon proper runs in
> WSL. The Hub runs on Windows. **WSL cannot reach the Windows host on this machine** … So the
> crossing runs entirely on Windows, through `skynetd once`."*

`scripts/skynet-verify-recordings-robot.mjs` says the same thing in its own header — *"§10's
pairing runs over `skynetd`'s Unix socket, which does not exist on Windows"*.

**This session is one Linux host with both repositories checked out on it, a working Docker
daemon, `psql`, and Node.** The daemon's socket exists here; the e2e proves it by talking to a
running `skynetd` over it 117 times. Nothing has to cross a firewall, because there is no second
machine. So the item round thirty-four left — *the daemon ran single passes, not the supervised
loop, and §10's pairing did not run* — is the first thing on the list that is neither a vendor's,
a machine's nor the owner's **on this machine**, and it has been carried as blocked for three
rounds on a blocker that was never about the code.

What still has to be true for it: a real Supabase behind the Hub, which means image pulls. That is
the first thing to try rather than the thing to assume.

## Three documents behind the tree, again

The failure §73's own header is written against, and the third round in a row to find it.

* **`PROGRESS.md` is six rounds behind.** `docs/README.md` names it the OS repository's log —
  *"Where progress is logged: in `PROGRESS.md`, here"* — and this file's own header says its
  findings *"are folded into those two when a round closes"*. Rounds thirty-one through
  thirty-six are closed and none of them is in it: the last entry is the seventy-second,
  2026-09-07, §27's `policy`. Missing from the canonical log: §47.6's mapping half, the second
  supervisor, the audit, §79's two halves meeting, `skynet doctor` and a black box that can be
  read, and replay. A reader of the repository's own log would plan around six rounds of holes.
* **`desktop/README.md`'s *"What is not here"* names three things that were built on 2026-09-09.**
  It says *"no tray icon and no global hotkey"* — `windows/Services/TrayIcon.cs` and `TrayStop.cs`
  are in the tree with twelve tests — and *"no black box replay"*, which the same file describes
  at length two hundred lines earlier under a heading of its own. The stale half and the current
  half are in one document, which is the shape that costs a reader the most.
* **Its pane count is wrong in both directions.** The same file says *"one of five panes"* in one
  place and *"macOS is seven panes and two features behind"* in another; `DetailPane` has five
  values and macOS has none of them. Session thirty-two's log said *"six panes and a tray"*. Three
  numbers, one enum, and the enum is two lines long.

## And one correction to the Hub's own log, in the other direction

Session thirty-two recorded `tsc --noEmit` as *"carrying eight errors — all of them in
`lib/__tests__/semantic-vector-store.test.ts`"*. It carries **none**; the file typechecks. Whatever
closed it was the owner's marketplace work in the commits after that session, and it is worth
recording that a stale *"still broken"* is the same defect as a stale *"still missing"* — a reader
believes it for free, and it makes the tree look worse than it is instead of better.

## The list, restated against this tree

**Buildable here, on this machine, needing nobody:** the three documents above, and §10's pairing
plus the supervised loop against a real Hub if Supabase will come up.

**A vendor's:** a capture backend to fill §47.6's pool; inference on what it captures; one real
RHAL driver.

**The owner's:** a third language edition; the countersignature on a conformance result (§80, and
§83.4 makes an operator attestation a third claim again); which robot data survives a marketplace
listing (`20260905200100_marketplace_listing_transactions.sql`).

**Hardware's:** a physical joint under the kernel; a TPM for `security.identity.attested`; an OTA
image this Hub has applied; three of §15's eight runtime tests; the certified tier's MCU and its
notified body; `vram_mb` and the `DeviceAllow` behind it.

**The Hub's infrastructure, and correctly queued rather than faked:** Phase 10's real worker (the
in-process runner does dataset-index and search-reindex and says so in its own header), Phase 11's
GPU training, inference endpoints and Spaces, and Phase 12's teleoperation session.

**The Console's:** USB and serial (no route on the robot to talk to); signing, notarisation and the
installer (certificates, and `CONSOLE_RELEASES` is deliberately empty until a pipeline fills it);
macOS, which has never been compiled and cannot be compiled here either — SwiftUI, `Network` and
`WKWebView` are Apple frameworks and this is a Linux container.

**Designed-open on purpose:** §30, §43, §53, §58.3, unchanged.

## One thing about the state of the branch, which neither log says

Both logs end *"Nothing is committed. Nothing is pushed."* Both are out of date: this repository has
**33 commits** past `origin/main` and BotMarket **27**, and in both cases `origin/claude/vibrant-ramanujan-wif8q6`
is level with `HEAD` — so the work is on the remote already, on a branch rather than on `main`.
Nothing has been merged. Said here because a sentence claiming nothing is pushed is exactly the kind
of sentence somebody plans around.

## What the audit then closed, in the same round

The three documents, because they are the whole of what an agent can close without a vendor, a
machine or the owner — and because leaving an audit's findings in the audit is the failure the audit
was written against.

* **`PROGRESS.md` gained rounds thirty-one through thirty-six**, as entries seventy-third to
  seventy-eighth, with findings 142-154. It is the OS repository's canonical log by `docs/README.md`,
  and it had been six rounds behind.
* **`desktop/README.md`'s *What is not here*** no longer says the tray icon, the global hotkey and
  replay are missing, and its pane count agrees with `DetailPane` — five values, one of which macOS
  draws, so **four panes and the stop behind**. The correction is left visible rather than deleted,
  which is the convention that section already used once.
* **The macOS application's own `NotInThisBuild` list** gained pairing and §41.3's bundle. It is
  drawn on screen, so it is the one stale list an operator sees rather than a reader.
* **`supabase/schema/skynet.sql`'s header** said closing the baseline was *"named as outstanding in
  the build log"*. The build log says the owner **withdrew** it on 2026-09-04 as scaffolding rather
  than product. Fixed in `scripts/skynet-schema-dump.mjs`'s `HEADER` and in the generated file, which
  the generator emits verbatim, so `--check` stays honest on a machine that has `pg_dump`.

That last one was found by trying to stand a local Supabase up. `supabase start` applies
`supabase/migrations` in order and dies on the **first** Skynet migration with `relation
"public.org_members" does not exist` — the header's paragraph arriving as an error message. So the
one buildable crossing above is blocked here after all, and on a decision the owner has already
taken rather than on effort: a fresh environment cannot be built from `supabase/`, and the file that
would change that was withdrawn.

The Hub's own log carries this as session thirty-three, in both trees, and the pins were taken with
`--pin` from the two agreeing copies.

`cargo test --workspace` 1,739 · `e2e.sh` 117 · Hub `vitest` 1,963 · `tsc --noEmit` clean · mirrors,
contract mirrors, golden path, RHAL flags and `scale-report --check` all green. No Rust changed, so
no figure in §73 moved.

Committed. **Nothing is pushed.**

---

# Round thirty-eight — §38.2's fifth layer, which the specification described and nothing implemented

Asked what a robotics engineer who has just installed Skynet OS would want on the Console to
*optimize, adjust, customize* — and whether the Console has it.

The answer came out of §83.2 rather than out of a wishlist, because that section forbids most of
the obvious one in advance: the Console is *a shell, not a client*, and everything the on-robot
console renders is already there through the embedded page. So the question narrows to **what can
an engineer not adjust anywhere at all** — not in the Console, not on the robot's own page, not
from the CLI.

Three things, and the first is the one this round is about.

## The finding

§38.2 lays out safety configuration as five layers:

> `vendor defaults → fleet profile → site profile → cell profile → session override`
>
> *"Session overrides are time-boxed and expire — a 'maintenance mode, half speed' that outlives the
> maintenance is a hazard that looks like a setting."*

The fifth is the only one a person sets **at the machine**, and it is the verb a bench engineer
actually has: *slow this thing down while my hands are near it.* It did not exist.

* `session_override` appeared **once** in 131,000 lines of Rust — in a doc comment, as a concept.
* `ResolveLimitsParams` had five inputs and none of them was a session.
* No route wrote one. No CLI verb set one. Nothing expired one.
* And **§43's open list said *"Profiles layer cleanly on one robot (§38.2)"*** — deferring only the
  fleet-wide version, which reads as a settled sentence about four layers where the specification
  names five.

That is the same shape as the stale READMEs two rounds ago, one level deeper: a document believed
for free, and the belief cost the feature.

## Why the Console could not have it

The Console is downstream. A pane cannot offer a control for a route that does not exist, so the
work is the OS's — which is also the half this machine can actually verify, unlike the WinUI half.

## What the kernel gained, and the one design decision in it

`SessionOverride` joins the same `narrowest()` minimum as every other layer, which is what makes
*"it only ever narrows"* a property of the arithmetic rather than a rule to remember. A widening is
recorded and discarded by the same expression that composes a legitimate narrowing.

**Checked against what the layer above allows, not against the envelope**, which is §38.2's own
wording and the case a reader would not think to write: a session asking for 1 rad/s under a profile
allowing 0.5 has not widened the *envelope* — 1 is well inside 2 — but it has asked to raise the
number actually in force, which is the one the operator can see and the one they will believe they
changed. It is a corpus case now.

Reported as `session_widened` rather than folded into `profile_widened`. Two documents, two
sentences: telling somebody their signed profile over-reached when it was their bench override sends
them to edit a file that is not the problem.

**Expiry is deliberately not in the kernel.** §35.2 keeps time out of it — `now_ms` is a parameter
to a cycle, never something the kernel reads — so it cannot ask whether a deadline has passed.
`skynetd` holds the deadline and simply stops handing the override to `resolve_limits`. One clock in
the system rather than two that can disagree.

## The output shape, which is why the corpus did not move

The obvious build adds a `session` key to the resolved limits. `limits_to_json` writes every key
explicitly with `null` for absent scalars, so that would have changed **every one of 1,142
expected byte-strings** across three editions, for a field that is null in all of them.

Instead the session reaches an operator two ways it already had: the **numbers move**, and a
`session_override` **notice** announces it by name. Notices are a list, so a robot with no override
emits nothing new — and the diff on `corpus/limits/resolution.json` is **243 insertions and zero
deletions**. Six new cases, 1,142 unchanged, and both editions still reproduce all 1,148 byte for
byte.

That the notice is emitted at all is the point rather than a courtesy. §38.2's hazard is not a slow
robot; it is a robot that is *quietly* slow for a week and a team that concludes the arm is faulty.
The notice reaches every surface that renders one — `skynet limits`, the robot's own console, the
Hub's device page — without any of them knowing this layer exists.

## The daemon, and what a bench actually gets

`crates/skynetd-io/src/session.rs`: a store with three readings — **live**, **expired**, and
**unreadable** — kept apart rather than collapsed. Composing a ceiling has no use for the
difference; telling an operator does, and a console showing an expired override as "nothing set"
leaves somebody wondering why the robot sped up.

Wall time rather than monotonic, by §45.4's reasoning: a deadline has to survive a reboot, and a
machine switched off for a day comes back with the override expired rather than reapplied.

**Eight hours maximum**, refused past it with the reason in the sentence: longer than a bench
session is a change to how the robot behaves, and that is what §38.1 signs profiles for. A ceiling
of zero or less is refused too — `narrowest()` would take it as the tightest input and produce a
robot that cannot move at all, which is safe and undiagnosable from a limits page.

`GET|POST|DELETE /session`, operator role for the two that write. Operator rather than admin because
the layer can only narrow, and §39.1 already gives an operator *capability grants*, which widen what
a package may do. Also because §38.2's argument is practical: if this is hard to reach, nobody uses
it and they work on a robot at full speed instead.

Both of §38.3's records are written, and the audit row carries **the previous value** — the half
that is easy to skip and the half that makes a log answer *what did somebody change* rather than
*what is it now*.

`skynet session [set|clear]`, with `--for` required and no way to omit it. Durations are suffixed
(`20m`, `2h`) rather than bare, because the two readings a person is likeliest to have in mind —
minutes and seconds — are sixty times apart on a control that slows a robot down.

## What the suite caught that no unit test could

`with_session` existed and **nothing called it**. The API test that asserts `GET /limits` moves
after a `POST /session` failed on its first run, which is the same lesson as round thirty-four's:
two halves that have only met doubles have not met. It is a helper now — `live_session()` — used at
every one of the five places a ceiling is composed, including §17's check-in, because a Hub device
page drawing ceilings this robot is not running at is the same drift by a different road.

## And one the e2e caught, where the test was wrong rather than the code

The bench assertion *"a widening override is refused"* failed, and the code was right. That robot
has no declared envelope and no profile ceilings, so there is nothing above the session to widen: a
high number there is not a widening at all, it is the only ceiling the robot has, and going from
*unbounded* to *99 rad/s* is a narrowing however alarming it looks written down. The check now
asserts that property under its own name, and the refusal is pinned where a body with declared
ceilings exists — the kernel's suite, and six corpus cases across three editions.

## Numbers

* `cargo test --workspace` — **1,775 passed** (1,739 before). Kernel 116, and the store's own 17.
* `sh crates/skynetd-io/e2e.sh` — **133 checks, 0 failed** (117 before), against a running daemon.
* `node tools/edition-report.mjs` — **1,148 of 1,148** in Rust and Python.
* `corpus/limits/resolution.json` — +243 lines, **0 removed**.
* clippy clean, fmt clean, `thumbv7em-none-eabihf` still builds with no allocator.
* Hub: `tsc --noEmit` clean, `vitest` 1,963 passed. `desktop/tests` **190 passed** (173 before).
* §73 corrected: 131,000 lines of Rust, 1,148 corpus cases, 58 CLI commands.

## One thing found and left alone, deliberately

`tools/check-golden-path.mjs` counts CLI commands by taking every string literal on a match arm
across the crate — broad by design, so that adding a command never fails a build falsely. That
breadth was equal to the true count by coincidence, and this round broke the coincidence: a `match`
over state names inside a printer put two non-commands into a figure §73 publishes.

The counting rule was **not** changed. Narrowing it to `main.rs`'s dispatch gives 34, and §73's
"55" has always meant the whole surface including subcommands — so a "fix" here would have silently
redefined a published number. The two spurious literals were removed from the printer instead, with
the reason written where somebody would otherwise reintroduce them, and the honest count (58) went
into §73. The fragility is real and is recorded here rather than papered over: the next unrelated
`match` over strings in this crate will move that number again.

## What round thirty-eight leaves

Two of the three things an engineer cannot adjust are still open, and both are smaller than this
one was:

* **local unsigned configuration** — §38.1 names *"log level, telemetry sampling, console theme"*
  as unsigned and local, and there is no route, no CLI verb and no file for any of them;
* **calibration** — `GET /calibration` reports; nothing runs or refreshes one.

The Console's session pane and bench pane are both **written and not compiled**, which is unchanged
and is stated in `desktop/README.md` under *What is not here* rather than implied. One
`dotnet build -p:Platform=x64` on Windows settles both.

Nothing is pushed.
