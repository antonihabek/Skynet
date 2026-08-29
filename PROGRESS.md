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
