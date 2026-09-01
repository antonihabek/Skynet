# Skynet OS

A universal operating system for robots — where "universal" means something specific and defensible,
and the specification spends a good deal of its length saying what it does not mean.

Skynet has two halves. **Skynet Hub** is where robot software is discovered, versioned, reviewed,
signed and sold. **Skynet OS** is what runs that software on the machine. This repository is the
second half.

> The Hub can already tell you *whether* a policy fits a robot. Nothing yet makes it *fit*.

Licence: Apache-2.0. The trademark and the certification programme are what SCTS gates — not the
interface. Implement it freely.

---

## The claim, stated exactly

There is no binary that boots on a Unitree G1, an ABB arm, a Roomba and a warehouse AMR. What is
universal is the **contract**: the same packages, the same transport planner, the same Hub, the same
conformance suite, the same CLI — reached at three depths of integration, each stating plainly what
it can and cannot promise.

| | **Guest** | **Host** | **Certified** |
|---|---|---|---|
| Runs on | A container beside the vendor stack | PREEMPT_RT Linux on the robot | Host, plus an independent safety MCU |
| Owns the control loop | No | Yes | Yes |
| Safety Kernel | **Advisory** — detects and records, enforces nothing | **Enforcing** | **Enforcing, redundant** |
| Stop path independent of the app processor | No | No | Yes |
| Can be evidence in a safety case | No | No | Yes |
| Effort to adopt | An afternoon | A vendor porting project | That, plus hardware |

Guest is how the ecosystem starts, and it is honest about being a passenger: it computes every
verdict, logs every violation, raises every alarm — and the command still reaches the robot
unchanged, because there is nothing underneath that Skynet controls. That is genuinely useful, it is
not a safety function, and every surface says so in those words.

```
SAFETY_CLAIM_RULE:
  Skynet OS supplies evidence for a safety case. It does not certify a machine. Conformity
  assessment against ISO 10218, ISO 13482 or any other standard is performed by a notified
  body against a specific robot in a specific application.
```

That constant lives in the code and is rendered on every surface that mentions the certified tier.
Shortening it would be the most damaging thing this product could do.

---

## The one invariant

> **The Safety Kernel is the only component with write access to an actuator.**

A policy does not command a joint. It writes a *desired* command into a ring the kernel reads, and
the kernel decides what reaches the motors. This is not a stylistic preference: most published
checkpoints are pickles, and unpickling executes code by construction. Without sole writership,
every published model is a remote code execution vulnerability bolted to a two-metre humanoid. It is
what lets Skynet run untrusted policies *at all* — which is the entire point of having a catalogue.

```
┌──────────────────────────────────────────────────────────────┐
│  .sky packages          policy · skill · service             │  sandboxed
│  no actuator access · declared budget · granted capabilities │  untrusted
├──────────────────────────────────────────────────────────────┤
│  Policy Runtime      inference · observation assembly        │
│  Embodiment Transport   resolved at install, applied here    │  system
│  skynetd             lifecycle · reconcile · OTA · blackbox  │  services
├══════════════════════════════════════════════════════════════┤
│  ███  SAFETY KERNEL  ███                                     │  real-time
│  the only writer to an actuator · 8 guards · no allocation   │  no AI
├══════════════════════════════════════════════════════════════┤
│  RHAL — stable C ABI, semver, drivers as shared objects      │  vendor
├──────────────────────────────────────────────────────────────┤
│  Vendor driver: ROS 2 · EtherCAT · CANopen · SDK · simulator │  boundary
├──────────────────────────────────────────────────────────────┤
│  PREEMPT_RT Linux            (certified tier: + safety MCU)  │
└──────────────────────────────────────────────────────────────┘
```

The double rule is the trust boundary. Everything above it may be untrusted, downloaded five minutes
ago, and written by a stranger. Nothing above it can move a joint except by asking the thing below
it, which says no when the answer is no.

---

## What is in this repository today

