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
