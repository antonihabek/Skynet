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

---

## 2026-08-29 (fourth) — the simulator, the schema, and the supervisor's brain

Three items from the previous session's list, and the environment is what decided the order: MuJoCo
installs and runs headless on this machine, so item 1 stopped being a driver whose only evidence
would be that it type-checks and became one that can be held to physics.

### `drivers/mujoco` — the simulator driver (§70)

> MuJoCo and Isaac Sim, and they are not a lesser path — the sim half of SCTS cannot run without
> them, so they are **load-bearing infrastructure**.

A scene file becomes a Skynet body: a descriptor built from the MJCF, a clock, a command path, a
camera, and five scenes in `scenes/` of which four exist only to be refused in a particular way.

**The design decision everything else follows from is that `read_state` advances the clock**, not
`write_command`. §35.3 makes the simulator's clock the kernel's clock so that watchdogs, rate limits
and deadlines are all measured in the units the simulator is producing — without which every
simulated run trips the watchdog in a few cycles and the usual workaround, disabling it in
simulation, means the thing being tested is not the thing that ships. That leaves the question of
what makes time move, and the reader is the right answer twice over:

* a **stalled policy still has time passing**, the last command keeps being applied, and
  `safety.watchdog.fires` stays testable in simulation — which is half the reason to have a
  simulator driver at all. Had the world advanced only on a command, a stalled policy would freeze
  the universe;
* it is what a real robot does. `read_state` returns a stamp that has moved whether or not anybody
  commanded anything.

One consequence, stated so nobody derives it from a surprising number: a command written at cycle
*N* is integrated during cycle *N+1*, so **`command_latency_ms` is exactly one control period**. It
is the one descriptor field a simulator fills in with more confidence than any hardware vendor, and
it is filled in by construction rather than by measurement.

**In a simulator the model is the body**, which sharpens the envelope rule rather than softening it.
Per-joint ranges read out of the scene are not design values that might be wrong about some
hardware — they are exactly what this body does, and the descriptor says so. `envelope` is still
null, and simulation is the strongest reason for it yet: a safety envelope bounds what a machine may
do to the people near it, there is no machine and there are no people, and a policy validated against
an envelope minted from a scene file would carry that validation to hardware — the exact transfer
§70 forbids when it says no simulated run may be presented as bench evidence.

**It assigns no roles at all.** Not "suggests them behind a flag" — none, unless the configuration
hands it a mapping. Role suggestion already exists, once, in `skynet descriptor from-urdf`, with the
human-accept step §63.4 requires, and a second implementation of "which joint is this, really" is a
second answer to one question. `skynet driver check` reports `embodiment.roles.coverage` at 0% for a
MuJoCo body, and that is the designed outcome; the README names the two ways to fix it.

**Cameras are declared only if one actually rendered.** The rule `skynet driver check` produced when
it caught the same bug in the LeRobot bridge a session ago: *a declared stream that cannot be read
binds a policy's observation to nothing*. On a machine with no GL the descriptor declares no sensors
and `read_sensor` reports `ENOSYS`, with the reason on stderr.

Four things the RHAL cannot carry are named on stderr and left out of `joints[]` rather than dropped
quietly — a tendon actuator, an actuator with internal state, a ball joint, and (for the frame graph
only) a body carrying two joints. Each is a real thing a modeller writes and each would otherwise
become a joint index whose commands do not mean what the mode says they mean.

### Two bugs found by building it, both worth the record

1. **A diverged MuJoCo resets itself.** A driver checking only for non-finite values sees a robot
   that teleported to its home pose at full speed and reports it as a perfectly good state — with no
   error anywhere and a policy above it reacting to a body that jumped. Divergence is detected from
   the warning counters instead, and reported as `SKR_EPROTO`. A full contact or constraint buffer
   is a different fact and sets `FLAG_DEGRADED`: contacts were dropped, so collisions in that run
   are not reliable.
2. **A failed GL probe cannot be retried in one process.** The first attempt with no context raises,
   which is fine; the *second* aborts the interpreter from inside the native library, with no
   exception to catch. A supervisor restarting a body on a headless machine would take the whole
   process down. The answer is that whether this machine can render is a property of the process,
   decided once and remembered.

And one gap in the repository: **`drivers/python-host/Makefile` did not exist**, so CI's step
"A Python driver loads through the host and reaches guest tier" could never have run. Written, and
it now drives the MuJoCo driver too. A check that quietly does not run is the failure mode §15
spends a whole rule on, and this one was in our own CI.

### `frames` and `simulator`, pinned — and made checkable

Two sessions ago the URDF importer emitted a `frames` block whose field names were the tool's rather
than the specification's, and the finding was recorded as the thing most likely to bite a second
edition. The MuJoCo driver is that second edition, so this session closed it — mechanically rather
than in prose.

**§44.3 now pins the shape field by field**, with a rule that turned out to matter: *a node carries
at most one joint.* URDF cannot express anything else, MJCF can, and a body with two joints on one
link therefore emits `frames: null` rather than a tree with a hinge missing from it — §44.5 already
says what that means. Whether a node should instead carry an ordered list is open and named as open.

One rename while nothing consumes the field: `frames.source` became `frames.generator`, because
§44.3 gives `source` to the per-node descriptor-versus-calibration distinction, and one name meaning
two things in one object is how a consumer reads the wrong one.

**§35.3 gained the field name for the simulator declaration** and says where the cap is enforced.
`awardCertification` reads `simulator` off the *descriptor* rather than off the driver's declared
ceiling, because a simulator driver written in C++ has no language ceiling to fall back on — the
Python one being capped twice is a coincidence of the first two drivers we happen to have. The cap
runs after the evidence walk, so a vendor reads "the evidence supports host; the body is simulated,
so guest" rather than a silent guest.

Both are now **checkable**: `validateFrames` in the oracle and `validate_frames` in Rust, with 11
corpus cases holding them to each other, plus 5 for the certification cap. All **270 corpus cases
reproduce byte for byte**, and the sixteen new ones did so on the first run — the two
implementations agreed without adjustment, which is the corpus working in the pleasant direction for
once. `skynet driver check` validates the graph and reports a simulated body's cap where a vendor
will see it.

### `crates/skynetd` — the supervisor's brain (§33)

The largest remaining component, built as far as it can be built without lying. Everything in it is
pure: `plan` is a function of the desired state and the observed state, supervision is a state
machine that takes a clock reading rather than reading one, health is an aggregation, leases are a
table with one pre-emption rule. **The I/O half is not written** — the systemd launcher, the RAUC
slot reader, the package store, the Unix socket — and is named as absent rather than stubbed.

That split is not tidiness. §33.3 claims the reconciler converges from any starting state and is
idempotent, and those are properties you can only assert about a *function*; about a program that
also talks to systemd they are a hope. So the suite carries a model of the machine, applies plans to
it and observes it again — including from a starting state nobody would arrange on purpose:
half-installed, half-running, an image staged for a version nobody wants, a package the desired state
has never heard of, and a lease held by a package that does not exist. It converges.

**§33.1's property is now mechanical rather than remembered.** *skynetd is not safety-critical
because it never touches an actuator*, and everything else in §33 — the reconciler, the network
exposure, the JSON parser, the allocation — is licensed by that one sentence. So the crate does not
depend on `skynet-rhal` and CI asserts the edge is absent, because acquiring the ability to write a
joint would begin with exactly that dependency and nobody would notice.

Three things the writing settled rather than transcribed:

1. **The plan does not take the safe state.** §33.3's sketch puts the check inside the loop; writing
   it that way produced an unused parameter, and the reason turned out to be a property rather than
   an oversight. What an action *requires* is a fact about the action; whether the robot is there is
   a fact about now. Separating them means reaching a safe state needs no re-plan, and two operators
   reading the same plan are reading the same plan.
2. **Dropping a ceiling from a profile is a widening.** §33.4's asymmetry reads naturally as
   "smaller number, apply now", and the case that matters is the edit that *deletes* a field. That
   removes a limit, and treating it as the harmless direction would apply it under a running policy.
3. **Removing a running package takes two passes, and that is correct.** The removal is emitted only
   once the stop has been observed. A reconciler that did both in one pass would be acting on what
   it intends to be true rather than on what it observed — the discipline §33.3 forbids.

The rest is the specification made executable: the six-stage ordering and the safe-state barrier, a
deferred action that stays in the plan carrying its reason, exponential backoff whose jitter only
ever *reduces* the delay (jitter that adds can exceed its own cap, and a cap that is sometimes
exceeded is not a cap), a crash window that forgets by time rather than by a successful start, a
driver that is never restarted automatically, liveness told from readiness, and exclusive leases
with exactly one pre-emption and no priority number — because a numeric priority invites a second
package to be given a slightly higher one.

### Verification

* **268 Rust tests**, up from 227. `cargo fmt --all --check` clean, `cargo clippy --workspace
  --all-targets` clean at `-D warnings`.
* **270 corpus cases reproduce byte for byte**, up from 254.
* **`drivers/mujoco`: 103 checks against real physics** with `MUJOCO_GL=osmesa`; 96 with no OpenGL,
  one skipped, which is the branch a container takes.
* **The whole chain, on Linux**: `skynet driver check` → the C host → a forked interpreter → the
  MuJoCo bridge → a six-joint arm with a wrist camera under physics. 23 pass, 1 fail (roles at 0%,
  as designed), 1 not measured, guest tier — with the simulated cap named in the report.
* Unchanged and re-run: the mock's 24 checks, the LeRobot bridge's 70, the scaffold's
  build-load-and-reach-guest-tier test, and the LeRobot chain through the host.
* `Cargo.lock` holds nine packages, all ours. The no-dependency property is intact.

### What was changed in the Hub

The oracle is mirrored, so the frame graph and the simulator cap were made in both copies:
`lib/skynet/os/embodiment.ts` and `lib/skynet/os/conformance.ts`, plus the specification. The Hub's
Skynet OS suites went from 80 tests to 90 and pass; `npm run typecheck` is clean.

This repository's copies of the other two mirrored documents were **stale** — the Hub had added a
completion roadmap to both since the last session — and were synced from the Hub, which is the
origin. `node tools/check-mirrors.mjs` now reports all three identical.

### What is still zero lines

The I/O half of `skynetd`: the systemd launcher, cgroup enforcement (§40), the RAUC slot reader, the
package store and installer, the outbound queue, the Unix socket and the LAN listener. And,
unchanged: the policy runtime, the SPSC ring and observation assembly, OTA, the black box, the
on-robot console, the frame graph *consumer* (two emitters and a validator now exist; nothing
computes FK from it), calibration, the watchdog chain, POST and homing, the safety MCU, and the
drivers that talk to real hardware — `ros2_control`, EtherCAT, CANopen and the vendor SDKs.

§59.5's several-engineer-years figure is unchanged.

### Three findings recorded rather than fixed

1. **§52 names five SCTS ids that do not exist**: `embodiment.frames.complete`,
   `safety.command_loss.behaviour`, `safety.homing.envelope`, `power.safe_state.reachable` and
   `rhal.sensor.latency`. This session's MuJoCo harness named one of them by accident and it read
   exactly like naming a real one, so the harness now checks its ids against `SCTS_TESTS` and fails
   if it invents one. The ids themselves should land as **one deliberate SCTS expansion** with the
   corpus regenerated and reviewed, because adding a required test changes every certification
   verdict that carries no result for it — which is correct, and is a change worth making on
   purpose rather than as a side effect.
2. **The frame graph has two emitters, a validator and no consumer.** Nothing computes forward
   kinematics from it yet, so `workspace` and `self_collision` still report unavailable. The shape
   is now pinned and tested, which was the point — but a pinned shape nobody reads is still a shape
   nobody has been forced to get right.
3. **`skynet driver check` opens a driver with an empty configuration**, which is why the MuJoCo
   driver needs `SKYNET_MUJOCO_SCENE`. That is a workable seam and it is also the reason a body's
   roles cannot be supplied to the check. Worth revisiting when the command grows a way to pass one.

### Next

1. **The I/O half of `skynetd`**, and the local API with it (§33.7) — at which point `skynet ps`,
   `logs`, `install`, `grants` and `estop` stop being commands that name themselves absent. The
   brain they drive is written and tested; what is missing is the systemd launcher, the package
   store and the socket.
2. **The Hub side** (§32.4): `/skynet/os` readiness over the live registry, the transport endpoint,
   the embodiment tables. Needs no new tables for step one and answers a question the ecosystem has.
3. **The §52 SCTS expansion**, as one reviewed change with the corpus regenerated.
4. **A frame graph consumer**: forward kinematics in the kernel, so `workspace` and `self_collision`
   stop reporting unavailable on a body that declares a tree.
5. **Run the LeRobot bridge against the real library**, and against an SO-101 if one can be found.
   Everything it says about itself is provisional until then.

## 2026-08-29 (fifth) — the I/O half, and a daemon that runs on a real machine

Item 1 from the previous session's list. `skynetd`'s brain was written and tested against a model;
what was missing was everything that touches the machine — the systemd launcher, the package store,
the socket — and the eight commands that named themselves absent because none of it existed.

It runs. The end of this entry is a robot's package being staged, verified, activated through a
symlink, launched as a transient systemd unit under the cgroup limits its manifest declared, and
driven over a Unix socket by `skynet ps`. Not on a model of a machine: on one, with `memory.max`
read back out of `/sys/fs/cgroup`.

### `crates/skynetd-io` — the half that can be wrong about the world

A second crate rather than more modules in `skynetd`, and the reason is the same one that keeps the
daemon away from `skynet-rhal`. §33.3 claims the reconciler converges from any starting state and is
idempotent, and those are properties you can assert about a *function*; about a program that also
talks to systemd they are a hope. Keeping the brain pure is therefore worth a crate boundary, and
now worth a CI check too: **`crates/skynetd/src` may not name `std::fs`, `std::net`, `std::process`,
`std::thread`, `Instant::now` or `std::os`**, and the RHAL invariant covers both halves rather than
one, because §33.1's property is about the daemon and "it never touches an actuator" stops being
true the moment either crate can.

Eleven modules, 4,700 lines, 73 tests. The parts worth naming:

**The store (§37) keeps its rollback set in the links themselves.** §37.2 pins "the running version
and the previous N" and §37.3 says collection never touches them, so something has to know what
"previous" means. It could have been a history file; it is `prev.1` and `prev.2` beside `current`,
symlinks like any other. Collection is then "remove a blob nothing points at" and *cannot* reach the
rollback set — not because a flag says so but because there is no path through the function that
looks at anything else. The links live in a directory named for the package rather than beside it as
`<name>.prev.1`, because a package legitimately called `grasp.prev.1` would otherwise have its blobs
deleted when `grasp` is upgraded.

**Activation is `symlink` then `rename`.** §37.1's guarantee — no window in which the robot has half
a policy — is exactly what `rename` gives and exactly what unlink-then-symlink does not: between
those two calls the package has no current version at all.

**The launcher writes `Restart=no` into every unit**, which looks like throwing the feature away and
is the point. §33.5's policy is per package *kind* and counts failures in a window, and both live in
`skynetd::Supervision`, which is a pure state machine with a suite. Two supervisors with two
opinions about when to restart is one supervisor too many.

**`AllowedCPUs=` is refused rather than omitted when every core is isolated.** An omitted cpuset is
*every* core including the one the safety kernel is on, so the failure mode of a misconfigured
isolation list would be precisely the thing §40.2 says is not a tunable.

**The HTTP is hand-written and refuses more than it accepts.** Chunked encoding is answered `411`, a
duplicated header is `400` — last-wins is how a second `Authorization` overrides the first — and a
path with a `..` in it never reaches routing even though nothing today maps a path onto the
filesystem, because that is the kind of thing a later handler adds without re-reading the parser.

**§39.1's table is data, and the suite asserts it has exactly two open entries.** `GET /status` and
`POST /stop`, and an unrecognised route needs *admin* rather than nothing, so a route added without
a table entry fails closed.

### `version_digest` — the check that could not be performed

`admitPackage` has always taken `observed_digest` as an argument: the digest the bytes on disk
actually hash to. Nothing computed it. A verification step whose input comes from whoever is calling
is decoration, and the store cannot verify a package without one — so this session added the
function, in Rust, in the reference TypeScript, and in the Hub, with 19 corpus cases holding the
first two together.

It is the Hub's `computeVersionDigest` rather than a new rule, because a digest is only useful if
two parties compute the same one. Two things follow.

**A refusal where the two could disagree.** The Hub canonicalises with `JSON.stringify`, which
reaches for an exponent at 10²¹ and below 10⁻⁶ where §31.4's rule forbids one; and it orders object
keys by UTF-16 code unit where the canonical rule orders by UTF-8 byte, which differ only above
U+FFFF. Rather than hope no manifest ever lands there, both implementations **refuse** — a package
that does not install and a person reading a reason is a better outcome than one whose identity is
one thing on the robot and another in the registry.

**The separator is a NUL, and finding that out was the whole exercise.** The first implementation
used a space, because `lib/skynet/digest.ts` holds a raw `\0` byte in a template literal and a
terminal draws it as nothing at all. Every case with no files matched the Hub; every case with files
did not. It is now written `\u0000` in both copies, and `lib/__tests__/skynet-os-digest.test.ts`
compares the OS rule against the Hub's production function on eight inputs so the next divergence is
found by a test rather than by a fleet deciding its packages have been tampered with.

### Four bugs the machine found that the suite did not

The suite was written first and passed. Then the daemon ran, and each of these came out of running
it — which is the argument for `e2e.sh` existing at all.

1. **A staged package's entry point was not executable.** systemd reported `203/EXEC`, which is
   `EACCES`. `fs::write` makes a file `0644` and a policy has to be run. The fix is not to preserve
   the source's mode: a mode is not part of the digest, so a mode-preserving unpack would let two
   robots holding one digest disagree about which files may be executed. It is derived from
   `runtime.entrypoint` — which *is* digested — so exactly one file is executable and the manifest
   says which. A package needing a second executable cannot express that today; recorded as a limit
   rather than papered over.
2. **A running policy could never be upgraded.** The activation waits for the package to stop, and
   nothing in the plan would stop it — the desired state wants it *running*, so stage 5 had no
   reason to. The plan deferred for ever while reporting itself as merely waiting. `plan` now emits
   the stop, §33.4 is amended to say so, and the pure suite gained the case. A widened *grant* is
   the opposite and correctly waits without forcing anything, because §33.4 says a widening "waits
   for a restart" rather than causing one.
3. **A pass could report convergence before the start it had just enabled.** `Pass::converged` asked
   whether every action succeeded. §33.3 computes one plan from one observation, so activating a
   package does not put the start in the same pass — and the daemon stamped generation 42 on a robot
   that was not running anything. The predicate is now "a plan taken from a fresh observation is
   empty", which is one condition instead of three and cannot be made to answer yes by an outcome
   nobody enumerated. §33.3 says so now.
4. **The CPU topology was read through the layout root.** A `Layout` roots `/data` and `/run` so the
   store can be exercised against a temporary directory; `/sys` is not part of that. A daemon under
   a test root found no CPUs and refused to launch anything — a plausible shape for a real failure
   on a robot whose `/data` is a separate mount.

And two gaps that were not bugs so much as absences: nothing could declare the robot at a safe state,
so no policy could ever start; and nothing could issue a credential, so no authenticated route was
reachable at all.

The first is `/data/state/safe-state`, read every pass, and **everything that reports it says
`declared`**. Whether a robot is at a safe state is a fact about the body, the body is the kernel's,
and §33.1 keeps this daemon away from it — so §33.4's barrier is standing on an operator's word and
the console says so rather than letting it read as measured. Anything but an exact `yes` is `no`.

The second is `skynetd credential add`, which is §39.2's bootstrap in the form a machine with no
chassis button has: you are on the robot and you can write `/data/state`, and that is the presence.
The token comes from `/dev/urandom` or the command **fails** — a robot that cannot generate a
credential must not issue a weak one — and it is printed once, with only a salted hash stored.

### The commands that named themselves absent

`skynet status · ps · plan · logs · grants · estop · rollback · install` are real, over
`/run/skynetd.sock` or `--host`. `pair` and `blackbox` are still absent and still named as absent:
pairing needs §10's attestation and the black box needs §13's recorder.

`skynet install` stages locally rather than over the API, which is the honest shape while §34's
fetcher does not exist — there is no way to hand a robot bytes over the network, so the bytes are
already there and the reconciler finds the blob present on its next pass. That is the same
"skipping any blob already present" path §37.1 describes, reached from the other direction.

`skynet logs` reads journald, and `GET /logs` says so: §41.1 spends a section on the black box *not*
being a log file, and a console offering one under the other's name would send somebody looking for
a command trace in a stack trace.

### Two checks in our own CI that were not running

Both are the failure §15 spends a rule on, found in the place least likely to be looked at.

1. **The no-third-party-dependency check had been failing since `skynetd` landed.** It grepped
   `Cargo.lock` for names not matching `^name = "skynet-`, and `skynetd` does not begin with
   `skynet-`. It now compares the lock against the workspace member list read from `Cargo.toml`,
   which also catches a member missing from the lock and needs no maintenance.
2. **`tools/check-mirrors.mjs` reported two identical documents as diverged, permanently.** The
   working tree had CRLF in 43 files that came from a checkout and LF in everything written since,
   because `core.autocrlf=true`. A repository built around byte-identical artifacts should not have
   that, so there is now a `.gitattributes` pinning `eol=lf`, and the two mirrored documents were
   normalised so the check is green. The other 41 files keep their endings until somebody runs
   `git add --renormalize .`, which is a whitespace-only change best made on its own.

### Verification

* **348 Rust tests**, up from 268. `cargo fmt --all --check` clean, `cargo clippy --workspace
  --all-targets` clean at `-D warnings`.
* **289 corpus cases reproduce byte for byte**, up from 270. The nineteen new digest cases agreed
  between Rust and the reference oracle on the first run.
* **The Hub's Skynet OS suites: 112 tests**, up from 90. `npx tsc --noEmit` clean.
* **`crates/skynetd-io/e2e.sh`, twice.** In a container with no systemd: 24 checks, and the launch
  step correctly reported as not run. On a machine with a real systemd manager, with
  `SKYNET_REQUIRE_SYSTEMD=1` so a silent skip fails the run: **all checks passed, including the
  package actually starting.**
* **The cgroup limits, read back from the kernel.** A manifest declaring `ram_mb: 64` and
  `cpu_cores: 0.5` produced `memory.max 67108864`, `cpu.max 50000 100000`, `pids.max 128` and
  `Restart=no` in `/sys/fs/cgroup`. §40.1 demonstrated rather than asserted.
* Unchanged and re-run: the mock's 24 checks, the LeRobot bridge's 70, the LeRobot chain through the
  C host, and the scaffold's build-load-and-reach-guest-tier test.
* `Cargo.lock` holds ten packages, all ours. The no-dependency property is intact.

### What was changed in the Hub

`lib/skynet/os/digest.ts` and `lib/skynet/os/canonical.ts` are new and are mirrors of the reference
oracle; `lib/__tests__/skynet-os-digest.test.ts` holds the OS digest rule against the Hub's
production `computeVersionDigest`. The raw NUL in `lib/skynet/digest.ts` became `\u0000`, which
changes no bytes and makes the algorithm readable. The specification was amended in both copies and
`node tools/check-mirrors.mjs` reports all three documents identical.

### What is still zero lines

The policy runtime, the SPSC ring and observation assembly, OTA (§12), the outbound queue (§34.4),
the black box (§13, §26), the on-robot console's HTML (§32.2 — the API behind it exists), the frame
graph *consumer*, calibration, the watchdog chain, POST and homing, the safety MCU, Ed25519 (so
every package this edition stages reports `signature: not_checked` and carries admission's `unsigned`
finding), and the drivers that talk to real hardware.

The reconciler's `apply` reports `StageOsImage` and `ApplyOsImage` as **unimplemented** and leaves
them in the plan, and the generation is not advanced past them. A supervisor that dropped them would
converge on a lie.

§59.5's several-engineer-years figure is unchanged.

### Three findings recorded rather than fixed

1. **`admit_package` is not called on the install path.** The store verifies the digest and stages;
   nothing runs admission or `resolve_transport` against the staged manifest, which §37.1 puts
   between `verify` and `stage`. Both functions exist and are corpus-tested; what is missing is the
   robot's `DeviceCapacity` — its RAM, its capability tokens, its embodiment descriptor — and
   inventing one to get a call site would be inventing exactly the numbers admission exists to
   check. It should land with the descriptor, as one change.
2. **A package gets one executable, and the manifest names it.** Enough for a policy behind a
   loader; not enough for a driver shipping a helper binary. The alternative — preserving the
   source's modes — takes the reproducibility with it, so this needs a manifest field rather than a
   relaxation.
3. **The safe state is an operator's declaration in a file.** It is labelled as one everywhere it is
   reported, which is the most that can be done without a kernel channel — and it is the single
   largest piece of standing-on-a-promise in the daemon. It should become a read of the kernel's
   state the moment there is one to read.

### Next

1. **The black box (§13, §26)**, which is the largest remaining component and the one every incident
   path depends on. The preallocated ring, the fixed schema, sealing on a fault, and the replay that
   makes `blackbox.replay.deterministic` mean something.
2. **The on-robot console (§32.2)** — the API is written and tested; what is missing is the small
   static page in front of it, with the stop control that is never below the fold.
3. **The Hub side** (§32.4): `/skynet/os` readiness over the live registry, the transport endpoint,
   the embodiment tables. Needs no new tables for step one.
4. **The §52 SCTS expansion**, as one reviewed change with the corpus regenerated. Carried from the
   previous session's list and still worth doing on purpose rather than as a side effect.
5. **A frame graph consumer**: forward kinematics in the kernel, so `workspace` and `self_collision`
   stop reporting unavailable on a body that declares a tree.

---

## 2026-08-29 (sixth) — the black box

The largest remaining component and the one every incident path depends on. §13 gives it eight
words a line; §26 gives it a format and six channels; §15 asks two conformance tests of it —
`telemetry.blackbox.complete` and `telemetry.replay.deterministic` — and neither could be answered
by anything that existed this morning. It is now a crate, a corpus, a daemon subsystem, three CLI
subcommands and three routes, and the two tests have something to be true of.

### The decision the whole design rests on: the ring is not an MCAP file

It was tried the other way first, which is the only reason it is worth a paragraph. MCAP is a linear
container — magic, header, records, a data-end, a summary computed over everything before it, a
footer — and a ring overwrites its oldest bytes, so the file's own framing is the first casualty of
the second hour. A black box whose format breaks precisely when it wraps works only during the
incidents short enough not to need it.

So the roles are split. **The ring is the recording**: fixed-size records, one slot each, a sequence
number that only increases, no structure a wrap can invalidate. **MCAP is the artifact**: produced
once, at seal, from whatever the ring holds at that instant. §26 chose MCAP so an incident opens in
Foxglove with no exporter, and that is a property of the file handed to an investigator rather than
of the bytes being written a thousand times a second. §26.1 says so now.

### `crates/skynet-blackbox` — 4,400 lines, 29 tests, `no_std`

`no_std` not because the recorder runs on the safety MCU — it does not, `skynetd` owns it (§33.2) —
but because a crate that cannot reach `std` cannot quietly acquire a file handle, a clock or a
thread in its hot path. `Ring::append` writes into a caller-provided `&mut [u8]`: a memcpy and some
integer arithmetic, no allocation, no clock read. Sealing allocates freely, because sealing happens
after the incident and no control deadline is waiting on it.

The parts worth naming:

**`desired` is `f32` and `command` is `f64`, and the asymmetry is what makes replay checkable.**
`desired` is what the policy produced and it reached the kernel through the RHAL, whose values are
`f32` (§24) — recording it as `f32` records exactly what happened, and widening it back is lossless.
`command` is what the kernel decided, and the kernel computes in `f64`: a clamp emits
`signum(v) * ceiling`, which in general has no `f32` that represents it. Recording *that* as `f32`
would round the verdict, and a replay comparing a recomputed `f64` against a rounded `f32` would
report a divergence on every clamped cycle — an alarm that fires on correct behaviour, which is the
fastest way to make a conformance test ignored.

**The policy digest is a `u16` index into a table in the ring header.** §13's line says "policy
digest", which is 32 bytes identical for hours at a time; at 1 kHz that is 115 MB an hour of one
repeated constant. It is in the *header* rather than in a preceding event record because an event
record is overwritten on the next wrap, and the recording would then know it had a policy without
knowing which.

**Every record carries a CRC.** Power fails mid-write. Without a check the next reader deserialises
whatever the drive happened to persist, and a half-written cycle is not obviously wrong: the numbers
are plausible, the timestamps are nearly right, and the incident report is confidently false.

**Sealing never refuses.** §41.1's guarantee is "complete, or the run is not conformant", and the
temptation is for `seal` to refuse an incomplete window. It does not: the recording most likely to
have a hole in it is the one covering the fault that made the hole, and refusing to produce it would
destroy the only evidence at the moment it is worth something. The truth travels with the file
instead — `Continuity` reports records, sequence range, holes, damaged slots and dropped records,
and the same facts are in the file's own metadata so a recording separated from this API still says
what it is. CI asserts the refusal has not been added.

**A gap is not a hole and neither is a wrap.** Three distinct facts, and collapsing any two would
mislead. *Overwritten* is the window moving on — normal, and not a defect. *A hole* is a sequence
number missing inside the window — a defect. *A gap* is §47.4's dropped frames, recorded by the
producer as a `Gap` record that consumes a sequence number of its own, so the numbering stays dense
and a reader can tell "the ring wrapped past these" from "these were never written".

### Two fields §13 did not have, and the reason they had to be added

The record now carries the **capability token** the command exercised and the **distance to the
nearest person**. Neither is derivable from anything else in the line, and without them the mode gate
and the proximity guard — two of the eight — cannot be recomputed. A recording that cannot reproduce
its own verdicts is a recording that reports a divergence on correct behaviour, so the alternative
was a replay that quietly excluded two guards from the property it claims to check.

The capability is an index into a second header table, for the same reason the policy digest is: it
is fixed for the life of a lease, and a value written into a record does not survive the wrap.

§13 is amended in both copies of the specification, and `node tools/check-mirrors.mjs` reports all
three shared documents identical.

### "Roughly an hour" turned out to be a number nobody had computed

§13 says the ring is "sized for roughly an hour". An hour is not a size, and the conversion is the
part worth writing down: a 30-joint humanoid with 30 action channels has a 936-byte record, so an
hour at a 1 kHz joint rate is **3.4 GB** — most of a small disk, against §28's storage budget — and
a 512 MB ring holds about ten minutes of it.

So the ring is sized by a **byte budget** and what that buys is computed: `Geometry::seconds_covered`
answers "how long a window is this, at this rate", and `budget_for` inverts it. An operator who asked
for an hour and got ten minutes is told ten minutes. A constant named `ONE_HOUR` in front of a ring
that holds a fraction of one is exactly the invented number this specification exists to prevent, and
§13 now says so rather than implying otherwise.

### Replay, and what it actually claims

Not "the policy would produce the same output" — that is a claim about a neural network on a
particular accelerator, and this crate cannot make it. The policy's output is *recorded*. What
`replay` verifies is the half of the cycle that is deterministic by construction: **given the
recorded desired values, the recorded timing and the same resolved limits, does this edition's
Safety Kernel reach the recorded verdict?**

Narrower, and much stronger, because it is checkable. It fails when the kernel changed behaviour
between the recording and the replay — an OS upgrade that quietly altered a clamp, the regression
nobody catches in the field — when two editions disagree, or when the limits handed to the replay
are not the ones that were in force, which is itself worth knowing.

Three details that took getting right:

* **`previous` is threaded.** The rate-limit guard compares against what was actually sent last
  cycle, so replay walks the window in order carrying the previous *command* forward, exactly as the
  control loop did. The recorded command, not the recomputed one: carrying the recomputed value
  forward would compare cycle two against a world the robot never entered, and one divergence would
  cascade into every cycle after it.
* **The first cycle has no predecessor**, so `Replay::warmed` reports how many cycles were compared
  soundly rather than pretending the first one was.
* **A skipped cycle is not an agreement.** `deterministic()` is false when anything was skipped, and
  false for an empty window — vacuous truth is the failure mode of every conformance predicate, and
  the same instinct that makes `field_conformance` return `undefined` for zero cycles.

### The corpus grew by seventeen, and caught a real divergence on the first run

Four new corpora: `schemas` (the six MCAP channel schemas, byte for byte — §31.2 puts them in the
short list of artifacts that stay single-source however many editions exist), `records` (eight, the
ring's record layout), `header` (two, including both tables) and `seal` (one whole file).

Two decisions made this cheap rather than expensive:

**The schemas are built and serialised through canonical JSON rather than written as literals.**
Canonical JSON is already dual-implemented, already corpus-tested, and already required to be
byte-identical across editions. Two editions constructing the same document emit the same bytes
because the form leaves them no choice, rather than because two people kept two literals in step.
The same reasoning makes MCAP message bodies canonical JSON: §26 asks for embedded schemas so the
file stays readable without the code that wrote it, and a JSON Schema with a JSON body meets that
literally where CDR would need an IDL and a generator to be anything but bytes.

**Every 64-bit field in a corpus input is a decimal string.** A wall-clock epoch in nanoseconds is
about 1.7e18, past 2^53. A corpus whose inputs went through a JSON number would be comparing two
editions on a value neither of them received intact. For the same reason the TypeScript oracle uses
`bigint` throughout.

`reference/typescript/blackbox.ts` is the second implementation — 753 lines, and the first file in
`reference/` that is *not* a copy of a Hub module, because the Hub does not write black boxes. The
README there says so rather than leaving a reader to assume it is vendored.

**Fifteen of the seventeen agreed on the first run.** The sixteenth and seventeenth were the seal,
and the two editions disagreed on **record order**: the Rust wrote schema, channel, schema, channel
in the data section, and the oracle wrote every schema then every channel. Both are legal MCAP — the
format only asks that a schema precede the channel referring to it — which is exactly the kind of
divergence that would never be noticed until two tools disagreed about a file. The Rust now groups
them, matching the summary section's own grouping.

Finding it took one minute, and the reason is worth recording: the case's expected value was the
file's SHA-256, which says *that* two editions disagree and not *where*. Temporarily emitting the
full hex let the runner point at byte 1566. The case now pins **the sequence of record opcodes**
beside the digest permanently — the digest holds the bytes, the opcode list says where, and eighty
kilobytes of hex in a file a human is supposed to review holds neither better.

### The daemon records, seals and serves it

`crates/skynetd-io/src/blackbox.rs` is the half that opens files. It preallocates the ring at
`/data/blackbox/ring` by **writing its whole length, zeroes included** — a sparse file would report
the same size and fail at the first write after the disk filled, which is precisely the failure §28
spends a paragraph on. One positioned write per record; sealing into `/data/blackbox/sealed/`, named
by the monotonic clock rather than the wall clock so that a robot whose clock was set between two
incidents does not produce two files whose names imply the wrong order.

Three routes, with the roles §39.1 gives them: `GET /blackbox` (observer — "the black box index"),
`POST /blackbox/seal` and `GET /blackbox/{name}` (operator — "seal and download the black box"). The
download name is validated rather than joined, because a route that joins a caller's path segment
onto a directory is how a download becomes a way to read `/etc/shadow`; the CLI checks the same rule
on its side so a mistyped name fails at the keyboard.

`skynet blackbox`, `blackbox seal` and `blackbox get` are real. `blackbox` was one of the two
commands the CLI named as absent; `pair` still is, and still needs §10's attestation path.

Adding a binary route meant the CLI's transport had to stop assuming JSON: the response body is now
split on the raw bytes rather than on a lossy string, because decoding an MCAP file as UTF-8 replaces
every invalid sequence with U+FFFD — which is to say, hands back a corrupted recording that looks
like a recording. A download refuses to overwrite an existing file: a recording is evidence.

### The honest part, stated everywhere it could be missed

**This robot records events and nothing else.** §26 gives the black box six channels; four of them
carry control cycles, and there is no control loop — the policy runtime, the SPSC ring and
observation assembly (§25) are unwritten. So the ring `skynetd` opens has a shape of zero joints and
zero channels, which is the honest description of a supervisor that is managing packages and not
commanding a body.

It is said in the module's own documentation, in `GET /blackbox`'s response as a
`cycles_unavailable` sentence, in `skynet blackbox`'s output, in the README, and in a test that
asserts the sentence is there. An incident report that looked complete because nobody had noticed the
cycles were missing would be worse than no incident report.

The ring is **recreated** when the shape changes — a record's size is a function of the body — and
what it held is sealed first, so turning on a control loop later costs the previous window a file
rather than costing it existence.

### A portability defect, found by trying to build

The workspace did not compile on Windows, and had not since `skynetd-io` landed. The crate is careful
about `cfg(unix)`; its *binary* was not, and reached for `serve` and `auth::random_hex`
unconditionally. Fixed properly rather than by a feature flag: `skynetd once` is `plan` and `apply`
over a directory and works anywhere, so the split is around the serving loop rather than around the
whole binary, and a developer can run a pass against a fixture root on any machine. `random_hex`
gained a non-POSIX arm that **refuses** — the alternative is a token derived from a clock or a
process id, which is fine on a workstation and catastrophic on a fleet.

This matters more than a convenience: "a vendor building a driver in 2036 should be able to
`cargo build` this with no network" is a property this workspace claims, and it was false for every
vendor on Windows.

### Verification

* **363 Rust tests pass on this machine**, up from 324. The 24 POSIX-gated tests in `skynetd-io` did
  not run here and are not counted; on Linux the total is 387.
* `cargo fmt --all --check` clean. `cargo clippy --workspace --all-targets` clean at `-D warnings`,
  with one `#[allow]` that carries its reason: `Body`'s large variant is the cycle, and boxing it
  would put an allocation on the common path to save bytes on the rare one.
* **306 corpus cases reproduce byte for byte**, up from 289.
* `node tools/check-mirrors.mjs`: all three shared documents identical.
* `Cargo.lock` holds eleven packages, all ours. The no-dependency property is intact — the MCAP
  writer and the CRC-32 are ours for that reason.
* **Not verified here: `crates/skynetd-io/e2e.sh`.** Ten checks were added to it — the ring is
  preallocated and is a real file rather than a hole, the index reports its window and says the cycle
  channels are empty, sealing names a file that exists and begins with the MCAP magic, the download
  arrives byte for byte, and a path in a name is refused. The script parses (`sh -n`) and the pieces
  it drives are unit-tested, but it needs a POSIX daemon and has only run in CI's hands.

### Four checks added to CI

1. `skynet-blackbox` joins the crates forbidden to depend on `skynet-rhal`. `skynetd-io` depends on
   it now, and a transitive edge reaches an actuator exactly as well as a direct one — and the black
   box records `rhal_state`, so that crate is the obvious dependency to take.
2. The same check stopped punishing prose. It grepped for the identifier anywhere under `src/`, which
   would have failed on a comment explaining why the edge is absent — the CI file already says a
   check that trips on the written reason is how the reason stops being written. It now ignores
   comment lines.
3. `seal()` may not grow a path that refuses an incomplete window.
4. The event vocabulary stays a closed enum, which is what keeps ordinary logging out of the black
   box (§41.1).

`skynet-blackbox` also joins the crates that must keep `forbid(unsafe_code)`.

### What is still zero lines

The policy runtime, the SPSC ring and observation assembly, OTA (§12), the outbound queue (§34.4),
the on-robot console's HTML (§32.2 — the API behind it exists), the frame graph *consumer*,
calibration, the watchdog chain, POST and homing, the safety MCU, Ed25519, and the drivers that talk
to real hardware.

The black box has moved out of this list and into a partial one: the format, the ring, the seal and
the replay exist and are held to the corpus; the four cycle channels wait on a control loop, and
every surface that reports the recording says so.

§59.5's several-engineer-years figure is unchanged.

### Three findings recorded rather than fixed

1. **The recording carries no embodiment digest and no OS image digest.** Both are zero, because this
   daemon has no descriptor and no attested image — the same absence that keeps `admit_package` off
   the install path. It is not a placeholder to tidy: a recording carrying invented digests could be
   replayed against the wrong body without anybody noticing, and §56.4's whole argument is that a
   replayable recording settles a dispute. It should land with the descriptor, as one change.
2. **Sealing reads the whole ring into memory.** Bounded by the ring, so it is correct rather than
   dangerous, and at the default 64 MiB it is fine. A ring sized in gigabytes would want a streaming
   reader — though the better answer is a smaller ring, since §34 already says a sealed recording is
   uploaded and a gigabyte of one is not crossing a metered bearer.
3. **A record is written with a `seek` and a `write`, not an `O_APPEND` or a `pwrite`.** Correct for
   one writer, which is what §33.2 gives it, and it would not survive two. If the control loop ever
   writes cycles from its own thread while the supervisor writes events, this needs the positioned
   write it is already shaped for — `Ring::stage` hands back the offset precisely so that the change
   is a syscall swap rather than a redesign.

### Next

1. **The on-robot console (§32.2)** — the API is now complete enough to sit behind one, including
   the black box index and the stop control that is never below the fold. What is missing is the
   small static page.
2. **The frame graph consumer**: forward kinematics in the kernel, so `workspace` and
   `self_collision` stop reporting unavailable on a body that declares a tree.
3. **The Hub side** (§32.4): `/skynet/os` readiness over the live registry, the transport endpoint,
   the embodiment tables — and now also the incident upload (§17's `POST /devices/{id}/incidents`),
   which has a file to receive for the first time.
4. **The §52 SCTS expansion**, as one reviewed change with the corpus regenerated. Carried from two
   sessions ago and still worth doing on purpose.
5. **The embodiment descriptor on the robot**, which unblocks three recorded findings at once:
   admission on the install path, the black box's real shape, and its provenance digests.

---

## 2026-08-29 (seventh) — the frame graph consumed: forward kinematics, and the two pose guards

`frames` had been in the descriptor since Part V and nothing read it. Two of the eight guards were
specified against it — `workspace` and `self_collision` — and both reported unavailable on every
robot, one of them for a reason that was wrong. This session made the kernel walk the tree.

It also closed a hole in the corpus that had been open for four sessions, and the hole turned out
to be more interesting than the feature.

### The kernel needed arithmetic, and the arithmetic had to be the specification's

Forward kinematics needs a sine and a square root. `core` has neither — they are `std`, which this
crate cannot link (§23.1) — so something had to supply them, and *what* is not a packaging question.

Three requirements point the same way and each alone would settle it. §31.4 demands byte-identical
output across editions, and ECMAScript permits `Math.sin` an implementation-defined error while
every C library rounds its own way, so two editions calling their own libraries diverge on the first
revolute joint — presenting as a specification disagreement rather than as what it is. §7.4's
certified tier mirrors this kernel onto a **separate safety processor with a different toolchain**,
where a libm difference makes the redundant channel disagree with the primary about a clamp: exactly
the event redundancy exists to detect, fired by correct behaviour, which is the fastest way to have
redundancy switched off. And the workspace has no third-party dependencies and builds for bare
metal, where there is no library to call.

So `crates/skynet-kernel/src/real.rs` writes `sin`, `cos` and `sqrt` out as an explicit sequence of
IEEE-754 binary64 operations — Cody–Waite reduction onto `[-π/4, π/4]` and fdlibm's minimax
polynomials, a fixed five-step Newton iteration for the root. Addition, subtraction, multiplication
and division *are* correctly rounded on every conforming platform, so the same sequence produces the
same bits everywhere. That is a weaker claim than "accurate" and a much stronger one than what a
platform library offers, because accuracy varies by platform and this does not.

Three details are load-bearing and each is a place an edition could quietly differ:

* **The branch count cannot depend on the input.** fdlibm takes its second Cody–Waite step only when
  the first cancelled badly; this takes it always. A specification that says "sometimes three
  multiplications and sometimes six" is one two editions can implement differently without noticing.
* **The Newton iteration count is fixed at five**, not run to a convergence test. Two editions taking
  different numbers of correctly rounded steps can land on different bits.
* **Rounding half away from zero is written out.** ECMAScript's `Math.round` sends −0.5 to −0 and
  Rust's `f64::round` sends it to −1; two editions differing there differ by a whole quadrant.

**809 values cross-checked between the two editions: zero divergences**, NaN cases included. §31.2
gains a row, because this is now a thing that can never be per-edition.

The reduction is exact for `|angle| < 1.65 × 10⁶` radians — a quarter of a million revolutions —
and past that returns NaN, which the kinematics reads as an unresolved frame and names. A wheel that
has turned 250 000 times is told rather than placed wrongly.

### `crates/skynet-kernel/src/kinematics.rs` — 907 lines, and the shape is the design

§44.3's graph names parents by string, which is right for a signed document and wrong for a control
cycle: resolving a parent by name is a string comparison against every other frame, on every frame,
on every cycle. So it is **flattened once**, at lease time, into frames carrying a parent index
strictly smaller than their own. Three things fall out, and each removes a failure mode rather than
only some work:

* Forward kinematics is one forward pass over a slice. No recursion, so no stack depth to bound on a
  safety MCU.
* **A cycle in the graph becomes unrepresentable** rather than a hang in the middle of a control
  cycle. `FrameSet::new` refuses an order without the property, once, and no cycle checks again.
* A parent's pose is always computed before its child is reached.

What it refuses is as important as what it computes. A **ball or floating joint** cannot be
parameterised by one scalar, so a frame driven by one is unresolvable and so is everything below it
— reported, never approximated by assuming the shoulder has not rolled. A **zero-length axis** is a
body-description defect and not a fixed joint: reading it as one would silently weld a limb in the
guard's model while it moved in the world. And unresolvability is a **static** property, so
`statically_resolvable` answers it when the lease is granted rather than by holding the robot on
every cycle for the life of the lease with the reason removed.

### The two guards, and the three things they cannot do

`workspace` checks every frame an operator constrained against volumes in the root frame.
`self_collision` tests every pair of collision capsules that is neither adjacent nor declared an
exception, in squared distance so that **no square root can move a verdict** — the root is taken
only for the number the report carries.

Layers compose by **conjunction**: a frame must lie inside at least one volume of every layer that
names it. That is §38.2's "each layer may only narrow" as a predicate rather than as geometry.
Intersecting a box with a sphere is a shape that is neither, and a composition rule producing a
third shape would be one no operator could check by reading their own configuration.

Three limits, now in §44.5 rather than waiting to be discovered:

1. **A volume is in the robot's own root frame**, and for a mobile base that is not the cell. §44.2
   gives the kernel a tree rooted at `base_link` and no world pose; localisation lives in the
   perception tree, which it deliberately does not subscribe to. For a fixed-base arm the
   distinction is empty. For a mobile base the "stay inside this region" volume travels with the
   robot, and fixing it needs a pose the kernel would have to trust from outside itself.
2. **The commanded configuration exists only on a position-commanded body.** §7.1 specifies both
   guards one step ahead; on a velocity- or torque-commanded body there is no next configuration to
   compute, so what runs is a check of where the robot is *now*. A real check and a weaker claim,
   and the verdict carries `lookahead` so a log cannot imply the stronger one.
3. **The cost is quadratic**, and this is the first part of the kernel where that matters. Forty
   collision bodies is 780 pairs and roughly fifty thousand operations per cycle — comfortable on an
   application processor, and *not* obviously comfortable on a safety MCU without double-precision
   hardware, where each is a software routine. `FrameSet::collision_pairs` reports the number and
   `skynet frames` prints it, so §7.4's certified tier has something to answer before mirroring this
   guard onto a second processor.

### A verdict that said "stopped" about a robot that kept moving

Found while wiring the new guards in, and it predates them. `proximity`'s §7.1 row is `stop_cat1`,
and the verdict's response was derived from the rows of whichever guards had tripped. But between
the two separation distances the proximity guard does not stop anything — it scales the command and
lets it through. So every speed-reduction cycle produced a verdict reading `stop_cat1` while the
robot moved on at half speed, and a black box recording that is a black box recording something that
did not happen.

Fixed at the root rather than special-cased: the cycle now accumulates the response each guard
**actually demanded**, and `hold` or stronger is what withholds the command. That also removed three
early returns whose ordering was a latent bug — a `workspace` hold returning immediately would have
hidden a `proximity` stop behind it, reporting the weaker of the two. §7.1 says so now.

### `available.solvers` was checking the wrong thing, and `available.collision_model` was checking nothing

The `workspace` guard was gated on the descriptor declaring an **inverse**-kinematics solver. §44.2
gives it forward kinematics computed in-cycle by the kernel itself; a robot with no IK solver at all
can run the guard perfectly well, and one with three cannot run it without a frame graph.

`collision_model` was worse in kind: a boolean the descriptor asserts about itself. A descriptor
could say `true` with no geometry in it and the guard would report itself **active** while having
nothing to evaluate — §7.2's failure in its most damaging form, since the whole point of that
paragraph is that a guard which cannot run says so. Both fields are gone. The guards ask the frame
graph: volumes naming frames the tree actually reaches, two or more placeable collision shapes. CI
asserts neither field comes back.

### The corpus had a hole, and the hole was in the *inputs*

`proximity` and `mode_gate` have been implemented in the Rust edition since the first session and
absent from the reference edition ever since. It was recorded at the time — "three guards go
slightly beyond the reference… with reference-derived vectors, none of the three has its input, so
behaviour is bit-identical and the corpus still holds" — and that reasoning was correct and the
conclusion was too comfortable. Two of the eight guards were pinned by one edition's own suite and
by nothing else, which is precisely the arrangement §31.4 exists to prevent.

Nothing caught it because the generator never produced a cycle carrying a capability token or a
separation distance. **A corpus grows a blind spot wherever an input field is missing, not only
where an implementation is** — and that generalises past this instance, which is why it is written
down here and in `corpus/README.md`.

Closing it exposed a second defect immediately: the reference's `ResolvedLimits` had **no
`proximity` field at all**. `partitionGuards` reported the guard active from the profile while
`evaluateCycle` had no thresholds to compare against, so the reference's proximity guard had been
inert since it was written. One corpus case with a distance in it found that in a minute.

The reference edition now implements all eight guards, `reference/typescript/kinematics.ts` is its
second implementation of the kernel's arithmetic and geometry (753 lines, and the second file in
`reference/` that is not vendored from the Hub — the Hub computes no forward kinematics), and the
fuzz generator draws capabilities, separation distances and proximity profiles.

The fuzz corpus was also **not testing what it claimed**. A uniform command lag over 300 ms against
a 100 ms watchdog window made two cases in three a watchdog stop, and a watchdog stop returns before
any other guard runs — so most of forty cases exercised one guard while the file reported coverage
of eight. Staleness is now drawn deliberately: **six of the eight guards appear in the fuzz
verdicts**, up from three.

### The corpus grew by 44, and the new ones freeze poses rather than verdicts

350 cases, 29 files. `kinematics/solve` is a new corpus and it exists because `cycle` cannot do its
job: a guard trip is a boolean, and two editions could agree on every trip in the corpus while
disagreeing about where the arm is by a millimetre — right up to the day one of them clamps and the
other does not. Fourteen cases freeze the poses themselves, thirty digits at a time, which is what
holds the two editions' sine, cosine and square root to each other.

**All 350 reproduced byte for byte on the first run**, including a translation of
`0.0000000000000002220446049250313` — one ulp of 1, in canonical positional form, computed
independently in two languages.

### Everything that reads or writes a frame graph now knows about the geometry

* **§44.3 gains four fields**, amended in both copies of the specification: `collision_shape` (a
  sphere-swept segment — one primitive, a sphere when its endpoints coincide, bounding a cylinder
  exactly and a link's real geometry conservatively), `collision_ignore` (SRDF's
  `disable_collisions`, without which the guard trips on a humanoid standing still and gets switched
  off), and `mimic_multiplier` / `mimic_offset` (the commonest linkage in robotics is a parallel
  gripper whose second finger runs at −1, and a field that could only say "follows" would put that
  finger on the wrong side of the object).
* **`collision: true` and `collision_shape` stay two different facts.** A vendor's model is usually
  meshes and no honest reduction of a mesh to a primitive exists without reading the mesh, so a
  frame may declare geometry and offer no shape — and `self_collision` then reports unavailable
  naming those frames, rather than testing a body with limbs quietly missing from it.
* **The URDF importer converts three of URDF's four geometries** and declines the fourth. A sphere
  is exact; a cylinder is the primitive itself; a box becomes a segment along its longest axis with
  a radius of half the cross-section's diagonal, which is loose — for a cube it is the circumscribed
  sphere — and loose is the only direction of error a collision bound may have. A mesh produces
  nothing.
* **The MuJoCo driver** emits a capsule containing all of a body's contact geoms, and `null` for a
  mesh or height field. Verified against the four scenes in the repository with real MuJoCo
  installed.
* **`skynet frames`** is a new command: the tree, where a configuration puts it, and — the part that
  matters — what the two guards will be able to do with it. A body with one collision capsule has
  nothing to collide with, and reading that on a workstation beats inferring it from a guard
  silently doing nothing on a robot.
* **`skynet driver check`** reports the same thing as its own result, because the report prints
  detail only for a result that is not a plain pass and "the guard cannot run" must not be swallowed.

### Verification

* **456 Rust tests pass on this machine**, up from 374. The kernel went from 45 to 101; `skynet-urdf`
  from 37 to 44; the CLI's integration suite from 10 to 16.
* **350 corpus cases reproduce byte for byte**, up from 306, and regenerating is idempotent.
* `cargo fmt --all --check` clean. `cargo clippy --workspace --all-targets` clean at `-D warnings`.
* The kernel still builds for `thumbv7em-none-eabihf` — one source, two targets.
* `node tools/check-mirrors.mjs`: all three shared documents identical.
* `Cargo.lock` still holds eleven packages, all ours. The `real` module is why: a `libm` dependency
  was the obvious alternative and it would have been the first third-party crate in the workspace.
* The drivers: mock 24 checks, LeRobot 70, MuJoCo 96 (one skipped for want of a GL context), the
  Python ABI view offset-asserted, and `skynet driver check` driven through the C host against real
  MuJoCo physics.
* **Not verified here: `crates/skynetd-io/e2e.sh`**, which needs a POSIX daemon under systemd. It
  was not changed this session.

### Three defects found in passing, each fixed rather than noted

1. **`tools/check-mirrors.mjs` could not find the Hub.** It looked for `../BotMarket` and the
   checkout is `../Botmarket`; on a case-sensitive filesystem those are different directories, so
   the script reported "not found — nothing to compare" and exited zero. Its own file already says
   a check that cannot run is not a check. It now tries several spellings and says which it used —
   and with it working it immediately reported the divergence this session created.
2. **The workspace had not compiled clean under a current clippy.** `is_link_local` carried
   `|| v4 == Ipv4Addr::UNSPECIFIED && false`, which reads as a rule and is a comment the compiler
   deletes. The rule it was trying to state — `0.0.0.0` is a request to bind *every* address,
   including the routable one §34.2 forbids — is now a comment, which is what it always was.
3. **A test failed on any machine without an IPv6 stack.** It asserted `[::1]` binds, which is a
   claim about the machine and not about the policy. It now distinguishes the two: a bind that fails
   for want of a stack is fine, and one refused by our own §34.2 check is not. Skipping the case
   would have been the shorter exam §15 spends a rule on.

### What was changed in the Hub

`docs/skynet-os-architecture.md` only, and only because the mirroring rule requires it: §7.1's
response column, §7.2's evidence rule, §7.3's workspace composition, §31.2's single-source table,
§38.2's layering, §44.3's four new fields, §44.5's three limits and §52's table. The change was made
here first and carried across by hand; `node tools/check-mirrors.mjs` reports all three identical.
No Hub code was touched.

### What is still zero lines

The policy runtime, the SPSC ring and observation assembly, OTA (§12), the outbound queue (§34.4),
the on-robot console's HTML (§32.2 — the API behind it exists), calibration (§45), the watchdog
chain (§46), POST and homing (§48), the safety MCU, Ed25519, and the drivers that talk to real
hardware.

The frame graph has moved out of this list. `balance` is now the only guard of the eight that
neither edition implements — there is no centre-of-pressure input in either `CommandCycle` — and it
reports itself unavailable rather than passing.

§59.5's several-engineer-years figure is unchanged.

### Three findings recorded rather than fixed

1. **§52 now names `embodiment.frames.complete` in three rows and no such SCTS test exists.** It
   named it in one row before this session and the gap was already carried; adding two more rows
   pointing at an absent test makes the case for closing it stronger rather than weaker. The §52
   expansion has been on the Next list for three sessions and should be done as one reviewed change
   with the corpus regenerated, not folded into a feature.
2. **Self-collision excludes only parent–child pairs and whatever the descriptor declares.** That is
   the right rule and it puts the whole burden on `collision_ignore`. A real humanoid needs a
   populated exception list before this guard is usable, and nothing generates one: the URDF
   importer cannot, because URDF does not carry it, and reading an SRDF is a second file format this
   importer does not open. Until then, `skynet frames` showing the pair count is the honest warning.
3. **One capsule per frame is a coarse model, and the MuJoCo path shows how coarse.** A body with
   several scattered geoms gets a capsule containing all of them, which for the arm scenes in this
   repository is a sphere considerably larger than the link. It is conservative, so it trips early
   rather than late — but "early" on a body modelled that way may mean "constantly". A list of
   shapes per frame is the fix and it changes the kernel's inner loop from quadratic in frames to
   quadratic in shapes, which is exactly the cost §44.5 already flags.

### Next

1. **The §52 SCTS expansion**, as one reviewed change. It is now overdue by three sessions and this
   session made the gap wider.
2. **The on-robot console (§32.2)** — the API is complete enough to sit behind one; what is missing
   is the small static page.
3. **The embodiment descriptor on the robot**, which still unblocks three recorded findings at once:
   admission on the install path, the black box's real shape, and its provenance digests.
4. **The Hub side** (§32.4): `/skynet/os` readiness over the live registry, the transport endpoint,
   the embodiment tables, and the incident upload.
5. **Calibration (§45)**, which is what makes `source: "calibration"` in a frame node mean anything.
   Every extrinsic in every descriptor this repository can generate today says `descriptor`, and
   §44.3 is explicit that where both exist the measured one wins.

---

## 2026-08-30 (eighth) — the §52 expansion, and the half of §44.3 nothing was checking

The Next list has opened with **the §52 SCTS expansion** for four sessions, and the last session
made the case stronger by widening the gap it was about. This session closed it.

§52 collects the descriptor fields Parts IV and V added and gives each a "Measured by" column
naming the SCTS test that measures it. Nine rows, seven distinct tests — and **five of the seven
did not exist**. Three rows pointed at `embodiment.frames.complete`, which no edition implemented.
A vendor reading §52 as a list of obligations would have found no way to fail five of them, which
makes those rows worse than absent: an obligation with no test is a field the vendor may fill in
with anything and still be conformant.

### The five, and where each number has to come from

| Test | Suite | Tier | What it measures | From |
|---|---|---|---|---|
| `embodiment.frames.complete` | sim | host | The frame graph is a tree, every name in it resolves, every shape is one this edition can evaluate | §44.3 |
| `safety.command_loss.behaviour` | bench | host | Commands stop; what the drives *do* is what `on_command_loss` declares | §46.2 |
| `safety.homing.envelope` | bench | host | The homing sweep stays inside the restricted envelope, measured at the joint | §48.3 |
| `rhal.sensor.latency` | bench | host | Per sensor, event to timestamp — a backend stamping arrival rather than capture fails | §47.2 |
| `power.safe_state.reachable` | bench | host | From full rated load, timed to the nearest safe state, against `time_to_safe_state_s` | §36.2 |

`power` is a ninth area, and it earns one: the energy a robot keeps back to stop with is not a
property of the RHAL, the body, the kernel, packages, OTA, telemetry, offline operation or
security. §36.2 computes the reserve budget from a declared figure, and a body that declares two
seconds and takes six has a third of the reserve it needs — which is the kind of number that is
otherwise discovered in an incident report.

All five are `host` rather than `guest`. Guest is the portable core every integration passes, and
four of these need a bench session and the fifth needs a frame graph most bodies do not have yet.
Requiring them at guest would refuse bodies that are safe by the same argument the rest of the
design is safe by.

The suite is **35 tests across 9 areas**. §15's prose said 29 across 8 and the code said 30; the
count now agrees in the prose, the oracle and the Rust, and §15 says why it grew.

### The test that would have caught this three sessions ago

`every_test_52_names_as_a_measure_exists` transcribes §52's third column and asserts each id is in
the suite. The table is prose in a mirrored document and the suite is code, so nothing makes them
agree by construction — what this does is make the second half of a §52 row impossible to forget.
Add a row naming a new test, and the suite fails until the test exists.

Cheap, and it is the mechanism whose absence let three rows point at nothing for three sessions.

### `embodiment.frames.complete` had to measure something, and the obvious half was already done

A test that exists and measures nothing is the defect this session was fixing, one level down. So
the question was what "complete" means for a frame graph, and the answer split in two.

The first half already existed: `validate_frames` checks the tree — a single root, no cycles, no
duplicate names, no parent that resolves to nothing, shapes this edition can evaluate, ignore lists
naming frames that exist. `skynet driver check` was already running it. It was filing the result
under `embodiment.descriptor.valid`, so this session moved four results onto the id §52 names, which
is also the more honest tier: a malformed graph is a real defect, but §44.5 already bounds its
consequence to two guards going unavailable, and blocking guest on it was too strong.

The second half did not exist in either edition, and it is the interesting one.

### `joint` and `mimics` do not name anything in the graph

They name entries in the **joint list**, and `validate_frames` is never given one. It is handed a
tree and asked whether the tree is well formed, and it answers correctly without ever seeing the
descriptor the tree came out of. So a `joint: "shoulder_pitch"` on a body whose joint list says
`shoulder` is a graph that passes every check either edition had — and it is wrong in a way that
reaches the robot.

`validate_frame_bindings` is the missing half, in both editions, with four faults:

* **`dangling_joint`** — a dynamic frame names a joint the body does not declare.
* **`mimic_missing`** — a linkage follows a joint the body does not declare.
* **`mimic_self`** — a frame follows the joint it is driven by. `value = multiplier × driver +
  offset` over one value is a fixed point, not a linkage.
* **`mimic_coefficient`** — a multiplier or offset with no `mimics` to apply it to, or one that is
  not a finite number.

It deliberately does **not** require that every declared joint have a frame. A gripper whose fingers
nobody modelled is a legitimate partial tree and §44.5 already has the guard report unavailable
naming the frame it wanted. "Complete" here means every name the graph uses resolves — not that the
graph covers the body.

### What a dangling joint actually does, which is not what I first wrote

The first draft of this function said a dangling joint makes the frame hold its rest pose while the
guards read that pose as the truth. That is wrong, and checking it rather than asserting it changed
the argument for the check into a better one.

`Kinematics::solve` marks a frame with no binding **unresolved**. `check_workspace` treats
`(None, _)` as a trip rather than a pass — "the guard has not shown the command safe" — and
`check_self_collision` does the same. So nothing is silently believed. What happens instead is that
both pose guards trip on that frame **every cycle, forever**, which is correct and useless: the
robot holds still and no report traces the cause back to a spelling in a file.

§44.5 asks for better than that. A guard which cannot run is supposed to report so **when the lease
is granted**, naming the frame, and `statically_resolvable` is that mechanism — but it reads the
joint's *kind*, so it catches a limb hanging below a ball joint and cannot catch a joint name that
binds to nothing, because the joint list is not in the graph. This check is what closes that gap: it
turns a permanent in-cycle trip into a refusal while somebody is still looking at the file.

The wrong sentence had reached six places including two user-facing fault messages. All six were
corrected and the corpus regenerated.

### Reachable from a workstation, not only from a driver

`skynet frames` now runs the binding check, because the command a driver author actually has in
front of them should be the one that finds this. It reads the descriptor's `joints` array and
refuses on a fault.

An **absent** `joints` array is left alone, and that is a decision rather than an oversight. This
command is run on partial descriptors while one is being written, and refusing every frame in the
tree because the joint list has not been filled in yet would make it useless for the job it exists
for. An absent joint list and a joint list that omits the joint are two different statements.

`skynet driver check` runs it too, filed under `embodiment.frames.complete`, which is where a driver
gets caught — the graph and the joint list are commonly produced by two different code paths in one.

### The corpus grew by ten, and one case could not be written

`corpus/frames/bindings.json`, ten cases, and the joint list is what separates it from
`frames/validate`: same trees, plus the body they came out of.

The eleventh and twelfth cases were written, generated, and removed. The non-finite coefficient rule
cannot be frozen in this corpus, because the generator writes every input through `JSON.stringify`,
which renders infinity as `null` — so the frozen input would describe a *different* body from the
one its expectation was computed from. That is worse than no case: it would fail an edition for
reading the file correctly. It was caught by reading the generated file rather than by trusting the
generator, and the reason is now a comment in `generate.ts` so the gap is a decision.

The rule is held by unit tests in the Rust instead, and the two editions reach it differently in a
way worth recording — see the findings below.

### The corpus caught five bugs of mine in one run

Every one of the five string literals in the new Rust function had its line continuations eaten on
the way into the file, so each fault message carried twenty-odd spaces in the middle. The Rust
compiled, the function was correct, and every test that did not compare exact bytes passed.
`cargo test -p skynet-corpus` failed on the first run and named the byte offset of the first
difference. That is the mechanism §31.4 exists for, working on its author.

### Verification

| | |
|---|---|
| `cargo test --workspace` | **24 test binaries, all ok**, 0 failed |
| `cargo clippy --workspace --all-targets` | clean |
| `cargo fmt --all --check` | clean |
| `cargo test -p skynet-corpus` | this edition reproduces all **360** cases byte for byte |
| `node tools/corpus-gen/generate.ts` | regenerates identically; the only diff is this session's |
| `node tools/check-mirrors.mjs` | all three shared documents match |
| Hub `npx vitest run lib/__tests__/skynet-os-packages.test.ts` | 28 passed |
| `skynet frames` on a hand-broken descriptor | refuses with `dangling_joint`, naming the frame and the joint |

`skynet-contracts` 90 → 93 tests, `skynet-cli`'s CLI suite 16 → 18, corpus 350 → 360 cases across
29 → 30 files.

### A failing test at HEAD, and why it failed on this machine and not in CI

`cargo test --workspace` was red before any of this session's work.
`a_descriptor_with_no_frame_block_says_which_of_the_two_absences_it_is` asserted a phrase against
raw stderr, and `term::wrap` reflows a refusal to 92 columns with the file path in front of it. On
a machine whose temp directory is long enough, the wrap point lands inside the phrase. CI's is not.

`said()` — stderr with its whitespace collapsed — already existed in that file for exactly this, and
most of the file already used it. Five assertions did not, of which two were failing and three were
one longer path away from it. All five now use it.

Worth stating plainly: a test that passes or fails on the length of a temp path is a test that
reports on the machine rather than on the code, and it had been committed green.

### What was changed in the Hub

Two files, and the mirroring rule was applied to both.

* `docs/skynet-os-architecture.md` — §15's count and the paragraph saying why it grew. Made here
  first and carried across by hand; `node tools/check-mirrors.mjs` reports all three identical.
* `lib/skynet/os/conformance.ts` — the five tests and the ninth area. This file is genuinely
  byte-identical to `reference/typescript/conformance.ts` (it imports nothing outside itself), the
  Hub consumes SCTS to award tiers, and the Hub's own suite passes unchanged at 28.

No other Hub code was touched.

### What is still zero lines

The policy runtime, the SPSC ring and observation assembly, OTA (§12), the outbound queue (§34.4),
the on-robot console's HTML (§32.2), calibration (§45), the watchdog chain (§46), POST and homing
(§48), the safety MCU, Ed25519, and the drivers that talk to real hardware.

Four of the five tests added this session are `bench` tests, and **nothing runs a bench suite**.
They are specification and award arithmetic today: the award machinery will now correctly refuse a
host claim whose command-loss behaviour was never measured, and there is no harness that could
measure it. That is the honest state and it is the same state the other fifteen bench tests have
been in since §15 was written — but five more of them is worth saying out loud rather than counting
as progress.

`balance` remains the only guard of the eight that neither edition implements.

§59.5's several-engineer-years figure is unchanged.

### Three findings recorded rather than fixed

1. **The two editions disagree about a descriptor containing `1e999`, and no corpus case can catch
   it.** `crates/skynet-json` refuses it at parse — "a literal so large it overflows to infinity is
   a corrupt corpus file, not a value" — so the Rust edition rejects the whole document. The
   TypeScript oracle has no parser of its own; whoever calls it arrives through `JSON.parse`, which
   answers `Infinity`. So the same file is a parse error in one edition and an infinite coefficient
   in the other, and §31.4's byte-identical requirement has nothing to say about it because the
   corpus cannot express the input. The non-finite check exists in both editions and is load-bearing
   in exactly one. The fix is a canonical *parser* in the oracle rather than `JSON.parse`, which is
   a real piece of work and should not be folded into a feature.
2. **`reference/typescript/embodiment.ts` and `safety.ts` have diverged from the Hub's copies, and
   the README claimed for two sessions that they had not.** Ahead by 214 and 380 lines
   respectively — the frame graph, its geometry and the two pose guards, all built here because the
   Hub has no consumer for them. The divergence is defensible; the stale claim of identity was not, because a claim
   nobody re-checks is how two copies get to differ by four hundred lines without anybody noticing.
   The README now says which five files are byte-identical and which two are not. Whether the Hub
   should carry the frames block at all is a Hub question this repository should not answer alone.
3. **Self-collision still excludes only parent–child pairs and whatever the descriptor declares**,
   carried from last session and untouched by this one. `validate_frame_bindings` checks that a
   `collision_ignore` entry names a frame that exists; it says nothing about whether the list is
   *populated*, and nothing generates one. A real humanoid needs a populated exception list before
   the guard is usable.

### Next

1. **The on-robot console (§32.2)** — the API is complete enough to sit behind one; what is missing
   is the small static page. It is now the oldest item on this list.
2. **The embodiment descriptor on the robot**, which still unblocks three recorded findings at once:
   admission on the install path, the black box's real shape, and its provenance digests.
3. **The Hub side (§32.4)**: `/skynet/os` readiness over the live registry, the transport endpoint,
   the embodiment tables, and the incident upload.
4. **Calibration (§45)**, which is what makes `source: "calibration"` in a frame node mean anything.
   Every extrinsic in every descriptor this repository can generate today says `descriptor`, and
   §44.3 is explicit that where both exist the measured one wins.
5. **A canonical JSON parser in the oracle**, from finding 1. It is the only way the `1e999`
   divergence becomes something the corpus can hold.

---

## 2026-08-30 (ninth) — the on-robot console, and what it refuses to draw

The console is §32.2, it was the oldest item on the Next list, and the API had been complete enough
to sit behind one for three sessions. What was missing was the page.

### Three files, and why not one

`crates/skynetd-io/console/` — `console.html`, `console.css`, `console.js` — compiled into the
binary with `include_str!` and served from `GET /`, `/console.css`, `/console.js`.

Compiled in rather than read from disk, because the console is what somebody reaches for when
something is wrong, and a page that could be missing because a partition failed to mount is a page
that is missing exactly when it is needed. A binary that started has its console.

Three files rather than one self-contained page, which is the less obvious choice. A single file
needs inline `<script>` and inline `<style>`, and a `Content-Security-Policy` permitting those
permits every injection the script's own escaping discipline exists to prevent. Split, the policy is

```
default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self';
img-src 'none'; font-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'none'
```

with no `unsafe-inline` anywhere. It costs two extra requests on a link-local socket. It is still no
build step: three files copied into a binary is not a toolchain.

### The rule the page exists to obey

> **It must never show a value it did not receive.**

One sentence, three mechanisms, and they are the whole design:

1. **`absent()`** — a field the daemon returned as null renders as *the reason it is null*. Never a
   zero, never a dash, never a blank cell. §33.6 spends a paragraph establishing that zero guard
   trips and no measurement of guard trips are opposite facts, and a console rendering them the same
   way undoes that at the last possible moment. A dash is the specific trap: it reads as zero.
2. **`Feed`** — every panel is drawn from a feed that remembers *when* its last answer arrived. The
   age prints beside the heading, counting up twice a second, and past three poll periods the panel
   goes `.stale`: values dimmed, age in warning colour. A failed poll does not clear the panel —
   losing the last thing the robot said is worse — but it cannot leave it looking live either.
3. **Panels with no source render what is missing and why.**

### The guards panel is the one worth reading

§32.2 asks for live guard state: which are active, which are unavailable with the exact input
missing, trip counts this session. None of it exists. §33.1 keeps `skynetd` away from the body
entirely — it supervises processes and never commands a joint — so the Safety Kernel is not
evaluating cycles here and trip counts have nothing to count.

So the panel lists the eight guards of §7.1 in evaluation order and, for each, names the input this
robot does not have: `workspace` wants a frame graph and forward kinematics in cycle, `proximity`
wants a separation distance from a sensor, `balance` wants a centre of pressure no edition
implements, `watchdog` wants a command deadline from a running control loop. Eight rows, eight
`unavailable`, eight specific absences.

That is §7.2's rule — a guard that cannot run reports what it wanted — applied to a screen. An empty
panel would have read as "nothing wrong", and eight green ticks would have been the one thing worse.

The same discipline runs through the rest. The status panel reports `enforcement` and says in the
same line that it is *enforced, not awarded* — §15 awards a tier from evidence and this robot has
none, and "host tier" over a machine nobody measured is the precise failure the platform exists to
avoid. §27's state machine position is named as unreported, with the two unwritten things that would
supply it. The health panel's five measurements arrive as null and each renders as the daemon's own
sentence about why. The black box carries its `cycles_unavailable` note verbatim.

### Two plans share a word, and the panel says which one it is showing

§32.2's Plan panel is the resolved **transport** plan with clamps and provenance. `GET /plan` is the
last **reconcile** pass. They are different objects and it would have been easy to put one under the
other's heading.

The reconcile pass is real, so it is shown under its own name, with stage, action, subject, outcome
and reason per row. The transport plan is reported absent, with what it would need — an embodiment
descriptor on the robot and a policy to plan for — and where the same computation already runs
today, which is `skynet transport` on a workstation.

### The stop button

In a `position: sticky` header, above everything, at every scroll position. No confirm dialog.

Sticky rather than a scroll listener, because a scroll listener that fails leaves the stop control
somewhere up the page and sticky positioning cannot fail that way. No dialog because §32.2 is
explicit and §39 gives the reason: an authentication prompt — or any prompt — between a person and a
stop button is a hazard.

The one failure the script refuses to swallow is a stop that did not reach the daemon. It does not
retry quietly and it does not print "stopped": it says the button did nothing and to use the
physical estop.

### Served before the credential check, deliberately

The asset arm runs at the top of `api::handle`, before authorisation. All three files are
`Role::Anonymous`.

`Fixture::new` builds an unprovisioned robot, and `the_console_loads_on_an_unprovisioned_robot_with_no_credential`
asks for all three with no token. §39.2 says that robot's console offers status and stop and nothing
else — and a page that answered 401 would offer neither. The catch-all in `requirement` is
`Role::Admin`, so this is permanently one table entry away from being wrong; `console::is_asset` is
also wired into that table, unreachable today, so that any future caller asking it about the console
is not told the page needs an administrator.

### What the tests actually hold

Six unit tests and three integration tests, and the ones worth naming are the ones that hold a
design decision rather than a behaviour:

* the stop button is inside the sticky header, the header is sticky, and the script never calls
  `confirm()`;
* no file contains `http://` or `https://` — the CSP forbids reaching off the robot, but a page that
  needs the CSP to stop it is a page that fails differently on a browser ignoring the header;
* the policy contains no `unsafe-inline`, no `unsafe-eval`, and no wildcard;
* the script has an `absent()` and never assigns to `innerHTML`.

That last one failed on its first run, correctly, and for the wrong reason: it searched for the bare
word `innerHTML` and found it in the comment saying the script does not use it. Reading the property
is harmless; assigning is the risk. It now matches `innerHTML =`, `innerHTML=`,
`insertAdjacentHTML` and `document.write`, which is both stricter about what matters and immune to
the file describing its own discipline.

### One check Rust structurally cannot do

`console.js` is `include_str!`'d, so Rust sees a string. A missing brace compiles, ships, serves with
a 200, and produces a blank page on a robot with every test in `console.rs` still green — and a
blank console is a robot whose stop button is gone.

`node --check crates/skynetd-io/console/console.js` is the whole of the fix, and it is now a CI step
in the `invariants` job.

### §32.4 ordered this fifth, and it was built second

Worth recording rather than quietly departing from. §32.4's items 1 to 4 are Hub routes ordered by
the tables they need; item 5 groups devices, certification and the console, deferring all three
because *"until then they would be showing invented state"*.

The console is not a Hub route — `skynetd` serves it from the machine it describes, so it needs no
table and no fleet. The only thing it was waiting on was that last clause, and that is the one thing
§32.2 already forbids. A console built to the rule on a robot with no kernel does not invent a state
machine position; it names §27 and says which unwritten thing would supply one. **The emptier the
robot, the more the page is worth, because what it is reporting is exactly the gap.**

§32.4 now says this, in both copies. The other two in item 5 stay where they are: they are Hub routes
over tables that do not exist.

### Verification

| | |
|---|---|
| `cargo test --workspace` | **446 passed** across 24 binaries, 0 failed (437 before) |
| `cargo clippy --workspace --all-targets` | clean |
| `cargo fmt --all --check` | clean |
| `node --check console.js` | parses |
| HTML tag balance, by script | balances; 23 elements with ids |
| Every `$('id')` the script reaches for | all 16 exist in the page |
| `node tools/check-mirrors.mjs` | all three shared documents match |

`skynetd-io` 83 → 92 tests. On this machine 68 of them run; the other 24 are `cfg(unix)`, which is
why the count is asserted from the source rather than from a run.

### What is honestly not verified

**This console has never been rendered in a browser.** There is no Linux here, `serve` is
`cfg(unix)`, and there is therefore no listener to point a browser at. What is verified is that the
three files are served with the right content types and headers through the real router, that the
script parses, that the HTML's tags balance, and that every element id the script reaches for
exists. What is *not* verified is that it looks right, that the poll loop behaves against a live
daemon, or that the object-URL download path works — that last one is the least certain, because a
`default-src 'none'` policy and a blob-backed `<a download>` interact in ways that differ by
browser.

That is a real gap and it is the kind that a screenshot would close in a minute on the right
machine. It is recorded rather than papered over.

### What was changed in the Hub

`docs/skynet-os-architecture.md` only — the §32.4 amendment, made here and carried across by hand;
`node tools/check-mirrors.mjs` reports all three identical. No Hub code was touched.

### What is still zero lines

The policy runtime, the SPSC ring and observation assembly, OTA (§12), the outbound queue (§34.4),
calibration (§45), the watchdog chain (§46), POST and homing (§48), the safety MCU, Ed25519, and the
drivers that talk to real hardware.

The on-robot console has moved off this list. Everything it reports as unavailable is still on it,
which is the point of the panel.

### Three findings recorded rather than fixed

1. **The console is untranslated, and §32.3 requires otherwise.** "Every string translatable — a key
   in `messages/en.json`, then `i18n-catalog.mjs sync`. Both interfaces, all 35 locales." Every
   string in these three files is English in the source. The Hub has a catalogue and a sync tool;
   `skynetd` has neither, and giving it one means either shipping 35 catalogues in the binary or
   serving a locale the robot may not have. Neither is obviously right, both are more than a page
   deserves before anybody has read it, and the constraint is real — an operator who cannot read the
   stop button's panel is exactly who this page is for.
2. **The console polls, and a robot in trouble is when polling is worst.** Five sequential requests
   every two seconds, each on its own connection, against a single-threaded listener that handles one
   request per connection (§33.7). That is fine on a healthy robot and it is the wrong shape when the
   daemon is struggling — which is when somebody is looking at the page. An event stream would be the
   answer and it is a change to the listener, not to the console.
3. **`GET /plan` returns the reconcile pass, and §32.2's Plan panel wants the transport plan.** The
   console now shows the first under its own name and reports the second absent. Both are correct and
   the route name is the thing that is wrong: `/plan` will eventually have to become `/reconcile` or
   acquire a sibling, and doing it before a client depends on it is cheaper than after.

### Next

1. **The embodiment descriptor on the robot.** It is now the item that unblocks the most: admission
   on the install path, the black box's real shape, its provenance digests — and, after this session,
   the console's transport panel and the pose-guard half of its guards panel.
2. **The Hub side (§32.4)**: `/skynet/os` readiness over the live registry, the transport endpoint,
   the embodiment tables, and the incident upload.
3. **Calibration (§45)**, which is what makes `source: "calibration"` in a frame node mean anything.
4. **A canonical JSON parser in the oracle**, from the eighth session's first finding. It is the only
   way the `1e999` divergence becomes something the corpus can hold.
5. **Render the console once on a Linux box** and fix what a browser says. It is a small job and it
   is the only thing standing between "the tests pass" and "the page works".

---

## 2026-08-30 (tenth) — the robot learns what body it is

This item has been on the Next list under one wording or another for four sessions, always with the
same justification: it unblocks more than anything else on the list. It does, and this session found
out that it unblocks slightly less than advertised — see the finding at the end.

### The daemon had no idea what it was running on

Not a gap in a feature. A gap that three separate things were waiting behind:

* `admit_package` off the install path, because `DeviceCapacity` needs the body;
* the black box's `embodiment` digest at zero, because a recording carrying an invented one could be
  replayed against the wrong machine;
* the console reporting the whole body absent where one field was actually missing.

`crates/skynetd-io/src/embodiment.rs` reads `/data/state/embodiment.json` and closes the second and
third of those.

### It arrives as a file, and on this side it has to

§4 gives the driver a `describe()` that returns exactly this document, and calling it here is
impossible. §33.1 says `skynetd` never touches an actuator, the workspace denies it a dependency on
`skynet-rhal`, and CI asserts the edge is absent. Loading a driver to ask what body this is would
hand the supervisor a vtable of function pointers into vendor code, which is the whole of what that
rule prevents.

So the descriptor is written at provisioning (§50), by `skynet descriptor from-urdf`, or by vendor
tooling, and the daemon reads it the way it reads everything else about the world.

### Three states, not two

`Body` is `Absent`, `Malformed { detail }`, or `Present`. That is §7.2's rule rather than
fastidiousness, and it is the distinction the whole module is shaped around: **"nobody has told this
robot what it is" is a provisioning step that did not run, and "somebody did, and the answer does not
parse" is a generator emitting something wrong.** Different people, different jobs, and a surface
rendering both as an empty panel hides the only fact that decides which.

A malformed descriptor is never partially believed. No field-by-field salvage, because a body
description half of which parsed is a body nobody has established the shape of.

An **empty file is `Absent`**, not malformed — it is what a truncated write leaves behind, and
"the descriptor does not parse" would send somebody hunting for a schema error in a file with
nothing in it.

### The eighth session's work, doing its job two sessions later

Loading validates the frame graph in both halves: `validate_frames` for the tree, and
`validate_frame_bindings` for the tree against the joint list. The second is what the §52 expansion
added, and this is the first place it runs on a robot rather than on a workstation.

A descriptor whose graph names a joint the body does not declare is refused at load, with the code
and the joint named. On the robot that defect is a frame whose pose never resolves and two pose
guards tripping on it every cycle — correct, permanent, and traceable to nothing. Refusing it here
turns it into a provisioning error somebody can read.

### The digest is over canonical JSON, and that is the point of it

Two robots handed the same descriptor with different whitespace must agree about which body they
are. Hashing the file would give them two answers, and a fleet where one robot's recording cannot be
matched to another's is a fleet where §56.4's replay argument stops working.

So: sha256 over `skynet-json`'s canonical form, under `skynet.embodiment.v1` — the same rule
`version_digest` applies to a manifest, and domain-separated for the same reason. Three tests hold
it: whitespace does not change the answer, a different body does, and the digest is not a bare hash
of the canonical form.

### What the recording now says

`supervisor_meta` fills two fields it could not before.

**`embodiment`** is the real digest when this robot has a descriptor and zero when it does not. Zero
is readable as "no body was known", is not a value anybody could mistake for a digest, and is what
lets the recorder open on an unprovisioned machine — §28 preallocates the ring precisely so nothing
can silence it.

**`simulator`** comes from the descriptor, for the reason §56.2 gives: a benchmark result replayed
from a simulated recording must never be mistaken for a field one. It is now in the header rather
than in somebody's memory of which machine they ran on. An unknown body is not a simulator, which is
§52's one asymmetric field pointing the way it should.

`os_image` is still zero and that is **not** the same kind of gap: §10's attestation is unwritten,
so nothing has established which image is running. A digest of the running binary would be a
different claim wearing this field's name.

`a_sealed_recording_names_the_body_it_was_taken_from` asserts the whole path — descriptor on disk,
meta, ring header, sealed MCAP, and the file on disk rather than only the bytes in hand.

### `GET /embodiment`, and two console panels that got narrower

An observer-role read returning the three states, the digest, the shape, and — named rather than
implied — that **no signature was checked**. §5 has the vendor sign the descriptor and
`embodiment.descriptor.signed` is an SCTS test; nothing here verifies one, so the answer says the
digest identifies the file and not its provenance.

The console gains a **Body** panel and its guards panel gets more precise. With no descriptor, the
two pose guards are missing a frame graph. With one, the tree is *here*, and what is missing is a
control loop to walk it in cycle — so the panel now says "3 of 4 frames move with a joint — what is
missing is a control loop" rather than "a frame graph on this robot". Both are `unavailable` and
they are different jobs; a panel naming the wrong one sends somebody to write a generator that
already ran.

The transport panel narrowed the same way: this robot now has a body, and what is missing is a
policy declaring the embodiment it was trained on.

### §28 pins the path

`/data/state/embodiment.json`, written into the specification, for the reason §37.1 pins the package
layout. Two editions that disagree about where the body is written are two editions where a robot
provisioned by one is a robot the other believes has no body — and a robot with no body records a
zero digest, cannot answer the capability half of an admission decision, and reports both pose
guards unavailable. That is a lot of wrong behaviour to inherit from a filename, and none of it
announces itself.

One file rather than a directory: a robot is one machine, and *which* of two bodies the kernel is
guarding is not a question this system should ever answer at run time.

### Verification

| | |
|---|---|
| `cargo test --workspace` | **458 passed** across 24 binaries, 0 failed (446 before) |
| `cargo clippy --workspace --all-targets` | clean |
| `cargo fmt --all --check` | clean |
| `node --check console.js` | parses |
| Every `$('id')` the script reaches for | exists in the page |
| `node tools/check-mirrors.mjs` | all three shared documents match |

`skynetd-io` 92 → 104 tests.

### A test bug of mine, found the worst way

Two of the new tests shared a scratch directory — the helper named it after a hash of the descriptor
text, so two tests handed the same descriptor got the same path, and `remove_dir_all` on entry raced
the read in the other. It passed until an unrelated change to `supervisor_meta` shifted the timing,
and then failed *looking like it belonged to that change*.

That is the expensive kind of test bug: not that it is wrong, but that it accuses the wrong code. The
helper now takes a counter, nothing is removed, and no two calls can interfere however the harness
schedules them. Run three times to confirm.

### What was changed in the Hub

`docs/skynet-os-architecture.md` only — §28's storage tree and the paragraphs pinning the descriptor
path. Made here and carried across by hand; `node tools/check-mirrors.mjs` reports all three
identical. No Hub code was touched.

### What is still zero lines

The policy runtime, the SPSC ring and observation assembly, OTA (§12), the outbound queue (§34.4),
calibration (§45), the watchdog chain (§46), POST and homing (§48), the safety MCU, Ed25519, and the
drivers that talk to real hardware.

### The finding this session went looking for and found instead

**`admit_package` still cannot be wired onto the install path, and the reason is no longer missing
code.** It is that §8 never says what determines the set of capability tokens a *robot provides*.

`DeviceCapacity.capabilities` is documented as "capability tokens the robot plus its adapter actually
provide", and nothing decides that set. The descriptor does not carry it — there is no
`capabilities` field on `EmbodimentDescriptor` — and §8 defines only the package's side: what it
declares it needs, and what the operator grants. The robot's side is assumed to exist.

Which leaves two ways to wire admission today and both are wrong:

* **Derive the set from the body** — a gripper implies `manipulation.grasp`, an audio sensor implies
  `sensing.audio` — which means a package is admitted or refused on the strength of a mapping table
  nobody specified and I would have invented. That is exactly the "invent a plausible number"
  failure the eighth session's finding warned about, one level up.
* **Leave it empty** — and `admit_package` reports `capability_ungranted` as an *error* for every
  capability any package needs, refusing every install on a robot that is fine.

So the blocker was never the descriptor. Four sessions of Next lists said this item unblocked
admission, and it does not: it unblocks the digest and the console, and admission needs a
specification change first. Writing it down is worth more than a call site.

The fix is probably a `capabilities` array on the descriptor, which is a §52-shaped change — a
schema addition, a "measured by" row, and an SCTS test — and it touches the Hub's schema, the
oracle and the corpus. It should be one reviewed change, not folded into a feature. It is now the
head of the Next list.

### Two more findings recorded rather than fixed

1. **The descriptor's digest identifies the file, not its provenance.** §5 has the vendor sign it and
   `embodiment.descriptor.signed` is an SCTS test at host tier; §19's Ed25519 is unwritten, so
   nothing here verifies anything. `signature_checked: false` is reported on every answer and the
   console prints the reason, which is the most that can be done — but an operator who edited
   `embodiment.json` on a robot's disk would change the body the kernel guards and the recording
   names, and nothing would notice. That is the same threat §38.1 spends a section on for profiles.
2. **The route enumeration behaviour is fail-closed and undocumented.** `POST /embodiment` returns
   401 rather than 405 to an unauthenticated caller, because authorisation runs before the route
   match — so 404 and 405 are only distinguishable by somebody the robot already trusts. That is the
   right behaviour and it surprised a test I wrote expecting the documented 405. The §33.7 table
   should say so.

### Next

1. **`capabilities` on the descriptor**, from the finding above. It is what actually unblocks
   admission on the install path, and it is a §52-shaped change: schema, a "measured by" row, an
   SCTS test, the oracle and the corpus.
2. **The Hub side (§32.4)**: `/skynet/os` readiness over the live registry, the transport endpoint,
   the embodiment tables, and the incident upload.
3. **Calibration (§45)**, which is what makes `source: "calibration"` in a frame node mean anything —
   and which now has a descriptor on the robot to write into.
4. **A canonical JSON parser in the oracle**, from the eighth session's first finding.
5. **Render the console once on a Linux box** and fix what a browser says.

---

## 2026-08-30 (eleventh) — what a robot provides, and admission finally running

The tenth session's finding was that `admit_package` could not be wired because §8 never said what
determines the set of capability tokens a *robot provides*. This session answered that, and then ran
admission — which turned up a second, sharper question that had been hiding behind the first.

### The answer was already in the Hub

`SkynetRobot.capabilities` and `SkynetRobotAdapter.capabilities` are registry columns, and the
compatibility engine already answers "will this run?" from them before anybody downloads forty
gigabytes. So the fact exists; what was missing was its copy on the robot.

**§5.0** puts it in the embodiment descriptor, and the two reasons are worth keeping:

* **The descriptor is produced by the adapter, for the body.** "The robot plus its adapter" is
  precisely the question a driver's `describe()` is in a position to answer, and nothing else on the
  robot sees both halves.
* **§11's invariant is that no decision on a robot requires a round trip.** Fetching the set from
  the Hub at install time would make a package installable only while connected, on a machine the
  design expects to spend a week offline.

`capabilities` and `extensions`, on `EmbodimentDescriptor`, in both editions.

### The one place the descriptor's usual rule inverts

Everywhere else on this descriptor, absent means *nobody declared it* and the consumer reports
unavailability. That rule cannot apply here, and §5.0 says why: a missing list would leave admission
choosing between refusing every package and admitting every package, and there is no honest third
option.

So **an absent list reads as the empty set** — this body provides nothing — and the vendor is
required to write it down. The failure direction is refusal, which is visible, names the tokens, and
is fixed by declaring them. It joins `simulator` as the second exception in §52's table, each with
its own reason.

### The measure arrived with the field

`embodiment.capabilities.declared` — sim, host — added **in the same change** as the field it
measures. That is the rule the eighth session's §52 expansion was meant to establish, applied for
the first time to a new field rather than to a backlog of five. **36 tests across 9 areas**, and the
transcribed §52 list in `every_test_52_names_as_a_measure_exists` grew with the table.

The failure it guards against is silent and permissive: a body claiming `manipulation.grasp` it
cannot perform admits a package that will command a gripper nobody fitted.

### `crates/skynetd-io/src/capacity.rs` — nothing here is estimated

`DeviceCapacity` wants memory, capability tokens, grants and what is installed. Every field is read
from something, or the capacity does not assemble — which is why `read` returns a `Capacity` rather
than a struct with plausible defaults in it.

`ram_mb` comes from `/proc/meminfo`'s `MemTotal`, and two decisions inside that are load-bearing:

* **`MemTotal`, not `MemAvailable`.** Available memory would make admission depend on what happened
  to be running at install time, so the same package would install in the morning and be refused
  after lunch.
* **The unit is checked rather than assumed.** It has been `kB` forever; a unit misread by a factor
  of 1024 is a memory budget misread by a factor of 1024, in the permissive direction. An unfamiliar
  unit is refused with the reason.

On a host that is not Linux the capacity does not assemble and says so. That is a real limitation on
a laptop and it is correct on a robot: §23.5 says a robot runs Linux, and one whose memory cannot be
read is one nobody should install a policy onto. A `ram_mb` of 8192 because that is a common figure
would admit a package the robot cannot feed, and §8.1 is explicit about why that check exists — *a
phone that runs out of memory kills an app; a robot that runs out of memory mid-stride falls over.*

`vram_mb` and `disk_mb` stay `None`, which admission already reads as "not declared" rather than as
zero, and the wire says which absences they are.

**The body is read before the memory**, and the ordering is deliberate: a descriptor that does not
parse is a file on *this robot* somebody can fix, and an unreadable `/proc/meminfo` is a property of
the host the daemon was started on. Reporting the robot's own fault ahead of the environment's is
what an operator needs — and it keeps the descriptor branch reachable on a machine that is not a
robot, where the memory read would otherwise mask everything behind it.

### The sharper question, stated rather than resolved quietly

Admission runs. It also reports `unsigned` as an **error on every package**, because §37.1's verify
step is "digest, then Ed25519 signature" and **this edition has no Ed25519**.

Which forces a choice, and `crates/skynetd-io/src/admit.rs` opens by making it in the open.

A gate that refuses everything is not a gate. Wiring this to block the install path today would
refuse every package on every robot — including the ones the e2e run and the README's golden path
depend on — and the only way to use the system would be to take the gate out again. That is how a
safety check becomes a flag somebody turns off.

So **it reports and does not block**, and the reporting is where the value is: a budget the robot
cannot feed, a capability the body does not provide, a dependency that is not installed, an OS too
old. Four real findings, none of which was being made at all before this session.

**What it does not do is pretend the signature was checked.** Nothing rewrites the finding into a
warning to make the output look clean, and the sentence saying the report does not gate is printed
on *every* run including a clean one — because a reader who sees no findings must not come away
believing the signature was verified. `every_report_says_the_signature_was_not_checked` asserts
exactly that, on a clean report and a dirty one.

The exception is one code, `UNAVOIDABLE_HERE`, and `the_exception_is_exactly_one_code` asserts the
size. An exception list that grows by a line at a time is how a gate becomes decorative, and adding
a second entry is a one-word edit somebody would make to get a build green.

The gate belongs at **activation**, with a signature to check. A staged blob is inert (§37.1) and
the consequential moment is the symlink swap.

### `GET /capacity`

An observer read: what admission would be told about this robot, or why it cannot be told anything.
It is the panel that makes the gap concrete — an operator can see the capability set the body
declares, the grants in force, and what is unmeasured, rather than reading a refusal and guessing.

`denied` is empty and is the one field whose emptiness is **not** a claim: §38.1's operator profile
is signed, this edition verifies no signature, and a withheld-capability list read from an unverified
file would be a safety-relevant value anybody on the robot could edit. The wire carries the note.

### The compiler caught a real bug

`skynetd stage` passes an OS version to `capacity::read`, and the obvious local variable in that
scope is `version` — the *package's*. Passing it would have made `min_os_version` compare a policy's
version against itself, a check passing or failing for reasons unrelated to the robot. It did not
compile, and the fix is `api::VERSION`.

Worth recording because it would have type-checked in most languages, and the resulting check would
have looked like it was working.

### Verification

| | |
|---|---|
| `cargo test --workspace` | **470 passed** across 24 binaries, 0 failed (458 before) |
| `cargo clippy --workspace --all-targets` | clean |
| `cargo fmt --all --check` | clean |
| `cargo doc -p skynetd-io --lib` | no unresolved links |
| `node tools/corpus-gen/generate.ts` | regenerates identically |
| SCTS lists, TS against Rust | 36 each, identical order |
| `node tools/check-mirrors.mjs` | all three shared documents match |

`skynetd-io` 104 → 117 tests. The corpus is unchanged by the descriptor field, which is itself
informative: nothing serializes a descriptor, so the addition is additive in the strict sense.

### What was changed in the Hub

`docs/skynet-os-architecture.md` only — §5.0, §52's new row and its amended exception paragraph, and
§15's count. Made here and carried across by hand; the mirror check reports all three identical. No
Hub code was touched: `lib/skynet/os/embodiment.ts` has already diverged from this repository's copy
(recorded in the eighth session) and adding a field there is a Hub schema question rather than a
mirroring obligation.

### What is still zero lines

The policy runtime, the SPSC ring and observation assembly, OTA (§12), the outbound queue (§34.4),
calibration (§45), the watchdog chain (§46), POST and homing (§48), the safety MCU, **Ed25519**, and
the drivers that talk to real hardware.

Ed25519 has moved up that list from "one of ten" to the thing three separate mechanisms are now
explicitly waiting on: the admission gate, the descriptor's provenance, and §38.1's profile.

### Three findings recorded rather than fixed

1. **`AdmissionParams.signature_verified` is a bool, and it conflates two facts.** "The signature
   failed" and "this edition cannot check a signature" are §7.2's distinction exactly, and the
   contract cannot express the second. The consequence is the whole of this session's awkwardness:
   `unsigned` reads as a verdict when it is really an absence, and the daemon has to explain in prose
   what a type could have carried. The fix is a tri-state in the contracts, which touches both
   editions and the corpus, and it is worth doing before Ed25519 rather than after — because after,
   nobody will notice the ambiguity again.
2. **The store holds no version string, so a dependency range is checked by presence.**
   `InstalledPackage.version` is filled with the digest, because §37.1's on-disk shape is
   `blobs/<digest>` and the version lives in desired state. `MissingDependency` therefore fires only
   when a package is absent entirely, and a package present at the wrong version passes. Filling the
   field with the digest is visible in `GET /capacity`; inventing a version would not have been.
3. **`denied` is empty and permissive.** See above — it needs §38.1's signature verification, which
   is the same Ed25519 as everything else.

### Next

1. **Ed25519 (§19)**, which is now the head of three queues at once: the admission gate, the
   descriptor's provenance, and the operator profile's. Nothing else on this list unblocks as much.
2. **The tri-state signature in `AdmissionParams`**, from finding 1 — and it is cheaper before
   Ed25519 than after.
3. **The Hub side (§32.4)**: `/skynet/os` readiness over the live registry, the transport endpoint,
   the embodiment tables, and the incident upload.
4. **Calibration (§45)**, which now has a descriptor on the robot to write into.
5. **Render the console once on a Linux box** and fix what a browser says.

---

## 2026-08-30 (twelfth) — a signature has four states, and a bool held two

The eleventh session's first finding, taken while it was still cheap. It said this was worth doing
**before** Ed25519 rather than after, because after, nobody would notice the ambiguity again — and
that turned out to be right in a way that showed up in the first caller.

### What was wrong

`AdmissionParams.signature_verified: bool`. Three different facts collapsed into `false`:

| State | What happened | Whose job |
|---|---|---|
| `verified` | Checked against a key the operator trusts | — |
| `failed` | A signature was present and did not verify | **Stop.** The bytes are attested to somebody other than the manifest claims |
| `absent` | Nobody signed it | The publisher's |
| `unverifiable` | This platform cannot check at all | The operating system's |

They have different owners and different remedies, and an operator reading `unsigned` could not tell
whether to chase a publisher, upgrade the OS, or stop and investigate.

This is §7.2's rule — a check that could not run reports *what was missing* rather than returning a
verdict — applied to the one field where the missing thing might be an attack.

### The finding was worded as a fact about the wrong thing

The old message read *"No verified publisher signature. The bytes may be intact and nobody has
attested to who produced them."*

On this robot that sentence is false. Every package `skynetd` sees may be perfectly well signed;
what is missing is an OS that can check. The finding described the *bytes* when it was describing
the *reader*, and the remedy it implied — go and get this signed — is work somebody already did.

`signature_unverifiable` says "this platform cannot verify a release signature… that is a gap in the
operating system rather than in the package", and the daemon's exception constant moved with it:

```rust
pub const UNAVOIDABLE_HERE: PackageRejectionCode = PackageRejectionCode::SignatureUnverifiable;
```

An exception for *we cannot check* is defensible. One for *nobody signed it* would have been the
gate quietly admitting unsigned packages, and yesterday's constant was spelled the second way while
meaning the first. The test now asserts all three: it is `SignatureUnverifiable`, and it is neither
`Unsigned` nor `SignatureFailed`.

### `failed` earns its own code

The state where something *went wrong* rather than something is missing. It should not share a
bucket with a package nobody got round to signing, and the message says so: *"treat this as
tampering until somebody proves otherwise."*

Three findings for three states: `unsigned`, `signature_failed`, `signature_unverifiable`.

### No default, anywhere

`admission_params_from_json` used `flag(value, "signature_verified")`, which reads a missing key as
`false` — which is exactly the conflation being removed, one layer down. A document that does not
name a signature state is now refused with the four spellings listed, rather than silently becoming
"unsigned".

### The corpus

One case and one boolean became three cases, one per state that is a finding, each with the job at
the end of it in its note. `every-finding-at-once` now uses `failed`, so the case that exists to
prove all findings are reported together includes the dangerous one.

`admission/install` 21 → 23 cases, and both editions reproduce them byte for byte.

### The rename found a dead assertion in the Hub

`lib/__tests__/skynet-os-packages.test.ts` had a case passing `signature_verified: false` into a
spread. After the rename that key does nothing — the default `signature: 'verified'` survived it —
and the test asserting "every reason at once" dropped from three errors to two and failed.

It failed loudly, which is the good outcome. Worth recording anyway: a spread over a renamed
optional field is a silent no-op, and the only reason this one was caught is that the assertion
counted. `grep signature_verified` over the Hub is now clean.

### What was changed in the Hub

* `lib/skynet/os/package.ts` — the four states, the two new codes, the switch. Byte-identical to
  `reference/typescript/package.ts` apart from the import path, as it was before.
* `lib/__tests__/skynet-os-packages.test.ts` — the fixture, and the unsigned test rewritten as
  `tells the three signature failures apart`, which loops the three states against the three codes.
* `docs/skynet-os-architecture.md` — §37.1 gains the four-outcome table and the paragraph on why the
  collapse hurt. Carried across by hand; the mirror check reports all three identical.

Hub suite: **28 passed**, unchanged in count.

### Verification

| | |
|---|---|
| `cargo test --workspace` | **470 passed** across 24 binaries, 0 failed |
| `cargo clippy --workspace --all-targets` | clean |
| `cargo fmt --all --check` | clean |
| `cargo test -p skynet-corpus` | reproduces all 362 cases byte for byte |
| Hub `npx vitest run lib/__tests__/skynet-os-packages.test.ts` | 28 passed |
| `node tools/check-mirrors.mjs` | all three shared documents match |

The test count did not move, which is correct: three cases replaced one in the corpus and one Hub
test replaced another. The change was to make existing checks say something truer, not to add
checks.

### What is still zero lines

Unchanged from the eleventh session, and **Ed25519 is now the head of four queues**: the admission
gate, the descriptor's provenance, §38.1's operator profile, and — new this session — the ability to
ever report `verified` or `failed` at all. Three of the four states this change introduced are
currently unreachable in the daemon.

That is worth saying plainly rather than counting the enum as progress: the type is now able to
express the truth, and the truth it expresses today is still "this OS cannot check".

### Two findings recorded rather than fixed

1. **Three of the four signature states are unreachable on a robot.** `Verified`, `Failed` and
   `Absent` are exercised only by the corpus and the unit tests; the daemon hard-codes
   `Unverifiable`. That is honest and it is also a type whose value is deferred — the payoff arrives
   with Ed25519, and until then the enum is mostly documentation with a compiler behind it.
2. **`skynetd stage`'s admission report is not recorded in the black box.** It prints to a terminal
   and vanishes. §13 records package events, and "this package was admitted with these four
   findings" is exactly the kind of thing an incident review would want — particularly the
   `signature_unverifiable` on every install, which is the sentence that explains why a robot was
   running something nobody attested to. It should be an event, and the event schema already exists.

### Next

1. **Ed25519 (§19)**, now the head of four queues. Nothing else on this list unblocks as much, and
   three states added this session cannot be reached without it.
2. **The admission report as a black box event**, from finding 2 — small, and it is the difference
   between a decision that was made and a decision anybody can review.
3. **The Hub side (§32.4)**: `/skynet/os` readiness over the live registry, the transport endpoint,
   the embodiment tables, and the incident upload.
4. **Calibration (§45)**, which now has a descriptor on the robot to write into.
5. **Render the console once on a Linux box** and fix what a browser says.

---

## 2026-08-30 (thirteenth) — Ed25519, and the four queues waiting on it

The twelfth session ended with the same first item the eleventh did: *"Ed25519 (§19), now the head of
four queues. Nothing else on this list unblocks as much, and three states added this session cannot
be reached without it."* This session wrote it, and then went and emptied the four queues.

### `crates/skynet-ed25519` — written, because the alternative was worse

The workspace has no third-party dependencies, for the reasons its `Cargo.toml` gives. A signature
verifier is the place where that policy costs the most, and it is also the place where abandoning it
would cost more: the alternative is a supply chain reaching into the one component whose whole job is
to decide what may run on a robot.

So: SHA-512, GF(2^255 − 19), scalars modulo `L`, extended twisted Edwards points, and RFC 8032
verification. `no_std`, `forbid(unsafe_code)`, no dependencies, about 1,100 lines with the
explanations in.

Three decisions worth recording rather than discovering later in a diff:

* **The constants are checked against their definitions.** `d` is verified by `d · 121666 + 121665 ≡
  0`, `sqrt(−1)` by squaring it, and the basepoint by `5y = 4`. A mistyped digit in any of the three
  produces a coherent implementation of a curve nobody else is on — one that verifies its own
  signatures and no published one — and that is exactly the failure a suite written against itself
  cannot see. The basepoint is stored *compressed* and decompressed on use for the same reason: it
  makes the routine that parses attacker-supplied keys run on the one input whose answer is published.
* **The scalar reduction is schoolbook long division, one bit at a time.** ref10's `sc_reduce` is a
  hundred lines of hand-derived shifts and it is fast and nearly unreviewable. 512 compare-and-
  subtracts cost a few microseconds against a verification that already spends milliseconds in the
  curve, and every line is the step as it is defined. Buying that time back with code nobody in this
  project could check would be a bad trade twice over.
* **Nothing is constant-time, and that is a decision.** Everything verification touches is public —
  the key, the message and the signature, two of the three chosen by the attacker. There is no secret
  to leak. The moment that stops being true is *signing*, which is why signing is behind a
  `test-signing` feature that CI asserts appears in no manifest section that ships.

The strictness is the part that matters for a fleet, and §19 now says so: non-canonical `y` in a key
or commitment, a scalar at or above `L`, and small-order keys are all refused, and the group equation
is checked cofactorless. Every one of those is a case where two implementations otherwise disagree
about **the same bytes** — and a disagreement about whether a package is signed is worse than either
answer, because it means two robots in a fleet running different software.

`41 tests`: the FIPS 180-4 SHA-512 examples, RFC 8032 §7.1 in both directions (the vectors verify,
*and* their seeds re-derive the published signatures byte for byte, which tests the whole computation
rather than the final comparison), the eight small-order encodings, the nineteen long spellings of a
field element, `S + L`, and a bit-flip sweep over a message and a signature.

### The fifth signature state, which only exists once something can verify

The twelfth session's four states — `verified`, `failed`, `absent`, `unverifiable` — turned out to be
missing one, and it appeared in the first hour of having a verifier. **A signature can be entirely
valid and made by a stranger.** That is not `failed`, because nothing went wrong; it is not
`verified`, because §37.1 says *"against a key the operator trusts"* and this one was not; and
reporting it as either sends an operator to the wrong place. `untrusted` is a fact about a decision
nobody made, and the remedy is in the trust store rather than anywhere near the package.

This is the same finding the eleventh and twelfth sessions made, one layer further in, and it is worth
noticing that the shape repeats: **a check that gains a new capability gains a new outcome**, and the
outcome is usually the one where nothing is wrong except that nobody has decided anything.

`signature_untrusted` is an error rather than a warning. *Somebody* attested to these bytes and it
was nobody here.

### A signature travels with the key that made it

An Ed25519 signature does not name its signer, which means a verifier holding a set of trusted keys
and a signature that verifies under none of them **cannot tell `failed` from `untrusted`** — an
attack from an administrative gap, which are the two states in that table furthest apart. So the wire
shape is a pair, `{ "key": …, "value": … }`, and the robot asks two questions in order: does it verify
under the key it names, and is that key one the operator trusts.

Asking them the other way round collapses the distinction the pair exists to preserve. That ordering
is three lines of code and it is the whole reason the fifth state can be reported at all.

The message is **domain-separated** — `skynet.package.v1`, `skynet.profile.v1`,
`skynet.embodiment.v1`, each followed by the digest — because §38.1 signs profiles with the same
machinery and §5 has the vendor sign the descriptor. Without a tag, a signature over one document's
digest verifies over another's, and a test asserts exactly that it no longer does.

### `skynetd-io/src/trust.rs` — "a key the operator has trusted" acquires a referent

The phrase is in §19 and in §37.1 and had none. It is now `/data/state/trust.json`: Ed25519 public
keys with the operator's own label for each.

The **label** is why this is not a list of hex strings. A report that says *"signed by
`d75a980182b10ab7…`"* has told an operator nothing they can act on.

Four rules, each of them §39.2's credential rule applied to keys:

| | |
|---|---|
| No file | Trusts **nobody**. Every signature reports `untrusted`, which is true and names the file to write |
| A file that does not parse | Also nobody, and it says *that* — one is a robot nobody provisioned, the other is a file somebody broke |
| One malformed entry | Skipped and reported; the rest work. A key that fails to parse can only ever *remove* trust, so skipping is safe in the only direction that matters, while refusing the file turns one typo into a robot that installs nothing |
| A small-order key | **Refused outright**, because those eight points verify signatures nobody produced |

### The gate is real now, and it is §19's gate rather than a bigger one

`admit.rs` has carried an argument since it was written: a gate that refuses everything is a gate
somebody takes out, so the report reported and gated nothing. With a verifier there is **no longer an
unavoidable rejection** — every error it reports is one somebody can act on — and `UNAVOIDABLE_HERE`
and `only_unsigned` are both gone.

Where the gate landed is §19's sentence, taken literally: *"a driver runs native and must therefore be
signed by a key the operator has trusted."* A **driver** whose signature is anything but `verified`
does not activate. Not `untrusted` either — a stranger's valid signature on native code is precisely
the case the rule exists for, and it is the case nobody thinks of.

At activation rather than at stage, because a staged blob is inert and the symlink swap is the
consequential moment. Refused rather than deferred, because the same bytes and the same trust store
give the same answer every pass and reporting it as "waiting" would suggest otherwise.

**Every other kind still installs carrying its findings, and that asymmetry is a gap rather than a
principle.** Nothing on a robot can *produce* a signature — the Hub signs releases — so a developer
staging a package they built five minutes ago has nothing to sign it with, and refusing those would
refuse every local install and the golden path in the README, and the fix would be a flag. Closing it
needs a way to sign locally, which is a design question about developer keys rather than a missing
primitive. It is recorded in §37.1 in those words.

### §38.1 — the robot was believing a boolean it should have been checking

The worst thing found this session, and it was found by going to wire up the profile signature.

`DesiredProfile.verified` was decoded straight out of the desired state: `"verified": true` in the
document meant the limits were applied. And §38.1 names the attacker in its own text — *"the
interesting case is not a corrupt file, it is somebody editing limits on a robot's disk"* — while
**the desired state is a file on the robot's disk**. Anybody who could edit the limits could edit the
assertion that somebody had checked them, in the same pass, with the kernel's ceilings on the other
side of it.

The decoder now sets `verified` to `false` whatever the document says, the document carries the
*signature* instead, and `skynetd-io` verifies it against the trust store on the pass that reads it.
A document that could assert its own verification is not a document anybody has verified. CI greps
that the decoder never reads that key again.

The incident §38.1 wants for a signature that *fails* — as against one merely missing — is still not
raised: there is no outbound incident path (§34.4). The profile is not applied either way, so the
limits are safe; what is missing is the alarm, and that is recorded rather than quietly skipped.

### §5 — the descriptor's provenance, separated from its digest

`GET /embodiment` reported `signature_checked: false` and a paragraph saying why. It now reports the
verdict, the signer's label, and a note that says which of the two questions the digest answers: the
digest identifies **which document this is**, the signature identifies **who stands behind it**, and a
console showing only the first lets a fact about a file imply a provenance it does not carry.

The signature is **detached** — `state/embodiment.sig.json` beside the descriptor — because the digest
is taken over the descriptor's canonical form, so a signature written inside the document would change
the digest it is a signature of. A signature file that exists and cannot be read reports `failed`
rather than `absent`: somebody put it there, and "the file you wrote is unusable" is a different job
from "nobody signed this".

### The console, and what happened when somebody finally looked at it

`GET /packages` gained `signature` and `signer` per row, and the table gained a **Signed by** column
that keeps the four outcomes apart — `verified` names the signer, `failed` is red, `untrusted` is
amber and says *untrusted key*, `absent` says *nobody*. The body panel gained the same row.

Then the thing that has been item 5 on every "Next" list since the console was written: **it was
opened in a browser.** It had never been. Two bugs, in the first thirty seconds, and neither was
findable by anything this repository was already doing.

**`GET /plan` answered two different shapes, and the reconcile panel had never once rendered.** With
no pass it returned `{"pass": null, "note": …}`; with a pass it returned a *flat* object and no
`pass` key at all. The console discriminated on `value.pass === null`, fell through to the other
branch, and threw `TypeError: Cannot read properties of undefined` on every poll from the moment a
robot did anything. The CLI happened to read the flat shape and was fine. Two consumers, two shapes,
one route — and a test for each consumer, neither of which looked at both branches.

The route now nests under `pass` in both cases, the CLI reads it that way, and there is a test that
asserts the *same shape before and after a pass* rather than one branch at a time. That test is the
one that was missing, and its absence is more interesting than the bug: every individual assertion
passed.

**The health panel printed the same thirty-word sentence five times**, once per unmeasured field.
§32.2's rule is that a null renders as the reason it is null, and this was that rule applied
literally enough to bury the two fields that carry an answer. It is now a short marker beside each
field and the reason once underneath — which is the shape the guards panel next to it already used.

`cargo test -p skynetd-io -- --ignored console_fixture` is what made looking possible: it writes the
page and one answer per route it fetches — **status codes included**, because a 503 replayed as a 200
renders an error object as data and hides the behaviour worth looking at — from the real router, into
a directory any static server can serve. The console needs nothing else, which is §32.2's whole
design and now also its development loop.

Confirmed by looking: the body panel's *Signed by · bench-robotics*, the packages table's new column,
the black box panel correctly reporting a 503 as a missing recorder rather than as fields, and no
horizontal overflow at a phone width.

### Verification

| | |
|---|---|
| `cargo test --workspace` | **542 passed** across 25 binaries, 0 failed (470 before) |
| `cargo clippy --workspace --all-targets` | clean |
| `cargo fmt --all --check` | clean |
| `cargo test -p skynet-corpus` | reproduces all 363 cases byte for byte |
| `node tools/corpus-gen/generate.ts` | regenerates identically |
| `node --check crates/skynetd-io/console/console.js` | parses |
| Hub `npx vitest run lib/__tests__/skynet-os-packages.test.ts` | **29 passed** (28 before) |
| `node tools/check-mirrors.mjs` | all three shared documents match |

`skynetd-io` 92 → 122, `skynetd` 37 → 38, `skynet-contracts` 84 → 93, and the new crate 41. One of
`skynetd-io`'s is `#[ignore]`d — `console_fixture`, which writes files and is a thing somebody does
rather than a thing CI checks.

Three CI invariants added: the signer is in nothing that ships, a profile cannot assert its own
verification, and a driver does not activate without a trusted signature — the last checked from the
activation arm rather than by the function merely existing.

### What was changed in the Hub

* `lib/skynet/os/package.ts` — the fifth state, the new code, the switch arm. Byte-identical to
  `reference/typescript/package.ts` apart from the import path, as before.
* `lib/__tests__/skynet-os-packages.test.ts` — "the three signature failures" became four, plus a test
  that a stranger's valid signature is reported as neither tampering nor an unsigned package.
* `docs/skynet-os-architecture.md` — §5, §19, §28, §37.1 and §38.1. Carried across by hand; the mirror
  check reports all three identical.

### One thing lost, and how

`crates/skynetd-io/src/admit.rs` was rewritten with a whole-file write, and its test module went with
it — the file is new since the last commit, so there was nothing in git to recover from. The tests
came back out of `target/doc/src/skynetd_io/admit.rs.html`, which is rustdoc's source view and holds
the complete file. Worth writing down twice over: **rustdoc's source pages are a backup of last
resort for an uncommitted file**, and a whole-file write over a file with tests at the bottom is a
way to delete them without the compiler noticing.

### What is still zero lines

The policy runtime, the SPSC ring and observation assembly, OTA (§12), the outbound queue (§34.4),
calibration (§45), the watchdog chain (§46), POST and homing (§48), the safety MCU, and the drivers
that talk to real hardware.

Ed25519 is off that list, and with it the four things that were waiting on it. What replaced it at the
head of the queue is smaller and more specific: there is no way to **produce** a signature anywhere in
this system except the Hub, and that is what keeps the driver gate from being the install gate.

### Three findings recorded rather than fixed

1. **A robot cannot sign anything, so the gate stops at drivers.** The full §37.1 gate — no package
   activates unverified — is one design decision away, and the decision is what a developer does with
   a package they built themselves. A local developer key in the CLI is the obvious answer and it puts
   a non-constant-time signer into a shipped binary; an explicit recorded per-digest acceptance is the
   other, and it is a mechanism the specification does not currently describe. Either is a session's
   work; guessing between them is not.
2. **`signature_unverifiable` is now unreachable in this edition, and stays in the contract.** It is
   correct for an edition whose runtime has no Ed25519 (§31 permits those), and it is now documentation
   with a compiler behind it here — the mirror image of the twelfth session's finding, which was that
   three of four states were unreachable. Worth watching rather than removing.
3. **The trust store has no `skynetd trust add`.** Keys are added by editing JSON, which is fine for a
   file an operator writes once at provisioning and wrong for anything else: there is no place that
   records *when* a key was trusted or *by whom*, and §38.3 asks for exactly that of profiles. The
   store should be an audited surface before a fleet has one.

### Next

1. **The Hub side (§32.4), items 2 onwards.** Item 1 — `/skynet/os` readiness over the live registry
   — **is built** (`app/[locale]/skynet/os/page.tsx`), and this list has been carrying it as
   outstanding for several sessions, which is its own small lesson about writing a "Next" from
   memory. What is genuinely absent is everything after it: `/skynet/os/editions`,
   `/skynet/os/devices`, `/skynet/os/transport`, `/skynet/os/certification/[slug]`, the OS panel on
   the robot page, and the incident upload — which is also §38.1's missing alarm.
2. **`skynetd trust add`**, with the provenance §38.3 asks for, from finding 3.
3. **Calibration (§45)**, which has had a descriptor on the robot to write into since the eleventh
   session.
4. **The admission report as a black box event** — still open from the twelfth session, and now more
   worth having, because the report finally says something that varies.
5. **Look at the console again after the next panel lands.** Two bugs in thirty seconds was not
   bad luck; it is what four sessions of writing a page nobody opened produces, and the fixture makes
   the next look cost a minute.

## 2026-08-30 (fourteenth) — the trust store becomes a surface somebody audits

The thirteenth session's third finding: *"The trust store has no `skynetd trust add`. Keys are added
by editing JSON, which is fine for a file an operator writes once at provisioning and wrong for
anything else: there is no place that records **when** a key was trusted or **by whom**, and §38.3
asks for exactly that of profiles."* This session wrote it, and looking at the result found two
things nobody was checking.

### Provenance is recorded, never inferred

A trust entry gains `added_at`, `added_by` and `note`, and every one of them is optional — because a
store written by hand before this existed has none, and the honest report of such an entry is
**`unrecorded`** rather than a fabricated date.

The same rule decides what `skynetd trust add` does without `--by`: it records **`unattributed`**,
not `$USER`. An environment variable is not an identity, and a provenance nobody typed is exactly
the confidently-wrong record the feature exists to remove. CI greps that nothing in `skynetd-io`
reads a username out of the environment.

`Provenance::describe()` produces the four sentences, one per corner of the two optional fields, and
the fourth — *"provenance unrecorded — this key predates the audited store, or was added by editing
the file"* — is the one that made a notice worth adding: a store where **1 of 2** keys record nobody
is an entry somebody hand-edited, and **9 of 9** is a robot provisioned before any of this existed.
Those are different machines. The notice names the count and the labels.

It is a notice and not a refusal, and that distinction is the whole of it: a key nobody recorded
still verifies. The gap is in the audit, not in the security.

### The rewrite that would have deleted the bug report

`trust add` writes the file back, and the first version of that would have dropped every line the
parser could not use — which is to say **adding a key to a file with a typo in it would quietly take
the typo away.** The typo is the bug report. It is the entry an operator came to this command to
find.

So the store holds `Vec<Entry>` in file order, where an entry is either a key or a skipped line
*carrying its raw JSON*, and a rewrite re-emits the skipped ones verbatim, in place. Order is
preserved rather than reconstructed, so a rewrite is a diff somebody recognises. A skipped line's
position is the only handle it has, which is why `trust remove --at keys[1]` exists and why
`trust list` prints that exact command under every unusable row.

Three refusals, each naming the remedy rather than the rule:

| | |
|---|---|
| The same key twice | Refused, naming the label already holding it. Silently relabelling would rewrite somebody else's decision, and the case it hides — one key under two names — is the case worth seeing |
| The same label twice | Refused, naming the key. Two keys under one name make every report ambiguous about which one signed, which is the one thing a label exists to prevent |
| A store that does not parse | **Nothing is added.** Writing over it would destroy the keys it still holds, and this robot cannot tell which those are. The remedy is a person |

### `crates/skynetd-io/src/audit.rs` — the half of §38.3 the black box cannot be

§38.3 asks for two records of every configuration change — *"a black box event and an audit-log
entry"* — and they are not redundant. The black box is a fixed-size ring that **wraps**: it holds
the last few minutes at full fidelity, which is what an incident needs and precisely the wrong shape
for *"when did this robot start trusting that key"*, a question whose answer is eleven months old.

`/data/state/audit.jsonl`, one canonical JSON object per line. JSON Lines rather than an array, and
the reason is the design: **an append must not require rewriting the file.** An array has to be
read, parsed, extended and written back on every entry, so every append is an opportunity to lose
the history — and rewriting an audit log is the operation an audit log exists to make difficult. It
degrades in the right direction too: a line truncated by a power cut costs that line, not the file.
There is a test for exactly that, and one asserting a broken line is *reported* rather than skipped,
because a log with invisible gaps is worse than no log — somebody is relying on it.

Wall time, deliberately. §35.1 gives `CLOCK_REALTIME` three jobs — *"logging, scheduling,
correlation with the Hub"* — and this is the first. Nothing here is read to make a decision, so a
stepped clock spoils a record and cannot spoil a verdict; a monotonic reading would be wrong twice,
meaning nothing after a reboot and comparable with nothing on an operator's calendar. `rfc3339()` is
Hinnant's civil-from-days, checked against `date -u` on eight values including 1970, both sides of
2000-02-29, and 2024-02-29 — vectors from outside this repository, because a calendar tested against
its own arithmetic is the mistyped-constant failure the thirteenth session wrote about.

**The sequence number, and why the black box half is not written here.** The ring has exactly one
writer by construction, and `skynetd trust add` is a *different process* that may run while the
daemon is up; two writers to a preallocated ring is corruption, not a race. So each entry carries a
`seq`, the change is written to the log by whoever makes it, and the daemon mirrors anything past
its cursor. The consequence, stated rather than discovered: a black box event for a configuration
change is **delayed by up to one reconcile period**, and on a robot with no `skynetd` running it
does not appear at all — while the audit entry always does. That is the correct direction for a
record whose job is to survive.

### Two bugs found by opening the page, again

The thirteenth session's fifth item was *"look at the console again after the next panel lands"*.
Two panels landed, the page was opened, and neither of the two things it found was findable by
anything this repository runs.

**The fixture was honest and the way it was being served was not.** `console_fixture` writes one
file per route *and a `<route>.status` beside it*, with a comment explaining that a 503 replayed as
a 200 renders an error object as data. Nothing reads those files: a plain static server answers 200
with the error body, and the black box panel duly printed `null · undefined records of undefined B`
for a robot that had said **service unavailable** — the exact §32.2 violation the console is built
not to commit, arriving through the way it was being looked at. `console/fixture-serve.mjs` now
ships with the fixture and replays each recorded status; CI parses it beside `console.js`. Served
that way the same panel reads *"Service Unavailable — a daemon whose black box could not be opened
still serves stop and status, which is why this panel can be the only one missing."*

**The console pushed 378 px off the right edge of a phone, and had been doing it for six sessions.**
`main` is a CSS grid, and a grid item's default `min-width: auto` means it cannot be narrower than
its own min-content — so the packages panel, whose table of grants and buttons will not shrink below
752 px, set the width of *every panel beside it*. The status panel, which holds nothing wide at all,
hung 360 px over the edge. `main > * { min-width: 0 }` plus `overflow-x: auto` on the panel: a table
that genuinely will not fit now scrolls inside its own card instead of taking the page with it.

Worth recording that the thirteenth session reported checking this by eye and found nothing. The
difference this time was measuring `scrollWidth - clientWidth` and then bisecting `min-content` per
panel, which is the sort of thing a person does not see and a browser answers in a second.

### The admission verdict, recorded at last — and the contradiction underneath it

Open since the twelfth session: the verdict was printed to whoever ran `skynetd stage` and kept
nowhere. It is a decision about whether code may run on this machine, which makes it precisely what
an incident review asks about, and a terminal that has been closed answers nothing.

It could not simply be written to the ring, for the reason the trust changes could not: `stage` is
not the daemon. So it goes to the audit log and the daemon mirrors it, and the event vocabulary
gains two closed codes — `config_changed` and `admission` — chosen by the audit entry's action.
`admit::summary()` is deliberately one line: §41.1 keeps the black box out of the logging business,
so the *decision* lands on the channel and the findings stay in the report a person reads.

It is recorded on **every** path out of a successful stage, including the two where admission
reached no verdict at all — which is how the next thing was found, because one of those paths turns
out to be the only path.

> **`manifest.digest` cannot be this package's own version digest, and two shared documents disagree
> about whether it is.**

`reference/typescript/package.ts` says of the field: *"Version digest from the Hub. The identity that
matters."* §37.1's store layout says the version digest is *"computed by `versionDigest()` over the
manifest as a value and the remaining files as a `(path, sha256)` list."* Those cannot both hold: the
digest is a hash **of** the manifest, so writing it into the manifest changes it. Demonstrated rather
than argued —

```
manifest with no digest field  → 08f85c6cfe2fa185e1f52bf8ce00844da033fccfd86dcf868e3c848c0fe07def
the same manifest declaring that value → c40ef1ac0f5cd09b598648e6ab8023fbf25c74de007f2afc5ae11d612acfc0b2
```

— and the consequence is not cosmetic. On the robot's staging path, one of two things happens,
always. Either the field is **absent**, the common case since nobody can compute a satisfying value,
and `manifest_from_json` refuses, so **admission does not run at all**: §37.1's `verify → admit →
stage` loses its middle step in silence, and everything the twelfth and thirteenth sessions built on
that report never executes. Or the field is **present** with any value and `digest_mismatch` fires,
on every package, for ever.

Nothing caught it because `corpus/admission/*` uses `"sha256:pkg"` for both `manifest.digest` and
`observed_digest` — a placeholder that makes them trivially equal, and one that also violates the
same section's rule that a digest is *"64 lowercase hex characters and nothing else — no `sha256:`
prefix"*. **A corpus case is only as sharp as the value it pins**, and this is the second time this
project has found a check satisfied by the question rather than by the answer.

Not fixed here, and the reason is that both fixes are contract decisions: exclude `digest` from the
hashed manifest, or remove it from the manifest and let the identity travel in the desired state,
where §33.4 already puts it. Either changes the identity of every package across three
implementations and the corpus. It is recorded in §37.1 in those words and pinned by
`a_manifest_cannot_carry_its_own_version_digest`, which asserts what happens **today** — so the next
session cannot fail to see it, and the test says in its own comment that it should be deleted the day
the field becomes satisfiable.

### The first mirrored pass records a baseline, not the history

The one decision in `mirror_audit` worth defending. A robot provisioned a year ago has hundreds of
audit entries and a ring that holds minutes: replaying the log on the first pass after this ships
would **wrap the recording**, discarding the incident window in order to record a year of
configuration history, most of it about keys since removed. The two files answer different questions
and the ring's answer is the perishable one.

So the first pass writes one event — *"40 earlier configuration change(s) are in state/audit.jsonl
and are not replayed here"* — and jumps the cursor to the end. The absence of the cursor *file* is
what marks that pass, which is why reading it returns an `Option` rather than defaulting to zero: a
cursor of zero means "start from the beginning", and a robot legitimately in that state has a new log
too. A burst is capped at sixteen a pass for the same reason, so a provisioning script cannot wrap
the ring either.

### Calibration (§45) — the only safety state that drifts

The descriptor is signed and static; profiles are signed and operator-set. Calibration is measured,
it decays, and several guards are built on it, which makes it the one thing in this system that can
be **wrong without anything being broken**. §45.1 states the failure and it is not a dramatic one:
*"a camera extrinsic off by four centimetres means the `proximity` guard measures separation to the
wrong place while reporting perfectly healthy."* Nothing trips. The number is simply about the world
as it was last spring.

`skynet-contracts::calibration` is pure, like the other four: the clock and the conditions are
supplied, so the Hub can answer *"is this fleet's calibration current"* from a check-in and the robot
can answer it offline, by the same function. §45.2's table — which parameter set feeds which guard —
is **data rather than prose**, because that is what turns "we should calibrate the cameras sometime"
into "an expired extrinsic disables the proximity guard", and a test holds every guard it names to
the kernel's own vocabulary: a table naming `proximty` would disable nothing while reading as though
it disabled something.

**Five states, and the two absences that are not the same absence.**

| | |
|---|---|
| `valid` | Believed — and it carries **what was actually checked**, which is empty for a record with no policy. A console must be able to tell *still valid* from *nothing was checked*, and a state alone cannot |
| `failed` | The residual exceeds its own method's threshold. It was never a calibration, so it does not become one at any age and it does not *expire* |
| `expired` | It was one, and §45.4 has caught up with it |
| `absent` | Nobody ever measured this set here |
| `unusable` | A record exists and cannot be judged: no method, no residual, or filed under one set and declaring another |

`unusable` is `untrusted`'s cousin from §37.1 — nothing says the measurement is bad, and nothing says
it is good either — and the misfiling case is the sharpest: whichever half is wrong, some set is
being judged by another set's policy, and the guards that lose their input are the **wrong guards**.

**A missing reading expires; it does not extend.** The tempting alternative — skip the condition you
cannot evaluate — inverts the policy on exactly the robot it was written for: a machine whose thermal
sensor has failed is precisely the machine whose force/torque bias should not be trusted. In this
edition that is loud, because §47's sensor pipeline and the policy runtime are unwritten and nothing
produces a temperature or a travel count, so every such record reports expired on day one naming the
reading nothing supplies. Which is true.

### The sentence that keeps a panel from being a lie

The calibration panel was drawn, and then the page was opened, and the first thing visible was that
it sat directly beneath a Guards panel reporting **the same guards unavailable for a different
reason**. Both true. Together they invite exactly one wrong conclusion — that re-measuring the camera
would restore `proximity`. It would not: §27's kernel is unwritten, so nothing evaluates a cycle at
all.

So every surface carries the precedence, from the daemon rather than from the page: *"every guard
here is already unavailable for a prior reason … these are the calibrations that would still be
missing once they do."* It is in `GET /calibration`, in the console panel above the table, and at the
foot of `skynetd calibration`. A panel that is true line by line and misleading as a whole is the
failure §32.2 is actually about.

### The base kind decides, and looking at the fixture is what found it

The first `sets_for` said: a body that declares no capabilities gets every parameter set, because
over-reporting what is missing is the conservative direction. Rendered against the bench arm — which
declares `"base": "fixed"` and no capabilities — that produced `balance unavailable: nobody has ever
measured the IMU bias`, on a robot bolted to a table.

That is a **permanent red mark nobody can ever clear**, which is how an operator learns to ignore a
panel. The two absences are not the same absence: undeclared capabilities mean nobody said what this
body senses, and a declared `fixed` base is a *positive statement that it cannot fall over*. §45.2
says as much directly — the gravity vector drifts with base tilt, and a fixed base does not tilt. An
unknown base is mobile, because that is the first kind of absence again going the other way, and a
fixed body that does declare an IMU keeps both sets since it evidently has one.

### The corpus, and why §45.7's three cases are one corpus

18 cases across `calibration/provenance`, `calibration/expiry` and `calibration/events`, generated
from `reference/typescript/calibration.ts` and reproduced byte for byte by the Rust edition —
**381 cases across 33 files**, up from 363.

§45.7 asks for three conformance additions and they are one question asked of one input: given these
records and this moment, what is believed and which guards lose their input. Split, an edition could
pass `expiry_enforced` while disagreeing about which guard the expired set feeds — and the guard is
the half that matters to whoever is standing next to the robot.

Every number inside a message goes through `json::number_text` and `json::fixed` rather than Rust's
native float formatting, because §31.4 wants byte-identical output and `{}` on an `f64` and
JavaScript's `${value}` disagree often enough that a residual would eventually differ by a digit in a
sentence nobody reads until it is a support call.

### The Hub: readiness where the vendor already is (§32.4 item 3)

§32.4 orders the Hub surfaces by the tables they need and puts this third: *"small, and puts the
answer where people already look."* `/skynet/os` answers *where is the ecosystem stuck*; a vendor
looking at their own robot has a narrower question — **what do I publish next** — and the blockers
were already the answer to it, sitting on a page their vendor does not visit.

`components/skynet/OsReadinessPanel.tsx`, in the robot page's sidebar directly under the safety
envelope, because the envelope is the single largest input to the verdict: a vendor who has just
read *"no envelope declared"* finds the consequence in the next panel rather than on another page.
The adapter count comes from the query the page already ran.

Two decisions carried over from `/skynet/os` rather than re-argued:

* **The whole check list, not three and a count.** A fleet row has one line; a panel has room, and
  the passes are worth as much as the failures to the person publishing the next one — nine green
  and two grey is the shape of the work.
* **The vocabulary is not certification's, and the panel says so above the list rather than under
  it.** A reader who takes *attainable* for *awarded* has already misread the panel by the time a
  footnote reaches them.

Six new keys, `i18n-catalog.mjs sync` run across all 35 locales. Worth recording that sync copies
the **English text** into each catalog rather than leaving the value empty, and
`translate-with-openai.mjs` picks up "still same as English" as one of its four reasons to
translate — so the pipeline is consistent, and the keys are pending rather than done. The translate
script needs `OPENAI_API_KEY` and was not run here.

Rendered both states before believing it: a fully-declared body (certified attainable, 11 of 11) and
a bare one (not yet, 4 of 11, *"Next to publish: Published adapter"*), with every translation key
resolved against `en.json` rather than assumed. `npx tsc --noEmit` is clean; `npx eslint` cannot run
in this repository at all — `typescript-eslint` refuses TypeScript 7.0 — which is pre-existing and
unrelated to this change.

### §32.4 item 4 is not waiting on an endpoint

The thirteenth session's Next list treated `/skynet/os/transport` as waiting on "the transport
endpoint". It is not. `resolveTransport()` is already in the Hub and tested; what it needs is a
**source** — a policy's declared action channels, where the model manifest's `action_space` is an
unshaped `object` — and a **target**, an `EmbodimentDescriptor` with per-joint names, roles and
ratings. The registry carries `dof` and an actuator list *aggregated by type*, and no table holds a
descriptor at all.

So item 4 is blocked exactly the way items 2 and 5 are: on a table. Building it now would produce a
page that answers "nothing can bind" for every robot in the registry, which is honest and useless.
Recorded rather than built.

### The outbound queue (§34.4) — and §38.1's alarm, raised at last

Bounded, on disk, survives a reboot, four classes with four **different** policies. That the table
is four policies rather than one policy with four thresholds is the whole of it: telemetry is a
stream where the newest is worth most and any single item is worth nothing; a check-in is
level-triggered state, so enqueueing one removes the last; an episode is data somebody collected and
paid for; an incident is the entire reason the black box exists.

Two rules fall out and both are held by CI:

* **Nothing is evicted for something less important than itself.** Filling a disk with telemetry
  must not take out an episode, so when everything left outranks the newcomer the enqueue is
  *refused* rather than allowed to evict upwards.
* **The reserve is a floor for incidents, not a ceiling for everybody else.** A queue packed to its
  shared limit still takes an incident, and takes it *without discarding anything* — the reserve is
  not made of other people's items. An incident that does not fit even there is an error naming a
  **full disk**, never a licence to discard the previous incident.

Every discard increments a counter that lives with the queue, which is §47.4's rule applied to a
queue rather than to a ring: a robot that dropped forty thousand telemetry items says so instead of
showing a suspiciously tidy series. Collapsing a check-in is counted *apart* from a drop, because
nothing was lost.

**Full jitter, not a perturbation.** `exponential ± 10%` leaves a hundred robots that lost the same
access point retrying inside the same tenth of a window — the thundering herd smeared rather than
broken up. Uniform over `[0, exponential]` is the form that decorrelates them, and the jitter is
*supplied* rather than drawn, for §35.2's reason. Writing the test found the cap bites on the
eleventh attempt rather than the twelfth, because doubling reaches 1,024,000 ms and fifteen minutes
is 900,000 — a number worth having in a test rather than in somebody's head.

**§38.1's alarm now has somewhere to go.** A profile signature that *fails* — as against absent, or
from an untrusted key — is queued as an incident, because a signature verifying against nothing was
made over other bytes, which is what "somebody editing limits on a robot's disk" leaves behind. The
other two states raise nothing, for the reason §37.1 keeps them apart from `failed`: both are
administrative gaps, both are already visible, and an incident for either trains an operator to
close incidents without reading them. Queueing the alarm cannot fail silently either — the limits
are refused either way, so a queue that could not take it leaves a robot *safe and quiet*, which is
worse to be quiet about than the original event.

### The budget belongs to the queue, and looking at the panel is what showed it

The console panel reported a 64 MiB budget over a queue built with 4 KiB, because the route
constructed its own `Queue` with the defaults while the fixture had constructed one with its own —
two openers, two drop policies, one directory, and a console reporting a limit nothing was applying.

§34.4 says disk is reserved for incidents *at install*, and that sentence taken literally is the
fix: the budget and reserve are written into the queue's directory the first time it is opened and
adopted by every later opener. Re-sizing an existing queue is deliberately **not** a side effect of
opening it, because it changes retrospectively which items would have been dropped.

### Verification

| | |
|---|---|
| `cargo test --workspace` | **660 passed**, 0 failed (566 before) |
| `cargo clippy --workspace --all-targets` | clean |
| `cargo fmt --all --check` | clean |
| `cargo test -p skynet-corpus` | 394 cases across 35 files, all reproduced byte for byte (363 before) |
| `node tools/corpus-gen/generate.ts` | regenerates identically |
| `node --check` on both console scripts | parses |
| `node tools/check-mirrors.mjs` | all three shared documents match |
| The console, rendered at 1100 px and 390 px | no page overflow at either; every new row drawn |
| Hub `npx tsc --noEmit` | clean |
| Hub `npx vitest run` | **1159 passed** across 43 files (1150 before, over drifted contracts) |
| `node tools/check-contract-mirrors.mjs` | every contract the Hub mirrors matches the reference |
| Hub `node scripts/i18n-catalog.mjs audit` | all 35 catalogs OK |

`skynetd-io` gains 7 audit tests, 13 trust tests, 8 calibration tests, 16 outbound tests, 5 watchdog
tests, 5 POST tests and 7 API tests; `skynet-contracts` gains 29. Eleven CI invariants: a trust rewrite preserves what it could
not parse, provenance is never guessed from the environment, one panel cannot widen the console,
every calibrated set names real guards, a missing reading expires a record rather than extending
it, an incident is never dropped to make room, backoff jitter is full with the cap holding, a
coasting body is refused host and nothing else, the watchdog windows nest strictly, a self-test that
could not run is not one that passed, and the shared documents and contracts have not diverged.

Spec amended in both repositories, carried across by hand and checked identical: §19 (the audited
trust store), §26 (two event codes), §37.1 (the recorded verdict, and the digest contradiction),
§38.3 (provenance, the audit log, the mirroring), §34.4 (the four policies, the reserve, the
recorded drop, full jitter), §45 (§45.7–§45.10) and §46 (where the windows come from, the
host-shaped refusal, what the chain reports about itself) and §48 (§48.4–§48.5).

### The power-on self-test (§48.2), which is where the session's parts meet

§48.1's reason is blunt: with incremental encoders — still common on arms — joint positions at
power-on are unknown, forward kinematics is meaningless, every pose-dependent guard is unavailable,
and the robot must not be permitted to run a policy. POST is what decides whether it may leave that
state, and its table turned out to be a list of the things this session built: calibration expiry
(§45), the black box and its reserve (§28, §34.4), and the watchdog nesting (§46.3).

**Four outcomes, and two of them are not failures.** §48.2 gives a check two answers; it needs four.

| | |
|---|---|
| `pass` | The check ran and this robot is good on this point |
| `fail` | The check ran and it is not. §48.2's `SAFE`: sticky, needs a human |
| `unavailable` | The input does not exist. Nothing is broken and nothing was established |
| `elsewhere` | Not this daemon's check to run — see below |

`may_run` is true only when **every** check passed, and the verdict for the rest is
`post_incomplete` rather than `safe`, because a robot whose checks could not run does not need a
human. It needs the rest of the operating system. Collapsing `fail` and `unavailable` would produce
the two worst sentences available: *"self-test passed"* on a robot that tested almost nothing, or
*"self-test failed"* on a robot with nothing wrong with it.

Two rows are worth their argument. **An expired calibration does not fail POST** — §45.5 degrades
rather than refuses, and a self-test that refused to boot on a stale camera extrinsic would be a
robot nobody could get into a state where they could recalibrate it; what the row does is make the
list of starved guards known at power-on rather than at the first command. **An unreachable watchdog
layer does fail it**, because §46.3's inversions mean a layer cannot fire, and booting past that is
exactly what "before anything moves" is for. An *undeclared* window does not: nothing contradicts,
and a body nobody has measured is not a body that is wrong.

#### Half of §48.2's table is not the supervisor's to answer, and that falls out of §33.1

The structural finding. `skynetd` never touches an actuator — that single property licenses
everything else about the daemon, and CI already asserts it cannot even take a dependency on the
RHAL. Five of §48.2's eight checks need precisely what that denies: the RHAL ABI, `describe()`, the
estop assert-and-observe, encoder sanity, and the kernel's corpus self-check.

So they are reported as `elsewhere` — **not** as unavailable, which would be a different and wrong
sentence. A POST that silently omitted them would be a supervisor claiming to have proved a stop
path it is forbidden from touching; one that called them unavailable would suggest they might become
available here. They will not. Every row still carries what a failure would *mean*, which is what
makes an `elsewhere` row worth showing rather than hiding.

The homing procedure itself (§48.3) is in the same position and is not built: it moves the robot,
through the RHAL extension channel, which is the one thing this process may not do.

### The watchdog chain (§46)

Four layers, and the one that carries the argument is layer 3: the SoC hardware watchdog is petted
by the **Safety Kernel**, not by `skynetd`. A watchdog petted by a supervisor tells you the
supervisor is alive, which is not the question anybody is asking.

**The refusal is host-shaped, and that is unusual.** §46.2 says a body whose drives *coast* on
command loss cannot safely reach the host tier, because layer 3's own recovery — reset the processor
— is on that machine a fall. Guest is fine: enforcement is advisory and nothing relies on a hardware
watchdog. Certified is fine: layer 4 cuts drive enable independently of the reset. **Only the middle
rung is unsafe**, which a tier ladder that caps downward cannot express, so it is a predicate rather
than a ceiling — and CI asserts it stays one, because rewriting it as a cap would silently permit
exactly the configuration it exists to refuse.

**Silence is not `hold`.** A body that declares nothing is refused host for the same reason a
coasting one is: the recovery path has not been characterised. Treating an absence as the most
convenient value is inventing an answer to the only question layer 3 turns on. A value outside the
vocabulary gets its own sentence, because *"this body says `freewheel`"* and *"this body says
nothing"* are different facts with different remedies.

The nesting (§46.3) is four windows and two of them are new descriptor fields, because a board's
watchdog window and a drive's command timeout are the only two things in the nest that this
operating system cannot read for itself. Three decisions in the check:

* **An undeclared window is not a passing one** — reported as missing, and *stepped over* rather
  than ending the walk, because the pair either side of a gap still inverts and that inversion is
  real whatever sits between them.
* **Two booleans, not one.** `ordered` means nothing contradicts; `complete` means every window is
  declared as well. One boolean would have to lie about the other, and the lie it tells — "sound"
  over a chain nobody has measured — is the one §46.3 exists to prevent.
* **Strictly less than.** Equal windows are a race decided by interrupt latency, and a chain whose
  order depends on which timer fires first is not a chain.

Each inversion carries its own code *and its consequence*, because those are the half worth reading:
"the processor resets before the kernel executes its controlled stop, so the robot never performs
its best available behaviour" and "the drive faults first, so layer 3 is present, configured, and
never reached" are different bugs.

13 cases in `watchdog/nesting` and `watchdog/command-loss` — **394 across 35 files** — and the Rust
and TypeScript editions agreed byte for byte on the first run.

The certification half needed nothing: `safety.command_loss.behaviour` is already an SCTS test
required at host, and §46.2 already says the field is *measured, not trusted*. What was missing was
the declaration to measure against.

#### Alphabetical order on a panel about order

The watchdog panel rendered its four windows as `drive_command_timeout_ms`, `envelope_watchdog_ms`,
`kernel_cycle_worst_case_ms`, `soc_wdt_ms` — alphabetically, on a panel whose entire subject is
whether those windows *nest*. Canonical JSON sorts an object's keys, and `declared` was an object.

A reader cannot see a nest that is not in nesting order, and a table that presents one order while
discussing another teaches the wrong one. It is now an array, in all three implementations, and the
inversion is visible as a break in the sequence: 100 ms, then 40 ms, then 500 ms. Found by looking
at the page, which is now three sessions running.

### The worst thing found this session: the Hub's contracts had drifted six sessions

Found by going to add a field to the embodiment descriptor and diffing the three copies first.

`lib/skynet/os/safety.ts` — the Hub's copy of the **Safety Kernel** — had no pose guards, no
`PoseTrip`, no `PoseOutcome` and no `withGrants`. `embodiment.ts` had no collision shapes, no
`validateFrameBindings` and no `capabilities` on the descriptor. `conformance.ts` was missing an
SCTS test **required at host** (`embodiment.capabilities.declared`), which means
`awardCertification()` on the Hub would award a host badge to a body the robot refuses.
`kinematics.ts` did not exist there at all.

Git says why, and it is not a mystery: `reference/typescript/safety.ts` was last written in the
seventh session's *"the frame graph, consumed"*, and `lib/skynet/os/safety.ts` has been touched
exactly once, by the commit that created it.

**Nothing could see it.** §31.4's corpus holds the Rust edition and the TypeScript oracle to each
other on every run, byte for byte, 381 cases — and the Hub's copies are outside it. The Hub's own
suite passed the whole time, 1150 tests, because it was asserting the behaviour of the contracts
that repository happened to hold. `check-mirrors.mjs` covers the three shared *documents* and has
been run every session; there was no equivalent for the code, so the documents stayed identical
while the implementations of them drifted apart.

That is the failure §31 exists to prevent, arriving through the gap between two checks rather than
through either of them.

#### Repaired, and then made mechanical

`tools/check-contract-mirrors.mjs` compares all eight modules, normalising the **one** legitimate
difference — the import specifiers, since the reference is standalone with `.ts` extensions and its
own `types.ts` while the Hub resolves through its bundler and takes `SafetyEnvelope` from its own
object model. Normalising anything more would be a check that cannot see a divergence in the code.

Its first version reported the two-line drift in `conformance.ts` as **247 lines**, because it
compared line by line at the same index and every line after an insertion counts as changed. A
number that wrong in that direction is a number somebody learns to ignore, so it counts a multiset
difference instead and now agrees with `diff`.

Two checks, because they catch different things and only both together close it:

* **`check-contract-mirrors.mjs`** catches *the reference having moved ahead*, and can only run
  where both repositories are checked out. Wired into CI beside the document check, where both are
  no-ops when BotMarket is absent — which is the case in that workflow today and the reason they
  cost nothing to have.
* **`lib/__tests__/skynet-os-mirrors.test.ts`** catches *the Hub's copy being edited in place*,
  which is what a well-meaning fix to a type error in a Next.js application looks like. It hashes
  each file against a manifest the sync writes, runs in the suite that already runs, and was
  confirmed to fail on a one-line edit and pass again when it was reverted — a check nobody has
  seen fail is a check nobody has tested.

#### Two things the sync then broke, both worth having broken

**`kinematics.ts` needs BigInt literals** — its `sqrt` seeds Newton from the exponent field of an
IEEE-754 double — and the Hub targeted ES2017. Bumped to ES2020. The alternative was forking the
one file that must not fork away from the oracle every edition is measured against.

**The Hub's safety test passed `solvers` and `collision_model`**, two flags §44's frame graph
deliberately removed: both were claims about the body that nothing checked, and a descriptor
asserting `collision_model: true` with no geometry produced a guard reported as running with
nothing to evaluate. The test now supplies an actual frame tree with two collision bodies and a
workspace volume — which is the work that was skipped six sessions ago when the file was not
carried across, and precisely the work that would have caught the drift then.

Hub: **1159 tests across 43 files**, `tsc --noEmit` clean. Rust: unchanged at 639.


### What is still zero lines

The policy runtime, the SPSC ring and observation assembly, OTA (§12), the safety MCU, and the
drivers that talk to real hardware.

§48's **homing** is on that list and belongs there rather than in a gap: it moves the robot through
the RHAL extension channel, which is the one thing §33.1 forbids this process. §48's self-test is
built, and reports five of its eight checks as somebody else's for the same reason.

§46's chain is off the list as a *contract* — the layers, the nesting, the command-loss rule — and
two of its four layers are still hardware nothing in this edition can see, which every surface now
says rather than implies. The outbound *queue* is off; the outbound **path** is not — nothing sends from it, because
§34.2's transport and §10's pairing are unwritten, and every surface that shows the queue says so
rather than presenting a queue that only grows.

### Findings recorded rather than fixed

1. **`manifest.digest` cannot be this package's own version digest**, and two shared documents
   disagree about whether it is. Demonstrated, pinned by a test that asserts today's behaviour, and
   recorded in §37.1. The fix is a contract decision with two defensible answers, either of which
   changes the identity of every package across three implementations and the corpus.
2. **`skynetd once` records nothing.** The reconcile loop writes pass events and mirrors the audit
   log; `once` does neither, so a robot operated entirely through `once` — which §33's own
   documentation calls *"a legitimate way to operate a robot"* — has a black box holding only
   `recording_started`. Whether a one-shot pass belongs in the incident record is a real question and
   the current answer is an accident rather than an answer.
3. **`/skynet/os/transport` is blocked on a descriptor store, not on an endpoint.** See above. The
   same table unblocks §32.1's `/skynet/os/devices/[id]`, which wants "active and unavailable guards
   with the missing input named" — which is now a thing the robot can actually answer, since
   calibration produces exactly that list. It is also why `calibration.ts` and `watchdog.ts` are
   deliberately **not** in the Hub's mirror list yet: two modules nothing imports, waiting on the
   same table. The mirror tool says so in the list rather than leaving it to be rediscovered.
4. **Nothing consumes the outbound queue.** Items accumulate under the drop policy for ever. That is
   correct today and it means the drop counters are the only evidence the queue is working, which
   makes them worth watching rather than worth trusting.
5. **`npx eslint` cannot run in the Hub at all** — `typescript-eslint` refuses TypeScript 7.0. It is
   pre-existing and unrelated to anything here, and it means `tsc --noEmit` and the vitest suite are
   currently the only static checks that repository has.
6. **The two mirror checks only meet where both repositories are checked out.** The CI job that runs
   them has one, so both are no-ops there; the real coverage is a developer with both trees and the
   Hub-side digest test. Closing that properly means a job that checks out both, which needs a token
   this session cannot arrange.
7. **`lib/skynet/os/mirror-digests.json` is written by a tool in the *other* repository.** That is
   the right direction — the reference is the origin — and it means a Hub-only change to a contract
   cannot be made honest without the OS repository present. Recorded because it is a workflow
   constraint somebody will hit and mistake for a broken test.

### Next

1. **The descriptor store on the Hub** (`skynet_embodiments`, or a column on the robot). It is the
   single largest unblock left on that side: §32.4 items 4 and 5 both wait on it, and the robot can
   already produce and sign a descriptor (§5).
2. **A sender for the queue**, even a stub one behind a flag, so the drop counters stop being the
   only evidence. §34.3's bearer classes are the interesting half — bulk waits for an unmetered
   link, and the wait is visible rather than silent.
3. **The three POST checks the kernel owns**, which arrive with §27 rather than before it: the
   RHAL ABI, `describe()`, the estop assert-and-observe, encoder sanity and the corpus self-check.
   The report already has their rows and says who they belong to.
4. **`skynetd once` and the record**, from finding 2.
5. **Look at the console again after the next panel lands.** It has now found something on three
   consecutive looks — a 503 rendered as data, 378 px of overflow, a budget nothing was applying,
   and four windows in alphabetical order on a panel about their order — and the fixture plus
   `fixture-serve.mjs` make the next look cost about a minute.

---

## 2026-08-30 (fifteenth) — the command path: a frame that never existed, and the ring it crosses

§25 is the one section of Part II that had no code at all. It is two things — an **observation
assembler** on the way in and an **SPSC ring** on the way out — and they had been on the "still zero
lines" list since the first session, which is a reasonable place for them to have been: they sit
between a policy runtime and a kernel host, and neither of those exists yet.

They came off it this session for a reason that has nothing to do with either of those programs.
§25.1 describes a failure that **does not announce itself**, and a failure that does not announce
itself is not made safer by waiting until there is something to run it against.

> A policy is trained on synchronised frames: image and joint state as of one instant. A real robot
> has three cameras at 30 Hz with 20–60 ms of pipeline latency, joint state at 1 kHz with 1 ms, and
> an IMU at 200 Hz — none of them aligned, none of them stamped the same way. Feed a policy the most
> recent value from each and it receives **a frame that never existed**, in which the arm is where it
> is now and the image shows where it was two frames ago. **Policies degrade in ways that look like
> bad training.**

The last sentence is the expensive one. A skewed frame presents as a policy that grasps slightly
late, or oscillates on contact, or works on the bench and not on the robot — and every one of those
sends somebody to retrain a model that was fine.

### The descriptor field §52 had been owed since Part V

`sensors[].latency_ms` is in §52's table of descriptor additions, measured by `rhal.sensor.latency`,
which was already an SCTS test with its own sentence about backends that stamp arrival rather than
capture. The field itself did not exist in either edition. The assembler cannot be written without
it — §25.1's whole mechanism is "declared per-sensor latency, so the assembler knows how far back a
camera's *now* actually is" — so it went in first, nullable, in Rust and in the TypeScript reference,
and out to the Hub through the mirror.

Nullable, because §7.2's rule holds here and breaking it here would be invisible. **Zero is a claim
no imaging pipeline can make**: it says a sample is available the instant the event happens. A stream
that has not declared a latency is assembled at its raw stamp and *named in the frame*, and the
frame's skew bound goes to null rather than pretending the omission cost nothing.

### The assembler, and the four answers it has to keep apart

`skynet_contracts::observation::assemble` and `assembleObservation` in the reference. For each stream
it corrects every sample by the declared latency, picks the one nearest the target instant, and
reports what accepting it cost; then it measures the spread across streams, which is the frame's own
internal inconsistency and the number the whole section exists to produce.

Four outcomes, and collapsing any pair of them would produce a sentence that sends somebody to the
wrong place:

* **stale** — over the policy's declared budget. §25.1: *"Exceeding it is a `hold`, not a silently
  older frame."* A hold.
* **no sample** — the stream produced nothing. Also a hold, and a different code, because an empty
  capture path and a slow one are different repairs.
* **undeclared latency** — the body never said. Not a hold; nothing is broken. The measured skew is
  still reported and `skew_upper_ms` is null, because the true skew is larger by an unknown amount
  and a bound that claimed otherwise is worse than no bound.
* **undeclared budget** — the *package* never said. Also not a hold, and named separately from the
  one above, because the gap is in a different declaration and the remedy is a different person.

Three decisions in it are worth recording because each had a plausible alternative:

**The budget is one-sided.** A chosen sample from *after* the target instant gives a negative age and
is not reported as staleness. It cannot happen on a live robot and happens routinely in replay, and
calling it stale would send somebody to look at a capture pipeline that is working. The age is
signed so the evidence survives; clamping it at zero would erase the only trace of a clock that ran
backwards.

**A tie takes the earlier sample.** Two samples equidistant either side of the target are equally
good by the distance rule and are not equally good in any other sense: the earlier describes
something that had already happened at the instant the frame claims to be. The tie-break also has to
exist for a duller reason — without it two editions iterating a capture ring in different orders pick
different samples, and the corpus diverges on an input where nothing is wrong.

**One contributing stream has no skew rather than zero skew.** The spread of a single instant is
undefined, and zero would let a frame with one working stream and three empty ones look perfectly
synchronised — which is the presentation §25.1 is written against.

There is also `latest_common_instant`, which answers the question §25.1 raises and does not settle:
*which* instant should a live loop target. Aim at *now* and the slowest camera is the stalest stream
in every frame for ever — a problem the loop created and the assembler would faithfully report. It
is offered rather than imposed, and it is in the corpus, because an edition could reproduce every
assembled frame and still pick a different target every cycle: a disagreement about every frame in
the field and none in the corpus.

### The corpus: 458 cases across 38 files, and the fuzz that was not looking

`observation/frames` (12 curated), `observation/target` (4) and `fuzz/observation` (48). The Rust
edition and the TypeScript oracle agreed byte for byte on the first run.

The curated case worth reading is `synchronised`, because it demonstrates the **inversion** rather
than a difference in width. By the stamps alone the camera is the freshest thing in the frame, 15 ms
ahead of the joint sample; once each stream is moved back by its own declared latency, the camera
turns out to describe the earliest instant in it. Taking the latest value from each stream would have
built the frame around the stalest one while believing the opposite, which is §25.1's failure stated
as arithmetic instead of as prose.

The fuzz generator's first version drew its staleness band **per sample**, so a stream with three
samples nearly always held one fresh enough to be the nearest, and forty-eight generated frames
produced three stale ones. That is the blind spot the corpus README already warns about, arriving in
a new corpus on the day it was written. The band is now drawn per stream, and the generator prints
its own coverage next to the file counts —

```
fuzz/observation coverage: ok 28, hold 20, stale 14, no_sample 8,
  undeclared_latency 18, undeclared_budget 11, negative_age 9, tie 28
```

— so a change that flattens the distribution is visible when it is made rather than three sessions
later.

### A trap in the corpus runner, found by adding a corpus to it

`fuzz` was dispatched as `"cycle" | "fuzz"` — a catch-all. So `fuzz/observation` did not arrive as an
*unsupported corpus*, which is the one thing that runner reports loudly and refuses to skip; it
arrived as forty-eight cycle vectors with no `limits`. The run still failed, so nothing shipped
wrong. It named the wrong problem, which on a quieter day is how a whole corpus comes to be
dispatched into an arm that was never written for it. Every arm now says which files it is for.

### The ring, and the `unsafe` that did not happen

`crates/skynet-ring`: 4 KiB slots, a per-slot seqlock, latest-wins, and no method that can block.

Every crate in this workspace carries `#![forbid(unsafe_code)]` except the two that cannot — the ABI
and the driver loader — and a lock-free structure shared between two processes is the obvious place
for that streak to end. It does not end here. The shared region is modelled as **`&[AtomicU32]`**,
which has interior mutability, so both ends read and write it concurrently with the memory model
spelling out what each observes and with no `unsafe` anywhere. What genuinely needs it — turning an
`mmap` of a `memfd` into that slice — is one call, and it belongs to the mapping layer rather than to
the protocol. The protocol is the part where a mistake is a moving robot, and it is the part the
compiler checks.

It also makes the thing testable on any platform: the suite backs a ring with an ordinary array and
is exercising the identical arithmetic a shared mapping would.

**Latest wins, and it is not a queue.** §47.4 draws the distinction for sensor frames — *"a queue in
front of a policy is latency wearing the costume of reliability"* — and it holds one step later for
the commands a policy produces. A kernel draining a backlog would be executing a trajectory planned
for a robot that has since moved. The several slots exist so a producer at full rate cannot overwrite
the slot a consumer is mid-read of; they are not pending work.

**Nothing blocks.** `read` returns `Stale` after four attempts and leaves the caller's buffer
untouched, so the kernel runs its cycle on the command it already holds and §7.1's watchdog guard
sees a sequence that has not advanced. Four is a statement rather than a knob: each attempt fails
only if the producer published a whole command during the copy, and four consecutive failures is a
misconfigured rate rather than a race worth waiting out.

#### Two bugs the tests caught, both about zero

**`published == 0` is not "nothing has been sent".** It is a wrapping `u32`, so it means that on the
first cycle and *four billion commands* on a robot that has been up about seven weeks at a kilohertz.
The first draft returned `Empty` on it. Emptiness is now asked of the **slot** — a seqlock of zero
means nothing has ever been written there, which is true before the first publish and false ever
after — and the producer steps its counter *over* zero rather than onto it so that stays an invariant
rather than an approximation. Both are pinned by tests that drive the counters to the top of a `u32`
by hand.

**A shorter command must not inherit the previous one's tail.** The slot is reused, so publishing a
2-channel command into a slot that held an 8-channel one leaves six values above it. Today's consumer
never looks past `channels`, and "never looks" is a property of today's consumer while the bytes
outlive it. The whole fixed array is written every publish.

#### The test that was made to fail before it was trusted

`a_reader_racing_a_writer_never_sees_a_torn_command` runs a producer and a consumer on two real
threads over a **one-slot** ring, so the producer overwrites the only slot on every publish and the
consumer races it on every read. The invariant is self-consistency, not delivery: missing a command is
the design working, and every field of a command that *is* delivered is derived from its sequence
number and checked against it.

Every other assertion in that file stages a race by hand, which proves the arithmetic and proves
nothing about the memory ordering — a single thread observes its own stores in program order whatever
orderings are written. So the tear detection was deleted and the test run again; it failed, and was
restored. A check nobody has seen fail is a check nobody has tested.

And the slot fitting inside its page is now a `const` assertion rather than a runtime one. Three
constants decide it, each plausible to change alone, and the failure of getting it wrong is a publish
that writes into the head of the *next* slot — a torn command under a perfectly valid seqlock, which
is the one corruption this format's reader cannot see.

### The `memfd` under it, and the third crate allowed to use `unsafe`

`crates/skynet-ring-shm`. §25 asks for "a `memfd`-backed mapping" and `skynet-ring` deliberately
stops one step short of it; this is the step. Four `extern "C"` declarations, one `mmap`, one
`slice::from_raw_parts`, and a `Drop` that unmaps and closes. That is the whole crate, and the size
is the point: a reviewer asking whether the command path is memory-safe reads a short file, because
everything that could move a robot wrongly is in the protocol next door where the compiler checks it.

**Who creates it settles three things at once.** The runtime creates the ring and the sandboxed
policy receives the descriptor — never the other way round — and each of the three properties that
follows is a thing an untrusted policy would otherwise be able to do:

* **The geometry is not the policy's to choose.** Written and validated before the fd is handed over,
  and `Ring::open` caches the slot count at that moment, so a policy rewriting the header afterwards
  moves nothing on the reading side.
* **The file cannot be shrunk.** `F_SEAL_SHRINK` and `F_SEAL_GROW` are added before the fd leaves the
  process, with `F_SEAL_SEAL` so the set cannot be reduced later — a seal that can be lifted is a
  comment. Without it a policy inside the sandbox could `ftruncate` the mapping to zero and make
  every subsequent read by the runtime a `SIGBUS`: **untrusted code killing the supervisor with one
  syscall.**
* **The runtime cannot write it.** Its mapping carries no `PROT_WRITE`, so "the policy is the only
  writer" is enforced by the page tables rather than agreed between two programs, one of which is
  hostile.

What a hostile producer can still do is put nonsense in a slot — a wild `channels`, a stalled
sequence, a seqlock that never goes even. None of it is memory-unsafe on the reading side, and all of
it is *a command*, which is what §7's guards are for. This crate's only job is to make sure a bad
command is the worst thing that can arrive.

Two smaller decisions worth recording. `off_t` is declared 32-bit on a 32-bit target, because glibc
compiles it as `long` unless `_FILE_OFFSET_BITS=64` is defined and 32-bit ARM is a live robot
controller — an `i64` there sizes a file to something nobody asked for. And the four syscalls are
declared rather than depended on: a bindings crate would be the first non-vendored dependency in a
workspace whose absence of them is a property a vendor can check with `cargo build` and no network,
and four declarations are visible in a diff in a way a transitive dependency is not.

#### It is tested where it runs

Windows cannot run any of it, so the crate returns `Unsupported` there rather than failing to
compile, and the Linux path was exercised in a container — `rust:1-slim-bookworm` over the checkout.
Nine tests, and the two that matter are the ones a single mapping cannot fake:

* **`a_command_crosses_two_independent_mappings_of_one_file`** — two `mmap`s of one `memfd` are the
  same memory, so a store through one is a load through the other.
* **`a_command_crosses_two_processes`** — the same claim across a process boundary, which is the one
  §25 actually makes: the policy is in a **sandbox**, and everything above proves only that a process
  can talk to itself. The child is this test binary re-executed with the descriptor duplicated
  without `FD_CLOEXEC`; the production flag stays, because a ring leaking into every child a runtime
  spawns is exactly the accident `MFD_CLOEXEC` prevents. It was made to fail — the child's expected
  sequence changed by one — and the parent reported `exit status: 101` before it was trusted.

The seals are tested by trying to break them: `ftruncate` on the sealed fd returns `EPERM`, and so
does a second `F_ADD_SEALS`.

#### And the unsafe check now points the other way

CI listed the crates that must carry `forbid(unsafe_code)`. That is the same check with its default
inverted: a crate added to the workspace joins nobody's list and is watched by nothing. It now
enumerates `crates/*` and requires the attribute of everything outside a three-name allow-list —
`skynet-rhal`, `skynet-cli`, `skynet-ring-shm` — and requires each of those three to actually contain
`unsafe`, so a crate that no longer needs to be on the list comes off it.

### §25.1's last bullet, and the two things a record cannot say

> The frame's skew and per-stream ages go into the black box, so "the policy behaved oddly" can be
> checked against "the frame it was given was 80 ms skewed" months later.

The record already existed — `/sk/frame`, channel 5, written in the sixth session with `target_ns`,
`worst_skew_us` and a per-stream age. What did not exist was anything that could produce one. So the
frame now projects onto it, in both editions, inside the frame's own corpus output rather than beside
it: an edition could otherwise agree about every assembled frame and disagree about the only version
of it that survives the day.

Writing the projection is what surfaced the interesting part, which is that **the record could not
say two things the assembler is careful to distinguish** — so the record format is now version 2.

* **A stream that produced nothing had no row and no count.** The record carries a row per
  *contributing* stream, so a frame with one working camera and two dead capture paths recorded one
  stream and said nothing about the rest. Read months later, that is a robot with one camera. The new
  `streams_absent` carries it, and it is deliberately **not** the existing `streams_omitted`, which
  means "streams the record could not fit": one is a limit of the format and the other is a limit of
  the robot, and an investigation reading one as the other goes looking for a bigger record instead
  of a dead camera.
* **`worst_skew_us` had no spelling for "not measurable".** The frame reports null when fewer than
  two streams contributed, because the spread of one instant is undefined; an unsigned field has to
  write something, and the something it wrote was zero — a frame with three dead capture paths
  reading as a perfectly synchronised one, which is exactly the sentence §25.1 exists to prevent.
  `SKEW_UNMEASURABLE` is `u32::MAX`, seventy-one minutes of skew, so nothing real collides with it,
  and the two facts "no skew at all" and "no skew measurable" stay two facts. Both are asserted.

That is a **format change to something §31.2 makes single-source**, so it was made as one: the
record layout, both editions' encoders and decoders, the MCAP channel schema in both, the corpus, and
`FORMAT_VERSION` from 1 to 2 — which exists for precisely this and had never been used. The stream
table moves four bytes, and the version is what stops a version-2 daemon reading a version-1 ring as
though the fields were where it expects them: an incident report that is confidently wrong is worse
than no incident report. Nothing has shipped, so there are no version-1 rings; the bump is the
mechanism working rather than a migration.

The corpus grew a case for it — `blackbox/frame-with-a-dead-capture-path`, the record version 1 could
not write — and the sealed-file digest changed with it, which is how the two editions were confirmed
to agree about the new bytes rather than only about the new fields.

**The third loss is real and stays**, and is asserted from both sides so that it remains a decision
rather than an oversight: `age_us` is unsigned, so a sample from *after* the target saturates at
zero. It cannot arise on a live robot — every target a runtime picks is at or before now, and
`CLOCK_MONOTONIC_RAW` does not step — and the signed figure stays on the frame for everything that is
not the recorder. The corpus shows both side by side, `"age_ms":-4` on the frame and `"age_us":0` in
the record.

One thing worth recording about *how* this went in: the first version of the schema strings reached
the file with their Rust line-continuations eaten, so three descriptions carried twenty-space runs
inside them. Nothing caught it except the corpus, which compared the Rust schema against the
TypeScript one byte for byte and pointed at the offending column. A schema is a document somebody
reads, and the corpus turns out to proof-read it.

### `skynetd once` recorded nothing, which was two call sites rather than a decision

Finding 2 from last session, closed. The reconcile loop recorded every pass and mirrored the audit
log; `once` did neither, so a robot operated entirely through it — which §33's own documentation
calls *"a legitimate way to operate a robot"* — had a black box holding `recording_started` and no
evidence of one thing it had done afterwards.

Last session recorded it as an open question about whether a one-shot pass belongs in the incident
record. Looking at the code, it is not a question anybody answered: the two paths were two call
sites, the loop grew a recorder, and the other one did not. Nothing chose.

So they are now **one function**. `converge_and_record` reconciles, records each pass on §26's event
channel and mirrors §38.3's audit log, and both entry points come through it — which is the fix for
the drift as well as for the gap, because there is now one place either behaviour can change. It
stopped being `#[cfg(unix)]` on the way, and the reason it had been is the reason it was missing:
only the daemon loop called it, and the loop needs a Unix socket, while `once` is plan-and-apply over
a directory and runs anywhere.

`e2e.sh` asserts it at the point in the script where **the daemon has not started yet**, so anything
in the ring there was put there by `once` and by nothing else. It was made to fail — the recorder
replaced with `None` — before it was trusted.

### And the end-to-end run had been red on a wording change

Found by running `e2e.sh` in a container to check the line above. One assertion was failing, and it
had nothing to do with this session:

```
✗ and the signature is reported as unchecked, not as signed - expected 'not checked' in:
    signature: absent — nothing accompanied these bytes to say who produced them
```

§19's staging report *used* to say a signature was "not checked". The trust store landed two sessions
ago and it now names the state the signature is actually in — `absent`, `untrusted`, `invalid` or
`verified` — and `admit.rs` has a test whose documentation says so in as many words. The end-to-end
script kept grepping for the old phrase, so it has been failing ever since, on a wording improvement,
while everything it exists to check was fine.

It is now two assertions instead of one, and the split is the point: the first names the state that
is true here, and the second checks separately that **nothing claims the package was verified**. The
second is the half that matters, and it is the half a grep for one phrase can lose again the next
time somebody improves the phrasing.

### A sender for the queue, and §34.3's bearers

Next item 2 from last session, and the half it called interesting. §34.4's outbound queue has been
complete for two sessions and nothing had ever taken an item out of it — so the drop counters were
the only evidence it worked, and *"nothing sends from this queue yet"* was a sentence in the payload
rather than a mechanism anybody could look at.

`connectivity.rs` is the decision half. §34.3's table has three rows and **one rule**:

> An unmetered bearer carries everything. A metered one carries incidents and check-ins.

The rows differ only in what each kind *assumes* about metering when nobody has said — ethernet no,
Wi-Fi no, cellular yes, anything unrecognised yes. Written as one rule rather than three, "Wi-Fi in a
hotel, marked metered" behaves exactly like cellular, which is what §34.3's *"everything unless
marked metered"* asks for and what a table read row by row would not give.

The uncomfortable part is correct and is asserted as such: **an incident is on the list of things
allowed over the most expensive link a robot has**, and it is the biggest payload in the queue. §34.4
settles it — *"there is nothing in this queue worth less than an incident"* — and a fleet that
discovers the recording of its worst day was waiting for Wi-Fi has saved a few pounds and lost the
thing the black box exists for.

**Declared, then observed**, the same shape as §33.4's safe state. Which interface is the modem is a
fact about the machine that nothing can infer — `/sys/class/net` will not say whether `wwan0` is a
modem or a dongle somebody left plugged in — so the bearers come from `state/bearers.json` and only
their liveness is read from the kernel. A robot with no such file reports that it has no bearers
rather than inventing one.

**The sink is a trait with two implementations and neither is a transport.** §34.2's outbound path is
HTTPS to an endpoint this robot was paired with in §10, and §10 is unwritten: no identity, no
attestation, no endpoint, and no TLS stack in a workspace with no dependencies. `Unpaired` is the
default and refuses everything with that sentence; `Directory`, behind `--outbox <dir>`, writes to
this machine so the drain path — pick, decide, send, remove, back off — is exercised by something
rather than by nothing. Both `skynetd` and `skynetd once` drain through it.

Two decisions worth recording. The drain **stops at the first item §34.3 will not carry** rather than
skipping past it to a lower-priority one it could: the queue is in priority order and routing around
a wait would quietly invert §34.4's table. And the retry schedule lives **in memory** rather than on
disk, because a backoff deadline is a monotonic instant (§35.1) and a monotonic instant survives a
reboot as a meaningless number — so a restart makes every item eligible again, which is also the
right behaviour, since a reboot is a change in the world. The attempt *count* does persist, so the
backoff a restarted robot computes is the one its history earns.

### Looking at the console found two things, which is now four sessions running

The panel went on, and then the page was opened.

**Every bearer reported as `down` on a machine that cannot measure one.** `observed_up` read
`/sys/class/net/<if>/operstate` and returned `false` when the file was not there — so a developer's
laptop, and a robot whose modem is managed outside `/sys`, reported a *measurement nobody took*. It
is now `Option<bool>`: the kernel's answer where there is one, a declared `up` where there is not,
and `false` only when neither exists. Every row says which of the three it is showing, and the
"nothing is up" sentence no longer claims to be the machine's own answer when it is not.

**The panel said the queue was fine on a robot that will never send anything.** §34.3's verdict —
*"the active bearer carries this class"* — was concatenated with the sink description, and on an
unpaired robot the result read:

```
Sends to: none — this robot is not paired with a Hub (§10). the active bearer carries this class
```

Two true statements rendering as one claim that nothing is wrong. They are two different questions —
*will this link carry this class*, and *is there anywhere to send at all* — and a panel that cannot
keep them apart will always be reassuring about the wrong one. `Sink::obstacle()` now answers the
second, the payload carries both, and the console renders them as separate sentences with the
obstacle in the warning colour.

While there: a verdict that said only "yes" forced its caller to reconstruct the question, and the
console reconstructed it badly — *"the active bearer carries this class"* is a sentence whose subject
is not on the page. `Dispatch::Send` now carries the class and the bearer, and reads *"the next item
is incident and `wlan0` carries it"*.

### A string with a hole in it, three times

Rust joins a literal across lines with a trailing backslash, which also eats the next line's
indentation. Lose the backslash and the indentation stays inside the string: the compiler is happy,
every test passes, and an operator reads

```
...has no Hub to send to.              §10's pairing and attestation are unwritten.
```

It happened twice this session — in an MCAP channel schema and in that console message — and a sweep
then found a third that had been sitting in `skynetd-io/src/auth.rs` for four sessions, in the
sentence a robot prints when it cannot issue a credential. The corpus caught one of them, because it
compares two editions' schema bytes and pointed at the column. Looking at the page caught another.
**Nothing was watching for the third**, and nothing would have.

There is now a CI check: any string literal over eighty characters, excluding embedded JSON, with a
run of ten or more spaces in it. Its first version matched the *source line's* indentation and fired
on forty perfectly good sentences — a check in the shape somebody switches off — so it extracts the
literal with `grep -o` first. It was made to fail and then to pass again before it was trusted.

The cause is a tooling one and it is recorded outside this repository, because it will happen again
in any file: this session's shell mangles backslashes inside heredocs, so patch scripts containing
them have to be written to a file rather than piped.

### The worst thing found this session: both mirror checks were failing on every Windows checkout

Found by running `check-contract-mirrors.mjs` after a one-field change and being told **six contracts
had diverged, 324 lines in a 162-line file**.

Both repositories are cloned with `core.autocrlf=true`. The reference kept LF because its files were
written by tools and never re-materialised; the Hub's copies came back as **CRLF** on the first
`git checkout` after the previous session's merge. From that moment:

* `tools/check-contract-mirrors.mjs` reported all six mirrored modules as diverged in every line, and
* all eight assertions in `lib/__tests__/skynet-os-mirrors.test.ts` failed,

on two trees that were character for character identical. Both checks were written *last* session,
verified passing, and were broken by the merge that landed them.

This is the failure `.gitattributes` in both repositories already describes — in a comment that says
it "has already happened once, on the other side of the mirror" — pinned for the three shared
**documents** and never extended to the eight shared **modules** when they arrived.

The repair is three guards, because the first two can each be true while the third is not:

1. **`.gitattributes` in both repositories** now pins `reference/typescript/*.ts`, `corpus/**/*.json`
   and the Hub's `lib/skynet/os/*` to `eol=lf`.
2. **The sync tool normalises** before comparing, before diff-counting and before hashing. A line
   ending is never a semantic difference in TypeScript, and a comparison that treats one as a
   contract divergence reports the wrong problem.
3. **The Hub's digest test normalises** the same way, so it is checkout-independent on a machine that
   has never heard of the OS repository.

The diff count needed the same fix in a second place — the equality check was normalised and the
count was not, so the tool went from "2691 lines differ" to "15 lines differ" only after both. Fifteen
is the `latency_ms` block, which is the correct answer.

**Why this one is the worst.** The drift it exists to catch is real and has happened: `safety.ts` on
the Hub was six sessions behind the reference, awarding a host badge to a body the robot refuses. The
check that found it was then broken by its own merge, in a way that fails *loudly and always* — and a
check that always fails is a check somebody deletes. Nothing was wrong with the contracts for the
whole of this window, which is exactly what makes it dangerous: the next real divergence would have
arrived as line 7 of an alarm everybody had learned to skip.

### Changed in the Hub

Three files, and each is recorded here because the log has to be a complete account of what was done
from this repository:

* `lib/skynet/os/embodiment.ts` — `sensors[].latency_ms`, taken from the reference by `--sync`.
* `lib/skynet/os/mirror-digests.json` — rewritten by the sync, as always.
* `lib/__tests__/skynet-os-mirrors.test.ts` — normalises line endings before hashing.
* `.gitattributes` — pins `lib/skynet/os/*` to LF.
* `supabase/migrations/20260830120000_skynet_embodiments.sql`, and the migration **applied** to the
  BotMarket project — see the descriptor-store section below.
* `lib/skynet/robots/embodiment.ts`, `app/api/skynet/v1/robots/[slug]/embodiment/route.ts`,
  `lib/skynet/db.ts`, `lib/skynet/activity.ts`, `messages/*.json`.

The Hub's suite passes and `tsc --noEmit` is clean.

### The descriptor store on the Hub, and where this session was stopped

Next item 1 from last session, and the last unbuilt entry on §22's *"buildable now, no robot
required"* list: *"the transport endpoint on the Hub, the embodiment tables, the descriptor publish
path"*. Everything above it in that list has existed for sessions in two editions and is held to the
corpus; what has never existed is anywhere on the Hub to **put a descriptor**, which is why §32.4's
items 4 and 5 have both been recorded as waiting on "a table" rather than on an endpoint.

**Applied to the BotMarket project**, with the owner's agreement, additively:

* `supabase/migrations/20260830120000_skynet_embodiments.sql` — the table, its indexes and its RLS.
  Append-only and content-addressed, because every transport plan pins a descriptor digest and a
  descriptor edited in place would make an existing plan unreproducible while looking fine. A
  partial unique index makes *"at most one current published descriptor per robot"* a property of
  the database rather than of whichever code path last ran.
* `lib/skynet/robots/embodiment.ts` — the digest, the signature check, and the store.
* `app/api/skynet/v1/robots/[slug]/embodiment/route.ts` — `GET` (public, `?digest=` resolves a
  superseded one) and `POST` (namespace-owned, `namespace.write`).
* `lib/skynet/db.ts`, `lib/skynet/activity.ts` and `messages/en.json` — the table in the known set,
  `embodiment.publish` in the audit vocabulary, and eleven translatable strings synced to all 35
  locales.

Three decisions worth recording.

**The signature scheme had to be written again rather than reused, and that is the interesting
part.** This repository already has `verifyDigestSignature` in `lib/skynet/signing.ts` — and it is
base64 over SPKI DER, over the bare digest, with no domain tag, because it verifies *package
releases*. The robot verifies a descriptor with raw hex keys and signatures, over
`"skynet.embodiment.v1\n" + digest`. Reusing the first for the second would have produced a Hub that
marks a descriptor `verified` and a robot that refuses the same bytes: the §31 failure one level up
from the one the contract mirrors already guard against. The two encodings are the same key —
Ed25519 SPKI DER is a fixed twelve-byte header plus the 32 raw bytes — so `rawKeyFromDer` re-encodes
a registered key rather than asking a vendor to register two.

**An unsigned or unverified descriptor is published, not refused.** §5 keeps four states apart, and
only the last sets `verified`. Refusing the middle two would leave a vendor with no way to publish a
body description at all, which pushes them back to the guesswork the descriptor exists to replace.
What the platform will not do is *call* any of the first three verified.

**The document is served exactly as it was published.** A descriptor carries `digest: null` and the
digest is over the document including that null, so the Hub never fills the field in — a robot that
downloaded a descriptor and recomputed its digest would otherwise get a different answer, and would
be right to. The digest travels beside the document.

#### Stopped mid-edit, and the tree is not clean

The session was stopped while building the cross-edition **signature vector**: the thing that would
prove the paragraph above rather than assert it. `crates/skynetd-io/src/embodiment_vector.rs` was
written and registered in `lib.rs`, and it **does not compile**:

* `use skynet_ed25519::SigningKey` should be `use skynet_ed25519::signing::SigningKey` — the signer
  lives behind the `test-signing` feature, in its own module.
* `sign()` returns `[u8; 64]` rather than a `Signature`, and `public_hex()` already returns hex, so
  the two helpers around them are wrong.
* **The frozen digest constant in it is a placeholder, not a computed value.** It was written to be
  replaced by the output of the `#[ignore]`d generator, and that generator has never run.

So `cargo build -p skynetd-io` succeeds and **`cargo test -p skynetd-io` does not compile**. Nothing
else in the workspace is affected and nothing was committed. Finishing it is: fix the three lines,
run `cargo test -p skynetd-io --lib -- --ignored embodiment_vector --nocapture`, paste the printed
digest into the constant, and freeze the same four values in a Hub test that verifies the signature
with Node's crypto. Backing it out instead is deleting the file and its `pub mod` line.

The Hub side is unaffected by that: `npx tsc --noEmit` is clean and the audit-vocabulary test passes.

#### Not done, and not started

`GET /repos/{ns}/{name}/versions/{v}/transport` — §17's other new route, and the one §32.4 item 4
actually waits on — is unwritten. It is now unblocked: the descriptor it needs has somewhere to live.

The eleven new strings are in all 35 catalogues with their English text, which is this project's
spelling of *pending*: `scripts/translate-with-openai.mjs` treats a value identical to English as
untranslated. It has not been run.

### Where the numbers stand

`sh crates/skynetd-io/e2e.sh` passes end to end in a `rust:1-slim-bookworm` container, with the
launch step reported as not run because there is no systemd in it.

**One caveat on every number below: they were measured before the session was stopped mid-edit.**
`crates/skynetd-io/src/embodiment_vector.rs` is half-written and `cargo test -p skynetd-io` does not
currently compile because of it. The library builds, nothing else in the workspace is affected, and
the descriptor-store section above says exactly what is wrong and how to finish or back it out.

Rust: **690 tests** on Windows and **721** in a Linux container — the difference is `skynetd-io`'s
systemd-adjacent suite and the whole of `skynet-ring-shm` — with `cargo fmt --check` clean and
`cargo clippy --workspace --all-targets` clean under `-D warnings` on both. The kernel and the ring
both still build for `thumbv7em-none-eabihf`. Corpus: **459 cases across 38 files**, reproduced byte
for byte.

### What is still zero lines

The policy runtime, OTA (§12), the safety MCU, and the drivers that talk to real hardware.

§25's command path has come off that list as a **mechanism** and is not off it as a **path**. The
ring, the `memfd`, the seals, the two mappings and the frame's projection onto the black box record
all exist and are exercised, including across two real processes. What does not exist is anything
that would call them on a robot: no capture backend fills a stream (§47), no policy sandbox receives
the descriptor (§8), and no loop turns a frame into an inference and a command. Every surface says so
rather than implying otherwise.

§34's outbound path is in the same half-state and for a related reason. The **decision** is built —
which bearer, whether it carries this class, when to retry — and the **transport** is not: §34.2 is
HTTPS to an endpoint §10's pairing would have established, and §10 is unwritten. So the sink a robot
has is one that refuses everything and says why, and the one that actually moves bytes writes them
into a directory on the same machine.

§48's **homing** stays on the list and belongs there rather than in a gap: it moves the robot through
the RHAL extension channel, which is the one thing §33.1 forbids `skynetd`.

### Findings recorded rather than fixed

1. **§52's table cites §25.2 for `sensors[].latency_ms`, and the mechanism is in §25.1.** §25.2 is
   *"Two packages, one arm"* — capability leasing — and has nothing to say about sensor latency. It is
   a cross-reference in a shared document, so it is recorded rather than corrected: amending the
   specification is a decision, and this one is cosmetic enough that it should be made deliberately
   rather than folded into a session that happened to notice it.
2. **The ring's slot layout is a format, and §31.2's list does not name it.** Two processes read those
   bytes, and on a robot running a Python policy runtime against a Rust kernel host they are two
   *editions* — which is precisely the condition §31.2 says makes a thing single-source: *"a format
   with two implementations of its own definition is not a format."* The layout is written out in full
   in the crate documentation, so a second edition has something to implement rather than something to
   reverse-engineer, but it is not in the corpus and it is not in §31.2's table. Adding it to either is
   a specification decision.
3. **Nothing consumes `Reading::Stale`.** The ring's consumer reports it and no caller exists to
   count it. §25 makes the count the evidence that the producer is outrunning the reader, and
   evidence nobody collects is a field in a struct. Unchanged by this session's sender, which drains
   the *outbound* queue rather than the command ring. They are two different paths and only the
   outbound one now has anything reading it.
4. **The two mirror checks still only meet where both repositories are checked out**, unchanged from
   last session and now with one more reason to care: this session's defect was found by a human
   running the tool, and CI would not have caught it because the job that runs it has one repository.
5. **`--outbox` is a sink that looks like a success.** A `Directory` send returns `Ok`, so the item
   is removed from the queue and the counters record a delivery — which is true of the sink and
   false of the robot. It is named as a development sink in its own documentation, in
   `Sink::describe`, and on the console, and that is three places relying on a reader noticing. The
   honest fix is for a non-transport sink to be visible in the *counters* as well, which means the
   queue distinguishing "delivered" from "handed to something local", and that is a queue change
   rather than a sink one.
6. **`npx eslint` still cannot run in the Hub** — `typescript-eslint` refuses TypeScript 7.0.
   Pre-existing, unrelated, and it means `tsc --noEmit` and vitest remain that repository's only
   static checks.

## 2026-08-31 (sixteenth) — the vector closed, the transport path, and four sections nobody had noticed were buildable

Last session was stopped mid-edit, and its own notes said so precisely: `embodiment_vector.rs` was
written, registered in `lib.rs`, committed, and did not compile. This session opened by finishing
that, and then took the item the descriptor store had unblocked.

It then kept going, and the shape of what it found is worth stating at the top rather than leaving
to be assembled from ten sections below.

**§22's *"buildable now, no robot required"* list was completed and turned out not to be the list.**
Its last entry — the transport endpoint, the embodiment tables, the descriptor publish path — went
in, and with it §32.4's fourth page. That should have left only work that needs hardware. It did
not: §22 was written against Part I, and Parts IV–VI added sections with exactly the same property.
Three of them were built in the hours after that list was finished — §47.4's capture split, §51's
offline entitlement, §50's birth certificate — and a fourth, §49.1's warning channel, followed. Each
is pure, testable, and about a *discipline* rather than a device. The pass stops at §54, which is
recorded rather than started.

**Two divergence checks now exist that did not, and both caught something real the same day.** The
mirror tools were made to fail with one checkout, which is the configuration CI runs in — and hours
later that check caught a divergence *this session had just committed*, in the reference contract it
had edited on one side only. The gap that let it happen — a vocabulary can drift where no corpus
case has been written — was then closed too, with 21 closed string sets frozen as corpus cases.

**Two recorded findings were fixed and one was created and closed.** The `--outbox` sink that
reported deliveries it never made, the mirror checks that could not fail, and the vocabulary gap
above.

### The cross-edition signature vector, finished on both sides

The three defects the last session recorded were the three that were there:

* `use skynet_ed25519::SigningKey` → `use skynet_ed25519::signing::SigningKey`. The signer lives in
  its own module behind the `test-signing` feature, which is the point of that feature: nothing
  that ships may name it, and `cargo build -p skynetd-io` does not compile it.
* `sign()` returns `[u8; 64]`, not a `Signature`, and `public_hex()` already returns hex. Both
  helpers around them were written against an API this crate does not have.
* The frozen digest was a placeholder — thirty-two bytes somebody typed — waiting for the
  `#[ignore]`d generator that had never run.

The generator ran. What it printed is now frozen in **four** constants rather than one, and that is
the substantive change rather than a transcription:

```
digest      29fe4b8d49ec7c32b6936b2e25c3cbd7b7b96c8bb21ad8de5dce3e44f8d2e543
public key  79b5562e8fe654f94078b112e8a98ba7901f853ae695bed7e0e3910bad049664
signature   cde12b51…feeae708   (128 hex characters, over the domain-tagged digest)
```

**The signature is frozen, not re-derived.** The half-written test signed the vector and then
verified what it had just signed, which passes for any self-consistent scheme including a wrong
one: an edition that signed the bare digest would sign it and verify it and report success. Ed25519
is deterministic — RFC 8032 §5.1.6 derives the nonce from the key and the message, so one key over
one message has exactly one signature — so the signature can be *compared to a constant*, and that
is what makes a vector a vector. `the_vector_signature_is_what_the_hub_holds` compares; the
separate `the_frozen_signature_verifies_here` runs the **verifier** against bytes it did not
produce, which is the side a robot actually runs. They fail separately on purpose.

Three more assertions were added because each names a substitution a reader of the scheme would
plausibly make: the seed still expands to the frozen key; the bare digest is *not* the signed
message (asserted to fail, since a verifier accepting both would accept a signature made for any
other document whose digest happened to be this hex); and filling the descriptor's own `digest`
field in before hashing changes the digest, which is why the Hub serves a published document
untouched.

**The other half now exists.** The Rust module's doc comment has pointed at
`lib/__tests__/skynet-embodiment.test.ts` since it was written, and that file did not exist — a
cross-edition vector pinned in one edition is a constant. It exists now, in BotMarket: 17 tests
that recompute the same four values with Node's `crypto` and the Hub's own canonicaliser. Both
sides fail *in their own suite* without the other checkout present, which is the property finding
№4 (the mirror checks only meet where both repositories are checked out) says the mirror tools do
not have.

The Hub half also covers what only the Hub can get wrong: `rawKeyFromDer` round-trips the vector's
key through the base64 SPKI DER this repository has always stored signing keys as, proving that is
a re-encoding rather than a derivation — one registered key, two spellings — and refuses both a
non-Ed25519 header and a DER of the right shape and the wrong length. And upper-case hex is
rejected, which is not pedantry: `publishEmbodiment` compares a signing key to the namespace's
registered keys *as strings*, so a verifier lenient about case would verify a signature and then
call the registered key that made it untrusted.

Numbers after this item: Rust **697** tests on Windows (was 690), `cargo fmt --check` and
`cargo clippy --workspace --all-targets -D warnings` both clean. Hub: **1176** tests across 44
files, `tsc --noEmit` clean.

### `GET /repos/{ns}/{name}/versions/{v}/transport` — §17's last unbuilt read route

The route §32.4 item 4 waits on. §6.2's claim is that a repository page answers *"does this run on
my G1, and how well?"* before anyone downloads forty gigabytes, and until now the Hub had the
planner, the descriptor store, and no endpoint joining them.

The planner itself needed nothing: `resolveTransport()` is pure and already held to the corpus by
three editions. What this session had to build is the **impure edge** — turning a stored manifest
and a stored descriptor row into the planner's two arguments — and every decision there turned out
to be about *whose word counts*. It is `lib/skynet/robots/transport.ts` rather than code in the
route for that reason: the same edge is what `/skynet/os/transport` and the device pages will need.

Four decisions, in the order they matter.

#### `verified` and `digest` come from the row, never from the vendor's document

A descriptor is stored exactly as published — correct, and it means the document contains two
fields the vendor *wrote* and the platform *decides*. A vendor who puts `"verified": true` in their
own document has asserted something rather than established it, and handing that document to the
planner unaltered suppresses `unverified_embodiment` on precisely the descriptors that most need
it: the unsigned ones. `targetFromRow` overwrites both from the store's own columns, and a test
asserts the consequence at the level a reader cares about — the warning still appears in the plan —
rather than only at the field.

The digest half is less alarming and equally necessary: a published document carries `digest: null`
by construction, so a plan built from the document alone would have a null `target_digest` and
nothing to pin.

#### An undeclared action space is a refusal, not an empty plan

`resolveTransport()` given zero action channels returns **`verdict: "direct"`** with a
`no_source_embodiment` warning. It is the right answer to "did anything fail to bind" and it is
read as "yes, this runs" by anyone looking at the verdict field, which is what a verdict field is
for. See the findings below: all three editions agree and the corpus freezes it, so nothing
differential can catch it.

So the route refuses — 422, naming the manifest key and what to put in it — rather than serving a
yes nobody checked. Changing the verdict vocabulary is a specification decision across three
editions and the corpus; declining to publish a misleading one is not.

The route's three no-plan answers are deliberately three rather than one, because they imply
completely different work: **404 robot** (a typo), **404 descriptor** (the robot exists and its body
has never been described — a vendor's work, and nothing about the policy matters), **422
declaration** (the body is described and the policy does not say what it emits — the publisher's
work).

#### Nothing infers a channel from `action_space.dimensions`

`ModelManifest.action_space` has said `{ type, dimensions, control_hz, limits }` since long before
transport existed, and reading "12 joint_position dimensions" as twelve channels is one line of
code. It is also wrong: a channel binds by canonical role or exact joint name, neither of which is
in a dimension count, so twelve nameless channels produce twelve `blocked` bindings and a `refused`
verdict — for a policy whose actual problem is that nobody wrote down what its outputs drive.
Refusing a policy for under-declaring and refusing it for not fitting are different sentences, and
this codebase does not conflate them anywhere else either.

The per-channel declaration is a new manifest block, `source_embodiment`, which is the same key
`skynet transport` already accepts around a policy file on disk.

#### The reader had to be made to agree with the one on the robot, and the first draft did not

This is the part worth recording, because the first version of `parseSourceEmbodiment` was written
the way a careful TypeScript reader is normally written — validate against the union type, drop what
does not match, collect issues — and that is a **divergence from `source_from_json`**, which reads
the same declaration on the robot at install time. Two rules, and the draft had both backwards:

* **A missing required field fails the whole declaration.** Rust propagates with `?`, so one channel
  without an `index` makes the source undecodable — not that one channel absent. The friendlier
  reader would hand the Hub a plan with N−1 bindings where the robot builds none at all, and *"the
  plan you were shown had a channel missing from it"* is the exact failure a pre-download check
  exists to prevent.
* **An unrecognised value is carried, not dropped.** Rust types `kind`, `modality` and
  `control_mode` as plain `String` and hands them to the planner. Dropping a channel whose `kind`
  this edition does not recognise deletes it from the Hub's plan while the robot still binds it —
  a divergence in the **fail-open** direction, because a channel absent from a plan raises no
  warning at all while a channel bound raises whatever the planner raises.

Neither the corpus nor any differential test covers the *readers*; both cover the planner they
feed. So the parity is asserted directly, in `parity with the reader on the robot`, one test per
rule with the reason in the test.

What this edition adds on top is a second output rather than a different decode: it **names** every
value it did not recognise, so a publisher whose channel says `joint_impedance` is told which
channel and which word instead of finding a binding with no target in a plan. The route returns
those as `declaration_issues`, separate from `plan.warnings` — the planner's warnings are about a
declaration it could read, and a publisher reading only those would never learn that their channel
is in the plan under a kind nothing understands.

#### Also landed

* `source_embodiment` added to `MODEL_SCHEMA` and to `ModelManifest`, typed `unknown` on purpose:
  the shape is a format with a reader per edition (§31.2), and a structural type here would drift
  from both readers while looking authoritative.
* The docs page listed neither embodiment route — a gap from last session — so `GET`/`POST
  /robots/{slug}/embodiment` and the new transport route were added to it, with three new strings
  synced to all 35 locales (102 values added; `en.json` grew by exactly three lines).

Numbers after this item: Hub **1204** tests across 45 files, `tsc --noEmit` clean, both mirror
checks matching.

### `/skynet/os/transport` — §32.4 item 4, built the moment its endpoint existed

The interactive half: *"pick a policy version and a robot, get the per-channel plan before
downloading anything."*

**It is a `<form method="get">` and a server render, not a client that calls the API.** The
endpoint computes a plan from `sourceForVersion`, `targetFromRow` and `resolveTransport`; so does
this page, from the same three functions. Fetching the endpoint from a client component would put a
second copy of the *shaping* — which field means what, which refusal reads how — behind a network
hop, and §6.2's argument for the planner being pure is exactly that every caller computes rather
than asks. The practical consequences are that the page works with JavaScript off, renders for a
crawler, and cannot display a plan the endpoint would not have produced. The selection is in the
URL, so a plan is a link somebody can send to the person who has to act on it.

**Both selectors list only what can carry a plan.** Robots with a current published descriptor
(`robotsWithEmbodiment`), and repositories with at least one published version declaring an action
space (`listTransportableVersions`, newest first, so the option shows the newest release that
actually declares one — which is not always the latest published). Offering anything else would be
offering a button that always fails.

**It never renders an example.** §32.3's rule has teeth here: a transport plan is precisely the kind
of thing that looks instructive as a mock-up and is a fabricated statement about a physical machine.
With no descriptors and no declarations the page says so — and says the two *separately*, because a
missing descriptor is a vendor's work and a missing declaration is a publisher's, and telling
somebody the wrong one wastes their day.

Three smaller things the page holds to, each of which the codebase holds to elsewhere: a null
control rate reads as *not declared* rather than as a number; a clamp with no `source` says a limit
with no provenance is a rumour; and the declaration issues are a section of their own, apart from
the planner's warnings, with a lede explaining that the distinction is the publisher's whole remedy.

#### A defect found by writing the second page rather than by reading the first

`/skynet/os` uses class names that are in no stylesheet: `skynet-h` (every other Skynet page uses
`skynet-h1`/`skynet-h2`), `data-variant` on badges (the stylesheet keys badges off `data-tone`;
`data-variant` belongs to the button), and a `muted` value that is not one of the four tones. So its
headings have rendered unstyled and its badges untinted since the page was written, and nothing
failed — a class name that matches no rule is not an error anywhere in the pipeline.

Found because the new page was written by copying the old one's markup, which is how this class of
defect propagates. Both are fixed; the "muted" cases now omit the attribute rather than passing a
tone that does not exist, which is what "muted" meant.

### The outbound sink that reported deliveries it never made — finding 5, fixed

Recorded last session: *"`--outbox` is a sink that looks like a success. A `Directory` send returns
`Ok`, so the item is removed from the queue and the counters record a delivery — which is true of
the sink and false of the robot."* It was named in three places — the sink's documentation, its
`describe`, and the console — and three places relying on a reader noticing is not a mechanism.

The fix is the one the finding named: **the queue distinguishes "delivered" from "handed to
something local"**, because the queue is the only thing that can. Once an item is removed nothing
downstream can tell the two apart, and *"the queue is empty"* is true of both while meaning opposite
things about the robot.

* `Sink` gains `fn delivers(&self) -> bool` with **no default**. A default of `true` is the one that
  lies — it is exactly what `--outbox` would have inherited — and `false` would make a real
  transport under-report by omission. `Directory` answers false; `Unpaired` answers true and is
  never asked, because it is the placeholder for §34.2's HTTPS transport and answering false would
  describe a robot with nowhere to send as one handing items to something local.
* `Queue::accepted(delivered)` records it at the one moment both facts are in hand — beside
  `attempted`, which is its opposite number — into the same counters file that already holds
  `dropped_*` and `collapsed`. So it survives the process, which `Drained` does not.
* `Status`, `to_json`, `Drained`, the supervisor's log line and the console panel all carry the two
  apart. The log line for a hand-off does not contain the word *sent*, and the console renders the
  hand-off count as a warning pill saying the items are still on this machine.

**The test suite had the bug written into it.** `the_drain_sends_over_an_unmetered_link_and_empties_the_queue`
asserted `sent == 2` over a `Directory` — a sink writing to a directory on the same disk — and
passed. It is now `the_drain_empties_the_queue_over_an_unmetered_link_and_calls_it_a_hand_off`, and
the drain path is unchanged: only the word for what happened is right. Four tests around it,
including one asserting a transport sink and a local one against each other in the same queue —
a suite that only exercised the directory case would pass with `accepted` hard-coded to `false` —
and one asserting both keys reach `to_json`, because the surface an operator reads showing a single
number is what made the defect invisible.

### The two mirror checks now fail with one checkout — finding 4, fixed

Recorded for two sessions: *"The two mirror checks still only meet where both repositories are
checked out, and CI would not have caught it because the job that runs it has one repository."*

The CI job is called *"The shared documents and contracts have not diverged"* and it runs both
tools. With one checkout each printed `BotMarket not found` and exited **zero**. So a job named
after a check has never been able to fail, and the one divergence the contract tool has actually
caught — `lib/skynet/os/safety.ts` six sessions behind, `conformance.ts` missing an SCTS test
required at host — was caught by a human running it locally.

The fix is the technique this session opened with, applied one level up. The embodiment vector works
because **each side fails in its own suite, with one checkout**. The Hub already had that property
for the contracts: `lib/skynet/os/mirror-digests.json` is checked by `skynet-os-mirrors.test.ts` on
a machine that has never heard of this repository. This repository had nothing equivalent, in either
direction.

Both tools now pin what they guard, and both verify the pins when the Hub is absent:

* `reference/typescript/mirror-digests.json` — the same digests as the Hub's copy, written by the
  same `--sync`, so the two cannot drift from each other. It catches a **reference contract changed
  without the Hub being resynced**, which is the direction that matters more: the reference is the
  oracle the corpus holds the Rust edition to, so a change here is a change every edition owes.
* `docs/mirror-digests.json` — the three shared specifications. It catches a document edited *here*
  and not there, which is exactly what §72.1's rule forbids: a shared document is edited in both
  repositories or in neither.

Verified by breaking it on purpose in both directions: a line appended to `reference/typescript/safety.ts`
fails with the Hub present *and* absent; a line appended to `docs/skynet-architecture.md` gives
`CHANGED … no longer matches its pinned digest` with one tree and `DIVERGED` with two. Both restore
clean.

Three details that decide whether this is a check or a nuisance:

* **A fresh clone passes.** The pins are of committed bytes, so somebody who has only cloned this
  repository is still never told their tree is broken because a private one is missing — which is
  the property the original exit-zero was protecting and the reason it was written that way.
* **`--sync` re-pins only when the two copies agree.** Pinning a divergence would record a
  disagreement as the agreed state, which is the one thing a pin must never do.
* **The pins are verified with both trees present too**, so a pin left stale by an edit made
  correctly in both repositories fails on the developer's machine rather than surviving to CI.

`docs/*.md` is now named explicitly in `.gitattributes`. `* text=auto eol=lf` already covered it,
but these digests are over file *bytes*, and this repository has already had a checkout convention
present as a divergence once — six modules reported as differing in every line on two trees that
were character for character identical. The list of things whose bytes are their meaning should be
readable rather than inferred from a wildcard.

The CI comment that said both checks were *"no-ops when BotMarket is not checked out beside this
one … the reason they are cheap to have"* has been rewritten, because it is no longer true and a
stale comment beside a check is how the next reader concludes the check is decorative.

### §47.4's capture-side split — `skynet-capture`

§47.4 is titled *"Two consumers, and they must not share a queue"* and calls itself **the design
decision in this section**. It was unbuilt, and — unusually for what remains — it needs no hardware
at all, because the decision is about *which frame each consumer sees and what happens when one
falls behind*, and none of that involves a pixel.

| Consumer | Wants | Gets |
|---|---|---|
| Policy | The freshest frame; an old frame is worthless | `LatestSlot` — new frames overwrite unconsumed ones |
| Black box | Every frame; completeness is the guarantee | `RecordQueue<N>` — bounded, oldest-first, gaps annotated |

The two disciplines are opposite on purpose: the slot **discards** to stay fresh, the queue
**retains** to stay complete. Neither can stall capture, and that sentence is really about priority
— §47.4: *"stalling capture starves the policy to protect the recording, which is precisely
backwards."* `Distributor::deliver` therefore has **no error path at all**, and the absence is the
invariant rather than an oversight.

Four decisions worth recording.

**It holds no pixels.** §47.3 passes capture buffers as `dmabuf` handles by reference, so what moves
through here is a handle, a stream index, a sequence and an acquisition stamp. That is a performance
decision in the specification and a *testability* decision here: sixty frames arriving during one
long policy cycle is a loop over integers, and the assertion that the policy reads the sixtieth is
the whole of §47.4's claim.

**Every displaced or refused frame is handed back.** A zero-copy pipeline has one hard rule — a
buffer handed onward is handed back, or the pool runs dry and capture stops for a reason that looks
like a driver fault. So `publish` returns what it overwrote, `push` returns `Dropped(frame)` with
the frame in it, and `deliver` returns a `Delivered` carrying both. An API that swallowed them would
be one where the leak is invisible in the type system and obvious three hours in, which is §47.5's
description of an unbounded capture pool.

**A run of drops is one annotated gap, and `dropped` and `gaps` are separate counters.** §47.4: *"a
gap in the black box that is annotated as a gap is honest; a gap that looks like a quiet period is
not."* A hundred consecutive drops collapse into one `Gap` with first and last sequence and stamp —
a hundred gaps of one frame each is the same information rendered unreadable. But the two counts are
kept apart because two hundred frames lost in *one* gap is a stall and two hundred lost in *two
hundred* gaps is a queue sized one frame too small, and those call for completely different work.
There is a test that produces both shapes and asserts they are told apart.

**`StampSource` has no `Arrival` variant.** §47.2 says estimating is acceptable and silently
substituting arrival time is not, because it folds a variable queue delay into a number the policy
treats as ground truth. So the type makes the dishonest option unspellable: `Captured::estimated`
takes a `NonZeroU64` uncertainty, and there is no way to construct an estimate that claims to be
exact. A backend with only an arrival time has to write a number down.

`no_std`, no `alloc`, no `unsafe`, capacity as a const parameter — the per-stream declaration §47.5
asks for, so the pool is bounded by construction rather than by a policy somebody enforces. It joins
the kernel and the ring in CI's `thumbv7em-none-eabihf` build for the reason that step already
gives: it sits in the cycle a control loop runs, and a crate that quietly acquired a `Vec` would be
an allocation on a path that must not have one, and nothing else in the workspace would notice.

**It is not wired to anything, and that is stated rather than implied.** No capture backend produces
the handles it moves — V4L2 and GenICam are hardware — and no loop reads the slot. It lands the way
`skynet-ring` landed: the mechanism, exercised, with the surfaces saying what does not exist.

Seventeen tests. The one worth reading is `a_stalled_encoder_does_not_add_latency_to_the_policy`,
which is §47.4's argument as an assertion: the encoder stops, sixty frames arrive, the policy reads
the sixtieth, the recording is what lost, and all 115 displaced-or-refused handles come back.

#### The README said the `memfd` was unbuilt, two rows below the crate that builds it

Found while adding the crate to the table. `skynet-ring-shm` landed last session — sealed anonymous
file, consumer mapping with no `PROT_WRITE`, a test that re-executes itself so a command crosses two
real processes — and the paragraph below the table still read *"what is not built is the `memfd`
that a real ring is mapped over … today the producer and the consumer are two threads in a test
rather than two processes on a robot."*

Both halves were wrong, and the second was wrong in the direction that matters least and embarrasses
most: the README was understating what exists. Rewritten to split *built* from *not built* explicitly
and to say what happened, because a README that describes what its author remembers rather than what
is in the table above it will drift again.

### §51's offline entitlement — the whole section, and it needed no hardware either

The Parts IV–VI pass recommended a paragraph ago, done immediately. §51 was **zero lines**, it is
entirely a decision over declared facts, and it had never appeared on §22's list because §22 was
written against Part I.

§51.1 states one principle and says the whole design follows from refusing its opposite:

> **A licence check must never stop a running robot.** A billing system that can halt a humanoid
> mid-task has invented a safety hazard to protect revenue.

**The most important thing in `skynet_contracts::entitlement` is a type that does not exist.** There
is no outcome meaning *stop*, *halt*, *suspend* or *deny at run*. `Verdict` answers one question —
may this package be installed or upgraded — and the running robot is not a party to it. Adding a
variant that stopped a robot would mean editing a file whose first paragraph says why not, and it
would fail `no_verdict_stops_a_running_robot`, which iterates every variant rather than the ones the
suite happens to construct: a new variant has to be added to that list, which is the moment somebody
reads the sentence above it.

There is also a `permits_running(Verdict) -> bool` that returns `true` unconditionally. It looks
like nothing. It is §51.1 as something a reader can find, a test can assert, and a future change
would have to edit on purpose rather than erode by addition.

#### §51.2's four rows, and the one that looks like a bug

| Situation | Behaviour |
|---|---|
| Valid | Install, upgrade and run |
| Expired, inside grace | Runs. **Upgrades allowed.** Reported as lapsed. |
| Expired, past grace | **Keeps running.** Installs and upgrades refused. |
| Revoked | **Keeps running.** Reinstall refused, incident raised. |

Two rows carry *keeps running* in bold in the specification itself, which is unusual enough to read
as an instruction to whoever implemented it.

The second row is the one somebody will eventually "fix". Expired-inside-grace still permits
**upgrades**, which looks backwards until §51.2's reason for a ninety-day grace is read — *"the
failure it guards against (a robot in a basement losing a feature because a token expired) is worse
than the leakage it permits"* — and then it is obvious: a robot whose token lapsed three weeks ago
and which needs a security fix should get the security fix. Its test says so in those words.

Expiry deliberately does **not** raise an incident; only revocation does. A subscription ending on
the day it was always going to end is the system working, and a fleet that raised an incident for it
would teach its operators to ignore incidents.

#### A fifth verdict the table does not have

`Unattested`, for a token whose signature is absent, failed, untrusted or uncheckable. §51.2's table
assumes a token; a token nobody can verify has told the robot nothing at all — neither a grant nor
the absence of one — and installing on the strength of it would let anyone who can write a file into
a cache directory grant themselves a licence.

It **outranks revocation**, which is the ordering worth defending: an unverifiable token has not
told the robot it was revoked either, so it must not be able to raise an incident against somebody
any more than it can grant a licence. And revocation in turn outranks expiry, because a token
revoked and since expired is a dispute rather than a renewal, and calling it expired would send
somebody to a billing page instead of to whoever revoked it.

Like the other four, it does not stop anything.

#### The split, and the fourth domain tag

`skynet-contracts` decides and takes no clock and no key — the instant is an argument, for §35.2's
reason and §45's, so the Hub answering *"is this fleet lapsed"* from a check-in and the robot
answering it offline are the same function over the same facts.

`skynetd-io::entitlement` is the other half: reading the cached token, digesting its canonical form
under `skynet.entitlement.v1`, and checking the issuer's detached signature against the trust store.
That is a **fourth** domain tag beside package, profile and embodiment, and it is there for the
reason the first one was: §51.2's token is signed over the same shape — a digest — as a release, and
without its own tag a release signature would grant a licence to anything whose token digest
happened to collide. A test asserts the two messages differ.

Wall time, not monotonic, and that is a deliberate exception to §35.1: an entitlement is a fact about
a calendar, somebody's subscription runs to a date, and a monotonic instant cannot express one.
§35.1 forbids the wall clock for *control* decisions, and this is not one.

#### A hole found by writing a test for something else

`Layout::entitlement` flattens `namespace/name` into one path component, because a slash in a file
name is a directory nobody intended — or, from a desired-state document, a traversal. The first
draft's allowed set was alphanumerics, `-` and `.`, on the reasonable-sounding grounds that a dot is
harmless in a file name.

A package named exactly `..` then flattened to `..`, which is the parent directory. Nothing actually
escaped, because both callers append `.json` or `.sig.json` and `"...json"` is an ordinary name —
which is a safety property resting on a *suffix*, one refactor away from not being one. The dot is
out of the allowed set, and the reasoning is in the function rather than in this log.

The flattening stays lossy on purpose: `acme/grasp` and `acme_grasp` name one file. That is safe
only because `read` refuses a token whose own `package` field is not the one it was asked for, and
a test asserts the collision *and* the refusal together, so a future change that dropped the check
fails with a test that says why.

#### Where it is not

Not mirrored to the Hub, and recorded in `check-contract-mirrors.mjs` beside `calibration` and
`watchdog` — the same waiting room, for the same reason: `/skynet/os/devices/[id]` needs a table
that does not exist. Entitlement has a second reason, and it is the more interesting one. §57 makes
the token *"a field in the desired-state document, not an endpoint"*: the Hub's job is to **issue**
tokens and the robot's is to decide about them, those are not the same function, and mirroring the
robot's decision into the Hub before anything issues one would put the wrong half there.

Nothing calls it yet either — §51.2 checks entitlement at install and upgrade, and `admit.rs` does
not consult it. That is one line away and is deliberately not taken this session: whether a given
package *requires* an entitlement is §39's question about the marketplace it came from, and wiring
the check before that question is answered would refuse every open-source policy on a robot that has
never bought anything. The module header says so, and so does the fact that a missing token reads
`Ok(None)` rather than a refusal.

**35 tests** — 18 on the decision table, 17 on the disk.

### §50's birth certificate — third hardware-free section in one session

§50.3 says what its absence costs, in a sentence this module exists to make false:

> Without it, *"this is a Unitree G1 with these joint ratings"* is a claim the robot makes about
> itself, and the entire safety envelope rests on believing it.

Generating a birth certificate needs a secure element and a production line. **Checking one needs
neither**, and checking one is the whole of its value on a robot that already exists — which is why
§50 had been sitting behind §22's list along with §47.4 and §51.

`skynet_contracts::birth` takes a certificate and an [`Observed`] — what the machine can see about
itself right now — and returns findings.

#### The distinction the section turns on, enforced

§50.2: *"the birth certificate proves **what it is**; pairing (§10) binds it to **whose it is**. Two
separate facts, established by two separate parties … the vendor is the only party who can attest
to the first and has no business asserting the second."*

So `claims_ownership` is a field on the certificate rather than something a decoder drops, and a
vendor who filled it in gets `OwnershipAsserted` — a finding about the **document**, not about the
robot, because the machine is fine and the paperwork overreached. Reported rather than silently
ignored, because silently ignoring a field is how the next vendor comes to add it too. The forbidden
names are written down once, in `OWNERSHIP_FIELDS`, so a decoder and whatever eventually issues a
certificate cannot disagree about which they are.

#### A board swap is expected behaviour, and the type says so

§50.4: *"replacing a mainboard produces a robot whose birth certificate no longer matches its
hardware. That is the correct behaviour and not a bug."*

That sentence is why the result is a list of findings and not a boolean. `valid: false` would make a
repaired robot look broken, and the remedies for *"the serial does not match"*, *"the descriptor
digest does not match"* and *"the vendor's key is not one this robot trusts"* are three different
phone calls. Each finding carries its own, and a test asserts all three sentences differ.

The device-key sentence in particular says out loud that a board swap produces this, that it is
expected, that the remedy is a vendor-signed re-issue, and that the re-issue also invalidates the
calibrations a board swap could have disturbed (§45.4) — four things an operator staring at a red
line at 2 a.m. should not have to reconstruct from the specification.

#### The gap §5's `verified` flag cannot see

`descriptor_digest` is the field that closes §50.3, and the reason it needs closing is subtle: a
descriptor can be **perfectly well signed** and be for a different machine. §5's `verified` means
*the vendor signed these bytes*; it has never meant *these bytes describe this body*. Only the birth
certificate binds the two, and `DescriptorMismatch` is the only check anywhere that notices.

Which is not necessarily tampering — a vendor publishing an updated descriptor and a robot fetching
it produces exactly this — so the sentence says what it actually means: the joint ratings in force
are ones nobody has signed *for this machine*.

#### Two findings void a certificate; six narrow it

`Unattested` and `DeviceKeyMismatch` remove what is attested — the first because comparing a machine
to a document nobody signed is comparing it to a story it told itself, the second because every
other field binds *to* that key, so a wrong key means the document is about another machine. The
other six narrow it. `only_two_findings_void_a_certificate` asserts the split over every variant.

And a test asserts what `attests()` is **not**: no sentence in this module says a robot must not run.
§50 establishes identity and does not gate motion, and a robot with a void certificate is a robot
with an unattested identity — which is exactly the state every robot was in before §50 existed.

#### Two smaller things worth recording

**An absent field is unchecked, never matched.** §7.2's rule, and the shape prevents the specific
bug: an empty `serial` on the certificate compared against a machine reporting an empty serial would
have "matched", so a certificate binding nothing would confirm everything. An empty field on the
certificate is `Incomplete`; an absent observation is `Unobservable`; neither is a pass.

**`BirthCertificate` has no `Default`, and the compiler is why.** `SignatureState` does not
implement it, because there is no defensible default signature state — `Verified` would be a lie and
`Absent` would be a document that looks constructed and attests nothing. Deriving `Default` here
would have meant adding it there, to a type five other modules read. It has `unsigned(state)`
instead: the one argument with no defensible default is the one the caller must supply.

`manufactured` is a `String` and is never parsed. It is provenance, not a deadline, and a type that
invited comparison to a clock would invite the certificate to expire — which §50 never says it does,
and which would brick a robot in a basement for exactly the reason §51.1 rejects in the commercial
case.

Sixteen tests. Not mirrored to the Hub, recorded beside `calibration`, `watchdog` and `entitlement`
in the mirror tool: `birth` waits on §57's `POST /robots/{slug}/attestations`, which is unwritten.

#### The on-disk half, and the two fields of six a supervisor may read

`skynetd-io::birth` completes it to the depth §51 reached: read the document the vendor shipped from
`identity/birth.json`, digest its canonical form under `skynet.birth.v1`, check the detached
signature against the trust store — a **fifth** domain tag, and it earns one for a sharper reason
than the fourth. A birth certificate *binds a descriptor digest*, so a vendor signs a document
containing a hex string that is also a descriptor digest, and without its own tag one signature could
be presented as the other. A test asserts the three messages differ.

The interesting part is what this process may check. §50.2 binds six things and **`skynetd` can
compare one of them**:

| Bound | Readable by this daemon |
|---|---|
| Device key | No — the private half never leaves the secure element, and §33.1 gives this process no path to one |
| Serial | No — a DMI or vendor-bus read, which is the kernel's side of the RHAL |
| Hardware revision | No — same |
| **Descriptor digest** | **Yes** — the descriptor is a file this daemon already owns and digests |
| Model | Bound, not comparable: there is nothing on a machine to compare a marketing name to |
| Manufacture date | Provenance, compared to nothing |

The one it can check is §50.3's field — the one that closes *"the entire safety envelope rests on
believing it"* — and the other three come back as `Unobservable`, named, rather than being omitted.
A version that dropped them would produce a report reading *"certificate verified"* on a robot that
had compared a single field, which is the shape of answer this project refuses everywhere else. The
payload therefore answers **two** questions rather than one: `attests` (is this document about this
machine at all) and `fully_confirmed` (was every binding it makes checked and matched).

One asymmetry worth recording, because it looks like an inconsistency and is not: `entitlement::from_json`
**refuses** a token with no expiry, and `birth::from_json` **accepts** a certificate missing fields.
An entitlement with no expiry fails to be the thing it claims — §51.2's token is signed *and*
time-limited. A certificate missing a field is still a certificate; it binds less than §50.2 asks,
each absence is reported as `Incomplete` with the field named, and refusing it outright would throw
away the bindings it does make — one of which is the only one this daemon can check.

Thirteen more tests, twenty-nine for §50 in total.

### §49.1's warning channel, and the check built this session catching a real divergence

§49 opens by saying it is *"lower priority than everything above it, and included because one part
of it is safety-relevant."* That part:

> Standards for robots operating around people expect an audible indication before autonomous
> motion. That makes the warning tone a **system function, not an application feature**: `skynetd`
> owns it, it sounds on transition into `RUNNING` and on resumption after a stop, and **a package
> cannot suppress it.** … Packages may use the speaker for anything else only with an explicit
> `hri.speech` grant, and that grant never includes the warning channel.

*"That grant never includes the warning channel"* was a sentence in a document. It is now a token —
`hri.warning` — that admission refuses however it is asked for, which is what makes the sentence
true rather than a paragraph somebody has to remember.

**`capability_reserved` is a new rejection code rather than a reuse of `capability_denied`**, and the
distinction decides where a person goes. *Denied* is an operator's decision and an operator can
reverse it; reserved is not grantable by anybody, so an operator told "denied" would go hunting for
a setting that does not exist. The check runs before the denial check for the same reason: a package
that asked for both should be told the one it can never have.

The rule covers `requests` **and** `embodiment.capabilities`. A package that declared the capability
and never requested the grant would otherwise install and sit one manifest edit away from claiming
the channel.

`warningRequired(from, to)` is there too, and it takes **strings rather than an enumeration** — with
a comment saying why, because it looks like laziness and is not: §27's `HOLD`/`RUNNING`/`DEGRADED`/
`STOPPING`/`SAFE` machine is referred to in prose by `post.rs` and is declared nowhere. Inventing the
enumeration inside §49's module is how a state machine ends up with two definitions. It also records
that an unknown state name under-sounds rather than over-sounds, which is the wrong direction for a
safety control and is acceptable only because this function answers a question about a transition it
was handed — the enforcement point is `skynetd` owning the tone, and that is not built.

#### The mistake, and the check that caught it

The rule went into `crates/skynet-contracts/src/admission.rs` first, with tests, and everything
passed: 176 contract tests green, clippy clean, the corpus reproducing byte for byte.

**And the two editions had just diverged.** `admission.rs` is the Rust half of the mirrored
`reference/typescript/package.ts` — the oracle §31.4 measures every edition against — and the
reference had not moved. The corpus could not see it, for the exact reason a corpus never can: it
covers the cases that exist, and no case had ever mentioned a token invented ten minutes earlier.
This is the §31.2 failure this repository is arranged around, committed by the session that had
spent the afternoon building a check for it.

The check found it immediately. `node tools/check-contract-mirrors.mjs` — with **one** checkout, the
configuration that until this morning printed *"BotMarket not found"* and exited zero — answered:

```
CHANGED  package — this contract no longer matches its pinned digest.
```

That is the pinned-digest work from earlier in this session doing precisely what it was built for,
on a real divergence rather than a synthetic one, within an hour of existing. It is also a reminder
that the check catches the *reference* moving and cannot catch the *Rust* moving — the corpus is
what covers that direction, and only for cases it holds.

#### Landed properly

* The same rule in `reference/typescript/package.ts`: the code, the constants, `isReserved`,
  `warningRequired`, and the same deduplicating loop.
* **Three corpus cases**, because the rule has three edges and each is a different mistake a second
  edition could make: refusing the token however it is asked for, refusing it when only *declared*,
  and leaving `hri.speech` alone — *"a rule that refused audio outright is one nobody could ship an
  HRI package under"*, and vendors would route around the capability system entirely.
* Corpus regenerated: **462 cases across 38 files**, up from 459, all reproduced byte for byte by
  the Rust edition — which is the actual proof that the two now agree rather than the assertion that
  they should.
* The Hub's `lib/skynet/os/package.ts` resynced, both pin files rewritten, and BotMarket's own suite
  re-run: 1206 tests, `tsc --noEmit` clean.

One small defect found by a test on the way: the finding listed the same token twice when a package
both declared and requested it. Deduplicated in both editions — a finding naming one token twice
reads as two problems.

§49.2 — microphones as a privacy surface, `sensing.audio` denied by default and audit-logged — is
untouched. It is an operator-profile default rather than a mechanism, and this repository has no
shipped default profile to put it in.

### The vocabulary corpus — the gap that let the last divergence through, closed

The §49.1 divergence above was recorded as a finding with a note that *"a check that the Rust and
the reference export the same set of rejection codes would have caught this one in seconds, and is
a morning's work."* Written, then read back, then done — a finding whose fix is understood and
cheap is a finding that should not survive the session that wrote it.

**`corpus/vocabulary/closed-sets.json`, 21 sets.** Every other corpus holds an edition to a
*computation* — give it this input, emit these bytes. This one holds it to a **word list**, and the
reason it belongs in the corpus rather than in a lint is that §31.4 already makes the corpus the
thing an edition must reproduce before it may carry the name. A vocabulary is exactly the kind of
agreement it should have been carrying.

The sets: rejection codes, signature states, package kinds, isolation modes, reserved capabilities,
transport verdicts, methods and warning codes, action kinds, control modes, joint types, base kinds,
modalities, canonical chains and axes, OS tiers, enforcement modes, guards, fault responses, SCTS
suites and areas.

**They are emitted in declaration order, not sorted**, and that is a decision rather than laziness.
Order is part of the agreement for several of them — `OS_TIERS` and `SCTS_AREAS` are ranked, and an
edition that sorted them alphabetically would be one whose *"next tier up"* arithmetic silently
differed while every set still contained the same members.

**A vocabulary the corpus names and an edition does not is an error, not a skip.** The runner
returns *"no vocabulary named `x` in this edition"* rather than passing over it, because a set that
exists on one side and not the other is precisely the divergence this file was added to catch.

#### What had to change to make it possible

A TypeScript string union **vanishes at compile time**, which is why the reference's rejection codes
could not be compared against anything. `PackageRejectionCode`, `TransportWarningCode` and
`TransportVerdict` are now `as const` arrays with the union *derived from them* — so the value and
the type cannot disagree, and the value can be written into a file. On the Rust side, six enums
gained an `ALL`-style constant beside them: `OS_TIERS`, `ENFORCEMENT_MODES`, `FAULT_RESPONSES`,
`PACKAGE_REJECTION_CODES`, `SIGNATURE_STATES`, `TRANSPORT_VERDICTS`, `TRANSPORT_WARNING_CODES`.

#### Proved by breaking it

`capability_reserved` was removed from the Rust array — reproducing this morning's divergence
exactly — and the corpus failed on the first run:

```
vocabulary/closed-sets.json :: package_rejection_codes
  first difference at byte 180
```

with both lists printed in full. Restored, green: **483 cases across 39 files**, all reproduced byte
for byte.

So the two checks now cover the two directions. The pinned digests catch the **reference** moving
without the Hub; the vocabulary corpus catches an **edition** moving without the reference, for
every closed set rather than only for the cases somebody thought to write. Neither existed twelve
hours ago, and between them they would have caught both of this session's divergences — the one it
found in last session's work, and the one it committed itself.

### §56's provenance classes — the last item of this session

The Parts IV–VI pass reached §56, which opens with the sentence its whole design follows from:

> **A leaderboard fed by self-reported on-robot numbers is trivially gameable** — a publisher
> evaluating their own policy on their own robot, choosing their own seeds, and reporting their own
> score is not producing evidence. So an on-robot evaluation result is only worth as much as its
> attestation.

`skynet_contracts::provenance` is that made mechanical, and it is the first thing this session built
that **consumes** something else this session built: `birth_attested` is
`skynet_contracts::birth::Attested::attests`, supplied rather than recomputed, because duplicating
that decision would give the platform two answers to one question.

#### The order of the checks is the whole anti-gaming argument

Only one of the three classes is worth lying to obtain, so the order is load-bearing and is the one
thing in the file marked as not to be rearranged for tidiness:

1. **An untrustworthy device signature is `unattested` before anything else is looked at.** A bundle
   nobody can attribute makes claims about its own simulator flag, its own seeds and its own birth
   certificate, and none of those is evidence either.
2. **A declared simulator is `simulated` and can never be `field`** — asserted against an otherwise
   flawless bundle, because that is the version somebody would ship. §35.3 puts the obligation to
   declare on the driver, and this is where that obligation pays.
3. **`field` needs all four of its conditions.** Three of them is `unattested`, not a fourth class:
   a fourth class would be a way of saying *"nearly evidence"*.

`unattested` is **accepted and never ranked**, which is §56.2's own phrasing and the same posture
§51 takes about a lapsed entitlement and §5 takes about an unsigned descriptor: the platform stores
what it was given and refuses to *call* it something it has not established.

Two smaller decisions with tests behind them. A **simulator is not asked about its birth
certificate** — a simulated body has no mainboard and no secure element, and a finding with no
remedy teaches people to ignore findings. And **§56.4's replayability never changes the class**: a
result can be perfectly attested and unreplayable, and conflating the two would let a missing
recording quietly demote a genuine number.

§56.3's seeds — *"supplied to the robot, never generated by it"* — is a sentence about a protocol,
and its mechanical form is that the bundle's seeds must be the benchmark's **in the benchmark's
order**. A benchmark that fixes its seeds is fixing the sequence, not the set, so the same seeds
shuffled are a different run.

§56.5's two roll-ups are a `scores_agree` with a `1e-9` tolerance that exists for the floating-point
representation and not for the arithmetic — *"a caller tempted to widen it is a caller papering over
the divergence §31.4 exists to catch"* — and a non-finite score never agrees with anything, itself
included, because `NaN == NaN` reporting agreement would say a robot and a Hub that both failed to
compute anything were in perfect accord.

Twenty-two tests.

#### Why it is not on the Hub, and the finding underneath that

This is the first of the three new contracts where the Hub is the **consumer** rather than a second
opinion: §56.2 says the Hub verifies the signature and assigns the class. So its absence there is
less comfortable than `entitlement`'s or `birth`'s, and the reason is worth recording as a finding
rather than as a to-do.

`skynet_evaluations` already carries `verified: boolean` and
`source: platform | self_reported | imported`. **That is a different axis from §56.2's, not an older
spelling of it.** `source` is about *who submitted a result*; `field | simulated | unattested` is
about *what the result is evidence of*. A leaderboard filtering on the first would happily rank a
self-reported simulator run as a platform result, which is precisely §56.1's failure.

Reconciling them is a schema decision and a migration on a live project, and that is the owner's
call rather than something to fold into a session that happened to reach §56. Recorded in
`tools/check-contract-mirrors.mjs` beside the other deferrals, with the reason.

### §54 examined and deliberately not started, with the design work recorded

The Parts IV–VI pass reached §54 (skills), and it stops here on purpose. What follows is what the
examination found, written down so the next session does not re-derive it.

**§54.2 is the piece, and it calls itself that**: *"the decision that matters most in this part."*
`preconditions` in `SkillManifest` is a list of prose strings, and *"prose is documentation, and
documentation does not stop a robot"*, so preconditions become predicates the OS evaluates before a
skill may start.

§54.2 says this *"needs no new machinery"* because the Hub already has a pure predicate engine at
`lib/skynet/compatibility/predicate.ts` — 148 lines, `and / or / not / eq / neq / in / exists /
includes / includes_all / includes_any / gte / lte / gt / lt / semver_satisfies`, plus `__ref` values
that read from the context instead of holding a literal.

**That sentence is true of the Hub and not of the robot.** The engine exists in one edition, is not
in `reference/typescript/`, is not in the mirror set, and has no Rust counterpart — so "the skill
runtime evaluates the same predicate shape" currently means "a second implementation somebody will
write", which is the §31.2 condition: *a format with two implementations of its own definition is
not a format.*

#### The design decision, worked out and not yet built

The Hub's engine is **total and boolean**: it never throws and returns `false` for anything
malformed, unresolvable or unknown, because *"one bad rule row must not take down the compatibility
check for every user."* Correct for its job.

It is **not** correct for §54.2's, which asks for something the boolean cannot express:

> A precondition that cannot be evaluated refuses the skill, **naming the path that was
> unresolvable**. Not "assume true", which is what free-text preconditions do by construction.

`false` refuses, which is the right direction — but it collapses *"the world is not ready"* and
*"this manifest names something this robot does not expose"* into one answer, and those are
different facts with different remedies. The first is a battery that needs charging; the second is a
skill that will never run here and whose publisher needs telling.

So the robot's edition should return three values — satisfied, unsatisfied, unevaluable(path) — and
the way to add that **without** diverging is the useful part:

> `satisfied(outcome)` must equal the Hub's boolean for every corpus case.

The richer answer is then a refinement rather than a second opinion, and the corpus can hold the two
editions to it directly: freeze the boolean, and freeze the three-valued outcome beside it.

#### Why it is a session and not an afternoon

Doing it under §31.2 means the engine moves into `reference/typescript/`, joins the mirror set, and
the Hub's copy becomes a re-export — one implementation, three checkouts. The obstacle is
`semver_satisfies`: it calls `lib/skynet/semver.ts`, the reference has no semver module, and **the
Rust workspace has no range satisfaction at all** — `compare_versions` in `admission.rs` compares two
versions and does not parse `^1.2.0`. So the real shape is:

* `reference/typescript/semver.ts` and `predicate.ts`, both added to `MIRRORS`;
* `lib/skynet/semver.ts` and `lib/skynet/compatibility/predicate.ts` reduced to re-exports — and
  `semver.ts` is one of the most-imported modules in that repository, so that is the step to be
  careful with;
* range satisfaction in Rust, which does not exist;
* the predicate evaluator in Rust, with the three-valued outcome;
* corpus cases for both, including the malformed and unresolvable inputs that are the whole reason
  the Hub's engine is total.

Roughly seven hundred lines across two repositories, touching the Hub's most-imported module. That
is a coherent piece of work and a bad thing to leave half-done — which this session knows, because
it opened by finishing a file the last one was stopped in the middle of.

**The rest of §54 waits on it** rather than being separately available: §54.3's success criteria are
the same predicates over post-state, §54.4's lifecycle re-checks the `hold` subset every cycle, and
§54.6's four conformance tests all assert predicate behaviour. The lifecycle also wants §27's state
machine, which as §49.1's note above records, is referred to in prose and declared nowhere.

### Where the numbers stand

Rust: **810** tests on Windows, up from 690 at the start of the session. `cargo fmt --all --check`
clean; `cargo clippy --workspace --all-targets` clean under `-D warnings`; and the kernel, the ring
and now the capture split all build for `thumbv7em-none-eabihf`.

The corpus is **483 cases across 39 files**, up from 459. Twenty-one of the twenty-four new cases
are a new *kind*: the closed vocabularies, which hold an edition to a word list rather than to an
answer. The other three are §49.1's reserved-channel cases, and they are the only change to the
reference implementation's *behaviour* this session made. §51's entitlement and §50's birth
certificate are new contracts rather than changed ones, and neither is in the corpus or the mirror
set, for the reasons the mirror tool now records.

Hub: **1206** tests across 45 files, `tsc --noEmit` clean, `next build` clean with both new routes
registered (`/[locale]/skynet/os/transport` and
`/api/skynet/v1/repos/[handle]/[name]/versions/[version]/transport`), and all 35 locale catalogues
carrying 5258 source keys.

Both mirror checks pass, and — for the first time — would have failed if they should have.

### Findings, restated after this session

Three are now fixed and are recorded above rather than here: **№4**, the mirror checks that could
not fail; **№5**, the `--outbox` sink that reported deliveries; and one this session both created
and closed — the gap that let an edition's vocabulary drift from the reference, which is now the
vocabulary corpus. The rest stand, with two additions, both found by building on top of the contracts rather than by
looking for them.

1. **§52's table cites §25.2 for `sensors[].latency_ms`, and the mechanism is in §25.1.** Unchanged.
   A cross-reference in a shared document, and amending the specification is a decision that should
   be made deliberately rather than folded into a session that happened to notice it. It is now
   slightly more expensive to fix than it was — the shared documents are pinned, so the amendment
   lands in three files — which is the correct price for the check being real.
2. **The ring's slot layout is a format, and §31.2's list does not name it.** Unchanged.
3. **Nothing consumes `Reading::Stale`.** Unchanged, and deliberately not "fixed" this session. The
   obvious move is to add a counter to the consumer, and a counter nobody reads is the same shape of
   nothing as a variant nobody counts. What the finding actually waits on is the control loop — no
   capture backend fills a stream, no policy sandbox receives a descriptor, and nothing turns a frame
   into a command — and the honest place to collect this evidence is in the loop that would be
   outrun.
4. **`resolveTransport()` returns `verdict: "direct"` for a policy that declared no action space.**
   New, and the most interesting one, because it is a defect *nothing in this system could have
   caught*. All three editions compute it identically — the Rust `derive_verdict`, the TypeScript
   reference, the Hub's mirror — and `corpus/transport/declaration.json`'s `empty-action-space` case
   freezes `"verdict":"direct"` as the expected answer. The corpus note beside it reads *"Nothing
   about the fit can be checked, and the plan says so"*, which is true of the `no_source_embodiment`
   warning and false of the verdict field sitting next to it. `isInstallable()` returns true.
   §6.2 argues that three implementations of one pure function cannot disagree, and that is exactly
   right and exactly why differential testing is blind here: they agree, and they are all wrong
   together. Working around it at the Hub's edge — a refusal rather than a `direct` verdict nobody
   backs — is what this session did, and it is a workaround. The fix is a specification decision:
   either an `unknown` verdict, or a rule that zero channels refuses. Both change three editions and
   the corpus, which is why it is recorded here rather than taken.
5. **§56.2's provenance class and the Hub's `EvaluationSource` are two axes, not two spellings of
   one.** New. `skynet_evaluations` carries `verified: boolean` and
   `source: platform | self_reported | imported`, which answers *who submitted this result*. §56.2's
   `field | simulated | unattested` answers *what this result is evidence of*. They overlap enough
   to be mistaken for each other and a leaderboard filtering on the first would rank a self-reported
   simulator run as a platform result — which is exactly §56.1's failure, arrived at by using the
   wrong existing field rather than by missing one. Reconciling them is a schema decision and a
   migration on a live project, so it is recorded rather than taken.
6. **`npx eslint` still cannot run in the Hub** — `typescript-eslint` refuses TypeScript 7.0.
   Pre-existing, unrelated, and it means `tsc --noEmit` and vitest remain that repository's only
   static checks.

### Not done, and not started

The policy runtime, OTA (§12), the safety MCU, and the drivers that talk to real hardware remain at
zero lines. §22's *"buildable now, no robot required"* list is, with this session, **complete**: the
transport endpoint, the embodiment tables and the descriptor publish path were its last entry, and
the two simulator/LeRobot drivers above it have existed for several sessions.

§32.4's remaining items are 2 (`/skynet/os/editions`) and 5 (devices, certification), and both wait
on tables that do not exist — `skynet_os_editions`, `skynet_conformance_results`,
`skynet_device_state`, `skynet_incidents`. That is the next boundary, and it is a schema decision
rather than a coding one. §32.4 gates item 2 on there being more than one edition, and there is one.

**What §22's list does not cover is where the remaining hardware-free work is**, and this session
ended by taking one piece of it: §47.4's capture-side split. §22 was written against Part I, and
Parts IV–VI added sections with the same property — a decision that is pure, testable, and about
*discipline* rather than about a device. The pass through Parts IV–VI then produced §51 and §50 in
succession — three such sections in one session, from a list that was supposed to be exhausted —
which is strong evidence that *"what is left needs hardware"* was the wrong conclusion to have been
drifting toward, and that the right next move is to finish that pass rather than to assume it is
done. §49 was then examined and produced its safety-relevant half, and §56 produced its provenance
classes. §54 was examined and deliberately not started, with its design decision recorded above so
the next session begins from it rather than from the specification. §55 (environments), §64 (errors
as a design surface) and §65 (the golden path) were not reached — the pass is not finished, and on
this session's evidence it should be.

§48.2's power-on self-test is **not** one of them, and checking rather than assuming is the reason
this paragraph is shorter than it started. `crates/skynetd-io/src/post.rs` already runs the three
checks §33.1 permits a supervisor that may not touch an actuator — calibration expiry, black box
writable with its reserve intact, watchdog window nesting — and reports the other five as the
kernel's rather than as unavailable, which is a different and correct sentence. What is missing
there is the kernel-side half, and it needs a loaded RHAL driver: that is the `skynet-cli` driver
loader's territory, not a gap in POST.

Neither repository has been committed or pushed. Both trees are clean of build artifacts and every
check above was run against the working tree as it stands.

---

## 2026-08-31 (seventeenth) — §54, §55, §64 and §65: the Parts IV–VI pass finished, and the engine stops turning ignorance into a pass

The last session examined §54 and deliberately did not start it, writing down the design decision so
this one could begin from the decision rather than from the specification. That is what happened.
The piece was scoped there as *"roughly seven hundred lines across two repositories, touching the
Hub's most-imported module"*, and that estimate was close: about 1,900 lines with the corpus and the
comments, across both.

What the scoping note did not predict is the rest of this entry. **Moving one function from an
application into a contract found four defects in it**, three of which had been shipping for
months, and the fourth of which is a decision the specification had never been asked to make. None
of them were found by looking for defects. They were found because a second edition has to
reproduce the first exactly, and "exactly" is a question nobody asks a function until somebody has
to write it twice.

### What §54.2 asks for, and why it needed a session

> `preconditions` in `SkillManifest` is a list of strings — human prose describing what must hold
> before the skill is safe. Prose is documentation, and documentation does not stop a robot. So
> preconditions become **predicates**, and they are evaluated by the OS before the skill is allowed
> to start.

§54.2 says this *"needs no new machinery"* because the Hub already has a pure predicate engine. That
sentence is true of the Hub and false of the robot, which is §31.2's condition exactly: *a format
with two implementations of its own definition is not a format.* So the engine moved.

| | Before | After |
|---|---|---|
| Definition | `lib/skynet/compatibility/predicate.ts` | `reference/typescript/predicate.ts` |
| Hub's copy | that file | `lib/skynet/os/predicate.ts`, mirrored, with the old path a re-export |
| Rust | did not exist | `crates/skynet-contracts/src/predicate.rs` |
| Held together by | nothing | `corpus/predicate/evaluate.json`, 83 cases |

`semver.ts` came with it, for the same reason and with a sharper edge: `semver_satisfies` is one of
the operators, the Rust workspace had **no range satisfaction at all** — `compare_versions` in
`admission.rs` compares two versions and cannot read `^1.2.0` — and a robot deciding whether its
runtime satisfies a skill's window differently from the Hub that sold it the skill is the failure
in its most expensive form.

The mirror set is now **ten** modules rather than eight, and `predicate` and `semver` are the first
two that arrived *from* the Hub rather than from here.

### The design decision: one evaluator, and the boolean is a view of it

The last session recorded the shape and the constraint. The shape: the Hub's engine is total and
boolean, which is right for a marketplace check and cannot express what §54.2 asks — *"a
precondition that cannot be evaluated refuses the skill, naming the path that was unresolvable"*.
`false` refuses, which is the right direction, but it collapses **"the world is not ready"** and
**"this manifest names something this robot does not expose"** into one answer, and those have
different remedies: charge the battery, or tell the publisher.

The constraint was `satisfied(outcome)` having to equal the Hub's boolean for every corpus case. The
implementation went one better than checking that: **there is one evaluator, and `evaluatePredicate`
is `satisfied(…)` of it.** The refinement cannot become a second opinion because there is nothing
for it to disagree with, and the corpus freezes *both* answers in every case so an edition that
quietly reimplemented one of them is caught disagreeing with itself rather than with the corpus.

Three values, and one closed vocabulary of seven reasons — `malformed`, `unknown_op`,
`missing_path`, `missing_operand`, `unresolved_path`, `not_a_number`, `not_a_version` — because
§54.2 requires a refusal to *name* something, and a free-text reason is the same nothing as a
free-text precondition. All three sets are in `corpus/vocabulary/closed-sets.json`, which is what
the last session added the vocabulary corpus for.

### Finding: the engine was reporting ignorance as a pass, in five shapes

Deriving the boolean changed answers, and the change was not incidental — it is the finding.

| Input | Was | Is |
|---|---|---|
| `not` over a predicate whose path does not resolve | `true` | refused |
| `not` with no operand at all | `true` | refused |
| `neq` against a path that does not resolve | `true` | refused |
| `eq` where neither side resolves — `undefined === undefined` | `true` | refused |
| `includes_all` / `includes_any` whose `__ref` does not resolve | `true` | refused |

Every one of these is JavaScript's `undefined` arriving somewhere that reads it as a fact: at `!`,
at `!==`, and at `[].every(…)`, which is *true* over an empty list. The consequences are not
theoretical:

* `not(eq(robot.mode, "estop"))` — "the robot is not in emergency stop" — **passed on a robot that
  had never reported a mode.**
* `includes_all(robot.capabilities, {__ref: subject.required_capabilities})` — the example in the
  module's own documentation, and the built-in rule the platform's compatibility answer rests on —
  **passed when the policy's capability list was missing.**

A rule that passes because a fact is absent is worse than one that fails, because nobody goes
looking for it. The direction of every change is the same, and it was checked rather than asserted:
a differential harness ran the old engine and the new one over **4,283 predicates × 7 contexts =
29,981 evaluations**.

```
agreed:       23,378
true → false:  6,567
false → true:      36   — every one of them naming a JavaScript host property
semver:  120 satisfaction pairs, 0 diverged
```

`exists` is the escape hatch and always was: it is the one operator whose job is to ask whether a
path resolves, so it answers *unsatisfied* rather than *unevaluable*, `not(exists(p))` still means
what its author meant, and an author who wants "absent counts as false" writes
`and(exists(p), includes(p, x))` — which is what the built-in rules already do. Nearly all of them.

### Finding: one built-in rule was resting on the vacuous truth

`adapter-implements-required-capabilities` asks whether the adapter covers
`subject.required_capabilities`, and guarded that with `exists(adapter.id)` — the adapter half, and
not the subject half. Its sibling two entries above, `robot-capabilities-cover-policy`, guards the
subject half. So with no subject in the context the reference resolved to nothing, `includes_all`
was vacuously true, and the rule **passed rather than being skipped**: a check that had nothing to
check reporting that it had checked. Under the new engine the same input would have become a false
alarm instead, which is how it was found.

The guard its sibling already carried is now on it too. And because "no rule depends on a vacuous
truth" is the kind of property that stops being true the next time somebody adds a rule, it is now
a test: every built-in rule is run against the sparsest context the application ever assembles, and
any rule reaching a verdict it could not evaluate fails the suite.

### Finding: "total" was not true, and one bad database row could take the platform's compatibility check down

The module's first paragraph has said since it was written that it *"never throws on a malformed
rule, it returns false, because one bad rule row must not take down the compatibility check for
every user."* Four shapes threw:

```
{"op":"eq","path":5}                → path.split is not a function
{"op":"exists","path":["a"]}        → path.split is not a function
{"op":"and","predicates":"nope"}    → (predicate.predicates ?? []).every is not a function
{"op":"or","predicates":{"a":1}}    → (predicate.predicates ?? []).some is not a function
```

Rules are operator-supplied rows in `skynet_compatibility_rules` and `evaluateCompatibility` has no
`try`, so this is the exact failure totality was claimed to prevent, reachable from a row somebody
could save today. Both shapes are read defensively now.

The `predicates` half needed a decision rather than a guard. *Absent* and *not a list* had to become
different answers: absent is **no children**, and a conjunction over no children is satisfied — a
skill declaring no preconditions does not fail them — while a `predicates` that is a string is a
**broken rule**, and answering "no children" there would have turned a broken rule into a pass,
which is the failure the whole change is about.

### Decision: a path addresses a member of the document, and nothing else

This one is a specification decision rather than a defect, and it is the one that moved 36 answers
the other way.

`readPath` walked JavaScript property access, so `capabilities.length` resolved to a number and
`robot.constructor` resolved to a function. Neither is a member of the document. An edition without
JavaScript arrays cannot reproduce either, and *a path vocabulary one edition can read is not a path
vocabulary* — so a path now addresses object members and canonical decimal array indices, and
nothing else. `"01"` is not an index, which is JavaScript's own answer for arrays; the prototype
chain is unreachable, which is the right answer for a path composed by somebody else.

The cost is stated rather than buried: **36 evaluations moved from `false` to `true`**, all of them
a negation over a host property — `not(exists(caps.length))` was false when the array was non-empty
and is true now that the path resolves to nothing. No built-in rule uses such a path. A rule row
somebody wrote by hand might, and would now behave differently; that is the price of the vocabulary
being one vocabulary, and it is cheaper paid now than after a second edition shipped.

If a rule ever needs a count, the answer is an operator or a context field, not a JavaScript
property that happens to be reachable.

### `===` on two objects, written down instead of borrowed

The subtlest part of porting, and it would have been silent drift in any edition that skipped it.
`evaluatePredicate` compares with `===`, which on two objects is **reference equality** — so a
literal array in a rule is never equal to anything, and two structurally identical arrays at
different context paths are not equal either, while the *same path read twice* is. An implementation
holding a parsed document has no references to compare, so the rule is now stated in the reference
and reproduced in the Rust:

> Two operands are identical when both are primitives and equal, or when both were read from the
> **same path of the same context** — which is the only way two non-primitive operands can be one
> object. A context whose object graph aliases is outside the specified domain, because a context
> that arrived as JSON cannot express one.

That generalises to membership: an element of a collection carries the path `collection.index`, so a
needle read from `robot.guards.active.1` is the same object as element 1 of `robot.guards.active`,
and both editions agree that it is.

### The Rust edition, and the four places JavaScript had to be reproduced rather than improved

No regex in `no_std`, so the semver parser and the range tokenizer are hand-written against the
reference's two regular expressions. Four behaviours are deliberately not what a Rust author would
write, and each is marked where it happens:

* **Numeric components are `f64`**, because the reference reads them with `Number()`. A version with
  twenty digits of major must round the same way in both editions rather than parsing in one.
* **`compare_versions` answers "equal" when either side does not parse**, because the reference
  does. A comparator returning an error here would make two editions order a malformed list
  differently.
* **JavaScript's whitespace set**, not `char::is_whitespace`. They differ on `U+FEFF`, which
  `trim()` removes and Rust does not — a divergence that would have been found once, in the field,
  by somebody who could not explain it. There are corpus cases for a non-breaking space and a
  byte-order mark inside a range.
* **The range tokenizer is a scan, not an anchored parse**, with the operator as an *ordered*
  alternation that backtracks. So `v1.2.3` yields the token `1.2.3`, `1.2.3.4` yields `1.2.3`, and
  `=>1.0.0` is read as the `>` at its second character. All three are corpus cases, because all
  three are what a hand-written parser gets subtly and differently wrong.

`Number(string)` is its own function — hex, octal and binary prefixes, `Infinity` rejected as
non-finite, and Rust's own `inf` / `infinity` / `NaN` spellings refused because JavaScript does not
accept them. A battery reported as `"17"` has to compare the same way on both sides of the platform.

One asymmetry is deliberate and recorded: `sortVersionsDesc` and `nextVersion` are the Hub's
presentation helpers, nothing on a robot sorts versions or invents one, and porting them would have
added two functions no edition-level consumer calls. And `package.ts` keeps its own numeric-segment
comparator for `min_os_version`, because an image reports `2.0` as often as `2.0.0` and a strict
parser in that position would silently admit a package onto an older OS. Two comparators with two
jobs, and both now say so.

### §54.3, §54.4 and §54.5, which the predicate work had made cheap

With the evaluator in place the rest of §54's *pure* half was a session rather than a week, and it
went in: `reference/typescript/skill.ts` and `crates/skynet-contracts/src/skill.rs`, held together
by 43 corpus cases across four files.

**The substantive decision in it is an asymmetry, and it is worth stating on its own.** A
precondition and a success criterion are the same shape — a sentence and a predicate — and the OS
treats *not knowing* about them in opposite directions:

| | Ignorance means |
|---|---|
| A precondition nobody can evaluate | **refuse** (§54.2: "Not 'assume true'") |
| A criterion nobody can evaluate | **`unlabeled`**, never `failure` (§54.3: "Honest ignorance beats an invented pass") |

That is not an inconsistency. Before the skill runs, ignorance is a reason not to act. After it
runs, ignorance is a reason not to *claim* — and calling an unmeasured run a failure poisons a
dataset exactly as thoroughly as calling it a success. Both halves are one `judgeClause`, and the
difference is entirely in how the results are folded.

The other decisions, each with its corpus case:

* **Prose refuses.** `SkillManifest.preconditions` is `string[]`, and a sentence has no predicate,
  so under §54.2's rule **every skill published today refuses.** That is the point rather than a
  regression: a skill whose safety preconditions are sentences has not been made safe, it has been
  described. The report lists each unenforceable statement separately rather than returning one
  verdict, so it is a publisher's to-do list in the same way `evaluateReadiness`'s blockers are a
  vendor's. What a *guest*-tier robot should do about a prose skill is a tier policy this contract
  deliberately does not decide.
* **The `hold` subset is a phase, not a flag on the answer.** `entry` evaluates every clause;
  `hold` evaluates only the marked ones, which is what runs every cycle. The corpus pins the pair
  over one clause list, and the moment §54.2 describes — the battery falling through reserve
  mid-skill — is a case.
* **The operator may add and cannot remove**, and that needed no enforcement code: the site's
  clauses are appended, a removal would need a mechanism, and not building one *is* the
  enforcement. Publisher's clauses come first so a refusal names the skill's own requirement before
  the site's, which is what tells an operator whether to call the publisher or walk to the cell.
* **Evidence of failure beats absence of evidence.** A false criterion outranks an unreadable one
  even when the unreadable one comes first in the list — there is a corpus case whose whole purpose
  is that ordering, because an edition taking the first non-satisfied clause would answer
  `unlabeled` where the answer is `failure`.
* **A skill that declares no success criteria is `unlabeled`, not `success`.** Scoring yourself by
  declaring nothing is the same gaming §54.3 exists to stop.
* **A skill that declares no duration reports that its timeout cannot be enforced**, rather than
  being given a guessed ceiling. §54.4 says the OS enforces a timeout and does not say what to do
  about a skill with no estimate; refusing it and permitting it are both defensible, and inventing
  the answer inside a pure function is how a policy nobody agreed to becomes load-bearing.
* **A cycle is refused at install with the chain named.** Depth-first in declaration order, so the
  chain a fault names is reproducible — an edition iterating its edges differently would refuse the
  same graphs and blame different skills, which is the half an operator reads. A diamond is not a
  cycle, and there is a case for it, because an edition marking nodes globally visited rather than
  tracking the current chain would refuse a legal graph.

Two details cost more thought than their size suggests. A `predicate` field holding a **number** is
an *absent* predicate (`value && typeof value === 'object'` never reaches it) while one holding an
**array** is a *broken* one that reaches the evaluator and answers `malformed` — different reports,
because they have different authors, and both are corpus cases. And `readCalls` exists in both
editions so that malformed edges are discarded at the same boundary: a typed `SkillCall` in Rust
cannot hold `{ "from": 5 }`, so filtering inside `validateComposition` would have meant the corpus
case for a malformed edge was testing two different functions.

### §55.1, and the package kind the OS did not have

Recorded as a finding an hour before it was fixed, which is the right order: §55.1 says

> **environments are loaded only by a simulator RHAL driver.** Installed onto a robot whose driver
> is physical, an environment package is inert, and the OS says so rather than silently accepting
> it. Its `required_capabilities` are still checked at admission, since an environment that needs a
> gripper is not runnable by a robot without one in sim either.

— and `PACKAGE_KINDS` was `policy | skill | driver | service | embodiment`. Saying so needed the
kind to exist first, which is a vocabulary change across three editions and the corpus rather than
an `if`.

All three landed: `environment` in the kind vocabulary, `simulator` on `DeviceCapacity`, and
`environment_inert` in the finding codes. Three decisions worth their sentences:

* **A warning, not an error.** Installing an environment on a physical robot is not dangerous, and
  refusing it would stop an operator staging a package onto a robot they are about to point at a
  simulator. What is not acceptable is *silence* — a package that will never do anything, sitting
  installed, looking exactly like one that works.
* **`simulator` is a `bool` and absence means false**, which is §52's rule for the same flag on the
  descriptor rather than this reader being lenient: a real robot should not have to declare what it
  is not, the obligation sits on the simulator driver, and the asymmetry is deliberate because a
  simulator presented as a robot is the failure while a robot presented as a robot is not.
* **The daemon reads it from the descriptor**, so `skynetd-io` needed one line and it is the line
  that connects §35.3's declaration to §55.1's decision. A body that is absent entirely is not a
  simulator either, which is the conservative direction.

The vocabulary corpus did its job on the way through: the change broke two cases —
`package_rejection_codes` and `package_kinds` — **before** anything was regenerated, which is
exactly the drift the last session added it to catch, seen from the other side.

### §55.2, the cell descriptor — and a check that could not have existed a day earlier

§55.1's other half. An environment is a simulation artifact; its real-world analogue is the **cell**
— where the fixtures are, which volumes are forbidden, where the safe parking pose is, what the
operator has declared about the space — and §55.2 refuses to make it a repo kind:

> An environment is published, versioned and shared; a cell is site-specific, changes when somebody
> moves a table, and is nobody's intellectual property. Making it a repo kind would invite people to
> publish their factory layout to a marketplace, and would put safety-relevant workspace volumes
> behind a fork-and-star workflow.

So `cell.ts` and `cell.rs` have no publishing, no version and no digest. A cell is a profile document
(§38) that does two jobs, and both are now pinned by `corpus/cell/`: it supplies **`env.*`** to
§54.2's precondition context, and it supplies **allowed volumes** to the `workspace` guard as one
layer of §38.2's stack.

**The interesting part is a fault nothing could have found yesterday.** `env.*` is addressed by a
dot path, so a fact written `{"door.closed": true}` is a fact **no precondition can ever name** —
`env.door.closed` is the path to `closed` inside an object called `door`, and the two are different
documents. It is the obvious way to write a hierarchical key for anybody who has not read
`readPath`, it would have failed silently for ever, and it was not a *defect* until this session
gave `env.*` a path vocabulary to be wrong in. `validateCell` refuses it, and refuses a key that
would shadow `env.cell` or `env.fixtures` — a cell whose facts overwrote its own name would answer a
precondition truthfully about the wrong thing.

Four more decisions, each with its case:

* **A cell that draws no volumes adds no layer**, which is not the same as a cell that allows
  nothing. An empty layer would forbid every frame it named, and a site that has not yet measured
  its cell would find the robot refusing to move with no volume anywhere to explain why.
* **An unreadable volume is reported and withheld from the guard.** A workspace layer silently
  missing the volume an operator drew is a robot with more room than anybody agreed to; a shape the
  kernel cannot evaluate is not a constraint, and both facts have to be true at once.
* **Every fault, not the first** — unlike `validateFrames` next door, and the difference is about
  who is reading. A frame graph that does not form a tree cannot be analysed further; a cell's
  problems are independent and an operator is editing it by hand, so `admitPackage`'s reason applies
  unchanged.
* **A cell with faults still produces a context.** Refusing to would mean a typo in a fixture name
  took away the battery check as well, and §54.2 already has an answer for a fact that cannot be
  read: the precondition naming it is refused, individually, with the path named.

One port decision worth recording because it is invisible from either side alone: **fact keys are
reported in sorted order in both editions.** A `Json::Obj` is a `BTreeMap` and has no insertion
order to offer, so the reference sorts to match rather than reporting in the order somebody typed.
That is the honest answer as well as the compatible one — a canonical JSON document's key order *is*
sorted by the time either edition sees it.

### The oracle had never been type-checked, and that is now a CI job

Found while porting, in the most embarrassing way available: `reference/typescript/predicate.ts`
imported `Predicate` from its own `types.ts`, **which did not export it**, and everything ran
perfectly. Node 23 and later *strip* types rather than checking them, so the generator executed an
oracle whose annotations were decoration.

The Hub type-checks its copies of the ten mirrored contracts. Nothing checked the rest —
`kinematics.ts`, `blackbox.ts`, `shape.ts`, `skill.ts`, `types.ts` and the corpus generator were
outside every check in either repository. A `tsconfig.json` and one CI step close it; the first run
found the missing export and one genuine looseness in the generator (a `string[]` where a control
mode union was wanted). There is still no build step, still nothing to install to run the oracle,
and `npm install --no-save` in CI keeps a checkout free of `node_modules` — the claim in
`reference/typescript/README.md` stays true.

One small piece of housekeeping came with it: `round_half_away` existed privately in
`observation.rs` and `skill.rs` needed it, because `f64::round` is not available under `no_std`
without `libm`. It moved to the crate root rather than being copied, which is §31.2's rule applied
to a five-line function — the second caller is how a private helper becomes two implementations of
one decision.

### §64 and §65 — two sections that describe the repository, and now two checks that say so

The last two unvisited sections of the pass, and neither is a feature. Both are **claims about what
already exists**, which makes them the two that can decay without anybody noticing.

**§64: "every refusal in this system names the fix … a refusal that does not tell you what to change
is a bug report the user has to write themselves."** That is true today and has no test, because it
is not a thing anybody built — it is a property of thirty separate messages. So
`crates/skynet-corpus/tests/refusals.rs` walks every JSON document in the corpus and applies §64's
own table as nine rules: a finding's message must be a sentence and must not restate its code, a
fault must carry a detail, an unevaluable outcome must name a reason, a transport binding must give
a reason *even when it succeeded* ("why is this channel scaled to 0.44" is the same question as "why
was it refused"), a failed conformance check must name what it measured, an unavailable guard must
name its missing input, a refused award must name what is missing.

The bar is deliberately "a sentence" rather than "not empty". The failure this guards against is not
an empty string, which somebody would notice; it is `message: "refused"` — populated, passing every
is-it-set check, telling the operator exactly what the code beside it already told them. All 709
cases pass, and the check was proved able to fail by shortening one message to its own code, which
produced two violations.

**§65: "a vendor engineer should reach a moving robot without reading Parts II through VII, and if
they cannot, the fault is in the golden path rather than in their patience."** The golden path
itself already exists, in `README.md`, five steps long. What it did not have is anything stopping it
from becoming wrong: a path moves, a `make` target is renamed, a CLI subcommand is folded into
another, and the README keeps saying what it said — confidently, to somebody with no way to tell.
Nothing in CI read a README.

`tools/check-golden-path.mjs` reads the three kinds of instruction the developer-facing documents
give and checks each against the repository: every relative link resolves, every `make` target
exists in the Makefile of the directory it names, and every `skynet` command is one the CLI
dispatches — read out of `main.rs`'s own match arms rather than from a second list that could drift
from the first. Four documents, 29 commands, clean; and proved failable by moving a link, renaming a
target and pluralising a command, which produced exactly those three findings.

One false positive was worth fixing rather than suppressing: `cd my-arm && make` names a directory
that does not exist, because the line above it is `skynet driver new my-arm`. Reporting that would
have been reporting the golden path for working, so a directory an earlier line in the same block
created is skipped.

### Where the numbers stand

Rust: **865** tests, up from 810 at the start of the session. `cargo fmt --all --check` clean,
`cargo clippy --workspace --all-targets` clean under `-D warnings`, and `skynet-contracts` still
builds for `thumbv7em-none-eabihf` — the predicate evaluator, the semver parser and the skill
contract are all `no_std` with `alloc`.

Hub: **1,229** tests across 45 files, up from 1,206. `tsc --noEmit` clean, `next build` clean. Both
mirror checks pass, and the contract check failed correctly before the sync — `MISSING
lib/skynet/os/predicate.ts`, `UNPINNED predicate` — which is the behaviour the last session built
into it.

The corpus is **709 cases across 48 files**, up from 483 across 39. Nine new files, and four
cases added to `admission/install.json` for §55.1:

| File | Cases | What it pins |
|---|---|---|
| `predicate/evaluate.json` | 83 | The boolean **and** the outcome, for every operator, every refusal reason, every path rule, and all five behaviour changes |
| `semver/satisfies.json` | 47 | What a range admits, **and whether the range is readable at all** |
| `semver/compare.json` | 14 | Ordering, prerelease precedence, and the parse result beside it |
| `skill/preconditions.json` | 17 | Prose refusing, the `hold` subset, the operator's additions, and what counts as a predicate |
| `skill/composition.json` | 11 | Cycles, depth, the diamond that is legal, and which fault a walk finds first |
| `skill/timeout.json` | 9 | The deadline, the factor that is not a factor, and the skill with no estimate |
| `skill/success.json` | 6 | §54.3's three answers and the order they are decided in |
| `cell/validate.json` | 19 | Every fault a cell descriptor can have, including the two §54.2 made findable |
| `cell/context.json` | 7 | The `env.*` a precondition reads and the workspace layer the guard reads, pinned together |

Plus nine closed vocabularies, taking `vocabulary/closed-sets.json` from 21 sets to 30 — and two
existing sets grew, which is what made the corpus fail before it was regenerated.

The corpus was checked for the failure a corpus can have, which is passing because nothing ran: one
character was changed in the Rust `eq` arm and **18 of 630 cases diverged**, then it was changed
back. Regenerating is a byte-for-byte no-op, which is what the CI job asserts.

### Not done, and what it waits on

§54's pure half is complete. What is left of it is not pure, and the list is short and specific:

* **The lifecycle itself** (§54.4's state machine). `idle → preconditions → acquiring → running →
  verifying → terminal` needs capability **leases**, which do not exist, and §27's state machine,
  which is referred to in prose and declared nowhere — recorded by the fifteenth session and still
  true. Every decision that machine has to make is now frozen and testable; nothing loops.
* **The `hold` re-check at cycle rate.** The evaluator is ready and there is a corpus case for the
  moment it matters; what is missing is the loop, which is the same missing loop as above.
* **§54.5's lease rule** — "leases are held by the root invocation, not re-acquired per call" — is
  the one composition limit that is not a graph property, and it waits on leases.
* **§54.6's four conformance tests.** All four assert runtime behaviour, and there is no runtime.
  `skill.preconditions.enforced` and `skill.success.independent` are now one function call away
  from being writable; they need something to call them.
* **§54.2's `safety_notes` acknowledgement** — "displayed at grant time and acknowledged once per
  skill per robot, recorded in the audit log" — is a Hub and console feature rather than a contract,
  and it is untouched.

§55 is done in both halves. What it leaves is one sentence and one integration:

* **A cell descriptor is not yet loaded from anywhere.** §38.3 puts profiles in
  `/data/state/profiles/` and §38.1 requires a signature, and `skynetd-io` reads neither for a cell.
  The contract is ready and nothing calls it, which is the same shape as `skill.rs` and for the same
  reason — both wait on the daemon rather than on a decision.
* **§38.2's five-layer stack still resolves as one layer list.** `cellWorkspaceLayer` produces the
  cell's, correctly named so an operator can tell which layer stopped the robot, and the vendor,
  fleet, site and session layers around it are still whatever a caller assembles.

**The pass through Parts IV–VI is finished.** §54, §55, §56 (last session), §64 and §65 have all
been visited, and what each of them leaves is recorded above rather than left to be rediscovered.
The next boundary is the one §32.4 named two sessions ago and is unchanged: items 2 and 5 wait on
tables that do not exist — `skynet_os_editions`, `skynet_conformance_results`, `skynet_device_state`,
`skynet_incidents` — and that is a schema decision on a live project rather than a coding one.

The other direction is the runtime. Three contracts now exist with no caller: `skill.rs` waits on
leases and §27's state machine, `cell.rs` waits on the daemon reading a signed profile out of
`/data/state/profiles/`, and `entitlement.rs` has waited since §51. None of the three is blocked on
a decision; all three are blocked on the same missing loop.

### Findings, restated

1. **§52's table cites §25.2 for `sensors[].latency_ms`, and the mechanism is in §25.1.** Unchanged.
2. **The ring's slot layout is a format, and §31.2's list does not name it.** Unchanged.
3. **Nothing consumes `Reading::Stale`.** Unchanged, and still waiting on the control loop rather
   than on a counter.
4. **`resolveTransport()` returns `verdict: "direct"` for a policy that declared no action space.**
   Unchanged. Still the most interesting one, because all three editions agree and are wrong
   together, and the corpus freezes the wrong answer.
5. **§56.2's provenance class and the Hub's `EvaluationSource` are two axes, not two spellings of
   one.** Unchanged; a schema decision on a live project.
6. **`npx eslint` still cannot run in the Hub** — `typescript-eslint` refuses TypeScript 7.0.
   Pre-existing and unrelated.
7. **A compatibility finding whose rule could not be evaluated says the wrong thing.** New, and
   created by this session's own change. `evaluateCompatibility` still calls the boolean, so a rule
   refused for an unreadable path produces its ordinary failure message — "This robot does not have
   every capability the policy requires" — when the truth is "nobody declared them". The engine now
   has `evaluatePredicateOutcome` available and does not use it. Fixing it means a field on
   `CompatibilityFinding`, a UI change and i18n keys, which is a deliberate second piece of work
   rather than something to fold into a contract move.
8. **§55.2 says "which volumes are forbidden" and the `workspace` guard admits only allow-lists.**
   New. The guard's model is that a frame must be inside at least one volume of every layer that
   names it, and layers compose by conjunction — which is what makes "each layer may only narrow"
   true by construction. An *exclusion* volume cannot be expressed that way: the complement of a box
   is not a box, and intersecting to produce one would be the third shape `WorkspaceLayer`'s own
   documentation refuses to invent. §55.2's next sentence says the cell supplies "the allowed volumes
   to the `workspace` guard", so the mechanism sentence and the descriptive one disagree, and the
   contract implements the mechanism. Whether the kernel gains an exclusion is a safety-model
   decision and belongs in a session that is making it deliberately.
9. **§54.4's terminal states and the episode vocabulary are spelled differently.** New, and small:
   the lifecycle diagram ends in `succeeded | failed | aborted`, and §54.4's own next paragraph says
   the outcome matches the Hub's `EpisodeOutcome`, which is `success | failure | aborted |
   unlabeled`. The contract uses the episode spelling, because that is the one that reaches a
   dataset. The diagram is the thing to amend, and amending a shared document is a deliberate act
   rather than a tidy-up.

Neither repository has been committed or pushed. Both trees are clean of build artifacts, and every
check above was run against the working tree as it stands.
## 2026-09-01 (eighteenth) — §27's state machine, declared at last, and the four holes it closes

Six sections describe §27's operating state machine. None of them declared it, and the last two
sessions recorded that as a finding rather than fixing it. The most explicit record is a comment
this repository wrote about its own code:

> The states are §27's, as strings rather than as an enumeration, because **§27's state machine does
> not exist in this codebase yet** — `post.rs` refers to it in prose and nothing declares it. A
> function here that invented the enumeration would be putting §27's design decision in §49's
> module, which is how a state machine ends up with two definitions. Strings until there is one, and
> then this becomes a two-line change.

There is one now.

### What the machine is, and the decision that shaped it

`crates/skynet-contracts/src/opstate.rs` and `reference/typescript/opstate.ts`. Seven states —
§27's `HOLD`, `RUNNING`, `DEGRADED`, `STOPPING`, `SAFE`, with §48.1's `POST` and `UNHOMED` in front
of them — eight observed conditions, seventeen reasons, and one function.

**It is a function of what is observed, not a table of edges**, and that is §33.3's decision applied
to state rather than to packages:

> An edge-triggered supervisor acts on transitions … Miss one edge, because the process was
> restarting or the disk was full or the network dropped mid-message, and the system stays wrong
> forever. A level-triggered supervisor asks "what is true, what should be true, what is the
> difference" and converges from *any* starting state, including states nobody anticipated.

§27 gives a table of **entry conditions**, not a list of edges, and that is the shape it should
keep. `evaluate(previous, conditions)` is total, pure and clockless; every state is reachable from
every other by the conditions changing; there is no edge to miss and recovery is not a special case.
Re-evaluating from the state it produced changes nothing, which is what makes it safe to call from a
loop that runs on a timer and on every event.

**The machine has exactly one bit of memory, and that bit is `SAFE`.** `previous` is an argument for
one reason — §27's "`SAFE` is deliberately sticky" — and being explicit about the one exception is
what keeps it from quietly becoming three. A machine that remembered why it was degraded, or which
policy was running before the stop, would be a machine whose state could disagree with the robot's.

**Every default is the least capable reading.** `Conditions::default()` is a robot with no kernel,
no RHAL, no self-test and homing pending, and it evaluates to `safe` / `rhal_lost`. A caller who has
observed nothing has established nothing, and a machine that read "nobody told me about the kernel"
as "the kernel is live" would be the single most dangerous line in this repository. It is the first
assertion in the suite and the second case in the corpus.

### Two things §27 does not say, and how they were answered without inventing

**Where a cleared `SAFE` goes.** §27 says leaving it "requires an explicit human action" and never
names a destination. Nothing here invents one: a clearance clears the **stickiness**, and the state
is then whatever the conditions say. That has the property worth having and no design in it — an
operator clearing `SAFE` on a robot whose POST still fails, or whose estop is still pressed, gets
`SAFE` straight back, and gets *the reason that put it there* rather than the one they cleared.
`safe_sticky` is reported only when every condition would otherwise have let the robot go.

**Whether `DEGRADED` outranks `HOLD`.** It does not, and this is the one place the machine could
most plausibly have been built differently. §27's `DEGRADED` row says the robot "**runs**, reports,
and refuses any package whose grants depend on the failed guard" — so it is the impaired *running*
state, not a third resting one. The degraded rules therefore sit below the policy rules: a robot with
an unavailable guard and nothing to run is `hold`, because calling it degraded would tell a console a
robot is running while it stands still. Inverting exactly this ordering is the change used to prove
the corpus bites, below.

The rest of the ordering is §33.4's asymmetry, stated out loud: *"anything that reduces what the
robot may do applies immediately … because making a robot less capable is never the dangerous
direction."* The rules are tried least-capable-first, and where two conditions both hold the one that
stops the robot wins — which is also what an operator needs, because the condition holding the robot
is the condition they have to clear.

### §49.1's tone was under-inclusive, and the comment saying why is what found it

The old predicate was `to == "running" && from != "running"` over two strings. `hold → degraded` is a
policy beginning to command a body. It is not a transition into `running`, so **the tone did not
sound** — on a robot with a lapsed calibration, which §45.5 makes the ordinary case rather than the
exotic one. A safety control whose failure mode is silence had a hole in it for exactly the robots
most likely to fall through it.

Phrased over `commands_autonomously()` the two spellings cannot come apart, and the rule now matches
the sentence it implements rather than one example from it. "Resumption after a stop" falls out:
`stopping` and `safe` command nothing, so leaving either for a commanding state sounds.

The function **moved** rather than being fixed in place. It belongs beside the state machine, not
beside package admission — filing it with the reserved-token rule because both sentences are in §49
would have been filing by section number rather than by what the code depends on. `package.ts` keeps
`hri.warning`; `opstate.ts` gets `warningRequired`.

Three tests went with it, including the one whose own comment said it "is the one that should be
deleted" once the states stopped being prose. One of its assertions was
`!warning_required("hold", "degraded")` — the wrong answer, frozen as correct, in a test written to
protect case-insensitivity.

### The four string-typed holes, closed

* **`post.rs`** returned `"safe" | "unhomed" | "post_incomplete"` as a `&'static str` described as
  "the state §48.1's machine should be in after this". It was never quite that. POST supplies **one
  of §27's eight conditions**, and it now returns a `PostVerdict`; §48.4's three words are still
  produced, *derived from* that verdict so the two cannot disagree about a robot. A self-test cannot
  know whether the body needs homing, whether the kernel came up, or whether anybody stopped the
  robot while it ran.
* **`SafeState`** was a newtype whose own comment worried that "false at a call site that also takes
  three other booleans is how a barrier gets inverted", and every caller asserted it anyway.
  `SafeState::from_state` reads it off the machine, and a caller asserting a safe state while a
  policy commands is no longer expressible. `stopping` is the entry that makes it worth having: a
  robot mid-ramp is the state most likely to be declared safe by somebody watching it slow down.
* **`Daemon::refresh_safe_state`** now *narrows* the operator's declaration by the machine rather
  than replacing it — §33.4's asymmetry applied to the two sources of one fact. Either may close the
  barrier; neither opens it alone. On this edition the machine says `safe`, so the declaration still
  decides; when the kernel channel exists and reports a policy commanding, a file somebody left set
  to `yes` stops being able to outvote it. The rule is in place before anything can trip it, and
  there is a test asserting it from that direction.
* **The console's Status panel** said, in as many words: *"State machine — unreported: §27's HOLD /
  RUNNING / DEGRADED / STOPPING / SAFE needs the kernel and the policy runtime, and neither is
  written."* It has its own panel now.

`skynetd` took its first dependency on `skynet-contracts`, and the rule that had kept it out is the
one that let it in: *"adding the dependency before there is code that calls it would make the
manifest a statement about what somebody intends rather than about what this crate does."* There is
code that calls it. CI's `skynetd cannot reach the RHAL` invariant is unaffected — the edge is
`skynet-json`, `skynet-kernel`, `skynet-sha256`, all `no_std`, none of them the RHAL — and the
purity check still passes because the contracts do no I/O.

### `GET /state`, and the field that keeps it from being a lie

The state, the reason, the three properties other sections read off it, and **which of §27's eight
conditions anybody actually observed**. One: the self-test. §33.1 keeps `skynetd` away from the
actuator path, so the kernel, the RHAL, the guard set, the stop and the homing state are on the far
side of a line this process may not cross — which is §48.5's finding about POST's own table arriving
a second time, from a different direction.

That is not a placeholder producing a plausible-looking state. It is the true one, and `post.rs`
already said as much about the same edition: *"`may_run` is false, and that is the correct answer for
a robot with no kernel, no driver and no proven stop path."* What the machine adds is that the answer
now carries a **reason naming the missing input**, instead of being a string three modules spelled
differently.

### Looking at the console found three things, which is now five sessions running

The fixture was rendered and read, which is the practice that has found something every time.

1. **The fixture drew a robot mid-boot.** It never called `refresh_operating_state`, so the panel
   showed `post` / `post_incomplete` while the rest of the fixture described a robot with a body, a
   trust store and four calibration states. The whole point of building the fixture from
   `api::handle` is that the page is rendered against a robot that exists.
2. **"one of nine conditions" was wrong in five places.** There are eight. `Conditions` has ten
   fields, two of which are not observations — `stop_settled` belongs to `stop`, and `cleared_by` is
   an operator's action. A count nothing counts is the kind of number that survives for years.
3. **"Needs a human to leave: yes" read as an errand when nobody was waiting on one.** True —
   `safe` is sticky — and useless: on this robot the *condition* is what is holding it, and a
   clearance would put it straight back. The daemon already distinguishes the two, because
   `safe_sticky` is reported only when nothing else is holding the robot, so the row now says which
   case it is and what clearing would achieve.

The same reasoning caught a fourth in the daemon's own output: the startup line said "safe state is
declared, not measured", which stopped being the whole truth the moment the machine narrowed it, and
`GET /status` reported `safe_state_source: "declared"`. Both now say `declared_and_derived` and name
both sources — and both still say, because it is still true, that nothing here has measured the body.

`GET /status` also gained `state` and `state_reason`, which its own route table has claimed it
carried since the module was written and which it did not.

### Where the numbers stand

Rust: **902** tests, up from 865. `cargo fmt --all --check` clean, `cargo clippy --workspace
--all-targets` clean under `-D warnings`, and `skynet-contracts` still builds for
`thumbv7em-none-eabihf` — the state machine is `no_std` with `alloc`, so an L2 edition on a robot
with no operating system underneath it can still answer what state it is in.

The corpus is **753 cases across 51 files**, up from 709 across 48. Three new files:

| File | Cases | What it pins |
|---|---|---|
| `opstate/evaluate.json` | 34 | Every rule, every reason, both stickiness paths, and the two idempotence cases |
| `opstate/properties.json` | 1 | What a state *means* to §33.4, §49.1 and §27, for all seven |
| `opstate/warning.json` | 1 | All forty-nine ordered pairs of §49.1's tone, in full |

The second and third are there because they are the divergences no evaluation case can see: those
answers are read *off* a state rather than produced by the machine, and an edition could reproduce
every transition while disagreeing about whether `stopping` is a safe state. The warning matrix is
frozen whole rather than sampled because the failure mode of that predicate is silence — and the
version it replaced was under-inclusive for exactly one pair family, `→ degraded`, which a sampled
corpus is not guaranteed to hold.

Eight closed vocabularies were added, taking `vocabulary/closed-sets.json` from 30 sets to 38. The
states and the reasons are emitted in **rule order, not sorted**, and the order is load-bearing
twice: the states run least-capable-first, matching the order `evaluate` descends through, and the
reasons are one per rule in the same sequence.

The corpus was checked for the failure a corpus can have, which is passing because nothing ran. The
degraded rule was moved above the policy rules — the alternative design the ordering deliberately
rejects — and **3 of 753 cases diverged**, naming the states and the reasons. Reverted, all 753
reproduce byte for byte, and regenerating is a no-op.

### Changed in the Hub

`reference/typescript/package.ts` lost `warningRequired`, which makes it a mirrored contract that
moved. `node tools/check-contract-mirrors.mjs` reported `DIVERGED package — 23 line(s) differ` and
`CHANGED package` against its pin before the sync, which is the behaviour the sixteenth session built
it for, on its second real divergence. Synced: `lib/skynet/os/package.ts` and both pin files.
Nothing in the Hub imported the function.

`opstate` is **not** added to the mirror list, and the reason is recorded in
`tools/check-contract-mirrors.mjs` beside the others. Two reasons, and the second is the durable one:

* §27's machine describes what a robot is doing right now, and the Hub has nowhere to put that —
  `skynet_device_state` is one of the four tables §32.4 items 2 and 5 have been waiting on for three
  sessions.
* Every mirrored module answers a question **from declared facts**, which is why both sides can
  compute it and neither is authoritative. §27's machine answers from *observations of a body*, and
  the Hub has none. It would be displaying a robot's answer rather than computing a second opinion,
  which is a renderer rather than a mirrored contract — and mirroring it would suggest a
  disagreement between the two was possible.

The Hub's own suite was run: **1,227 of 1,229 pass**, and the two failures are the in-progress
`botmarket24.com` → `robotoverflow.com` rename sitting uncommitted in that working tree
(`sitemap-urls` and `hreflang-gating` both assert the old domain). `npx tsc --noEmit` reports one
error from the same rename, in `lib/global-delivery`. Neither is this session's, and neither is
anywhere near `lib/skynet/`. The eleven mirror assertions in `lib/__tests__/skynet-os-mirrors.test.ts`
pass.

### What §27 still waits on, and what it has stopped blocking

The machine is complete and eight of its conditions have one observer between them. **What is
missing is not a decision — it is the kernel channel**, and that is the same missing piece §48.5
found for five of POST's eight checks and the fifteenth session found for `Reading::Stale`.

What has stopped waiting:

* **§54.4's skill lifecycle** was recorded as blocked on "capability **leases**, which do not exist,
  and §27's state machine, which is referred to in prose and declared nowhere". Half of that was
  wrong when it was written — `skynetd::lease` has held §25.2's table since the fifteenth session —
  and the other half is now false. What the lifecycle still needs is a loop to run in.
* **§49.1's enforcement point.** "`skynetd` owns the tone" needs an audio device and a state
  transition. The transition exists, is recorded on every evaluation, and carries `warning`.

### Findings, restated

1. **§52's table cites §25.2 for `sensors[].latency_ms`, and the mechanism is in §25.1.** Unchanged.
2. **The ring's slot layout is a format, and §31.2's list does not name it.** Unchanged.
3. **Nothing consumes `Reading::Stale`.** Unchanged, and still the control loop.
4. **`resolveTransport()` returns `verdict: "direct"` for a policy that declared no action space.**
   Unchanged, and still the most interesting one: all three editions agree and are wrong together.
5. **§56.2's provenance class and the Hub's `EvaluationSource` are two axes.** Unchanged.
6. **`npx eslint` still cannot run in the Hub.** Pre-existing and unrelated.
7. **A compatibility finding whose rule could not be evaluated says the wrong thing.** Unchanged
   from the seventeenth session; still a deliberate second piece of work.
8. **§55.2 says "which volumes are forbidden" and the `workspace` guard admits only allow-lists.**
   Unchanged; a safety-model decision.
9. **§54.4's terminal states and the episode vocabulary are spelled differently.** Unchanged.
10. **§27 does not say where a cleared `SAFE` goes.** New, and answered rather than left open: the
    clearance clears the stickiness and the conditions decide. Recorded because it is a place the
    document is silent and this edition is not, and a second edition reading §27 alone could
    reasonably have sent it to `POST` instead. The corpus freezes this answer.
11. **§27's `DEGRADED` row conflates two axes.** New. Four of its states say what the robot is
    *doing* and one says how *healthy* it is, and the two are orthogonal in principle — a robot can
    be holding with a guard unavailable. This edition resolves it by reading the row's own behaviour
    column ("Runs, reports…") as making `degraded` a running state, which means a healthy-but-idle
    robot and an impaired-but-idle robot both report `hold` and are told apart by the health report
    rather than by the state. That is defensible and it is a choice; a document that wanted the
    other answer would need a sixth state or a second field.

### One thing tidied on the way past

`README.md`'s table had been carrying counts from two sessions ago: `skynet-contracts` at 197 tests
against 285, `skynetd-io` at 246 against 249, and the corpus at "483 cases, 39 files, of which 21 are
the closed vocabularies" against 753, 51 and 38. It is §65's golden path — the document a driver
author reads first — and a table of numbers nobody re-derives is one a reader learns to skip.

Neither repository has been committed or pushed. Both trees are clean of build artifacts — the
`node_modules` `npx tsc` needs was removed after the type-check — and every check above was run
against the working tree as it stands.
## 2026-09-01 (nineteenth) — §54.4's lifecycle, and the two things it turned out to need

The previous session's log ended by saying what §54 was still waiting on:

> **§54.4's skill lifecycle** was recorded as blocked on "capability **leases**, which do not exist,
> and §27's state machine, which is referred to in prose and declared nowhere". Half of that was
> wrong when it was written — `skynetd::lease` has held §25.2's table since the fifteenth session —
> and the other half is now false. What the lifecycle still needs is a loop to run in.

It needed less than that. A lifecycle is a decision about *where an invocation is*, and a decision
is a function; the loop is what calls it. So `advance(phase, progress)` is written, and the sentence
that had been true for three sessions — "three contracts now exist with no caller" — is down to two.

### The machine

`crates/skynet-contracts/src/skill.rs` and `reference/typescript/skill.ts`. Six phases, sixteen
fields of evidence, eighteen reasons, one function.

**A phase advances on evidence, and absence of evidence is not advancement.** `entry_permitted` is
an `Option<bool>` rather than a `bool`, and that is the whole discipline in one field: a runtime
that has not evaluated §54.2 yet must not be able to start a skill by *failing to ask*.
`Progress::default()` is an invocation nobody has made, and it stays in `idle`. It is also what
makes the machine safe to drive from a loop faster than the work — calling `advance` every cycle
while the skill's own code runs returns `running → running`, and nothing has to remember it already
asked.

### Three things the document does not settle, decided and recorded

**A refusal has to fit in the Hub's four words.** §54.4's diagram ends in `succeeded | failed |
aborted` and its next paragraph says the outcome is `EpisodeOutcome` — `success | failure | aborted
| unlabeled` — which is the seventeenth session's finding 9, still open. So a skill refused at
`preconditions`, and one denied a lease, are terminal with **`aborted`**, and the reason is what
tells them apart. `failure` would be a lie a marketplace consumes: a skill never allowed to start
did not fail at its task, and a success rate computed over refusals would punish publishers for
operators' cell configurations.

**What stops a running skill, and in what order.** §54.4 names three arrows — timeout, guard trip,
precondition lost — and its `running` row adds a fourth by implication: *"a skill is not an exemption
from the kernel"*. A skill whose robot has stopped commanding is over, whatever stopped it, and §27's
machine is what says so.

The order is **causes before effects**. A timeout makes the OS stop the skill under a controlled
stop; a guard trip stops the robot. Both therefore *produce* a robot that is no longer commanding,
so reporting the robot state first would hide the cause behind its own consequence. The specific
reasons are tried first and `robot_stopped` is the fallback for whatever the machine could not name.

`hold` is deliberately **not** an aborting robot state, and it is the exclusion worth writing down:
it is where §27's machine sits between a lease being granted and a policy actually commanding, so
aborting on it would kill every skill in the moment it started. A guard asking for a hold reaches
the lifecycle as `guard_tripped`, which is the input that means it. `degraded` is excluded for the
opposite reason — §27's `DEGRADED` row runs, §45.5 makes an unavailable guard the ordinary case, and
a skill refusing to proceed on one would refuse on most real robots.

**`root` defaults to true**, and it is the one field whose safe default is the permissive one.
§54.5 says leases are held by the root invocation and not re-acquired per call, so a *nested*
invocation skips `acquiring` entirely. Defaulting to nested would let a caller who forgot the field
past §25.2's arbitration altogether, which is two policies driving one elbow; defaulting to root
costs a redundant acquisition that §25.2 answers `AlreadyHeld`.

### §54.5's other two limits, which the lifecycle made cheap

* **"Leases are held by the root invocation, not re-acquired per call."** The one composition limit
  that is not a graph property, and it is a lifecycle transition: `preconditions → running`, skipping
  `acquiring`, with its own reason.
* **"The root's timeout bounds the whole tree."** `bounded_deadline` is the child's deadline or what
  is left of the root's, whichever is sooner. Two properties fall out and both are the point: a child
  that declares *no* duration is still bounded by its parent, so an unbounded skill nested under a
  bounded one does not make the tree unbounded; and a root with nothing left bounds its child at
  **zero** rather than at a negative number, which some other arithmetic could read as "no deadline".

### The thing that only becomes visible from §54.4: acquisition is all or nothing

§54.4's `acquiring` stage takes leases for a **list** of tokens, and §25.2 was written about one.
The rule it does not have to state, because it is invisible from where it sits:

**A skill that got two of its three tokens holds two joints it cannot use and refuses them to
everything else.** The symptom is a second package that will not start with no reason on any screen
— which is exactly the symptom `release_all` was written to prevent, arriving through the front door
instead.

So `LeaseTable::acquire_all` rolls back everything *this call* took when any token is refused, and
rolls back **only** what this call took: a requester that already held a token keeps it, because it
is not this acquisition's to give away, and a level-triggered loop calling it every pass would
otherwise drop a lease in the middle of being used.

It is also where the projection lives that `skill.rs` promised was at a call site rather than in a
second enumeration. `LeaseState` — `pending | held | denied` — answers the one part the lifecycle
acts on; `PreemptionOutcome` keeps the rest, including which package was displaced by §25.2's single
pre-emption. Two definitions of a lease result would be §31.2's failure in miniature; a projection of
one onto the other is a mapping, and `Acquisition::into_progress` is it.

§25.2's grant-before-holder rule survives the projection, which is the detail worth checking: a
token the requester was never granted reports `holder: None`, because "nobody has the arm, this
package may not ask for it" and "somebody else is using the arm" are different errands.

### §54.6's four conformance additions, half-written on purpose

All four assert **runtime** behaviour and there is still no runtime. What now exists is the half that
can exist without one: five tests that drive the *contract* end to end — the evaluation, the
judgement and the lifecycle together — and assert the answer §54.6 requires, each named for the SCTS
id it stands for.

* `skill.preconditions.enforced` — a false predicate refuses, and the clause's own sentence reaches
  the detail an operator reads. The naming is shared between `evaluate_preconditions`, which finds
  the clause, and the lifecycle, which carries it; neither is complete alone, and asserting the join
  is what makes the conformance test writable.
* `skill.preconditions.rechecked` — the same clause satisfied at `entry` and false at `hold`, with a
  battery that starts at 62% and reaches 11%, driving the lifecycle to `aborted`.
* `skill.timeout.enforced` — an overrunning skill that *claims it finished*, stopped anyway.
* `skill.success.independent` — a false criterion producing `failure` all the way into the terminal
  transition, plus its mirror image: an unenforceable criterion producing `unlabeled`, because a
  conformance test that only checked the failing case would pass on an edition that always reported
  `failure`.

**They are deliberately not added to `SCTS_TESTS`,** and that is a decision rather than an omission.
That catalogue decides what a tier *requires*, and `award_certification` reports a required test with
no result as `missing` — so adding four tests nothing can run would strip the tier from every robot
on the strength of evidence nobody can produce. §54.6 also does not say which tier they belong to,
and guessing one is how a certification rule nobody agreed to becomes load-bearing.

### Where the numbers stand

Rust: **947** tests, up from 902. `skynet-contracts` 285 → 323, `skynetd` 38 → 45. `cargo fmt --all
--check` clean, `cargo clippy --workspace --all-targets` clean under `-D warnings`, and
`skynet-contracts` still builds for `thumbv7em-none-eabihf` — the lifecycle is `no_std` with `alloc`,
so a Guest-tier edition can decide whether a skill may start with no operating system underneath it.

`skynetd` grew its second use of `skynet-contracts`, and CI's two invariants about that crate still
hold: no `skynet-rhal` edge, and no I/O in the pure half.

The corpus is **793 cases across 53 files**, up from 753 across 51:

| File | Cases | What it pins |
|---|---|---|
| `skill/lifecycle.json` | 31 | Every transition, both refusals, all four aborts and their ordering, the two robot states that do *not* abort, and every terminal outcome |
| `skill/bounded-deadline.json` | 6 | §54.5's third limit, including the unbounded child and the exhausted root |

Three closed vocabularies added — `skill_phases`, `lifecycle_reasons`, `lease_states` — taking
`vocabulary/closed-sets.json` from 38 sets to 41. The phases are emitted in **diagram order** and the
reasons in **rule order**, and the first is load-bearing twice over: the suite asserts the machine
never moves backwards through that list, so an edition that sorted it would be asserting a different
property with the same words.

The corpus was checked for the failure a corpus can have, twice, because this machine has two kinds
of decision in it:

* the deadline comparison was changed from `>=` to `>` — one character — and **2 of 793 cases
  diverged**;
* the robot-state check was moved to the top of the abort ladder, which is the effect-before-cause
  mistake the ordering exists to prevent, and **5 of 793 diverged**, naming all four robot states and
  the case built for exactly that question.

Both reverted; all 793 reproduce byte for byte and regenerating is a no-op.

### Changed in the Hub

Nothing. `skill` is not one of the ten mirrored contracts and this session did not change one that
is, so `node tools/check-contract-mirrors.mjs` passes untouched. The reason `skill` is still out of
that list is recorded in the tool and unchanged: it answers questions a repository page will want,
and that page needs UI and thirty-five locale catalogues.

### One check that went red, and it is not this session's

`node tools/check-mirrors.mjs` matched at the start of this session and reports
`DIVERGED docs/skynet-build-plan.md` at the end of it. The Hub's copy changed at 10:47 while this
work was in the OS repository, and the diff is the **`BotMarket` → `RobotOverflow` rename** that is
in flight in that working tree: the title, the ecosystem paragraph, and `botmarket24.com` →
`robotoverflow.com`, 24 lines of it.

It has not been synced, and that is deliberate. `--sync` is the sanctioned remedy and BotMarket is
the origin of all three shared documents, so the direction is not in doubt — but the rename is
unfinished (two of the Hub's own tests and one `tsc` error are still on the old domain), and pulling
a half-renamed specification into this repository would mean the OS's copy of the design says
`RobotOverflow` while the Hub is still deciding. That is the owner's call and it is one command:
`node tools/check-mirrors.mjs --sync ../BotMarket`.

### Not done, and what it waits on

**§54 is finished except for the loop, and the list is now one item long.** Everything §54.4, §54.5
and §54.6 describe that is a decision has been made and frozen. What is left is a runtime: something
that calls `advance` every cycle, runs the skill's policy, writes the black box events §54.4 asks for
on every transition, and emits the episode.

Two contracts still exist with no caller, down from three:

* `cell.rs` waits on the daemon reading a signed profile out of `/data/state/profiles/` (§38.1,
  §38.3). Not blocked on a decision.
* `entitlement.rs` has waited since §51, for the same kind of reason.

`skill.rs` is off that list: `LeaseTable::acquire_all` calls into it, which is one real caller for the
`acquiring` half. The other five phases still have none.

### Findings, restated

1–9 unchanged from the seventeenth session; 10 and 11 unchanged from the eighteenth.

12. **§54.4 says "one episode is emitted per invocation", and a refused invocation is an
    invocation.** New. So a skill refused at `preconditions` — before anything ran, before a lease
    was taken, with no duration and no observations — produces an episode, and §54.4's other
    sentence says skills "generate training data as a side effect of being used". A row in a dataset
    for a skill that never started is not training data. This edition follows the document rather
    than inventing an exception: every terminal transition is one episode, and a consumer can tell
    the empty ones by their reason. Whether the Hub's episode schema should carry that reason is a
    schema decision on a live project, which is the same wall §32.4 items 2 and 5 have been at for
    four sessions.
13. **`is_shared` makes every `sensing.*` token non-exclusive, and `acquire_all` therefore cannot
    fail on one.** New, and benign today, but worth recording because it is invisible from the
    lifecycle: a skill whose `required_capabilities` are all sensing tokens can never be denied, so
    its `acquiring` phase is a formality. That is correct — §25.2 says sensing is shared freely — but
    it means `skill.preconditions.enforced` is the only gate on a read-only skill, and a future token
    that *looks* like sensing and is not would be admitted by a prefix check. §25.2 argues for the
    prefix over a maintained list and the argument still holds; the exposure is recorded rather than
    changed.

Neither repository has been committed or pushed. Both trees are clean of build artifacts, and every
check above was run against the working tree as it stands.

## 2026-09-01 (twentieth) — the loop, and the context nothing had ever built

The previous session's log ended with a list one item long:

> **§54 is finished except for the loop, and the list is now one item long.** Everything §54.4, §54.5
> and §54.6 describe that is a decision has been made and frozen. What is left is a runtime:
> something that calls `advance` every cycle, runs the skill's policy, writes the black box events
> §54.4 asks for on every transition, and emits the episode.

That is what this session built. Writing it turned up a second item nobody had listed — §54.2's
context has seven branches and nothing had ever assembled one — and then a third, which is a bug in
a contract three sessions old that only a caller could have found.

### The half that was missing: `crates/skynet-contracts/src/live.rs`

`evaluate_preconditions` has taken a context as an argument since the seventeenth session and
`predicate::evaluate` since the sixteenth. **Nothing had ever built one.** Every corpus case, every
test and every doc example handed those functions a context somebody had typed, so the question
§54.2 actually asks — *what does a robot put in it* — had never been answered anywhere.

It is a contract rather than daemon code for the reason every other file in that crate is one: the
path a skill publisher writes into a predicate is a promise the OS makes about what it will supply,
and a promise assembled ad hoc inside a daemon is undocumented, untested, and different in the next
edition.

**One rule decides every field.** §54.2's first: *"A precondition that cannot be evaluated refuses
the skill, naming the path that was unresolvable. Not 'assume true', which is what free-text
preconditions do by construction."* So a fact the robot does not know is **absent** — never `null`,
never a plausible zero. That distinction is `skynet-json`'s oldest one and this is the module where
it is load-bearing rather than tidy: a robot with no battery sensor reporting
`robot.state.battery_percent = 0` refuses every skill, one reporting `100` runs every skill, and one
that omits the path refuses only the skills that asked and names the path in the refusal.

Three shape decisions the document does not make, and their reasons.

* **Grants are a list, not an object, and the reason is a dot.** `cell.rs` found from the other side
  that a fact key containing a dot is a fact no precondition can address, and answered by *refusing*
  the key — an operator can rename a fact. A **capability token contains a dot by construction**:
  `arm.left`, `sensing.camera`, `locomotion.base`. So `skill.grants.<token>` is unaddressable for
  every token that exists and nobody can rename them. The shape changes rather than the names:
  `skill.grants.tokens`, sorted, read with `includes` — which is the operation a precondition about
  grants wanted anyway. `robot.frames.resolved` is a list for the second half of the same reason,
  and because §54.2's question about frames is *"whether the frames the skill needs resolve"*, which
  is containment and not lookup.
* **Joints keep the object, and unaddressable names are reported rather than dropped.** The question
  about a joint is its *value*, and a list cannot answer that. The OS cannot rename a body's joints
  either, so a joint called `arm.1` is omitted and named in `unaddressable` — otherwise the symptom
  is a precondition refusing with `robot.state.joints.arm.1` unresolvable and an operator who can see
  the joint on the console.
* **`robot.state.operating` is published**, which is inside §54.2's `robot.state.*` and is not one of
  its four examples. *"Only when the robot is running"* is the first precondition anybody writes, and
  §27's machine is the only part of `robot.state.*` this edition can actually observe.

`skill.grants.tokens` is the **one** branch always present, even empty, and that exception has a
reason the others do not: a grant is a file the daemon owns. "This package was granted nothing" is an
answer. Every other branch is something the OS learns from a driver, a kernel, a clock or an
operator's survey of a room, and any of them can be missing.

That last rule was got wrong first and corrected inside the session, which is worth recording because
the wrong version was defensible. The first draft made `robot.guards.active` always present and empty
too, arguing that "no guard is running" is a fact the OS always has. It is not: guards run inside the
kernel and §33.1 keeps `skynetd` on the far side of that line, so an empty list from this daemon
would be a **claim** rather than an observation — the exact failure the module was written to prevent,
made by the module itself. It is now `Option`, and the two states are different contexts: an empty
list is a kernel reporting no guard is evaluating, and an absent branch is nobody having asked a
kernel. A corpus case pins both.

15 cases in `corpus/live/context.json`, a TypeScript mirror, and 14 tests — several of which assert
against `predicate::evaluate` rather than against the document, because the property that matters is
not what the context *contains* but what a clause reading it *answers*.

### The corpus's §64 check found a shape before a reviewer did

`robot.frames.fault` was a sentence in the first draft. `crates/skynet-corpus/tests/refusals.rs` —
§64 made mechanical, walking every refusal the corpus contains — refused it: a `{"fault": …}` in this
system carries a code, its subjects and a sentence, and a bare string under that key is a refusal
that names no fix.

It is now the frame validator's own `FrameFault`, which is better than the string in a way the check
did not have to argue for: a precondition can name the **code**. *"Refuse when the graph has a
cycle"* is a clause somebody will write, and a string forces them to match on prose.

### `crates/skynetd-io/src/skill.rs` — the runtime

**Decisions in, effects out.** `Runtime::advance_all` starts no process, writes no record and queues
no episode; it returns `Effect`s and the daemon performs them. That is not tidiness. §54.4's terminal
row is *"leases released, outcome recorded, episode emitted"* — three things that must happen exactly
once, on the transition **into** `terminal` and never on a later pass over an invocation already
there. A level-triggered loop performing them inline emits a second episode the first time anything
re-enters the function, and the symptom is duplicate training rows in a dataset nobody is watching.
As a returned list they are assertable, and the suite asserts them: one episode, one release, and a
second pass over a terminal invocation producing nothing at all.

**One live invocation per package.** §54.5 holds leases at the root invocation; §25.2's table holds
them by holder string, and the holder here is the package id. The two agree because a second
concurrent invocation of the same package is refused outright — a skill runs as one unit (§33.5) and
the supervisor is keyed by package id, so two live invocations would be two lifecycles arguing about
one process. Refusing keeps the lease holder equal to the thing an operator sees on the console.

**§54.3 is judged over the same context a precondition reads.** The post-state is the live context
observed after the skill returned, deliberately: a publisher writing `robot.state.gripper` in a
criterion and in a precondition is naming one thing, and a second vocabulary for post-state would be
§31.2's two-definitions failure wearing different clothes.

**Composition is checked against the chain that will actually run**, not the one a manifest declared.
`validate_composition` is reused rather than re-derived — a second implementation of "acyclic and
depth-limited" is a second answer to the same question, and this one has a corpus behind it — and the
edges are built from the live ancestor chain plus the new call.

### What this edition cannot observe, and does not pretend to

§33.1 keeps the daemon off the actuator path, so five of §54.2's seven branches have no source.
`Daemon::live_inputs` is written so that shows, the same way `refresh_operating_state` is:

| Branch | This edition |
|---|---|
| `robot.state.operating` | §27's machine, as of the last observation |
| `robot.calibration.*` | `calibration::status` over `/data/state/calibration` |
| `robot.frames.*` | the descriptor's graph, flattened (§44) |
| `env.*` | §55.2's cell, out of the **signed** operator profile |
| joints, gripper, battery, base pose | absent — no RHAL channel |
| `robot.guards.active` | absent — no kernel channel, and an empty list would be a claim |
| `robot.tier` | absent, and this is a finding rather than an omission |

**Nothing on this robot decides its tier.** §15 awards one from conformance evidence and §11
delivers it in desired state, and neither path is written — so a `tier` in the context would be this
module choosing one, which is precisely the invented fact the arrangement exists to prevent. A skill
whose precondition names `robot.tier` is refused, with the path named, which is the true answer.

**`intervention_count` is absent from the episode rather than zero.** §54.4 asks the episode to carry
it. There is no kernel channel, so nothing counts, and a zero is a number a marketplace reads as
evidence of a clean run. `Cycle::guards_observable` is the switch and it is the same shape
`calibration::conditions_now` uses for a temperature nothing reads. CI asserts the `Option` stays.

### The fifteenth event code

§54.4: *"Every transition is a black box event."* The vocabulary is a closed enum by design (§41.1),
so the transition needed a code rather than a free string: `skill_transition = 15`, in both editions
and in §26's schema text.

**Every *changed* transition, not every evaluation**, and the decision lives in the recorder rather
than in the lifecycle. A skill sitting in `running` for four hundred cycles would fill the ring with
four hundred identical records and push out the cycles an incident needs — the completeness guarantee
destroyed by the thing meant to strengthen it. `advance` is pure and reports what it found; what is
worth a record belongs where the ring is. The consequence is stated rather than left to be found: a
sealed recording shows a skill's **ladder** and not its dwell time, which is in the episode's duration
and in the invocation's own timestamps, both of which outlive the ring.

### `cell.rs` gets its caller, which was the second item on the previous list

> `cell.rs` waits on the daemon reading a signed profile out of `/data/state/profiles/` (§38.1,
> §38.3). Not blocked on a decision.

It waited because nothing consumed `env.*`. The live context does, so the path is now built:
`DesiredProfile` carries the cell document raw, `write_profile` writes it into `active.json`, and
`live_inputs` reads it back through `cell_from_json` and `cell_environment`.

The document travels **inside the profile** rather than beside it, which is how §55.2's *"signed like
every other safety-relevant profile"* becomes true by construction rather than by a second check:
`active.json` is written only for a profile whose signature verified (`reconcile.rs` step 4 emits no
action for an unverified one), so a cell that reaches a precondition is a cell somebody signed. It is
carried raw in `skynetd` because that crate is the pure half and the cell reader lives in
`skynet-contracts`; decoding it there would have added a dependency the reconciler does not need.

Two contracts still exist with no caller, down from three. `entitlement.rs` is the last, and it has
waited since §51 for the same kind of reason.

### A bug in a three-session-old contract, found by writing a caller

§25.2 keeps a **grant** and a **lease** apart, and the fifteenth session's `Acquisition::into_progress`
already reported the distinction correctly: a token the requester was never granted comes back with
`holder: None`, because *"nobody has the arm, this package may not ask for it"* and *"somebody else is
using the arm"* are different errands.

`advance` then collapsed both into one sentence:

```
`arm.left` is held by another package
```

for a token **nobody was holding**. An operator reading that goes hunting for a package that does not
exist, when the fix is one `POST /grants`. That is §64's failure exactly — a refusal that does not
name the fix — sitting in the function §64's own corpus check walks, and invisible to it because the
check reads shapes and this one is a sentence.

It was invisible to the corpus too, and the reason is worth more than the fix: **every lifecycle case
for `lease_denied` supplied a holder.** A case written from the document imagines the contended arm,
because that is the scenario §25.2 spends its length on. The unconfigured robot — the ordinary one,
the one a developer meets first — has no holder at all, and nobody had written it down.

Fixed in both editions, with two new corpus cases: a token never granted, and a denial that names no
token at all (which says so rather than blaming a package).

### Three more corrections, all made during the session

* **`finished` was a silent hang.** The first draft required having *seen* the unit running before a
  missing one counted as a skill that returned, to cover the gap between issuing `Start` and the
  launcher reporting the process. The gap it opened is worse: a unit that starts and exits between
  two passes is never seen running, so the invocation sits in `running` until its timeout — and a
  skill with no declared duration sits there for ever. It is now "not running, from the pass after
  the start", whose own failure mode — a `Start` the launcher refused — is reported on stderr and
  then judged by §54.3 over a post-state nothing changed, which is honest.
* **A cycle is named before "already running".** Every ancestor of a nested call is live by
  definition, so checking `already_running` first reported every cycle as a busy package and hid the
  chain that produced it. The test is `a → b → a` rather than a self-call for exactly that reason.
* **The ancestry edge must carry the parent's package, not its invocation id.** `validate_composition`
  walks a graph of skills; an id in it makes every chain acyclic by accident, because ids are unique
  by construction and a cycle can never close. The test caught it on the first run.

### The surfaces

* `GET /skills`, `GET /skills/{id}` (observer), `POST /skills` (operator), `POST /skills/{id}/stop`
  (operator).
* `POST /skills` answers **202, not 201**, and creates the invocation in `idle`. The reconcile loop
  moves it, so a caller cannot get a skill past §54.2 by calling the route and reading the answer,
  and a skill invoked from a console walks the same ladder as one invoked by anything else.
* `POST /skills/{id}/stop` **makes the skill return; it does not abort it.** §54.4 names four things
  that end a running skill and an operator stopping one is none of them; adding a fifth reason would
  put a word in a closed vocabulary that no corpus case came out of the document. So the process is
  stopped and the lifecycle sees what it would see for any skill whose code returned — which is also
  the more honest answer, because a skill stopped halfway has not met its criteria and is recorded on
  the evidence rather than on the fact that somebody pressed a button.
* `POST /stop` ends every live invocation **first**, then stops the units. The other order would let
  the next pass discover the skills had "returned" and record a `failure` for a robot an operator
  stopped — a defensible-looking number and a lie a marketplace would consume.
* The console gained a Skills panel. `aborted` is deliberately not coloured as a failure: §54.4 is
  specific that a skill never allowed to start did not fail at its task, and a page painting the two
  alike trains an operator to read a cell configuration problem as a broken skill.
* The CLI gained `skynet skills`, `skynet skill run` and `skynet skill stop`, and
  `check-golden-path.mjs` now knows `skill` dispatches a second word — so `skynet skill strat` in a
  README fails the check. Verified by making it fail.

### Three new CI invariants

* **The runtime never decides a skill succeeded.** It may write `aborted`, which is a lifecycle fact
  §54.4 owns. A `SkillOutcome::Success` or `::Failure` literal in `skynetd-io/src/skill.rs` fails the
  job, because those are §54.3's and come out of `judge_success` over post-state. A runtime that can
  write the word is a runtime that can be talked into writing it by the skill.
* **An intervention count is absent rather than zero**, asserted from the direction it would be lost:
  somebody deleting the `Option` to make a field always appear.
* **The live context has no nulls in it.** A `null` resolves, so the clause is answered instead of
  refused, and a robot with no battery sensor starts every skill that asked about its battery.

### Where the numbers stand

Rust: **982** tests, up from 947. `skynet-contracts` 323 → 337, `skynetd-io` 249 → 270. `cargo fmt
--all --check` clean, `cargo clippy --workspace --all-targets` clean under `-D warnings`, and the
TypeScript oracle type-checks.

The corpus is **810 cases across 54 files**, up from 793 across 53:

| File | Cases | What it pins |
|---|---|---|
| `live/context.json` | 15 | §54.2's seven branches, and what a robot that cannot observe one puts there |
| `skill/lifecycle.json` | +2 | §25.2's other refusal, and a denial that names nothing |

The vocabularies are unchanged at 41 — this session added no closed set, which is the right outcome
for a session that built a loop rather than a decision.

Checked that the corpus bites, twice:

* `robot.guards.active` made unconditionally present — the shape rejected above — and **12 of 810
  diverged**;
* the calibration `valid` boolean loosened to "anything but absent", which is the direction that lets
  a guard run on a stale calibration, and **1 of 810 diverged** — the case built to hold all five
  states at once, which is the one that exists for it.

Both reverted; all 810 reproduce byte for byte and regenerating is a no-op.

### Changed in the Hub

Nothing. `skill` and `live` are not among the ten mirrored contracts, and `node
tools/check-contract-mirrors.mjs` passes with every pinned digest intact.

### Not done, and what it waits on

**§54.2's last paragraph.** *"`safety_notes` … is displayed at grant time and acknowledged once per
skill per robot, recorded in the audit log."* Not built, and it is a **grant-time** flow rather than a
run-time one — it belongs with `POST /grants` and the audit log, not with the lifecycle. Building
half of it inside the runtime would put an acknowledgement check on the wrong side of the decision it
is supposed to inform.

**A per-invocation abort.** Deliberately absent. It needs a lifecycle reason §54.4 does not have, and
inventing one puts a word in a closed vocabulary that no corpus case came out of the document.
`POST /skills/{id}/stop` is what exists, and it means something different on purpose.

**The policy runtime, still.** §54's lifecycle runs; nothing a skill's process does reaches a robot.
No capture backend fills a stream, no policy sandbox receives a descriptor, and nothing turns a frame
into a command. A skill on this edition starts a unit, is supervised, is judged and is recorded —
against a body that does not move. The README now says that in the paragraph beside the one about
the policy runtime, because the two are one word apart and a reader skimming would merge them.

### Findings, restated

1–9 unchanged from the seventeenth session; 10 and 11 from the eighteenth; 12 and 13 from the
nineteenth.

14. **A refusal can be correct in its data and wrong in its sentence, and only the sentence is read.**
    New, and the general form of the `lease_denied` bug above. `Acquisition::into_progress` reported
    §25.2's two refusals correctly for five sessions; `advance` rendered them as one sentence, and
    every test and every corpus case checked the field rather than the prose. §64's mechanical check
    walks shapes — a `detail` that is present, non-empty and a sentence passes it — so a *wrong*
    sentence is exactly what it cannot see. The fix is one function; the exposure is that nothing
    checks the other few hundred.
15. **A context assembled by the thing that cannot observe it is where a platform starts lying.**
    New. Five of §54.2's seven branches have no source on this edition, and each of them has an
    obvious, reasonable, one-line default that would make the system appear to work: an empty guard
    list, a tier of `guest`, a battery of 100, an age of zero. Every one of those turns a refusal
    into a pass. The module is written so that the absence is the visible thing and the CI check is
    written from the direction the absence would be removed, because the pressure to fill these in
    will come from somebody making a demo work rather than from anybody deciding to lie.

## 2026-09-01 (twenty-first) — §12, and the gate the document calls the most important test in the suite

`ota` had been in the "not built" column since the first session and `Outcome::Unimplemented` had
existed to name it. §12 gives OTA three rules it calls *"not standard practice"*, and the third one
comes with a sentence the rest of this system is arranged around:

> **OTA is not a route around certification.** An update that changes the Safety Kernel or the
> envelope is **refused** without a fresh bench pass. Without this rule, every guarantee in §7 and
> §15 is one silent update away from being void — so `ota.safety_gate` tests it, and it is the
> single most important test in the suite.

### Rule 1 was already built, and saying so is the first decision

`skynetd::reconcile::plan` has emitted `StageOsImage` with `Gate::Open` and `ApplyOsImage` with
`Gate::SafeState` since the fifth session. That **is** *"never apply mid-task"*, structurally: an
apply cannot be emitted for a robot that is not at a declared safe state.

So `skynet-contracts::ota` implements rules 2 and 3 and deliberately does not implement rule 1. A
second copy of it would be §31.2's failure in miniature — two answers to a question already
answered — and the module header says that rather than leaving a reader to notice the absence. A
test asserts the two gates are still the way round they are, because the property now lives in one
place and nothing else was watching it.

### Rule 3, and the two ways it would be got past

`admit_bundle` takes a release manifest and the robot it would be applied to, and returns every
finding rather than the first — an OS update is the case where each trip round the loop costs a
reboot.

The gate itself is two rules, and each is one line to remove:

**An unrecognised component is gated.** A bundle declares which components it changes. Two of them,
`safety_kernel` and `envelope`, require a bench pass. A publisher wanting to ship a kernel change
past a two-word matcher would not write `safety_kernel`; they would write `core`, or `rt`, or
`platform`. So `gated()` returns true for §12's two names **and for anything this edition does not
recognise**, and the refusal says which word it did not recognise and why that is enough.

The two readings fail in opposite directions and the asymmetry is the whole argument. Strict: an
update waits until somebody fixes a typo. Lenient: §7's guarantees are void and nothing says so.

**A bench pass is bound to a digest.** The same rule §56.3 applies to benchmark seeds, arriving in
another section: evidence that is not bound to what it is evidence *of* is not evidence. A gate
satisfied by *"this robot has passed a bench test at some point"* is satisfied by every robot the
gate exists to protect. A pass for other bytes is `bench_pass_stale`, a pass recording a failure is
`bench_pass_failed`, and a bundle with no digest is refused twice over — once for having no digest
and once for having nothing a pass could be bound to — because an operator who fixed only the first
would still not be able to apply it.

Three findings are **warnings** rather than refusals, and each because refusing would break a real
robot: a bundle declaring no compatible machines (§12 does not require the field, and refusing would
stop every image built before it existed), a machine that cannot report its board (refusing would
make an unknown machine unupdatable, which is the state a robot is in exactly when it most needs an
update), and the version already running (re-converging on a satisfied desired state is the ordinary
case after a rollback).

### Rule 2, and why it is a function rather than a script

`confirm_boot` decides what to tell a bootloader about the slot the robot has just booted into:
`not_pending`, `wait`, `confirm`, `roll_back`. Three properties, and each is why a rule of this shape
is written down rather than left in a shell script somebody wrote once:

* **Absence of evidence is not health.** Every check is an `Option<bool>` and `None` never confirms.
  A supervisor that marked a slot good because it had not yet been told otherwise would replace
  §12's unattended recovery with a race between the health check and the confirmation.
* **The budget is what makes it unattended.** Without one, a robot that never becomes healthy waits
  for ever in the state the rollback exists to leave. With one, the worst case is a reboot.
* **A robot that is not pending decides nothing.** `mark bad` on an ordinary boot is a rollback of an
  update nobody made.

A budget that is not a positive number **waits** rather than rolling back, and that is the one place
in the file where the patient reading is the safe one: an unattended rollback triggered by a
misconfigured budget is a working robot rebooted into an older image for no reason.

### `crates/skynetd-io/src/ota.rs` — the half that opens files

Two documents, both signed, and the second matters more than it looks.

A **release manifest** says what a bundle changes. A **bench pass** says somebody ran the suite
against those exact bytes and it passed. §12's third rule is the conjunction of the two — so an
unsigned bench pass on a robot's disk is §38.1's attacker exactly: *"somebody editing limits on a
robot's disk"*, one file, no network, and the Safety Kernel replaced.

So **a bench pass whose signature does not verify is not a bench pass.** It reaches the contract as
`None` and the gate refuses for want of evidence, with a note saying it was the signature and not the
file — because an operator who can see the pass on the disk and is being told there is none needs to
know which.

Two new trust domains, `skynet.release.v1` and `skynet.bench.v1`, for the reason the other five have
one: a signature over a release digest must not be presentable as a signature over anything else.

`Layout::release` and `Layout::bench_pass` flatten a version and a digest into one path component, so
`1.4.0` and `1_4_0` name one file. Each document therefore declares its own identity and a mismatch
is refused — the same rule `entitlement.rs` applies to a token's `package` field, and it is what
makes the flattening safe rather than clever. Both directions have a test.

`Updater` is a trait, with RAUC behind it and a recording implementation beside it, for the reason
`Launcher` is one: `ota.rollback.works` asks for a deliberately broken image and unattended recovery,
and a rollback that can only run on real hardware is a rollback nobody exercises until a fleet needs
it. The whole ladder — wait, expire, `mark-bad`, keep the marker, and the ordinary boot afterwards
that finds a marker for a version it is not running — is driven in the suite without a bootloader.

### Four decisions in the daemon worth recording

* **The image is assessed at stage *and* again at apply.** Not because anything is expected to have
  changed, but because a stage and an apply are separated by a safe-state barrier that can be hours
  wide (§33.4). In that gap a key can be revoked, a bench pass replaced, or a channel changed.
  Re-running the gate costs nothing and is the difference between a rule that holds and one that held
  when the bytes arrived.
* **Nothing reboots from inside a reconcile pass.** `apply_image` marks the next boot as the one that
  must prove itself and returns. A supervisor whose most destructive action has no confirmation and
  no audit entry of its own is a supervisor nobody should run.
* **The elapsed time comes from `/proc/uptime`, not from the daemon's own start.** §12's budget is a
  *boot* budget, and a daemon that restarted twice while a driver crash-looped would otherwise reset
  the clock on a rollback meant to be unattended. Where there is no `/proc`, the fallback is used and
  the answer says which clock it was.
* **`packages_healthy` is `None` when there are no packages.** A robot the desired state asks nothing
  of has no package health to report, and reporting one would confirm a slot on the strength of an
  empty list — which is how an update that broke every driver looks exactly like an update that
  installed nothing.

### One rollback state that only exists because the marker outlives the boot

A pending marker naming a version the robot is **not** running is the rollback that already happened:
the bootloader returned to the previous slot and the marker survived. It is cleared, and the boot is
reported as ordinary. Leaving it would make the next ordinary boot look pending for ever.

Its opposite is also deliberate: on a `roll_back`, the marker is **kept**. The rollback happens on
the next boot, and a robot that forgot it had condemned the slot it is still running would report a
healthy, pending-free state while running the image it just condemned.

### The surfaces

`GET /ota` (observer) carries the slots, the board, the channel, the verdict and the pending-boot
state together, because they are one question — a console showing the slots without the verdict would
render a robot that appears able to update and is refused every pass. It reads and never decides:
a `GET` that told a bootloader anything would be a state change behind a verb that promises not to be
one.

`skynet ota` prints the same, findings in full. The console gained an OS image panel that lists the
gated components **above** the findings, because *"this image touches the safety kernel"* is true
whether or not the update is refused and is what somebody needs before they go and book bench time.

`EventCode::Ota` finally has a writer: a `confirm` or a `roll_back` is recorded on §26's event
channel. A `wait` is not — it is the ordinary state for most of a boot budget, and a record every
pass would fill the ring with a robot deciding nothing.

### One test changed, and the change is the point

`the_plan_route_reports_a_deferral_with_its_reason` asserted `unimplemented`, using OTA as a stand-in
for *"an action nobody can apply"*. It now asserts `failed`, with a reason naming the release: this
robot is told to run `9.9.9` and holds no manifest describing it, and §12 will not install an image
whose manifest nobody has seen — because the manifest is what says whether the update touches the
Safety Kernel. `Unimplemented` is still reachable and still honest: an admitted release whose bundle
is not on the disk, because §34's inbound fetcher is not written.

### Where the numbers stand

Rust: **1014** tests, up from 982. `skynet-contracts` 337 → 356, `skynetd-io` 270 → 283. `cargo fmt
--all --check` clean, `cargo clippy --workspace --all-targets` clean under `-D warnings`, the
TypeScript oracle type-checks, and the golden path still describes the repository.

The corpus is **842 cases across 56 files**, up from 810 across 54:

| File | Cases | What it pins |
|---|---|---|
| `ota/bundle.json` | 18 | §12's third rule from every direction somebody would come at it, plus §37.1's five signature states over an image |
| `ota/boot.json` | 10 | Every boot decision, both sides of the budget boundary, and the two ways a budget stops being one |

Four closed vocabularies added — `os_components`, `certification_gated`, `bundle_faults`,
`boot_decisions` — taking `vocabulary/closed-sets.json` from 41 sets to 45. `certification_gated` is
frozen **beside** `os_components` rather than instead of it, because it is not the whole rule: an
unrecognised component is gated too, and a corpus holding only the two names would let an edition
drop the half that does the work.

The corpus was checked against the two ways rule 3 would actually be lost:

* `gated()` narrowed to §12's two literal names — the rename hole — and **1 of 842 diverged**, plus
  two unit tests;
* the bench pass unbound from the digest — the stale-evidence hole — and **2 of 842 diverged**, plus
  three unit tests.

Both reverted; all 842 reproduce byte for byte and regenerating is a no-op. Both holes now have a CI
invariant of their own, written from the direction they would be removed rather than asserting the
behaviour, because both are a one-line deletion that looks reasonable in a diff.

### Changed in the Hub

Nothing. `ota` is not one of the ten mirrored contracts, and `node tools/check-contract-mirrors.mjs`
passes with every pinned digest intact.

### Not done, and what it waits on

**The fetch.** §34's outbound queue is written and its inbound counterpart is not, so nothing goes
and gets a bundle. `stage_image` reports `Unimplemented` with the path it looked in, rather than
reporting success for a file nobody delivered. That is the honest half of §12 and it is the half a
robot in a warehouse needs.

**A real A/B device.** None of this has run against real RAUC. `RaucUpdater` shells out to the
commands §12 names and `RecordingUpdater` is what the suite drives; the second is what makes the
rollback testable at all and is not the same as having watched one. `ota.rollback.works` is a bench
test and stays unclaimed.

**Where a bench pass comes from.** This session reads one; nothing produces one. §15's suite runs
on a robot and §57's `POST /devices/{id}/conformance` carries the result, and the loop from *"the
suite passed"* to *"a signed document on this robot's disk"* has no implementation on either side.
Until it does, a fleet applying a kernel update does it by an operator putting a file in place —
which is exactly what §12 intends, and is not automation anybody should mistake for it.

**§54.2's `safety_notes` acknowledgement**, unchanged from the twentieth session: a grant-time flow,
not a run-time one.

### Findings, restated

1–9 from the seventeenth session; 10 and 11 from the eighteenth; 12 and 13 from the nineteenth; 14
and 15 from the twentieth.

16. **A vocabulary can be frozen and still lose the rule it was frozen for.** New. `certification_gated`
    is two strings and freezing it proves an edition spells them the same way — and says nothing about
    whether that edition gates anything else. The rule that carries the weight is *"and anything
    unrecognised"*, which is a **function** and not a set, so the corpus holds it only through the
    cases that exercise a name outside the vocabulary. One case does. That is thin for the rule §12
    calls the most important in the suite, and it is why this one got a CI invariant as well: the
    corpus proves two editions agree, and an invariant proves the surviving edition still has the
    rule.
17. **The most dangerous defaults in this system are the ones that make a demo work.** New, and the
    general form of the twentieth session's finding 15. Every check in `confirm_boot` has an obvious
    permissive default that would make a robot update smoothly on a bench: `post_passed` defaulting
    to true, an empty package list counting as healthy, a zero budget meaning "no limit". Each turns
    §12's unattended recovery into a coin toss, and each would be added by somebody trying to get an
    update to go through on a machine where the self-test does not run — which is every development
    machine. The CI invariant for this is written as a grep for the *defaults*, not for the behaviour,
    because the behaviour is what a hurried change would preserve while removing the reason.

---

## 2026-09-02 (twenty-second) — §10, and the two halves of "connected"

Three sessions in a row have ended with the same sentence on the not-done list, in three different
files: *§10's pairing is unwritten.* `connectivity.rs` said it in the sink that refuses every
outbound item, `ota.rs` said it about the fetch, and the README said it about the fleet. It is the
smallest unwritten thing with the most callers waiting on it, so it is this session's.

### What §10 actually asks for, and why it is three functions rather than one

§10 is four sentences and every one of them is answered by a different party:

* **A pairing code**, typed by an operator off a Hub screen. Answered on the robot, before any
  network exists.
* **A device credential**, minted by the Hub and *"shown exactly once"*. Answered by the Hub, and
  checked on the robot against what §19 says a device credential may be.
* **Hardware binding** — a key in a TPM or secure element, which `security.identity.attested`
  checks. Answered by the hardware.
* **Image attestation** — *"the Hub records not 'robot X is online' but 'robot X is running image
  `sha256:…`, policy `sha256:…`, against body `sha256:…`'"*, which §10 calls the sentence an
  incident investigation needs and nobody can currently produce.

So `crates/skynet-contracts/src/identity.rs` is three functions, not one, and the corpus is three
files for the same reason: an edition could agree about the code and disagree about the credential,
and nothing in the pairing file would see it.

### The code, and the three decisions in eight characters

§18 writes the command out in full — `skynet pair ABCD-1234` — which settles more than it looks.
**Eight characters, two groups of four**, from the document's own example, and Crockford's base32
because a code read off a screen and typed on a keyboard loses to `1`/`I`/`l` and `0`/`O` more than
to anything else. Those letters are **folded** rather than refused: an operator who types the letter
they saw gets the code they saw.

**There is no check character, and that is a decision rather than an omission.** One would turn a
mistyped code into a local refusal instead of a round trip, and it would make `ABCD-1234` invalid —
which is this module overruling the specification for the convenience of a better error message. So
the local check is the *shape*, and whether a well-shaped code exists is the Hub's question, because
the Hub is the only party that can answer it.

**No refusal ever echoes the code.** A pairing code is a bearer secret for the length of a pairing,
and a refusal that quotes it puts it in a terminal scrollback, a shell history and the audit log of a
robot the operator is standing in front of. The faults carry a **position** and never a character.
That costs the operator a little — *"character five is not one a code uses"* is worse advice than
naming the character — and it is the right trade for a secret whose whole lifetime is the minute
somebody is typing it. There is a test that asserts it over every refusal, because this is the kind
of property a single helpful commit removes.

**The order of the checks is the content.** A credential pasted where a code belongs — which is what
an operator holding two secrets at once will eventually do — is caught **before** anything is
stripped, because `skd_` contains an underscore and would otherwise be reported as an unusable
character at position four: true, useless, and a person hunting for a typo in a string that has none.
`skd_x` is five characters, so a parser that stripped first would call it too short. Both have cases.

### §19's `security.credential.scope`, and the direction that survives a rename

§19: *"A device credential is assumed lost. It cannot read repositories, claim jobs, or write to
another namespace."* The Hub has declared no scope vocabulary, so this edition freezes one — §17's
and §57's device-facing routes, one scope each — and says in the doc comment that the *vocabulary* is
a default in the sense `DEFAULT_BOOT_BUDGET_MS` is a default.

What is **not** a default is the direction. It is an **allow-list**, and anything outside it is
refused, for exactly the reason §12 gates an unrecognised OS component: a deny-list of §19's three
phrases would be got past by a credential that spelled repository read any other way, and the one
thing §19 says about this credential is that it is assumed lost. The corpus proves two editions agree
about the vocabulary and says nothing about the direction of the test, so the direction got a CI
invariant of its own — written as a grep for `FORBIDDEN_SCOPES`, because the change that removes it
is somebody inverting the test to admit a scope the Hub added, which reads like a fix.

A credential that declares **no** scopes is a warning rather than an error, and the sentence says the
check did not run here rather than that it passed. That is the ordinary shape of an opaque bearer
token, and refusing it would make this edition unpairable against a Hub that keeps scopes server-side
— which is most of them.

### The finding that only exists because §10 and §50 are both built

§50.2: *"the birth certificate proves **what it is**; pairing (§10) binds it to **whose it is**."*
With both built, the seam between them can come apart, and `key_not_attested` is what it looks like:
the key this robot was paired on is not the key its vendor signed. §50.4 says that is the expected
result of a mainboard swap and not a bug, so the message names the remedy — a vendor-signed re-issue
— and the severity is still an error, because the other reading is a robot paired on a key nobody
attested.

It is also the one error `pair` does **not** refuse on, and the split is worth recording. Findings
about the **credential** — no namespace, the wrong class, a scope nobody recognises — refuse, because
the Hub has just handed this robot something §19 says it should not have and storing it first would
leave it on the disk. Findings about the **machine** report and do not refuse, because whether to
adopt a repaired robot is an operator's decision and a pairing that refused it would leave a repaired
fleet unpairable until somebody found the vendor.

`attested` takes **both** halves — a hardware key *and* the vendor's signature over it. Either alone
is not the property: a hardware key nobody attested is one the robot generated for itself and can say
nothing about what the machine is, and an attested key sitting in a file is an attestation that
copies to any machine somebody puts the file on.

### §10's sentence, and the rule it borrows from §13

`attest` has **no failure mode**. No `Option`, no `Result`, no path that declines to produce a
sentence. That is §13's rule for a recording with a hole in it — seal it, and say it is incomplete —
applied to a report, because the reason is the same: the report most likely to have a hole in it is
the one covering whatever made the hole. A check-in withheld because a digest could not be read is a
robot the fleet believes is **offline**, at the exact moment somebody needs to know what it is
running, and it converts a named gap into a silence nobody can tell from a dead machine.

Each of the three digests has three states, and `nothing` is a real answer for all three. A
guest-tier robot runs *"a container beside the vendor stack"* (§1) and has no Skynet OS image. A
robot between policies is running none. A robot whose driver has not come up has no body — §27's
`kernel: down` — and reporting a body digest for it would attest joint ratings nothing is applying.
`nothing` and `unmeasured` are separate for §37.1's reason: *"there is no policy running"* and *"a
policy is running and its digest could not be read"* are two facts with two different people at the
end of them.

### `crates/skynetd-io/src/identity.rs` — the half that opens files

**Two files.** `identity/device.json` is rendered by the console, returned by `GET /identity` and
quoted by anything that wants to know whose this robot is; `identity/credential` is read by one
caller. A single document would have to be either unreadable by the console or a bearer secret on a
LAN listener. It has a second consequence that decides what a half-finished pairing looks like:
**whether a robot is paired is read off the credential, not off the document**, so an interrupted
write leaves a robot that reports itself unpaired and can pair again, rather than one claiming a
fleet it cannot talk to. There is a test that removes the credential and pairs again.

**The code and the credential never cross a command line.** §34.2's transport is HTTPS, this
workspace has no dependencies, and the precedent for that is already in the tree — `ota.rs` shells
out to RAUC and `launch.rs` to systemd — so the one request pairing needs is made by `curl`. That
buys a maintained TLS stack for nothing and costs one thing that has to be got right:
`/proc/<pid>/cmdline` is world-readable on Linux. The body goes in on **stdin** (`--data-binary @-`)
with the code inside it, and the credential comes back on **stdout**. A version that passed
`--data '{"code":"…"}'` would work, pass every test here, and publish a bearer secret to every local
user for the length of the request. That one has a CI invariant, because it is not a bug anybody
would notice.

**A paired robot does not re-pair.** Doing it silently moves a machine between fleets with nothing
recording which one it left. `unpair` is the way, it is audited, and it does **not** tell the Hub — a
robot that has been unpaired may have been unpaired precisely because it cannot reach one, and an
unpair that needed a round trip would fail exactly when it is needed. The audit entry says to revoke
the credential, because nothing here can.

**Two things this edition cannot measure, and does not pretend to.** The key binding is reported
unmeasured on every robot: `/dev/tpm0` says a TPM exists, not that this key is in it, and the absence
of a private key file says only that this daemon cannot find one. Either probe would turn an
unmeasured robot into an attested one on a bench, which is the twenty-first session's finding 17 —
the most dangerous defaults are the ones that make a demo work. And nothing generates a device key,
because `skynet-ed25519` verifies and signing is a test feature, so the key a robot presents is the
one its vendor put in §50's certificate and a robot without one pairs without a key.

### Two policies is not one policy

§10's sentence names *"policy `sha256:…`"*, singular, and nothing in the design says a robot runs
one — §43 leaves multi-robot on one compute open. So `running_policy` reads the units the supervisor
reports and the manifests in the store, and **two** running policies produce `unmeasured` naming
both rather than one of them chosen by iteration order. A check-in that silently picked the first
would attest software while omitting software, which is worse than attesting nothing.

It reads from the *actual* observation rather than from the desired state, for §33.3's reason: a
robot running a policy the Hub never asked for is exactly the robot §10's attestation exists for.

### The surfaces, and the sink that now gives two answers

`GET /identity` (observer) carries the identity, the findings, §50's certificate and the attestation
together, because §50.2 makes them one question asked of two parties. It carries **no credential** in
any form — not truncated, not masked, absent — and the prefix is the only part of one any surface has
a use for. `POST /identity/pair` and `/identity/unpair` are Admin, which is §39.1's table quoted.
`skynet pair <code|-> --hub <url>`, `skynet unpair` and `skynet identity` are the CLI half; `-` reads
the code from stdin, because §18's own spelling puts it on argv and this is the one thing an operator
can do about that.

There is **no default Hub**, and the refusal says why: §39.2 gives this robot no default password,
and a robot that would report to a host nobody named is the same mistake in a different field.

`connectivity::Unpaired` used to say *"§10's pairing and attestation are unwritten"*, which is no
longer true, and replacing it with one updated sentence would have been the wrong fix. There are now
**two** sinks: an unpaired robot is told to run `skynet pair`, and a paired one is told §34.2's
sending half is missing and which Hub it would have used. Two problems, different people at the end
of them, and the daemon picks between them by reading the identity off the disk rather than assuming.

### The corpus caught a shape before a reviewer did, for the second session running

`CodeResult` was first written flat — `{ code, fault, detail, position }` — and `tests/refusals.rs`
went red. The rule it tripped is that a node carrying `code` and `detail` must have a `detail` that
is a sentence, and on a *successful* parse `code` was the pairing code and `detail` was empty.

The fix was not to weaken the check. `{ code, fault: null | { code, detail, position } }` is the
shape frame validation and the cell descriptor already emit, and it is what puts these refusals
**inside** §64's check rather than beside it. A flat shape would have made every refusal in this
module invisible to a cross-cutting check while looking exactly as correct.

### And opening the console caught the other one

The identity panel rendered, and §10's sentence read *"This robot is running image an unmeasured OS
image (…)"*. The template carried the label and the clause carried the noun, so the two collided on
every branch except the one every test used. The bytes were canonical, the corpus reproduced them
byte for byte, and the sentence was unreadable.

The label now belongs to the clause: `known` carries it and the other two do not, which is how
English works — *an unmeasured OS image* already says what it is. All nine attestation cases were
regenerated and read correctly afterwards, which is the check that the fix was a fix rather than a
different arrangement of the same collision.

That is six sessions running in which looking at the console found something no assertion was
watching for.

### Where the numbers stand

Rust: **1058** tests, up from 1014. `skynet-contracts` 356 → 374, `skynetd-io` 283 → 309.
`cargo fmt --all --check` clean, `cargo clippy --workspace --all-targets` clean under `-D warnings`,
the TypeScript oracle type-checks, `node --check` passes on the console, and the golden path still
describes the repository.

The corpus is **889 cases across 59 files**, up from 842 across 56:

| File | Cases | What it pins |
|---|---|---|
| `identity/pairing.json` | 18 | Every way eight characters are and are not a code, and — readable in the `expected` strings — that no refusal contains what was typed |
| `identity/credential.json` | 15 | §19's scope rule from both directions, §10's two key questions, and the finding that only exists because §50 is built |
| `identity/attestation.json` | 9 | §10's sentence, and the three states each of its three digests can be in |

Five closed vocabularies added — `code_faults`, `device_scopes`, `key_bindings`,
`identity_findings`, `measure_states` — taking `vocabulary/closed-sets.json` from 45 sets to 50.

Four CI invariants, each written from the direction the rule would be lost rather than asserting the
behaviour:

* **`attest` has no failure mode.** The change that removes it is somebody making it fallible in a
  refactor and threading a `?` through the callers, which reads like tightening.
* **The scope check is an allow-list.** The change that removes it is somebody inverting the test to
  admit a scope the Hub added, which reads like a fix.
* **The pairing code never reaches a command line.** `--data-binary @-` is asserted present and
  `--data` asserted absent.
* **The identity document holds no credential.** The change that breaks it is one field added to a
  JSON object by somebody making a check-in easier to write.

### Changed in the Hub

Nothing. `identity` is not one of the ten mirrored contracts, and `node tools/check-contract-mirrors.mjs`
passes with every pinned digest intact. §17's `POST /devices/pair` is a route the Hub does not have
yet; `CurlEnrolment` names it and the suite drives `RecordingEnrolment`, which is what makes every
refusal branch reachable and is not the same as having paired against a real Hub.

### Not done, and what it waits on

**The sending half.** §34.2's check-in, incident upload and episode push. A robot now knows whose it
is and where it would report, and reports nothing — the queue holds and the surfaces say so. This is
the largest single thing between here and a fleet, and it is now unblocked rather than blocked.

**The fetch**, unchanged from the twenty-first session, and now unblocked by the same work: §12's
`stage_image` still reports `Unimplemented` with the path it looked in, because nothing goes and gets
a bundle.

**A real Hub.** Nothing here has paired against one. `CurlEnrolment` shells out to a `curl` that has
never been run against `POST /devices/pair`, because that route does not exist.

**`security.identity.attested`.** No probe, deliberately, for the reasons above. Every robot reports
the binding as unmeasured and no robot claims the property.

**A real A/B device**, and **where a bench pass comes from**, both unchanged from the twenty-first
session. **§54.2's `safety_notes` acknowledgement**, unchanged from the twentieth.

### Findings, restated

1–9 from the seventeenth session; 10 and 11 from the eighteenth; 12 and 13 from the nineteenth; 14
and 15 from the twentieth; 16 and 17 from the twenty-first.

18. **A shape can put a decision outside the check that was written to guard it.** New, and the
    generalisation of what `tests/refusals.rs` caught. §64's check finds refusals by their *shape* —
    a `code` beside a `detail`, a `fault` that is null or an object — which is the only way a
    cross-cutting check can work over a corpus it does not know the contents of. A module that
    invents its own arrangement of the same fields is not caught by it and does not fail it; it is
    simply not seen, and looks identical in review. The corpus caught this one because a *successful*
    parse happened to collide with the rule. It would not have caught a shape that collided with
    nothing, which is the more likely accident.
19. **The most careful thing in a design can be undone by the most ordinary line in it.** New. This
    module refuses to echo a pairing code in an error, keeps the credential out of the document every
    surface reads, and keeps it out of the audit entry — and all of that is worth nothing beside
    `--data "$body"`, which is what somebody writes when they want to see whether the request works.
    The properties that were argued for at length are the ones a reviewer checks; the one that
    matters most here is a flag on a subprocess, and it needed a CI invariant rather than a comment.
20. **Two facts that produce the same silence still need two sentences.** New, and the shape of
    §37.1's rule found somewhere else. A robot that is not paired and a robot that is paired with
    nowhere to send both have a queue that does not move, and the temptation is one honest sentence
    covering both. One of them is fixed by an operator in a minute and the other is waiting on
    software nobody has written, and an operator sent to pair a robot that already is has been told
    something true and useless.

---

## 2026-09-02 (twenty-third) — §17's check-in, and the field the whole rule rests on

The previous session ended with §34.2's sending half named as *"the largest single thing between here
and a fleet, and it is now unblocked rather than blocked."* This is that. A robot that knew whose it
was and reported nothing now reports.

### §17 gives the check-in four fields and one rule

> **The platform never claims a robot is running something it has not heard from.** A deployment is
> `pending` until a check-in.

The check-in is the *only* evidence the Hub has, so a check-in that overstates is a fleet console
that lies and there is no second source to catch it. Everything in
`crates/skynet-contracts/src/checkin.rs` exists to make that rule survive contact with a robot that
is halfway through converging, or waiting for a safe state, or running software nobody asked for.

### `applied` is derived, and it has four answers rather than two

§33.3: *"Convergence is one plan being empty, not one pass succeeding. A supervisor that reported
'converged' because every action it applied succeeded would stamp the generation on a robot that has
not started the policy yet."* So the daemon does not set `applied`; `claim_for` derives it from what
is left to do. Writing it out produced four states rather than the boolean §17 shows:

* **`applied`** — the plan is empty and the generations agree. The only one where §17's field is true.
* **`converging`** — there is work and somebody is doing it.
* **`deferred`** — *everything* left is waiting on §33.4's barrier. A claim of its own rather than a
  flavour of converging, because an operator reading "converging" waits and an operator reading this
  one goes and does something. It requires **every** pending action to be waiting: one action still
  moving means the robot is converging, and reporting a robot that is actively working as stuck is
  how the report that should send somebody to the robot becomes the one they learn to ignore.
* **`inconsistent`** — the plan is empty and the generations still disagree. This is a reconciler
  that computed a plan closing nothing, which is a supervisor that cannot see the difference it
  exists to close. Reported as `applied` it would be a green tick over a bug in the one place nobody
  looks, so it gets a name.

### The one field the rule lives or dies on

**The generation on the wire is the one the robot applied, never the one it was asked for.** A
check-in echoing the desired generation would mark every deployment applied the moment the robot
heard about it, which is precisely the claim §17 forbids the platform from making. It has a test
across all four claims and a CI invariant, and the invariant is written from the direction the field
would be lost: a fleet console shows a robot as behind, somebody traces it to the check-in, and the
obvious edit is to report the number the Hub sent.

### A check-in carries no history, and that is what makes collapsing safe

§34.4 puts check-ins in the class where *"only the newest matters, so older ones are collapsed rather
than queued"*, so *"a week offline should produce one current check-in on reconnect, not ten thousand
historical ones."*

That collapsing is only safe because **every field is a statement about now**. No deltas, no counters
since last time, no event list. A check-in carrying *"twelve guard trips since the last one"* would
make collapsing lossy, and the queue would be discarding data while believing it was discarding
duplicates. It is a property of a document rather than of a function, so it is the second thing this
session gave a CI invariant — grepped for the shape, because the addition that breaks it is one field
somebody adds to make a dashboard nicer.

It is also what makes **enqueueing every reconcile pass** correct rather than wasteful. The queue
holds exactly one and it is always current. The alternative — enqueue only when the document changed
— is edge-triggered, which is §33.3's longest paragraph, and a robot whose check-in was dropped for
want of disk would then never build another because nothing changed.

### §33.6's last four words, which nothing was keeping

> Aggregate health rolls up […] **Every field carries its age.**

`skynetd::health::Health` keeps half of it: per-package readings carry an age and the five aggregate
fields do not. Nothing had noticed because all five are `None` on every robot this edition runs. The
check-in is the first consumer that would put one on a fleet console, so this is where it had to be
closed.

The rule as built: **a value with no age is not reported as a value.** It is dropped and named in
`undated`, apart from `unmeasured`, because *"nobody measured this"* and *"somebody measured this and
lost track of when"* send different people to different places. Passing the number through would be
worse than dropping it — a console renders whatever it is given as current, and the battery reading
from four hours ago is exactly the number somebody drives out on. A negative age is treated the same
way: a reading dated in the future is worse than an undated one, because the second is visibly
missing and the first looks fresh.

`generation_lag` is the one field this daemon can both compute and date, and it is dated with the
observation it came from rather than with the wall clock. The other four are absent, which is the
fact §33.6 asks for and is not the same as zero.

### `connectivity::post_json` — one function, one rule

The previous session put a `curl` call in `identity.rs` for pairing, with the code on stdin because
`/proc/<pid>/cmdline` is world-readable. This session needed a second one for the queue, and copying
it would have been the mistake: **two callers with the same rule is two places for the rule to be
lost, and the second one is always written by somebody copying the first — who copies the shape
rather than the reason.**

So there is one function now, and pairing was moved onto it. Two channels, each carrying what it
must:

* **The credential** goes into curl's own config on **stdin** (`--config -`), which is the only stdin
  curl has. Nothing about it is ever an argument.
* **The body** goes into a file, passed by path. A path is not a secret, and a body can be tens of
  megabytes — a sealed black box — which rules out putting it in the config beside the credential.

A credential containing a quote, a backslash or a control character is **refused rather than
escaped**: the escape set curl's config parser accepts is small and version-dependent, and a
credential with a quote in it is a Hub doing something nobody designed. The file is created with an
owner-only mode *before* it is written, and removed whether or not the request worked.

The CI invariant that guards this has four parts, and the fourth is the one that keeps the other
three worth having: **only one function in this daemon spawns curl.**

### The sink that lasted one session

`connectivity::PairedNoTransport` was written last session for a robot that was paired and had
nowhere to send. Its entire content was a missing implementation, and this session wrote the
implementation, so it is deleted rather than reworded. That is the right lifetime for a type of that
shape — the alternative is a sink that keeps saying something untrue because deleting it felt like
losing work.

What replaces it is `Hub`, chosen by `identity::credential_for_sending`, which is **the one function
in this daemon that returns the credential** and is shaped so a caller cannot get it by accident: it
returns the endpoint, the device id and the credential together or nothing at all, so there is no way
to ask for the secret alone and no reason for any other path to hold one.

### An unpaired robot queues nothing, deliberately

§17's route is `POST /devices/{id}/checkin` and an unpaired robot has no `{id}`. Building one anyway
would put a document addressed to nobody in the queue, where §34.4 would hold it, collapse it and
eventually count it in the drop counters — and a console would then report a robot discarding its own
state, for want of a Hub it was never given. The honest state is an empty queue and an identity panel
saying why, and `GET /checkin` says it in the `queued_note` rather than leaving somebody to infer it.

### Telemetry has nowhere to go, and that is a finding

§34.4 gives telemetry a priority, a drop policy and a counter. Neither §17's eight endpoints nor
§57's revised nine gives it a route. The `Hub` sink refuses it by name and holds the items, because a
robot that guessed a route would be inventing a Hub surface. It is the first place the two documents
have been held against each other closely enough for a gap to show.

### Looking at the console found the fourth one

The check-in panel rendered correctly and listed every health field **nobody had measured**, and
never showed the one that was. A health report that can say what it does not know and never that a
robot is well is one nobody reads twice — and §33.6's age is the half of a reading that decides
whether to act on it, so a panel omitting the value omits the whole point of having required an age.
Fixed on the console and in `skynet checkin`, which had the same shape for the same reason.

That is seven sessions running in which opening the page found something no assertion was watching
for.

### Where the numbers stand

Rust: **1084** tests, up from 1058. `skynet-contracts` 374 → 386, `skynetd-io` 309 → 323.
`cargo fmt --all --check` clean, `cargo clippy --workspace --all-targets` clean under `-D warnings`,
the TypeScript oracle type-checks, `node --check` passes on the console, and the golden path still
describes the repository.

The corpus is **913 cases across 62 files**, up from 889 across 59:

| File | Cases | What it pins |
|---|---|---|
| `checkin/claim.json` | 8 | Every way §17's `applied` can be false, and that the generation is never the desired one |
| `checkin/health.json` | 8 | §33.6's age rule from both sides, and the zero-lag case a falsy check breaks |
| `checkin/document.json` | 4 | The bytes that cross to the Hub, whole |

Four closed vocabularies added — `checkin_claims`, `reconcile_stages`, `readiness_states`,
`health_fields` — taking `vocabulary/closed-sets.json` from 50 sets to 54. `reconcile_stages` is
worth the entry on its own: a fleet console groups by them, and six spellings of one stage is six
columns.

Three CI invariants, each from the direction the rule would be lost:

* **No secret reaches a command line**, in four parts, the last being that only one function spawns
  curl.
* **A check-in reports the generation the robot applied.**
* **A check-in carries no history.**

### Changed in the Hub

Nothing. `checkin` is not one of the ten mirrored contracts and `node tools/check-contract-mirrors.mjs`
passes with every pinned digest intact. §17's `POST /devices/{id}/checkin` is a route the Hub does
not have yet, so nothing here has been received by one.

### Not done, and what it waits on

**A real Hub**, and it is now the only thing between this and a working fleet loop. Every piece on
the robot exists: pair, build, queue, drain, present a credential. Nothing has been answered by a
server, because `POST /devices/pair` and `POST /devices/{id}/checkin` do not exist on the Hub side.
That is the next session's work and it is on the other repository.

**The inbound half.** §11's `GET /devices/{id}/desired` — the robot still reads its desired state off
the disk, so a fleet cannot change one. This is the other half of the loop and it is the smaller
half, because the transport now exists.

**The fetch**, unchanged: §12's `stage_image` still reports `Unimplemented`, and it is now one
`GET` away rather than a transport away.

**§56's evaluations and the episode push.** `Class::Episode` has a route and nothing produces one.

**A real A/B device**, **where a bench pass comes from**, and
**`security.identity.attested`**, all unchanged. **§54.2's `safety_notes` acknowledgement**, unchanged
from the twentieth session.

### Findings, restated

1–9 from the seventeenth session; 10 and 11 from the eighteenth; 12 and 13 from the nineteenth; 14
and 15 from the twentieth; 16 and 17 from the twenty-first; 18–20 from the twenty-second.

21. **A boolean in a specification is often four states in an implementation, and the extra ones are
    where the value is.** New. §17 writes `applied: true`. Writing the function that produces it
    turned up three distinct falses — working, waiting for a person, and a supervisor whose plan
    closes nothing — and the third only exists as a *name* because somebody wrote the branch out.
    Implemented as the boolean it is written as, that state would have reported `applied: false`
    beside a robot that was doing nothing about it, indistinguishable from one mid-fetch.
22. **A rule that is only violated by code nobody has written yet is still worth building.** New.
    §33.6's *"every field carries its age"* was half-kept for six sessions and nothing was wrong,
    because all five aggregate fields were absent on every robot. The rule only becomes load-bearing
    the day somebody adds a battery reading — and that is the day nobody is thinking about the rule,
    because they are thinking about the battery. Closing it while the fields are empty cost an
    afternoon; closing it afterwards would have meant finding the fleet console that had been
    rendering a four-hour-old charge as current.
23. **The second caller is where a security rule dies.** New, and the concrete form of finding 19.
    The pairing exchange got the argv rule right and documented it at length. The check-in needed the
    same request, and the natural way to write it was to copy the working one — which copies the
    shape and not the reason, and would have been the version somebody later "simplified" back to
    `-H`. The fix was not a better comment; it was one function and a CI check that there is only
    one.

---

## 2026-09-02 (twenty-fourth) — §11's pull, and the two refusals that are not symmetric

The loop closes here. A robot pairs (§10), pulls its desired state (§11), reconciles it (§33), and
reports what it did (§17). Every piece of that now exists on the robot side, and the only thing
between it and a working fleet is a Hub with the four routes on it.

### The rule this module exists for is an order

**Fetch, decode, assess, *then* write.**

A version that wrote the response and let §33.8's ladder sort it out afterwards would work almost
always. It would be wrong in the one case that matters: a Hub — or a captive portal, or a proxy, or
a load balancer serving an error page as `200` — returning something that is not a desired state
would overwrite a good one, and the robot would fall back to last-known-good on the next boot having
silently lost the intent it was reconciling against.

§33.8 is a **recovery ladder, not a licence to write first.** It is what a robot does with a file
that got corrupted; it is not a reason to let a network response become that file unchecked.

The invariant for it is written as a **line-order check** — the assessment happens before the write,
and the write is guarded by its verdict — because that is literally what the rule is. Every test in
the suite asserts the file's *contents* rather than when they were decided, so a refactor that moved
the write earlier would pass all of them.

### The two refusals, and why they point opposite ways

`assess_offer` has four findings. Two refuse and two report, and the pair that refuses versus the
pair that does not is the whole content of the module.

**A reused generation is refused.** §11 rests one property on the counter: *"the `generation`
counter makes it idempotent."* A Hub serving two different documents under one generation has broken
that, and the robot is the only party who can see it — the Hub has no way to know this robot already
applied a different 41. The two ways to handle it are not symmetric:

* **Adopt**, and the robot runs the new content while reporting generation 41 as applied. A fleet
  console then shows robots converged on 41 that are running different software — which is exactly
  the claim §17 forbids the platform from making, and it is **invisible**, because every robot looks
  right.
* **Refuse**, and the robot keeps the old content and reports a fault naming the Hub's mistake. A
  console shows a robot that is not adopting and says why; an operator bumps the generation.

One produces a visible, actionable state and the other an invisible wrong one. So it refuses.

**A generation that went backwards is followed.** The opposite call, deliberately. That is a Hub
restored from a backup, and a robot that refused to follow one would be a fleet an operator could not
recover — permanently, with no remedy short of visiting every machine. The replay it would otherwise
guard against is already what the credential and TLS prevent. Adopted, and reported once, because
*"intent went backwards"* is a sentence somebody wants to see.

The other two report and adopt: a document that **empties a running robot** — an operator clearing a
robot and a Hub serving a stub are indistinguishable from here, and stopping is never the dangerous
direction (§33.4) — and nothing at all, for an ordinary poll.

### `changed` is not derived from `adopt`, and that is the most common case

Most polls adopt a document identical to the one on disk. §11's loop asks the same question every few
seconds and the answer is usually the answer it already has. A caller that inferred *"adopted,
therefore write"* would rewrite the desired state on every tick — churning a file whose mtime the
black box and the audit log both care about — so `changed` is its own field, and the io half writes
only when both are true.

The digest it compares is over the **canonical** form rather than the bytes as they arrived, so a Hub
that reflowed its JSON does not read as a changed document. And the file is stored canonically for
the same reason: storing the Hub's own formatting would make the *next* poll report a change against
a document that had not changed. Both have tests.

### The route reports the pull; it does not make one

`GET /intent` carries two things because there are exactly two answers to *"where did this robot's
desired state come from"*: a pull, or §33.8's fallback ladder. A route showing one would leave the
other invisible — and the fallback is the one that matters, because **a robot reconciling against a
last-known-good copy looks identical, from every other surface, to one reconciling against a current
desired state.** That is what the console panel was added for.

It reads `daemon.last_pull` rather than fetching. A `GET` on the local API that reached out to a Hub
would be a network round trip behind a verb that promises to read, and it would let anyone holding an
observer credential make this robot talk to the internet. That one got an invariant too.

### `get_json`, and the count that keeps the rule alive

The pull needed a second curl call site. The previous session's finding 23 said the second caller is
where a security rule dies, so this one was written beside `post_json` rather than inside it — two
functions, eight lines apart, each with the credential going into curl's own config on stdin, so a
reader looking for the argv rule finds it twice rather than once behind a branch.

The invariant is a **count**: every `Command::new("curl")` in `connectivity.rs` must be matched by a
`--config` channel. Two and two today. A third call site written without one fails the build rather
than shipping a credential to every local user.

### Where the numbers stand

Rust: **1107** tests, up from 1084. `skynet-contracts` 386 → 395, `skynetd-io` 323 → 337.
`cargo fmt --all --check` clean, `cargo clippy --workspace --all-targets` clean under `-D warnings`,
the TypeScript oracle type-checks, `node --check` passes on the console, and the golden path still
describes the repository.

The corpus is **925 cases across 63 files**, up from 913 across 62. One new file,
`intent/offer.json`, with 11 cases — every way a response can fail to become a desired state, and
the two that adopt anyway. One new vocabulary, `offer_faults`, taking `vocabulary/closed-sets.json`
from 54 sets to 55; which two of the four refuse is the content of the set.

Three CI invariants:

* **A pulled desired state is decoded before it is written**, checked by line order.
* **Reading the intent does not pull.**
* **Every curl gets its credential on stdin**, checked by counting call sites against config
  channels.

### Changed in the Hub

Nothing. `intent` is not one of the ten mirrored contracts and `node tools/check-contract-mirrors.mjs`
passes with every pinned digest intact.

### Not done, and what it waits on

**A Hub.** This is now the whole of it. `POST /devices/pair`, `GET /devices/{id}/desired`,
`POST /devices/{id}/checkin` and `POST /devices/{id}/incidents` do not exist on the other side, so
nothing here has completed a round trip. Every branch is driven by the suite and none of it has been
answered by a server.

**The bundle fetch.** §12's `stage_image` still reports `Unimplemented`. It is deliberately not a
second call to `get_json`: an OS image is hundreds of megabytes and wants resume, progress and
§34.3's metering, which is its own piece of work.

**Episodes.** `Class::Episode` has a route and nothing produces one.

**Telemetry has no route at all**, unchanged from the twenty-third session and still the one place
§34.4 and §57 do not line up.

**A real A/B device**, **where a bench pass comes from**, **`security.identity.attested`**, and
**§54.2's `safety_notes` acknowledgement**, all unchanged.

### Findings, restated

1–9 from the seventeenth session; 10 and 11 from the eighteenth; 12 and 13 from the nineteenth; 14
and 15 from the twentieth; 16 and 17 from the twenty-first; 18–20 from the twenty-second; 21–23 from
the twenty-third.

24. **When two failure modes are not symmetric, the asymmetry is the design — and it is usually
    visibility.** New, and the shape of both of this session's refusal decisions. A reused generation
    and a reversed one are the same *kind* of Hub mistake, and they get opposite treatment, because
    adopting the first produces a fleet console that is confidently wrong and adopting the second
    produces one that is visibly odd. The rule that fell out is worth stating generally: **prefer the
    failure somebody can see.** A robot that refuses and says why is recoverable by anyone who reads
    the message; a robot that complies and reports the wrong thing is recoverable only by somebody
    who already suspected.
25. **A recovery mechanism is not a licence to be careless upstream of it.** New. §33.8's ladder is
    good, thorough and tested, and it made "just write the response and let the ladder handle it"
    sound reasonable — it *would* have handled it, in the sense that the robot would keep running.
    What it would not have recovered is the thing that was lost: the intent the robot was
    reconciling against, gone, with a last-known-good copy that might be weeks old standing in for
    it. A fallback restores service, not state, and the two are only the same thing in a system that
    has none.

---

## 2026-09-02 (twenty-fifth) — the Hub answers, and the three digests the sentence had swallowed

Twenty-four sessions built a robot that pairs, pulls, reconciles and reports, and the last four ended
with the same sentence: **there is no Hub on the other side.** This session is the other side.

It opens with a finding that had to be closed first, because everything the Hub stores about a robot
comes through the one document the robot sends.

### §10 asks for three digests and the wire carried a sentence

> The Hub records not "robot X is online" but "robot X is running image `sha256:…`, policy
> `sha256:…`, against body `sha256:…`" — which is the sentence an incident investigation needs and
> nobody can currently produce.

`attest` produces that sentence and has since the twenty-second session. `check_in_to_json` put
`attestation_report_to_json(&attest(…))` on the wire — `complete`, `sentence`, `unmeasured` — and
**dropped the three measures the sentence was rendered from.**

Nothing was wrong on the robot. The sentence is correct, the console renders it, and every assertion
about it passed. What was missing only becomes visible from the Hub end: §32.1 puts a **running
digests** column on `/skynet/os/devices`, and *"which robots are running image `sha256:aa`"* is a
query. The only way to answer it from a sentence is to parse English out of prose that is
deliberately not machine-readable — `no policy`, `an unmeasured OS image (the slot could not be
read)`, `an unnamed body`. Those renderings are right for a person and useless for an index, and a
Hub that regex'd them would be re-deriving a fact the robot had measured and thrown away.

**The shape already existed, in the wrong place.** `skynetd-io::identity::attestation_to_json` — the
answer `GET /attestation` gives the LAN console — carried the report *and* the three measures, and
had done since the session that wrote it. So for two sessions the console on the robot could see the
digests and the Hub could not, and the document that is *"the only evidence the Hub has"* was the
poorer of the two.

That is the twenty-third session's finding 23 in its other form: two callers, one rule, and the one
written second got the smaller half. The function moved into `skynet-contracts::identity`, the
daemon's copy is now a one-line delegation, and the check-in carries:

```
"attestation": { "complete": …, "sentence": …, "unmeasured": [ … ],
                 "os_image":   { "digest": "sha256:aaaa", "state": "known",      "why": null },
                 "policy":     { "digest": null,          "state": "nothing",    "why": null },
                 "embodiment": { "digest": null,          "state": "unmeasured", "why": "…" } }
```

`digest` and `why` are **present-and-null** rather than omitted, for the reason §33.6's five health
fields are: a consumer that has to discover which keys this robot happened to fill in is a consumer
that gets it wrong on the robot that filled in none.

The regression test asserts on the digest **as a field** rather than on its presence anywhere in the
document, because the sentence contains it too — a looser check would have passed against the exact
version this test exists to prevent. The CI invariant greps both directions, because the edit that
loses this is a revert to the shorter function name, and the shorter one compiles, passes every
assertion about the sentence, and renders correctly on every console.

**Where the numbers stand after part one.** Rust: 1120 tests. `cargo fmt --all --check` clean,
`cargo clippy --workspace --all-targets` clean under `-D warnings`, the TypeScript oracle
type-checks, and `node tools/corpus-gen/generate.ts` reproduces — the corpus's four `checkin/document`
cases changed shape and both editions agree on the new bytes, which is the whole point of having two.

### §11's counter says *which intent*; nothing said *which document*

The second finding fell out of writing the Hub, and it is the same shape as the first.

§11 rests idempotence on the generation, and `assess_offer` enforces it — a *different* document
under a generation this robot has already applied is refused, loudly, naming the Hub's mistake. The
twenty-fourth session spent a long paragraph on why that refusal is right, and the paragraph is
still right. What it did not notice is that **the refusal is the robot's alone.**

The Hub knows which generation a robot has applied. It has never had any way to ask *which
document*. So the case §11 names by name — a Hub restored from a backup — plays out like this: the
Hub serves generation 41 with different contents, every robot in the fleet refuses it, every robot
reports generation 41 as applied, and the fleet console shows a hundred machines that have quietly
stopped converging with no field anywhere that says why. The information exists. It is on each
robot, one comparison away, and it never crossed.

So the check-in carries `desired_digest` now: `sha256:…` over the canonical form of the document the
robot is reconciling against, empty when it holds none.

**The test it had to pass is §34.4's.** Check-ins are collapsed — *"only the newest matters"* — and
that is safe only because every field in one is a statement about **now**. A field about the past
would make the collapse lossy while still looking like a duplicate, which is the twenty-third
session's CI invariant. A digest of the document currently on disk is exactly as present-tense as
the `generation` sitting beside it, and the invariant's grep agrees.

Empty rather than absent for a robot that holds nothing, for §33.6's reason applied one field over:
*"reconciling against nothing"* is a state somebody acts on, and a consumer that has to discover
which keys a robot happened to send is a consumer that gets it wrong on the robot that sent none.

`held_digest` is its own function in `skynetd-io::intent` because two callers now need it for two
reasons — `held` compares it against an offer, the check-in puts it on the wire — and a second
spelling of *"what document is this robot running"* is a second answer to it.

### The Hub, which is the other half of everything above

On the other repository, and the whole of it is in that session log. In summary: three tables
(`skynet_pairing_codes`, `skynet_device_state`, `skynet_incidents`), five routes, and the three
contracts this repository owns mirrored into it — `identity`, `checkin` and `intent`, taking the
mirror set from ten to thirteen.

The reason those three and not others is the same test the mirror list has always applied: **both
sides must be able to compute the answer from declared facts, and neither is authoritative.**

* `identity`, because a pairing code that one side folds and the other does not is a code an
  operator typed exactly as printed and got refused for.
* `checkin`, because §17's rule is enforced by whichever side decides what `applied: false` means.
* `intent`, because `assess_offer`'s own header asked for it two sessions ago: *"the same function
  runs on the robot and on a Hub checking what it just served."* That sentence is now true.

`opstate` (§27) was the name most likely to join, and it did not. The table it was waiting on exists
now, which removes the *first* of its two reasons and leaves the second, which was always the real
one: §27's machine answers from observations of a body, and the Hub has none. It would be running
the evaluator over conditions a robot sent it, which is displaying a robot's answer rather than
computing a second opinion — a renderer, not a mirrored contract.

### One thing the Hub found in this repository's tooling

`tools/check-contract-mirrors.mjs` looks for the Hub beside this checkout under four spellings of
`BotMarket`. The Hub was renamed to Robot Overflow, so for however many sessions that has been true
the tool has printed *"BotMarket not found"* and fallen back to checking its own pins — which is the
degraded mode written for CI, doing its job silently on a machine that had both trees. Both names
are in the list now. It is a two-line fix and it is worth the paragraph: the tool's own header
spends four paragraphs on the danger of a check that cannot fail, and it had quietly become one
again for a reason nobody would go looking for.


### §12's manifest, which existed on neither side

The third piece, and the one that made the session's shape obvious in retrospect.

`admit_bundle` has answered §12's third rule — *"an update that changes the Safety Kernel or the
envelope is refused without a fresh bench pass"*, the rule the document calls **the single most
important test in the suite** — since the twenty-first session. It is pure, corpus-verified, and
wired into `stage_image` at exactly the right point. In four sessions of running it has never
returned anything but *"nothing on this robot describes release `1.4.0`"*, because `assess` reads a
release manifest from `Layout::release` and nothing has ever put one there.

The gate was built, tested, connected, and had no input. `skynetd-io::releases` is the input.

**The fetch is split, and the split is the design.** §12's transfer is two transfers with nothing in
common: a manifest is a few hundred bytes of JSON saying *what an image would change*, and a bundle
is hundreds of megabytes wanting resume, progress and §34.3's metering. Fetching the small one first
is not an ordering convenience — **the third rule is decided entirely from the manifest**, so a
robot that holds one refuses an update it must not take *before* spending a hundred megabytes of
somebody's metered link discovering the same thing. The expensive transfer belongs after the
decision, and until now the decision had no input to be made from.

So the bundle fetch stays unwritten and `stage_image` still reports the same honest
`Unimplemented`, and that is not the same list item it was this morning: what was *"§12 cannot
decide"* is now *"§12 decides, and cannot act"*.

**A release manifest is immutable, and this refuses to pretend otherwise.** §12 binds a bench pass
to a digest because *"evidence that is not bound to what it is evidence of is not evidence."* A
manifest is the other half of that binding — it says which components those bytes change — so a Hub
serving different content under one version has broken it exactly as a Hub serving a different
document under one generation breaks §11. It gets the twenty-fourth session's treatment for the
twenty-fourth session's reason: adopting produces a robot judging an update against a description of
some other image, invisibly; refusing produces a robot that says the Hub changed a release under it.
**Prefer the failure somebody can see.**

**One manifest, not a catalogue.** The desired state names one version. Caching the rest would put
statements about images this robot will never install on its disk, where an operator listing the
directory has to work out which are real.

**`arch` comes from `std::env::consts::ARCH`, not `uname`.** It is what this executable *is*, and a
robot running an aarch64 supervisor under emulation on an x86 host would have `uname` answer the
wrong question. The one `GET /os/releases?arch=` asks is which image boots the machine this code is
already running on.

### Opening the page found the eighth one

The OTA panel renders `assessment.because`, and before this session that sentence had exactly one
value on every robot in existence. It has four causes — nobody published this version for this
architecture, the Hub was unreachable, the robot is not paired, the Hub changed a release under its
own version — and they want four different people. The panel now carries the release pull under the
verdict it feeds, and the last of the four gets the loudest treatment on the page, because it is the
only one that is somebody else's mistake and the only one an operator cannot fix from the robot.

That is eight sessions running in which opening the console found something no assertion was
watching for.

`skynet releases` joins the CLI as the forty-second command, and `GET /releases` joins the local
API — reporting the last pull rather than making one, for `GET /intent`'s reason and one more: this
request carries the device credential and goes out over whatever bearer is up, so a read on the LAN
that triggered one would let an observer spend a fleet's metered allowance a request at a time. The
existing invariant was widened to cover it.

### §15's evidence, and the certificate that stops being a document

The fourth piece, and the second one this session that found a contract with no consumer.

`award_certification` and its thirty-six tests have existed in both editions for many sessions and
have never been given a result to weigh, because §57's `POST /devices/{id}/conformance` did not
exist and nothing stored one. It exists now, over two tables that are separated on purpose:
`skynet_conformance_results` holds **what somebody measured** and `skynet_certifications` holds
**what somebody awarded from it**, and every read of the second recomputes from the first.

The reason is §15's own closing sentence — *"the third is the one nobody else does, and it is where
certification stops being a document and becomes a live property"* — and it rules out the obvious
schema. A tier stored on a robot is right on the day it is written: bench evidence lapses after a
year, and field conformance below 0.999 **suspends** an award rather than downgrading it.

Two things the Hub had to name rather than assume, and both are this session's pattern again:

* **Nothing is signed.** §57 says *"signed SCTS results"*; no runner on any robot emits a signature,
  so `verified` is false on every row. §12's gate — the single most important test in the suite — is
  reading evidence that is asserted rather than attested, and that sentence is now in the route.
* **Field conformance is `undefined`, not `1`.** §34.4 gives telemetry a priority, a drop policy and
  a counter, and neither §17 nor §57 gives it a route, so nothing carries the cycle counts. Passing
  1 would award on evidence nobody collected — which is the exact failure §15's third suite exists
  to prevent, committed by the platform that defined it.

Nothing changed on the robot for this. That is worth recording: it is the first piece all session
where the OS side was already complete and correct, and the gap was entirely that no second party
had ever been built.


### §74 — the one place the two documents did not line up

The fifth piece, and the only one this session that changed the specification.

§34.4 gives telemetry a priority, a drop policy, a counter and a paragraph of its own. §17 lists
eight endpoints and §57 lists nine, and **none of them is a route telemetry can be sent to.** Three
sessions recorded that as a finding and left it open, which was right at the time: from the robot's
side it looks like a queue that fills, and from the Hub's side it looked like nothing at all, because
there was no Hub.

Building the Hub made it a decision rather than an observation, because **two things upstream turned
out to be waiting on the same absence**:

* **§15's field conformance** — the suite §15 calls the one nobody else does, and the mechanism by
  which certification *"stops being a document and becomes a live property."* Without a route it is
  permanently unmeasured, and the fourth piece of this session had to pass `undefined` into
  `award_certification` and write a paragraph explaining why.
* **§32.1's guard trip rate.** A column on the fleet page, and one a check-in **structurally cannot
  carry**: a rate is a count over a window, and §34.4 collapses check-ins precisely because every
  field in one is a statement about now. A rate in a check-in would make the collapse lossy.

So the architecture document has a Part X and a §74, and `POST /devices/{id}/telemetry` is the tenth
endpoint. Amending a normative document is not a thing to do quietly, so the reasoning is in the
section rather than here.

### The shape came from three rules that already existed

A telemetry item is a **window**: a half-open interval carrying counts over it. Each alternative
breaks something:

| Shape | Breaks |
|---|---|
| A series of samples | §34.4 — a week offline is ten thousand items, which is what collapsing check-ins exists to avoid |
| A running total since boot | §47.4 — the next total absorbs a drop and the series stays tidy |
| Counts since the last successful send | Makes delivery load-bearing for correctness; a retry that succeeded twice double-counts |

What windows buy is §47.4's rule **from the shape of the data rather than from an annotation**: a
dropped window loses that window and nothing else, and the loss is visible because the windows no
longer tile. A Hub holding `[10:00, 10:05)` and `[10:15, 10:20)` knows exactly what it does not have,
and nobody had to remember to say so.

**Both clocks, and their disagreement reported.** §35.1 already assigns them — wall time for
*"correlation with the Hub"*, monotonic for anything measured — and a window does both jobs, so it
carries both. Wall time as a denominator would let an NTP step turn four hundred guard trips into a
plausible rate with nothing looking wrong; monotonic bounds would make two robots' windows
incomparable. `(to − from) − elapsed` is the skew, and it is a fact about the robot rather than a
reason to discard a measurement.

**Six of the eight counters are unmeasured on every robot this edition runs**, and they are absent
rather than zero. §74.3 is §33.6's rule one field over, and it is not academic here: four need a
kernel, one needs teleoperation, one needs the supervisor to count restarts. The two that are real
are `queue_dropped` — §34.4's own counter, the one the section is written around — and
`reconcile_passes`.

### Two small decisions that are the whole design in miniature

**There is no timer.** A window is closed by the pass that opens the next one, so a robot that stops
reconciling stops producing windows — and the gap is then visible on the Hub as a gap, which is
exactly what a heartbeat on a separate timer would have hidden.

**The counters live in memory and a reboot ends a window.** A counter that survived a reboot would be
a running total in disguise. The counts in the open window are lost and the *gap* is what the Hub
sees, which is the honest report of a robot that restarted.

### `assess_window` runs on both sides and refuses on both

The robot will not enqueue an unusable window — the one place this differs from a check-in, which is
enqueued whatever holes it has, and the reason is the drop policy: a check-in is level-triggered
state a fleet needs even when incomplete, and a window with no duration can produce no rate and no
coverage, so queueing it would spend the budget of the class §34.4 drops *first* on an item that can
never be worth anything.

The Hub refuses the same window with the same function, and **names it back** in the response. That
matters more than it looks: a robot whose clock has moved under it is producing windows nothing will
ever count, and its own queue accounting would show them as delivered.

### The test that had been pinning the finding

`every_class_but_telemetry_has_somewhere_to_go` is now `every_class_has_somewhere_to_go`, kept in the
same shape rather than deleted, with a general assertion added: no class is silently unroutable. The
day a fifth class arrives, that is where the decision will be made.

`Episode` is now the only class with a route and no producer.

### And the page it lands on

`/skynet/os/devices/[id]` renders the windows: how many, **observed against spanned**, the intervals
with no evidence, and either the field-conformance figure or the sentence saying which of the two
absences it is.

Observed against spanned is the pair worth having on a screen, and it is the one number a fleet page
would otherwise get wrong. They are equal until §34.4 drops something, and §34.4 drops this class
*first* — so the moment they diverge, the difference is exactly the time nobody has evidence for.
§15 divides by the first; an operator reading the second would believe they had a day of data.

§32.1 asks that page for four things. Three were named gaps this morning; the fourth, guard trip
rate, came off the list when windows got somewhere to arrive.

### §45, and the gap that narrowed rather than closing

The sixth piece. `GET|POST /devices/{id}/calibration` is the eighth of §57's ten, `calibration` is
the sixteenth mirrored contract, and §45.2's table is why: `guards_unavailable` converts *"we should
calibrate the cameras sometime"* into *"an expired extrinsic disables the proximity guard"*, which
is §32.1's ask in its own words.

**Absent is not valid.** §45's own warning — *"treating never calibrated as no evidence of a problem
is what puts a proximity guard on a robot whose camera transform is whatever the CAD model said"* —
so a set that applies to the body and has no record is `absent`, its guards are down, and the page
lists it rather than showing an empty table that reads as a clean bill.

**The Hub can judge half of §45.4, and says which half.** Age and invalidating events are in the
record; joint travel and temperature are properties of the robot at the moment of asking. So the
reporter sends the conditions it observed, every verdict carries the age of the reading it was
judged against, and the answer names the rules it could not check at all. The clock is aged forward
and the temperature is not — a record judged against a two-hour-old `at_s` would report valid for two
hours after it expired, and a temperature from an hour ago is not a temperature now.

**The gap narrowed rather than closing**, and the entry was rewritten rather than deleted: a guard
can also be unavailable for want of a sensor, a descriptor or a driver that has not come up, and
none of those crosses. Two of §32.1's four asks are answered, one is half answered, one is named.

Nothing on a robot reports a calibration yet. `skynetd` holds records and serves them on its own
console, and no §34.4 class carries them — the bench path is what works today, which is the right
half to have built first, because §45.3 records *who* took a calibration and a technician with a
fixture is one of the two answers.

### Where the numbers stand

Rust: **1132** tests, up from 1119 at the session's start. `cargo fmt --all --check` clean,
`cargo clippy --workspace --all-targets` clean under `-D warnings`, the TypeScript oracle
type-checks, `node --check` passes on the console, and `node tools/check-golden-path.mjs` still
describes the repository.

The corpus is **940 cases across 65 files**, up from 925 across 63. Two new files —
`telemetry/window` with 7 cases and `telemetry/coverage` with 5 — and one new case in an old one,
`checkin/document.json → a-robot-that-holds-no-desired-state`, which pins the empty digest. The four
existing cases in that file changed shape twice in one session, once for the attestation's measures
and once for `desired_digest`; both editions reproduce the new bytes, which is what having two is
for.

Two new vocabularies, `telemetry_counters` and `telemetry_window_faults`, taking
`vocabulary/closed-sets.json` from 55 sets to 57.

The Rust port of `telemetry.ts` reproduced all twelve decision cases on the first run, which is
worth one line: the two editions disagreed about nothing except the arithmetic nobody wrote down.
`Math.round` is half toward positive infinity and this workspace's `round_half_away` is half away
from zero, and they differ on exactly the negative halves a backwards clock produces — so the skew
in the message is **truncated toward zero** in both, which is the one reading each edition can spell
without a shared helper.

Two new CI invariants and one widened:

* **A check-in carries the three digests, not only the sentence**, grepped in both directions
  because the edit that loses it is a revert to a shorter function name — and the shorter one
  compiles, passes every assertion about the sentence, and renders correctly on every console.
* **A release manifest is not rewritten under its own version**, checked for the *refusal*, because
  the edit that loses it is somebody making the pull idempotent by writing unconditionally.
* **Reading the intent does not pull** now covers `/releases` as well, with the second cost spelled
  out: that request carries the device credential and goes out over whatever bearer is up.

`skynet releases` is the forty-second CLI command and `node tools/check-golden-path.mjs` counts it.

On the Hub: **570** tests (up from 485, after a pre-existing red suite was repaired),
`npm run typecheck` clean, `npx next build` succeeds with every new route registered, and all 35
locale catalogues audit clean.

### Changed in the Hub

Everything in `## Session — 2026-09-02 (ninth)` of `skynet-build-plan.md`. Three mirrored contracts
are new there and `node tools/check-contract-mirrors.mjs` reports thirteen matching.

§32.4's item 5 is also built — `/skynet/os/devices` and `/skynet/os/devices/[id]` — and the reason
it was last in the order turned out to be the interesting part. The ordering constraint was never
the tables; it was the same one the console faced in the ninth session: *"until then they would be
showing invented state."*

There is still no first real device, and both pages hold the line anyway. The one that took work is
the fleet table's `silent` tone: a robot that has never checked in has `null` in every reported
column, `applied: null` reads as *not applied*, and `generation - null` is 41 in JavaScript — which
renders as *"41 generations behind"*, a specific and actionable and entirely invented number about a
physical machine. It sorts first, its lag is null rather than a number, and both have tests, because
both failures are one careless `?? 0` away and neither would look wrong.

The device page is mostly a **panel of named gaps**, and that is the honest version rather than a
reduced one. §32.1 asks it for guards, resolved limits and the transport plan; the robot computes
all three every cycle and serves them on its own console, and nothing carries any of them to a Hub.
Each could be approximated here from what a check-in does carry, and each approximation would be a
confident statement about a machine nobody measured. They are listed, with the reason each is
missing, which is the fourth thing this session has found by asking what the other end can actually
see.

### Not done, and what it waits on

**Three migrations are written and not applied** — the fleet tables, the release channel, and
conformance with certification. Applying DDL to a live project is the owner's call rather than a
session's, and until they are run every route above answers with a PostgREST "no such table".
Nothing else blocks the loop.

**A round trip.** Every branch on both sides is driven by a suite and no robot has yet been answered
by this Hub. That is the next session's work and it is a deployment step rather than a coding one.

**The bundle bytes.** §12's `stage_image` still reports `Unimplemented`, and it is not the same
entry it was this morning: the manifest now arrives, so what was *"§12 cannot decide"* is
*"§12 decides, and cannot act"*. The transfer wants resume, progress and §34.3's metering, which is
`skynet_contracts::bundle`'s subject — written, corpus-verified, and with nothing calling it.

**The sealed black box.** §17 calls `POST /devices/{id}/incidents` an upload and both sides now treat
it as a document. §13's seal produces hundreds of megabytes and wants resume, progress and §34.3's
metering; `skynet_incidents.artifact_id` exists and is always NULL so the gap is visible in the
schema rather than only here.

**Telemetry** is closed. §74 gives it the tenth endpoint, both halves are built, and §15's field
conformance is computed from real windows rather than from an `undefined` with a paragraph attached.
What is left is that six of §74's eight counters have no source on this edition — which is a missing
kernel rather than a missing route.

**Where a bench pass comes from** has moved rather than closed. There is a route for one and a
table under it; what is missing is a *runner* — nothing on a robot executes §15's thirty-six tests
and nothing signs a result. §12's gate now reads real rows and cannot tell an attested one from an
asserted one.

**Field conformance** is wired and will read `null` for a while yet. The route exists, the windows
arrive and the arithmetic is over what was observed rather than over what elapsed — and `cycles` is
one of the six counters nothing on this edition measures, so the figure is honestly absent rather
than absent for want of plumbing. That is a better absence than it was this morning and it is still
an absence.

**Episodes.** `Class::Episode` is now the only queue class with a route and no producer, which is a
sharper statement of the same gap.

**A real A/B device**, **`security.identity.attested`** and **§54.2's `safety_notes`
acknowledgement**, all unchanged.

### Findings, restated

1–9 from the seventeenth session; 10 and 11 from the eighteenth; 12 and 13 from the nineteenth; 14
and 15 from the twentieth; 16 and 17 from the twenty-first; 18–20 from the twenty-second; 21–23 from
the twenty-third; 24 and 25 from the twenty-fourth.

26. **A rendering is not a record, and the difference is invisible until somebody has to query it.**
    New, and both of this session's findings are instances of it. `attest`'s sentence is correct,
    tested, and reads beautifully on a console — and it is the *only* place the three digests went,
    so §32.1's "which robots are running image `sha256:aa`" had no answer but a regular expression
    over English. The generation is the same mistake in the other direction: a number that is a
    perfect summary of an intent for the one party that also holds the document, and useless to the
    party that does not.
27. **A gap between two systems is invisible from either end.** New, and the reason both of these
    survived twenty-four sessions of a repository that finds something every time somebody opens the
    console. Nothing on the robot was wrong: it measured all three digests, kept them, served them
    on its own API, and rendered them correctly. Nothing on the Hub was wrong either, because the
    Hub did not exist. The defect lived in the space between a producer that had the fact and a
    consumer nobody had written yet — which is exactly the space no test on either side can reach,
    and it is why *"build the other half"* found two things that *"review this half"* had not.
28. **A gap that everybody can see stays open until somebody needs it, and what closes it is a
    consumer rather than more thought.** New, and it is the counterpart to 27 rather than a repeat.
    Telemetry's missing route was *not* invisible: three sessions recorded it, by name, as the one
    place §34.4 and §57 do not line up. It stayed open anyway, and the reason it stayed open is
    exactly the reason it was right to leave it — there was no second party, so any route would have
    been invented, and inventing a Hub surface is the failure the finding was guarding against.
    What changed today was not insight. It was that **two things upstream turned out to be waiting
    on the same absence** — §15's field conformance and §32.1's guard-trip column — and a gap with
    two consumers is a decision. The practical rule: an open finding with no consumer is correctly
    open, and the thing to do about it is to build whatever would consume it rather than to think
    harder about the finding.

---

## 2026-09-02 (twenty-sixth) — §12's bytes, and the gate that was never applied to the thing it names

Every session since the twenty-first has ended with the same entry under *not done*: **the bundle
bytes.** `stage_image` assessed a release, admitted it, and then reported `Unimplemented` with a
sentence that was exactly true — *"§34's inbound fetcher is not written; nothing here can go and get
it."* An image had to be put on a robot by hand.

This session wrote the fetcher, and it turned out that the interesting part was not the half that
was missing.

### The gap, and why neither end could see it

§12 is four pages about **applying** an image: A/B slots, the safe-state barrier, health-gated
rollback, and the rule it calls the single most important test in the suite. It says nothing about
**getting** one, because getting one looks like an implementation detail.

From the robot's side everything worked. The manifest arrived (§57's catalogue, twenty-fourth
session), the signature verified against the trust store, the compatibility list matched, the bench
pass gated the kernel change — and the action reported, correctly, that there was nothing to install.
From the Hub's side there was a catalogue of manifests and a `skynet_os_releases.artifact_id` column
that nothing had ever asked for. Neither end was wrong. There was no route.

That is finding 27 again, and it is the second time in two sessions that building the other half
found the thing reading either half could not. The specification gained **§75** for it, in Part X
beside §74, because a route that arrives after the fact should say when it arrived and why rather
than being retconned into §12.

### `crates/skynetd-io/src/bundle.rs` — the transfer

`skynet_contracts::bundle` has decided since the sixteenth session: seven fetch decisions, four
verdicts on what arrived, no network and no clock, corpus-verified against the TypeScript oracle —
and nothing called it. This is the caller.

Four rules, and none of them is about HTTP:

**A slice, not a transfer.** §33.2's loop is one thread: reconcile, serve the local API, ping the
watchdog, for ever. A pass that blocked for the twenty minutes a hundred-megabyte image takes over a
warehouse link would be a supervisor that had stopped supervising — which is precisely why
`report_drain` is bounded per pass, and the payload here is two orders of magnitude larger. So each
pass moves what it can inside twenty seconds and the next resumes. **Resume is what makes a bounded
slice free**, and it is why the partial is keyed to the digest: a partial keyed by version would let
a resume append one release's tail to another's head.

**The partial's length is the progress.** No counter, no percentage, nothing written anywhere.
§34.4's queue keeps no index for the same reason — a second record of what the directory already says
is a record that disagrees exactly once, after the crash that matters. `os/transfer.json` holds only
what the disk cannot say: how the last attempt ended, how many have failed in a row, when the next
may start.

**The bytes are checked before they are installed, however they arrived.** See below.

**Nothing here decides whether the image may be applied.** `assess` runs first, before a byte moves,
so a robot never pays for a hundred megabytes to arrive at a refusal it could have reached for
nothing. There is a test whose fetcher panics if it is ever called, and the release it is given
changes the safety kernel.

Two smaller decisions that only exist because the budget does:

* **A slice that runs out of budget must not back off.** The distinction the caller needs is *did the
  file grow*, not *did the process exit zero* — `curl` exits 28 on a timeout having moved forty
  megabytes, and doubling a delay for that makes a poor link exponentially poorer. Progress resets
  the failure count and keeps the reason.
* **A server that ignores the range is caught at the status code, not at the digest.** A `200` to a
  resume means the whole file was appended to what was already there. The digest would catch it
  hours later; the status catches it immediately and throws the partial away.

The credential rule from `connectivity` is carried across unchanged and for the same reason:
`/proc/<pid>/cmdline` is world-readable, so it goes into curl's own config on stdin and never into
an argument. `--location` is used and `--location-trusted` deliberately is not, so the device
credential is dropped at the cross-host redirect to storage rather than handed to a CDN. `--fail` is
there because without it an HTTP error page is a body, and a body is written into the partial —
which is how a 404 becomes forty bytes of HTML appended to an operating system.

### The hole: a gate bound to a digest, and nothing checking the digest

Writing the fetcher found something that had been open since §12 was built, and it was not in the
part that was missing.

**Every gate §12 applies is bound to the manifest's digest — and nothing compared that digest against
the bytes handed to RAUC.** `stage_image` took `assessment.bundle_path` and called
`updater.install(&bundle)`. The signature was checked. The bench pass was bound to the digest and
refused if it named other bytes. The compatibility list was matched. And the file itself was
installed on the strength of its *filename*.

The reachable path was not exotic: putting a bundle on the disk by hand was the **only** way to
update a robot before this session, and it is what the refusal sentence told operators to do. A file
at the right path with the wrong contents went in with every gate reporting a pass.

`bundle::verify_file` is now called immediately before `install`, whatever put the file there, and a
mismatch deletes it — because an unverified bundle left on a disk is an artifact some future code
path finds and treats as an image. The test writes somebody else's image to the exact path the
manifest names and asserts that nothing reached the bootloader and the file is gone.

This is the sharper half of the session and it is the one worth generalising: the gate was complete,
every test about the gate passed, and the gate was applied to a **name**.

### `Outcome::Waiting` — the reconciler had no word for it

Four outcomes: `Applied`, `AlreadyTrue`, `Unimplemented`, `Failed`. A robot on cellular that is
admitted to update and is waiting for an unmetered bearer is none of them.

Each of the three available answers is a lie in a different direction. `Failed` puts a red line on a
console every ten seconds for a robot doing exactly what §34.3 designed it to do. `Unimplemented`
says nobody built this, and somebody did. `AlreadyTrue` is worse than either. §34.1 *assumes the
robot is offline* — no link and a metered link are the ordinary condition of a fleet, not faults in a
machine — so the vocabulary needed a fifth word, and the absence of it would have forced the fetcher
to misreport its most common state.

It also earns a rule in the black box. `record_pass` skips a `Waiting`, quoting the precedent the
boot gate set three sessions ago: *"a `wait` is not recorded: it is the ordinary state for most of a
boot budget, and a record every pass would fill the ring with a robot deciding nothing."* A transfer
over a slow link produces one wait per pass for hours, and §26's ring is bounded.

### Where it shows

`GET /ota` gains a `transfer` block, and every field in it is computed from the disk and the bearer
table on the read — the decision, the sentence, whether the bundle is here, how many bytes are, the
total *and whose claim the total is*, the consecutive failures, the reason, and when the next attempt
may start. §34.3 asks for the wait to be visible rather than silent; this is where it is visible.

`skynet ota` renders it, and the console draws it under the assessment panel. Both refuse to draw a
percentage without a total, and both say whether the total came from the manifest or from the server:
a size in a manifest is a publisher's signed statement and a size from a server is the party serving
the bytes describing its own file. Those are different kinds of claim and a progress bar that hid the
difference would be inventing the more convincing one.

### The other half, in the Hub

`GET /api/skynet/v1/os/releases/{version}/bundle?arch=` — the eleventh endpoint, and the first that
serves bytes rather than documents. A 302 to a short-lived signed storage URL rather than a stream,
because an OS image is hundreds of megabytes and the platform's own artifact route settled this
argument already; object storage serves the `Range` requests the slice design rests on, which
`readHead` in `lib/skynet/storage/store.ts` has been relying on since the scanner was written.

Public and rate-limited, like the catalogue beside it, and the argument is stronger here rather than
weaker: the party fetching holds a device credential, which `requireActor` deliberately refuses
because a device token is not a user, and the bytes are checked against a digest inside a signed
document. Authentication would protect nothing the signature does not.

**A location is not evidence**, which is why the URL is not in the manifest. The obvious design —
a signed manifest carrying a download URL — reads as the more secure one and is the weaker: the
digest already makes the content unfalsifiable, so signing the location buys a manifest that goes
stale the day a hostname changes. It also keeps `releases::adopt`'s decision intact, which copies
five fields out of the Hub's row and deliberately not the artifact id.

`bundleSource` is a decision function rather than four lines in the route, and it exists to keep two
answers apart. *No such release* and *a release published as a manifest with no artifact* send an
operator to two different places, and the second is a state nothing else on the Hub can name: it
validates, it lists, and every robot on the channel holds it, gates it, admits it and then waits for
an image nobody uploaded.

### Numbers

Rust: **1160** tests, up from 1140. `skynetd-io` 352 → 372 — thirteen against `crate::bundle`
directly and seven through the reconciler, including the hand-placed bundle and the fetcher that
panics if the gate lets anything through it. `cargo fmt --all --check` clean, `cargo clippy
--workspace --all-targets` clean under `-D warnings`, `node tools/check-golden-path.mjs` and
`node tools/check-contract-mirrors.mjs` both clean.

`cargo fmt --all --check` was **not** clean at the start of the session, and not because of anything
here: three files in `skynet-contracts` and `skynet-corpus` had been committed unformatted, which
means CI's first step has been failing on `main`. Formatted, in the same run, because a red check
that everybody has learned to ignore is worse than no check.

On the Hub: **1457** tests, `npx tsc --noEmit` clean, three new tests for `bundleSource`, two new
i18n keys synced across all 35 catalogues, and the endpoint added to §18's documented surface — which
`skynet-api-docs.test.ts` holds to the routes that actually exist.

### Not done, and what it waits on

**Three migrations are written and not applied**, unchanged: the fleet tables, the release channel,
and conformance with certification. Applying DDL to a live project is the owner's call.

**A round trip.** Both halves of the fetch are driven by suites and no robot has yet fetched an image
from this Hub. That is now a two-sided deployment step rather than a coding one, and it is the same
entry the check-in loop has carried since the twenty-fifth session.

**The manifest declares no size.** The fetch contract has a branch for one — *a partial cannot be
larger than what it is part of* — and it is unexercised, because §57's five-field manifest carries a
digest and no size while the Hub holds `size_bytes` on the row. Closing it is a mirrored contract
change: `BundleManifest` in Rust, in `reference/typescript`, and in the Hub's copy, plus regenerated
corpus vectors. It was left open deliberately rather than closed with the server's own number, which
would put an untrusted value into a decision that deletes a file. `declared_size` is a single
function on the daemon so the day the field exists, both consumers become true at once.

**The sealed black box.** §17 treats `POST /devices/{id}/incidents` as a document and §13's seal
produces hundreds of megabytes. It wants exactly what this session built, pointed the other way:
resume, progress and §34.3's metering on an **upload**. `skynet_incidents.artifact_id` exists and is
always NULL, so the gap is visible in the schema. This is now the closest thing to a shovel-ready
piece of work in the repository — the transfer discipline is written, tested and has a shape.

**A bench pass runner.** Nothing on a robot executes §15's thirty-six tests and nothing signs a
result, so §12's gate reads rows it cannot tell attested from asserted. Unchanged, and it is the
oldest open item that nothing else is waiting on.

**Six of §74's eight telemetry counters have no source**, which is a missing kernel rather than a
missing route. **Field conformance** reads null for want of `cycles`, which is one of the six.
**Episodes** remain the only queue class with a route and no producer. **A real A/B device**,
**`security.identity.attested`** and **§54.2's `safety_notes` acknowledgement**, all unchanged.

### Findings, restated

1–9 from the seventeenth session; 10 and 11 from the eighteenth; 12 and 13 from the nineteenth; 14
and 15 from the twentieth; 16 and 17 from the twenty-first; 18–20 from the twenty-second; 21–23 from
the twenty-third; 24 and 25 from the twenty-fourth; 26–28 from the twenty-fifth.

29. **Evidence bound to an identifier proves nothing until somebody checks the identifier names the
    object in hand.** §12 binds the signature, the bench pass and the compatibility list to a digest,
    and for twenty-five sessions the bytes that went to the bootloader were selected by filename. The
    reason it survived is that *every test about the gate passed*, and they were all correct: the
    gate was complete and it was being applied to a name. The general form is worth carrying into the
    incident upload and the conformance runner, both of which are about to produce documents bound to
    digests: **check the binding at the point of use, not at the point of issue.**
30. **A vocabulary with no word for "nothing is wrong and nothing happened" forces every implementer
    to lie.** The reconciler had four outcomes and the honest answer for a robot waiting on an
    unmetered link was none of them, so the first version of this fetcher reported `Failed` — which
    is what the type system pushed it towards, and which would have put an alarm on a console for the
    ordinary state of a fleet. §34.1 assumes the robot is offline; a supervisor whose vocabulary
    cannot say *offline and fine* will say *broken* instead. Worth checking the other enumerations
    for the same missing word before they are consumed by something that has to render them.

---

## 2026-09-02 (twenty-seventh) — the episode neither side had ever put through the other

The twenty-sixth session's finding 27 says a gap between two systems is invisible from either end.
This one is the same finding with the parts swapped: not a producer with no consumer, but a producer
and a consumer that had **both** been built, both tested, and never once introduced.

### The two documents did not agree about a single field name

Five sessions of log entries record *"`Class::Episode` has a route and no producer"*. That entry was
wrong, and re-reading the code rather than the log is what found it: `Effect::Episode` has existed
since §54.4's lifecycle landed, `perform` enqueues it, `Hub::route` maps the class to `/episodes`,
and the whole path works. What nobody had done was compare the document it sends against the
document the route reads.

| The robot sent | The route reads |
|---|---|
| `skill` | `task` |
| `duration_ms` | `duration_seconds` |
| `intervention_count` | `interventions` |
| `invocation`, `digest`, `arguments`, `reason`, `detail`, `root`, `parent` | — |
| `outcome` | `outcome` ✓ |

Every episode this daemon produced would have been stored with a null task, a null duration, and
none of the skill — and the Hub would have answered `201`, because `POST /episodes` ignores fields it
does not know, which is the right behaviour for an ingest route taking documents from agents in the
field.

Neither suite could see it. The robot's asserted what the robot sends; the Hub's asserted what the
Hub accepts. Both were correct and both were about half a boundary.

The episode is now written in the route's field names, with the facts §54.4 asks for that the schema
has no column for going into `metadata` — the invocation id, the digest of the bytes that ran, the
arguments, the lifecycle reason. Dropping those would leave an investigation with a row saying a
skill failed and no way to find what was running.

**§76** records the general rule, because it is not about episodes: a document crossing §17 is the
receiving route's shape, and the receiver is the one with a table under it.

### The absence that a default destroyed

`Invocation::episode` omits the intervention count when nothing on the robot counts interventions,
with a comment saying exactly why: *"a zero would be consumed by a marketplace as evidence of a clean
run."* That is finding 30 from this morning, written down before this session found where it
actually bites.

`validateEpisodeInput` did `optionalNumber(params.interventions) ?? 0`, and the column is
`integer not null default 0`. So the careful absence became a zero on arrival — twice, once in
validation and once in the schema, either of which would have been enough.

Both are fixed. The validation preserves the absence, a migration makes the column nullable with the
same `CHECK` §74's telemetry windows already carry, and `summarizeEpisodes` now reports
`interventions_measured` beside the total — because a total over four hundred episodes of which
twelve counted is a smaller number presented as a better one. The repository view says *"not
measured"* rather than `0`, and *"3 across 12 of 400 episodes that counted"* when only some did.

The general form is §74.3's rule, which the telemetry table was built to and the older episode table
was not: **zero and unmeasured are opposite facts, and the one that gets misread is always the one
that looks like good news.**

### `recorded_at`, and the week that would have landed in one minute

A third defect in the same document, found by reading the route rather than the schema.
`normalizeRecordedAt` stamps `new Date()` when the field is absent, and the robot was not sending it.

§34.4's queue exists precisely so a robot offline for a week keeps its episodes. Every one of them
would have been filed at the minute the link came back — on the column whose entire purpose is to
say when something happened, in a dataset whose value depends on that.

The invocation now records the wall clock at its terminal transition and the episode carries
`recorded_at`. §35.1 permits exactly this and nothing more: the monotonic clock measures the
duration, so an NTP step cannot move it, and the wall clock places the record on a timeline the Hub
shares with other robots. A robot with no clock omits the field, because a Hub's guess is better than
a robot's invention.

### The mechanism, because a finding without one is a note

Finding 31 says a boundary needs a test that holds one side's real output against the other side's
real contract. That is a practice, and practices are not mechanisms — so the session did not stop at
writing it down.

**`reference/wire/`** now holds the exact canonical body this daemon puts on the wire for three §17
routes, produced by the real producers and frozen by this suite:

| File | Route | Producer |
|---|---|---|
| `episode.json` | `POST /episodes` | §54.4's lifecycle, at the terminal transition |
| `telemetry.json` | `POST /devices/{id}/telemetry` | §74's window, closed by a reconcile pass |
| `incident.json` | `POST /devices/{id}/incidents` | §38.1's profile-signature alarm |

Three properties, and each closes a way this could have been theatre:

* **Produced by the code, never typed.** A fixture written beside the test that reads it is a record
  of what its author believed. `SKYNET_WRITE_WIRE=1 cargo test -p skynetd-io` re-freezes; the
  environment variable is the whole of the write path, because a fixture that rewrites itself
  whenever it disagrees is a log of the last run.
* **Read by the far side's real contract.** The Hub's `skynet-wire.test.ts` pushes each document
  through the receiving route's own validation — `validateEpisodeInput`, `assessWindow`,
  `incidentContext` — rather than through a parser written for the test.
* **Pinned in both repositories.** `check-contract-mirrors.mjs` gained a second set and the pins are
  namespaced `wire/…`, so a fixture that moves here without the Hub being resynced fails in CI with
  one checkout, which is what CI has.

The result is the property neither side could have alone: a producer that changes fails here, with
the command that re-freezes it; a route that stops reading a field fails there; and the two land
together. **§31.6** records it in the specification beside the corpus, as the same mechanism pointed
at a boundary the corpus structurally cannot see.

The check-in is deliberately **not** in the set. It already has this treatment in the other
direction — the Hub's fleet suite carries the document verbatim from the corpus, under a header
explaining that a Hub tested against a document a Hub author invented is a Hub tested against its own
assumptions — and a second frozen check-in would be a second source of truth for one document, which
is the failure §31.2 names.

### Numbers

Rust: **1164** tests, `skynetd-io` 372 → 376, `cargo fmt --all --check` and `cargo clippy
--workspace --all-targets` clean. The first new test is the one that would have caught this: its
field names are copied from `lib/skynet/devices.ts` rather than from the producer, which is the only
way a test like it is worth anything. The other three are the frozen documents.

Hub: **1475** tests, `npx tsc --noEmit` clean, `npx next build` clean. Two tests for the absence —
one that it survives validation, one that the aggregate carries its denominator — and ten in the new
`skynet-wire.test.ts`, none of whose fixtures were typed by hand.

### Not done

**The migration is written and not applied**, and it now joins the three from earlier sessions.
`20260902320000_skynet_episode_interventions.sql` is the smallest of the four and the only one that
alters an existing column rather than creating tables; until it runs, an episode carrying no
intervention count is refused by the `not null` constraint rather than stored as an absence.

Everything else from the twenty-sixth session's list is unchanged.

### Findings, restated

1–28 as before; 29 and 30 from the twenty-sixth session.

31. **Two halves that were built separately have never been introduced, and their suites cannot
    introduce them.** The episode path was complete on both sides for five sessions: a producer that
    enqueued, a queue that sent, a route that accepted, and a table that stored. Every test passed
    because each side tested itself. What was missing was a test that takes *the producer's actual
    output* and holds it against *the consumer's actual contract* — and the only way to write one is
    to copy the field names from the far side's source rather than from your own. The fleet suite on
    the Hub already does this in the other direction: its check-in fixture is copied verbatim from
    the OS corpus, with a header explaining that a Hub tested against a document a Hub author
    invented is a Hub tested against its own assumptions. That practice now runs both ways and is a
    mechanism rather than a habit — see §31.6 and the section above.
32. **A stale log entry is worse than no log entry, because it stops people looking.** *"Episodes
    have a route and no producer"* was recorded five times, carried forward each session, and was
    false the whole time — the producer had been written before the sentence was first typed. It
    survived because each session copied the previous session's "not done" list rather than
    re-checking it, and because the claim was plausible: nothing on this edition can actually *run*
    a skill, so no episode has ever been produced in anger. The correct entry was *"nothing has ever
    exercised this path end to end"*, which is a different sentence that would have sent somebody to
    look at the two documents.

---

## 2026-09-02 (twenty-eighth) — one field, and the two rules that were waiting on it

Short, and it is the entry the twenty-sixth session wrote in advance: *"the manifest declares no
size, and the fetch contract has a branch for one."*

`BundleManifest` now has six fields. `size` is `Option<u64>` in the Rust contract, `size?: number`
in the TypeScript oracle, and the Hub's `manifestOf` copies it from the release row's `size_bytes`
when the row has one.

### What was waiting on it

**§12's partial check.** `plan_fetch` refuses to resume from a partial *larger than the whole
bundle* — which is how a leftover from another image is caught before a transfer rather than after
one. With nothing to compare against, the branch was unreachable on every robot and the digest
caught the mismatch at the end of a transfer instead, which on a warehouse link is hours.

**A progress line that is a fraction.** `GET /ota` reported a count of bytes, because the only total
available came from whichever server answered — a number the publisher's signature does not cover.
It now reports `total_declared_by: "manifest"` when the size is signed and `"server"` when it is
not, and the console and CLI say which. Those are different kinds of claim and a bar that hid the
difference would be presenting the more convincing one.

### Absent, not zero

Said in four places because it is the whole of the field's design: a `0` would be a publisher
declaring an **empty image**, and every robot on the channel would discard its partial as
larger-than-the-whole on the first pass. The contract's reader takes a size only when it is a whole,
non-negative, representable byte count; the Hub omits the key rather than sending `null`; and the
robot's fallback reassembly — the path that reads the Hub's *row* rather than its manifest — applies
the same rule to `size_bytes`.

### The cost, which is real and is cheap exactly now

The manifest's canonical bytes changed. A robot holding the five-field version of a release will
refuse the six-field one with *"the Hub is serving a different manifest for release `1.4.0`"* —
which is §12's immutability rule working as designed, not a regression. It costs nothing today
because no robot holds one, and it would cost a version number after a fleet exists. Recorded in
§75.5 rather than only here.

### Numbers

Rust: **1169** tests, up from 1164. Five new: three in `releases` for the field arriving by either
route and being absent when the row says nothing, and two in `ota` — one that the signed size
reaches `GET /ota` and is labelled as the publisher's, and one that drives the branch this field
exists for, asserting the fetcher is asked to start from **zero** rather than resuming into bytes
from another image.

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets` clean.
`node tools/corpus-gen/generate.ts` reproduces all 68 files and 973 cases byte for byte — the
`ota/bundle` vectors feed `admit_bundle`, which does not read the size, so an optional field added
to the manifest changes no expected output. That is the corpus doing its job in the quiet direction:
a change that should not move anything, and did not.

Hub: **1476** tests, `npx tsc --noEmit` clean. One test updated (the manifest is six fields now) and
one added, for the omission.

---

## 2026-09-02 (twenty-ninth) — the incident that never named its window

§13 is one sentence about this: *"sealed and uploaded on any fault, estop, or operator request."*
§17 lists the incident route as a **sealed black box upload**.

Neither half was true. The sealing has existed since the sixth session and nothing called it, so a
fault produced a document while the ring kept turning and the window describing the fault was
overwritten in the ordinary course of a robot running. On the Hub, `skynet_incidents.artifact_id`
was NULL on every row the platform could hold, and an investigator reading an incident had no way to
know a recording had ever existed.

### The seal belongs to the raise

`raise_incident` now seals and attaches, so every incident this daemon ever raises carries a window
and no future producer has to remember to. §38.1's profile alarm is the only producer today and it
went from being the caller that formats a document to being one line.

**`blackbox::seal_existing` reads and never creates**, which is the whole reason it is a new function
rather than a second `Recorder`. `Recorder::open` deliberately *replaces* a ring whose shape no
longer matches the body — correct for the one process that owns the recording, catastrophic for
anybody else, because a second opener on a robot whose descriptor had changed would wipe the window
somebody was about to seal. So the sealer holds no handle: it reads `<blackbox>/ring`, walks it,
writes the file, and returns the identity.

### Naming a thing you cannot send

A sealed window is tens to hundreds of megabytes; §34.3 puts it in the bulk class, and the upload —
resume, progress, metering, the mirror of §75's fetch — is not written. Between *send it* and *say
nothing* there is a third option that is honest and costs a few hundred bytes: **send the window's
identity.** Name, size, digest, records, damaged, `uploaded: false`.

A Hub can render that: *"this incident has a 4 MB recording on the robot, digest `sha256:…`, that
nothing has collected."* Saying nothing is the worse error of the two, because it closes an
investigation rather than opening one — an operator reading a blank cell concludes no recording was
taken.

The digest is what makes it worth carrying now rather than later. When the bytes do cross, they will
be checked against a digest recorded before the transfer existed, by the party that produced them —
which is §12's rule about evidence, applied to the other direction.

### One shape, discriminated

`recording` is always present and always an object: `sealed: true` with the window, or
`sealed: false` with the reason — no ring, or a ring that would not open. The absence of the field
altogether is a third state and means an older edition said nothing, which is **not** evidence that
a robot has no black box.

The first version of this used an object for a seal and a sentence for a failure. That is a union a
consumer has to type-check before it can read a field, and the case it would get wrong is the common
one on this edition: a development root has no ring at all. Changed before it left the repository.

### A seal that fails does not swallow the incident

The failure goes **into** the document and the incident is queued anyway. §34.4 makes this the one
class that is never dropped, and a robot that suppressed an alarm because its own recorder was
broken would be losing the report and the recording together — which are exactly the two things that
fail at the same moment.

### The mechanism caught its first change, one session after being built

The wire fixture did what §31.6 says it is for, on the first document to change after it existed:

```
reference/wire/incident.json no longer matches what this daemon produces.
This document crosses §17 and the Hub keeps its own copy of it. If the change is intended,
re-freeze and resync — and expect the Hub's suite to have something to say, which is the point.
```

Re-frozen, resynced, and the Hub's suite did have something to say — not a failure, because the
incident route stores what it does not recognise verbatim, but the fixture is what turned *"we
changed a document"* into *"here is the document the other side is now holding"*. The fixture also
got better under it: the incident's robot now has a ring, so the frozen document carries a **sealed**
recording rather than the honest-but-uninteresting sentence a fixture with no recorder produces.

### On the Hub

`incidentRecording` reads the three states, and the device page renders the size with the digest on
hover. It lives in `components/skynet/fleet-types.ts` rather than beside the rest of the fleet code,
because that page is a client component and `lib/skynet/fleet.ts` reaches for `node:crypto` and the
service-role database client — the same reason `formatBytes` is spelled twice, which is the cheaper
side of that trade by four lines.

### Numbers

Rust: **1169** tests, unchanged in count and not in content — the incident tests now assert the
window on both paths, and the wire fixture carries a real seal. `cargo fmt --all --check` and
`cargo clippy --workspace --all-targets` clean.

Hub: **1478** tests, up from 1476: the recording read out of the frozen document, and the three
states told apart. `npx tsc --noEmit` clean, three new i18n keys synced across 35 catalogues.

### Not done

**The bytes.** Unchanged, and now with a consumer: `skynet_incidents.artifact_id` is still NULL and
the page says so per incident rather than leaving it blank. §34.3's upload wants resume, progress
and metering, and the honest blocker is on the Hub's side rather than the robot's — object storage
takes a single-shot signed PUT, and a two-hundred-megabyte PUT over a warehouse link is exactly what
§75.2 spends its length on not doing. Resumable upload (TUS), chunk-and-assemble through a job, or
a signed session are three designs with different costs, and that is an owner's decision rather than
a session's.

---

## 2026-09-02 (thirtieth) — the safety note that nobody had ever been asked to read

Open since §54 was written, carried in every *not done* list since the seventeenth session, and one
of the last two items on it that nothing else was waiting on.

> `safety_notes` stops being decorative too: it is displayed at grant time and acknowledged once per
> skill per robot, recorded in the audit log. **A safety note nobody has read is a safety note that
> does not exist.**

The field has been in the Hub's skill manifest since skills were built — a string, four thousand
characters — and **nothing on either side read it**. A publisher could write *"clear the cell before
granting"* and every robot in the fleet would grant the capability without showing it to anybody.

### The grant is the right moment, and it is the only one

§54.2 names it and building it confirmed the choice. A grant is the one point in a skill's life
where a person is present, deciding, and able to stop; an invocation is a robot doing what it was
told, often by a reconcile loop at three in the morning, and a prompt there is a prompt nobody
reads.

`POST /grants` now refuses with `409` when a skill declares notes this robot has no acknowledgement
for, and **the refusal carries the notes**. *Displayed* is the word the rule uses, and the only
place a robot can be sure a person sees them is in the answer to the request they just made. The
CLI prints them, then prints the command that acknowledges them with the digest already filled in —
because a rule that is annoying to comply with is a rule somebody scripts their way around.

### Bound to the text

The acknowledgement records the **digest of the notes**, so a publisher who edits a warning has
written a new warning. §54.2 says "once per skill per robot" and the reading that makes the sentence
worth anything is the one where a changed note is a note nobody has read yet — the alternative lets
a package acquire its warning *after* the operator agreed to a blank one. There is a test that
installs a second version with different notes and asserts the next grant asks again.

Acknowledging *other* notes is not acknowledging these, either: a digest that does not match is
refused the same way, because that is what a replayed command from last version looks like.

### Two files, two questions

`<state>/acknowledgements.json` answers *may this grant proceed* — decidable offline and instantly,
which is the only kind of answer §11 permits in a path a person is waiting on. The **audit log**
answers *who read it and when*, carrying the digest and the note's first line, which is what
somebody asks after an incident and is the one of the two that cannot be reconstructed.

An acknowledgement that could not be written **refuses the grant**. Granted-and-unrecorded is the
worst of the three outcomes: the next grant asks again, and no incident review can find that anybody
ever read it.

### Where the notes are read from, and the tense it is in

Two places, in order: the **active** digest, then the digest the desired state names. They are the
same question in a different tense — what the package is now, and what it is about to be — and a
grant written during a rollout is a grant about the version arriving.

A package this robot does not hold at all yields no notes and the grant proceeds. Grants and the
desired state are both level-triggered and either order is valid, so refusing a grant for want of a
manifest nobody has fetched would break the ordinary order of a deployment. The note is read at the
next grant.

### It gates a widening and nothing else

§39.1: the safe direction never waits. Revocation is untouched, and there is a test for it. The
on-robot console needed no change at all, which is worth recording as a small confirmation of the
design — it can revoke a grant and has never been able to add one, so the surface that cannot show
a person a paragraph is also the surface that cannot need to.

### Numbers

Rust: **1178** tests, up from 1169. Nine new, and the two that matter most are the ones about
*absence*: whitespace-only notes are not a warning (requiring an acknowledgement of nothing is how
an operator learns to acknowledge without reading), and a package with no notes is granted without
ceremony.

The suite runs on Windows, which took one decision: the fixture stages the package and names it in
the desired state rather than activating it, because activation is §37.1's atomic symlink swap and
that is POSIX-only. `mod posix` still covers the activation path; this covers the rule, which is
about the manifest this robot holds and is the same document either way.

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets` clean.

### Findings, restated

1–28 as before; 29 and 30 from the twenty-sixth session; 31 and 32 from the twenty-seventh.

33. **A field that only one side reads is decorative, and it stays decorative silently.**
    `safety_notes` was validated, stored, rendered in a manifest schema and shipped for however long
    skills have existed — and no robot had ever displayed one. Nothing was broken: the Hub's job was
    to carry it, and it carried it. The failure is that a *consumer* was specified in one sentence
    of §54.2 and never written, which is finding 27's shape with the halves swapped and is now the
    third instance in five sessions. The generalisation worth acting on: **a schema field whose
    consumer is one sentence in a specification is a field to go and check**, and this repository
    has a list of them — every "measured by" column in §52 that points at something, every manifest
    key the Hub validates. The next session could usefully spend an hour reading the Hub's manifest
    schema against what the robot reads.

---

## 2026-09-02 (thirty-first) — the sweep finding 33 asked for, and the one field it caught

Finding 33 ended with an instruction rather than an observation: *"a schema field whose consumer is
one sentence in a specification is a field to go and check, and this repository has a list of them."*
This is that hour, spent immediately rather than recorded as future work.

### The method

Every field in the Hub's five card schemas (`lib/skynet/manifests.ts`), read against what the robot
consumes. Not against what the specification says the robot consumes — against `grep`.

### What it found

| Card field | On the robot |
|---|---|
| `source_embodiment` (model) | Read — §6's transport planning |
| `required_capabilities` | Read — admission, and §25.2's leases |
| `runtime` | Read — the entry point |
| `preconditions`, `success_criteria` (skill) | Read — §54.2, §54.3 |
| `expected_duration_seconds` | Read — §54.4's deadline |
| `composition` | Read — §54.5, at invocation, through `validate_composition` |
| `safety_notes` | Read **since yesterday** (§54.2) |
| **`task`** (skill) | **Read by nothing** |
| `required_extensions` | See below |
| `architecture`, `parameters`, `precision`, `modalities`, `action_space`, `observation_space`, `training`, `intended_use`, `limitations`, `hardware_requirements` | Correctly Hub-only |
| every dataset, environment and benchmark field | Correctly Hub-only — none of them is installed on a robot |

Two results, and the shape of the second is the reason the sweep was worth doing.

### `task`, which every episode had been getting wrong

`SkillManifest.task` — *"pick the cup off the table"* — has been validated, stored and rendered on
every skill card since skills existed. No robot had ever read one.

§54.4's episode carried the **package id** in `task` instead, and §17's column means *what the robot
was doing*. So every episode a fleet produced would have arrived with `acme/pick` in the one column
a training set is filtered by — forty thousand rows saying which *thing* ran and none saying what it
was doing. Not a null, not an error, and nothing on either side would have flagged it.

Fixed: the declared task when there is one, trimmed, with the package id as the fallback — a poor
label and a correct one, against a null column on every row. The id stays in `metadata.skill`,
because grouping needs an identifier and a sentence is not one.

**The wire fixture caught it twice in one change**, which is the second time in three sessions:
re-freezing the episode failed the OS suite, and the resync then failed *the Hub's* assertion that
`task` is `acme/pick`. That assertion was correct when it was written and wrong the moment the robot
started reading the card. Neither suite could have found this alone; the pair found it in two
commands.

### `required_extensions`, which is not a gap and is a divergence risk

Both sides read extensions and they read them from **different documents**. The Hub's compatibility
engine consumes the *card's* `required_extensions`; the robot's admission consumes the *package
manifest's* `embodiment.extensions`, checked against the body's declared extensions.

Neither is wrong and nothing checks they agree. A publisher who fills in one and leaves the other
empty gets a pass from whichever side reads the empty one — the Hub says compatible because the card
requires nothing, or the robot admits because the manifest requires nothing. Both documents come
from the same publisher and nothing on the platform compares them.

Left open deliberately: the check belongs on the Hub at publish time, where both documents are in
hand — the card is a column and the package manifest is a file in the version — and it is a piece of
work rather than a line.

### Numbers

Rust: **1179** tests, up from 1178. One new, covering the declared task, the fallback, and the
whitespace-only case that is not a declaration.

Hub: **1478**, with the wire expectation corrected to the document the robot now sends.

### Findings, restated

1–28 as before; 29 and 30 from the twenty-sixth; 31 and 32 from the twenty-seventh; 33 from the
thirtieth.

34. **A sweep prompted by a finding is worth more than the finding.** Finding 33 was written to be
    acted on later and was acted on within the hour, which is why `task` is fixed rather than being
    a line in a list somebody reads next month. The method transfers: take the *other* side's schema,
    grep this side for every key in it, and read the ones that return nothing. It took an hour and
    found a defect that would have degraded every episode the platform ever collected — silently,
    because a wrong string in a text column is not an error anywhere.

---

## 2026-09-02 (thirty-second) — the table that said "Designed" about eighty thousand lines

§73 is the specification's own summary of what exists. A vendor reads it first, and it was wrong in
the direction that matters: it listed the Safety Kernel, `skynetd`, OTA, the black box, identity and
the Hub contract as **Designed**, and closed with

> Implemented means running, tested TypeScript in `lib/skynet/os/`: 2,430 lines across transport,
> admission, certification and readiness, with 80 unit tests inside a suite of 1,117.

That paragraph was true when the Rust workspace did not exist. It has been false for thirty
sessions, and nobody updated it for the ordinary reason: **nobody updates a summary while they are
building the thing it summarises.** The result understated the platform by roughly eighty thousand
lines and, worse, understated it *unevenly* — a reader could not tell which rows were prose and
which had a suite behind them.

### Three states, and a fourth column

Rewritten with the states the work actually has:

* **Designed** — prose. Two rows are still this, and one of them (§36, power and thermal) is now
  named as unwritten rather than left to be inferred from a word.
* **Built** — code with a suite.
* **Built on both sides** — the robot half and the Hub half exist, and a document has been put
  through one into the other (§31.6). Seven rows.

And a fourth column: **what is still missing, per row.** That is the half the table was missing more
than it was missing accuracy. A state table of green ticks is exactly the artifact this document is
written against — the useful sentence is not *"the kernel is built"* but *"the kernel is built and
nothing has driven a physical joint."*

The rows that carry the sharpest entries are the ones this month's sessions produced:

* OTA — built on both sides, and **no robot has yet applied an image from this Hub**.
* Black box — built, and **the sealed bytes do not cross §17** (§77).
* Conformance — built on both sides, and **nothing on a robot runs the suite or signs a result**.
* Skills — built, and **nothing on this edition can run a policy, so no skill has ever executed**.
* §32's interface — built on both sides, and **three of §32.1's four panels are named gaps**.

### The numbers, measured rather than remembered

85,700 lines of Rust across fifteen crates with 1,179 tests; 12,600 lines of the TypeScript
reference edition the corpus holds it to; 5,400 lines of drivers; 8,100 lines in the Hub's mirror;
1,478 tests on the Hub. Counted with `wc`, not recalled — the previous paragraph's numbers were
recalled, which is how they survived being wrong.

The replacement says so: *"numbers go stale; the shape does not"*, and points a reader at the last
column rather than at the totals.

### Findings, restated

1–28 as before; 29–34 from the sessions above.

35. **A state table is a claim, and it decays faster than the code it describes.** Nothing in this
    repository could have caught it: no test asserts a paragraph, the mirror checks only that both
    copies agree — and they did, both being equally out of date. The mechanism that would catch it
    does not exist and is probably not worth building; what is worth doing is what this session did,
    which is to **read the summary last** after a stretch of building, on the assumption that it is
    wrong. The tell is a table whose rows all say the same word.

---

## 2026-09-02 (thirty-third) — the graph that was only ever discovered at runtime

§54.5 is one sentence with two halves, and this repository had built the first:

> Depth-limited (default 4) and **acyclic** — a cycle is refused at install by walking the declared
> graph, **not discovered at runtime**.

`Runtime::execute` validates the **live ancestry** on every nested call: it takes the chain of
invocations that actually happened, appends the edge about to be created, and refuses the one that
would close a cycle. That is correct, it has tests, and it is precisely the discovery the second
half forbids being the only one — it fires *on a robot*, with leases held and a deadline running,
which is the worst moment to learn that a skill calls itself.

The declared graph had never been read. `read_calls` was written, corpus-verified, and had no caller
outside the corpus runner — the same shape `skynet_contracts::bundle` had before the twenty-sixth
session, and the third instance this month of a contract function waiting for a consumer.

### Walked in two places, for two different reasons

* **At install**, reported. `skynetd install` and `skynet install` both print the fault with the
  path — `acme/pick → acme/grasp → acme/pick` — because §64 makes a refusal name the fix, and the
  person who can fix a call graph is the one running the command.
* **At activation**, enforced. Beside the driver-signature gate and for the same structural reason
  §19 gives: a staged blob is inert and the symlink swap is the consequential moment. Reporting
  alone would leave the rule to whoever reads terminal output.

One function decides both, and it is the contract's `validate_composition` — the same one the
runtime uses. Two implementations of *acyclic and depth-limited* would be two answers to one
question, and only one of them has a corpus behind it.

### Numbers

Rust: **1184** tests, up from 1179. Five new: a graph that terminates, a declared cycle, a graph
past the depth limit, a package that declares no graph (which is nearly all of them, and a check
with an opinion about those would be a check that refuses things for saying nothing), and the
either-spelling reader.

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets` clean; the golden path still
counts 44 commands.

### The spec says both halves now

§54.5 gained the sentence that says where each check lives and why there are two, because the
version that only said "refused at install" is the version that let this sit for six sessions
looking done.

---

## 2026-09-02 (thirty-fourth) — the suite the last commit left red

`cargo test --workspace` failed on this branch before anything new was written. Two tests, both
from the previous commit, and neither failure was a regression in the thing the commit was about —
which is why they shipped: the work was the bundle route, the bundle route passed, and the two
casualties were somewhere else.

Starting a session on a red suite is starting without a baseline. Nothing built on top of it can be
said to have broken anything, because everything is already broken.

### The OTA test that had outlived its own subject

`an_os_update_nobody_can_apply_is_reported_rather_than_dropped` asserted `Outcome::Unimplemented`
with a reason containing "§12". It got a `Failed` naming release `2.0.0`.

Both are the same *behaviour* — the action stays in the plan, the generation does not advance, the
robot does not report itself up to date — and the reason changed underneath it because §75 wrote
OTA. The refusal is no longer *"there is no bundle fetcher"*; it is *"nothing on this robot
describes release `2.0.0`"*, which is a condition of this robot that the next check-in can fix
rather than a feature nobody has written. `Failed` is the right variant for that and
`Unimplemented` was the right variant for the other.

The dead arm came with it. `Runtime::apply` still carried

```rust
#[allow(unreachable_patterns)]
ActionKind::StageOsImage { .. } | ActionKind::ApplyOsImage { .. } => Outcome::Unimplemented { … }
```

behind the two arms that now handle both — an `#[allow]` whose only job was to stop the compiler
saying the code was already dead. Removed, along with the `converge` doc comment that still gave
*"an OS update while §12 is unwritten"* as its example of an action nobody can apply. §12 is
written. The example is now the release manifest nobody has sent.

### The cell test that was written against a document neither edition reads

`the_workspace_layer_is_computed_and_says_it_is_not_applied` built its cell with

```json
{"frame":"base","shape":{"box":{"min":[0,0,0],"max":[2,2,2]}}}
```

and asserted one volume in the layer. The reader — and the corpus, and the reference edition —
spell a shape `{"kind":"box","min":…,"max":…}`. So the volume was unreadable, the layer was `null`,
and the assertion counted zero.

Fixed by writing the document the way both editions read it. But the failure is worth more than the
fix, because the *route's* answer to a malformed volume had no test at all, and it is an answer with
two halves that only mean something together:

* **No layer**, not an empty one. An empty layer forbids every frame it names, so a cell whose
  geometry nobody could parse would refuse to let the robot move, with no volume anywhere to explain
  why. `cell_workspace_layer` returns `None` and the route renders `null`.
* **A `volume_shape_invalid` fault naming it.** Without that, `null` is indistinguishable from a
  cell that draws nothing on purpose — and those are opposite situations. `CellVolume::shape` is an
  `Option` for precisely this reason, and the comment on it says so; nothing had checked the route
  honoured it.

`a_volume_whose_shape_cannot_be_read_draws_no_layer_and_is_named` now pins both halves, using the
same broken document that produced the red test.

### And one warning

`cargo clippy` wanted `skew > 60_000.0 || skew < -60_000.0` folded into
`!(-60_000.0..=60_000.0).contains(&skew)`. It is the same expression for every number and a
different one for `NaN`: the range says *not contained* and raises the fault, the two comparisons
say *neither greater nor less* and do not. The reference edition spells it with two comparisons.
An edition difference reachable only through a value JSON cannot carry is still an edition
difference, and the corpus exists to have none — so the comparisons stay, with an `#[allow]` whose
comment says which of the two readings is load-bearing.

`cargo fmt` had one diff outstanding from the same commit.

### Numbers

Rust: **1221** tests, from 1219 green plus two that were not running and one new. `cargo fmt --all
--check` and `cargo clippy --workspace --all-targets` clean, the mirrors match, the corpus passes,
and the golden path counts 45 commands.

### Findings, restated

1–28 as before; 29–35 from the sessions above.

36. **A commit can be green in the thing it was about and red beside it.** Both failures were in
    subsystems the bundle work never touched — one had been true for however long OTA has been
    written, one was a test that never passed. The habit that catches it is not a smarter test, it
    is running the whole suite before writing anything, and treating a red suite as the session's
    first task rather than as noise to work around. A baseline you did not measure is not a
    baseline.
37. **A test written against a document the reader rejects tests nothing, and looks like it tests
    something.** It asserted a count and got zero, which is the same shape a genuine regression has.
    The tell was that the fault list — which the route was filling in correctly the whole time —
    already said `volume_shape_invalid`. When a test fails on a count, read what the code *did* say
    before assuming it said nothing.

---

## 2026-09-02 (thirty-fifth) — §36, the one row that said "Unwritten"

§73's last column had one entry that was not a gap in something built. Every other row named a
thing waiting on hardware, or a signature nobody can produce yet, or a panel nobody has drawn.
§36's said:

> **Unwritten.** `battery` and `temperature_c` are reported absent in every check-in, which is why
> the health rollup has two permanently unmeasured fields.

Eleven parts of a specification and four subsections of prose, and no code had ever read a word of
it. `power.safe_state.reachable` has been in the conformance suite since Part II, measuring a
declared figure against *"the `time_to_safe_state_s` it declares (§36.2)"* — and no descriptor on
either edition had a `time_to_safe_state_s` to declare. A test with nothing to test, sitting in the
required set for the host tier, for six parts of a document.

Built as `crates/skynet-contracts/src/power.rs` and `reference/typescript/power.ts`, held together
by 28 corpus cases and seven frozen vocabularies.

### Five things §36 does not say, and code cannot avoid deciding

This is the part worth recording, because the next person to read §36.1's table will find the
numbers there and needs to know where they came from.

**1. The band widths.** §36.1 says `low` is "approaching reserve" and `reserve` is "at the reserve
budget" — a shape, not a number. They are now 2.25 × and 1.25 × the reserve, and both are multiples
of the **reserve** rather than percentages of the pack, because the reserve is the only energy scale
the body declares and a percentage means something different on every robot.

The two widths have different arguments. The reserve band is a quarter of a reserve wide so a gauge
that jitters near the boundary does not chatter between *begin a controlled stop* and *stop
immediately*, on a robot already mid-ramp. The low band is **one whole reserve** wide because
`low`'s only behaviour is *a running task may finish and no new one may start* — a warning arriving
with less than one further safe stop in hand is a warning the robot cannot act on, and the row's
promise would be one it cannot keep.

**2. A fifth state.** §36.1's four all assume a battery. An arm bolted to a bench and run off mains
has no state of charge, and forcing it into one of the four either immobilises it or claims a
headroom nobody measured. `unmetered` grants leases and is listed last rather than under `critical`,
because it is not a rung of the same ladder.

**3. What a silent gauge is.** Two bodies reach `unmetered` and they are not the same robot: one
declares no pack, one declares a pack whose gauge stopped answering. The difference is a **fault**
rather than a state, because it is a fact about whether the number can be believed rather than about
how much energy is left. And it is deliberately not `critical` — §36.4 overrides the gauge on a
*positive* reading from the bus, missing evidence is never a positive finding anywhere else in this
system, and a robot that stopped dead on a failed I²C read would be a robot that stops dead on a
failed I²C read. It withholds new leases and leaves the running task alone, which is `low`'s
behaviour reached from the other direction.

**4. How the OS recognises a brownout, which as written it could not.** §36.4 says voltage sag
arrives as `RHAL_DEGRADED` with a vendor fault code. §24 says a fault code is *"vendor-specific,
surfaced verbatim to the black box"* — the OS is told never to interpret one. Both sentences cannot
be honoured by an OS that guesses, and `RHAL_DEGRADED` also covers a dead encoder and a thermal
fault, so guessing means either stopping a working machine or failing to stop a falling one.

The resolution is `power.brownout_fault_codes`: the vendor who defined the codes names the ones that
mean sag, and the OS **matches** a number against a list rather than interpreting anything. §24's
rule survives intact. An empty list is not *this body cannot brown out* — it is *nobody has said*,
reported as `brownout_undeclarable` at the one moment it matters, with the robot still running,
because a hole in a vendor's declaration is a reason to say so and not a reason to stop a machine.

**5. How deep §36.3's ladder is cut.** The section gives an order and no schedule. Throttling gives
up the first three rungs — telemetry, OTA, black box compression — and critical gives up all five.
The line between three and four is where housekeeping ends: the first three cost the robot nothing
it was hired to do, and rungs four and five change what it can *do*. `NEVER_SHED` is data rather
than a comment, so "never the RHAL, the kernel or the control loop" is something a test checks.

"Sustained", for the DEGRADED that §36.3 raises, is thirty seconds — one cycle makes every heavy
motion a degraded robot, ten minutes makes DEGRADED a state a thermally limited machine never
reaches while spending its afternoon at the throttle point.

### The reserve is a floor, and it says so

§36.2 computes the reserve from three things: the declared figures, the current payload, and the
distance to the nearest safe state. **One of the three exists on this platform.** There is no
payload mass anywhere on the descriptor and no distance-to-safe-state anywhere in an observation.

So the arithmetic is the identity — the reserve *is* `safe_stop_energy_wh` — and `unaccounted` names
the two missing terms on every robot. A reserve with a non-empty `unaccounted` is reported as a
**floor**: every missing term can only add energy, so the figure under-estimates, and
under-estimating is the direction that runs a robot past the point where it can still sit down.

Scaling by a plausible payload factor would have been one line and a number nobody could defend.
This is the number that decides when a two-metre humanoid stops walking.

### The corpus caught a constant nothing tested

Halfway through, a tamper test: move `RESERVE_BAND` from 1.25 to 1.9 and confirm the corpus fails.
**It passed.** Then 2.5 — passed. The runner was fine; every fixture sat comfortably inside its
band, so a constant deciding where `low` becomes `reserve` was not discriminated by a single one of
the twenty-four vectors. Two editions could have disagreed about it and both reproduced the corpus.

Four boundary cases in watt-hours fixed it — 24.999, 25, 44.999, 45 against a 20 Wh reserve — and
both constants now fail the corpus when moved. A matching unit test carries the same four readings
and the reason they exist.

The general form is worth keeping: **a corpus proves an edition reproduces the cases it has, and
says nothing about the constants no case sits near.** Tampering with each constant and requiring a
failure is the only check that the vectors discriminate what they appear to. It cost four cases.

### And a mirror run the wrong way

`node tools/check-mirrors.mjs --sync ../RobotOverflow` copies **from** the Hub **into** this
repository — the script says so plainly — and running it after amending §36 here silently restored
the unamended copy. Nothing was lost that was not reproducible, and the tell was `git diff` showing
no change to a file that had just been edited six times.

The rule the script enforces is right and the direction is not guessable from the flag name. The
sequence that works is: edit here, copy the file across, then `--sync` to re-pin the digests from
two copies that already agree.

### Numbers

Rust: **1252** tests, up from 1221. Thirty-one new, all in `power`. The corpus is **1008 cases
across 73 files**, up from 997 across 73 — 28 in `corpus/power/` and seven vocabularies in
`closed-sets.json`.

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets` clean; both mirror checks
match; the golden path counts 45 commands.

### What §36 still does not have

Named in §73's last column rather than left to be inferred: **nothing on a robot reads a gauge or a
thermal zone.** The contract decides what a reading means and nothing supplies one, so `battery` and
`temperature_c` are still absent in every check-in. That is the next session's work and it is
`skynetd-io`'s, not the contract's.

Two smaller ones, recorded here because they are the kind that survive by not being written down:

* **Readiness has no power check.** `ReadinessSubject` is a registry row with no power fields, and a
  useful check — *this body declares a pack and no safe-stop energy* — needs to tell that from *this
  body has no pack*, which the row cannot express today. Adding it means plumbing the profile
  through the subject and the Hub's mirror.
* **`power` is not in the Hub's mirror list**, and the reason is now written into
  `check-contract-mirrors.mjs` beside the other three: nothing on the Hub reads a `power` block, so
  a mirror would be a copy with no reader. The trigger for changing that is a robot page wanting to
  say the reserve has no boundary — which is a readiness question, which is the bullet above.

### Findings, restated

1–28 as before; 29–37 from the sessions above.

38. **A conformance test can outlive its subject by years without anybody noticing.**
    `power.safe_state.reachable` measured a descriptor field that did not exist, in the required set
    for the host tier, since Part II. Nothing could catch it: the test list is data, the field list
    is a struct, and no check relates them. The mechanism that would — assert every quantitative
    test names a field some edition reads — is worth building, and is a session of its own.
39. **A corpus does not test the constants its cases do not sit near.** Tamper with each and require
    a failure; the ones that pass are the ones two editions can disagree about while both being
    green. Cheap to check, and it found one on the first try.
40. **Two rules in one document can be individually right and jointly unimplementable.** §24 forbids
    interpreting a vendor fault code; §36.4 requires acting on one meaning of one. Neither is wrong
    and an implementation must do something. The way out was to move the interpretation to the party
    that owns the vocabulary — the vendor declares which codes mean sag — which is a third option
    that only appears once somebody tries to write the code.

---

## 2026-09-02 (thirty-sixth) — the gauge, and the two columns that had never held a number

The last session built §36's contract and left one line in §73:

> Nothing on a robot reads a gauge or a thermal zone, so `battery` and `temperature_c` are still
> absent in every check-in.

This is that half. `crates/skynetd-io/src/power.rs` reads `/sys/class/power_supply` and
`/sys/class/thermal`, and the two fields §33.6 has named since the health rollup existed now carry
numbers on a machine that has them.

They had never held one. Not "sometimes null" — the rollup contained `battery: Measured::none()` as
a **literal**, so a check-in from a robot with a full battery said exactly what a check-in from a
robot with no battery said. There was no code path that could have distinguished them.

### Which supply is the pack is a question; which zone is hottest is not

Every `/sys/class/power_supply` entry declares its own `type`, so the batteries are *found* rather
than guessed at. What is left is the real ambiguity: a machine with two batteries has not said which
one drives the legs. Summing them would be a guess about whether the second is a backup cell, a tool
battery or half of one pack — and §36.2's reserve is compared against the answer.

So: no battery reads nothing; one battery is the pack; more than one reads nothing and names them,
until `state/power.json` says which. That is exactly §34.3's shape for bearers, and it is here for
§34.3's reason — the parts a machine can measure are measured, the parts it cannot are declared, and
every surface says which is which.

A declaration naming a supply this machine does not have reads **nothing**, rather than falling back
to whatever is present. The operator named a pack; substituting another is the OS deciding which
battery the reserve is about.

The thermal zones need no declaration at all, and the asymmetry is worth stating rather than
noticing: the maximum over every zone the kernel exposes *is* "the hottest thermal zone", whatever
each zone happens to measure. There is no question there to get wrong.

### The conversion that was not done

A gauge exposing `charge_now` and `voltage_now` but no `energy_now` can be turned into watt-hours by
multiplying them, and every desktop battery applet does. It is not done here.

The product is an energy computed from an *instantaneous* voltage, and voltage sags under load — so
the figure under-reads exactly when the robot is working hardest, which is exactly when §36.1's
bands are being consulted. A named gap beats a number whose error moves with the thing it is
measuring, and the contract already has the fault for it (`capacity_undeclared`) and the other path
(a fraction plus a declared `pack_capacity_wh`).

### One health source, because two would eventually disagree

The first wiring passed the reading into `checkin::health` as a fourth argument, leaving
`Daemon::health` — which `GET /health` renders — still returning `battery: None`. That would have
been a robot showing one battery figure on its own console and sending a different one to its Hub,
which is the shape of divergence this repository spends most of its checks on.

Rewritten so `Daemon::health` takes the reading and fills the two fields, and `checkin::health` reads
them back out of the same struct. One source, one number, two renderings. `checkin::health` also
lost a parameter on the way: its three measured fields are all taken in one pass, so there is one
measurement instant and one `age_ms`, and `dated()` is that rule written once instead of three
`match`es.

### `throttled_for_ms` is the one thing §36 remembers

A duration is not observable in a single reading. The instant the temperature first crossed the
threshold lives on the daemon beside `counting`, and for `counting`'s reason: a throttle window that
survived a reboot would be a robot reporting an hour of continuous throttling across a power cycle
it spent switched off.

It clears the moment the temperature drops back under, which makes "sustained" mean *continuously*
rather than *cumulatively* — a body that crosses the line for a second every minute is not a body
that has been throttling for an hour, and §36.3's DEGRADED is about the second robot.

And the subtraction is `saturating_sub`, because §35.1 keeps two clocks and corrects them
independently: a `now_ms` behind the instant the window opened is a corrected clock, and the
alternative to zero is a wrapped `u64` reporting eighteen quintillion milliseconds of throttling.

### `GET /power`, `skynet power`, and a note that says what the word cannot mean

The route reports the contract's assessment and the reader's notes **apart**, and that is not
tidiness: a fault is *§36 has a problem with what it was given* and a note is *this daemon could not
read a number*. Different remedies, usually different people, and one merged list is how an operator
learns to skim both.

It also carries a `brownout_note`, because §36.4's override has no input on this edition — the RHAL
flag and its fault code reach the daemon through a path §33.1 denies it. A robot can therefore report
`normal` while its bus is sagging, so the route says in as many words that `normal` here means *the
gauge agrees*, not *the bus does*. A word that cannot carry a claim should not be left looking like
it does.

The console needed no change at all, which is the pleasant confirmation that the wiring landed in the
right place: it has rendered `Battery` and `Temperature` from `GET /health` since the panel was
written, and both had been showing "unmeasured" for as long as the panel had existed.

### A checker that would have admitted the false name it exists to catch

`tools/check-golden-path.mjs` builds the CLI's command table by harvesting quoted literals followed
by a `|` or a `=>`. Adding one command took the count from 45 to **48**, because `print_power`
contained

```rust
match state { "critical" | "reserve" => term::bold(&headline), _ => headline }
```

and the harvester read the match arm as two more commands. Rewriting it as `state == "critical" ||
…` took it to 47 — the regex reads the first `|` of a `||` as an alternation, so a boolean is a
match arm as far as it is concerned.

Both halves are now fixed: the CLI uses a slice (`["critical", "reserve"].contains(&state)`), and
the regex guards its `\|` against `||`. The count is 46, which is 45 plus `power`.

The cost of leaving it was small and pointed the wrong way: the check exists to catch a document
writing a `skynet` command that does not exist, and every stray literal it absorbs is one more false
name it would have waved through.

### Numbers

Rust: **1279** tests, up from 1252. Twenty-seven new — nineteen in `skynetd-io::power`, six on the
route, two on the check-in. `cargo fmt --all --check` and `cargo clippy --workspace --all-targets`
clean; the corpus regenerates byte-identically; both mirrors match; the golden path counts 46
commands.

The suite's own fixture now points its sensors at a directory that does not exist, and that is
load-bearing: a daemon left on the real `/sys` would read *the build machine's* battery, so the same
fixture would produce different check-in documents on a laptop and in a container, and the suite
would pass on one and fail on the other for a reason nothing in it mentions.

### What §36 still does not have

* **The brownout override has no input**, as above. It is the same gap §48.2's self-test has, and it
  closes when the kernel channel does.
* **Nothing acts on the assessment.** `grants_new_leases` is computed, reported and read by nobody:
  §25.2's lease table does not consult it, so a robot at `reserve` will still be handed a new task.
  That is the next piece and it is the one with teeth — everything up to here is reporting.

### Findings, restated

1–28 as before; 29–40 from the sessions above.

41. **A field that is a literal `None` is not an unmeasured field, it is an unwritten one, and they
    render identically.** `battery: Measured::none()` sat in the rollup for as long as the rollup
    existed, and every surface downstream correctly reported "unmeasured" — the console, the
    check-in, the fleet page. The whole stack was honest and the bottom of it was a constant. The
    tell is a `None` with no `if` above it.
42. **Wiring a new reading into two consumers is where the divergence gets built.** The first
    attempt filled the check-in and left `GET /health` on the old literal, which would have been a
    robot with two battery figures. The habit that catches it: after adding a source, grep for every
    consumer of the *field*, not of the function you just changed.
43. **A check that harvests source text will absorb whatever the source starts saying.** One `match`
    on two strings put two false command names into the set of things `skynet <x>` may be, silently,
    in the check whose only job is to reject a `skynet <x>` that does not exist. Nothing failed. The
    tell was a count that moved by three when one command was added — which is only visible if you
    read the number the tool prints, rather than the word `ok` beside it.

---

## 2026-09-02 (thirty-seventh) — the row that was a rule nothing obeyed

§36.1's table has four rows and one of them contains an imperative:

> `low` — Approaching reserve — **No new task leases granted**; running task may finish; operator
> notified

The last two sessions built the state that computes it and the sensors that feed it.
`Assessment::grants_new_leases()` has been returning `false` on a robot below its reserve since
yesterday, and **nothing called it.** §25.2's lease table granted whatever it was asked for, on any
battery, and the row was a paragraph.

This session is the call site.

### Where the check goes decides whether the *other* half of the row works

§54.4's `acquiring` stage, before §25.2's table is consulted. Three consequences, and two of them
are the reason it is there rather than anywhere else:

* **The table is not touched at all.** A refusal that had already taken two of three tokens would be
  holding an arm for an invocation that is about to abort — the leaked-lease symptom
  `release_all` exists to prevent, arriving through the front door.
* **A running task finishes.** A skill past `acquiring` holds its leases and is not disturbed when
  the battery falls. That is §36.1's row read exactly, and it is not a separate rule: it falls out of
  gating the *acquisition* rather than the *holding*. A policy dropped mid-motion is the failure a
  controlled stop exists to avoid.
* **It outranks a held token.** Both refusals can be true at once, and the robot-wide one is what an
  operator needs first. It is also the only order that works: the runtime does not ask the table when
  power withholds, so `leases` reads `pending`, and the other order would pin the invocation in
  `acquiring` until its deadline expired.

Refused rather than parked, for the same reason. An invocation waiting for somebody to bring a
charger burns §54.4's timeout and then aborts with `timeout` on it — the wrong reason, on the wrong
screen, for the wrong person.

### One field, not two, and its default is the permissive one

`Progress.power_withheld: Option<String>` — the sentence, or nothing. A boolean and a reason beside
it are two fields that must agree, which is two fields that eventually do not; §27's
`Conditions::stop: Option<StopCause>` is the same shape for the same reason.

The sentence is composed by the *runtime* rather than by the contract, because the runtime is where
§36's assessment is in hand — and the two ways a robot withholds are different sentences:

* below its reserve, which wants a charger;
* a declared pack whose gauge answers nothing, which wants a look at the machine.

Both withhold. A shared sentence sends half the operators to the wrong errand, which is the whole
argument §64 makes.

The default is `None` — permissive — and that is unlike almost everything else in this crate, so it
is written down where the field is: **it is not a guess.** §36's own answer for a body nobody has
observed is `unmetered`, which grants leases. `None` is what the power contract computes, not an
assumption made in its absence. A default of "withholding" would have refused every skill on every
robot with no power subsystem wired up, silently, which is every development host and every test in
this suite.

### `power_withheld` is its own lifecycle reason

Not a second `lease_denied`. That enumeration now has nineteen entries and its own doc comment says
why: *"only the reason tells an operator whether to call the publisher, look at the cell, find the
package holding the arm, or read the black box."* Add **put the robot on a charger** to that list.
Three errands behind one word is two operators sent to the wrong place.

### The suite's own completeness test found it before I did

`every_lifecycle_reason_is_produced_by_some_evidence` failed on the first build after the variant
was added — *"no evidence produces `power_withheld`"* — because it walks `LIFECYCLE_REASONS` and
asserts each is reachable from some `Progress`. A vocabulary entry no rule produces is a wire value
nothing can ever send, and this repository has one test that makes that impossible to add by
accident.

It is worth naming the shape: the test is not about the new reason, it is about the *set*. Written
once, it catches every future addition. That is a different and better thing from a test per
variant.

### Numbers

Rust: **1285** tests, up from 1279. Six new — four in the contract, two through the runtime. The
corpus is **1011 cases across 73 files**, up from 1008: three lifecycle vectors (the refusal, the
order against a held token, and an empty sentence that is not a refusal) and the
`lifecycle_reasons` vocabulary, which now has nineteen.

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, both mirrors, the corpus and
the golden path all clean.

### What §36 still does not have

One thing, and it is the same shape as the other outstanding one:

* **Nothing performs a controlled stop.** §36.1's `reserve` and `critical` rows ask for one — "a
  controlled stop, then a safe state" — and `requires_safe_state` and `stops_immediately` are
  computed, reported on `GET /power`, and read by nobody. The stop path is the kernel's (§33.1), and
  this is the third entry in §73's last column waiting on the same channel as §36.4's override and
  five of §48.2's eight POST checks.

That is the honest boundary of what this edition can do about power: it can **refuse**, because a
refusal is a decision not to start something, and this daemon is what starts things. It cannot
**stop**, because stopping is the kernel's, and pretending otherwise is exactly the display §1 says
the OS must refuse to show.

### Findings, restated

1–28 as before; 29–43 from the sessions above.

44. **A computed boolean nobody reads is not enforcement, and it looks exactly like enforcement.**
    `grants_new_leases()` was correct, tested, corpus-frozen and rendered on a route for a whole
    session, and a robot at `reserve` would still have been handed a new task. The tell is a
    predicate with no call site outside its own tests — the same shape `read_calls`, `bundle` and
    `SkillManifest.task` had, which is four times in this repository now.
45. **Where a check goes decides which half of a two-clause rule you get for free.** §36.1's row
    says *no new leases* and *running task may finish*. Gating the acquisition gives both; gating
    the holding gives the first and breaks the second; a check in the daemon loop would have given
    neither cleanly. The second clause was never written as code, and it is tested.

---

## 2026-09-02 (thirty-eighth) — the sweep, and the install stage that never ran

Finding 44, from an hour ago: *"a computed boolean nobody reads is not enforcement, and it looks
exactly like enforcement."* The log has now recorded that shape four times — `read_calls`,
`skynet_contracts::bundle`, `SkillManifest.task`, and `grants_new_leases` — each found by accident,
each after months. So the first thing this session did was look for the rest of them mechanically.

### The sweep

Every `pub fn` in `skynet-contracts`, minus the corpus wire-shape helpers, against every reference
to it anywhere in the workspace outside its own module, its own tests and the corpus runner.

**Seven of eighty-eight decision functions had no consumer at all.** Four are legitimate API
surface — a convenience wrapper (`role_key_of`), a borrow-chain helper (`with_frame_set`), a display
formatter (`format_pairing_code`), a console sentence generator whose console does not exist yet
(`explain_omissions`). Three were real:

* `field_conformance` and `conformance_of` — §15's field suite, computed from a telemetry window and
  read by nothing. Left for a session of its own; it belongs with the conformance runner §73 already
  names as missing.
* **`is_installable`** — *"Whether the OS will install a policy under this plan."*

The third is the one worth an evening, and it is worse than the sweep suggested. It is not that the
predicate has no caller: **`resolve_transport` itself has no caller on a robot.** Its three callers
are the Hub's repository page, `skynet transport` (a developer diagnostic), and the corpus.

### §37.1 has six stages and one of them had never run

```
resolve → fetch → verify → admit → transport → stage → activate
```

> `transport` — `resolveTransport()` against this body; a `refused` verdict fails the install

§6 calls transport *"the heart of the system"* and §6.2 says it runs *"on the robot, at install
time, with no network at all."* `admit_package` has taken a `transport_verdict: Option<_>` since
admission was written, and on a robot it was `None` every time, with a comment saying why: *"there
is no policy to plan for at stage time and no source embodiment to plan from."*

Which was true, and was the whole of the gap. The manifest had no action space in it.

`PackageManifest.source_embodiment` is the missing half — the body the policy was trained on and
the channels it emits, which is exactly what `skynet transport` has been reading out of a file all
along. The other half, the descriptor, has been on the robot since §28.

So the stage runs. A policy whose required channel this body has no equivalent for is now **refused
at install**, which §6 has said since Part I and no robot has ever done.

### Two options, decided rather than declared

**`requested_hz` is the package's `budget.control_hz`**, not the source embodiment's. Those are two
declarations by one publisher in one document and they may disagree: the first is the rate the loop
will run at *here*, the second is the rate it was *trained* at. The plan is made against the first,
because a plan that cleared a rate the package never intends to run is a plan about a different
robot.

**`allow_retarget` is the operator's**, and §6.1 says why it cannot be the publisher's:
*"retargeting changes what the policy is doing. That should be a decision somebody made, not a
fallback that happens quietly."* A publisher setting it in a manifest is precisely the quiet
fallback. So it is `--allow-retarget` on `skynetd stage` — the person running the command is the
somebody — and the reconcile loop, which has nobody standing at it, passes `false`.

Asking for it does not manufacture a solver, and there is a test that says so: a body declaring no
IK solver refuses the channel however the flag is set, which is §6.1's *"the request is refused
rather than promised and discovered at the first command."*

### Two ways there is nothing to plan, and neither is a pass

* **The package declares no action space** — a driver, an environment, a dataset. None of them
  command a joint. A verdict about them would answer a question nobody asked.
* **The robot has no usable descriptor.** §7.2's rule: the absence is reported by the surfaces that
  report descriptors, not converted into a verdict here. A `refused` invented from a missing body
  would fail every install on an unprovisioned robot with a sentence about the *policy* — which
  would send an operator to the publisher for a problem in their own provisioning.

Both produce `None`, which admission reads as *no verdict was reached* rather than as a pass. That
distinction was already in `admit_package` and had never had a second case to distinguish.

### The refusal names the channel

`TransportRefused`'s message is *"The policy's action space cannot be transported onto this body."*
True, and it tells nobody which joint to look at. So the report carries every binding that came out
anything but `direct` — its index, its declared role, its method, its target and its reason — and
the `direct` ones are omitted, because naming all forty on a humanoid buries the one that is
blocked. That is §8.2's rule about findings, applied to the thing the findings are about.

### The mirror check earned its place again

`package.ts` is a mirrored contract, so adding `source_embodiment` to the Rust manifest and to the
reference left `check-contract-mirrors.mjs` reporting `DIVERGED package — 18 line(s) differ`
immediately. The Hub's copy took the same edit and the pins were rewritten from the two agreeing
copies. Nothing about `admitPackage` changes — the Hub does not read the field, the robot does —
which is exactly the case where a mirror would otherwise rot unnoticed for months.

### Numbers

Rust: **1290** tests, up from 1285. Five new, all on the stage: a policy that fits, a required
channel that does not, the two absences, the retarget decision, and the rate the plan is made
against. `cargo fmt --all --check` and `cargo clippy --workspace --all-targets` clean — the latter
after boxing `Report::Ran`'s plan, because a binding per channel on a forty-joint body would
otherwise make every `Report`, including the one that carries a single sentence, that size.

The corpus is unchanged at 1011 cases, and that is the right answer: `admitPackage` does not read
the new field, so no frozen vector moves. Both mirror checks match, the golden path counts 46
commands.

### What is still missing here

§73's transport row said "—" in its last column, and it was wrong in the direction §73 is always
wrong in. It now says the true thing: **the ceilings the plan computes are handed to nothing.** §6.3
is explicit that deciding and enforcing are separate and that the clamp goes to the Safety Kernel —
and `TransportClamp` has no reader on this robot, because no policy runtime exists to carry a
command as far as a kernel. Planning at install is now real; enforcing at 1 kHz is still the
kernel's, through the channel §33.1 denies this daemon.

### Findings, restated

1–28 as before; 29–45 from the sessions above.

46. **The orphan sweep is worth running, and it is four lines of Python.** Every `pub fn` against
    every reference to it outside its own module, its tests and the corpus. Four of the seven hits
    were legitimate API surface, which is a false-positive rate a person can read in a minute — and
    the three real ones included a whole install stage that had never executed. The log had recorded
    this shape four times without anybody thinking to look for the fifth.
47. **An orphaned predicate can be the smaller half of the problem.** `is_installable` had no
    caller; the sharper fact was that the *function it asks about* had no caller either. When a
    sweep turns up an unused decision, the next question is not "who should call this" but "does
    anything compute what it decides about" — and here the answer was: three places, none of them
    the one the specification names.

---

## 2026-09-02 (thirty-ninth) — the other two orphans, and one formula written twice

The sweep in the previous session flagged three genuine orphans and one was §37.1's transport stage.
The other two were `field_conformance` and `conformance_of` — and the interesting thing about them
is not that neither had a caller. It is that **they are the same formula.**

```rust
// certification.rs                       // telemetry.rs
(cycles - interventions) / cycles         ((cycles - interventions) / cycles)
    .clamp(0.0, 1.0)                          .clamp(0.0, 1.0)
```

Two crates, two doc comments arguing the same clamp, neither with a consumer, and nothing anywhere
comparing them. §15's field conformance is *"the mechanism by which certification stops being a
document and becomes a live property"* — the number that decides whether a robot's certificate is
suspended — and it had two definitions.

`conformance_of` now calls `field_conformance`, in both editions. One definition, and it belongs to
the module that checks it against §15's floor.

### A value both editions compute and neither corpus holds

Worse than the duplication: **`conformance_of` was in no corpus at all.** `shapeCoverage` emitted
gaps, totals, spans and unmeasured names, and not the one number the coverage is computed *for*.
Two editions, free to disagree, with a green suite either way.

It is in the shape now, and with it four cases that the existing five could not have discriminated —
because every one of them sat comfortably above §15's floor:

| Case | Conformance |
|---|---|
| under the floor — 40 interventions in 30,000 cycles | 0.99867 |
| exactly at the floor — 30 in 30,000 | 0.999 |
| cycles counted, interventions not | 1 |
| more interventions than cycles | 0 |

The tamper tests now bite: removing the clamp fails two vectors, and removing the
absent-interventions rule fails two. Before them, both changes passed.

The third case is the one that needed a decision. §74.3's rule is that unmeasured and zero are never
the same, and here they are: a window that counted cycles and did not count interventions is a robot
whose **kernel intervened in none of them**, because the counter that would have recorded one is the
same loop that recorded the cycles. The rule bends exactly once and the reason is written where it
bends. Absent *cycles* is still absent.

### `package_restarts` had a source and had never been asked

`skynetd-io/src/telemetry.rs` opened with:

> **six of the eight counters have no source.** … `interventions` needs teleoperation and
> `package_restarts` needs the supervisor to count them, **which it does not**.

That last clause was a statement about the wiring dressed as a statement about the machine. §33.5's
supervisor decides every restart on this robot — `on_exit` returns `RestartAt`, `Failed` or
`AwaitOperator` — and the daemon threw the decision away.

Counted at the **decision** rather than at the start it leads to. The two are one reconcile pass
apart, and a decision whose start never happened is a robot in *worse* trouble, so a counter waiting
for the start would under-report exactly the robot a fleet page is hunting for. `Failed` and
`AwaitOperator` are not restarts and are not counted: they are a package that stopped coming back,
which the readiness column already says.

It is a count since the window opened rather than a reading taken at both ends — unlike
`queue_dropped` beside it — because the supervisor keeps a *bounded* failure list whose entries
expire out of §33.5's ten-minute window, and subtracting two readings of that would report a
negative number of restarts on a robot that had been quiet.

Three of eight now, and the five that remain are the four the kernel owns plus teleoperation.

### Both freezes fired, which is why they exist

Two tests failed the moment the counter appeared, and neither was a nuisance:

* the unit test asserting which six counters are unmeasured;
* **`reference/wire/telemetry.json`** — the frozen bytes this daemon puts on §17's wire, which the
  Hub holds a copy of and asserts against.

The wire fixture was re-frozen, resynced, and the Hub's own assertion updated — *"this edition
measures two of the eight"* became three, with `package_restarts` asserted as **zero rather than
undefined**, because a quiet robot has to be distinguishable from one that counts nothing or a fleet
page filtering for churn has no robot to compare against.

That is the twenty-seventh session's mechanism paying off again: a producer and a consumer in two
repositories, and the change could not land in one without the other being made to say something.

### Numbers

Rust: **1291** tests, up from 1290. The corpus is **1015 cases across 73 files**, up from 1011 —
four coverage vectors and the conformance field on every existing one. `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets`, both mirrors and the golden path clean; the Hub's two
synced contracts typecheck.

§73's telemetry row goes from *"six of eight counters have no source"* to five, and now says which
five and why: four are the kernel's, one is teleoperation's.

### Findings, restated

1–28 as before; 29–47 from the sessions above.

48. **Two orphans can be one duplicate.** The sweep found `field_conformance` and `conformance_of`
    separately and the finding was not "two unused functions" — it was that §15's number had two
    implementations. An unused function is cheap; an unused function that is a *second* copy of a
    rule is a divergence waiting for the first caller. Worth asking of every sweep hit: is anything
    else computing this?
49. **"Which we do not" is a sentence to check, not to believe.** The telemetry module said
    `package_restarts` needed a supervisor to count them, which it did not — written when it was
    true of the wiring, and read for months as though it were true of the machine. The supervisor
    was three lines away. The tell is a doc comment that explains an absence by naming a component
    the repository already has.

---

## 2026-09-02 (fortieth) — the suite, run by the robot on itself

§73's conformance row has said the same thing since it was written:

> No runner executes the suite on a robot, and nothing signs a result.

The second clause turns out not to be a gap at all — §19 says a robot verifies and never signs, so a
robot signing a certificate about itself is a thing the design forbids rather than a thing it lacks.
The first clause was real, and this session is it.

### The eight sentences, and five that had gone stale

`skynet driver check` runs the driver-facing sim tests and lists, by name, eight it cannot cover:
tests that are properties of the **runtime** rather than of a driver. Each carries a sentence
explaining the omission, and each sentence is of the form *"needs X"*.

Five of them named an X the platform has since built:

| Test | Said | Built since |
|---|---|---|
| `package.signature.required` | needs the package installer | §37.1's installer, five signature states |
| `package.grants.gate` | needs the permission model | grants, admission, `effective_grants` |
| `ota.safety_gate` | needs the updater | §12, and §75's bundle route |
| `telemetry.replay.deterministic` | needs the black box | ring, MCAP, sealing, replay |
| `security.credential.scope` | needs device tokens | `skd_` credentials and their scopes |

This is finding 49 at scale, and it is the same mechanism: a sentence explaining an absence is a
claim, it decays exactly as fast as the absence does, and nothing in a repository re-reads one.

Re-asking the eight questions turned out to be worth as much as answering them:

* **Two execute now.** `ota.safety_gate` and `package.signature.required`.
* **One is answerable and is not this machine's to answer.** `security.credential.scope` asks
  whether a credential lifted from a robot can read repositories or write to another namespace —
  which is a question about whether *the Hub* enforces the scopes. A robot reporting a pass would be
  reporting on somebody else's enforcement. The sentence now says so.
* **One turns out to be half-built, and not the half the test measures.** `package.grants.gate` is
  about a package that *commands* an ungranted capability and the `mode_gate` guard refusing it.
  Admission refuses one that *requests* one, at install. Those are different moments and different
  components, and the second is the kernel's.
* **Two are waiting on something real.** `safety.policy.isolation` needs a sandbox; the black box's
  replay needs `resolve_limits`, which exists in the kernel crate and has no caller on the daemon —
  another orphan, and the next session's.

### The result the runner exists to produce

`package.signature.required` — *"An unsigned package, and one whose bytes do not match its digest,
are both refused"* — runs against this robot's real capacity, through the same `admit_package` its
installer runs, over two synthetic manifests. Both are refused by the decision.

**And it reports `fail`,** because §37.1's `admit` stage *reports* on this edition rather than
gating, for every package kind but a driver. An unsigned package installs. `crate::admit` has said
so in its own header for sessions; nothing had ever turned it into a verdict.

That is the whole argument for a runner. Reporting it as `skipped` would hide a deliberate decision
behind an unmeasured absence, and the two look identical on a dashboard. It is a fail, it caps this
robot's own evidence below guest, and a platform whose suite says that about it is a platform whose
suite means something.

`ota.safety_gate` passes, and the pass is real rather than a restatement: an image changing
`safety_kernel` **and** `envelope`, with no bench pass, put through the same `admit_bundle` the
updater runs, refused.

### A `GET` that runs checks

Worth defending, because a read that does work is how a route acquires a side effect nobody notices.
Neither check has one: both are pure functions over synthetic documents, nothing is written and
nothing is staged. There is a test that runs the suite twice and asserts `GET /status` is
byte-identical either side of it.

### Two rules the runner applies to itself

* **Absence is not a pass**, harder here than anywhere: a conformance runner that reported `pass`
  for a test it did not perform would be the most damaging thing this repository could ship. Every
  entry is a check that executed or a named absence, and there is no third state. A test that
  produced neither would be a row with no sentence, and there is a test for that too.
* **A skipped test carries no date.** §15 decays an award from the age of the oldest result, so a
  gap dated today would postpone the decay it should be causing.

And the tier: `attainable` claims `certified` deliberately, because §15's rule is that the evidence
caps the award rather than the claim — a low claim would produce a low award for a reason that has
nothing to do with the machine. With six of eight skipped and one failed, it cannot come out
anywhere but the floor, which is the correct answer for a robot that has run two sim tests.

### Numbers

Rust: **1303** tests, up from 1291. Twelve new — nine on the runner, three on the route. `cargo fmt
--all --check` and `cargo clippy --workspace --all-targets` clean; both mirrors match; the golden
path counts 47 commands, which is 46 plus `conformance`.

§15 gains §15.1, which says which runner runs which half of the suite and why nothing on a robot
signs. §73's row moves from "no runner" to "two of the eight execute; the other six name what they
wait for".

### Findings, restated

1–28 as before; 29–49 from the sessions above.

50. **A list of things you cannot do is a list that rots, and it rots silently in the direction that
    matters.** Five of eight sentences named a component the repository had since built, so the list
    understated the platform *and* pointed every reader away from the place the work should go. It
    cannot be caught by a test, because there is nothing to compare a sentence against — the only
    mechanism is to re-read the omissions when the components they name are built, which is a habit
    rather than a check.
51. **Running the suite on yourself is worth doing before it passes.** The valuable output of this
    session is a `fail` on a guest-tier test, produced by the platform about itself, sitting on a
    route an operator can read. A runner built only when it would come out green would be a runner
    that never measured anything anybody did not already believe.

---

## 2026-09-03 (forty-first) — the limits a robot is under, and the replay that was waiting for them

The fortieth session ended by naming this one: `telemetry.replay.deterministic` was skipped because
*"nothing on this daemon calls `resolve_limits`"*, and `resolve_limits` was an orphan on the robot.
It is not an orphan any more, and closing it turned out to answer four other things.

### The suite was red on the machine it is written on

Before any of it: `cargo test --workspace` failed here, on one assertion, and had been failing since
the fortieth session landed.

`the_conformance_route_runs_what_it_can_and_names_what_it_cannot` asserts that
`package.signature.required` comes back `fail`. It does — **on Linux**. §8.1's memory figure comes
from `/proc/meminfo`, `crate::capacity` refuses to invent one anywhere else, and the runner then
skips the admission probe rather than running it against a machine that does not exist. All of that
is correct and every piece of it was written deliberately. The test simply asserted one of the two
answers.

CI is `ubuntu-latest`, so it was green there and red on the developer's machine — and red on the
*one row the runner exists to report*, which is the row a reader most wants to trust. Both branches
are asserted now, with the executed count either side of them, so neither host is sitting a shorter
exam than the other.

The thing worth keeping is not the fix. It is that a repository whose suite is red locally and green
remotely teaches its author to stop reading the suite, and it does that within about a week.

### An orphan with three callers

`resolve_limits` is the function §7.3 exists to describe: three sources of ceilings — the declared
envelope, the transport plan, the operator profile — composed by minimum into the one set the kernel
enforces. Its callers were the kernel's own suite, the corpus runner, and `skynet-contracts`'
renderer.

Every one of them hands it a document. **None of them is a robot.** So the function that decides what
a machine may do had never once been asked what *this* machine may do — and no sweep finds that,
because the function is used.

`skynetd-io/src/limits.rs` is the composition, `GET /limits` serves it, `skynet limits` prints it.
What it reads is the descriptor's envelope and frame graph, the signed profile's ceilings, and
§55.2's cell; `?package=<id>` folds in §37.1's transport clamp for one installed package, which is
§6.3's *"hands the ceilings to the Safety Kernel"* performed for the first time.

### The tier is an input, and the robot does not have one

`resolve_limits` takes a tier. Nothing on a robot decides one — §15 awards it from bench evidence a
human witnessed, §11 would deliver the award in desired state, and neither path is written.
`crate::runtime` already refuses to fill `robot.tier` for exactly that reason.

Choosing one anyway would be the invented fact the whole arrangement exists to prevent. What makes it
safe to resolve at all is a property of the function, and the property is asserted rather than
asserted-in-a-comment:

> The tier changes what is done with the limits. It never changes what they are.

`the_ceilings_do_not_depend_on_the_tier_and_only_the_enforcement_moves` resolves the same machine
three times and requires every ceiling, every active guard and every unavailable guard to be
identical, then requires the two notices that *do* move — `advisory_only` and `no_independent_stop`
— to move. The day it fails is the day the assumption has to be replaced by an awarded tier rather
than defended, and the failure will say so.

The daemon resolves at **guest**, which is a description of this build rather than a claim about the
machine: §1 defines guest as the tier that does not own the control loop, and no policy runtime
exists here to own one.

### Three inputs with no source, named rather than defaulted

`OperatorProfile` has seven fields. §11's desired state carries four ceilings and a cell. So:

| Input | Why it is absent |
|---|---|
| `profile.proximity` | no field in desired state — so the `proximity` guard can never be active on this edition, however the body is described |
| `profile.self_collision_margin_m` | the same; two of this robot's own bodies must meet before the guard holds |
| `available.people_detection` | §5 declares nine sensing modalities and **none of them is a person detector** |

The third is the one worth defending. A camera is not a person detector — detection is a perception
component, not a sensor — and reading `people_detection` off an `rgb` stream would be precisely the
*claim about the body that nothing checks* that `Available` had two fields deleted for. It is `false`
on every body and the `proximity` guard says so, naming what it wants.

### The cell's volumes stop being reported and start being applied

`crate::cell` has said this about §55.2's workspace layer since it was written:

> the layer is computed and **reported**, with the reason it goes no further

It goes further now. The layer is folded into the operator profile the kernel reads, so a robot with
a surveyed cell and a frame graph that reaches the frames it names has `workspace` among its active
guards for the first time — and one whose graph does not reach them gets the missing frame named,
which is §44.5 working rather than a body quietly unguarded.

### A guard can be active and have nothing to compare against

The replay check found this and it is the most useful thing in the session.

`envelope` has no missing input, so it is active on every robot. The window the runner records asks
for a thousand times any plausible ceiling — and it is forwarded unchanged, on a body that declares
`joint_velocity_max_rad_s: 1.0`.

That is correct. A joint's ceiling comes from the transport plan's `per_channel` (§6.3), because
nothing at the kernel's layer knows whether a channel carries radians per second or newton-metres —
and a robot running no policy has no plan in force. So the honest report is: envelope declared,
guard active, nothing clamped.

`active` reads as `enforcing`. §7.2 spent a subsection making *unavailable* guards visible and this
is the case it does not cover, because the guard is genuinely available and genuinely idle. Both the
route and the conformance detail now say it in a sentence rather than leaving it to be derived from
`active_guards` and an empty `per_channel`.

### The third of eight, and what it caught on its first run

`telemetry.replay.deterministic` executes. This robot's ring holds audit events and no control
cycles, because nothing here drives a loop — so the runner records a window itself, the same way
`signature_required` builds two manifests and `ota_safety_gate` builds a bundle. What is synthetic is
five commands. What is real is everything that decides the answer: this body's channel names, its
resolved limits, the same `evaluate_cycle` a control loop would call, the same ring the recorder
writes, and the same `replay()` an incident review runs.

The window is shaped for the fields a round trip can lose — an unwarmed cycle, a warmed one, one far
above any ceiling, **one stamped long enough ago that the watchdog discards it and emits no command
at all** (a different record from a command of zero), and a return to zero.

It failed the first time it ran, on two cycles, with `command recorded shoulder=0.2, recomputed
shoulder=0.20000000298023224`.

Not a bug in replay. §25's command ring carries `f32` and §26 records `desired` as `f32`, so on a
robot every desired value the kernel sees is already exactly representable in `f32` — which is what
makes an unclamped command survive the recording bit for bit, and `replay()` compares commands by bit
equality on purpose. `0.2` is not representable. The window was made of numbers no policy on this
robot could ever have produced.

The fix is one line and it is load-bearing: the runner quantises through `f32` before the kernel sees
anything, so it models the wire. Loosening the comparison would have been the other option and it
would have thrown away the guarantee the check exists for.

### Numbers

Rust: `skynetd-io` goes from **450** tests to **463** — thirteen new: eight on the limits module,
three on the route, two on the conformance runner. The workspace figure in §73 moves by the same
thirteen, 1,303 to 1,316. A Windows checkout runs 1,286 of them, because six `memfd` tests and the
systemd end-to-end need Linux.

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets` clean; both document mirrors
match and were re-pinned; the two synced contracts match; the golden path counts **48** commands,
which is 47 plus `limits`.

§73 gains §7.6 behind it and moves five rows: transport (the hand-off is performed), the Safety
Kernel (resolution runs on a robot, at an assumed tier), the black box (a robot replays a window
against its own limits — and no *policy* recording has ever been replayed, because none has run),
conformance (two of eight execute becomes three), and configuration (the three undeliverable
profile fields). The stale figures in §73's summary and the README's crate table were re-measured
while the tables were open.

### Findings, restated

1–28 as before; 29–51 from the sessions above.

52. **A suite that is green in CI and red on the machine it is written on stops being read.** The one
    failing assertion here was about the *host* rather than about the edition, and it failed on the
    single row the runner exists to report. The fix is not the interesting part: the mechanism is
    that a developer who sees red every morning learns within a week to run the suite and skim past
    it, and no test catches that.
53. **An orphan can have callers.** `resolve_limits` had three and every one of them was a test, a
    corpus or a renderer. Dead-code tooling finds a function nobody calls; nothing finds a function
    nobody *in the place that matters* calls, and the tell is a caller list in which no entry is the
    production path. Worth asking of any core function: which of its callers is the machine?
54. **An assumption is defensible exactly when you can prove it decides nothing.** The tier had to be
    assumed. What makes that honest rather than a fudge is a test that resolves the same robot at all
    three tiers and requires the numbers to be identical — so the assumption is confined to a label
    on a report, mechanically, and the confinement breaks loudly.
55. **A guard reported active is not a guard enforcing anything.** `envelope` is active on every robot
    and clamps nothing without a transport plan. §7.2 made *unavailable* guards say what they lack;
    the available-and-idle case has no such mechanism, reads as the reassuring one, and had to be
    written down in a sentence because there is no field that carries it.
56. **A wire format's precision is a constraint on every synthetic input, not only on the wire.** The
    replay window diverged because `0.2` does not survive `f32`, and the answer was to model the wire
    rather than to loosen the comparison. A test input the real path could not have carried is a test
    reporting on arithmetic instead of on the machine — and the tempting fix, an epsilon, would have
    deleted the guarantee the check exists for.

---

## 2026-09-03 (forty-second) — the Guards panel, which had been confidently wrong

`GET /limits` existed for about an hour before it became obvious where it was most needed. §32.2's
console has a Guards panel, and this is what it drew:

> §32.2 asks for live guard state, this daemon has none, and eight green ticks would be a lie told
> convincingly.

The instinct was right and the execution had rotted. What the panel actually contained was a
hand-written list of eight guards, each with a hand-written sentence naming what it was missing, all
eight drawn as `unavailable`. Written when that was true. Maintained by nobody, because nothing
compares a sentence against a machine.

### Two things it said that were not so

**`balance` — "implemented by no edition".** `skynet-kernel` implements it, in the 101 tests that row
of §73 counts, and on a fixed base it reports **not applicable to this body** — which is a distinction
§7.2 spends a paragraph on, so that *"nobody reads a shape difference as a defect"*. The console read
it as a defect and told the operator no edition had built it.

**All eight `unavailable`.** `envelope` and `mode_gate` have no missing input at all; they are active
on every robot. `watchdog` is active on any body that declares a window. The panel drew three active
guards as off.

A page whose one rule is *never show a value it did not receive* was showing five values it had made
up, in the panel an operator opens to find out why the robot did not stop.

### What it draws now

The route. Every string in the guard table is the kernel's own, computed for this body:

```
GUARD           STATE                 MISSING INPUT
watchdog        active
envelope        active · no ceiling   a per-channel ceiling — §6.3 hands one to the kernel
                                      from a transport plan, and no policy is running to have one
mode_gate       active
rate_limit      unavailable           profile.max_delta_per_cycle
workspace       unavailable           profile.workspace, embodiment.frames
self_collision  unavailable           embodiment.frames
balance         unavailable           a legged base — not applicable to this body
proximity       unavailable           profile.proximity
```

Above it the five ceilings with their provenance, and `nobody declared one` where there is none.
Below it the notices, the tier and why it is assumed, and the three inputs with no delivery path.

**Trip counts stay absent**, and they are the reason the old panel's instinct was right: §33.1 keeps
the daemon off the actuator path, nothing evaluates a cycle here, and a `0` in that column would read
as *nothing tripped*. It is a sentence under the table instead.

### Three small things, all found by opening the page

The fixture (§32.2's, from the thirty-first session) exists precisely so somebody looks, and looking
found three:

* **`guard_unavailable` notices, five of them, saying what the table said.** The kernel emits them
  because a consumer holding only the notice list would otherwise never learn a guard is off. This
  panel has the table, with the missing input in its own column — so the notice is one fact in two
  places, and five copies of it pushed the four notices that *do* carry something new off the bottom.
  Dropped, and only that code.
* **A pill wrapped to three lines.** A `border-radius: 999px` on a box that has wrapped is not a
  rounded rectangle; it is a shape with its text falling out of the corners. `.pill` is
  `white-space: nowrap` now, because a pill is a label and not a sentence, and the label that would
  not fit is shorter.
* **Two feeds the fixture never recorded.** `releases` and `cell` are polled by the page and had no
  answer in the fixture, so anybody rendering it saw two panels unreachable — which looks exactly
  like the failure the fixture exists to let somebody look at. The route list is a constant now, and
  `every_console_feed_has_a_fixture_route` reads `new Feed(` out of the page and checks it. Two
  lists in two languages, kept in step by reading one of them.

### Numbers

Rust: **1,317** tests, one new. `cargo fmt --all --check` and `cargo clippy --workspace
--all-targets` clean; the mirrors match and were re-pinned; the golden path counts 48 commands.

§73's interface row moves from *"three of §32.1's four panels are named gaps"* to a sharper
statement: the robot now computes two of the device page's four panels, **and neither crosses §17**.
The check-in carries no guard set and no limits, so the Hub cannot render what the robot knows. That
is a producer with no consumer, which is the exact shape Part X was written about — and it is the
next thing to build.

### Findings, restated

1–28 as before; 29–51 from the sessions above; 52–56 from the forty-first.

57. **A panel that says "this is not measured" can itself be a fabrication.** The old Guards panel was
    written to avoid showing eight green ticks — the right instinct — and did it by hand-writing
    eight rows of what was missing. Two of them were false and three had the wrong state. Refusing to
    invent a *value* while inventing the *explanation* is the same failure wearing the humbler
    costume, and it is harder to spot because the page reads as scrupulous.
58. **When a computed answer arrives, the hand-written one has to be deleted rather than checked.**
    The temptation on wiring `/limits` in was to keep the sentences and fill in the states. Every
    sentence in that table is now the kernel's, for this body, and none of it can drift again —
    which is the only version of "keep it up to date" that has ever worked.

---

## 2026-09-03 (forty-third) — the limits cross §17, and the Hub stops guessing

The forty-second session ended naming this one:

> the robot now computes two of the device page's four panels, **and neither crosses §17**. That is
> a producer with no consumer, which is the exact shape Part X was written about.

**Touches the Hub.** Both repositories, in one session, per `docs/README.md`'s rule — and recorded
here because this log has to be a complete account of what was done from this side.

### What each half already said about the other

The Hub's device-state route has carried this since it was written:

> §32.1 wants *"active and unavailable guards with the missing input named, resolved limits and
> their provenance, current transport plan"* on this page. **None of those are in a check-in.** The
> robot has all three and serves them on its own LAN console, and no route brings them here.

And it did the honest thing: a `gaps` list, three entries, each naming what was missing and why —
rather than deriving a guard state from the health rollup, which is what a fleet page would be most
tempted to do. Two of those three entries are deleted now, and **deleted rather than softened**: a
gap that has closed and is still described points every reader away from the thing that works.

### The block, and the two fields nobody may collapse

§17's check-in carries `limits`:

```
resolved_at  guest          the tier these limits were resolved at
enforcement  advisory       what that tier permits
enforced     false          whether anything on the robot applies them
… five ceilings, each null or a number, and where they came from
active       [watchdog, envelope, rate_limit, workspace, self_collision, mode_gate]
unavailable  [{balance, ["a legged base — not applicable to this body"]}, …]
notices      [{advisory_only, []}, {no_collision_margin, []}, …]
```

Four decisions in that shape are worth defending.

**`resolved_at` is not called `tier`.** §15 awards a tier from bench evidence a human witnessed and
§19 says a robot never awards itself one. A field called `tier` on a document a robot sends would be
read as a claim within a week; `resolved_at` is `ResolvedLimits`' own field under its own name, and
it means what it says.

**`enforcement` and `enforced` are different facts.** The first is what the tier permits; the second
is whether anything does it, which on an edition with no policy runtime is false at every tier. A
consumer holding only the first renders a green guard list over a robot that enforces nothing, which
is the single most misleading thing this page could draw.

**Notices cross as codes and subjects, never sentences.** Both sides hold the same catalogue, and
the Hub renders in thirty-five locales — a robot shipping English prose puts the translation on the
side that cannot do it. `guard_unavailable` is carried anyway, even though the guard table beside it
says the same thing: the wire is evidence, and a *renderer* decides what to show. Both renderers
filter it; neither wire does.

**It is a field on the check-in rather than a route.** §34.4 collapses check-ins because every field
in one is a statement about now, and these are — the limits in force this pass, not a count, a delta
or a history. About five hundred bytes, which is what a robot reconnecting after a week offline
should send exactly one of.

### Relayed, and the restraint is the point

The Hub does not recompute a single row, and that is a rule rather than an omission. It holds a copy
of the descriptor **and** a mirror of `resolveLimits()`, so a computed table would typecheck. It
would also be wrong twice over:

* it would report the limits a robot *should* be running rather than the ones it is — a profile that
  has not been applied yet is precisely the case §17's own rule exists for;
* it would get `balance` on a fixed base wrong first, reporting a missing IMU where §7.2 requires
  *"not applicable to this body"* — the distinction that section spends a paragraph on so that
  *"nobody reads a shape difference as a defect"*.

That sentence now crosses two repositories verbatim, and there is a test on each side asserting it
does.

### A fourth wire fixture, and the one that was overdue

`reference/wire/checkin.json`. The check-in is the **most-sent document on §17** and it had never
been frozen: both repositories decoded it, both suites passed, and nothing held one side's real
bytes against the other side's real route — which is the arrangement the twenty-seventh session
found the episode path in, where the two ends did not agree about a single field name.

Taken from a robot that is **described and configured**, on purpose. A fixture from a bare
development root freezes a `limits` block of all nulls, and the shape a fleet page actually has to
render — six active guards, two unavailable with their reasons, ceilings with provenance — would go
untested on both sides. Six assertions run against it in the Hub's suite.

### The corpus, and both editions

`checkin/document` gains two cases: a robot under limits, and one that reports none. The second is
the distinction the field is shaped around — `limits: null` is *"this edition does not compose
them"* and a block full of nulls with a `no_envelope` notice is *"this robot has no ceilings"*, and a
consumer switching on a missing key cannot tell them apart.

The Rust edition reproduced both byte for byte on the first run, which is what the reference and the
corpus are for.

### Numbers

Rust: **1,321** tests, up from 1,317 — three on the contract and one wire fixture. The corpus is
1,017 cases across 73 files. `cargo fmt --all --check` and `cargo clippy --workspace --all-targets`
clean; both document mirrors and all eighteen contract mirrors match and were re-pinned, along with
a fourth wire fixture.

Hub: **1,485** tests, up from 1,479 — six on the new fixture. `tsc --noEmit` clean. Sixteen new
`messages/en.json` keys, synced to all thirty-five locales; one existing string was rewritten, which
invalidated its thirty-four translations, which is the sync doing its job.

§73 moves four rows: the Safety Kernel becomes **built on both sides**, the Hub contract counts four
frozen fixtures, configuration notes that the ceilings a profile resolves to now reach the Hub
rather than only the digest that was applied, and the interface row goes from two panels that do not
cross to one panel that remains — the transport plan, which belongs to a package on a robot running
none.

### Findings, restated

1–28 as before; 29–51 from the sessions above; 52–56 from the forty-first; 57–58 from the
forty-second.

59. **A well-written gap list is the thing most likely to survive the gap.** The Hub's `gaps` array
    was exemplary — three entries, each naming what was missing and why, refusing to approximate.
    That quality is exactly what made it read as current, and it would have gone on being read as
    current for as long as nobody re-derived it. The habit that catches it is not a test: it is
    treating "the other side says it cannot" as a claim to check whenever this side builds something.
60. **Two implementations of one rule is a defect even when one of them is right.** The Hub could
    compute the guard table — it has the descriptor and a mirror of the kernel — and the argument
    against is not that it would be wrong today. It is that the robot's answer is about the profile
    the robot *applied*, and the Hub's would be about the profile it *sent*, and the two differ
    exactly in the window an operator is looking at the page.
61. **A field's name is a load-bearing part of the contract.** `tier` would have been read as a tier
    claim by the first person to touch the fleet page, and no doc comment reaches that person.
    `resolved_at` cannot be misread the same way. The same is true of the `enforcement` / `enforced`
    pair: two words apart, and the distinction between what is permitted and what is done.

---

## 2026-09-03 (forty-fourth) — the Plan panel, and a plan for a package that is not running yet

§32.2's console has a Plan panel, and half of it said this:

> **Transport plan** — *unavailable on this robot.* Resolving one needs a body and a policy trained
> on some other body. This robot now has the first… `skynet transport` computes exactly this plan on
> a workstation, and the numbers it produces are the numbers this panel will show.

Every clause was true when it was written. §37.1's `transport` stage has been running at install
since the thirty-ninth session, `GET /limits?package=` has been folding its clamp into
`resolve_limits` since the forty-first, and the panel had gone on saying the plan was computed
somewhere else.

### The route answers with the plan, not only its clamp

They are different questions and §32.2 asks both. The clamp is *the number a guard would enforce*;
the plan is *why that number is what it is*. A ceiling of 80 Nm on `shoulder` says nothing about the
sentence a warning carries:

> Commands to "shoulder" are scaled to 44% because the policy was trained on a stronger joint.

That is §6.1's *"scaling never guesses"* arriving in front of an operator, and no ceiling carries it.
`GET /limits?package=` now answers with both, `skynet limits --package` prints it, and the console's
subpanel asks which package and draws it.

### Staged, and the answer says so

The route looks up the active digest first and then the **staged** one. §37.1 makes *staged but not
activated* an observable state deliberately — fetching and verifying happen under a running policy
and only the symlink swap waits for a gate — and it is the state this answer is most wanted in. §6.2
is explicit that a pure planner exists to answer *"does this run on my robot, and how well"* **before**
anything is committed to, and a route that only spoke about running packages would answer it only
after.

`package_state` says which, and both the panel and the terminal print it, because *"what would this
be held to"* and *"what is this held to"* are different sentences and only one is about the robot now.

That decision came out of a test failure rather than out of design. The first version activated the
package and died on `the atomic symlink swap §37.1 requires exists on POSIX and this is not POSIX` —
a test that would have been green in CI and unrunnable on the machine it was written on, which is
finding 52 from three sessions ago arriving again in a different costume. The fix was not to gate the
test on `cfg(unix)`. It was to notice that the route was refusing an answer it had.

### A picker, and why not a list

A plan belongs to a package, so the subpanel needs one named. Rendering all of them would be one
request per installed package every two seconds, on a §33.7 listener that serves one request per
connection. The operator picks; the panel fetches once.

### Three things found by opening the page again

* **`print_plan` already existed.** `skynet plan` is the last *reconcile* pass and this is the
  resolved *transport* plan; the console's own HTML has carried a comment since it was written
  saying two different plans share a word. Two functions sharing one name is how that comment stops
  being enough, so the new one is `print_transport_plan`.
* **The fixture server dropped the query.** `/limits` and `/limits?package=openrobot/grasp` are two
  different answers — the body's own ceilings, and those with a package's clamp folded in — and the
  fixture served the first where the console asked for the second, which is precisely the §32.2
  violation the fixture exists to make visible rather than to commit. The query is part of the
  request now, flattened into the filename by a rule written in both languages.
* **The fixture's package declared no source embodiment**, so the panel would have rendered the
  honest-but-uninteresting *"this package commands no joints"* and the shape §32.2 actually asks for
  would have gone unlooked-at. Same reasoning as the check-in fixture in the forty-third session: a
  fixture taken on an empty robot freezes the empty case.

Opened, the panel draws: `remap` by role onto `shoulder`, a ceiling of 80 with its provenance, and
four warnings — unverified embodiment, an unsupported control mode, an undeclared latency, and the
scale.

### Numbers

Rust: **1,322** tests, one new — the route test, which plans a staged policy against a described arm
and asserts both halves: that the `scaled_down` warning survives, and that the clamp reached
`resolve_limits` so the `envelope` guard has a ceiling to compare against. That last assertion is
§6.3's hand-off, checked rather than described.

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets` clean; the golden path counts
48 commands; the mirrors match.

§73's interface row loses its last device-page qualifier on the robot side: all four of §32.2's
panels now draw something measured. What remains is that the transport plan does not cross §17 —
which is where it should stay until a policy runtime exists to be running one.

### Findings, restated

1–28 as before; 29–51 from the sessions above; 52–56 from the forty-first; 57–58 from the
forty-second; 59–61 from the forty-third.

62. **A test that will not run on the machine it is written on is worth re-reading as a design
    question.** The activation this test needed is POSIX-only, and the reflex is `cfg(unix)`. The
    second look found that the route was refusing to answer for a staged package — which §37.1 makes
    an observable state on purpose and §6.2 says is the state the answer is most wanted in. The
    platform limitation was pointing at a real gap, and gating the test would have hidden both.
63. **Two functions with one name is a comment that has stopped working.** The console had a note
    saying two different plans share a word. The note was right and it does not travel: it lives in
    HTML and the collision happened in Rust, where the compiler is what said so.

## 2026-09-03 (forty-fifth) — the five counters that were never waiting on a kernel

§74 gives telemetry eight counters. Five of them have been reported unmeasured since the section was
written, and `skynetd-io/src/telemetry.rs` said why in its own header:

> **five of the eight counters have no source.** `cycles`, `guard_trips`, `clamps` and `estops` need
> a kernel, which this daemon does not host, and `interventions` needs teleoperation.

The first clause is true and the conclusion does not follow.

### They need a kernel to happen. They do not need one to be counted

§13's ring is the record of what the kernel did. `Cycle` carries the guards that tripped and the
channels that were clamped; `Cycle::intervened` has computed the numerator §15 divides by since the
ring was written, and its doc comment says so in as many words — *"the input to `field_conformance`
(§13, §15)"*. §26 puts an estop on the event channel beside them.

And `skynetd` **holds that ring**. It opens it before the first reconcile pass, writes §26's events
into it, seals it on a fault, serves it at `GET /blackbox` and downloads it at `GET /blackbox/{name}`.
The producer and the consumer have been in one process for twenty sessions and nothing had walked
from one to the other. This is finding 33's shape again — a field whose only consumer is a sentence
in a specification — except that here the *producer* was the one nobody had read.

`skynet_blackbox::tally` is the walk. It takes a sequence number, counts every record at or after
it, and returns the five with the three facts that decide whether they may be reported at all.

### `interventions` was wrong for a second reason

It was recorded as needing **teleoperation**. §15 defines field conformance as *"the ratio of cycles
the kernel did not have to intervene in"* — a kernel intervention.

> **Corrected in the forty-eighth session.** This entry originally went on to say that a human taking
> over is §54.4's `intervention_count` on an episode, *"a different number on a different route"*.
> That is not in the specification and both implementations contradict it: §54.4's count is the same
> kernel number over a narrower window, and §54.4's own diagram ends `running` on a guard trip. The
> original sentence was invented here to explain the wrong claim it was correcting, which is a worse
> failure than the one it fixed. See the forty-eighth session.

The comment that was replaced made a counter look impossible, which is the cheapest kind of defect to
introduce and among the more expensive to notice: nothing is broken, a counter is simply absent, and
the reason beside it reads as considered.

### Under-counting is the failure to avoid here, and it is detectable

A ring is circular. A window whose opening sequence has been overwritten still yields a number, and
that number is a **floor presented as a count** — §47.4's *"gap that looks like a quiet period"*, in
the one place §74.1 picked its entire shape to avoid it. A robot logging at 1 kHz into a ring that
holds seventy seconds overwrites a five-minute window four times over.

So `Tally::complete` is not advisory. Truncation, a gap inside the window, a hole in the sequence and
a torn slot each make the counts unreportable, and the window goes out with the five absent — which,
because §74.1 made the windows tile, leaves the Hub with a hole it can name rather than a small number
it believes.

`damaged` is counted over the **whole ring** rather than the window, and it cannot be otherwise: a
torn record's sequence number is part of what was torn, so there is nothing to place it in a window
by. Any damage puts the window in doubt. That is the safe direction.

### The other reason to say nothing, and why it is a property of the ring

A ring created with no action channels is a robot with **no command path**. §26's four cycle channels
have no producer, and neither has the safety path that asserts a stop. Zero cycles there is the
absence of a writer, not a measurement, and a fleet page rendering it would show a green guard column
over a robot that has never commanded a joint.

`Recorder::index` has said this in a sentence since it was written. `Tally::commands` is the
machine-readable half of the same fact, and putting it on the **ring** rather than in a `cfg` or a
constant is what makes this change do nothing today and everything later: the day a control loop
opens a ring with channels in it, the five counters begin reporting and no line here changes.

The two conditions are kept separate — `complete()` and `commands` — because collapsing them would
make a wrapped ring indistinguishable from a robot that never commanded anything, and those send an
operator to opposite ends of the building.

The five also arrive **all five or none**. They come from one walk of one recording, so a window
carrying some of them would describe a ring that was partly readable, which is not a state a ring
has — and would leave a consumer dividing a measured numerator by an absent denominator.

### `GET /telemetry` says which of the three silences it is

Five nulls look identical and mean three different things to go and fix: the disk would not answer,
this robot has no control loop, or the window fell out of the ring. The route now carries the
sentence rather than the reader carrying the guess, and `unmeasured_note` is a function so the
console and the terminal name it the same way. It is `None` when the five are present, because a note
explaining an absence that is not there is text a reader learns to skip.

### Two fixtures for one route, and the argument for the pair

§31.6 freezes the exact document a robot puts on the wire, and there are now two for this one.

`telemetry` is unchanged, byte for byte — which is itself the check that this change breaks nothing:
the shipping robot has a recorder, opens it at zero channels, and produces exactly the window it
produced before. It is now taken through a **real** `Recorder` rather than from a hand-built
`Counting`, so the fixture is the daemon's output rather than a reconstruction of it.

`telemetry-counted` is what a robot that commands a body sends: a real ring at a real shape, cycles
encoded by the ring's own encoder, counted by the real tally, closed by the real `Counting::close`.
The cycles are the only synthetic ingredient, which is the same and only one
`telemetry.replay.deterministic` has.

Freezing only the first would leave the Hub's guard-trip column and §15's field figure exercised
against nulls for as long as no robot commands anything — a column nobody has seen work. Freezing
only the second would freeze a document no robot can currently produce. That is the third time in
three sessions that a fixture taken on an empty robot has turned out to freeze the empty case; the
answer this time is not to move the fixture but to have both.

### `guard_trips` and `interventions` are different numbers

One cycle in which three guards fire is **one** intervention and **three** trips; `clamps` counts
**channels** whose command was altered. Each counter is at the granularity of the thing its name
names. Had they been collapsed, a console would draw one line twice and §15's ratio would be computed
from the wrong numerator — and nothing would look wrong, because both are plausible small integers.

### One re-export, found by needing it

`skynet-blackbox` exposes `CycleView { tripped: &[Guard], response: Option<FaultResponse> }` in its
public API and re-exported neither type. Anything that builds or reads a cycle had to take a direct
dependency on `skynet-kernel` to name them — which would put the `no_std` kernel in the graph of
consumers that have no other reason for it. `skynetd-io` is the first crate that needed to, which is
why nobody had noticed.

### Numbers

Rust: **1,336** tests, 14 new — 7 on the tally in `skynet-blackbox`, 6 on the window in
`skynetd-io/src/telemetry.rs`, 1 the second wire fixture. The suite runs **1,306** of them on this
machine: 4 are `#[ignore]`d and **26 are POSIX-only** and do not compile on Windows, which is where
this was run. That decomposition is worth writing down — it is the first time the gap between the
number in §73 and the number `cargo test` prints has been explained rather than restated, and
1,292 + 4 + 26 = 1,322 reproduces the previous session's figure exactly.

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets` clean; the golden path counts
48 commands; the mirrors match, with `wire/telemetry-counted` added to the mirrored set.

Hub: **1,490** tests, 5 new. `columnsFor` was split out of `recordWindows` so §74.3's boundary — a
measured zero arriving as `0` and an unmeasured counter as `null` — can be exercised without a
database. It could not be before, which is to say it never had been.

§73 loses two claims it should no longer make: the black box row no longer says five counters have no
source, and the interface row no longer says nothing counts a trip. Both gain the honest replacement,
which is that the counting exists and a robot that trips a guard does not.

### What this does not do

**No robot reports a number today**, and the change is written so that this is visible rather than
worked around. Every shipping robot's ring has zero action channels, so every window still goes out
with the five absent — and now says which of three reasons that is. The value banked is that the path
is built, tested against a document produced by the real producer, and gated on a property of the
ring rather than on a sentence somebody has to remember to delete.

§15's `field` suite is therefore **measurable rather than measured**. It has a numerator, a
denominator, a coverage figure and a wire fixture proving they survive the crossing. What it needs is
a robot that runs a cycle, which is the same thing five other rows of §73 are waiting for.

### Findings, restated

1–28 as before; 29–51 from the sessions above; 52–56 from the forty-first; 57–58 from the
forty-second; 59–61 from the forty-third; 62–63 from the forty-fourth.

64. **"X needs Y" is a claim about when X happens, and it gets read as a claim about when X can be
    measured.** Four counters needed a kernel to occur and none of them needed one to be counted,
    because the thing that occurs is written to a ring this process already holds. The sentence was
    true and the conclusion drawn from it was not, and it survived twenty sessions because nothing
    was broken — an absent counter with a reason beside it reads as considered.
65. **A comment that explains why something is impossible is a claim, and it should be checked like
    one.** `telemetry.rs` said `interventions` needed teleoperation. It does not — and the
    correction written here first said the word meant two things in two sections, which was also
    not true and was invented to make the original error make sense. ~~A word that names two facts
    will eventually be written down as the wrong one.~~ *Restated in the forty-eighth session, which
    found the same wrong claim in `skill.rs` and no support for the second meaning anywhere.*
66. **Put the "can this be measured" test on the artifact, not on the edition.** `Tally::commands`
    asks the ring whether it has action channels. A `cfg` flag or a constant naming this edition
    would have been shorter and would need deleting by somebody who remembered it existed.
67. **A number that a suite cannot print is a number worth decomposing once.** §73's test count has
    been 30 higher than `cargo test` on this machine for as long as the machine has been Windows.
    Nobody had subtracted the POSIX-gated tests from it, so every session either copied the figure
    forward or quietly disagreed with it.

## 2026-09-03 (forty-sixth) — the admission gate that ran and checked nothing

The Hub's build plan has carried an item since its nineteenth session, phrased as a mismatch between
two documents: *"`required_extensions` is consumed here by the compatibility engine; the robot
consumes the package manifest's `embodiment.extensions` … nothing compares them."* It was filed as
tidy-up. Going to look at it found something worse than the description.

### They are not two documents

§37.1's store pins the on-disk shape of a staged package, and the sentence has been in `store.rs`
since it was written:

> `manifest.json` at the package root is the `.sky` manifest — **the Hub's `manifest` column**.

`compute_version_digest` depends on it: the manifest is hashed as a value and the files as a list,
so a manifest that also appeared in the file list would be hashed twice and nothing the Hub
published would verify. One document.

So this is one JSON object with **two places a publisher can declare what a package needs**, each
with exactly one consumer: `embodiment.capabilities` — which §8.2 makes the field the entire
admission gate is computed from — and top-level `required_capabilities`, which is the Hub's own flat
spelling for its compatibility engine. Neither side had ever looked at the other's.

### What that produces, which is not a mismatch but a silence

A publisher who fills in the Hub's fields — the ones its schema validates and its forms present —
and leaves `embodiment` empty publishes a package that `admit_package` accepts with **every check
passing and nothing checked**:

* no unmet capability, because the required set is empty;
* no over-request, because `requests` is a subset of nothing only when it is also empty, and a
  publisher using the card fields has no reason to have written `requests` either;
* a clean report, to an operator who reads it as one.

The corpus case says it better than a paragraph: `requirements-the-embodiment-block-does-not-carry`
used to produce `{"admitted":true,"findings":[]}`. It now refuses. That diff is the defect, as data.

`manifest_from_json` is where the silence is manufactured — `value.get("embodiment").cloned()
.unwrap_or_else(Json::obj)` — and the field immediately below it is the counter-example. A
`source_embodiment` block that does not decode is an **error**, with a comment saying why: *"an
action space nobody could parse and an action space nobody declared are opposite facts."* The same
care one field up would have caught this.

### Not fixed by picking a winner

The obvious repair is to make one field authoritative and delete the other, and both halves are
unavailable. `embodiment.capabilities` is normative and cannot move. `required_capabilities` is on
every card the Hub has stored and in every compatibility rule it evaluates, and rewriting published
manifests would rewrite documents whose digests are cited by benchmark results.

So §78 states the rule in terms of the two consumers instead, and the asymmetry is the content:

* **Reading is a union.** The Hub resolves requirements from both. The failure on that side is
  *under*-reporting — saying compatible where a robot refuses — and a union cannot under-report.
* **Publishing refuses one direction.** A card declaring what its own `embodiment` block does not.
* **Admission refuses the same shape**, with `embodiment_undeclared`, the seventeenth rejection
  code. Not redundant with the publish check: that one protects versions published from now on, and
  every version already published carries what it carries. The robot is the party that has to
  decline to install something it cannot check.

A card declaring *less* than `embodiment` is refused by nothing. That package is enforced correctly
and only described incompletely, and refusing it would refuse the manifest whose enforced
declaration is the stricter one.

### The fields exist on the robot to be disagreed with

`PackageManifest` gained `required_capabilities` and `required_extensions`, and **nothing enforces
from them**. They are read for one purpose: so the check can notice they disagree with the block that
is enforced. Reading them as a second source of requirements would have been the shorter fix and the
wrong one — it would make the duplicate spelling load-bearing on both sides, which is how a
duplication stops being removable.

### Verification, including the part that came out wrong first

`cargo test --workspace` — **1,306** passing here, 1,336 counting the 4 ignored and 26 POSIX-only.
The corpus grew to **1,020 cases** across 73 files, three of them new, and the Rust edition
reproduces all of them.

Two things went wrong on the way and are worth recording:

* The first corpus run diverged on **whitespace**. A Rust line-continuation inside a string
  literal, after `cargo fmt` had reflowed it, kept the padding; the reference produced the sentence
  and this edition produced the sentence with twenty spaces in it. That is the corpus doing exactly
  its job on the smallest possible divergence, and it is a good argument for `concat!` over
  continuations in any string two editions have to agree on.
* Regenerating the corpus after adding only the vocabulary entry produced a **one-line diff**, which
  was the signal that no case exercised the new code at all. A closed set that grows without a case
  is a vocabulary entry, not a behaviour.

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets` clean; the golden path counts
48 commands; the mirrors match.

### Findings, restated

1–28 as before; 29–51 from the sessions above; 52–56 from the forty-first; 57–58 from the
forty-second; 59–61 from the forty-third; 62–63 from the forty-fourth; 64–67 from the forty-fifth.

68. **"Two documents disagree" is a much better bug when it turns out to be one document.** The item
    was filed as a consistency check between a card and a package manifest. They are the same JSON
    object, which upgrades the finding from *a publisher could confuse two files* to *the admission
    gate can run and check nothing*, and changes where the fix goes.
69. **A defaulting parse is a decision, and it should be made once per field rather than once per
    parser.** `embodiment` defaults to empty; `source_embodiment` below it refuses to. The second
    has a comment explaining why absence and unparseability are different facts. Nobody applied that
    reasoning upward.
70. **A check that costs a publisher an error message and a check that costs a fleet an unchecked
    install are both worth having.** The publish gate cannot reach what is already published, and
    the install gate cannot tell a publisher before they ship. Neither replaces the other, and
    "we already check that at publish" is how the second one gets argued away.
71. **A rule stated in one section is not read by code written for another.** §55.1 says an
    environment's `required_capabilities` are checked at admission. The Hub's resolver returned an
    empty list for environments, because it was written for models and skills and the sentence
    lived in the section about simulated worlds.

## 2026-09-03 (forty-seventh) — two fields the kernel accepted and nothing could deliver, and the classifier that would have inverted one

§7.6 recorded three of `resolve_limits`' seven profile inputs as having no delivery path. Two of them
were gaps in §11 rather than in the body, which makes them buildable, and this closes both.

`self_collision_margin_m` and `proximity` are on the profile now, beside its four ceilings and
§55.2's cell — so they arrive under the profile's own signature (§38.1) rather than through a second
path, which is the same argument §55.2 makes for the cell travelling with the profile instead of
getting a delivery mechanism of its own.

### The two land differently, and the report says which

`self_collision_margin_m` changes what the machine does. §7.1's fifth guard reads it directly —
`limits.self_collision_margin_m.unwrap_or(0.0)` — so a robot with a margin holds its collision bodies
apart instead of at the moment they touch, and `no_collision_margin` stops being emitted on every
robot alive.

`proximity` changes what the machine *says*. It also needs `available.people_detection`, which is
`false` on every body because §5 declares nine sensing modalities and none of them is a person
detector. So the guard is still unavailable — and it now names **the sensor it has not got** rather
than a profile field that could not be delivered. That is the difference between *"this platform
cannot do it"* and *"this robot has not got one"*, and only the second is a fact about a body.

Both halves are asserted rather than described: one test reads the margin off `GET /limits` and
checks the notice is gone, the other reads the `proximity` row and checks the missing input is no
longer `profile.proximity`.

### The classifier that would have inverted the margin

This is the part worth the session.

§33.4's asymmetry — *anything that reduces what the robot may do applies immediately, anything that
increases it waits* — is computed by `ProfileLimits::narrows`, and every limit it had ever seen was a
**ceiling**, where smaller is stricter. `narrower()` says so in one line.

A collision margin is a **floor**. So is a stop distance, and so is a slow distance. Halving a
collision margin lets two bodies get twice as close, and run through `narrower()` that edit is
classified as a *narrowing* — which means it is **applied under a running policy**, which is the
precise failure §33.4's asymmetry exists to prevent, arriving through the function written to
prevent it.

There is now a `wider()` beside `narrower()`, identical in every case but the comparison, and the
three separation thresholds are classified individually: two floors and one ceiling, because
`slow_factor` is a multiplier where smaller means slower. `shrinking_a_collision_margin_is_a_widening`
was **confirmed to fail** with the classifier swapped back, which is the only way to know a test of
this shape is doing anything.

### All three or none

`skynet_kernel::Proximity` requires all three numbers and a profile may carry fewer. The promotion is
a `?` chain, so a block with two of three stays absent and the guard goes on naming
`profile.proximity` as its missing input. Defaulting the third would have been shorter and would have
been this layer choosing a safety parameter — §7.2's rule is that a guard with a partial input
reports unavailable rather than running on a number nobody chose.

### It crosses §17, and the reason is one sentence

The margin is on the check-in's `GuardReport` and rendered on the device page, in the same table as
the four ceilings. It is the same question an operator is asking — *what is in force* — and a page
showing four numbers while a fifth applied silently would be a page that reads as complete.

The type says the floor thing beside the field, on both sides, because a page that sorted or
coloured these by magnitude would get exactly one of them backwards.

The frozen `wire/checkin` fixture carries it as **present and null**, with `no_collision_margin`
beside it, and that is the right freeze rather than a missed opportunity: it is the shape every robot
sends until somebody writes a margin into a profile, and it exercises the three-state distinction —
absent, null-with-a-reason, a number — which is the thing a consumer gets wrong. The number itself is
exercised cross-edition by the corpus case, which both editions reproduce.

### Numbers

Rust: **1,342** tests, 6 new — 3 on the classifier in `skynetd`, 2 on the delivery in `skynetd-io`,
1 on the decode. **1,312** run on this machine. The corpus is unchanged in size at 1,020 cases and
20 of them changed shape, which is `shape_limits` gaining a field.

`cargo fmt --all --check` and `cargo clippy --workspace --all-targets` clean; the golden path counts
48 commands; the mirrors match. Hub: **1,499** tests, typecheck clean.

### Findings, restated

1–28 as before; 29–51 from the sessions above; 52–56 from the forty-first; 57–58 from the
forty-second; 59–61 from the forty-third; 62–63 from the forty-fourth; 64–67 from the forty-fifth;
68–71 from the forty-sixth.

72. **A classifier written for one kind of number acquires a second kind silently.** `narrows` had
    handled ceilings correctly for twenty sessions. Nothing about adding a floor to the struct it
    reads makes it complain; the compiler is satisfied, the tests pass, and the only symptom is that
    the wrong direction of one specific edit becomes immediate. Adding a field to a struct that
    something classifies is a change to the classifier whether or not it is edited.
73. **The interesting half of "give it a delivery path" is what it does not change.** One of these
    two fields makes a guard hold at a distance; the other makes a guard report a better sentence
    and nothing else. Shipping both and saying which is which is more useful than shipping the first
    and being quiet about the second.

## 2026-09-03 (forty-eighth) — the sweep the forty-fifth session's finding asked for, and the sentence it had to retract

Finding 64 said *"'X needs Y' is a claim about when X happens, and it gets read as a claim about when
X can be measured."* A finding like that is worth a sweep, so this session grepped every *"no
source"*, *"nothing carries"*, *"nowhere to arrive from"* and *"cannot be measured"* in the tree and
read each against what is actually available.

It found one, and it found it in the file next door.

### The same wrong claim, in `skill.rs`

> **`interventions` is absent rather than zero** when nothing on this robot counts them. §54.4 asks
> for the number; this edition has no kernel channel and therefore no source for it.

Word for word the argument the forty-fifth session removed from `telemetry.rs`, in the module that
produces §54.4's episode. The conclusion is wrong for the same reason: the kernel does not have to be
**asked** what it did, because it wrote it down, and this daemon holds the recording.

Everything else was already built and built well. `Invocation::interventions` is an `Option<u32>`,
`Cycle::guards_observable` is the switch that keeps a zero out of the episode, `episode()` writes
`interventions_measured: false` into `metadata` when it has nothing. One producer was missing, and
`runtime.rs` supplied a hard-coded `false` with a comment saying why.

### And the retraction, which is the more important half

The forty-fifth session's correction did not stop at *"`interventions` is a kernel intervention"*. It
went on:

> A human taking over is §54.4's `intervention_count` on an **episode**, which is a different
> number, measured by a different thing, on a different route.

**That is not in the specification.** §54.4 lists *"intervention count"* with no qualifier; the only
sentence anywhere claiming a human-takeover meaning is the one written in the forty-fifth session.
Both implementations contradict it — the Hub's `devices.ts` says *"a robot with no kernel channel
cannot count interventions"* — and §54.4's own diagram ends `running` on a **guard trip**, which is a
kernel intervention by definition.

So a session that corrected an invented explanation invented one of its own, in the same paragraph,
to make the original error make sense. That is worse than the defect it fixed: a wrong comment in one
crate misleads whoever reads that crate, and a wrong sentence in §74.6 is in the document every
edition is measured against. §74.6, the forty-fifth entry above and the Hub's twelfth are all
corrected in place rather than quietly rewritten, because a log that edits away its own mistakes is
not evidence of anything.

### Added, not edge-detected

`Cycle::interventions_since` is an `Option<u64>` and it replaced `guard_tripped: bool` plus
`guards_observable: bool`. The pair is worth a sentence: §7.2 dislikes exactly that shape — a value
and a second value saying whether the first is real — and an `Option` says both and cannot be read
without the question being asked.

The counting is a **sum**, because the ring reports how many cycles the kernel intervened in over an
interval and a reconcile pass is not a control cycle. The rising edge it replaced existed only
because the input was a boolean, and it collapsed a whole pass of trips into one. On the real shape
that matters more than it sounds: §54.4 ends `running` at the first trip, so the interval that
reports interventions is usually the *only* one an invocation sees — and the number it carries is the
whole of the episode's count. Reporting `1` where the ring said `3` would not be a rounding error,
it would be the number.

`guard_tripped` did not disappear; §54.4's transition still asks a yes/no question and
`interventions_since.is_some_and(|n| n > 0)` answers it. `None` reads as `false`, which is the value
the daemon used to supply, so a robot that counts nothing aborts no skill it would not have aborted
before.

### One invocation at a time, and the limit is named rather than hidden

A count off the ring covers an **interval**; an episode wants an **invocation**. With one skill in
`running` they are the same thing. With two, the interval's trips belong to whichever policy was
commanding — the recording knows by digest and nothing reads that yet — so a pass with more than one
invocation in `running` leaves the count unmeasured **for all of them**.

Attributing an interval's trips to every live invocation would have been one line shorter and would
put a number on the wrong skill, which §7.2 rules out: a measurement nobody can attribute is not one.

### What it does not do

**No robot reports a count today**, for the same reason §74.6's five counters report nothing: the
tally is `None` unless the ring can hold a cycle, and no edition writes one. The value is that the
path exists, is tested against the real producer, and is gated on a property of the ring rather than
a constant.

### What the sweep cleared, which is the other half of running one

Ten claims, one defect. The rest hold, and they are recorded so the next sweep does not re-derive
them:

* **`power.rs`'s two unaccounted reserve terms.** `payload` and `distance_to_safe_state` genuinely
  have no source. `Conditions::payload_kg` exists and looks like one — §45.3 lists payload among a
  calibration's conditions — and it is a field an operator *declares on a record*, not something the
  robot senses; `conditions_now` returns `None` for it and says why. `mass_kg` on a frame is the
  geometry's mass, not the carried load. `Reserve::unaccounted` carries the fact and
  `Reserve::complete` is a method rather than a constant so the day one arrives the consumers do not
  have to be found again.
* **`capacity.rs`'s "field that has no source"** is a runtime message about one robot's missing
  reading, not a claim about the platform.
* **`limits.rs`'s three** are the ones the forty-seventh session dealt with; one remains
  (`people_detection`) and it is a gap in §5's vocabulary rather than in a delivery path.
* **`blackbox.rs`'s "nothing carries the bytes"** is §77, which is an owner's decision on upload
  design rather than a missing consumer.
* **`skill.rs`'s** was the defect.

The pattern that distinguishes them is worth stating: a claim about **a body or a document** —
"no payload sensor", "§5 declares no person detector" — held up every time. A claim about **this
process** — "there is no kernel channel" — was wrong twice, because the thing being reached for was
in the same process under a different name.

### Numbers

Rust: **1,346** tests, 4 new, **1,316** run here. `cargo fmt --all --check` and
`cargo clippy --workspace --all-targets` clean; golden path 48 commands; mirrors match.

Two of the four tests were rewritten after failing, and both failures were the code being right:
§54.4 aborts at the first guard trip, so an invocation cannot accumulate trips across passes and the
episode is emitted *at* the trip rather than after the unit exits. The tests as first written assumed
otherwise and asserted a shape the lifecycle does not produce.

### Findings, restated

1–28 as before; 29–51 from the sessions above; 52–56 from the forty-first; 57–58 from the
forty-second; 59–61 from the forty-third; 62–63 from the forty-fourth; 64–67 from the forty-fifth
(65 restated there); 68–71 from the forty-sixth; 72–73 from the forty-seventh.

74. **A correction is a claim and deserves the same evidence as the thing it corrects.** The
    forty-fifth session's fix was right and its explanation was invented. The invention was more
    dangerous than the original error because it landed in the specification rather than in a
    comment, and because it read as the kind of careful distinction this document is full of.
75. **A finding that names a *pattern* is worth grepping for the same day.** Finding 64 was written
    about one file. The identical sentence was two files away, in the module that produces the
    document §76.3 spends a section on protecting, and nobody had looked.
76. **A test that fails because the system is right is the cheapest lesson available.** Both rewrites
    here came from assuming an invocation could accumulate interventions across passes. §54.4 ends
    at the first one, which is a property worth knowing and was not written down anywhere near the
    counter.

## 2026-09-03 (forty-ninth) — the sentence §77.1 left unfinished, and the corpus that was never asked about the other half

§73's table has carried one row in the last column for as long as it has existed: *"the sealed bytes
do not cross §17 (§77)."* §77.1 wrote the reason down honestly and stopped there:

> A sealed window is tens to hundreds of megabytes. §34.3 puts it in the bulk class, which wants
> resume, progress and metering on an **upload** — the mirror of §75's fetch, and unwritten.

That interim was right, and it is the kind of right that expires. A Hub could say *"this incident has
a 4 MB recording on the robot, digest `sha256:…`, that nothing has collected"* — a true sentence,
and one that tells an investigator exactly enough to know what they cannot read. This session wrote
the mirror.

### §79, and the one thing that is not symmetric

The two directions share four rules and they are all properties of §33.2's single thread rather than
of a direction: a slice rather than a transfer, one subject per pass, the bytes checked however they
arrived, and nothing here deciding whether the subject should exist. §75's fetch has a fifth that is
what makes progress free:

> The partial's length is the progress. No counter, no percentage written anywhere.

**An upload cannot have that rule, because the partial is on the other machine.** Everything §79 adds
falls out of that one sentence:

* **There is a probe**, and it is a *decision* in the vocabulary rather than a step hidden in a
  caller. `plan_upload` has eight where `plan_fetch` has seven.
* **The offset is a claim.** It places the next byte and never concludes a transfer; that conclusion
  is the receiver's digest against the one recorded at the seal, before the transfer existed.
* **A failure discards nothing.** `check_bundle` deletes a partial that will never verify.
  `check_upload` can only stop believing an offset and start again at zero — this robot has no
  authority over the receiver's disk, and a contract whose remedy nothing could carry out would be
  a worse contract than one that says so.

Two answers are refused rather than followed, and both refusals cost something to get right. A
receiver claiming **more bytes than the window has** is `plan_fetch`'s discard in the direction where
nothing can be discarded. A probe that **answers without a number** moves nothing at all: pushing
from zero whenever a probe failed would re-send the whole window every pass for as long as the
failure lasted, which is the one behaviour a bulk transfer must not have.

### The tension §34.3 and §34.4 have carried since they were written

> §34.3: *"Bulk transfer respects metering. A sealed black box is tens of megabytes […] Bulk classes
> wait for an unmetered bearer."*
>
> §34.4: *"There is nothing in this queue worth less than an incident."* Cellular carries
> *"check-ins and incidents only"*.

`connectivity.rs` had a section titled **"The uncomfortable part of that rule, which is correct"**,
conceding that an incident meant tens of megabytes over the most expensive link a robot has and
arguing — well — that this was the right trade.

It never was tens of megabytes. `Class::Incident` has only ever carried a **document of a few
hundred bytes**. The two sentences were in tension only while sealing and uploading were one act, and
§77 ended that a session ago without anybody noticing what it had freed. §79.3 states the rule the
split allows:

> **The alarm goes over anything that is up. The window waits for a link nobody is billed for.**

An operator learns within seconds, on cellular, in a field, with the digest of the recording — and
the fleet is not billed by the megabyte for the evidence. The earlier reading gave up the second to
buy a first it was already getting.

### `uploaded` stays false, which is the part worth arguing about

The obvious next move is to flip §77.2's `uploaded` to `true` when the bytes land, and it is wrong.
The incident is queued **at the raise**; the upload happens minutes or a week later, by which time
the document has usually been delivered and has always been sealed into the queue. A robot that went
back and rewrote a queued alarm to flip a flag would be **editing evidence after the fact**.

So the field keeps saying what was true at the raise, like every other field on that document, and
the Hub answers *"do we have the bytes?"* by looking for the object at the digest the incident named.
That is what content addressing was for, and it is why the route is
`PUT /devices/{id}/recordings/{sha256}` rather than something nested under an incident id the robot
never learns — §34.4's queue is fire-and-forget, so a nested route would mean the bytes could not
move until the document had been accepted.

### The corpus had never been asked about `bundle.ts`

Writing the mirror meant writing corpus cases for it, and writing those meant looking at the ones for
the original. There were none.

`reference/typescript/bundle.ts` is 242 lines implementing an eleven-branch decision about **deleting
operating system images from a robot's disk**. It is vendored here as the oracle. `tools/corpus-gen/generate.ts`
had never imported it. Two editions, one of them normative, and nothing holding them to each other —
across twenty-odd sessions, including the one that built the fetch.

The corpus found a divergence in the first run: three sentences where the TypeScript spelled an
apostrophe `’` and the Rust spelled it `'`. Every other `expected` string in 1,020 cases is ASCII;
the 78 typographic apostrophes in the corpus are all in `note` fields, which are prose for a human
and are not compared. So the convention was already unanimous and three shipped strings were outside
it, in the one module nothing was checking.

It is a small defect and it is exactly the kind the corpus exists for: an edition that reproduced
those three sentences byte for byte would have had to reproduce the wrong ones.

`corpus/transfer/` now holds four files — `fetch`, `arrival`, `upload`, `receipt` — and the reason
they are one corpus rather than two is that the point of the pair is that they mirror. A reader
comparing them is looking at the same order-of-cost decision written twice, and the four places they
differ are the four sentences §79 is about.

### Two findings in tooling, and one of them was blocking the work

**`check-mirrors.mjs` could not record what it had just verified.** The specification is mirrored
into the Hub repository and pinned by digest, and the workflow the file itself documents is *"an edit
made here first should be copied there by hand, deliberately."* There was no way to finish that
workflow. Pins were written only by `--sync`, which **copies the other tree's copies over ours** —
so the documented way to complete an edit made here ended in a command that could silently undo one.
It is not hypothetical: the Hub held an unrelated shared document mid-edit and uncommitted, and
`--sync` mirrored 836 uncommitted lines into this repository as the price of pinning a file it had
already reported as matching.

Two changes. `--pin` records agreement and moves nothing. And pinning is now **per document**: a
document the two trees differ on keeps its last agreed digest rather than being re-pinned or
dropped, so the single-checkout check CI runs stays armed for it. The all-or-nothing guard was the
reason the two operations had to be one.

**The console printed the status line's reason phrase where the daemon had written a paragraph.**
`api::error` writes both — `error` is *"Service Unavailable"* and `detail` is the sentence naming the
cause and the remedy — and `Feed.poll` read the first. Every failing panel in this console has said
two words for eight sessions. Same family as the guards panel and the attestation sentence, found
the same way, by looking at what a page actually renders.

### What the window manifest is for, and why it is a cache rather than a record

The collector has to answer *"which sealed window has not been collected"* every reconcile pass, and
the only sound key is the digest — §13 names a window for a monotonic instant, a reboot restarts that
clock, and `File::create` truncates, so a **name is reused and a file at that name is overwritten**.
A receipt believed on its filename alone reports a new window as collected because a previous one
with the same name was.

Hashing to find out is the obvious answer and it is unaffordable: fifty sealed windows is gigabytes
of reading per pass, to re-derive a number the sealing process had in a variable.

So the seal writes it down — which is §77.1's own argument, one step earlier: *the digest is recorded
by the party that produced the bytes, before any transfer exists*. `…-fault.mcap.window.json` sits
beside the recording, is invisible to everything that filters on `.mcap`, and is keyed by the file's
length so a manifest left beside a rewritten name is not believed.

It is still only a hint, and the distinction is where the care went: the manifest picks the
**candidate** and the hash confirms the **one window that is about to move**. Believing a sidecar
about bytes this robot is about to send would upload a window under another window's digest, which
the receiver would store, acknowledge and file against the wrong incident. One hash per pass at the
very most, and none once everything has a receipt.

### One rule that was in the wrong module

`404` from the probe means *the receiver holds nothing under this digest*, which is a **measured
zero** — it was asked and it answered — rather than an absence. That interpretation was written
inside `CurlUploader`, where it is invisible to any other uploader, and the scripted double in the
suite went straight past it: four tests failed on the first run because a receiver answering `404`
looked like a receiver that had said nothing at all.

The tests were right and the code was wrong. It is a property of the **route**, not of the transport,
so it moved into `advance`. An uploader written against a different client would otherwise have had
to rediscover it, and the one that did not would back off for ever on the first upload of every
window.

### What it does not do

**The Hub half is not built.** Nothing serves `/devices/{id}/recordings/{sha256}`, so a robot with a
window to send probes, finds nothing, and backs off on §34.4's curve — which leaves the window and
the incident naming it exactly where they were. §57.1's table now carries thirteen endpoints with the
thirteenth marked as the exception rather than counted in, because a table of routes that all return
200 is the failure that section was written against.

**No robot has run this against a real receiver.** What exists is §12's fetch and §13's upload as the
same shape with the same failure modes, tested against the same kind of recording implementation —
which is what made §75's rollback testable and is not the same as having watched one.

**Retention is not answered.** §79.4 keeps every window, including ones the Hub has acknowledged. A
robot sealing daily for a year does not fit inside §28's disk floor, and deleting on somebody else's
acknowledgement is a decision about evidence rather than about disk — §13 exists precisely for the
day that somebody is the party being asked questions. It is in §80 with two other things Part X
leaves open rather than settled in passing.

### Numbers

Rust: **1,369** tests, 23 new, **1,339** run here. Corpus: **77** files, **1,055** cases, 35 of them
new and 14 of those about code that was written twenty sessions ago. `cargo fmt --all --check` and
`cargo clippy --workspace --all-targets` clean; golden path 48 commands; contract mirrors match; the
specification is mirrored to the Hub and re-pinned.

### Findings, restated

1–28 as before; 29–51 from the sessions above; 52–56 from the forty-first; 57–58 from the
forty-second; 59–61 from the forty-third; 62–63 from the forty-fourth; 64–67 from the forty-fifth;
68–71 from the forty-sixth; 72–73 from the forty-seventh; 74–76 from the forty-eighth.

77. **A module with no corpus cases is not covered, whatever the file says about itself.**
    `bundle.ts` is documented as carefully as anything in this repository and had never been
    compared against the edition that ships. The absence is invisible from either file: nothing in
    a module says how many cases test it, and the generator's import list is the only place the gap
    is legible.
78. **Building a mirror is the cheapest audit of the original.** Every property §79 needed made a
    claim about §75, and one of them was false. The pair is now the artifact — four corpus files in
    one directory — rather than two modules that happen to rhyme.
79. **A rule two sections state differently is a rule waiting for the thing that separates them.**
    §34.3 and §34.4 were both right and could not both be obeyed while sealing and uploading were
    one act. The code split the difference and wrote a comment defending the compromise. §77 removed
    the reason for it a session before anybody noticed.
80. **A tool that cannot express the workflow it documents will be used the other way.** `--sync`
    was the only path to a pin, so pinning meant overwriting — and the safe operation was reachable
    only through the destructive one.
81. **An interpretation that lives in a transport is not part of the protocol.** `404` meaning zero
    was a property of the route sitting inside the curl client. The suite's double found it in one
    run, which is the argument for the double.

### Postscript, added after the commit: the pin was nearly wrong for a reason the pin exists to catch

This session's own tooling wrote several files with CRLF line endings, on a Windows machine, into a
repository whose `.gitattributes` opens with a paragraph explaining why the working tree is LF
everywhere — *"this repository is built around byte-identical artifacts […] on a machine with
`core.autocrlf=true` a checkout rewrites every line ending on the way in."*

Every local check passed. `git add` normalises on the way in, so the **commit** was correct; the
mirror was correct, because both trees had been given the same CRLF bytes; the digest was written
without complaint. What was wrong was the one thing nobody would have looked at: the pin described
a file that no checkout would ever produce, and CI — which checks out LF — would have reported the
specification as edited here without the pin being updated.

`check-mirrors.mjs` now refuses to pin a file holding CRLF, naming `.gitattributes` and
`git add --renormalize`. Refused rather than normalised, for the reason the file already gives about
hand-edited pins: a digest quietly taken over bytes other than the ones on the disk is a pin that has
stopped being a statement about a file.

82. **A check that reads the working tree can be defeated by a tool that writes it.** The mirror
    pin is the most byte-exact thing in this repository and it was one `git add` away from being a
    digest of bytes that exist on one machine. The guard costs four lines and the failure it
    prevents is one that only appears in CI, on a different platform, for a file nobody edited.

## 2026-09-03 (fiftieth) — every rule about the tone, and nothing that made a sound

§73's table has called §49 **Designed** since it was written, and the word was hiding a shape worth
naming. Two thirds of §49.1 were not designed — they were *built, enforced and corpus-verified* —
and the third was missing entirely.

* `hri.warning` is a **reserved** capability token. No grant confers it, `CapabilityReserved` refuses
  the package that asks, and `corpus/vocabulary/closed-sets.json` holds both editions to the
  refusal. *"A package cannot suppress it"* has had teeth for twenty sessions.
* `warning_required(from, to)` decides exactly which transitions demand a tone, and does it well: it
  is phrased over `commands_autonomously` rather than over the literal `"running"`, so *"into
  `RUNNING`"* and *"on resumption after a stop"* are one rule, and `hold → degraded` — a policy
  beginning to command an impaired body — cannot fall through the gap between them. An earlier
  session found and fixed exactly that bug.
* **Nothing made a sound.** `Transition::warning` reached `observe_state`, which dropped it.

Every rule about the control enforced, and the control absent. It is invisible from either half,
because each half is complete and neither mentions the other.

### Declared, like a bearer

Which output is the warning speaker is a fact about the machine, exactly as which interface is the
modem is (§34.3). `/sys` will not say whether an ALSA device is a klaxon on a mast or an HDMI port
nobody plugged in. So the command is declared in `state/audio.json` and `skynetd` runs it.

A robot with no declaration reports that it **cannot warn** rather than falling back to a default
device. That fallback is the worse failure by a distance: it produces a robot that believes it is
warning people while playing a sound into a disconnected port, and every surface agrees with it.

A declaration whose command list is empty is not a declaration either — that is what a half-finished
provisioning script leaves behind.

### The question §49 does not ask

*What does a robot do when a transition demands a tone and there is no channel?*

It cannot refuse. §27's state is **derived from conditions**, not granted by a supervisor: there is
no point at which `skynetd` decides whether `RUNNING` may happen, so there is nothing to withhold.
Inventing one would be a second definition of §27's machine, living in the module that plays sounds.

So the answer is the strongest one available to a process that does not hold the switch — an
incident, which §34.4 never drops, carrying the transition, the failure, and **what this robot
believed its warning channel was**. That last field is the one that separates a broken speaker from a
provisioning script that named the wrong device, and an investigator without it has to go and look.

Refusing to command a body that cannot warn is the right rule for a certified tier, it belongs in the
kernel's interlock where the transition is, and it is in §80 rather than invented here.

### The tone is before the motion, and this daemon cannot make that true

§49.1 puts the tone before autonomous motion. `skynetd` sounds it before committing the new state to
its own fields, which is the strongest ordering available from this side of §33.1's line and is **not
the same guarantee**: on a robot whose kernel owns the transition, this daemon learns about `RUNNING`
by observing it, and by then the body is commanded. A tone that sounds a reconcile period after
motion begins is a record, not a warning. Written down rather than smoothed over, because the
difference is the whole value of the control.

### The path is split out precisely because this edition cannot reach it

§33.1 leaves seven of §27's eight conditions unobserved, so the machine derives `safe` every pass and
`warning_required` is never true. **No robot on this edition ever enters a commanding state**, so the
tone never sounds and the incident is never raised — the same position §74.6's five counters are in.

That is why the failure handling is `Daemon::warn_for_transition` rather than four lines inside
`refresh_operating_state`. A safety control whose only exercise is a state this edition cannot reach
is a safety control that ships untested, and the extraction is what let the suite drive the
consequence: a declared channel that fails, one incident in §34.4's queue, and the channel named
inside it.

### §49.2 was already true, except for the half that was not

> `sensing.audio` is a capability token gated like any other […] the default is denied and the grant
> is explicit and audit-logged.

Two of the three needed nothing. A token nobody granted is not held; a grant is an explicit operator
action through `POST /grants`. The third was **false, and not only for this token**.

**A capability grant left no audit entry and no black box record.** §38.3 asks for both on every
configuration change — *"who, when, which layer, and the previous value"* — and a grant is the
configuration change with the largest safety consequence the platform has: the moment a package
acquires permission to do something to a body.

The shape of it is what makes it worth recording. §54.2's *acknowledgement* of a skill's safety notes
**was** written, and refuses the grant if it cannot be. So this robot recorded that somebody read the
warning, and not that it then handed over the capability the warning was about.

And `EventCode::GrantChanged` had existed since §26 was written, with exactly one producer: the
reconciler, applying a grant that came down in a desired state. The ring recorded every grant a *Hub*
made and none an operator made standing at the robot.

Both are now written, as `capability.grant` and `capability.revoke`, with `previous` set to `held` on
a revocation — for a token that is the whole of the previous value. A request that changed nothing
writes nothing: `already_held` and `not_held` are the replays an operator's shell history produces,
and a log that recorded them would teach its reader to skim.

The order is deliberately the opposite of the acknowledgement's. An unrecorded acknowledgement means
the operator is asked again — a grant that did not happen. An unrecorded grant has *already
happened*: the file is written and the reconciler will act on it, so refusing at that point would
return an error for a permission the robot now holds. Best effort, and loud when it fails.

### Numbers

Rust: **1,380** tests, 11 new, **1,350** run here. `cargo fmt --all --check` and
`cargo clippy --workspace --all-targets` clean; golden path 48 commands; corpus 77 files, 1,055
cases; mirrors match and the specification is re-pinned.

### Findings, restated

1–28 as before; 29–51 from the sessions above; 52–56 from the forty-first; 57–58 from the
forty-second; 59–61 from the forty-third; 62–63 from the forty-fourth; 64–67 from the forty-fifth;
68–71 from the forty-sixth; 72–73 from the forty-seventh; 74–76 from the forty-eighth; 77–82 from
the forty-ninth.

83. **"Designed" can mean "every rule enforced and the thing itself absent."** §73's table had one
    word for §49 and the truth needed three sentences: the token was reserved, the predicate was
    written and corpus-held, and no code could make a noise. A state table with three states cannot
    say that, which is an argument for the last column rather than against the table.
84. **A control that cannot be refused must be loud, and the loudness has to name the control.** The
    incident carries the channel this robot believed it had, because "the warning did not sound" and
    "the warning device is `/dev/nothing`" send different people to different places.
85. **The record of consenting to a risk is not the record of taking it.** §54.2's acknowledgement
    was logged and the grant it gated was not. Auditing the ceremony and not the act is a failure
    mode that looks like diligence.
86. **A code with one producer is a vocabulary entry half-used.** `GrantChanged` existed, was
    emitted by the reconciler, and was therefore a record of what the *Hub* did — so the ring was
    complete about remote changes and silent about local ones, which is exactly backwards for a
    device an operator stands next to.

## 2026-09-03 (fifty-first) — a function whose doc comment names a consumer that does not exist

Two sessions running have found the same shape — a fact a contract produces and nothing on the robot
reads — so this one went looking for it deliberately. The sweep is worth describing because the
obvious version of it is useless: *"which public functions in `skynet-contracts` have no caller in
the robot crates"* returns 49, and almost all of them are helpers called from inside the contracts
crate itself. Excluding intra-crate calls and test modules narrows it to **thirteen**, and of those
most are wire helpers reached through a method reference the regex could not see.

One was real, and it announced itself:

> `explain_omissions` — **What a console shows** about a context it could not fully build.

No console showed it. `advance_one` built §54.2's live context, read `built.context` for the
preconditions, and dropped `unaddressable` and `unreadable` on the floor.

### Why that is not cosmetic

§54.2's rule is that a precondition which cannot be evaluated **refuses the skill, naming the path
that was unresolvable** — never "assume true". The path is the *question*. These two lists are the
answer:

* a joint that reported NaN is **absent** from the context rather than false, and the refusal that
  names it looks identical to one about a value that is genuinely out of range;
* a fact whose own name carries a dot cannot be addressed by any precondition at all, because a dot
  path would read the name as two keys — and nothing can be done about that on the robot, because
  the *body* chose the name.

An operator reading *"refused by: `robot.tool_temperature` > 40"* and nothing else will go and look
at a temperature. The sentence they needed says the sensor is not reporting a finite number.

### Where it goes, and where it deliberately does not

On the **runtime**, refreshed every pass, and on `GET /skills` beside the invocations — not on each
invocation. `live_context` is built per package because `skill.grants` differs between two skills on
one robot; `unaddressable` and `unreadable` come from the robot's own facts and are identical for all
of them, so a copy per invocation would be one list rendered *n* times.

Refreshed rather than accumulated, for the same reason §36's throttle window lives in memory: both
entries are facts about the robot **now**. A sensor that resumes reporting a finite number stops
being unreadable, and a list that persisted would send somebody after a fault that has since cleared.

The key is always present and empty where there is nothing to say. A key that appeared only when
something was wrong would make its absence mean two things — *this edition does not report them* and
*there are none* — which is §7.2's rule about a shape rather than about a value.

The console draws it **above** the invocation list and whether or not anything has been invoked: a
robot whose sensors are not reporting is exactly the robot on which nothing will start, and a panel
that hid the reason behind an empty list would answer *"nothing has been invoked"* to the question
*"why"*.

### The same sweep, one layer down: fields rather than functions

`explain_omissions` was found by asking which *functions* nothing called. The same question about
**fields** — which members of a contract struct are never mentioned in the robot crates — returns 59
of 422, and most are false: a field serialised inside a `to_json` helper is never named by the robot
even though it crosses to the Hub on every window, and a struct the robot does not build at all
(`provenance::RunBundle`, `observation::Frame`) contributes a dozen names that are correctly absent.

Two were real, and both are on `LiveInputs` — the struct that *is* §54.2's precondition context.

**`battery_percent` was never filled.** This is the sharpest one in either sweep, because it is the
example §54.2's own module documentation uses to explain why absence matters:

> A robot with no battery sensor that reported `robot.state.battery_percent = 0` would refuse every
> skill; one that reported `100` would run every skill; one that omits the path refuses only the
> skills that asked, and names the path in the refusal. Only the third is honest, and only the third
> is useful.

The robot reads its own state of charge every pass. `GET /power` prints it, §33.6's health column
renders it as a bar, the check-in carries it to the Hub. The one place it did not arrive was the
context that decides whether a skill may start — so `robot.state.battery_percent > 20`, the most
ordinary precondition anybody would write, was refused as unresolvable on a robot that knew the
answer. The third case in that paragraph was honest and it was not useful, because the field was
absent for the wrong reason.

It is **supplied** rather than read where it is used, for the reason `Pass` states about the same
number: a battery figure taken in a different turn of the loop than the decision it informs is the
one thing that struct exists not to be. `advance_skills` now takes one reading and gives it to both
consumers — §36.1's lease refusal and §54.2's context — because reading the gauge twice would let a
skill be refused a lease for a level its own precondition had just cleared.

**`guards_unavailable` was never filled either**, and the asymmetry with `guards_active` beside it is
worth stating because it looks like an inconsistency and is the opposite. `guards_active` is `None`
and must stay `None`: which guards are *running* is a fact about a kernel, and §33.1 gives this
process no channel to one — an empty list from it would be a claim rather than an observation.
Whether a guard is **unavailable** is a fact about a *body*: §45.5 makes a guard unavailable when its
input is missing, and whether this chassis declares an IMU is written in a descriptor on this disk.
The robot already computes that set twice a pass, for `GET /limits` and for §7.6's block on the
check-in.

### The field the sweep must not touch

`LiveInputs::tier` is empty and stays empty, and finding out why is the argument for reading before
filling. `limits.rs` says it outright:

> `resolve_limits` takes a `Tier`, and **nothing on this robot decides one**: §15 awards a tier […]
> `crate::runtime` already refuses to fill `robot.tier` for that reason.

A sweep that filled every empty field on the struct would have made this robot assert a tier about
itself, which is the one thing §15 exists to prevent. There is a test asserting it stays `None`, so
the next sweep finds a reason rather than a hole.

### One more from the same sweep, smaller and worth the two lines

`admit_package` decided §37.1's transport stage by comparing a string to the literal `"refused"`. The
value genuinely arrives as text — `AdmissionParams` is decoded from a wire document — so it cannot be
an enum on that side. But the thing it is compared *to* can be, and now is:
`TransportVerdict::Refused.as_str()`.

The reference edition was already safe here and by a different mechanism: TypeScript types the field
as the union `'direct' | 'retargeted' | 'degraded' | 'refused'`, so a typo in the comparison is a
compile error. The Rust was the weaker of the two, which is the kind of asymmetry a corpus cannot
catch — both editions produce identical bytes today and one of them would keep producing them after
a rename.

`is_installable` remains uncalled on the robot and is **not** dead code to delete: it is the typed
spelling of the same rule, it exists in all three editions, and the Hub is entitled to it. What was
worth fixing was that the robot's spelling was a literal.

### What it does not do

No robot produces an omission today, for a familiar reason: §33.1 leaves the joint readings
unobserved, so `LiveInputs::joints` is empty on every pass and both lists are empty. The path is
built, tested against the real producer, and gated on what the inputs contain rather than on a
constant.

### Numbers

Rust: **1,387** tests, 7 new, **1,357** run here. `cargo fmt --all --check` and
`cargo clippy --workspace --all-targets` clean; corpus unchanged at 77 files and 1,055 cases; golden
path 48 commands; mirrors match.

### Findings, restated

1–28 as before; 29–51 from the sessions above; 52–56 from the forty-first; 57–58 from the
forty-second; 59–61 from the forty-third; 62–63 from the forty-fourth; 64–67 from the forty-fifth;
68–71 from the forty-sixth; 72–73 from the forty-seventh; 74–76 from the forty-eighth; 77–82 from
the forty-ninth; 83–86 from the fiftieth.

87. **A doc comment that names a consumer is a testable claim.** *"What a console shows"* was
    written about a function no console called, and it stayed true-looking for as long as nobody
    grepped for the caller. It is the cheapest kind of dead code to find and the most misleading to
    leave, because the sentence reads as a description of the system.
88. **"No caller" is three different questions.** No caller in the robot crates, no caller outside
    the module, and no caller anywhere-but-tests give 49, 13 and 1 answers on this tree. Only the
    third is a finding; running the first and stopping there would have produced a list nobody could
    act on.
89. **Two editions can be equally correct and unequally safe.** The TypeScript's union type made a
    comparison unbreakable; the Rust's literal made the same comparison a rename away from silence.
    A corpus compares *output*, so it cannot see this — which is an argument for reading the two
    implementations side by side rather than trusting that identical bytes mean identical strength.
90. **The documentation of a rule is a good place to look for a violation of it.** §54.2's module
    header explains, at length and correctly, why `battery_percent` must be absent when unknown —
    on a struct where it was absent because nobody had filled it in. The paragraph was true and it
    was describing a different robot from the one that shipped.
91. **An empty field is not a gap until you know why it is empty.** Three fields on one struct:
    `battery_percent` was an oversight, `guards_unavailable` was an oversight, and `tier` is a rule
    that another module documents in as many words. A sweep that filled all three would have made
    this robot award itself a safety tier.


## 2026-09-04 (fifty-second) — the other end of §79, and the request nobody had given a size

§79.6 has ended with the same sentence since it was written: **"The Hub half is not built here."**
This session wrote it, and writing it found two things about *this* half that no amount of reading
this half was ever going to find.

That is the shape worth naming first, because it is the third time this repository has hit it. §74
found a producer with no consumer. §77 found a consumer with no producer. This found **a client
asserting things about a route it does not own** — and neither end can see one of those alone,
because from here everything looks correct and from there nothing arrives.

### The first one is not about §79 at all

`crates/skynetd-io/src/connectivity.rs` ran `curl` without `--location`.

§17 gives every route as a path — `/devices/{id}/checkin` and its five siblings — and the Hub this
OS is built against sets `trailingSlash: true`, so it answers `308` to exactly the spelling §17
prints. Reproduced against the running Hub before anything was changed:

```
POST /api/skynet/v1/devices/nobody/checkin      -> 308  .../checkin/
POST /api/skynet/v1/devices/nobody/incidents    -> 308  .../incidents/
POST /api/skynet/v1/devices/nobody/telemetry    -> 308  .../telemetry/
POST /api/skynet/v1/devices/nobody/checkin/     -> 401
```

So **every check-in, every incident, every telemetry window and every episode this edition has ever
produced** was addressed at a redirect, was not followed, and went back onto §34.4's curve as a
delivery that did not happen.

The fix is one flag. The reason it is worth a section is that the argument for it was already
written in this repository, one module over. `crates/skynetd-io/src/bundle.rs` has carried
`--location` since the fetch was written, with a comment saying credentials are dropped on a
cross-host redirect and that this is the behaviour we want. The outbound queue never got the same
line — and the asymmetry between two modules doing the same kind of thing is exactly what finding 89
described two sessions ago, in the other direction:

> Two editions can be equally correct and unequally safe. […] an argument for reading the two
> implementations side by side rather than trusting that identical bytes mean identical strength.

Here it is two *modules* rather than two editions, and the stronger one had the comment.

### The second one is about §79, and it is a rule rather than a bug

**A slice bounded only by time is a request body bounded only by link speed.**

`SLICE_SECONDS` is twenty and it always has been. It is a promise to §33.2's single thread: while a
transfer runs, nothing serves the local API, nothing pings the watchdog and nothing observes the
machine, so a pass has to end. It promises the *receiver* nothing whatsoever.

A robot on a warehouse link offering a hundred-megabyte window inside twenty seconds is offering a
hundred-megabyte request body. There is no receiver anywhere that takes one of those, and the Hub
this specification is built against refuses a body over four and a half megabytes **before any of
its own code runs** — an opaque status, no sentence, and a robot that backs off for ever.

§79.2 already had the right rule for this and had applied it to one field only:

> §75's fetch has a rule that makes progress free: *"the partial's length is the progress."* An
> upload cannot have that rule, because **the partial is on the other machine.**

The size of a request is the same kind of fact for the same kind of reason. It is a property of what
the receiver is deployed on, and a sender cannot derive it from anything it can see. So it is
**answered** rather than guessed — `max_slice_bytes`, on the probe, and on the `404` especially,
because that is the first thing any sender ever sees and a limit stated only on rows that exist is a
limit stated only after it has been exceeded.

Three properties, and the third is what keeps this an addition rather than a change:

* **A sender that does not read it still completes**, one four-mebibyte slice at a time.
  `SLICE_BYTES` is a floor for an unknown receiver, not a preference.
* **A receiver's number is clamped rather than obeyed** — `SLICE_BYTES_CEILING`. It sizes a scratch
  file on a disk §28 reserves for recordings, so a Hub answering ten gigabytes gets sixty-four
  mebibytes.
* **The range is written out rather than delegated.** `--continue-at` sets an offset *and* sends the
  rest of the file. A bounded slice has an end that is not the window's end and there is no curl
  flag for one on an upload, so `curl_push` now extracts `[from, from+len)` and writes
  `Content-Range: bytes <from>-<to>/<total>` itself. That is the header `--continue-at` was
  generating, so the wire is unchanged for a receiver that was already reading it.

### The extraction costs a file, and the alternative costs more

`Layout::upload_slice` — one path, overwritten every pass, deleted after the request whether or not
it worked.

The tempting alternative is `--upload-file -` with the bytes fed to curl's stdin: no disk at all.
It sends a **chunked** body, because a stream has no length, and a receiver then has to buffer an
unknown quantity before it can place a byte — which is the shape §79.2 is written against. A slice
that costs one read and one write of at most four mebibytes is the cheaper of the two.

One path rather than one per window, and that is deliberate too: keying the scratch file to the
digest reads as more careful and is not. Nothing ever resumes from it — the resume offset is the
receiver's (§79.2) — so a slice is worthless the moment its request ends, and a directory of
abandoned ones on the disk §28 preallocated for recordings would cost the black box the space it was
preallocated to have, to hold copies of bytes already on the same disk.

### The push goes where the probe ended

`Answer::effective_url`, off `%{url_effective}`.

`--location` is on both the probe and the push, but the push should never use it: following a
redirect on an upload means re-sending the body. The probe runs on every pass before any bytes move,
so the discovery is a round trip already paid for, and `advance` sends the slice to the address the
probe ended at. The flag stays on the push as the fallback for a Hub that redirects a `PUT` and not
a `GET`, which is a shape nobody has met and which would otherwise be silent.

### What the Hub half turned out to need, stated here because it constrains this half

Two things, and both are now in §79.7 of the specification rather than only in the other
repository's log:

* **The receiver cannot ask whether it has the whole window.** It never learns the true length from
  anything it can check, so it asks whether what it holds hashes to the name it arrived under.
  *Complete* and *verified* are therefore one event over there, which is what makes this side's
  `check_upload` reachable in all four of its outcomes rather than three.
* **404 is an answer.** `advance` has read it as a measured zero since it was written — *"it was
  asked and it answered"* — and that comment has stopped being a claim about a route nobody had
  built. It is now the shape the route has, with `{"bytes": 0}` in the body as well, so an edition
  that reads the number and one that reads the status agree.

### Numbers

Rust: **1,362** tests run here, 5 new, of **1,392** (4 ignored, 26 POSIX-gated).
`cargo fmt --all --check` and `cargo clippy --workspace --all-targets` clean. Corpus unchanged at
77 files and 1,055 cases; regeneration is a no-op. Golden path 48 commands. Contract mirrors match.

The corpus is unchanged **and that is the answer rather than an omission**: everything this session
changed is transport. `plan_upload` and `check_upload` decide what to do and what an answer means,
and neither learned a new input — a slice's size is not a decision either of them makes, and
`SLICE_SECONDS` has never been in the contract either. An edition that reproduces the corpus
byte-identically and sends unbounded request bodies is still a conforming edition, which is a real
limit of what a corpus can hold and is worth writing down rather than fixing by moving transport
into it.

Hub side, in the other repository: 1,564 -> **1,594** tests, `tsc` clean, one migration written and
**not applied**.

### Findings, restated

1-28 as before; 29-51 from the sessions above; 52-56 from the forty-first; 57-58 from the
forty-second; 59-61 from the forty-third; 62-63 from the forty-fourth; 64-67 from the forty-fifth;
68-71 from the forty-sixth; 72-73 from the forty-seventh; 74-76 from the forty-eighth; 77-82 from
the forty-ninth; 83-86 from the fiftieth; 87-91 from the fifty-first.

92. **A client makes assertions about a route, and nothing on either end can check them.** The
    outbound queue asserted that §17's paths are the addresses. They are the *names*; a Hub
    canonicalises them. Both repositories' suites passed for twenty-five sessions either side of
    that, because a test double answers the URL it is handed.
93. **A bound that protects one party protects nobody else.** `SLICE_SECONDS` is a promise to a
    supervisor and was read for two sessions as though it were a promise to a receiver. The question
    that separates them is *whose failure does this prevent* — and a limit with only one answer to
    that needs a second limit beside it, not a larger value.
94. **The first refusal is the only one worth stating a limit on.** `max_slice_bytes` goes on the
    `404` because that is what a sender sees before it has sent anything. A limit published only
    where a record already exists is published only to senders that have already exceeded it.
95. **A corpus compares decisions, so it cannot see transport.** Every change in this session is
    invisible to 1,055 cases, and two of them were the difference between a robot that delivers and
    one that does not. The corpus is right to be scoped this way and the conclusion is that
    conformance is not deliverability — which nothing in §15 currently says.
96. **Building the other end is a different act from reading it.** §79 was specified, reviewed,
    implemented and tested on this side, and the two defects above survived all of it. They took
    about an hour to find once something was listening on the other end of a socket.


## 2026-09-04 (fifty-third) — a decision on this side read as a limit for two sessions

No Rust changed in this one. It is here because a paragraph in `runtime.rs` had been read as a
statement about what an incident *can* be traced to, and it was a statement about what the incident
path *should do* — and the two are not the same, which took building the other end to notice.

The paragraph:

> §10's attestation needs a trust store, a release manifest and a descriptor read, and an incident
> path that did all three would be doing work on the one code path §34.4 says is never dropped.

Correct, and it stays. What was wrong was the sentence that grew up beside it in the other
repository's outstanding list — *"an incident names its generation, not its policy"* — filed as a
gap on this side for eleven sessions.

**A generation identifies the code, because §11's document lists the packages.** Id and version
digest, per package, in a document the Hub itself wrote. The trace was one join away and the missing
link was on the Hub: it kept only the *current* desired state, on a row whose comment says it is not
a history table. That comment is right about check-ins — §34.4 collapses them here for exactly the
reason it gives — and it had been extended to **instructions**, which are the opposite kind of
document. §11 gives an instruction a monotonic counter so that other documents can name one; a
counter nobody keeps the documents for names nothing.

So `skynet_incidents.generation` pointed at bytes that had been overwritten, and this side's
decision looked like the cause.

### §77.4, written

The specification gained a subsection rather than the fix living only in a Hub session log, because
two of the rules are about how a **robot's** document is read and belong where the document is
defined:

* the version digest is the join and the package id is a cross-check — a digest survives a rename, a
  transfer and a fork, and a name plus a version string can match two repositories;
* a trace says what the robot was **told** to run, never what it was measured running, and §10's
  three digests are the second question. `context_at` already says whether even the generation is a
  fact this robot stated or an upper bound the Hub filled in at receipt.

### What it does not close

*Which code was running.* The honest shape of that answer is a history of attestations beside the
history of instructions — not a third digest on §34.4's never-dropped path, which would undo the
decision this session started by agreeing with.

### Numbers

Unchanged: **1,362** tests run here, `cargo fmt --all --check` and `cargo clippy` clean, corpus 77
files and 1,055 cases with regeneration a no-op, golden path 48 commands, mirrors match. Hub side:
1,604 tests, two migrations written and neither applied.

### Findings, restated

1-91 as before; 92-96 from the fifty-second.

97. **A decision recorded in one repository becomes a limit in the other.** *"An incident names its
    generation, not its policy"* was true, was a good decision, and sat in a list of gaps for eleven
    sessions because the list was kept where the decision was not. A design note travels; the
    reasoning behind it does not, unless somebody carries it.
98. **The comment that was right about one document was applied to two.** *"Not a history table"* is
    correct about a check-in and wrong about an instruction, and the two live in one row. A sentence
    scoped to a column and stored beside a table is a sentence that will be read about the table.

---

## 2026-09-04 (fifty-fourth) — the band in §3's diagram that the repository did not have

§3 draws a stack with the Safety Kernel as a band across the middle, and §25 draws the command path
through it. Both pictures were accurate about every part except the crank. `skynet-kernel` is the
eight guards and their arithmetic; `skynet-ring` is the channel; `skynet-ring-shm` is the `memfd`
under it; `skynet-rhal` is the wire; `skynet-blackbox` is the record. **Nothing turned them.** No
process in this workspace opened a driver, read the ring, asked the kernel and wrote a joint.

That absence is what §73's last column has been describing from eight different directions: five of
§74's counters unmeasured because §13's ring has no action channels, seven of §27's eight conditions
unobserved so no robot enters a commanding state, §49's tone never sounded, `field_conformance`
measurable rather than measured, no guard ever tripped on a machine. One missing process, eight
rows.

`crates/skynet-kernel-host` is that process, and `skynetk` is its binary.

### The loop, and the three rules that shape it

Read the body, take whatever the policy posted, ask the kernel, write what it allows, record what
happened — once per control period.

* **It never blocks on the producer** (§25). `Fresh`, `Empty` and `Stale` are three answers and none
  of them is a reason to wait.
* **A cycle always produces a record**, including the ones where nothing was posted, the driver
  refused the command, or the state read failed. §15's `field_conformance` is a ratio over cycles,
  and a loop that recorded only the interesting ones would compute it over a flattering denominator.
* **`last_sequence` is nearly all the loop remembers.** The kernel keeps nothing between cycles, so
  the little that must be kept — the sequence the watchdog compares against, what was actually sent
  for the rate limiter, where a ramp has got to — lives in the host and is passed back in. That is
  what lets the kernel stay `no_std` with no allocator.

### Three translations, and each one is a place this could have been quietly wrong

**A hold is a mask of zeros, never a command of zeros.** `CycleVerdict::command` is empty under a
hold, and the RHAL wire has no way to say *nothing* — it has values and a mask, and §24 defines a
zero mask entry as *hold this joint at its current setpoint*. Sending zeros with a mask of ones
instead commands the zero pose: on a position-controlled arm every joint slams to its origin, and on
a torque-controlled one it is not holding but going limp. §24 spells that difference out and this is
the code where it is honoured or not. Not writing at all is a third thing and also wrong — the
driver's own `on_command_loss` timeout would decide the response instead of the guard, at a time
nobody chose.

**The deceleration bound is the acceleration bound.** §7.1's Cat 2 is *"controlled ramp to zero
velocity"* and Cat 1 is that ramp then power removal. A ramp needs a rate and **no deceleration
figure exists anywhere** — not in the envelope, not in `ResolvedLimits`, not in the descriptor. It
does not have to be invented, because a declared number already means it: `max_delta_per_cycle` is
the jerk ceiling the rate-limit guard enforces on the way *up*, and ramping down at the same rate is
inside what the machine is declared to withstand, by the same declaration. Not a new number, and not
a guess — the existing one read in the other direction.

**An escalation is recorded as an escalation.** With no jerk ceiling there is no rate at all, so the
ramp cannot be performed and the response escalates to Cat 0. That is the wrong trade for a humanoid
holding a crate, and doing it *silently* is worse: an operator reading the black box would conclude
the guard demanded the harder stop. `Decision::escalated` carries what was demanded, what was
performed, and the missing input that is the reason.

### Where the unsafe went

`skynet-cli/src/dl.rs` was the loader and had one caller. It is now `crates/skynet-dl`, with the safe
RHAL wrapper — open, read state, write command, emergency stop, describe, close — beside it. The
host crate keeps `#![forbid(unsafe_code)]`, which it could not have done with its own `unsafe`
module: a crate that declares that and then makes an exception inside itself is making the claim and
breaking it in the same file.

### Run on a real driver

`drivers/mock` built as a shared object and driven at 200 Hz:

```
skynetk run --driver drivers/mock/mock_robot.dll --tier host \
            --velocity-max 2.0 --jerk 0.1 --watchdog-ms 50 --cycles 40 --hz 200
```

Tier Host, enforcement Enforcing. Four guards active. Four unavailable and each naming its missing
input — `Workspace` needs `profile.workspace` and `embodiment.frames`, `SelfCollision` needs
`embodiment.frames`, `Balance` is not applicable to a body with no legs, `Proximity` needs
`profile.proximity`. Forty cycles, zero read failures, zero write failures, **zero overruns**. That
is §7.2 on a machine rather than in a test.

### One bug this found in its own first run

`--jerk` was accepted and wired nowhere. The output said so without being asked: `unavailable
RateLimit — needs profile.max_delta_per_cycle`, on a run where `--jerk 0.1` had been passed. The
jerk ceiling is the **operator's**, read off `OperatorProfile`, and it had been put on the transport
clamp, which has no such field. The guard reporting what it lacked is what caught a flag that did
nothing.

### What this does not do

**It is not the policy runtime.** Nothing here loads a model, assembles an observation frame from
real sensors, or produces a desired command. The host is the ring's *consumer*; the producer is
still unwritten, so the ring is empty on every cycle and the kernel holds on every cycle. That is
not a degenerate case being tolerated — it is exactly §33.1's property, *"a robot with no `skynetd`
at all is a robot that holds position safely"*, and it can now be watched instead of only argued
about.

**It does not make a robot host-tier.** `Tier::enforcement` still derives enforcement and no caller
passes one in. `--tier host` says what to resolve limits at; whether the clamp is real is a property
of the scheduler underneath, and `overruns` is the one number this loop can honestly contribute to
that question.

**The ring is private to the process.** §25 has the runtime create the ring and hand the fd to the
sandbox; there is no sandbox, and `skynet-ring-shm` is Linux-only. The binary allocates the ring in
its own address space and prints that on every run rather than a path nothing can open.

### Made permanent, because a run that happened once did not happen

`crates/skynet-kernel-host/tests/loop.rs` compiles `drivers/mock` and drives `skynetk` against it:
the loop turns, host tier resolves to enforcing, four guards name the inputs they lack, forty
cycles with no read or write failure and every one of them idle. `#[ignore]`d because it needs a C
compiler, following `skynet-cli`'s scaffold test, and run explicitly by the `abi` job — a silent
skip is the failure §15 spends a rule on, so it panics with the compilers it tried.

Two of the three cases failed on their first run with `LoadLibraryW` error 32. Cargo runs
integration tests in parallel and all three were compiling the mock to one path, so one test was
writing the object another had mapped. Each builds into a directory of its own now. On Linux the
same race is worse than a failure: the loader maps a half-written object.

### Numbers

`cargo test --workspace` — **1,377 passed**, up from 1,362: fifteen new, plus three
`#[ignore]`d that the `abi` job runs. `cargo fmt --all --check`
and `cargo clippy --workspace --all-targets` clean. Corpus 77 files and 1,055 cases, golden path 48
commands, mirrors match and re-pinned after the §73 edit. The `abi` CI job gained a step so the
200 Hz run repeats on every push.

### One thing found on the way, unrelated to the loop

`main` was not `cargo fmt --all --check` clean. Two files from the fifty-second session's §79
upload work — `skynetd-io/src/upload.rs` and its tests — carried lines rustfmt reflows, and CI runs
that check with `RUSTFLAGS: -D warnings`, so the `rust` job would have failed on the branch. Fixed
by the `cargo fmt --all` this session ran for its own code; recorded because the session that
introduced it reported the check as clean.

### Findings, restated

1-98 as before.

99. **One missing process was eight rows of a status table.** Every entry in §73's last column that
    said some version of *nothing has run a cycle* was downstream of a single absent binary, and the
    table gave no way to see that they were one item rather than eight. A per-row "what is missing"
    column is honest and it does not show a shared cause.
100. **A guard that reports what it lacks catches a flag that does nothing.** `--jerk` was parsed,
     stored, and never reached `resolve_limits`. No test failed. What said so was §7.2's own output,
     on the first real run, in the form of a guard naming an input that had just been supplied on
     the command line.

101. **A repository can be one commit away from a red CI job and not know it.** `cargo fmt
     --all --check` fails on `main`, on two files whose own session log reports the check as clean.
     Nothing runs it locally; the job that would have caught it runs on push, and the push had not
     happened.

---

## 2026-09-04 (fifty-fifth) — the skip reason that named a path, and the path that now exists

`package.grants.gate` has been skipped for eleven sessions with a reason that was true when it was
written:

> half of this is built and it is not the half the test measures. Admission refuses a package
> requesting a capability the operator did not grant, at install (§8). The test is about a package
> that *commands* one at run time and the `mode_gate` guard refusing it — **which is the Safety
> Kernel's, through a path §33.1 denies this daemon.**

The last clause names a path. The fifty-fourth session built it. So the test was re-read rather than
re-skipped, and it turns out to be measurable — with a limit that is more interesting than the
measurement.

### What it measures

The reach `replay_deterministic` has, and for the same reason. Real: this robot's descriptor, its
channel names, `crate::limits`' resolved set out of its envelope and signed profile, the grants an
operator actually made on this machine, and the same `evaluate_cycle` the loop calls. Synthetic: the
three commands, because no policy has run here.

Three, not two, and the third is §7.2's:

| Command | Expected |
|---|---|
| exercising a capability the operator **granted** | admitted |
| exercising one **nobody** granted | refused, and sends no channel |
| naming **no** capability | neither — `None` is *the runtime did not say*, which is not *denied* |

A robot with no grants at all is **skipped rather than half-measured**: only the refusing half could
be shown, and a check that only ever refuses would pass on a kernel whose mode gate said no
unconditionally, which is the failure it exists to catch.

### And on every robot that exists, it skips — for a better reason

`ROBOT_TIER` is guest, guest derives `Enforcement::Advisory`, and §7.4 is explicit about what that
means: `evaluate_cycle` forwards the **desired** command whatever the guards said, so a guest-tier
deployment cannot look enforced in its own logs. The gate trips and the command goes out.

So a guest robot has not passed a test about a gate that refuses, and reporting one would be turning
guest observation into a safety claim — the one thing §42 forbids. It skips, and the reason it gives
is now a property this robot has rather than a component it lacks.

### The bug this found in yesterday's code

Writing the expectation *"a refused command sends no channel"* failed against the kernel, and the
kernel was right.

`skynet-kernel-host`'s `decide` zeroed the mask on **every** `Hold`, advisory ones included. A
guest-tier robot would have withheld a command the kernel had said to forward, while the record
beside it said `advisory: true`. That is §7.4 and §42's failure exactly, it was written yesterday,
and no test in that crate could have caught it: every fixture there resolves at host tier, because
that is the tier the interesting decisions live at.

`decide` now returns immediately on an advisory verdict, before anything that withholds, cuts power
or starts a ramp — and still records the trips and the demanded response, because observing and
reporting is the whole of what the guest tier does. Two regression tests in
`skynet-kernel-host/src/tests.rs` cover the hold and the stop.

### Numbers

`cargo test --workspace` — **1,385 passed**. `cargo fmt --all --check` and `cargo clippy --workspace
--all-targets` clean. Golden path 48 commands, mirrors re-pinned after the §73 edit.

### Findings, restated

1-101 as before.

102. **A skip reason that names a missing component expires when the component is built, and nothing
     re-reads it.** *"Through a path §33.1 denies this daemon"* was correct and became false, and it
     would have stayed in the suite indefinitely because a skipped test does not fail. The eight
     runtime tests each carry a sentence like that, and the sentences are the only thing holding
     them shut.
103. **A crate whose fixtures all use one tier cannot find a tier bug.** Every fixture in
     `skynet-kernel-host` resolves at host, because host is where clamps and ramps happen. The
     advisory path had no test and had the bug, and what found it was a suite in another crate whose
     expectations are written against the kernel rather than against the code under test.

---

## 2026-09-04 (fifty-sixth) — §13 gets cycles, and the sequence that lives in memory

The loop turns and throws its records away. `crate::cycle::record` builds a `Cycle` every pass and
nothing has ever written one, which is the last step between §25's loop and §13's ring.

`crates/skynet-kernel-host/src/recorder.rs` writes them. `skynetk --blackbox <dir>` preallocates the
ring, appends a record per cycle, and seals the window on exit — **the first recording of control
cycles this repository has produced.** Thirty cycles against the mock robot at 200 Hz seal into a
48 KB MCAP.

### The question that could not arise until now

`skynetd` has been the only writer of the ring since it was built. There are two now: the daemon
records events and gaps — a configuration change, a grant, a dropped frame — and the host records
cycles. Both are §26 records and both belong in one recording.

They cannot both write one file, and the reason is small and absolute: **a `Ring` holds its next
sequence number in memory.** Two of them over one file hand out the same sequence twice, encode two
records into one slot, and produce a recording that is intact by every check a reader applies and
wrong in the interval that matters. Nothing detects it until an investigator is reading the file to
answer a question, months later, and the answer is quietly false.

So the rule is written down rather than assumed, as §26.2: **a ring has one writer, and the writer
takes an exclusive lock on the directory.** A second is refused by name — the refusal says who holds
it — and releasing the first lets a second in, so it is a lock rather than a permanent refusal. Three
alternatives are recorded as rejected in the section: two rings joined by timestamp, the host sending
cycles to the daemon over IPC, and the daemon giving up events. The fix is the sequence as an atomic
in a mapped header claimed by fetch-add, which `skynet-ring-shm` already does for §25's command ring;
it is in §80 rather than half-built here.

### What the format refused, and the fix that was not the obvious one

The first run recorded nothing and said why:

```
Shape { field: "desired", expected: 3, found: 0 }
```

Every per-channel array in a record is the declared width, because a reader indexes them by channel,
and an idle cycle was writing an empty `desired`. Padding with **zero** would have satisfied the
check and been wrong: a recorded `desired` of 0.0 says the policy asked for the zero position, and on
an idle cycle nothing asked for anything. §74.3's rule that zero and unmeasured are different facts
applies to a channel exactly as it applies to a counter — so the fill is `NaN`, which is what
`nearest_person_m` in the same record has always been.

The same rule now covers a masked channel: a policy commanding the shoulder and saying nothing about
the elbow records `0.4` and `NaN`, not `0.4` and `0.0`.

### Numbers

`cargo test --workspace` — **1,388 passed**, three more than the last session: the ring round trip,
the seal, and the writer lock. `--ignored` 3 passed. fmt and clippy clean, mirrors re-pinned after
§26.2 and §80.

### Findings, restated

1-103 as before.

104. **A shared file is a design decision that only appears when the second writer does.** One
     process wrote the ring for fifty-three sessions and the sequence living in memory was
     invisible — correct, even — for all of them. The second writer did not reveal a bug; it
     revealed that a property nobody had needed to state was load-bearing.
105. **A width check caught a semantic error, and the obvious fix would have passed it.** The format
     refused an empty array and would have accepted an array of zeros. Zeros are a lie about what a
     policy asked for, and the difference between the two fixes is invisible to every check in the
     repository except the one sentence in §74.3.

---

## 2026-09-04 (fifty-seventh) — the day the sentence in `Tally::commands` was about

`skynet_blackbox::tally::Tally::commands` has carried this since it was written:

> the day a control loop opens a ring with channels in it, these counters begin reporting with no
> further change here.

`skynetk blackbox <dir>` is the reader that shows it. Not a client — `skynet blackbox` in the
workstation CLI asks `skynetd` over its socket and gets the **daemon's** ring, which holds events and
no cycles, and §26.2 is why there are two. This opens a ring file directly, so a window a control
loop just recorded can be read on the machine that recorded it with nothing else running.

Two hundred cycles against the mock robot at 200 Hz:

```
shape      3 joints, 3 channels — 262128 slots of 256 bytes
written    200 record(s)

§74's counters, off this ring:
  cycles          200
  interventions   0
  guard trips     0
  clamps          0
  estops          0
```

Five counters that have reported *unmeasured* for the life of this repository, reporting.

### The number it refuses to print

The first version printed `field conformance 1.0000 (200 of 200 cycles uninterrupted)` and that
line was the problem, not the achievement.

Nothing commanded that robot. §25's ring was empty on all two hundred cycles, so the kernel held on
all two hundred, so it intervened in none — and §15's ratio comes out at a **perfect score** by
arithmetic. On any page that rendered it, that is a robot which did everything right. It did nothing
at all.

So the reader counts how many cycles carried a command — `command_seq` is in every record, and zero
means the ring was empty — and over a window where none did, it says so and prints no figure. The
distinction is read off the records rather than assumed from the edition, which means it stops being
true on its own the day a policy runs.

### And the branch that is not refused

`a_commanded_window_counts_its_own_trips` publishes a velocity ten times the channel ceiling on
twenty consecutive cycles. The envelope guard clamps every one, and the tally reports 20 cycles, 20
interventions, ≥20 guard trips and ≥20 clamps — counted off the records rather than off anything the
test kept. Field conformance over that window is **zero**, which is the correct answer for a policy
that spent it outside the envelope.

That is §15's figure with a numerator and a denominator that both came from a machine, for the first
time.

### Numbers

`cargo test --workspace` — **1,389 passed**. fmt and clippy clean, golden path 48 commands, mirrors
re-pinned after the §73 edit.

### Findings, restated

1-105 as before.

106. **A ratio over an empty denominator's cousin: a perfect score nobody earned.** Field conformance
     is intervention-free cycles over total cycles, and a robot that was never asked to do anything
     scores 1.0000 on it. Every part of that arithmetic is correct and the number is meaningless, and
     nothing in the formula says so — the guard has to come from a fact beside it, which is whether
     anything was commanded at all.

---

## 2026-09-04 (fifty-eighth) — the rule that was stated and half-enforced

§26.2 was written in the fifty-sixth session and says both writers take the lock. One did.
`skynet-kernel-host` had a `WriterLock` of its own and `skynetd`'s recorder had none, so the rule
held on the side that had just been written and not on the side it was written about — which is a
rule that protects nothing, because the daemon is the writer that has been there all along.

`crates/skynet-lock` is the one implementation, and the justification is the same one `skynet-dl`
has: two callers, and neither may depend on the other. The daemon must not link the kernel host
(§33.1 keeps it off that path) and the host must not link the daemon — it would put an HTTP server on
a real-time process. `skynet-blackbox` would be the natural home and cannot be: it is `#![no_std]`,
argued as such in its own header, and a lock is a file.

### The two callers refuse differently, and that is the design

`skynetd` starts **without a recorder** and says so. The path already existed and already had the
right comment on it — *"Reported, not fatal. A robot that cannot open one at all still needs to serve
`POST /stop`"* — which is §33.1: a supervisor that cannot record is not a robot that cannot stop.

`skynetk` refuses to run. A control loop whose cycles go nowhere is the one case where the recording
is the point.

### A stale lock is named, not stolen

A process killed with `SIGKILL` leaves its lock. Clearing it automatically would mean a recorder that
takes over a directory another process is still writing, on the day the first one was slow rather
than dead — so the refusal names the holder and says the file is safe to delete, which is the
sentence that keeps this from being a robot nobody can restart.

### Numbers

`cargo test --workspace` — **1,392 passed**, three more than the last session, all in the new crate.
fmt and clippy clean, golden path 48 commands.

### Findings, restated

1-106 as before.

107. **A rule written down while building one side of it is enforced on that side.** §26.2 was
     correct, complete and half-applied within one session of being written, and the half it did not
     reach was the one that had been writing the ring for fifty-three sessions. Writing a rule down
     is not the same as applying it, and the gap is invisible because the new code obeys it.

---

## 2026-09-06 (fifty-ninth) — a robot answers a browse, and three things only a running machine said

§82 shipped one half in the previous session: the wire format, what a robot may advertise, and a
browse. **Nothing advertised**, so a browse on a real network found nothing — a true report and an
empty one. `skynet_mdns::responder` is what to answer and when; `skynetd_io::discovery` is the
socket under it; `skynetd --lan <addr> --advertise <label>` starts it; and `skynet discover` is the
49th CLI command, which the browse had been waiting for since it was written.

The split is §82.4's own and it earned itself immediately: the responder is a pure function of bytes
and a clock reading, so probing, defending, the conflict rule and RFC 6762 §6's answer delay are all
reachable from a test with no network. The socket half is sixty lines and a `Link` trait.

### The rule that is not RFC 6762's

The RFC renames on a name conflict. This refuses. An operator reads a label off a list and opens the
console it points at, and that console's largest control is a stop — so two robots answering to
`arm-01` is a stop pressed on the wrong machine, and `arm-01 (2)` is a label nobody assigned to
anything on the bench. The second robot refuses, names the address of the first and states the fix.
It is `skynet-lock`'s answer to a second writer, one subsystem over.

### Two daemons on one link, and two defects that had passed every test

Both were in this session's own code, both passed the crate's unit tests, and neither is reachable
by a single responder. Two containers on one bridge network found them in one run.

**The incumbent yielded its name to whoever booted last.** Treating every claim as a conflict means
the newcomer probes, the incumbent falls silent, and the newcomer takes the name — so the browse
pointed at the robot that arrived second. Backwards, and it was the exact hazard the design says it
exists to prevent. An established responder defends; only one still probing refuses.

**The three probes went out together.** They were queued 250 ms apart and drained when overdue, and
`skynetd` preallocates a 67 MB ring before its first poll — so the whole schedule was always in the
past and the name was claimed in one instant. The 250 ms is not pacing, it is the window in which
somebody else gets to object, and a robot that closes it wins every conflict against a robot already
there. Probes are paced from the last one that *went*, one per pass, and the claim waits an interval
after the last of them.

### And a third: the responder cannot live in the reconcile loop

At `--interval 1000` a robot took **six seconds** to take its own name rather than 750 ms, because
the poll cadence is bounded by how long a pass takes and a pass stages packages, drains a queue and
uploads a window. A browse waits two seconds. So the responder has a thread — the daemon's only one
— and the §33.2 argument it appears to break is about a scheduler in front of a reconcile and an
accept, which this is not: it owns its sockets and its state and shares nothing.

### The seal, which had been broken since §26.2 landed

`crates/skynetd-io/e2e.sh` had been aborting mid-way on a non-zero `skynet blackbox seal`. The
refusal was §26.2's: *the creator is the one that seals it*. **`skynetd` is stopped by a signal and
installs no handler**, so its writer lock is left behind on every restart, the next daemon attaches
rather than creates, and an attacher may not seal. Every robot lost the ability to close a window
from its second boot onward, silently, with the recording still running.

The fifty-eighth session's rule — a stale lock is named, not stolen — is right about the hazard and
wrong about the frequency: a stale lock is not the exception here, it is the normal state. So: **a
lock is never taken from a process that exists**, and existence is the kernel's answer. The note
carries the holder's pid *and its start time* (field 22 of `/proc/<pid>/stat`), because a pid alone
is not an identity — reused within hours, reset at a reboot. A first attempt compared `comm` with
the label in the note and was wrong in the dangerous direction: a live holder read as gone whenever
a caller's label was not the executable's name. Six tests said so by turning red on Linux and
passing on Windows.

A reclaim is reported rather than silent, up through `Writership::reclaimed_from` to a line in the
journal.

### The grant the reconciler took back, and the row nobody wrote

With the seal fixed the script reached its end for the first time, and one older failure became
visible: `grants add` answers `granted`, and `grants remove` a moment later answers `not_held`. The
reconciler had converged the operator's grant away — §33.3 working as specified — and had left **no
audit row**. A grant made at the robot was permanent evidence; the taking of it went to the ring
alone, which holds minutes. `audit_grant_changes` records both, attributed to *"desired state
(§11)"* rather than to a person.

What is left open is a decision rather than a defect: the console offers a button that says
`granted` about a permission that will not survive the next pass.

### Numbers

`cargo test --workspace` — **1,557 passed**, 0 failed. `cargo fmt --check` and `cargo clippy
--workspace --all-targets` clean, from a tree that had ten formatting diffs and four warnings at the
start of the session. `e2e.sh` — **all checks passed**, first complete run. Corpus 1,116 of 1,116 in
both editions. Golden path 49 commands.

### Findings, restated

1-107 as before.

108. **A protocol whose subject is two machines cannot be finished by a test with one.** Two
     defects, both in code that passed every unit test in its crate, both found in the first run
     with a second daemon on the link — and one of them had the responder implementing the exact
     inverse of the rule its own header states.
109. **A schedule drained by "whatever is overdue" is not a schedule.** Three probes 250 ms apart
     became three probes at once the first time the caller was late, and the caller is always late:
     the daemon preallocates a black box before it polls anything. The interval was doing safety
     work, and the queue silently deleted it.
110. **A rule can be right about the hazard and wrong about the frequency.** "A stale lock is named,
     not stolen" was written for the day a process was slow rather than dead. Every restart of the
     daemon leaves one, so the exception was the normal case, and the consequence — a robot that
     can never seal a window again — was invisible because everything kept recording.
111. **Two writers of one file need one story about who wins, and both need to write it down.** The
     API and the reconciler both edit the grant file; the reconciler always wins, which is correct,
     and it was the only one of the two that kept no permanent record of what it did.

---

## 2026-09-06 (sixtieth) — the sandbox at launch

§80's ring row ended with the sentence this session was assigned: *"What is still one process is a
sandbox: §8's permissions and the cgroup limits §40 resolves are the daemon's to apply to the unit
it launches."* The sentence names two mechanisms, and the audit found them in different states —
§40's shipped seasons ago (`Limits::from_manifest`, the RT-core-subtracting cpuset, both wired
into every `Unit` in the activate path), §8.3's never started (every unit launched with ambient
privilege; `isolation: "sandboxed"` recorded at admission and enforced nowhere at launch).

So the build is the missing half only: `launch::confinement_properties(kind)` — seven systemd
directives for `policy`/`service`/`skill`, none for `driver` by §8.3's own exemption (§19's
signature gate is the compensating control) — wired into `SystemdLauncher::command_line` where
the cgroup properties already are, with three tests on the argv systemd will actually see. The
directives left out are named in the module header with the unblocker for each: `PrivateTmp`
and `ProtectHome`/`strict` would fence stores under `/tmp` (which `e2e.sh` uses) or a home
directory; `DevicePolicy` waits on §40.3's accelerator allow-list, without which `closed`
refuses every GPU policy rather than the second one; syscall and address-family filters wait on
a measured allow-list, which §64 makes a defect to invent. The OCI sandbox of §23.4 stays the
designed end state, and nothing here claims to be it.

`safety.policy.isolation` stays skipped, and its reason is the most-edited sentence of the
session: the old one ("nothing on this edition sandboxes a package") is now false, but the
check as specified still has no machine that performs the forbidden hold — and the launcher
asserting its own argv would be an edition property, which is what `safety.kernel.parity` is
skipped for being. The audit also closed a stale bullet for free: round seven's "ships no unit
file" is now two files (`packaging/systemd/skynetd.service`, `skynetk.service`).

Deliberately untouched: `docs/skynet-os-architecture.md`. §80's sandbox sentence is half-stale
now, and correcting it means editing a file that mirrors BotMarket byte for byte, with
BotMarket absent from this machine. Noted in the build log rather than diverged in the spec.

### Numbers

`cargo test --workspace` — **1,628 passed**, 0 failed, summed over all 56 test targets;
baseline before the change 1,625, the +3 confirmed by stash-and-rerun to be the new
confinement tests. `cargo fmt --check --all` and
`cargo clippy --workspace --all-targets` clean. `node tools/check-*.mjs` not run — no `node`
on this machine; the change touches no mirrors, corpus, or golden-path surface.

### Findings, restated

1-110 as before.

111. **A gap described as one thing can be two halves in different states.** §80's sandbox row
     assigned §8 and §40 jointly to the launch, and read as a sentence it was one unfinished
     thing; audited as mechanisms, one half had shipped seasons ago and the other had never
     started. A joint sentence about two mechanisms hides a done inside an undone — audit the
     halves, not the sentence.

---

## 2026-09-06 (sixty-first) — a skill starts its unit

Round seven's skill bullet named a missing demo package and missed the unnamed half: whether
the daemon, given such a package, starts it at all. The contract suite proves the lifecycle
emits `Effect::Start`; nothing proved `Daemon::perform` performs it through the same `start()`
a desired state uses. The new test —
`a_skill_entering_running_starts_its_unit_and_leaving_it_stops_it` — drives the real fixture
daemon (`POST /skills`, three `advance_skills` into `running`, unit marked exited, two more
into `terminal`) and asserts `start acme/pick` then `stop acme/pick` in the launcher's calls.
It passed on the first run: the path was correct and only unwitnessed.

The same audit closed the "daemon does not launch the runtime" bullet for good — `skynetk` is
a boot unit by design, the daemon passes the rendezvous, both witnessed — and declined three
builds with named unblockers: the vision buffer pool (no capture backend, no consumer — a
pool now would be an unmeasured allow-list per §64), the isolation probe (half a route
without the CLI wiring, unverifiable without a systemd bus), and device confinement (still
`vram_mb: None` for want of a vendor runtime). Details and the round-nine list are in the
build log §60.

### Numbers

`cargo test --workspace` — **1,629 passed**, 0 failed; `cargo fmt --check --all` and
`cargo clippy --workspace --all-targets` clean.

### Findings, restated

1-111 as before.

112. **An effect nobody performs is a second launcher waiting to happen.** A lifecycle can be
     fully tested at its own boundary — every transition, every effect, exactly once — and
     still never have moved a process, because emission and performance are different halves
     owned by different modules. Test the seam, not just the sides.

---

## 2026-09-06 (sixty-second) — the reconciler killed every skill on its second pass

Building round nine's demo package found the bug underneath it: `perform` starts a skill's
unit through the ordinary path, and the next reconcile pass stops it — desired state never
names the package, so `!should_run` holds for ever. Every skill on a looping daemon lived
one interval. Fixed with `Actual.live_skills`, filled in `Daemon::observe` off the skill
table and spared in the plan's stop rule; no invocation live, the leftover is reaped. The
plan test failed red with the stop in full view before the fix, and the daemon spare test
drives the loop order end to end. `e2e.sh` §9 carries the live half a kernel-less robot
allows: the unit's trace file proves the start was performed, `robot_stopped` proves the
interlock (no kernel, no commanding — steady running here would be the interlock failing),
and the episode is queued. Full story, including the stash experiment that failed
identically with and without the fix and what that taught, is in the build log §61.

### Numbers

`cargo test --workspace` — **1,631 passed**, 0 failed; `cargo fmt --check --all` and
`cargo clippy --workspace --all-targets` clean; `sh crates/skynetd-io/e2e.sh` — all checks
passed.

### Findings, restated

1-112 as before.

113. **Red without green is a story, not a proof.** The new e2e failed with the fix stashed,
     exactly as predicted — and failed identically with the fix restored, because the
     interlock dominates a kernel-less robot either way. A failing end-to-end check is not
     evidence about your change until the passing run is also shown; here the pass lives one
     layer down, where no kernel confuses the question, and the e2e keeps the half it can
     honestly prove.

---

## 2026-09-06 (sixty-third) — the sandbox, read back off a real unit

§59's confinement was proven at the argv layer with a note that no systemd machine had been
asked what landed. This machine is one, so `e2e.sh` §3 now reads the daemon's own unit back
(`systemctl --user show`) and asserts ten properties — six directives, empty capabilities,
budget-as-bytes, task ceiling, non-empty cpuset with its value deliberately unpinned. No
Rust changed; the mechanism was probed with a throwaway unit first. Full e2e green.

The audit corrected a standing item rather than building one: round ten's "kernel in the
e2e loop" is misstated, because nothing outside contract tests ever reports kernel, RHAL or
homing state — the daemon derives `safe` on every robot, always, so a running skynetk alone
would change nothing observable. Restated in the build log §62 as design work (kernel
liveness into the daemon's conditions, within §33.1), alongside the conformance flip the
new e2e evidence half-delivers.

### Numbers

`cargo test --workspace` — **1,631 passed**, 0 failed; `sh crates/skynetd-io/e2e.sh` — all
checks passed, including the ten new unit-property reads.

### Findings, restated

1-113 as before.

114. **An assertion about argv is not an assertion about the machine.** The suite proved
     what `command_line` builds; only a running systemd could say what landed, and bytes
     (`MemoryMax=67108864`) disagree with strings (`MemoryMax=64M`) in exactly the place a
     substring assertion would hide. Read back what the platform reports, in its units, or
     the check proves the request rather than the result.

## 2026-09-07 (sixty-fourth) — §47.6's pool, and a rule the reference did not implement

The audit that opened this round listed §47.6 as *specified 2026-09-06, awaiting the editions*.
Filling it in looked like transcription: a TypeScript reference existed on the Hub, six unit
tests sat beside it, and the Rust, the Python and the corpus were missing. §31.4's third rule —
*a new case lands in the corpus before the fix lands in any edition* — is what turned it into
something else.

Ten scripted cases were written and **generated against the unfixed reference**, so the corpus's
first record of §47.6 was what the reference did rather than what §47.6 said. Three came back
wrong. §47.6 says `publish` *"reuses the oldest released slot"*; the reference took the
**lowest-index** slot nobody was holding. Those agree while a pool holds one frame and diverge
on every publish after that — and all six unit tests published exactly one frame into a
multi-slot pool, so not one of them could see it.

The cost lands on the reader. A policy holds a frame by calling `latest()` then `acquire()`, with
no lock across the two — there cannot be one, because §47.4 forbids capture waiting on a reader.
Capture publishes in between, and under the lowest-index rule that publish lands on the slot
`latest()` just named: `stale_handle` on nearly every cycle, a four-slot pool absorbing none of
the jitter it was declared for, and a symptom that reads as a slow policy. §47.4 already has a
one-slot latest-wins channel; a §47.6 that collapses into one is a duplicate of the section above
it rather than a weaker version of itself.

The rule is now in §47.6a: among unacquired slots an **empty** slot wins outright, otherwise the
**lowest sequence**, ties to the lower index — oldest by the frame, never by the slot's position,
because the two stop agreeing the first time a reader holds a frame and lets it go.

Three editions: `reference/typescript/media.ts`, `crates/skynet-contracts/src/media.rs`,
`editions/python/skynet_os/media.py`. Both ports reproduced all ten cases on their first run,
which proves nothing on its own — so the rule was reverted in each and the corpus re-run:
**1,124 passed, 3 failed** in both, exactly the three cases written for it.

The module also moved repositories. It was the Hub's, and `check-contract-mirrors.mjs` applies
one test to that list — *both sides compute this and neither is authoritative*. Nothing on the
Hub owns a camera, so the Hub's copy was a module nothing imported. `media` is reference-only
now, like `cell` and `blackbox`; the orphan and its six tests were deleted there.

`corpus/media/pool.json` is the first family in the corpus with **state**, so a case is a script:
a capacity, a list of operations, and every outcome in order. Nothing in it exposes the pool's
slots — the handles carry the slot they were issued against, which pins the selection rule
without any edition having to agree about how a slot is stored.

Four stale numbers turned up before anything was built, from the checks that exist for it: §73
said 78 corpus files and 1,116 cases, and `README.md` said **1,055 cases and 77 files** — two
sessions stale, in the table a vendor reads first.

### Numbers

`cargo test --workspace` — **1,641 passed** (1,631 before), 0 failed. Rust corpus **1,127 of
1,127**; Python corpus **1,127 passed**. Hub: `npx vitest run` — 1,853 passed, 81 files;
`npm run typecheck` clean. Every check tool green: mirrors, contract mirrors, golden path,
scale (§73 matches the tree), edition report.

### Findings, restated

1-114 as before.

115. **Six tests can pin a class and never once exercise its only decision.** Every one of
     §47.6's unit tests published a single frame into a multi-slot pool, so the rule deciding
     *which* slot a publish takes was unreached by all of them while each looked like a test of
     the pool. The corpus caught it in the first case that published twice — and only because
     the case was generated before the fix, which is what §31.4's third rule buys.

116. **A degenerate implementation of a section is not a weak version of it, it is a copy of the
     section above.** The lowest-index rule turns §47.6's pool into §47.4's latest-wins slot.
     The failure is not "the pool is smaller than declared"; it is that a section deferring to
     another one now has nothing to defer to.

117. **The mirror list's test has a direction, and a module can fail it from the Hub's side.**
     `media` was written where nothing consumes it. The list's own header names that mistake and
     the check could not catch it, because a module that is not on the list is not compared —
     absence from a mirror list is invisible to the mirror check by construction.

## 2026-09-07 (sixty-fifth) — §5's `detects_people`, and the guard it turned on

`resolveLimits` takes seven `available` inputs and the seventh had no path: §5's nine modalities
say what a sensor **produces**, and detection is what somebody does with the output, so no body
could declare a person detector and `skynetd` wrote `people_detection: false` on every robot.
§80 had carried it for four sessions with that diagnosis and the specified close.

Built as specified: `detects_people?: boolean` on the sensor, declared per sensor by the vendor,
with `hasPeopleDetection` deriving the input. No vocabulary, so no governance — which is what
unblocked it while §30's role vocabulary is still open. Three editions, nine cases in
`corpus/embodiment/detection.json`, and the daemon derives it through the contract rather than
three lines of its own so a Hub reading the same descriptor cannot answer differently.

Absent and `false` are one fact downstream and two in the descriptor. The corpus header says so
rather than implying a coverage it has not got: both make the guard unavailable, so no expected
value can separate them and an edition that folded them together would pass every case.

What the nine cases do pin is how each edition reads the **value**, because each does it by a
different mechanism with its own way to be wrong — `=== true`, `as_bool()`, `is True`. Python's
is the sharpest (`1 == True` is true, `1 is True` is not), so there is a case for the number one
and the mutation run catches every substitution: `== True` fails one case, truthiness fails two,
and Rust's `.is_some()` fails the declared-absent one.

Two cases pin that the modality is **not** consulted, including a force-torque sensor declaring
person distance. That looks wrong and is right: the alternative is a list of modalities a
detector may live behind, which is a vocabulary needing the governance this design avoided, and
whose wrong entry would disable a guard on a robot whose vendor had declared one.

Then the honest half, which is the finding. `proximity` acts on `cycle.nearest_person_m` and
**nothing on this edition produces that number**. A body that declares a detector now moves the
guard out of *unavailable* and into *active*, where it will never trip — and *active* is what
every surface renders as protection. §7.2's partition has no state for "every input resolves and
the reading is not coming", and adding one changes a partition the console, the check-in, the
device page and `skynet limits` all draw. So §80 gained a row rather than an answer, and
`skynetd`'s limits document now names `cycle.nearest_person_m` in `unmeasured` on every robot.

### Numbers

`cargo test --workspace` — **1,645 passed** (1,641 before), 0 failed. Rust corpus **1,136 of
1,136**; Python corpus **1,136 passed**. Hub: 1,853 passed, typecheck clean. Every check tool
green.

### Findings, restated

1-117 as before.

118. **Closing a hole can open a worse one one layer down, and the direction matters.** Before
     this, `proximity` was unavailable and named what it wanted — honest, and useless. After it,
     a declaring body gets a guard listed *active* that cannot trip — useful-looking, and a lie.
     The first failure sends somebody to buy a sensor; the second tells a room full of people
     they are being watched for.

119. **A vocabulary avoided is a governance process avoided, and that is worth paying for.** The
     obvious hardening — only honour `detects_people` on modalities a person could plausibly be
     measured in — costs a list of modalities, which is a closed set, which needs the process §30
     leaves open. And its wrong entry fails in the disabling direction on a robot whose vendor
     declared a detector. A boolean the vendor writes has no such entry to get wrong.

120. **Three editions is three ways to be wrong about one boolean.** `=== true`, `as_bool()` and
     `is True` are the same rule and not the same code, and the Python one is a genuine trap:
     `1 == True`. The case for the number one exists so that the paragraph describing the trap is
     a check rather than a claim.

## 2026-09-07 (sixty-sixth) — `safety.policy.isolation`, and the difference between argv and a unit

Round eleven left this half-delivered: §59 built the confinement, `e2e.sh` read ten properties
off a real unit, and the conformance runner still said **skipped**. The gap was not effort. §59's
suite asserts what `command_line` builds, which proves what was *requested*, and a directive
systemd rejected, dropped or does not support on this kernel produces exactly the same argv. It
is also the wrong kind of result: a check whose subject is this repository's own constant is an
**edition** property, which is what `safety.kernel.parity` two lines above is skipped for being.

`Launcher::confinement(id)` runs `systemctl show <unit> -p <directives>`; `confinement_shortfall`
names what did not land. Three decisions, each with an obvious wrong alternative:

* the comparison is the whole `Key=Value` line, because `CapabilityBoundingSet=` is asked for as
  the **empty** set and a key match would pass `cap_sys_admin`;
* an unreported directive counts as not landed — a sandbox nobody can read is not one anybody
  should certify;
* the `--property=` query is **derived** from `confinement_properties` rather than typed beside
  it, which removes a bug that was otherwise waiting: a directive added to the launcher and
  forgotten in the query would fail the robot's own isolation test while naming a directive that
  was in fact applied.

The route reads the first running **non-driver** unit — §8.3 makes a driver native by design, so
failing one for its absent confinement would be the robot failing itself for obeying the
specification. `RecordingLauncher` returns an error rather than an empty list, because an empty
list would make `--dry-run` fail a safety test on a machine it never touched. `conformance::run`
stays a function over facts; the I/O is in `api.rs`.

**What this session could not watch.** This container has `systemd-run` installed, no
`/run/systemd/system`, and `process_api` as PID 1 — `systemctl --user is-system-running` answers
`offline`. So `e2e.sh` took its no-systemd branch and asserted the **skip**. The pass assertion
is in the file and runs on a machine with a user systemd; round eleven's had one and this one
does not. The pass path has not been watched in this session, and reporting otherwise would be
the exact failure this runner's header exists to name.

### Numbers

`cargo test --workspace` — **1,655 passed** (1,645 before), 0 failed. `cargo clippy --workspace`
clean. `sh crates/skynetd-io/e2e.sh` — all checks passed.

### Findings, restated

1-120 as before.

121. **A launcher's argv is a request, and a conformance suite that reads it is grading its own
     homework.** The same seven flags are produced whether systemd honoured them, ignored them,
     or does not implement them on this kernel. Two rounds separated "the confinement exists"
     from "the confinement landed", and only the second is a fact about a robot.

122. **A query derived from a constant cannot drift from it; a query typed beside one will.**
     `--property=` and `confinement_properties` are the same list, and had they been two lists
     the failure would have been a robot failing itself over a forgotten string while naming a
     directive that was applied. The fix is one function, not one test.

123. **A fake must refuse, not answer emptily.** `RecordingLauncher` returning no properties
     would have been read as a unit with no confinement, so `--dry-run` would fail a safety
     conformance test about a machine it never touched. The honest empty answer is an error
     carrying its reason.

## 2026-09-07 (sixty-seventh) — the `kernel` condition stops being a default

Round eleven restated this item and said why it had been misstated: nothing outside contract
tests ever reports `KernelState::Live`, so `refresh_operating_state` derived from POST plus
defaults and a running `skynetk` would change nothing observable. This is the first half.

The obvious source is systemd, and it is wrong for one reason: **a hung kernel is an active
unit.** `ActiveState=active` says a process exists, which is what a robot with a wedged control
loop also has, and §27's most consequential condition cannot rest on the one signal that
conflates the dangerous case with the healthy one. A turning loop **writes a cycle every pass**
(§26), so liveness is cycles recorded since the last look and nothing else.

It does not cross §33.1, and two properties are why. It fails closed — cycles seen is `Live`,
and silence, an unreadable ring and no ring at all are all `Down`, so a wrong answer can hold a
robot and never release one. And the daemon **cannot forge it**: §26.2 gives the supervisor the
event channel and the loop the four cycle channels, so `skynetd` is structurally incapable of
writing the record it reads.

`KernelSighting` has three variants where §27 has two states, because a console needs the
difference: a ring with no action channels *cannot* hold a cycle, so silence from it says nothing
about a kernel — which is every robot on this edition — and showing "kernel: down" there would
send somebody to restart a kernel that is not the problem. `GET /state` reports `observed` per
condition, plus the reading, its detail and its source.

The cursor is advanced every pass whether or not anything was counted, and is deliberately
separate from `skills_sequence`: one cursor over one ring would make each consumer eat the
other's evidence.

It removes **one of seven** blockers, and the log says so rather than implying more. A robot with
a turning kernel still derives `safe` with `rhal_lost`. What changed is that the next condition
is a *mapping* rather than a channel: a cycle record already carries `rhal_result`, `state_flags`
and `fault_code` (§24), so `Present`/`Degraded`/`Lost` is a decision about §24's semantics.

### Numbers

`cargo test --workspace` — **1,658 passed** (1,655 before), 0 failed. clippy clean, e2e green,
§73 matches the tree. Four stale sentences corrected: the start-up banner, `GET /limits`'
`partial` note, `warn_for_transition`'s header, and two rows of §73.

### Findings, restated

1-123 as before.

124. **The signal that is one call away is the one that cannot see the failure.** `systemctl` can
     tell a running kernel from a stopped one and not a turning one from a hung one, and the hung
     one is the case §27 exists for. The evidence that separates them was already on disk,
     written for another purpose.

125. **Evidence a process cannot write about itself is worth more than evidence it promises not
     to.** §26.2 split the channels for a different reason — two writers, one ring — and the
     side effect is that `skynetd`'s liveness reading is unforgeable by construction rather than
     by discipline.

126. **A default and a reading are different even when they produce the same answer.** Every
     robot on this edition reports `kernel: down` before and after this change. Before, because
     nobody looked; after, because the recording cannot hold a cycle and says so. Only the second
     tells an operator where not to go.

## 2026-09-07 (sixty-eighth) — the record could not say the read had failed

Round fifteen named the next condition as a *mapping* over `rhal_result`, `state_flags` and
`fault_code` — "not a channel that does not exist". Reading those fields to write the mapping
found the channel missing after all, and worse than missing.

On a failed `read_state` the loop builds its observation from defaults: zero flags, zero fault
code, zero sequence, the loop's own clock as the acquisition stamp §24 says must be an acquisition
time, and joint arrays copied from buffers still holding the **previous cycle's** positions. Every
one of those is legal on a healthy robot. So a recording of a driver that had stopped answering
was byte-comparable with a recording of one that had not, and a replay of it showed a robot
holding still. `summary.read_failures` knew, and it dies with the process; the black box is what
outlives the robot.

`read_result` fits at offset 172 — **reserved padding**, so nothing moved and the ring's
`FORMAT_VERSION` did not change. Which made the question: what does a zero there mean in a
recording written last week?

The answer was a byte that had never been read. `slot[5]` has carried a per-record schema version
since the format was written, with a comment saying exactly why — *a ring outlives a daemon
upgrade and a reader must be able to tell which layout a slot is in* — and nothing read it. It
said `1` while the frame record had already grown a field, and `README.md` reported "Record format
2" about it. It says `2` now, the decoder reads it, and a schema-1 record's read outcome is
`Unrecorded` rather than `Ok`: reading absence as success is the failure the field exists to
prevent.

`Actuator::sample` now returns a code as well as a sentence. A recording is replayed by somebody
without this build's error strings, and `-110` is a timeout in any language.

### Numbers

`cargo test --workspace` — **1,659 passed** (1,658 before). Both corpora **1,137 of 1,137**.
Mutations: the Rust schema byte back to 1 fails 12 records; Python not writing the i32 fails the
new case. clippy clean, e2e green, §73 matches the tree.

### Findings, restated

1-126 as before.

127. **A record that fills its gaps with legal values is worse than one with a gap in it.** Zero
     flags, a zero sequence and last cycle's positions are all things a healthy robot produces, so
     the failure did not look like missing data — it looked like data. The counter that knew was
     in memory, and memory is not what an investigator has.

128. **A version byte nothing reads is not a version byte.** It had the right comment and the
     right intention for as long as the format has existed, went stale silently when the frame
     record grew, and was contradicted by the README. What made it real was needing it: four bytes
     whose meaning depends on it.

129. **Writing an edition before the reference is the wrong order, and the corpus is still the
     arbiter.** Rust went first here. What kept §31.4's rule meaningful is that the reference was
     written independently against the decision rather than transcribed from the Rust — had either
     guessed a different offset or byte, the corpus would have failed one of them.

## 2026-09-07 (sixty-ninth) — §27's `rhal`, and the flag that is not a loss

The mapping §26.4 cleared the way for. Three of §27's eight conditions are observed now.

`kernel` is an interval question — *cycles since the last look* — and `rhal` is not: §27's `rhal`
is a **state**, so the **newest** cycle decides it. A driver that missed one read in two hundred is
not lost; one that has answered nothing for a second is, however many good reads preceded it. Both
are read from the same records over the same interval, because a robot whose kernel is turning
while its driver has stopped answering has to report exactly that pair.

Five inputs onto three states, four of them mechanical: no cycle at all, a negative `read_result`,
and an unrecorded one (schema 1, which fails closed) are `Lost`; `RHAL_DEGRADED` is `Degraded`;
otherwise `Present`.

The fifth is a decision. **§24's `RHAL_FAULT` is not `Lost`.** It reads as though it should be —
"the drive has faulted; commands are being ignored" — and §27 states why not about itself:
`rhal_lost` is the row meaning *nothing else can be known*, first in `derive` because everything
below it becomes unanswerable. A faulted drive still **answers reads**; that is how anybody knows
about the fault, since the fault code is in the record because the read succeeded. Mapping it to
`Lost` would tell an operator the robot cannot be observed at the moment it is telling them
something, and would discard the code on the way. A fault is `Present` with the vendor's code
verbatim, and what a fault *does* belongs to §7's guards.

### Numbers

`cargo test --workspace` — **1,660 passed** (1,659 before). clippy clean, e2e green, every check
tool green.

### Findings, restated

1-129 as before.

130. **"Is it turning" and "what is it" are different questions of the same records.** An interval
     answers the first and only the newest record answers the second. Using one shape for both
     would make a flapping driver read as present half the time, or a healthy one read as lost
     because of a single missed read an hour ago.

131. **The strongest-sounding flag is not the most severe state.** `RHAL_FAULT` sounds worse than
     `RHAL_DEGRADED` and maps to a *better* condition, because §27's `rhal` asks whether the body
     can be observed rather than whether it is well. The read that reported the fault is the proof
     the answer is yes.

132. **A number stated in four places and derived in none goes stale every time it changes.**
     Three rounds running, moving a condition has left the count wrong in the banner, in
     `GET /limits`' note and in two rows of §73. Filed rather than fixed in passing.

## 2026-09-07 (seventieth) — §27's `guard_hold`, and three numbers that were published wrong

An audit first, because the last round closed by saying the four remaining §27 conditions were all
waiting on hardware or an ABI. One of the four was not, and finding that out was a matter of reading
the reason rather than the sentence.

`guard_hold` had been filed as **"the kernel's"**. So are `kernel`, `rhal` and `stop`, and all three
were closed in the three rounds before this one — whose fact it is was never what decided them.
What decides whether a supervisor may observe a condition is whether the kernel **wrote it down**,
and §26's cycle record carries the guard verdict beside the flags this daemon already reads every
pass. So `HoldSighting`, off the same newest cycle as the RHAL and the stop.

### The two refusals, which are the whole of the design

**An advisory hold is not a hold.** §7.4 lets a tier that cannot promise a clamp record a verdict
and forward the command anyway, and every robot on this edition is one — so a recorded `Hold` here
almost always describes a hold that was demanded and never applied. Mapping it onto §27 would report
`HOLD` for a robot that is moving, which is what §7.4 and §42 both exist to prevent and what this
repository already did once inside `decide` itself. `Advisory` is a third state: reported as an
observation, because a guard fired; not a hold, because nothing was withheld. `GET /state` carries
`held` and `advisory` together, since a console given only the first cannot tell a robot whose
guards are being overruled from one no guard has touched.

**A failed body read does not stop this one.** `stop` refuses such a cycle because §26.4's fill puts
the previous cycle's flags in it. The guard verdict is not a fill — `decide` runs whatever the read
returned, and what it produces is what the kernel did with the command. `rhal` and `stop` are facts
about the body; this is a fact about the kernel, and that is the line the two readings fall on
either side of.

### What it does not move

A robot's state. `guard_hold` is rule 7 of `derive` and `homing` is rule 5, defaulting to `Pending`
on every robot here because §24 gives the ABI no flag for it. So this edition reports `unhomed`
while carrying a good guard-hold observation, and the route test asserts that ordering rather than
leaving it to be discovered. `homing` is now the only one of the eight that no reader can close.

### And three published numbers

The audit that opened the round found `reference/reports/editions.json` claiming 1,137 corpus cases
against 1,142 actually passing — a figure the Hub mirrors and renders at `/skynet/os/editions`. The
tool that catches it was not in CI, and every neighbouring check passed honestly: the mirror check
compares two copies that were stale together, and a `--min-passed` ratchet cannot catch a stale
exact figure. It runs in CI now. `scale-report.mjs` was separately looking for two spellings of the
Hub's directory where the other two tools looked for seven, so it had been reporting §73's
cross-repository figure unmeasured, and passing. One list, in `tools/hub-path.mjs`, and the mirror
now measures.

### Numbers

`cargo test --workspace` — **1,680 passed** (1,676 before). clippy clean, fmt clean, e2e green
against a real daemon and a real control loop, 1,142 of 1,142 in both editions, every check tool
green. Hub: 1,895 unit tests, typecheck clean.

### Findings, restated

1-132 as before.

133. **"Whose fact is it" and "who wrote it down" are different questions, and only the second
     decides observability.** Four conditions were filed with the same one-word reason and three of
     them have since been closed without that reason changing. A reason that would equally justify
     leaving a condition open after it was closed is not a reason.

134. **A ratchet cannot catch a stale exact figure.** A floor is the right shape for a number that
     should only grow and the wrong shape for one that should be *equal* — and the difference is
     invisible while the figure happens to be above the floor, which it was for as long as anybody
     had been reading it.

135. **A check that cannot find what it compares against exits zero.** Two tools found the Hub and a
     third did not, because the third's list of directory names was five short. Nothing failed;
     one figure simply stopped being checked, and the message saying so was one line among eight
     that all said `ok`.

## 2026-09-07 (seventy-first) — §27's `guards`, and the list the field's own documentation named

The audit's other candidate, and it closed the same way as `guard_hold`: the reason for leaving it
out was a true sentence about the wrong place. *"A cycle records what guards did, not which were
available"* is correct about §13's ring, and §7.6's partition is not in the ring — it resolves **on
this daemon**, is served at `GET /limits`, drawn by `skynet limits` and the console, and carried
across §17 to the device page. The answer was being computed, rendered twice and reported to the
Hub every check-in while §27 took it at a default.

### The list nothing read

`guards_unavailable` is annotated in `skynet-contracts` as *"§45.5 is the usual source"*, and
§45.5's guard list — calibration expiry — is a **different** list from the resolver's, produced by
a different module and already consumed by POST and by `skynet calibration`. Either alone is right
about half the robots: a body that declares no IMU loses `balance` through the resolver with every
calibration in date, and a body with a flawless descriptor and an extrinsic that expired last month
loses `proximity` through §45.5 with nothing in the resolver noticing. The *usual* source was the
missing one.

So the condition is the union, merged rather than concatenated — one guard is one problem however
many lists name it — and each entry says which source took it out, because *fit the sensor* and
*recalibrate* send an operator to different parts of the building.

### What it is not, said where it is rendered

A **resolved** partition, never a per-cycle one: which guards can run given what the body declares
and what a calibration still believes. §80's `inert` state exists because that differs from what the
kernel had a reading for, and a declared sensor that fails at runtime is still invisible. The route
says so in its own `source` string rather than leaving a reader to assume the stronger claim.

And a robot **nobody has described** is `unobservable`, not a robot whose guards are all broken.
With no descriptor the resolver correctly reports nearly every guard unavailable, which as an answer
to §27 would send somebody to check wiring when what is missing is the document. The suite proves
the variant is necessary by first asserting that the resolver does have plenty to say about such a
body.

### Numbers

`cargo test --workspace` — **1,682 passed** (1,680 before). clippy clean, fmt clean, e2e green with
six of §27's eight conditions observed on a running daemon, 1,142 of 1,142 in both editions, every
check tool green. `homing` and `policy` remain, and `homing` is the only one with no path at all.

### Findings, restated

1-135 as before.

136. **A field's own documentation can name a source the code never read.** `guards_unavailable`
     has said *"§45.5 is the usual source"* since it was written, §45.5's list has existed for as
     long, and the two had never met. The annotation was not wrong and not enough: nothing checks
     that a doc comment naming a source is wired to it.

137. **The absence of a document is not a reading of what it would have said.** An undescribed body
     resolves to nearly every guard unavailable, which is correct arithmetic and a false report:
     the remedy for "your guards are off" is a technician and the remedy for this is a descriptor.

138. **A test helper that shadows the thing under test is a suite proving something about itself.**
     `limits.rs`'s `guards` helper predated the module function of the same name by months and
     would have silently taken every new call in the suite.

## 2026-09-07 (seventy-second) — §27's `policy`, and the sentence that was false about this daemon's own store

The last of §27's eight with a source in this process, and the only one whose honest answer is
*partly* unobservable.

`PolicyState` has three values and this daemon can establish two exactly. **`None`** — nothing
installed declares an action space — is a question about a store this process holds. **`Held`** is
four distinct facts it also holds: a refused plan, an unreadable manifest, two commanding packages,
or a resolved plan whose unit is not running, because a policy whose process does not exist holds no
lease. The third, **`Ready`**, asserts §25.2's lease. This daemon resolves the plan and launched the
unit, so it has both halves of the *necessary*; the lease table is the Safety Kernel's. So that one
case reports itself unobserved, is taken at `held`, and `ready` is a value the mapping cannot
produce on any input.

Reading the unit is not a §33.1 crossing, and the reason is worth keeping: the daemon **started**
the unit, so whether it is running is its own action's result rather than a question to the kernel.

### The defect it closed

The condition sat at its default, which is `None` — and §27's rule 8 splits `no_policy` from
`policy_not_ready` precisely because *"there is nothing to run"* and *"there is something and it has
not got a lease"* are different sentences and the second is the one with a remedy. A robot with a
commanding package installed and activated was reporting the first. That is not a cautious reading
of an unobserved condition; it is a false statement about a store this process holds, made by the
process that holds it — and the cautious reading was available the whole time, since `Held` is
weaker than `Ready` and is the only one of the three that is true.

It was latent because `homing` stops every robot here three rules earlier, which is why the suite
drives §27's machine directly to reach rule 8 and assert both sentences.

### Two things the tools caught before a person did

Clippy refused `refresh_operating_state` at eight arguments, six of which read as "a sighting". They
are a struct now, and the reason is stronger than ergonomics: §27's conditions are only comparable
when they are readings of one instant, and a struct assembled once per pass says that where six
parameters invite a caller to gather them at six different moments.

And `scale-report --check` reported that it was guarding nothing. Four sentences reached **one**
unobserved condition, one is singular, and rewording them made every pattern miss. The check counts
what it matched separately from what failed, so a grammar change produced a message instead of a
silent hole.

### Numbers

`cargo test --workspace` — **1,684 passed** (1,682 before). clippy clean with no warnings, fmt
clean, e2e green with **seven of §27's eight conditions observed** on a running daemon and the
policy condition naming the installed package, 1,142 of 1,142 in both editions, every check tool
green.

### Findings, restated

1-138 as before.

139. **A default that is safe can still be false.** `PolicyState::None` holds a robot, which is the
     safe direction, and says the store is empty, which this process knows it is not. "Cautious"
     has to be chosen among the values that are *true*, not among all of them.

140. **A condition can be observable on some robots and not on others.** Six of §27's eight are
     properties of the edition; `policy` is a property of the robot — answerable on every machine
     except the one where a policy is actually about to run, which is the machine where it matters.
     Reporting that per-robot is more honest than picking one answer for the fleet.

141. **A check with English in it can go stale in its grammar.** Counting what a pattern matched,
     separately from what failed, is what turns that from a silent pass into a message. The count
     reached one; the sentences became singular; every pattern missed; the tool said so.

---

## 2026-09-08 (seventy-third) — §47.6's mapping half, and the number that did not have to cross

`skynet-frames` ended the previous session as a crate with twelve passing tests and **no caller**,
which is §79.7's shape exactly — a module nothing imports is a module nothing has tested. This
gave it the route.

The crate is the Linux mapping half of §47.6: two shared regions per pool, pixels mapped read-only
into the reader and a loans region of one `u64` per slot mapped writable. It carries no copy of the
selection rule — that rule is `skynet-contracts`' `FramePool`, reproduced by three editions and
pinned by `corpus/media/pool.json` — and it is `#![forbid(unsafe_code)]`, because the one `mmap`
belongs to `skynet-ring-shm`.

### The geometry stays in the header, and that was a decision

`Reader::adopt_declared` reads the pool's shape out of the region rather than off the environment.
The four-line alternative was there: `SKYNET_RING_CHANNELS` already crosses beside the rings, and a
`SKYNET_FRAME_POOL=<slots>x<bytes>` beside it would have worked. It would also have been a second
copy of a number the region already states, and the failure when two copies disagree is **not an
error** — it is a reader addressing the wrong offset for every slot past the first and finding
plausible pixels there. So a receiver maps 64 bytes of a duplicate descriptor, checks magic and
version, reads the geometry, and only then maps the region.

### The count is the announcement

`pass::MAX_FDS` went two to four and the pool crosses in the **same** `SCM_RIGHTS` message as the
two rings — pixels third, loans fourth. Two descriptors is a runtime with no camera; four is one
with a pool. Never three: a policy handed pixels and no loans region can read a frame and cannot
stop the writer taking the slot out from under it, which is the one property the pool exists to
provide. `adopt_from` refuses any other count by name, and both descriptors are consumed on every
path including the refusals — asserted by calling `dup` on them afterwards and requiring it to fail.

`skynetk --frame-pool <stream>:<slots>x<bytes>` creates the region and hands it over, and refuses
without `--policy-socket`, because a pool nobody receives is a mapping a process talks to itself
through. It does **not** publish into it: this edition has no capture backend, so the pool stays
empty for the whole run and the summary says so rather than implying otherwise (§34.3). The region
and the hand-off are the operating system's; the pixels are a vendor driver's.

Two integration tests start `skynetp` as a real child and read its stdout — one where a pool crosses
and the policy names the slot the frame is in, one where none does and the policy says nothing about
frames. The second exists because *"four means a pool"* asserted and *"two means no pool"* assumed
is exactly the pair where the assumption fails quietly.

### Numbers

`cargo test --workspace` — **1,718 passed** (1,702 before). clippy and fmt clean, 1,142 of 1,142 in
both editions, 44 self-checks, `scale-report --check` green at 24 crates and 126,000 lines, mirrors
match.

**And `e2e.sh` reported three failing checks that this session did not diagnose**, recorded as the
next session's first task rather than as a footnote. Its first run reported eleven, and every one of
those was a harness error — run from `crates/skynetd-io`, where the mock driver's compile step
cannot find `drivers/mock/mock_robot.c`, so ten checks cascaded off one missing `.so`. The script
fails informatively for a missing compiler and does not for the wrong working directory, which is a
gap in the check an operator actually runs.

### Findings, restated

1-141 as before.

142. **A crate with no caller is the shape §79.7 is about.** Twelve passing tests over a module
     nothing imports say the module is self-consistent, and nothing else. The route is what tests it.

143. **A number the region already states must not also cross out of band.** When the two copies
     disagree the reader does not fail — it reads plausible pixels at the wrong offset, which is the
     failure mode with no error in it.

---

## 2026-09-08 (seventy-fourth) — the three failing checks, and the second supervisor

The previous session left one instruction: *"it was three, it was not diagnosed, do not assume
either."* It was not three, it was not a regression, and it was not in the daemon.

The first run this session reported **thirteen**, one cluster: every §59 confinement read-back, plus
§15's `safety.policy.isolation` and the two assertions about what its pass says. Each dumped every
directive present and every one of them a refusal — `NoNewPrivileges=no`, `ProtectSystem=no`,
`MemoryMax=infinity`, an empty `AllowedCPUs`. It reads as a sandbox that landed nothing. It is
**`systemctl show`'s output for a unit it has never heard of**: the manager's own defaults, printed
for whatever properties are named, with **exit status 0**.

### Why there was no unit

The package started and was stopped again half a second later, over and over. Nothing in `skynetd`
was doing it — `GET /plan` never contained a `StopPackage`, no skill emitted `Effect::Stop`, nobody
called `POST /stop`. Two units identical but for their names settled it: `probe-plain.service` stayed
up, `skynet:openrobot:probe.service` was stopped 1.7 s after starting, every time.

    3030  03:30:57  skynetd --root /tmp/skynet-e2e/kernel --user --interval 500

**A `skynetd` abandoned by an aborted run three and a half hours earlier.** §33's unit names live in
**one** user manager, so a second supervisor observes the first one's units, finds them absent from
its own desired state, and stops them. A robot has one `skynetd`, so this is not a defect in the
daemon; it is a defect in a script that started five background processes and cleaned up one. The
trap killed `$DAEMON`; `$MDNS`, `$KDAEMON`, `$KLOOP` and `$KSTOP` were killed only by the lines
that follow them, which an early exit never reaches.

### Fixed in three places, and only one of them is the script

1. **`confinement()` was reading an answer that was not about a unit.** A pure
   `confinement_from_show` now asks for `LoadState` alongside the directives — in the *same* call,
   because two calls could straddle the moment `--collect` reaps a transient unit — and returns
   `Err` unless it is `loaded`. This matters past the script: `policy_isolation` treats an
   unreadable confinement as a **skip** and a shortfall as a **fail**, so before this a robot whose
   policy had stopped would fail its own `safety.policy.isolation` naming seven directives that were
   in fact applied. `applied()` had the honest version all along (`MainPID` of 0 → *"it stopped
   between being listed and being read"*); the rung above it did not. A test keeps the old
   misreading demonstrable — the real not-found dump, parsed the old way, still produces a shortfall
   of exactly seven — so nobody can quietly undo it.
2. **The script kills what it starts.** `track` records every background pid, `kill_tracked` is the
   trap, and a `pkill -f "skynetd --root $ROOT"` follows it for a process whose pid the shell never
   learned.
3. **The script refuses to run beside another supervisor**, naming the pid and the root. A run
   against a poisoned machine that reports ten confinement failures is worse than one that does not
   start.

The read-back now reports **one** failure when there is no unit — *"the unit was started and is not
there to read"* — rather than ten sentences that are each false.

### Numbers

`sh crates/skynetd-io/e2e.sh` — **98 checks, 0 failed** (13 failed / 85 passed at the start).
`cargo test --workspace` — **1,721 passed**. clippy and fmt clean, corpus and self-check green,
`scale-report --check` green, mirrors match. All three mechanisms verified rather than assumed: a
clean run leaves no `skynetd`, a run started beside a stray refuses with exit 1 naming its pid, and
an interrupted run leaves zero daemons where it used to leave one for ever.

### Findings, restated

1-143 as before.

144. **`systemctl show` answers for a unit it has never heard of, with exit 0.** It prints the
     manager's defaults for whatever properties are named. Reading a confinement without reading
     `LoadState` in the same call is reading an answer that is not about a unit — and every value in
     it is a plausible one.

145. **A script that starts five processes and cleans up one leaves a supervisor on the machine.**
     It outlives the checkout that made it, and everything it then breaks names the wrong thing.

---

## 2026-09-09 (seventy-fifth) — the audit before the round, whose own first draft was wrong

No code. Asked whether the plan was finished, and the useful half of the answer was a correction to
the audit itself.

The first draft listed as open: *"a policy package that speaks §25 — `skynetp` appears nowhere in
`skynetd-io`, there is no manifest in the tree, and §54.4's lifecycle has never started a real
commanding loop end to end."* **All three clauses were false, and they were false when they were
written.** `e2e.sh` §9 has installed a real skill package since the ninth session — `acme/wave`, the
`skynetp`-in-a-blob pattern, staged through `skynet install`, activated through desired state,
invoked over the socket, watched into `running` and out through `terminal`. Building it is what
found the reconciler bug in the sixty-second session.

The draft was assembled from §73's Skills row and a stale bullet in this log, both of which said
*"nothing joins them"*. The grep that settled it took one command.

### What that cost, and what was fixed

Three documents were behind the tree, and closing them was the largest item of the session rather
than the smallest:

* `docs/skynet-build-plan.md` had **diverged** from the Hub's copy — four Hub sessions existed only
  in the other repository. 180 lines, purely additive, so `--sync` was the right direction and was
  checked to be before it was run.
* §73's **Skills row** now carries the package and the lifecycle it is driven through, and keeps the
  stale sentence visible with the reason: a stale *"still missing"* is the one kind of wrongness a
  *what is missing* column cannot catch by being read. What replaces it is the two things that
  **are** missing — no physical joint has produced the post-state §54.3 judges, and
  `intervention_count` is attributed only while an invocation is `running`.
* §80's **ring row** ended *"what is still one process is a sandbox"*, false since the fifty-ninth
  session. That session recorded why it had not fixed it — the fix means editing a mirrored document
  and the other tree was not on the machine. This was the pass that had both.

Both documents were edited in **both** repositories and `docs/mirror-digests.json` re-pinned with
`--pin` rather than `--sync`, which is the round the distinction mattered: `--sync` would have
silently reverted the edit just made in both trees.

### Numbers

No code. `e2e.sh` 98 checks, `cargo test --workspace` 1,721, `scale-report --check` green at all
eight figures, mirrors and contract mirrors match after the sync.

### Findings, restated

1-145 as before.

146. **A summary is evidence about when somebody last wrote it down, and nothing else.** Three of
     the four things this audit called open were closed in the tree, and the one genuinely open item
     was the one the documents said least about.

147. **A stale "still missing" is the one wrongness a "what is missing" column cannot catch.** It
     costs a reader nothing to believe, and the next session plans around a hole that is not there.

---

## 2026-09-09 (seventy-sixth) — §79's two halves meet, and both were wrong about a trailing slash

The previous session left one buildable item: the robot half and the Hub half are each exercised
against a double, and neither has met the other. This is that, and it found two defects neither
suite could have seen.

`scripts/skynet-verify-recordings-robot.mjs` lives in the Hub beside the receiver's own verifier,
because that is where the service-role client and the cleanup convention already live. `skynetk`
turns the loop against the mock RHAL driver at 200 Hz for 400 cycles and seals a real MCAP window;
an unpaired `skynetd once` reshapes and seals a second, which exercises §79's rule 2 — one subject
per pass, oldest first; then `skynetd once` runs in a loop until §79 goes `Idle`. **50 checks.** Both
windows complete, a receipt beside each naming the digest the recorder sealed, and the object in the
bucket **byte-identical** to the file on the robot's disk, with the two digests computed by two
implementations that share no code.

With the transport proven, the fault path closed the claim §73 had carried unproven:
`skynet_incidents.artifact_id` is filled by the digest the incident named — asserted as filled **by
the right window**, the artifact's own `sha256` against the digest inside the incident, because a
join on any window this device had sent would look identical and be wrong. The fault needed no
hardware: §38.1 supplies one, a profile whose genuine signature was made over other bytes.

### The first defect: curl appends a filename to a URL that ends in a slash

`--upload-file` has a documented behaviour that is a footgun here and nowhere else: if the URL ends
with a slash, curl appends the local file's name. The local file is the scratch slice, so the push
went to `…/recordings/sha256%3A<hex>/slice`. A slash-terminated URL is the **normal** answer from
the Hub this robot pairs with, and `advance` reuses `probed.effective_url`, which is right. What it
produced is §79.7's signature: the probe answered, the push 404ed, the window stayed pending, and
every surface said the robot was trying. The fix is in `curl_push` rather than in `advance`, because
it is a property of curl and not of §79.

### The second, found while looking at the first, and larger

`connectivity::get_json` had **no `--location`**. Its sibling `post_json` has carried one for
sessions. Its two callers are §11's desired state and §12's release list — so a robot paired to this
Hub could not be told what to run and could not be told there was an update, reported honestly and
uselessly as *the Hub answered HTTP 308*. What hid it is worth more than the fix: `get_json`'s own
doc comment said *"there is no body, so there is no file. That is the whole of the difference
between the two."* Two functions side by side, described as differing in one way, differing in two.

Both fixes were proven load-bearing by reverting them — `upload_target` reverted, 7 checks fail with
the exact 404 on the exact invented path; `--location` reverted, the desired-state check fails and
nothing else does.

One absence is asserted rather than left silent: §74's telemetry does **not** cross in this harness,
because `Counting` lives in memory on the `Daemon` and each `skynetd once` is a new process. That is
right rather than missing — §74.2's window carries a monotonic interval, which does not survive a
process boundary, and a window whose bounds came from two processes would be a fabricated duration.
Bytes on a disk outlive a process; an interval does not, which is why `report_upload` belongs in the
`once` path and `report_telemetry` does not.

### Numbers

`cargo test --workspace` — **1,722 passed**. `e2e.sh` 98 checks, 0 failed. `skynet-verify-recordings-robot.mjs`
— **50 checks, 0 failed**, new. clippy and fmt clean, corpus and self-check green, every check tool
green, both mirrors re-pinned.

### What it leaves

The gap that closed was between two halves; the one inside it is narrower and named. The daemon ran
**single passes**, not the supervised loop, and **§10's pairing did not run** — both waiting on a
Linux host that can reach the Hub, and neither a missing mechanism.

### Findings, restated

1-147 as before.

148. **Two components that have only ever met doubles have not met.** Both suites were green and had
     been for weeks, and between them sat a robot that could not upload a window and could not read
     its own desired state. The doubles were not wrong about the protocol; they were silent about
     the one thing a real receiver had an opinion on.

149. **Two functions described as differing in one way differed in two.** The sentence that named
     the difference is what stopped anybody looking for a second one.

---

## 2026-09-09 (seventy-seventh) — the two commands §41.3 specified, and a black box nobody could read

Asked to finish the OS and make the Console usable. The audit two sessions earlier said what was
left was a vendor's, a machine's or the owner's; that was true about the *list* and it was answering
the wrong question. The right one is **not what is unbuilt, but what somebody standing in front of a
stopped robot would reach for**, and three things came out of it — two of them specified and
missing.

### `skynet doctor`, specified since Part IV and not a command

§41.3: *"one command producing one bundle … so that the answer to 'what is wrong with this robot' is
a file rather than an interview."* `skynet doctor` printed *"`doctor` is not a skynet command"*,
which made that paragraph false in the way that matters — the answer **was** an interview.

It is an aggregator and holds no second opinion about anything: every section is a route on §33.7,
because a `doctor` that derived its own view of §7.2's guard partition would be a second
implementation of it. Twelve sections — §41.3's nine plus §27's state, §33.6's health and the
window's shape. **A route it could not read is recorded as unread, with the reason**, so a run with
no credential produces a bundle of one section and eleven refusals each naming the role it wanted,
which is a readable document rather than a failed run.

Two things it got wrong were found by running it against a robot rather than by reading the routes:
it counted a `guards` array with a `state` field that `GET /limits` does not have and printed
**"0 of 0 active"** for every robot, and the queue depth was the same mistake one route over. Both
are correct-looking output derived from a key nobody checked existed.

### `GET /blackbox/events`

§41.3 asks for the last N black box events and nothing could produce them: `GET /blackbox` reports a
window's **shape**, and a shape does not answer *what happened just before it stopped*. Gaps are in
the list, in their place — a `Gap` is not an `Event` in the type system and omitting it would present
a continuous list over a discontinuous recording, two events either side of four hundred dropped
records reading as consecutive. `held` is reported beside `returned` because *"the ring holds 12,000
and you are looking at 50"* and *"the ring holds 50"* are different robots. Observer rather than
operator, and the route sits **above** the download arm in both the router and the authorisation
table — asserted in both, because if it ever stops doing so `events` is read as a file name and
silently needs the wider role.

### Reading a sealed recording, which nothing had ever done

The strangest of the three. §13's seal has written MCAP **since the sixth session** and no code
anywhere had ever read one back — so every question about a window was a question for the robot that
produced it, which works until the robot is the thing that is broken. Evidence nobody can open is an
anecdote with a checksum.

`skynet-blackbox::read` is the reader and `skynet blackbox inspect <file>` the command — the one
robot command that talks to no robot. It stops short of replay deliberately. Three properties kept:
nothing trusts the file, every length checked against what is left; **truncated and damaged are
different answers**, because a transfer that stopped is what §79 resumes and a record running past
the end is a file to replace; and unknown opcodes are skipped by length and counted, so a reader from
one edition can open a file from a later one.

Two defects, both from pointing it at a real 200-cycle recording rather than a fixture: every topic
listed **twice**, because MCAP repeats channels in the summary section, and an empty metadata value
printing as a blank where the seal writes every key whether or not the robot had something to put in
it.

### Numbers

`cargo test --workspace` — **1,728 passed** (1,722 before). `e2e.sh` **119 checks** (98 before), 0
failed. clippy and fmt clean, corpus and self-check green, golden path green at 54 commands,
`scale-report --check` green at 127,000 lines.

### Findings, restated

1-149 as before.

150. **A bundle must record the sections it could not read, with the reason.** A robot answering 403
     for want of a credential and 503 because its recorder is not running is telling you something;
     omitting those sections, or writing them as empty objects, erases the two most interesting facts
     in the file.

151. **Evidence nobody can open is an anecdote with a checksum.** A format written for years and
     never read back is not a capability, and the gap is invisible from the writing side.

---

## 2026-09-09 (seventy-eighth) — a window that could be read and not replayed, and two defects only a real recording had

The audit first, and it corrected three things this log had been wrong about.

* **`cargo test --workspace` did not build on Windows.** `skynet-ring-shm` states, over the block
  that exists for it, *"a stub rather than a compile error, so `cargo test --workspace` works on a
  machine that is not the robot."* `Mapping::words` has that stub; `bytes_slice`, added with the
  mapping half, does not, and `skynet-frames` calls it from ten ungated places. **CI's `windows`
  job — which exists because of a break of exactly this shape — was red.** The previous entry said
  clippy and fmt were clean; both had been run on Linux only.
* **`cargo fmt --all --check` was not clean either** — fourteen hunks across five files, all of them
  the previous session's own new code.
* **Replay was not blocked, and most of it was already written.** `crates/skynet-blackbox/src/replay.rs`
  has evaluated recorded cycles against `ResolvedLimits` for sessions, and its own doc comment says
  it takes records rather than a `Ring` *"so that a recording read from a sealed file … replays
  through the same function."* Nothing joined them, and no FFI decision stood between them.

### What the file could not say

Reassembly is the inverse of §26's *one cycle becomes four messages*, and the inverse did not exist.
Three of `replay()`'s inputs reached the ring and stopped there: `capability`, `nearest_person_m` and
§26.4's `read_result`. Two of them the kernel **evaluates**, so a replay missing them does not fail —
it recomputes a different verdict and presents the difference as the kernel disagreeing with itself,
which is the worst available failure for a tool whose purpose is to say whether it still agrees.

They are in the schema now, in all three editions, and an older recording is neither unreadable nor
silently wrong: the missing keys become the ring's own *not known* values and `Assembled` counts the
cycles that lacked each, so a caller says *this window predates the field*. The wire fixture caught
the consequence before a person did — the schemas are embedded in every sealed file, so three added
fields moved every window's digest and `reference/wire/incident.json` stopped matching its pin.

**The join is by sequence, and the obvious version is wrong on the file that matters.** Reading it as
the same index in each channel fails on a transfer cut between two of a cycle's four messages: the
channels stay one apart for the rest of the window and every cycle after the cut is assembled out of
two different ones, each field plausible. Matched on the sequence, that cut is one reported
`Incomplete`.

### Then it was pointed at a real recording, and both halves were wrong

Two processes on Linux sealed a 300-cycle window. Replaying it reported **300 of 300 diverged**, with
every unit test in the repository green.

**One: it evaluated cycles the robot had never asked the kernel about.** §25's loop returns before
`evaluate_cycle` when the policy has posted nothing, recording a mask of zeros and an all-`NaN`
`desired`. The first fix was also wrong, and the real recording said so too — reading *nothing
posted* as *`desired` is empty* collapsed the window to two evaluated cycles, because a policy
publishing a **hold** records an all-`NaN` `desired` as well and the kernel did answer about those.
§24's sequence separates them: zero is *no command has ever arrived*. `unposted` is 167 and the
loop's own summary says `idle 167` — two numbers from two processes that share no code.

**Two: `GET /limits` cannot replay anything.** `evaluate_cycle` reads `watchdog_ms` off the declared
**envelope**, and `limits_to_json` does not serialise one. So the guard that decides nearly every
cycle on a bench had no window to measure staleness against, and two recorded `stop_cat2` verdicts
came back as `none` — **a replay saying a robot kept going where the recording says it stopped.**
`skynetk` now writes the limits it ran under beside every window it seals, which closes a hole larger
than the field: nothing else on the machine knows them.

Where a document cannot reconstruct a guard's input the refusal lives in the **replay**, not the
reader — whether an absence matters is a fact about the window, not about the document. `mode_gate`
is active on an ordinary robot and does nothing at all on a cycle that exercised no capability, so
refusing in the reader refused every real document.

`skynet blackbox replay` is the CLI's second documented exit-code exception: **0** reproduced, **2**
ran and did not, **1** could not run.

### Numbers

`cargo test --workspace` — **Windows 1,652 · Linux 1,739** (1,641 / 1,728 before). `e2e.sh` **129
checks**, 0 failed (119 before). clippy and fmt clean **on both hosts**, which they were not at the
start of the session. Corpus 1,142 of 1,142 in both editions, golden path green at 55 commands,
every check tool green, both mirrors re-pinned.

### Findings, restated

1-151 as before.

152. **A component that has only ever met a fixture has not been tested.** The reassembler had a
     round-trip test that passed, against a fixture built by the same hand that built the reader. The
     first real recording found two defects under it, one of which said a robot kept moving through a
     stop.

153. **Clean on one host is not clean.** A stub added for a non-robot target and not extended to the
     next function that needed it turned the workspace red on the machine this repository is
     developed on, while every tool reported green on the other.

154. **A replay must refuse the cycles nobody asked the kernel about.** Evaluating them produces
     divergences that belong to the tool, and a tool whose whole output is *the kernel disagrees with
     itself* cannot afford to manufacture one.

---

## 2026-09-11 (seventy-ninth) — the Python edition verified, and a C++ edition stopped at 899

Two things were asked: whether Hub, Console and OS are built to their plans, and then — given that
answer — a second edition of the OS in another language, with the Python edition verified first.
This entry records both, and **the second is unfinished and the working tree is red**; the state is
written down exactly as it stands rather than tidied.

### The audit, in one paragraph each

**Skynet OS** is built to §73's *what version one covers* table except where the remaining work needs
a robot: nothing runs a calibration (§45), and nothing can, because a procedure needs a driver, a
target and a pose set and §45.3 signs the result. **Skynet Hub** is at its plan's §33 log — 96 routes,
22 tracked migrations — with four items open and all four blocked on an owner's decision rather than
on code (account erasure, who may decide an abuse report, retention of sealed windows, collections).
**Skynet Console** is at its fifth owned layer and **cannot be built or tested on this machine**: its
target is `net9.0` and only .NET 8 runtimes are installed, no SDK. That is a fact about the machine,
not about the Console, and it is stated here so no later entry reads its silence as a pass.

Two numbers in this log were platform-specific and read as absolute: the test counts. ~87 tests are
`cfg(unix)` or `cfg(target_os = "linux")` gated, so **1,825 is Linux and 1,738 is Windows** for the
same commit. Both are correct; neither is *the* number.

One live documentation defect, unfixed: §73's transport row says *"no policy runtime exists to carry
a command as far as a guard"*, which the **Skills** row in the same table contradicts. The sentence
was true when written and the table outgrew it.

### The Python edition passes, and its README does not say so

`run_corpus.py` — **1,148 of 1,148 cases, 0 failed, 0 not implemented, 33 of 33 corpora**.
`self_check.py` — **44 checks, ok**. The edition is complete at L2 and has been for some time.

Its README opens with *"It passes the corpus: 1,109 of 1,109 cases, 31 of 31 corpora"* and repeats
the pair three more times. **A README that states a passing number is a claim that ages**, and this
one aged by 39 cases and two corpora with nothing in the repository checking it. `run_corpus.py`
prints the true number on every run; the document beside it printed an old one. Left as found, and
listed below.

### The C++ edition, at §42's L2

§31.3 puts C++ at a **Certified** ceiling with a **Full** standing and gives the reason — *"largest
population in robotics; ROS 2 is C++"* — so it is the edition worth having next. It is written from
`reference/typescript/*.ts` only: never from `corpus/`, which would agree by construction, and never
from the Python edition, which would prove only that one hand did both.

**The foundation was proved against the oracle before one contract was built on it.** §31.4 is
byte-identical canonical JSON or nothing, so the first executable thing in the edition is
`tests/json_roundtrip.cpp`: parse all 1,148 `expected` strings out of the corpus and re-serialise
them, requiring identical bytes. **1,161 checked, 0 failed.** It also settled a design that would
otherwise have been found much later and much deeper: the obvious object container keeps its keys
sorted, and it is wrong here. **The reference iterates objects in insertion order and several
contracts' output depends on it** — `evaluate_cycle` appends one `clamped` entry per channel it
reduced, in the order it walked them. Storage order and write order are two different things; the
container keeps insertion order and the *writer* applies §31.4's sort.

Under it: `sha256.cpp` against the FIPS 180-4 vectors; `kinematics.cpp` carrying the kernel's own
`sin`/`cos`/`sqrt` (§31.2 single-source, fdlibm coefficients, Cody–Waite reduction) rather than the
host libm; `js.cpp` for the divergences a host language does not share with the reference —
`Number::toString` going exponential outside 1e-7…1e21, `toFixed` breaking ties away from zero on the
exact binary value, UTF-16 code-unit ordering, `null > n` coercing to `0 > n`; `vocabulary.cpp` with
all **74** closed sets transcribed one file at a time, each with the reference file it came from.

Three things the gates caught that review had not:

* **A value computed and never stored.** `parse_role` derived the ordinal into a local
  `std::optional<double> index` and never assigned it to `parsed.index`. Both spellings type-check;
  three `roles` cases did not.
* **`-Werror` deleted a helper.** A `finite()` in `cell.cpp` had been superseded and left behind;
  `-Wunused-function` refused the build. §6.1's second-strongest gate doing exactly its job, and
  R-10 applied by the compiler rather than by memory.
* **Exit 127 from a binary that built cleanly** — the MinGW runtime DLLs. `-static` in `LDFLAGS`.

### Where it stopped, and what is red

| | |
|---|---|
| Corpus | **899 of 1,148 passed, 0 failed**, 249 not implemented — all 249 in corpora with no handler, **0 in a file a handler declined** |
| Corpora | **27 of 33** complete |
| Round trip | 1,161 checked, 0 failed |
| Size | ~11.0k lines of `.cpp`, ~1.2k of headers |

Complete: birth, calibration, cell, certification, cycle, digest, embodiment, frames, fuzz, intent,
kinematics, limits, live, media, observation, opstate, power, predicate, provenance, readiness, roles,
semver, telemetry, transfer, transport, vocabulary, watchdog.

Remaining: **identity (47), checkin (25), admission (40), blackbox (22), ota (29), skill (86)** — 249.

**The tree does not compile.** `src/contracts/identity.cpp` (462 lines, §10/§19/§50 — pairing-code
normalisation, credential state, and §10's attestation sentence) is written and registered in the
dispatch table, and its shared constants and helpers sit in an anonymous namespace inside
`skynet::identity` while the contract that needs them is in `skynet::contracts`. File-local is not
scope-local; three errors, and more behind them. The fix is to hoist those names to a scope both
namespaces see. It was not applied — the session was stopped here deliberately, and the half-built
state is left visible rather than reverted.

`power.cpp`, `calibration.cpp`, `identity.cpp` and `identity.hpp` are **uncommitted**; everything
else is on `main` at `6adad88`. Nothing is pushed.

### Open, and small

1. `editions/python/README.md` still claims 1,109 of 1,109 and 31 of 31. It is 1,148 and 33.
2. `editions/cpp/Makefile`'s ratchet `MIN_PASSED ?= 840` is below the 899 actually reached.
3. `editions/cpp/tests/self_check.cpp` is referenced by the `test` target and does not exist.
4. `editions/cpp/README.md` does not exist.
5. `tools/edition-report.mjs` invokes `python3`, which is not the interpreter's name on Windows, so
   the report tool cannot run the Python edition on the machine this repository is developed on —
   the same shape as finding 153.
6. The C++ edition is **not** in `tools/edition-report.mjs`'s `EDITIONS` list and must not be until
   it is at 1,148. The tool already refuses any edition with a non-zero unimplemented count, which is
   §31.4 enforced rather than described: *an edition that has not passed the corpus is not an
   edition.*
7. §73's transport row, above.

### Findings, restated

1-154 as before.

155. **Prove the serialiser against the oracle before building anything on it.** A canonical writer
     is the one component every case in the corpus passes through, and 1,148 round-trips cost an hour
     and dictated the object container's design. Found after twenty contracts, it is twenty rewrites.

156. **Storage order and write order are different things.** The reference iterates objects in
     insertion order; §31.4 writes them sorted. A container that does both by sorting on insert is
     correct on every case where the two agree and wrong wherever a contract's output is built by
     walking a map.

157. **A differential corpus catches what a type checker cannot.** A value computed into a local and
     never assigned to the field it was computed for type-checks, reviews clean, and fails three
     cases. Typing rules out a category; the oracle rules out the value.

158. **A README stating a passing number is a claim with no gate behind it.** The Python edition's
     drifted by 39 cases and two corpora while the runner beside it printed the truth on every run.
     A number that is not checked is a number that is eventually false.

---

## 2026-09-11 (eightieth) — the C++ edition at 1,199, and three editions brought back to one whitespace

Asked to continue the editions from the plan, log the progress, and keep each edition up to date with
Rust as the main one. The C++ edition the seventy-ninth entry left red at 899 is complete; and making
"up to date with Rust" mean something turned up a drift in **all three** editions — Rust included —
that the corpus had no input able to see.

### Where it stands

| | Before | After |
|---|---|---|
| Corpus | 1,148 cases | **1,199** (+51, every one a divergence probe) |
| Rust (reference) | 1,148 / 1,148 | **1,199 / 1,199**; workspace **1,740** on Windows, fmt and clippy `-D warnings` clean |
| Python | 1,148 / 1,148 | **1,199 / 1,199**; self-check **47** |
| C++ | 899 / 1,148, tree not compiling | **1,199 / 1,199, 33 of 33 corpora**; round trip 1,212 / 0 failed; self-check **20** |
| `editions.json` | 2 editions, runnable on Linux only | **3 editions × 1,199**, runs on Windows, synced into the Hub |

### One whitespace set, and nobody was using it

Every contract trims before it decides, and `trim()` removes a set no other language's trim does:
U+FEFF goes (Rust, Python and Go keep it), U+0085 stays (all three strip it), U+00A0 goes (an ASCII
strip keeps it). Rust's own `semver.rs` had written the rule down — *"trimming uses JavaScript's
whitespace set … a divergence nobody would find for years"* — and it was applied in **2 of Rust's 12**
files that trim, **3 of Python's 11**, and **4 of the 8** the C++ edition had written. The corpus held
no BOM, NEL or NBSP at a trim site, so all 1,148 cases passed everywhere.

42 cases now put two probes at every trim site in every family (roles, semver, predicate, identity ×3,
check-in, cell, live, intent, OTA, skill, transfer ×4). The probes are designed against each other:
U+FEFF U+00A0 must be removed and holds both characters, so an edition stripping only one still
fails; U+0085 must be kept and is **alone**, because a pair would hide a partial stripper — Rust
removes U+0085, stops at whatever follows, and agrees by accident. First run: **Rust 10+ failures,
Python 26, C++ 10** — the table's prediction, edition by edition.

Resolved toward the rule Rust had documented, in every edition, each with a gate rather than a
sentence:

* **Rust** — `js.rs` holds `js_trim`/`is_js_space` (moved out of `semver.rs`); 34 sites across 10
  files converted; `crates/skynet-contracts/clippy.toml` refuses `str::trim`, `trim_start`,
  `trim_end`, `split_whitespace`, `char::is_whitespace` and the ASCII case methods. The gate was
  written first and seen to flag all 36 sites before any was fixed.
* **Python** — `js_trim` at 15 sites in 8 modules; `self_check.py` walks the token stream and refuses
  any `.strip()` with no argument. The whitespace set itself is now written by code point: it used to
  be 25 characters, most of them invisible, and the edit tool could not even match it.
* **C++** — `js::trim` everywhere; the ASCII `text::strip` and `text::ascii_lower` are **deleted**,
  so reaching for one is a compile error.

### Case, and `\d`, `\s` and `$`

Three more of the same kind, each confirmed by running before a case was written:

* **Case mapping lands inside ASCII grammars.** `toLowerCase()` turns U+212A KELVIN SIGN into `k`, so
  `nec`+U+212A+`.link.pitch` is the role `neck` and `s`+U+212A+`d_…` is a credential. Rust ASCII-lowercased
  the credential prefix "because a full mapping would fold characters no credential prefix contains",
  which this character disproves; C++ ASCII-lowercased everything. Both now use the full mapping.
* **C++ had no case mapping at all**, and `ß`→`SS`, `ﬁ`→`FI` were two corpus failures. The tables
  are **generated from the engine the reference runs on** (`editions/cpp/tools/unicode-case.ts`,
  Unicode 17.0, 1,580 upper / 1,488 lower), with 219 whole-string vectors from the same engine. The
  vectors earned their place on the first deliberate break: final sigma's definition read literally
  counts a character that is both Cased and Case_Ignorable (U+02B0) as the preceding letter; the
  engine steps over it. The self-check names the string.
* **Python's regex classes are not JavaScript's.** `\d` matched Arabic-Indic digits (`١.٢.٣` parsed as
  a version; `#١` as ordinal 1), `\s` was Unicode whitespace, and `$` matched before a final newline
  (`list.0\n` indexed element 0). Eight patterns now spell `[0-9]`, `JS_SPACE` and `\Z`; the same
  token scan refuses the old spellings. 8 cases pin it.

### The reference edition had one of its own

`text_or_null` in Rust's `identity.rs` tested the trimmed digest for blankness and put the
**untrimmed** one on the wire — so a check-in's attestation carried a digest no fleet query could
match, while the sentence beside it had trimmed it. Python and C++ trimmed. One case
(`checkin/document :: attested-digests-in-whitespace-only-javascript-trims`), then the fix.

### The C++ edition, finished

Fixed the identity file's scoping (an unnamed namespace one level too deep) and wrote the five
remaining contracts from the reference: **ota** (31), **checkin** (28), **admission** (40), **skill**
(88), **blackbox** (22 — the six MCAP schemas, the ring's records and header, and two sealed files,
all byte-exact on the first build). Along the way `truthy` was extracted from two copies into
`js::truthy`, the bare-number rule (`null` in a case is NaN, corpus/README.md) became one
`js::bare_number`, and `js::math_round` is JavaScript's `Math.round` — ties toward +∞, exact at
0.49999999999999994.

The Makefile could never have linked: its hand-written source list missed four translation units and
the `test` target named a `self_check.cpp` that did not exist. Sources are now derived from the tree,
objects build in parallel with compiler-recorded dependencies (≈12 s rather than ≈56 s), MinGW's
bogus default `CXX` is overridden, and the ratchet is 1,199. `tests/self_check.cpp` exists: runner
accounting, the case vectors, the helpers at their edges, and a scan refusing `<cctype>`.

### The report, CI, and the Hub

`tools/edition-report.mjs` invoked `python3`, which on Windows is the Store stub — it had never run on
the machine this repository is developed on. It now picks executables by host and keeps the printed
runner strings fixed, so the file is byte-identical from either. It lists the C++ edition, and the
Python runner's Windows crash on printing a BOM through cp1250 is fixed and has a regression check.

CI: the Python ratchet was 1109 against a real 1,148 — raised to 1199; `make -C editions/cpp check`
added to the corpus job; a `cpp-case-tables` job checks the tables under Node 26, because Node 24's
Unicode differs. `tsconfig.json` now includes `editions/**/*.ts`; the Hub's installed TypeScript
(7.0.2) type-checks all 33 files clean.

`reference/reports/editions.json` was synced into the Hub with `check-contract-mirrors --sync`: two
files there (`lib/skynet/os/reports/editions.json`, `mirror-digests.json`), plus two comments in
`lib/skynet/editions.ts` that a third edition made false. The Hub's editions and mirror suites pass,
29 of 29. The page was not opened in a browser; the change is data and the suites cover the join.

### Open

1. **Nothing is committed**, in either repository.
2. CI's Linux run of the new steps has not happened; everything above ran on Windows.
3. Two reference behaviours were reproduced rather than tightened, because tightening is a
   specification change: `admitPackage`'s signature `switch` adds no finding for a state its type
   does not contain, and `shapeMeasure` passes an unknown measure state through where all three
   editions read it as `unmeasured`.
4. A fourth L2 edition was not started. §42 puts the realistic L2 count at 3–4, and with the oracle
   there are four; the next row of §31.3 with a Full standing is Go (Guest ceiling).
5. `round_half_away` exists twice in the C++ edition (`observation.cpp`, `kinematics.cpp`).
6. From before: §73's transport row is still contradicted by the Skills row.

### Findings, restated

1-158 as before.

159. **A rule written down in one module is not a rule.** The whitespace set was documented in
     `semver.rs` and reached 2 of Rust's 12 trimming files; the other editions did no better. What
     makes it hold is the gate — a disallowed method, a token scan, a deleted helper.

160. **A probe can hide the divergence it was written to find.** A pair of must-keep characters lets
     an edition that strips only the outer one agree by accident. The kept probe has to be alone.

161. **A full case mapping lands inside an ASCII grammar.** "Every identifier is ASCII, so ASCII
     lowercasing is exact" was false twice: the reference maps before it checks, and `ß`, `ﬁ` and
     U+212A all map into ASCII.

162. **Testing a value and emitting it are two operations.** Rust trimmed a digest to decide it was
     not blank and sent the one it had not trimmed.

163. **A generated table needs vectors from the same generator.** The table maps code points and
     cannot be wrong about a string; the walker can, and only whole strings from the engine caught
     the one character class the specification's own wording gets backwards.

164. **A runner that crashes describing a failure hides the failure.** The first whitespace case to
     fail took the Python runner down on a cp1250 console, and the edition looked broken in a way
     that said nothing about the case.

---

## 2026-09-11 (eighty-first) — the Go edition begins, and one generator where there were two

Asked to continue implementing new language editions and to keep them up to date with Rust. The
eightieth entry's open item 4 named the next one — *"the next row of §31.3 with a Full standing is
Go (Guest ceiling)"* — and Go was not installed on this machine, which §31.4 makes a blocking fact
rather than an inconvenience: an edition nobody has run against the corpus is not an edition. Go
1.27.1 is now installed user-locally via scoop, and CI takes its own from `actions/setup-go`.

### Where it stands

| | Before | After |
|---|---|---|
| Rust (reference) | 1,199 / 1,199 | unchanged, verified |
| Python | 1,199 / 1,199 | unchanged, verified |
| C++ | 1,199 / 1,199 | unchanged, verified after the generator move |
| **Go** | — | **624 / 1,199, 19 of 33 corpora complete, 1 partial, 0 failed** |
| `editions.json` | 3 editions × 1,199 | unchanged — Go is deliberately absent |

Every step of `make -C editions/go check` is green: gofmt, `go vet`, the case-vector tests, the
canonical round trip over all 1,199 expected strings, the self-check, and the corpus at its ratchet.
The nineteen complete are birth, calibration, certification, digest, intent, media, observation,
opstate, ota, power, predicate, provenance, readiness, roles, semver, telemetry, transfer,
vocabulary and watchdog. `checkin` is **partial**: its claim and health files pass and
`checkin/document` is declined, because it needs the guard report, which needs `resolveLimits`,
which needs the frames and kinematics stack. Declined rather than half-answered — §31.4 wants a
corpus an edition answers *part* of to be visibly neither complete nor missing, and the runner
prints it that way.

All but two of the files passed on their first run. The two that did not were the same mistake
twice, and it is worth naming: `telemetry/window` and `ota/boot` put their subject at the *top
level* of the case input where their sibling files wrap it in a named field, and reading the wrong
one produced a plausible answer rather than an error — `not_pending` for every boot, which is what a
boot with no evidence legitimately gets.

Go is absent from `reference/reports/editions.json` on purpose. `tools/edition-report.mjs` refuses
any edition with a non-zero unimplemented count, which is §31.4 enforced rather than described, and
the tool was not softened to let a partial edition onto the page.

### The canonical writer, proven before anything was built on it

Finding 155 said to do this first, and it took twenty minutes rather than twenty rewrites:
`cmd/roundtrip` parses every `expected` string in the corpus with `internal/canonical` and
re-encodes it, and the bytes came back identical on the first run — 1,199 of them, carrying every
number shape, key ordering and escape the system actually emits, none of them chosen by the person
writing this edition.

### Two things Go spells differently that no other edition had to care about

Both are properties of the language rather than of the contracts, and both were designed out rather
than reviewed for.

**Map iteration is randomised.** The reference iterates objects in insertion order and several
contracts' output depends on it — `evaluateCycle` appends one entry to `clamped` per channel it
reduced, so that array's order is the document's. The C++ header records this costing a rewrite
(finding 156). In Go it would have been worse than a rewrite: a `map[string]any` edition emits the
same set of clamps *in a different order on every run*, which is a flake rather than a stable wrong
answer. `internal/canonical` keeps objects as a slice of members and sorts only at the writer.

**A field that is absent is not a field that is null.** Rule 5 of the canonical form, and the whole
of `predicate.ts`'s three-valued design. Go's zero value cannot carry the distinction, so it is
carried explicitly: a `Set` that was never called for the writer, `maybe` and `operand.resolved` for
the evaluator. An edition collapsing the two answers `satisfied` where the reference answers
`unevaluable` on all five inputs the reference's own header table lists — and all five are in the
corpus.

### The near-miss standard library, and the gate that refuses it

The reason this edition needs an `internal/js` package is that Go has a function for nearly
everything the reference calls, and *nearly* is the problem.

| The reference calls | Go's near-equivalent | Why it is not the same |
|---|---|---|
| `trim()` | `strings.TrimSpace` | Keeps U+FEFF and strips U+0085. ECMAScript does the opposite. |
| `\s` in a pattern | `\s` | Five characters in Go, twenty-five in ECMAScript |
| `toLowerCase()` | `strings.ToLower` | The simple mapping, and no final-sigma rule |
| `toUpperCase()` | `strings.ToUpper` | Leaves `ß` and `ﬁ` alone where the reference gives `SS` and `FI` |
| `Math.round` | `math.Round` | Halves away from zero; `Math.round` goes toward +∞ |
| `Number(string)` | `strconv.ParseFloat` | `"0x1f"` is 31 there and an error here; `""` is 0 there and an error here; `"1_000"` is NaN there and 1000 here |
| `` `${n}` `` | `strconv.FormatFloat` | ECMAScript reaches for an exponent outside 1e-6..1e21; rule 3 never does |
| `JSON.parse` | `encoding/json` | Loses insertion order, and cannot tell absent from null |

`cmd/selfcheck` walks the AST and refuses every one of them outside the files that define the right
spelling — §6.1's fourth gate, for a convention no type checker expresses. It found **four real
violations on its first run**, and one was a defect rather than a style slip: `roleKey` interpolates
the ordinal into a template literal, and ECMAScript's `Number::toString` writes `1e+23` where rule 3
writes twenty-four digits. `js.NumberToString` is that function and is deliberately *not*
`canonical.FormatNumber`; the two exist side by side because the wire and a string are different
destinations. The exemption list names files rather than directories and prints its own length on
every run (§6.3).

`js.StringToNumber` earned its place immediately: `corpus/predicate/evaluate.json`'s context carries
`"0x1f"`, `"1e3"` and `"Infinity"` in adjacent fields, because every operand of a numeric comparator
goes through `Number()`.

### One derivation, two renderings

`editions/cpp/tools/unicode-case.ts` moved to `tools/unicode-case.ts` and gained a Go renderer.
Copying it beside the second edition would have put the derivation — which code points map, which
characters the sigma rule reads, which eight representatives the vectors are built from — in two
places, to agree on the day they were written and diverge at the next Unicode version, in whichever
edition was regenerated second (§3.3).

The move is provably behaviour-preserving: the C++ tables and vectors are byte-identical apart from
the one header line naming the generator's new path, and the C++ edition still passes 1,199 of
1,199. CI's `cpp-case-tables` job is now `case-tables` and checks all four generated files.

### One recipe that presented as a hung build

The Makefile's `fmt` target ran `go run cmd/gofmt`, falling back to `gofmt -l .`. `cmd/gofmt` is a
real command path in the standard library, so Go did not error — it started building the formatter
from source, and the target sat there for a quarter of an hour producing nothing. `gofmt` ships
beside the compiler and is now invoked directly.

### Open

1. **14 corpora remain**, and three of them are the expensive ones: `safety.ts` (limits and cycle,
   1,144 lines, and it needs the frames and kinematics stack first), `embodiment.ts`'s transport,
   and `skill.ts`. Those three unlock most of what is left: the frames/kinematics stack also
   unblocks `checkin/document` and `cell`, and transport and cycle unlock the 148 `fuzz` cases,
   which is the largest single lever remaining. `admission`, `identity`, `blackbox` and `live` are
   independent of all of it.
2. Go is not in `tools/edition-report.mjs` and must not be until it answers every case.
3. CI's Linux run of the new `make -C editions/go check` step has not happened; everything above ran
   on Windows.
4. From before: §73's transport row is still contradicted by the Skills row, and `round_half_away`
   exists twice in the C++ edition.

### Findings, restated

1-164 as before.

165. **A convention gate pays for itself on the run that installs it.** The AST scan refusing Go's
     near-miss standard-library calls found four violations in twelve files the moment it existed,
     and one of them was a real divergence nobody had looked for. Writing the gate first is cheaper
     than writing it after the thing it would have caught.

166. **Two functions that turn a number into text are two functions.** `canonical.FormatNumber` is
     rule 3 and never exponential; `js.NumberToString` is ECMAScript and exponential outside a
     window. They differ on exactly the inputs nobody tests, and an edition carrying only one of
     them is wrong in one of the two places.

167. **A language's convenience can turn a wrong answer into a flake.** The other editions' risk
     from sorted-on-insert objects was a stable divergence a corpus case pins. Go's map would have
     produced a *different* order per process, which no corpus case can pin and no bisect can find.

168. **A toolchain that is not installed is a blocking fact, not a detail.** §31.4 makes running the
     corpus the definition of an edition, so "write the Go and someone will run it" is not a plan
     that ends in an edition.

169. **A handler that reads the wrong field answers rather than failing.** Two corpus files put
     their subject at the top level where their siblings wrap it in a named field, and reading the
     absent field produced a well-formed verdict over an empty document — `not_pending` for every
     boot. Nothing threw, and the answer was one the contract legitimately gives. The corpus caught
     both on the first run, which is the argument for comparing bytes rather than for trusting a
     handler that returned something.

---

## 2026-09-11 (eighty-second) — the Go edition at 1,199, and the multiply-add two editions were fusing

Asked to continue the language editions if the plan still has one to write, keeping each up to date
with Rust. It had exactly one in flight: §42 puts the realistic L2 count at three to four, and the
fourth, Go, stood at 624. It is finished — and finishing it turned up a defect in **two** editions
that no corpus run on this repository's machines could ever show, because it lives in instructions
an amd64 host at its default level does not have.

### Where it stands

| | Before | After |
|---|---|---|
| Rust (reference) | 1,199 / 1,199 | unchanged, re-run |
| Python | 1,199 / 1,199 | unchanged, re-run; self-check 47 |
| C++ | 1,199 / 1,199 | 1,199 / 1,199 **with contraction off**; self-check 20; new `fusion` step |
| **Go** | 624 / 1,199, 19 of 33 corpora | **1,199 / 1,199, 33 of 33**; round trip 1,199; `make check` six steps green |
| `editions.json` | 3 editions × 1,199 | **4 editions × 1,199**, synced into the Hub |

Every figure above was produced by running the edition this session, not read from a file. Go's
remaining corpora — kinematics, frames, limits, cycle, transport, the check-in document, identity,
admission, cell, live, skill and blackbox, with the fuzzed cycle, transport and observation files —
are ~6,300 lines under `editions/go/internal/`, one file per reference module, and every file
reproduced its corpus on the first run: the poses to the last bit, the 104 transport plans, both
sealed MCAP files by SHA-256.

With it, §42's L2 row is at its upper bound. The next §31.3 row with a Full standing is **C**, and its
case is the safety-MCU build — an L4 decision §42 says "should be driven by a named customer's safety
case rather than by preference". That is the owner's to make, so no fifth edition was started.

### The multiply-add the compilers were allowed to fuse

§31.2 makes the kernel's sine, cosine and square root a fixed sequence of correctly rounded
operations, so that every edition — and both channels of a certified-tier pair — computes the same
bits. A fused multiply-add is a different sequence: it computes `x*y + z` without rounding `x*y`.
Rust never contracts; neither can Python or the TypeScript oracle. Go and C++ both may.

**Go.** The specification permits fusion *"possibly across statements"*. Measured before anything was
written: `x*y + z` fuses on arm64, amd64 at `GOAMD64=v3`, riscv64, ppc64le, s390x and loong64, and on
arm64 and amd64 v3 so does `p := x*y; p + z` (the other four were not measured for that). So the gate
reads the compiler, not the source: `cmd/selfcheck` cross-compiles the edition for all six with
`-gcflags=-S` and fails on any fused instruction, and `internal/fusionprobe` fuses on purpose so each
target proves the scan can see one. **It found a live defect on its first run**: `micros` passes
`ms * 1000` to `roundHalfAway`, the compiler inlines it, and on arm64, ppc64le and loong64 the
fraction was computed from the unrounded product — the half-way points that function exists to
decide. Fixed by the specification's own remedy, an explicit `float64(v)` in the helper. The new
geometry was written with `float64(a*b)` wherever a product meets a sum, and the scan confirms none
of it fuses.

**C++.** GCC's default for C++ is `-ffp-contract=fast`. With FMA available — every aarch64, a Jetson
included — `kinematics.cpp` compiled to **ninety** fused instructions; with `-ffp-contract=off`, none.
It passed the corpus everywhere it had run because baseline x86-64 has no FMA to contract into. The
flag is now `FP_FLAGS` on every compile, in its own variable so overriding `CXXFLAGS` cannot drop it,
and a `fusion` step in `check` compiles every unit with `-mfma` and refuses a fused instruction, with
`tests/fusion_probe.cpp` required to fuse. Removing the flag makes the step fail naming
`kinematics.s`; that was run, not assumed.

What this does **not** establish is a corpus run on arm64 hardware: none is available here. The scans
prove there is no fused instruction in the compiled output for those architectures, which is the
mechanism by which the results would differ; they are not a run.

### Two things earlier entries had wrong

* **`checkin/document` never waited on `resolveLimits`.** The eighty-first entry declined it for want
  of the guard report and the frames stack. `checkIn` carries the report through unchanged —
  `limits: input.limits ?? null` — and what the file waited on was `identity.ts`'s `attest`.
* **The C++ edition's "`round_half_away` exists twice" is not duplication.** `observation.ts` and
  `kinematics.ts` each define one and they are different functions: at 0.49999999999999994 one returns
  0 and the other 1. The Go edition keeps them as `roundHalfAway` and `quadrantRound`, with the reason
  in both. Merging them would pass every case the corpus holds today.

### A formatter that rewrote meaning

gofmt's doc-comment reformatting turns a pair of apostrophes into a closing quotation mark, so a
comment quoting the reference's empty-string default came out quoting something else. Three times
this session and twice in files an earlier session wrote. All five are fixed, and the source scan now
refuses a typographic quote in any comment, which is the only trace the rewrite leaves. (The same
pair of apostrophes also breaks this shell's heredocs, which is why this entry was not appended with
one.)

### The report, CI and the Hub

`tools/edition-report.mjs` lists Go (`go -C editions/go run ./cmd/corpus --json`), and the report
regenerates to four editions and passes `--check`. CI's comments had described two editions and an
incomplete Python one, and said Go "must not be" in the report — all corrected, with the fusion
reasons on the C++ and Go steps; the YAML was parsed to confirm it still loads. The header's "four
jobs, the third" is six jobs, `corpus` the fourth.

Synced into the Hub with `check-contract-mirrors --sync` (`lib/skynet/os/reports/editions.json`,
`mirror-digests.json`), and one comment in `lib/skynet/editions.ts` extended to Go. One Hub test went
red for the best reason: it used Go as its example of an **absent** edition. It now asserts the
property — every absent row is null, never a zero — over whichever rows are absent, instead of over a
named language. Hub suites 29 of 29. `/skynet/os/editions` was opened in the browser against the dev
server: the Go row renders as measured, "5 of 9 editions … exist as running code", no console errors.

### Open

1. **Nothing is committed**, in either repository. The Hub's working tree also still carries the
   eightieth session's uncommitted sync, now superseded by this one.
2. CI's Linux run of the new steps has not happened. The Go fusion scan cross-compiles the standard
   library for six architectures on a cold cache, which will add a minute or two to the `corpus` job.
3. No edition has been run on arm64 hardware (above).
4. From before: §73's transport row is still contradicted by the Skills row.
5. `make` is not on this machine's Git Bash path; `gmake` from Strawberry Perl is, and is what ran the
   Makefiles here.

### Findings, restated

1-169 as before.

170. **A clean corpus run proves the bits on the machine that ran it.** Two editions were fusing
     multiply-adds that §31.2 forbids, and every run passed, because the host had no FMA instruction
     to fuse into. A property of the compiled output has to be checked in the compiled output, for the
     architectures the edition is meant to run on.

171. **A scan that finds nothing needs a positive control.** "No fused instruction" and "a pattern that
     no longer matches this compiler's mnemonics" print the same line. Both fusion gates compile a probe
     that must fuse, per target, so an empty result is evidence rather than silence.

172. **Fusion happens after inlining, where no source rule can follow it.** The Go defect was neither
     `x*y + z` nor a product on the line above an add; it was a product passed into a helper that the
     compiler inlined and then subtracted from.

173. **A blocker in a log is a claim like any other.** `checkin/document` was declined for a
     dependency it never had, and reading the function would have shown it in one line.

174. **One name in the reference can be two functions.** An open item recorded a duplicate to merge;
     the two differ on the largest double below a half.

175. **A test pinned to a fact about progress fails when the progress happens.** The Hub asserted Go
     was absent; the property it meant was that absence renders as null.

176. **A formatter can change what a comment says.** gofmt rewrote a pair of apostrophes into a
     quotation mark five times across two sessions, each a comment that then misquoted the line it
     explained. The gate is a scan for the trace it leaves.

## 2026-09-12 (eighty-third) — the four §31.3 rows that had no code, and §31.3 now fully implemented

Four rows of §31.3's edition matrix had no implementation at the start of this session: **C**, **Zig**,
**Java / Kotlin** and **C# / .NET**. All four now exist at §42's **L2 — Contracts**, each held by its
own `--min-passed` ratchet and its own CI step. The ninth row is TypeScript, which is the oracle and
cannot be measured against its own output, so **every row of §31.3 that can have an implementation now
has one.**

Every figure below was printed by a runner, not estimated.

| Edition | §31.3 | Round trip | Self check | Corpus |
|---|---|---|---|---|
| C | Certified, Full | 1199 of 1199 | 87 checks, 0 failed | **240** of 1,199, 0 failed |
| Zig | Host, Community | 1199 of 1199 | 92 checks, 0 failed | **201** of 1,199, 0 failed |
| Java | Guest, Community | 1199 of 1199 | 101 checks, 0 failed | **201** of 1,199, 0 failed |
| C# | Guest, Community | 1199 of 1199 | 113 checks, 0 failed | **201** of 1,199, 0 failed |

The round trip — every `expected` string in the corpus parsed and re-serialised to identical bytes —
passed **1,199 of 1,199 on the first run in all four editions**, and so did every contract module. That
is worth stating plainly rather than celebrating: it is what a differential test against a frozen oracle
is supposed to do once the rules are written from the reference instead of from another edition.

None of the four is in `reference/reports/editions.json`, which still says four editions and 1,199
cases. The refusal inside `tools/edition-report.mjs` keeps any edition with a case unimplemented off the
published figure — the arrangement Go was in from 624 cases until the eighty-second session — and the
tool's `--check` was run after each edition to confirm it.

### What each platform decided differently

**C** — an arena allocator, one region per corpus case, and no `free` in any contract: the L2 decision
closest to the L4 MISRA-C target. `sk_arena_alloc` never returns NULL; it aborts naming the request it
could not serve, because the alternative is a NULL check at several thousand construction sites, which
in practice means a missing one somewhere. 240 cases across six corpora — the only one of the four to
get past the first four contracts.

**Zig** — `ReleaseSafe` by default, `zig fmt` as a build failure, and the fusion scan checking that the
*default* still holds: Zig's float mode is `.strict` and forbids contraction, so unlike C and C++ there
is no flag being set, and `@setFloatMode(.optimized)` is one line in one file away from making the
arithmetic differ on every machine with an FMA.

**Java** — the closest edition to the reference, which is what makes it dangerous: a `String` here is
UTF-16 exactly as a JavaScript string is, so nothing forces a decision about representation and the
places where Java *looks* like the reference and is not are the whole of the work.

**C#** — `ToString("R")` is the shortest round-trip decimal, so this is the one edition of the four that
runs no precision loop; it still reshapes every number, because `"R"` turns exponential at a narrower
band than Java does and signs negative zero.

Both self-check counts in that table were wrong in this entry's first draft — 96 for C and 97 for Zig
— and were corrected by re-running the four editions from the repository root, which is how CI invokes
them. That re-run is also what found the defect below.

### The CI step for Zig was broken, and a masked exit code is why it looked fine

`zig build --build-file editions/zig/build.zig check` is what CI runs, from the repository root. It
**failed** there and passed from `editions/zig`, because a Zig `Run` step with no `setCwd` inherits the
cwd of the `zig build` process — so `-Dcorpus`'s default `../../corpus` resolved against whatever
directory the caller was in. The three run steps now set their working directory to the build root, as
the fusion step already did.

It looked fine because the command that ran it was `zig build ... | grep ...; make -C editions/java ...
| grep ...`, whose reported exit code is the last `grep`'s. Zig's status was never examined, and the
missing `round trip` and `result` lines in that output were the only evidence — which reads as a quiet
step rather than a failed one. The step now passes from the root, with both fusion probes fusing.

### Five gates that found something

1. **GCC 13 implements `-ffp-contract=on` as *off*.** The C fusion probe — the positive control finding
   171 requires — reported that it could not fuse. The probe is now compiled with `-ffp-contract=fast`
   explicitly. The same source with the same flags fuses on GCC 14 and not on 13, which is the whole
   reason `FP_FLAGS` and the scan both exist.
2. **`char.ConvertToUtf32` throws on a string the oracle answers.** A JavaScript string may hold an
   unpaired surrogate and `codePointAt` returns it; .NET raises `ArgumentException`, and
   `ConvertFromUtf32` refuses to write one back. Every walk over a string in the C# `Js` module now goes
   through its own `CodePointAt`/`AppendCodePoint`. Without it, a lone surrogate anywhere in a role or a
   version string is an exception where the reference answers null.
3. **`javac` processes a unicode escape before the lexer runs, including inside a comment.** A comment
   containing the four characters that spell one does not compile. Same shape as the C edition's
   discovery that a C comment cannot contain the two characters that open one — and MSBuild added a
   third: an XML comment cannot contain two consecutive hyphens, so `SkynetOs.csproj` cannot quote a
   `dotnet run --` line at all.
4. **A static array initialiser is a method, and javac limits one to 64 KiB of bytecode.** The generated
   case tables are `error: code too large` as array literals. `tools/unicode-case.ts` emits numbered fill
   methods, one class per table, for Java only. Roslyn has no such limit on a flat `static readonly
   int[]` of constants — it becomes a metadata blob — but it *does* compile `int[][]` as per-row
   bytecode, so the C# target renders flat arrays and offsets.
5. **`dotnet run` forwards an option it does not recognise to the program.** `--nologo` arrived as the
   C# runner's first argument and it answered with its usage line. Right of the runner; the Makefile now
   runs the built assembly directly.

### Two of my own errors, caught by the gates rather than by reading

* **A self-check literal I wrote was wrong.** My 5e-324 expectation for the C edition was not the
  oracle's. It was regenerated *from* the oracle — `node` over `reference/typescript/canonical.ts` — and
  all sixteen probe values then agreed.
* **My Zig float probe printed `0.3` for `0.1 + 0.2`, and the probe was wrong, not Zig.** Comptime float
  arithmetic is f128. Routing it through a runtime `f64` gave `0.30000000000000004`, and the shape of
  that mistake is now what the self-check asserts.

### Documentation corrected, having been proved wrong

* `tools/edition-report.mjs` said §31.3's other rows were absent because there was no code to run. Four
  of the five now have code and are absent because they are *partial*. All three reasons are named.
* `README.md` said the corpus holds 1,137 cases and that the Rust runner reproduces all 1,137. It is
  1,199 on both lines, re-counted from `corpus/`.
* `README.md`'s repository layout did not mention `editions/` at all. Seven of them now.
* CI's header said three editions were partial and its case-tables job said five editions and twelve
  generated files. Four, six and fourteen.

### Open

1. **Nothing has been pushed yet at the time of writing this entry**; the branch is
   `claude/exciting-hypatia-k11axe`.
2. CI's Linux run of the four new steps has not happened.
3. No edition has been run on arm64 hardware.
4. The four new editions answer four corpora between them (six for C). The other twenty-six are
   unimplemented and counted as such on every run.
5. From before: §73's transport row is still contradicted by the Skills row.

### Findings, restated

1-176 as before.

177. **A per-language comment rule is a real constraint, not a curiosity.** Three of the four editions
     hit one: a C comment cannot contain the two characters that open one, a Java comment cannot contain
     a unicode escape, and an XML comment cannot contain two consecutive hyphens. Each was found by a
     compiler refusing a comment that explained something true, and each was fixed by rewording rather
     than by deleting the explanation.

178. **A compiler flag's documented default is not its implemented default.** `-ffp-contract=on` is what
     `-std=c17` selects and GCC 13 implements it as off. A gate that assumed the documented behaviour
     would have reported a clean scan on a compiler that was never contracting anything.

179. **A host library that throws where the reference answers is a defect in the edition, not the
     input.** Two .NET calls refuse a lone surrogate that a JavaScript string may legally hold. Caught by
     reading the API's contract before the corpus reached it — the corpus case that would have gone red
     is real, and would have read as a bug in the contract rather than in the string walk.

180. **The shortest-round-trip renderer is per-platform, and so is whether an edition needs a search.**
     Java's `Double.toString` gives two digits where one round-trips; .NET's `"R"` gives one. The same
     rule 3 therefore costs a precision loop in one edition and a reshaping in another, and an edition
     that copied the other's approach would be either wrong or slow for no reason.

181. **A gate has to be run the way CI runs it, from where CI runs it.** The Zig step passed from the
     edition's own directory and failed from the repository root, because a build-system run step
     inherited the caller's working directory and a relative default therefore meant two different
     things. Running it once from the wrong place is the whole test.

182. **A pipeline's exit code is the last command's.** `zig build ... | grep ...` reports the grep's
     status, so a failing gate read as a passing one with less output. Anything whose result matters is
     checked by its own status, not by whether the line one expected appeared.

## 2026-09-12 (eighty-fourth) — the closed vocabularies in four editions, and +296 cases

No §31.3 row is left without an implementation, so this session is **breadth**: `vocabulary/closed-sets`,
the corpus's single largest file after the fuzz corpora, added to all four of the partial editions.

| Edition | Before | After | Self check |
|---|---|---|---|
| C | 240 | **314** of 1,199, 0 failed | 91 checks, 0 failed |
| Zig | 201 | **275** of 1,199, 0 failed | 96 checks, 0 failed |
| Java | 201 | **275** of 1,199, 0 failed | 104 checks, 0 failed |
| C# | 201 | **275** of 1,199, 0 failed | 116 checks, 0 failed |

All four passed all 74 sets on the first run, and every ratchet was raised to the new figure. The round
trip is still 1199 of 1199 in each.

### Why this corpus, and why it is not generated

Seventy-four closed string sets, 442 members, and the generator's own comment says what it is for:
`capability_reserved` was added to the Rust `PackageRejectionCode` and not to the reference's, and nothing
could see it — the mirror pins guard the reference, and every corpus covers the cases it holds, none of
which had ever mentioned a token invented ten minutes earlier.

That argument decides how the tables were written. **Each edition holds its own table**, rather than one
generator emitting them into all four the way `tools/unicode-case.ts` emits the case tables. The case
tables come from the JavaScript engine, which no edition can re-derive by hand; a vocabulary is a list of
strings in the reference's own source, and a token added in one place and not another is precisely what
this corpus was added to catch. A generator could not catch a mistake it had made in every edition at once.

The member lists were read out of `reference/typescript` mechanically — one exported constant per set,
resolved through the `vocabulary()` calls in `tools/corpus-gen/generate.ts` that name which constant each
set is — and that reading was checked against the corpus **before** any edition code was written: 74 of 74
sets agreed. What each edition then carries is its own statement of the vocabulary, and each is
independently subject to the corpus from here on.

Order is part of the agreement for several sets, and the tables preserve declaration order rather than
sorting: `os_tiers` and `scts_areas` are ranked, `operating_states` and `state_reasons` run in the order
§27's `evaluate` descends through its rules, and `skill_phases` runs in diagram order so that "never moves
backwards through this list" is a property somebody can assert.

### What each language decided

* **C** — `SET(...)` is a compound literal in a static initialiser, which has static storage duration
  (C17 6.5.2.5p5), so the table costs nothing at run time and there is no second name per set to keep in
  step with the first.
* **Zig** — `zig fmt` collapses a wrapped list onto one line unless it carries a trailing comma, and with
  one it puts a single member per line. The formatter's preference is the right answer here for a reason
  beyond formatting: one member per line makes an added token exactly one line of diff.
* **Java** — `put` **throws** on a duplicate set name during class initialisation. §6.1's ladder: a
  violation that cannot be represented beats a test that finds it. A duplicate would replace the earlier
  set in place, and the corpus would still pass on whichever of the two survived.
* **C#** — a collection initialiser with a duplicate key throws at type initialisation, which is the same
  gate without the explicit method.

An undeclared set answers **JSON null, not an empty array**, in all four. Every one of the seventy-four
sets has at least one member, so an empty array could only mean *a set with nothing in it*; §54.2's rule
about naming what could not be resolved applies to a set name as much as to a path. The self-check asserts
both halves — that no set is empty, and that an undeclared name is null — which is the pair that makes the
distinction usable.

### Documentation corrected, having been proved wrong

* **The C and Zig self-check headers each said "four groups" and listed four, while both had five** — the
  case-tables group was never listed. Now six each, with every group named. The Java header said five
  (now six) and the C#'s six (now seven).
* The Java and C# READMEs quoted self-check counts of 101 and 113; they are 104 and 116.
* All four READMEs said `Dispatch` was the only file with no module beside it in the reference. The
  vocabulary contract is a second, and each README now says so and why.

### Open

1. The four editions answer five corpora between them (seven for C). Twenty-five remain, with `fuzz`
   (148 cases), `skill` (88) and `identity` (56) the largest.
2. CI's Linux run of the four steps still has not happened.
3. No edition has been run on arm64 hardware.
4. From before: §73's transport row is still contradicted by the Skills row.

### Findings, restated

1-182 as before.

183. **Whether to generate a table into every edition depends on whether a human could have derived it.**
     The case tables come from a JavaScript engine and are generated into six editions, because no edition
     can re-derive them. The closed vocabularies are lists of strings in the reference's source and are
     *not* generated, because the corpus that holds them exists to catch a token added in one edition and
     not another — and a generator cannot catch a mistake it made in all of them. Same shape of artefact,
     opposite answer, and the reason is the gate rather than the data.

184. **A doc comment that counts its own list is a claim, and two of them were already wrong.** The C and
     Zig self-check headers said "four groups" over five. Nothing checks a number in prose, which is why
     the number in prose should be the thing a reader can also count in the file below it.