| | State |
|---|---|
| `docs/` — the full specification, nine parts | Complete |
| `bindings/c`, `bindings/cpp` — the RHAL ABI | Written, compiling, layout-checked from both sides |
| `crates/skynet-kernel` — the Safety Kernel | `no_std`, no `alloc`, 8 guards, forward kinematics and its own arithmetic, 101 tests |
| `crates/skynet-contracts` — transport, admission, certification, readiness, observation, entitlement, birth, provenance, operating state, the live precondition context, OTA | Ported from the reference; §25.1's observation assembler; §51's offline entitlement, whose enumeration has no variant meaning *stop*; §50's birth certificate, where a board swap is an expected finding with a remedy rather than a failure; §56's provenance classes, where a declared simulator can never reach the field leaderboard; §27's operating state machine, which four other sections had been describing in prose; §54.4's skill lifecycle and §54.5's two runtime limits; §54.2's context, where a fact the robot cannot observe is **absent** rather than defaulted, so the precondition naming it is refused with the path named; §12's certification gate, where an unrecognised component is gated for the same reason the safety kernel is and a bench pass is bound to the digest it was taken against; 356 tests |
| `crates/skynet-json` — canonical JSON | 16 tests |
| `crates/skynet-rhal` — the Rust view of the ABI | 7 tests |
| `crates/skynet-sha256` — SHA-256 | Held to the FIPS 180-4 vectors, 5 tests |
| `crates/skynet-ed25519` — Ed25519 verification (§19) | SHA-512, the field, the curve; held to RFC 8032's vectors and to the encodings that divide implementations. Verification only — signing is a test feature. 41 tests |
| `crates/skynet-urdf` — URDF import (§63.4) | XML reader, model, role suggestion, descriptor, collision geometry, 45 tests |
| `crates/skynet-cli` — the `skynet` command | descriptor, transport, frames, role, driver new/check, plus the robot commands over the local socket, `blackbox`, `skill run` / `skill stop` / `skills`, and `ota`; 44 tests |
| `crates/skynetd` — the supervisor's brain (§33) | Reconciler, supervision, health, leases — including §54.4's all-or-nothing acquisition; pure, 45 tests, and CI asserts it stays pure |
| `crates/skynetd-io` — the I/O half, and the `skynetd` binary | Package store, systemd launcher, observation, local API, the on-robot console (§32.2), the black box on disk, the trust store (§19), §34.3's bearers and the outbound sender, §51's cached entitlement tokens, §50's birth certificate, §27's state observed once a pass and §33.4's barrier read off it, **§54's skill runtime** — the loop that drives the lifecycle, assembles the precondition context, takes the leases, records every transition and emits one episode per invocation — **§12's OTA half**: the signed release manifest and bench pass, the RAUC updater behind a trait, and the boot that confirms or condemns its own slot, the loop; 283 tests plus an end-to-end run against real systemd |
| `crates/skynet-blackbox` — the black box (§13, §26) | The preallocated ring, the MCAP writer, sealing, deterministic replay; `no_std`, 31 tests. Record format **2**: a frame now records the capture paths that produced nothing, and an unmeasurable skew rather than a zero one |
| `crates/skynet-ring` — the SPSC command ring (§25) | The only channel from a policy to the kernel: 4 KiB slots, a seqlock, latest-wins, never blocks. `no_std`, no `alloc`, no `unsafe`; 18 tests including a two-thread torture run |
| `crates/skynet-ring-shm` — the `memfd` under it | Creates, seals and maps the anonymous file; the consumer's mapping carries no `PROT_WRITE`. 9 tests, one of which re-executes itself so a command crosses two real processes |
| `crates/skynet-capture` — the capture-side split (§47.4) | A latest-wins slot for the policy and a bounded queue for the black box, which must never be one queue. Moves buffer handles, never pixels; every displaced or refused frame is handed back, and a run of drops is one annotated gap. `no_std`, no `alloc`, no `unsafe`; 17 tests |
| `corpus/` — the conformance corpus | 842 cases, 56 files, of which 45 are the closed vocabularies both editions must spell identically |
| `crates/skynet-corpus` — the corpus runner | Reproduces all 842 byte for byte |
| `reference/typescript/` — the oracle | Vendored from the Hub; generates the corpus |
| `bindings/python` — the Python RHAL | Driver protocol plus a ctypes ABI view, offset-asserted |
| `drivers/mock` — the mock robot | 24 sim-conformance checks, no hardware |
| `drivers/python-host` — the Python driver host | Runs a Python driver out of process, so its stop path is real |
| `drivers/lerobot` — the LeRobot bridge (§67) | Role tables, 70 checks against a fake; Guest tier |
| `drivers/mujoco` — the MuJoCo simulator driver (§70) | 103 checks against real physics; Guest tier, twice over |

