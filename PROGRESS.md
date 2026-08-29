# Build log

**This is where work on Skynet OS is logged.** Not `docs/skynet-build-plan.md` — that is the Hub's
document and its session log is the Hub's history, and it is one of three files mirrored between
this repository and BotMarket. Appending OS history to it would either put OS work into the Hub's
log or break the mirror. See `docs/README.md` for the mirroring rule.

Where a session changes something **in the Hub**, the change is made in both copies and recorded
here as well, so this log stays a complete account of what was done from this repository.

What has been built here, what it is measured by, and what is still zero lines. The specification is
`docs/skynet-os-architecture.md`; section references below are to it.

---

## 2026-08-29 — the repository split, and the parts that must be right before hardware

§22 sets the build order and gives the reason: *"Transport and the kernel are the parts that must be
right before anything touches hardware, and they are the parts that can be written and tested
without any. The temptation is to start with a driver because it produces visible motion; the result
of starting there is a system whose safety argument is retrofitted."*

So this session built those, plus the mechanism that keeps them honest.

### The repository exists, and that was the deadline

§72.3 says the split has to happen **before the first Rust lands**: moving a design document and a
few contract files is minutes, moving that plus a 40,000-line workspace and a CI matrix is a week.
The C header in the Hub's `bindings/` was already vendor-facing code sitting in a private
repository — the problem in miniature, and trivial to fix while it was still small.

Moved in: the specification (both halves plus the Hub build plan), the RHAL C and C++ headers, and
the TypeScript contracts as `reference/typescript/`.

### `crates/skynet-json` — canonical JSON

The rule the whole corpus comparison rests on, because §31.4 asks for **byte-identical** output and
that is only meaningful if there is exactly one way to write a value down. Sorted keys by UTF-8
bytes, no whitespace, integers without a fraction, everything else as the shortest positional
decimal that round-trips, and **absent is not null** — the distinction between "nobody declared a
safety envelope" and "the envelope is null" is the one the platform exists to preserve.

`no_std + alloc`, no dependencies. 16 tests.

### `crates/skynet-rhal` — the Rust view of the ABI

Not a second definition. §31.2 keeps the C ABI single-source "because two definitions of a struct
layout is a segfault", so every type is `#[repr(C)]` in header order and the offsets are asserted —
from Rust here, and from C in `drivers/mock/mock_robot.c` with `_Static_assert`. The two halves meet
in the middle: if either language's layout moves, one of the two suites fails. 7 tests.

### `crates/skynet-kernel` — the Safety Kernel

`#![no_std]`, and `alloc` is deliberately **not linked**, so "this never allocates" is a compile-time
property rather than a code-review promise (§23.1). It takes no clock — `now_ms` is a parameter
(§35.2) — which is what makes replay and simulation work with no special case.

The eight guards in specified evaluation order, the graduated responses, `resolve_limits` composing
three sources of ceilings **by minimum rather than by precedence**, and guards that cannot run
reporting unavailable with the specific missing input named. 45 tests.

It builds for `thumbv7em-none-eabihf` — one source, two targets, which is the certified tier's
argument made mechanical rather than asserted.

Three guards go slightly beyond the reference, and the reason is recorded rather than buried:

* **`mode_gate`** evaluates when the cycle names a capability, holding a command whose token the
  operator did not grant. The reference's cycle vector has no such field.
* **`proximity`** evaluates when something is measuring separation: stop inside the stop distance,
  scale between the two distances.
* **Frame overflow** is a `stop_cat2`, never a truncation — "there is a command I could not
  evaluate" and "there is a command I did not receive" are the same fact.

With reference-derived vectors, none of the three has its input, so behaviour is bit-identical and
the corpus still holds. They are covered by this edition's own suite instead.

### `crates/skynet-contracts` — L2

`resolve_transport`, `admit_package`, `award_certification`, `evaluate_readiness`, and the role
vocabulary all four lean on. Ported from the reference with the shapes pinned field by field. 84
tests. `no_std + alloc`, so an L2 edition can run at install time on a robot with no operating system
worth the name underneath it.

### `corpus/` — 254 cases, and the thing that makes editions survivable

Generated from the reference oracle by `tools/corpus-gen/generate.ts`, which runs curated inputs
through the TypeScript and freezes the canonical bytes. `expected` is a **string**, not an object,
because the requirement is byte-identical output and the bytes are therefore what the file holds.

