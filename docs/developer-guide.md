# Developer guide

This is the golden path (§65): what a developer reads, in order, to get from nothing to a moving
robot, and nothing else is required. It assumes you have read the [README](../README.md), which
explains what Skynet OS is and lets you run a virtual robot.

Every command below runs from the root of the repository. Every `§` points into
[the specification](skynet-os-architecture.md).

---

## The golden path

### 1. Concepts

The tiers, the kernel invariant, what a driver is. The README's
[How it keeps a robot safe](../README.md#how-it-keeps-a-robot-safe) and
[Three levels](../README.md#three-levels-guest-host-and-certified) sections are the short version;
[the specification](skynet-os-architecture.md) §1–§7 is the full one.

The layers, from the top down:

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
├──────────────────────────────────────────────────────────────┤
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

### 2. Build it

No network, no dependencies. The Rust workspace has none at all — not minimalism for its own sake,
but because the kernel is `no_std` without `alloc`, and a vendor building a driver in 2036 should not
need a package registry to have survived.

```bash
cargo test --workspace
cargo install --path crates/skynet-cli --locked    # puts `skynet` on your PATH
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
its own descriptor, so `embodiment.descriptor.signed` is not measured. Those two, plus the `TODO`s
in the generated source, are your to-do list.

`skynet driver check` runs the driver-facing half of the sim suite against the built shared object
and reports, in the terms of §15, **the tier the evidence supports** rather than a pass or a fail.
The sim tests that are properties of the runtime rather than of a driver are listed as not covered,
by name, on every run — a vendor arriving at certification believing they had evidence they do not
have is a failure of that list. Turning up to a bench session with this already green is what makes
bench time cheap.

[`drivers/mock/mock_robot.c`](../drivers/mock/mock_robot.c) is the reference. A vendor implements
one shared object exporting the C ABI in [`bindings/c/skynet_rhal.h`](../bindings/c/skynet_rhal.h),
and that is the entire integration. Four properties make it a contract rather than an interface:

* **`abi_version` is checked at load and mismatches refuse.** The OS ships weekly, the driver ships
  when the vendor has something to say, and neither blocks the other. This is Project Treble in one
  field.
* **`read_state` and `write_command` are wait-free and allocate nothing.** They run inside the
  real-time path.
* **`emergency_stop` must work from a signal handler and from a dying process.** A driver that
  implements it as "publish a message and await an ack" fails the test that matters most.
* **`describe` returns the embodiment descriptor.** The body description comes out of the driver, so
  it cannot drift from the hardware it describes without the driver being wrong.

**If your robot is one LeRobot already supports, the driver exists**:
[`drivers/lerobot`](../drivers/lerobot/) wraps `Robot.connect / get_observation / send_action`, and
[`drivers/python-host`](../drivers/python-host/) is what loads it. Guest tier — Python's GIL and
garbage collector cannot hold a real-time path, and the host refuses to load a Python driver that
claims otherwise.

**And if you have no robot at all**, [`drivers/mujoco`](../drivers/mujoco/) turns an MJCF scene into
one: a descriptor built from the scene, a clock that is the simulator's (§35.3), a camera where the
machine can render, and `simulator: true` on the descriptor so nothing it produces can be presented
as bench evidence. That is what makes the sim half of SCTS runnable against physics rather than
against a mock, which is why §70 calls simulator drivers load-bearing rather than a lesser path.

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

It prints the tree, where that configuration puts every frame, and — the part that matters — what
the `workspace` and `self_collision` guards will be able to do with it: how many frames can be
constrained, how many carry geometry the kernel can place, and how many pairs that costs per cycle.
A body with one collision capsule has nothing to collide with, and learning that here beats learning
it from a guard silently doing nothing.

Forward kinematics runs on the same code the robot runs, down to the sine: §31.2 makes the kernel's
`sin`, `cos` and `sqrt` single-source across editions, because no platform library is required to
round the way another one does, and a redundant safety processor that disagrees with the primary
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

### 8. Install on a real robot

Everything above runs on the bench. A physical robot gets the same system over SSH, driven
from this PC — the robot needs nothing installed first, only power, network, and SSH. The
whole install is seven steps; Unitree is the worked example, and every other model follows
the same shape with its own directory and SDK variable from `drivers/models.json`.

Have in hand: this checkout with `skynet` on PATH (step 2), the robot's address (hostname
or IP — whoever powers the robot knows it; Skynet advertises nothing before it is
installed), and SSH as a user who can write the system directories (`--user`, default
`root`).

1. Fetch the vendor SDK. The drivers link the manufacturer's own code, which is not
vendored; the manifest names the variable each model needs:

```bash
git clone --depth 1 https://github.com/unitreerobotics/unitree_sdk2 /tmp/unitree_sdk2
```

2. Build the driver library, on Linux — a library built on Windows or macOS will not run
on the robot:

```bash
make -C drivers/unitree lib UNITREE_SDK2=/tmp/unitree_sdk2
```

Success leaves `drivers/unitree/libskynet_unitree.so` behind, and `--robot unitree` picks
it up from the checkout you run in.

3. Check SSH before the wizard does:

```bash
ssh root@robot.local true && echo reachable
```

4. Dry-run the install — the plan and every `ssh` invocation, printed; nothing touched:

```bash
skynet setup robot.local --robot unitree --dry-run
```

5. Run it. Read the confirm screen — host, driver, bundle, and the driver's release stage — and say
yes. The stage is how far the driver has been tested: `beta` against the maker's own software,
`alpha` against counterparts written in this repository only. Every driver here is one or the
other, because none has yet run on a physical robot; the first run on yours is that run.

```bash
skynet setup robot.local --robot unitree
```

With no bundle anywhere, the wizard fetches the robot's release itself — checksummed,
like `packaging/get.sh` — and it refuses before copying when the robot is unreachable,
the wrong architecture, or not writable by the SSH user. Every installer line streams to
this terminal and to `skynet-setup-<host>.log` beside it. The admin token prints once —
paste the `export SKYNET_TOKEN=…` line it shows into your shell.

6. Start the units when the workspace is clear. The wizard offers; by hand, later:

```bash
ssh root@robot.local systemctl start skynetk skynetd
```

7. Verify. The wizard offers this too; by hand, later:

```bash
ssh root@robot.local skynet doctor
```

Two variations before step 5: a robot of your own rather than a supported model arrives as
`--driver ./libmy_arm.so`, with the body derived from the driver; and a direct cable
(link-local, `169.254.x`) can add `--lan <robot-ip>:8099` to step 5, after which status
and doctor run from this PC (`skynet doctor --host <robot-ip>:8099`) instead of over SSH.
On a normal LAN the console stays off by design (§34.2), and steps 6–7 go over SSH as
written. When anything fails,
[troubleshooting](troubleshooting.md#the-install-failed--where-the-record-is) says which log
to send.

On Windows without WSL, the CLI builds natively — install [Rust](https://rustup.rs), run
`cargo build -p skynet-cli --release`, then `target\release\skynet.exe setup …` from a
terminal with `ssh` on PATH — but step 2 still happens on Linux (WSL or another box), and
the built library crosses over with `--driver`. The only thing that differs at runtime is
checksum verification, which uses `certutil` where Linux uses `sha256sum`.

---

## When something is wrong

Every refusal names the cause and the next thing to do, usually with the section of the
specification behind it. Read the message before searching for it — it was written to be the
answer.

When a message is not enough, these are the questions in the order worth asking them. On a robot
they run under `sudo` (the socket is root's), and everything past `status` takes `--token`:

```bash
skynet status                      # is the supervisor up, which body, is stop reachable
skynet state                       # which of §27's conditions holds the robot where it is
skynet limits                      # which guards can run, and the input each missing one needs
skynet conformance                 # §15's runtime tests, run by the robot on itself
skynet blackbox                    # the recording's window, and what is missing from it
skynet doctor --out bundle.json    # all of the above and more, in one file
```

`skynet doctor` is the one to attach to a bug report. It reads every route §41.3 names, and a route
it could not read is recorded as unread *with the reason* — a robot refusing a request for want of
a credential is telling you something, and the bundle does not swallow it.

Underneath the CLI, the two processes log to the journal:

```bash
journalctl -u skynetk -u skynetd -f
```

`skynetk` is the control loop (the driver and the Safety Kernel) and `skynetd` is the supervisor.
They are separate on purpose (§33.1): the supervisor can crash, restart or be stopped, and the loop
keeps holding the robot. If only one of them is in trouble, that is usually the first clue.

For a driver you are writing, `skynet driver check ./libyour_driver.so` is the equivalent — it loads
the shared object, runs the simulated half of the conformance suite, and reports the tier your
evidence supports rather than a bare pass or fail.

Symptom-first reference, from install failures to fleet mysteries, is
[the troubleshooting guide](troubleshooting.md). It is held to the same mechanical checks as
this page: every command it shows is one the CLI dispatches, every `make` target exists,
every link resolves.

---

## Running the daemon by hand

`skynetd` is the supervisor (§33) and `skynet` is the CLI in front of it. Nothing here needs a robot;
`--root` puts `/data` and `/run` wherever you like.

```bash
# Stage a package. The digest is verified against the content, not taken on trust; add
# --signed-by and --signature and the release signature is checked too (§19, §37.1).
skynet install ./my-policy --root /tmp/robot \
  --id openrobot/grasp --kind policy --version 1.0.0 --digest <hex>

# Say what the robot should be running, then reconcile once and read the plan.
skynetd once --root /tmp/robot

# Or run it, with the local API in front (§33.7). Add --lan and it answers a browse, so a
# laptop on the same network can find it without being told the address (§82).
skynetd --root /tmp/robot --lan 169.254.3.4:8099 --advertise arm-01 &
skynet discover                                        # which robots are on this link
skynet status --socket /tmp/robot/run/skynetd.sock     # needs no credential (§39.1)
skynet estop  --socket /tmp/robot/run/skynetd.sock     # nor does this
skynetd credential add --root /tmp/robot --label laptop --role operator
skynet ps --socket /tmp/robot/run/skynetd.sock --token <token>

# Invoke a skill (§54.1). Accepted, not started: the next reconcile pass evaluates §54.2's
# preconditions against a live context and may refuse it, naming the clause.
skynet skill run openrobot/pick --args '{"target":"cup"}'   --socket /tmp/robot/run/skynetd.sock --token <token>
skynet skills --socket /tmp/robot/run/skynetd.sock --token <token>
```

Three things it will not do, and says so rather than pretending:

* **Nothing starts until somebody declares a safe state.** §33.4 gates it, and this edition has no
  channel to the kernel to ask — so an operator writes `yes` into `/data/state/safe-state`, and every
  report of the safe state says `declared` beside it.
* **There is no default password.** An unprovisioned robot answers status and stop and nothing else
  (§39.2), and that is the specified behaviour rather than a state to be fixed by a fallback.
* **It trusts nobody until told to.** `/data/state/trust.json` holds the keys whose signatures this
  robot believes, and there is no default there either — so a package reports `untrusted` until
  somebody writes one, and a **driver** does not activate at all (§19). That is the same
  useless-rather-than-open choice §39.2 makes for credentials.

`sh crates/skynetd-io/e2e.sh` runs the whole chain — install, verify, activate, launch, socket,
rollback — and reports the systemd launch step as not run where there is no systemd, rather than
passing a shorter exam.

---

## The corpus is the specification

An edition is any implementation that reproduces the conformance corpus and passes SCTS. Not "agrees
with", not "is equivalent to" — **reproduces, byte for byte**, because equivalence is where drift
hides. A warning in a different order today is a different clamp tomorrow.

```bash
node tools/corpus-gen/generate.ts   # regenerate from the reference oracle
cargo test -p skynet-corpus          # hold this edition to it
```

The cases cover roles, transport, cycle verdicts, limit resolution, frame graphs, forward
kinematics, the black box, admission, certification, readiness and more — the fuzzed ones generated
from seeded PRNGs written out in full, so regenerating produces byte-identical files and a diff means
a behaviour change rather than a reshuffle.

The newest of them freeze **poses**, not only verdicts. A guard trip is a boolean, and two editions
could agree on every trip in the corpus while disagreeing about where the arm is by a millimetre —
right up to the day one of them clamps and the other does not.

A divergence is *either* a specification ambiguity *or* an implementation defect. It blocks the
release of the edition that diverged and never of the others. [`editions/README.md`](../editions/README.md)
lists every edition and the command that holds it to the corpus.

---

## The processes a robot runs

| Binary | Crate | What it is |
|---|---|---|
| `skynetk` | `skynet-kernel-host` | The control loop: owns the driver, runs the Safety Kernel every cycle, records the black box |
| `skynetd` | `skynetd-io` | The supervisor: reconciles desired state, launches packages, serves the local API and the on-robot console |
| `skynetp` | `skynet-policy-host` | The policy host: the producer end of the command ring a policy writes into |
| `skynet` | `skynet-cli` | The CLI: talks to a robot, and scaffolds and checks drivers on a workstation |

The Safety Kernel itself is `skynet-kernel` — `no_std`, no allocator, no clock, its own `sin`, `cos`
and `sqrt`. [`editions/README.md`](../editions/README.md) explains why Rust lives in `crates/` rather
than beside the other languages, and [`Cargo.toml`](../Cargo.toml) lists every crate.

Why this is a separate repository from the Hub, and not a subdirectory (§72.1): a subdirectory of a
private commercial repository cannot be cleanly open-sourced later without a history-scrubbing split
or publishing commit history that leaks marketplace internals. Git history is permanent, so the
licence boundary has to exist before the code does.