**Not built, and the specification says so on its own behalf:** the policy runtime, the
safety MCU, and every driver that talks to real hardware — `ros2_control`, EtherCAT, CANopen and the
vendor SDKs. The realistic figure for a robot moving under this is several engineer-years. §59.5
states that plainly and this README does not improve on it.

OTA is a partial entry rather than either column, and the half that is missing is one nobody can
close from here. §12's three rules are built and held to a corpus: an apply waits for a declared safe
state, a boot confirms or condemns its own slot within a budget, and an image that changes the Safety
Kernel or the envelope is refused without a bench pass bound to its digest. What is **not** built is
the fetch — §34's inbound half — so a bundle has to be on the robot before it can be staged, and
`skynetd` says so rather than reporting success for a file nobody delivered. Nor has any of it run
against real RAUC on a real A/B device; the updater is a trait with a recording implementation, which
is what makes §12's unattended rollback testable at all and is not the same as having watched one.

§54's skill runtime is built and is **not** the policy runtime, which is a distinction worth making
here because the two are one paragraph apart. What runs is the *lifecycle*: preconditions evaluated
against a live context, leases taken and rolled back together, a timeout enforced, success judged by
the OS over post-state, every transition recorded and one episode emitted per invocation. What a
skill's own process then does with a robot is still nothing, because no policy sandbox receives a
descriptor and nothing turns a frame into a command. A skill on this edition starts a unit, is
supervised, is judged and is recorded — against a body that does not move.

§25's command path is a **half** entry rather than either column, and the halves are worth naming
separately — this paragraph said the `memfd` was unbuilt for a session after `skynet-ring-shm`
built it, which is the kind of drift a README acquires by describing what it remembers rather than
what is in the table above it.

Built and exercised: the ring's slots, its seqlock, its wrap and its refusal to block; the `memfd`
under it, sealed, with the consumer's mapping carrying no `PROT_WRITE`, and a test that re-executes
itself so a command crosses two real processes; §25.1's observation assembler, with the corpus
pinning both editions to the same frame; and §47.4's capture-side split, where the policy's
latest-wins slot and the black box's bounded queue are separate structures because a single queue in
front of a policy is latency wearing the costume of reliability.

Not built: the runtime that would sit either side of any of it. No capture backend fills a stream —
`skynet-capture` moves handles a V4L2 or GenICam backend would produce, and neither exists — no
policy sandbox receives a descriptor, and nothing turns a frame into an inference and a command. The
frames the assembler builds are ones the corpus handed it.

The on-robot console has now been opened in a browser, which found two things no test was watching
for: `GET /plan` answered two different shapes and the reconcile panel had never once rendered, and
the health panel repeated the same thirty-word sentence five times. Both are fixed.
`cargo test -p skynetd-io -- --ignored console_fixture` writes the page and one file per route it
fetches, from the real router, so anybody can serve the directory and look at it without a robot.

The black box is a partial entry rather than either column. The ring, the format, the seal and the
replay are written and held to the corpus; what a *supervisor* can put in one is §26's event channel
and no more, because the four cycle channels carry control cycles and there is no control loop to
produce them. `skynet blackbox` says so on every robot it is pointed at, rather than showing a
healthy-looking recording of a body that has never been commanded.

---

## Running the daemon

`skynetd` is the supervisor (§33) and `skynet` is the CLI in front of it. Nothing here needs a robot;
`--root` puts `/data` and `/run` wherever you like.