Coverage: roles (38), transport (36), cycle verdicts (16), limit resolution (14), admission (21),
certification (13), readiness (12), plus 100 generated from two seeded PRNGs written out in full so
regenerating produces byte-identical files and a diff means a behaviour change rather than a
reshuffle.

`crates/skynet-corpus` holds this edition to it. **All 254 reproduce byte for byte.**

Verified that the corpus actually bites: inverting one branch of `derive_verdict` produced 8
divergences with byte offsets and the exact differing values, and reverting restored all 254.

### `drivers/mock` — the mock robot

A body that answers the RHAL with no hardware behind it, and 24 sim-conformance checks against it,
each naming the SCTS id it stands for. Portable C11, clean under `-Wall -Wextra -Wpedantic`, no
dependencies beyond a compiler.

The value is the fault injection: a stalled command stream, a joint reporting NaN, an estop asserted
mid-motion, a bus timeout inside a cycle. Being able to develop the dangerous cases without a robot
in the room is the difference between a driver that has been exercised and one that has merely been
run (§63.2).

Its descriptor declares `"envelope": null` even though the mock knows its joints' design torque
limits, because a design value from a model is not a datasheet safety rating (§63.4). That is the
line a driver author is most likely to cross, so the self-test checks it explicitly.

### CI

Four jobs. `rust` (fmt, clippy at `-D warnings`, tests, the MCU build), `abi` (the mock's
conformance run, the C++ wrapper, and that the C header is still includable from plain C99),
`corpus` (regeneration must be a no-op, then every edition sits the same exam), and `invariants` —
claims this repository makes about itself, checked mechanically: the safety claim rule intact, only a
driver able to run native, the kernel linking no allocator, and no third-party dependency anywhere in
`Cargo.lock`.

---

## Two divergences found by porting, and what happened to them

Writing the Rust against the TypeScript surfaced two places where a faithful port and a sensible
implementation disagreed. Both were resolved toward the oracle, because the corpus is generated from
it and parity is the governing rule.

1. **`no_rate_limit` carried subjects in my first draft.** The reference emits none — the sentence
   already names the setting, and the `rate_limit` guard's own unavailability notice carries
   `profile.max_delta_per_cycle` as its missing input. Naming it twice would put one fact in two
   places for a reader to reconcile.

2. **`common_blockers` ordering.** I sorted ties by the declaration order of `READINESS_CHECKS`,
   which is arguably the nicer property — a fleet dashboard that reorders itself on a tie is a
   dashboard nobody trusts. The reference uses an insertion-ordered map, so ties break by
   first-encounter order. Changed to match, and the reason is in the code: it is a *different
   answer*, not a nicer spelling of the same one, and a corpus case with two equally common blockers
   would have diverged.

## One thing the specification says that the code does not

**§15 says "29 tests across 8 areas". The reference implementation has 30.** The ids match exactly
between the two; the prose is what is stale. The Rust follows the code, because the code is the
oracle the corpus is generated from, and the count is documented at the definition so the next
reader does not re-derive the discrepancy.

## One property of the reference worth flagging rather than fixing

`ceiling_for` consults the per-channel limits and the base-velocity ceiling, and **not** the
whole-body `joint_velocity_max_rad_s` or `joint_torque_max_nm`. That is correct as far as it goes —
nothing at that layer knows whether a given channel carries radians per second or newton-metres, and
the per-joint rating from the transport plan is where a joint's real ceiling comes from. But it means
a channel with no per-channel entry has no magnitude ceiling at all, and a body whose envelope
declares a whole-body torque limit does not get it applied per joint.

Ported faithfully rather than "fixed", because changing it would break parity with the oracle and the
corpus would catch it as a divergence — which is the mechanism working. It belongs in the
specification as a question about what the whole-body figures are *for*, not in a quiet patch.

---

## Nothing in the Hub was changed

No file in the BotMarket repository was modified. The Hub's `lib/skynet/os/*.ts` stays exactly where
it is: §23.2 gives it a new role — the reference and differential-test oracle — and that role is
served by it staying put and being vendored here, not moved.

The two observations above are findings *about* Hub files, not edits to them. Both are small enough
to fold into the next Hub session rather than reaching across the boundary for; when they are made,
they are made in both copies of the specification.

## What is still zero lines

`skynetd` and its reconciler, the policy runtime, the SPSC ring and observation assembly, OTA and
A/B slots, the black box, every real driver (LeRobot, `ros2_control`, EtherCAT, CANopen, the
simulators, the vendor SDKs), the on-robot console, the frame graph, calibration, the watchdog chain,
POST and homing, and the safety MCU.

§59.5 puts the realistic figure at several engineer-years before a robot moves under this, and
nothing in this session changes that estimate. What it changes is that transport, the kernel and the
admission contract are now written, tested, and pinned by a corpus — which §22 says is the order that
matters, because those are the parts whose safety argument cannot be retrofitted.

## Next, in the order §22 and §32.4 imply

1. **The LeRobot RHAL driver.** The single fastest path from zero to a real device fleet: every robot
   LeRobot supports becomes a Guest-tier Skynet robot for the cost of one driver (§16, §67).
2. **The MuJoCo simulator driver**, which is what makes the sim half of SCTS runnable end to end
   rather than a set of assertions about a mock.
3. **`skynet descriptor from-urdf`** (§63.4) — the highest-leverage tool in Part VIII, because the
   work is largely already done and sitting in a file the robot already has. Roles suggested, never
   assigned; envelope left null.
4. **The Hub side**: the transport endpoint, the embodiment tables, and `/skynet/os` registry
   readiness — which §32.4 puts first because it needs no new tables and answers a question the
   ecosystem actually has.
5. **`skynetd`**, once there is a driver for it to supervise.

---

## 2026-08-29 (later) — the developer path: URDF import, the CLI, and local conformance

The previous session ended with a five-item list. This one took **item 3**, `skynet descriptor
from-urdf`, and the two §63 tools it needs to be useful — the scaffold and the local conformance
run — rather than items 1 and 2, and the reason is worth stating rather than glossing.

**Why not the LeRobot and MuJoCo drivers first.** Both are the right next drivers and §22 puts them
in the "buildable now" column. Neither can be *verified* here: LeRobot and MuJoCo are not installed,
and a driver written against a library nobody ran is a driver that compiles. Everything in this
repository so far has landed with a test that exercises it — the kernel against 45 cases, the
contracts against 254 corpus files, the mock against 24 conformance checks — and writing two drivers
whose only evidence is that they type-check would break that rule at exactly the point where the
rule matters, since a driver is the component that moves a machine.

§63's tools are the other half of the same "buildable now, no robot required" column, they can be
exercised end to end against `drivers/mock`, and §63.5 states the milestones they are measured
against. So they landed first, measured.

### `crates/skynet-sha256` — 312 lines, 4 tests

SHA-256, `no_std` with no `alloc`, held to the FIPS 180-4 vectors and the million-`a` case rather
than to its own previous output. The specification names sha256 in more places than any other
primitive — content addressing (§17), the descriptor digest (§5), package verification, the black
box seal (§13) — and every one is a place where two implementations disagreeing about a hash means
two implementations disagreeing about *identity*. It belongs in the workspace on the same terms as
`skynet-json`: one definition, no dependency.

The hex form is a fixed 64-byte buffer rather than a `String`, so the crate needs neither `std` nor
`alloc` and the same code runs on a workstation and on the safety MCU.

### `crates/skynet-urdf` — 2,796 lines, 37 tests

§63.4: *"the highest-leverage tool in Part VIII, because the work is largely already done and
sitting in a file the robot already has."* Four layers, and the boundaries between them are the
design:

| Module | Does | Never does |
|---|---|---|
| `xml` | Reads XML | Know what a robot is |
| `model` | Says what the file declared | Interpret it |
| `suggest` | Proposes roles | Assign them |
| `descriptor` | Emits the descriptor | Invent a limit, a mode or an envelope |

The split between `model` and `descriptor` exists so that "what the file says" and "what we
concluded from it" are never the same value. The split between `suggest` and `descriptor` exists
because a suggested role that silently became an assigned one is the failure §63.4 is written to
prevent.

**The XML reader is written rather than depended on**, for the workspace's stated reason. It is
strict: an unknown entity, a duplicated attribute and a mis-nested tag are all errors with a line
and a column, because a lenient parser hiding a second `lower=` produces a joint limit nobody can
predict.

**What the importer refuses to do**, and none of it is a default a flag can reach past:

* **`envelope` is null. Always.** A URDF `effort` limit is a design value from a CAD model, not a
  datasheet safety rating, and copying one into `envelope.joint_torque_max_nm` would auto-populate a
  safety limit nobody verified. There is a CI invariant asserting the emitter writes a literal null
  and that nothing else can reach the field.