```bash
# Stage a package. The digest is verified against the content, not taken on trust; add
# --signed-by and --signature and the release signature is checked too (§19, §37.1).
skynet install ./my-policy --root /tmp/robot \
  --id openrobot/grasp --kind policy --version 1.0.0 --digest <hex>

# Say what the robot should be running, then reconcile once and read the plan.
skynetd once --root /tmp/robot

# Or run it, with the local API in front (§33.7).
skynetd --root /tmp/robot &
skynet status --socket /tmp/robot/run/skynetd.sock     # needs no credential (§39.1)
skynet estop  --socket /tmp/robot/run/skynetd.sock     # nor does this
skynetd credential add --root /tmp/robot --label laptop --role operator
skynet ps --socket /tmp/robot/run/skynetd.sock --token <token>

# Invoke a skill (§54.1). Accepted, not started: the next reconcile pass evaluates §54.2's
# preconditions against a live context and may refuse it, naming the clause.
skynet skill run openrobot/pick --args '{"target":"cup"}'   --socket /tmp/robot/run/skynetd.sock --token <token>
skynet skills --socket /tmp/robot/run/skynetd.sock --token <token>
```

Two things it will not do, and says so rather than pretending:

* **nothing starts until somebody declares a safe state.** §33.4 gates it, and this edition has no
  channel to the kernel to ask — so an operator writes `yes` into `/data/state/safe-state` and every
  report of the safe state says `declared` beside it.
* **there is no default password.** An unprovisioned robot answers status and stop and nothing else
  (§39.2), and that is the specified behaviour rather than a state to be fixed by a fallback.
* **and it trusts nobody until told to.** `/data/state/trust.json` holds the keys whose signatures
  this robot believes, and there is no default there either — so a package reports `untrusted` until
  somebody writes one, and a **driver** does not activate at all (§19). That is the same
  useless-rather-than-open choice §39.2 makes for credentials.

`sh crates/skynetd-io/e2e.sh` runs the whole chain — install, verify, activate, launch, socket,
rollback — and reports the systemd launch step as not run where there is no systemd, rather than
passing a shorter exam.

---

## The golden path

What a developer reads, in order, and nothing else is required to get to a moving robot (§65).

### 1. Concepts

The tiers, the kernel invariant, what a driver is. That is the section above, and
[`docs/skynet-os-architecture.md`](docs/skynet-os-architecture.md) §1–§7 in full.

### 2. Build it

No network, no dependencies. The Rust workspace has none at all — not minimalism for its own sake,
but because the kernel is `no_std` without `alloc` and a vendor building a driver in 2036 should not
need a package registry to have survived.

```bash
cargo test --workspace
```

### 3. Run the mock robot

A body that answers the RHAL with no hardware behind it, and — the part that matters more — one you
can provoke the dangerous failures on: a stalled command stream, a joint reporting NaN, an estop
mid-motion, a bus timeout inside a cycle.

```bash
cd drivers/mock && make check
```

### 4. Write a driver

Start from a scaffold rather than from a blank file (§63.1):

```bash
skynet driver new my-arm --lang c      # or --lang rust
cd my-arm && make
skynet driver check ./libmy_arm.so
```

The scaffold builds, loads, and **fails conformance in exactly the places where only you know the
answer** — the roles are null, so `embodiment.roles.coverage` fails at 0%, and a driver cannot sign
its own descriptor, so `embodiment.descriptor.signed` is not measured. Those two plus the `TODO`s in
the generated source are the to-do list.

`skynet driver check` runs the driver-facing half of the sim suite against the built shared object
and reports, in the terms of §15, **the tier the evidence supports** rather than a pass or a fail.
The sim tests that are properties of the runtime rather than of a driver are listed as not covered,
by name, on every run — a vendor arriving at certification believing they had evidence they do not
have is a failure of that list. Turning up to a bench session with this already green is what makes
bench time cheap.

`drivers/mock/mock_robot.c` is the reference. A vendor implements one shared object exporting the C
ABI in [`bindings/c/skynet_rhal.h`](bindings/c/skynet_rhal.h), and that is the entire integration.

**If your robot is one LeRobot already supports, the driver exists**:
[`drivers/lerobot`](drivers/lerobot/) wraps `Robot.connect / get_observation / send_action`, and
[`drivers/python-host`](drivers/python-host/) is what loads it. Guest tier — Python's GIL and garbage
collector cannot hold a real-time path, and the host refuses to load a Python driver that claims
otherwise.