* **Roles are suggested, never assigned.** The report prints every suggestion, the reasoning behind
  it, and whether each part came from the name or was inferred. `--accept-suggested-roles` is the
  "and a human confirms" step of §63.4 and is a separate action precisely so that it is one.
* **`digest` is null and `verified` is false.** The vendor signs the descriptor; this tool does not.
* **`control.modes` is empty unless an operator asserts it.** A URDF says nothing about what a body
  accepts.

**Three decisions that were not obvious.**

1. **Mimic joints are excluded from `joints[]` and kept in the frame graph.** A mimic joint moves and
   is not an actuator — it is a linkage, and commanding a parallel gripper's mirrored finger
   separately is how the linkage gets torn apart. It is not a degree of freedom either, so `dof`
   counts actuators.

2. **`base` defaults to `fixed`, and a `floating` or `planar` root refuses instead.** A URDF states
   how the root link moves *within the model* and nothing at all about whether the machine drives
   around a room. Where the model settles it, the tool asks; where the model is silent, `fixed` is
   the conservative reading, because a fixed base makes base-velocity channels *refuse* rather than
   command motion the body may not be able to produce. When the joints look like legs or wheels the
   report says so and names `--base`.

3. **`--control-modes` sets the body profile and every joint together.** Found by running the output
   through `skynet transport`: a descriptor with an empty `joints[].control_modes` has every channel
   **blocked** with "this joint does not accept position commands", which is truthful and useless.
   The operator passing the flag is asserting what the body accepts, and that assertion covers every
   joint; a wrist that is position-only on a torque-capable robot is an exception edited in by hand.
   The report's wording was corrected to match what the planner actually does — it had said
   `unsupported_mode`, and the real behaviour is a body-level `control_mode_unsupported` warning plus
   per-channel blocking.

**The frame graph** (§44.3) comes out of the link tree: every link a frame, `dynamic` where it is
reached through a movable joint and naming the joint and its axis, `static` otherwise, both carrying
the joint origin as a translation and an `xyzw` quaternion in REP-103 (§44.4). That is a complete FK
specification, which is what §44.2 needs — the kernel computes its own forward kinematics and
subscribes to nothing. Quaternion components are rounded to 1e-12 so two runs on two machines with
different libm last bits produce byte-identical output; a descriptor whose digest depends on which
laptop generated it is a descriptor nobody can verify.

### `crates/skynet-cli` — 3,696 lines, 31 tests

The `skynet` binary. Five commands, and the absences are deliberate.

```
skynet descriptor from-urdf robot.urdf > embodiment.json   §63.4
skynet driver new my-arm --lang c|rust                     §63.1
skynet driver check ./libmy_arm.so                         §63.3
skynet transport ./policy.json --robot embodiment.json     §18
skynet role left_arm.shoulder.pitch                        §5.1
```

`pair`, `install`, `grants`, `ps`, `logs`, `estop` and `blackbox` are **absent and named as absent**
in the usage text, with the reason: they talk to a running `skynetd`, which is not written. A test
asserts each one fails and that the usage names it. A command printing a plausible fleet status from
nothing is the exact failure this project is arranged around.

**`from-urdf` writes the descriptor to stdout and the reasoning to stderr.** §63.4 writes the
invocation with a redirect, and that only works if the reasoning does not end up in the file. It
also means the reasoning is *seen*: a report written to a file beside the descriptor is a report
nobody reads, and the whole value of the tool is in the four things it refuses to fill in.

**`driver new` produces something that fails on purpose.** §63.1 asks for "a building, loading,
failing-for-the-right-reasons driver… and the failures are the to-do list". A scaffold that passes
everything teaches nothing and invites shipping the stub; one that fails everything is
indistinguishable from a broken toolchain. So the generated driver gets the *shape* right — it
opens, it validates command shape before acting, a zero mask holds rather than zeroing, its estop is
observable, its descriptor decodes — and leaves the bus, the joint list, the roles and the envelope.
Both templates were compiled and loaded; the C one is clean under `-Wall -Wextra -Wpedantic`.

**`driver check` loads the shared object and reports a tier, not a verdict.** This is where the
session found the design rather than following it. The first version exited non-zero because
`embodiment.descriptor.signed` was `missing`, which is wrong: §15's second award rule is *the
evidence caps the award, not the claim*, and a driver that passes every guest-tier sim test and has
no signed descriptor is a **guest-tier driver** — a true statement about it, not a failure. Saying
so keeps a vendor in the programme instead of sending them off to write "ROS 2 compatible" on a web
page. So the tier is computed from `SCTS_TESTS` in `skynet-contracts` rather than from a second
table, only `sim` tests count, and the blockers to the next tier are named individually.

Two more things it does that a simpler harness would not:

* **It lists what it does not cover, by name.** `safety.kernel.parity` (an edition property —
  `cargo test -p skynet-corpus`), `safety.policy.isolation`, `package.*`, `ota.safety_gate`,
  `telemetry.replay.deterministic`, `security.*`. A vendor arriving at certification believing they
  had evidence they do not have would be a failure of that list, not of their diligence.
* **It checks the descriptor against the vtable.** A body declaring sensors with a null
  `read_sensor` fails: a declared stream nothing can read binds a policy's observation to nothing.
  Declaring neither is `not_applicable`, which §15 counts as a pass.

**The `unsafe` boundary.** `dl` is the only module in the workspace outside `skynet-rhal` with
`unsafe` in it, and it cannot be otherwise — the thing being loaded is arbitrary native code. It is
four platform functions written out rather than a `libloading` dependency, `RTLD_NOW` so a missing
symbol is a load failure rather than a crash halfway through a run, an absolute path on Windows so
"it loaded a different driver with the same name" cannot happen, and a `SAFETY` note on every block.
A new CI invariant asserts every other crate still carries `forbid(unsafe_code)`.

### CI

Three additions. The `abi` job now builds a fresh scaffold with a real C compiler, loads it, and
asserts it reaches guest tier with exactly the two blockers its own README names — §63.5's first
milestone measured rather than asserted. The test is `#[ignore]`d by default and run explicitly
there, which is scheduling rather than skipping: a check that quietly does not run is the failure
mode §15 spends a whole rule on. `invariants` gained the envelope rule and the unsafe boundary.

### Verification

* **227 tests pass**, up from 153. `cargo fmt --all --check` clean, `cargo clippy --workspace
  --all-targets` clean at `-D warnings`.
* The kernel still builds for `thumbv7em-none-eabihf`.
* All 254 corpus cases still reproduce byte for byte. Nothing in this session touched the contracts,
  the kernel or the oracle.
* `Cargo.lock` holds eight packages, all of them ours. The no-dependency property is intact.
* End to end, by hand and in the suite: a six-joint SO-101-shaped URDF → a descriptor → a transport
  plan against a policy, with the gripper channel correctly refused and the base-velocity channel
  correctly dropped.
* `drivers/mock` compiled as a shared object and run through `driver check`: 23 pass, 0 fail, 1 not
  measured, 1 not applicable, guest tier.
* Both scaffold languages compiled and loaded: 22 pass, 1 fail, 1 not measured, 1 not applicable,
  guest tier — and the failure is `embodiment.roles.coverage` at 0%, exactly as the generated README
  says it will be.

### Three findings, recorded rather than quietly fixed

1. **The `frames` block's field names are this tool's, not the specification's.** §52 lists `frames`
   as a descriptor field and §44.3 describes the tree, but neither pins the field-level shape, and
   the TypeScript oracle does not carry it at all. Nothing consumes it yet — the transport decoder
   ignores unknown keys, so the corpus is unaffected — but a second edition emitting a different
   spelling would be a divergence nobody's test catches. **The shape belongs in §44.3 and in the
   oracle before anything reads it.**

2. **`end_effectors` is always empty, and that blocks the channel most policies emit.** URDF has no
   way to say "this is a gripper" — a hand is a joint and a link like any other — and the transport
   planner routes a `gripper` channel to an end effector rather than to a joint. So a hand that
   exists in the kinematics and not in `end_effectors[]` is refused. The importer now says so in the
   report, and the honest fix is a person declaring the effector with its kind, dof and grip force.
   Guessing one from a joint named `gripper` is the same mistake as guessing an envelope.

3. **`--assume-side` is an operator asserting a fact the file does not contain**, and every
   suggestion it enables is marked `inferred` for that reason. It is right for a single-arm robot,
   whose URDF has no reason to write `left_`, and it is a loaded gun on a bimanual one. The
   suggestions still print, so accepting them is auditable — but the flag deserves a second look if
   anyone reports a mis-mapped arm.