**And if you have no robot at all**, [`drivers/mujoco`](drivers/mujoco/) turns an MJCF scene into
one: a descriptor built from the scene, a clock that is the simulator's (§35.3), a camera where the
machine can render, and `simulator: true` on the descriptor so nothing it produces can be presented
as bench evidence. That is what makes the sim half of SCTS runnable against physics rather than
against a mock, which is why §70 calls simulator drivers load-bearing rather than a lesser path.
Four properties make it a contract rather than an interface:

* **`abi_version` is checked at load and mismatches refuse.** The OS ships weekly, the driver ships
  when the vendor has something to say, and neither blocks the other. This is Project Treble in one
  field.
* **`read_state` and `write_command` are wait-free and allocate nothing.** They run inside the
  real-time path.
* **`emergency_stop` must work from a signal handler and from a dying process.** A driver that
  implements it as "publish a message and await an ack" fails the test that matters most.
* **`describe` returns the embodiment descriptor.** The body description comes out of the driver, so
  it cannot drift from the hardware it describes without the driver being wrong.

### 5. Describe the body

Most of the descriptor is already written and sitting in a file the robot has (§63.4):

```bash
skynet descriptor from-urdf robot.urdf > embodiment.json
```

The descriptor goes to stdout and the reasoning goes to stderr, so the redirect above captures a
file and not a lecture. Joints, types, limits, the frame graph and the degrees of freedom come
straight out of the URDF. Two things never do, and the report says so every time.

The role vocabulary is what makes cross-body work possible, and it is deliberately modest:

```
role  := chain "." link [ "." axis ] [ "#" index ]

left_arm.shoulder.pitch    right_hand.finger.flex#2    base.wheel#0
```

Wire names stay vendor-specific; the semantics become shared. An unrecognised role parses to
nothing and can then only be matched by exact name — it never silently maps to something plausible.
`skynet role left_arm.shoulder.pitch` checks one at the keyboard.

**Roles are suggested, never assigned.** A URDF carries no semantics, so `shoulder_pitch_joint` is
*probably* `left_arm.shoulder.pitch` — and probably is not a basis for cross-body remapping, because
a wrong role does not fail loudly, it binds a policy's elbow channel to a wrist. The importer prints
every suggestion and how it reached it; `--accept-suggested-roles` is the human-confirms step and is
a separate action precisely so that it is one.

**The `envelope` stays null until a person supplies a sourced figure.** A URDF effort limit is a
design value from a CAD model, not a datasheet safety rating, and copying one into the other would
auto-populate a safety limit nobody verified. The robot then renders "no safety envelope declared",
which is correct and slightly annoying, and that is the right trade.

### 6. Check the body's own geometry

The descriptor carries a frame graph (§44.3), and the Safety Kernel walks it in cycle to decide two
of the eight guards. What that walk can actually do is worth knowing before a robot is holding
itself still for a reason nobody can see:

```bash
skynet frames embodiment.json --joints shoulder=1.5708,elbow=0
```

The tree, where that configuration puts every frame, and — the part that matters — what the
`workspace` and `self_collision` guards will be able to do with it: how many frames can be
constrained, how many carry geometry the kernel can place, and how many pairs that costs per cycle.
A body with one collision capsule has nothing to collide with, and learning that here beats learning
it from a guard silently doing nothing.

Forward kinematics runs on the same code the robot runs, down to the sine: §31.2 makes the kernel's
`sin`, `cos` and `sqrt` single-source across editions, because no platform library is required to
round the way another one does and a redundant safety processor that disagrees with the primary
about a clamp is the failure redundancy exists to detect.

### 7. Understand what will be enforced

```bash
skynet transport ./policy.json --robot embodiment.json
```

```rust
let plan = skynet_contracts::resolve_transport(&policy, &body, Default::default());
```

Per channel, one of six answers — `direct`, `remap`, `retarget`, `synthesize`, `drop`, `blocked` —
and the plan's verdict is the worst binding in it. The planner is pure: no I/O, no clock, no
database. It runs on the Hub so a repository page can answer "does this run on my G1?" before
anybody downloads forty gigabytes, on the robot at install time with no network at all, and on a
workstation so a developer can ask what a body they do not own would do with their policy. All three
compute the same function over the same declared facts, so none is authoritative and they cannot
disagree.

---

## The corpus is the specification

An edition is any implementation that reproduces the conformance corpus and passes SCTS. Not "agrees
with", not "is equivalent to" — **reproduces, byte for byte**, because equivalence is where drift
hides. A warning in a different order today is a different clamp tomorrow.

```bash
node tools/corpus-gen/generate.ts   # regenerate from the reference oracle
cargo test -p skynet-corpus          # hold this edition to it
```

350 cases across roles, transport, cycle verdicts, limit resolution, frame graphs, forward
kinematics, the black box, admission, certification and readiness — 100 of them generated from two
seeded PRNGs written out in full, so regenerating produces byte-identical files and a diff means a
behaviour change rather than a reshuffle.

The newest of them freeze **poses**, not only verdicts. A guard trip is a boolean, and two editions
could agree on every trip in the corpus while disagreeing about where the arm is by a millimetre —
right up to the day one of them clamps and the other does not.

A divergence is *either* a specification ambiguity *or* an implementation defect. It blocks the
release of the edition that diverged and never of the others.

---

## Repository layout

```
docs/            The specification. Nine parts, and the Hub's architecture beside it.
bindings/        The RHAL C ABI and its C++ wrapper. Single-source, always (§31.2).
crates/          The Rust reference edition.
  skynet-json        Canonical JSON — the rule the corpus comparison rests on.
  skynet-sha256      SHA-256, for the digests the specification already names.
  skynet-rhal        The Rust view of the ABI. Layout-asserted against the header.
  skynet-kernel      The Safety Kernel. no_std, no alloc, no clock, its own sin/cos/sqrt.
  skynet-contracts   L2: transport, admission, certification, readiness, entitlement,
                     birth, provenance.
  skynet-urdf        URDF import. The only crate that links std, because it is a tool.
  skynet-cli         The `skynet` command, and the driver loader that needs unsafe.
  skynetd            The supervisor's brain: reconcile, supervise, health, leases. Pure.
  skynetd-io         Where it meets the machine: store, launcher, socket, loop. The daemon.
  skynet-blackbox    The black box: the ring, MCAP, sealing, replay. no_std, no I/O.
  skynet-ring        The SPSC command ring (§25). A protocol, no I/O, and no unsafe.
  skynet-ring-shm    The memfd under it. Four syscalls, and the third crate that may use unsafe.
  skynet-capture     The capture-side split (§47.4): a latest-wins slot and a bounded queue.
  skynet-corpus      Holds this edition to the corpus.
corpus/          The conformance corpus. Plain data, no code.
reference/       The TypeScript oracle the corpus is generated from.
tools/           The generator.
drivers/
  mock               A body that answers the RHAL with no hardware, and injects the faults.
  python-host        Runs a Python driver out of process. One shared object, one symbol.
  lerobot            The LeRobot bridge (§67). Guest tier, and it says so everywhere.
  mujoco             The MuJoCo simulator driver (§70). A scene file becomes a body.
```

Why this is a separate repository from the Hub, and not a subdirectory (§72.1): a subdirectory of a
private commercial repository cannot be cleanly open-sourced later without a history-scrubbing split
or publishing commit history that leaks marketplace internals. Git history is permanent, so the
licence boundary has to exist before the code does. It begins private and becomes public later, which
is a repository setting rather than a migration — and that only works if the repository is separate
from the outset.

---

## What this does not do, stated because the word "universal" invites the strong reading

* **Nothing runs without an RHAL driver for that specific robot.** The claim is precisely: any robot
  for which somebody has written a conformant driver.
* **A large class of robots cannot be commanded at joint level.** Behind a Fanuc, KUKA, ABB or older
  UR controller you upload a *program*; the controller owns the servo loop and enforces its own
  safety. Those machines are Guest tier permanently — not because of a missing feature, but because
  the architecture that makes them safe is somebody else's and is already there.
* **Transport makes a policy execute, not succeed.** A `retargeted` verdict means the commands are
  geometrically valid and inside the envelope. Whether the policy still does the task is an open
  research question.
* **At Host tier, Skynet OS replaces the thing it is said to be compatible with.** `ros2_control` and
  the Safety Kernel both want to be the component that writes to actuators, and they cannot both be.
  Anyone evaluating adoption should hear that in the first conversation rather than the third month.

The full version of this list is Part VII of the specification, and it is worth reading before the
rest.