### Nothing in the Hub was changed

No file in the BotMarket repository was modified, and `node tools/check-mirrors.mjs` reports all
three shared documents byte-identical. The two findings from the previous session about Hub files
are still open and still belong in the next Hub session.

### What is still zero lines

Unchanged from the previous session, minus the tooling: `skynetd` and its reconciler, the policy
runtime, the SPSC ring and observation assembly, OTA and A/B slots, the black box, every real driver
(LeRobot, `ros2_control`, EtherCAT, CANopen, the simulators, the vendor SDKs), the on-robot console,
the frame graph *consumer* (the emitter exists; nothing computes FK from it), calibration, the
watchdog chain, POST and homing, and the safety MCU.

§59.5's several-engineer-years figure is unchanged. What changed is that a vendor can now go from a
URDF and an afternoon to a driver that loads, a descriptor that is honest about what nobody has
declared, and a conformance report that names the tier it has earned.

### Next

1. **The LeRobot RHAL driver** (§16, §67), with the per-robot role mapping tables — now cheaper than
   it was, because `skynet driver check` is what will hold it, and the SO-101 mapping §67.2 gives by
   hand is the one the URDF importer already reaches from names.
2. **The MuJoCo simulator driver** (§70), which makes the sim half of SCTS runnable against a body
   with physics rather than against a mock.
3. **The Hub side** (§32.4): `/skynet/os` readiness over the live registry, the transport endpoint,
   the embodiment tables. Needs no new tables for step one and answers a question the ecosystem has.
4. **`frames` into the specification and the oracle**, before a second edition emits a second
   spelling of it.
5. **`skynetd`**, once there is a driver for it to supervise.

---

## 2026-08-29 (third) — the LeRobot bridge, and how a Python driver reaches the OS at all

Item 1 from the previous session's list: *"The LeRobot RHAL driver. The single fastest path from
zero to a real device fleet: every robot LeRobot supports becomes a Guest-tier Skynet robot for the
cost of one driver."*

Building it turned out to require answering a question the specification leaves implicit. §31.3 says
Python is a Guest-tier edition and §42 says almost everyone asking for their language wants L1 —
"write drivers and policies against an OS already running". Neither says **how a Python driver is
loaded by an OS that dlopens a C ABI**, and the obvious answer is wrong.

### The design finding: embedded CPython cannot implement `emergency_stop`

§4's second rule:

> `emergency_stop()` must work from a signal handler and from a process that is dying.

Calling into CPython from a signal handler is not safe. The interpreter takes locks and allocates,
and a stop path that can deadlock is worse than one that is slow. Writing one line to a pipe *is*
async-signal-safe.

So a Python driver runs **out of process**, and that is not a workaround — it is the only
arrangement in which one can implement the entry point that matters most. Two more properties fall
out of it the right way round: a GC pause stalls the interpreter rather than the host, which can
then report a late answer as `SKR_ETIMEDOUT` truthfully instead of absorbing it; and a Python driver
that crashes takes a child process with it while the OS keeps a live stop path.

The cost is a pipe round trip per cycle — tens of microseconds. Unaffordable on a 1 kHz torque loop,
irrelevant at Guest tier, which is exactly where a Python driver is capped. The cost lands where it
does not matter, which is the sign of a boundary drawn in the right place.

### `bindings/python/skynet_rhal.py` — 555 lines

What §4 promises ships. Two things sharing one vocabulary:

* **The driver protocol** — `Driver`, `State`, `Command`, `Frame`, and `serve()`, which speaks the
  host's wire format. `serve()` refuses any `ceiling` but `guest` and says why in the exception:
  §31.3's third rule is that an edition may not raise its own ceiling, and a Python process cannot
  make the promise the higher tiers encode.
* **A ctypes view of the C ABI**, for code that *consumes* a driver. Its field offsets are asserted
  against the same numbers `crates/skynet-rhal` asserts from Rust and `drivers/mock` asserts from C.
  §31.2 keeps the ABI single-source "because two definitions of a struct layout is a segfault", and
  the way to keep that true is to assert it from every side: three suites now fail if the layout
  moves, rather than one robot behaving oddly. Verified on this platform.

Standard library only. A driver author with numpy or LeRobot uses them in their own code; this
module imports neither.

### `drivers/python-host` — 800 lines of C

One shared object, exporting the one symbol, forwarding every RHAL call to an interpreter it
spawned. Newline-delimited ASCII over the child's stdin and stdout — deliberately not JSON, because
the host would then need a JSON parser in C and the only structured thing crossing the boundary is
the descriptor, which passes through opaquely.

The stop path is the part worth reading. `emergency_stop` sets a latch and writes `E\n`. The latch
is a single store to a flag the command path already reads, so **the stop takes effect whether or
not the interpreter ever answers**; the write is a best effort on top of it. Nothing on that path
allocates, locks, or calls into an interpreter. The reply is not waited for — waiting is what a stop
path must never do — so it is drained before the next exchange instead.

Configuration splits: `SKYNET_PYTHON`, `SKYNET_PYTHON_DRIVER` and `PYTHONPATH` say *what to run*,
and the config JSON is forwarded to the child verbatim because it configures the *robot*, which is
the child's business.

The handshake carries the ABI version, the protocol version and the ceiling, and a child announcing
anything but `guest` is **refused rather than believed**.

### `drivers/lerobot` — 1,486 lines of Python, 70 checks

The bridge, the role tables, a fake robot, and a conformance harness that names the SCTS id each
check stands for — the Python counterpart of `drivers/mock/selftest.c`.

§67.2's table is the cheapest large win in the whole design, and it is a few dozen lines:

```
shoulder_pan   → right_arm.shoulder.yaw      wrist_flex  → right_arm.wrist.flex
shoulder_lift  → right_arm.shoulder.pitch    wrist_roll  → right_arm.wrist.roll
elbow_flex     → right_arm.elbow.flex        gripper     → right_hand.gripper.grip
```

Tables for SO-100/101, Koch, LeKiwi and HopeJR. Two things about them are said out loud rather than
assumed: **the side is a convention** — an SO-101 is a single arm, nothing about it is left or right,
and the specification's own example writes `right_arm`, so a bimanual rig needs `role_overrides` —
and **an unmapped robot class gets no roles at all** rather than plausible ones, because a joint with
no role can only be matched by exact name, which is truthful.

**Four things the bridge does not know, and declares rather than inventing:**

1. **Joint ratings.** LeRobot does not publish limits through the `Robot` interface; they live in
   the motor bus config and the calibration file in raw encoder units. So `joints[].limits` is
   empty and every transport channel bound to this body is reported `unscalable_channel`. That is
   the correct outcome and worth understanding before it surprises somebody: a body with no ratings
   is a body a policy cannot be safely scaled onto.
2. **The envelope.** Null.
3. **Acquisition time.** `get_observation()` supplies none, so the bridge stamps at read (§67.1,
   §47.2) — later than acquisition by an unknown amount, which inflates latency budgets in the safe
   direction, and the provenance says so.
4. **Units.** LeRobot's `.pos` is normalised or degrees depending on the robot and its calibration;
   the RHAL is radians. The bridge converts only when told and otherwise passes through *and
   declares that it did*. Treating degrees as radians is a 57× error on every command and looks like
   a tuning problem for a day.

**One decision that surprised me while writing it.** A masked joint must *hold*, and holding has a
correct realisation in position (command where it already is) and velocity (zero) and **none in
torque** — holding a torque-controlled joint means gravity compensation, which the bridge cannot
compute, and commanding zero torque would drop the arm. So a torque command with any joint masked is
refused rather than approximated. That is the mask's whole purpose applied to its own hardest case.

### `skynet driver check` found two bugs in the host, the same session

Both real, and both the kind that do not present where they happen.

1. **The stop reply desynchronised the stream.** `emergency_stop` wrote `E\n` and did not wait for
   the reply — correct — but nothing drained it, so the `R 0` arrived where the answer to the *next*
   request should be and every later exchange sat one frame out. It presented as
   `an asserted stop is visible in the next state` failing, which is nowhere near the cause.
   Fixed with a pending-reply counter drained before each exchange.
2. **The descriptor declared a camera the vtable could not deliver.** The bridge advertised the
   wrist camera and the host's `read_sensor` was null. `driver check`'s consistency test caught it —
   *"a declared stream that cannot be read binds a policy's observation to nothing"* — and the fix
   was to implement frame forwarding rather than to quieten the check.

Then a third, from implementing that fix: on a failed sensor read the child sent an empty payload
line the host never consumed, desynchronising the stream again. The lesson is in the protocol now: a
failed read has no payload.

Writing the check first and the driver second is what made all three cheap. That is §63.3's argument
— *"turning up to a bench session with the sim suite already green is what makes bench time cheap"* —
holding a day earlier than it was meant to.

### `skynet driver check` gained a real sensor read

It used to check only that the vtable's `read_sensor` was non-null when the descriptor declared
sensors. It now reads the first declared sensor and checks the frame describes itself consistently —
`stride × height ≤ size`, a non-empty buffer, a non-zero stamp. The geometry assertion is the one
that matters: a frame whose declared shape does not fit the bytes behind it is a buffer overrun
waiting for the first consumer that trusts the header.

It also runs *after* the state checks now, because the OS's own order is state then sensors and a
driver that acquires frames per cycle has legitimately produced nothing before the first one.

### Verification

* **`skynet driver check` → the C host → a forked interpreter → the LeRobot bridge → a fake SO-101
  with a wrist camera: 24 pass, 0 fail, 1 not measured, guest tier.** The whole chain, on Windows.
* **`drivers/lerobot`: 70 checks, 0 failed**, including a protocol section that drives `serve()`
  through in-memory pipes — so the wire format is held on every platform, not only where a C
  compiler is.
* The Python ctypes ABI view matches the C header's layout.
* Unchanged and re-run: 227 Rust tests, 254 corpus cases byte for byte, the mock's 24 checks, the
  scaffold's build-load-and-reach-guest-tier test. `fmt --check` and `clippy -D warnings` clean.
* `Cargo.lock` still holds eight packages, all ours.
* The C host compiles clean under `-Wall -Wextra -Wpedantic`, which caught two portability bugs
  while it was being written: `CONTEXT` collides with a type in `<winnt.h>`, and `%zu` predates
  MinGW's printf.

### What is honestly not verified

Stated plainly because the whole point of this project is not to overclaim.

* **The bridge has never seen LeRobot.** It is written from the public `Robot` interface, which
  §62.2 says is how these drivers get written — but passing the harness means the adapter is
  *internally consistent*, not that its field names match the library's on the day you run it. The
  harness prints that sentence at the end of every run, and the README says it twice.
* **It has never seen hardware.** Guest tier either way.
* **The host's POSIX path has not run here** — this machine is Windows. CI's `abi` job runs it on
  Linux, which is why that job now builds the host and drives the whole chain through it. macOS is
  not exercised at all; it shares the POSIX path and the difference is untested, and the README says
  so rather than implying coverage.
* `SKYNET_LEROBOT_FAKE` is opt-in rather than a fallback, because a driver that silently substitutes
  a simulated body for a missing real one will one day report a successful grasp that never happened.

### CI

The `abi` job now also asserts the Python ABI view against the header, runs the LeRobot bridge's
harness, and drives `skynet driver check` through the C host on Linux — which is the half of the
host that cannot be exercised on the machine it was written on.

### What is still zero lines

`skynetd` and its reconciler, the policy runtime, the SPSC ring and observation assembly, OTA and
A/B slots, the black box, the on-robot console, the frame graph consumer, calibration, the watchdog
chain, POST and homing, the safety MCU, and the drivers that talk to real hardware — `ros2_control`,
EtherCAT, CANopen, the simulators and the vendor SDKs.

The MuJoCo driver (§70) was on this session's list and is **not** built: MuJoCo is not installed
here, and a simulator driver whose only evidence is that it type-checks is precisely what the sim
half of SCTS exists to prevent anyone shipping. It stays first on the list rather than landing
unverified.

### Next

1. **The MuJoCo simulator driver** (§70), on a machine with MuJoCo. It is now a much smaller job:
   the Python driver path exists, the host loads it, and `skynet driver check` is what will hold it.
   It declares `simulator: true`, which forces Guest and stamps every check-in and evaluation result
   (§35.3, §56.2), and it supplies the clock.
2. **Run the LeRobot bridge against the real library**, and against an SO-101 if one can be found.
   Everything it says about itself is provisional until then.
3. **The Hub side** (§32.4): `/skynet/os` readiness over the live registry, the transport endpoint,
   the embodiment tables. Needs no new tables for step one.
4. **`frames` into the specification and the oracle** — still open from the previous session, and
   still the thing most likely to bite a second edition.
5. **`skynetd`**, once there is a driver for it to supervise. There are now two.
