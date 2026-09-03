# Skynet OS — Technical Architecture

> Skynet has two halves. **Skynet Hub** is where robot software is discovered, versioned,
> reviewed, signed and sold — it is documented in `docs/skynet-architecture.md` and is largely
> built. **Skynet OS** is what runs that software on the machine. This document designs the
> second half end to end.
>
> The Hub can already tell you *whether* a policy fits a robot. Nothing yet makes it *fit*.

---

## 0. The thesis

The comparison everyone reaches for is Android, and it is the right one — but almost always for
the wrong reason. Android did not become universal because one binary boots on every phone. It
never did and it still does not. Android became universal because of three separable inventions:

1. **A stable vendor interface.** The HAL, and after 2017 Project Treble, let the OS and the
   silicon ship on different schedules. Before it, an OS update required every chip vendor to
   rebuild. After it, they were decoupled.
2. **A conformance suite.** CTS turned "is this Android?" from a marketing sentence into a test
   run with a pass or a fail at the end. Fail it and you lose the trademark and the app store.
   This — not the kernel, not the UI — is the mechanism that makes app portability real.
3. **A package format with declared permissions.** The APK let software be *installed* rather
   than *ported*.

Robotics has weak versions of the first (`ros2_control`'s `hardware_interface`, LeRobot's `Robot`
class), and nothing at all resembling the second or third. So a policy published for one humanoid
is a gamble on the humanoid standing next to it, and "ROS 2 compatible" is a phrase anybody may
write about anything.

And robots have a fourth problem Android never had. **The body is part of the API.** An app does
not care whether the screen is 6.1" or 6.7". A policy cares enormously whether the arm has six
joints or seven, whether joint 3 is rated for 80 Nm or 200 Nm, and whether what it calls
`left_shoulder_pitch` is what this robot calls `J2`. A phone app that mismatches its hardware
shows a squashed layout. A policy that mismatches its hardware destroys a wrist, or a person.

Skynet OS is therefore built around two primitives that have no Android equivalent, plus the
three that do:

| | Android's answer | Skynet OS |
|---|---|---|
| Vendor decoupling | HAL / Treble | **RHAL** — a versioned, stable vendor ABI (§4) |
| Conformance | CTS | **SCTS** — sim, bench and *continuous field* suites (§15) |
| Package | APK + runtime permissions | **`.sky`** — signed, budgeted, capability-gated (§8) |
| The body | *no equivalent* | **Embodiment Descriptor + Transport** (§5, §6) |
| Untrusted code | app sandbox | **Safety Kernel as the sole actuator writer** (§7) |

Those last two are the contribution. Everything else is careful engineering of ideas that already
work elsewhere.

---

## 1. What "universal" is allowed to mean

There is no binary that boots on a Unitree G1, an ABB arm, a Roomba and a warehouse AMR. Claiming
otherwise would be exactly the sort of fiction this platform exists to displace, and the claim
would collapse the first time somebody tried it.

What *is* universal is the **contract**: the same packages, the same transport planner, the same
Hub, the same conformance suite, the same CLI — reached at three depths of integration. Each tier
states plainly what it can and cannot promise, and the OS refuses to display a promise the tier
underneath it cannot keep (`resolveEnforcement()` in `lib/skynet/os/safety.ts` derives the
enforcement mode from the tier rather than accepting one as input).

| | **Guest** | **Host** | **Certified** |
|---|---|---|---|
| Runs on | A container or process on anything that can reach the robot over IP, serial or CAN | PREEMPT_RT Linux on the robot's application processor | Host, plus an independent safety MCU |
| Owns the control loop | No — the vendor stack does | Yes | Yes |
| Safety Kernel | **Advisory**: detects and records, enforces nothing | **Enforcing** | **Enforcing, redundant** |
| Stop path independent of the app processor | No | No | Yes, wired to drive-enable |
| Can be evidence in a safety case | No | No | Yes (§7.4) |
| Effort to adopt | An afternoon | A vendor porting project | A vendor porting project plus hardware |
| What it is for | Trying the catalogue, collecting data, evaluating a policy before trusting it | Production autonomy | Production autonomy around people |

**Guest is how the ecosystem starts.** It is honest about being a passenger: it computes every
verdict, logs every violation, raises every alarm — and the command still reaches the robot
unchanged, because there is nothing underneath that Skynet controls. That is genuinely useful
(an operator learns their policy commands 140% of rated torque *before* trusting it with
anything) and it is not a safety function, and every surface says so in those words.

This tiering is the load-bearing honesty of the whole design. An OS that displayed
"safety enforced" over a best-effort userspace process would be lying about the one thing it must
never lie about.

---

## 2. Decision summary

| Layer | Decision | Why |
|---|---|---|
| Base OS | Yocto-built Linux, **PREEMPT_RT**, mainline since 6.12 | Real-time is now a kernel config rather than a patch stack. 20–200 µs worst case is enough for every loop above the joint controller. Sub-10 µs work stays on the vendor's MCU where it belongs. |
| Init / supervision | `skynetd`, a single supervisor over systemd units | One thing owns lifecycle, health, reconciliation and the black box. Splitting it across shell scripts is how field fleets become unrecoverable. |
| Vendor interface | **RHAL**, a C ABI with a semver contract, drivers as shared objects | The Treble lesson. A vendor's motor code and the OS must ship on different schedules or the OS never updates. C because every robotics vendor can produce and consume it. |
| Middleware | ROS 2 is a **transport option, not a dependency** | ROS 2 is middleware, not an OS, and DDS discovery is a liability on a constrained robot. An RHAL driver may be implemented over ROS 2, EtherCAT, CANopen, gRPC or a vendor SDK; the OS does not care and does not require any of them. |
| Policy execution | Sandboxed process, no actuator access, posts desired commands to a ring buffer | Most published checkpoints are pickles that execute code on load. Without this, every published model is remote code execution attached to a two-metre humanoid. |
| Safety | **Safety Kernel is the only writer to an actuator**, always | The single invariant that lets the rest of the system be permissive. |
| Real-time boundary | Kernel and RHAL in the RT path; policy inference is *not* | Inference jitter is unbounded. A design where a slow forward pass delays a torque command is a design that falls over. The kernel runs at the joint rate regardless of what the policy is doing. |
| Cross-body portability | **Embodiment Transport**, resolved once at install, enforced every cycle | §6. The alternative — hand-porting each policy to each robot — is what the catalogue exists to end. |
| Packages | Content-addressed `.sky` bundles reusing the Hub's digests, signatures and scan verdicts | A parallel artifact format would fork provenance. Everything the Hub already proves about bytes should keep meaning on the robot. |
| Permissions | Capability tokens granted per package per robot by an operator, revocable, enforced by the `mode_gate` guard | Android runtime permissions, with physical consequences. `manipulation.grasp` is a permission, not a feature flag. |
| Fleet model | **Desired state, pulled and reconciled** — never pushed | A robot offline for a week must converge on reconnect without anyone intervening. Push assumes a link that robots do not have. |
| OTA | A/B slots + **RAUC**, signed by the Hub's existing Ed25519 keys, health-gated rollback | A/B costs storage and buys unattended recovery, which is the only property that matters when the device is in a warehouse at 3 a.m. |
| Observability | **Black box** ring buffer: every cycle, sealed on fault | Aviation's answer. Also the only credible route to insurability and to answering a regulator. |
| Conformance | **SCTS**, with a field suite that can suspend an award | A certificate that cannot expire is a certificate that lies with increasing confidence. |
| Offline | **No control decision may require a network round trip.** A conformance test measures it | Stated as an invariant because it is the property everything else assumes. |
| Identity | Existing `skd_` device tokens, upgraded with TPM/secure-element binding and image attestation | Reuse the Hub's credential class with the smallest blast radius; add proof of *what* is running. |

### Rejected alternatives

* **Build on ROS 2 as the foundation.** ROS 2 is a good transport and a poor foundation: it is
  middleware, its security story (SROS 2) is optional in practice, DDS discovery misbehaves on
  constrained and multi-homed networks, and depending on it would exclude every robot whose
  vendor stack is not ROS. Supporting it as one RHAL transport gets the ecosystem without the
  coupling.
* **A microkernel or a certified RTOS (QNX, seL4, Zephyr) for the whole system.** Right for the
  safety kernel, wrong for the system: the policy runtime needs CUDA, PyTorch, ONNX Runtime and
  a package manager, and none of that exists on a certified RTOS. The design instead *splits* —
  Linux for the runtime, a small kernel that can also be compiled for an MCU. This is what the
  certified tier is.
* **A "universal" ROS 2 message-level abstraction.** Tried repeatedly, and it collapses to a
  lowest common denominator that no serious robot can use, because the interesting capabilities
  are exactly the ones that differ. The Standard Robot Interface avoids this by making
  capabilities *typed tokens* with an explicit extension channel, so a policy needing a vendor
  feature declares the dependency instead of being silently degraded.
* **Retargeting policies on the Hub and shipping per-robot variants.** Multiplies the catalogue
  by the robot registry, and produces artifacts nobody signed. Transport is resolved from
  declarations at install time on the robot, and the plan is reproducible because it is a pure
  function of two digests.
* **Trusting the vendor's declared limits without measurement.** The bench suite measures joint
  ratings under load precisely because a datasheet is a marketing document until somebody checks
  it. A safety kernel enforcing unverified numbers is a safety kernel enforcing a rumour.
* **Self-certification.** Vendors run SCTS themselves — the results are signed telemetry, and the
  field suite keeps measuring after the badge is issued. Absence of a result is never a pass
  (`awardCertification()` returns it as `missing`), which is the loophole every self-certification
  scheme otherwise dies of.

---

## 3. The stack

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

The double rule is the trust boundary. Everything above it may be untrusted, downloaded five
minutes ago, and written by a stranger. Nothing above it can move a joint except by asking the
thing below it, which says no when the answer is no.

---

## 4. RHAL — the vendor interface

A vendor implements one shared object exporting a small C ABI. That is the entire integration.

```c
typedef struct {
  uint32_t abi_version;                  /* semver-major; refuse on mismatch */
  int (*open)(const char *config, rhal_ctx **out);
  int (*close)(rhal_ctx *);

  /* Called at the joint rate. Must be wait-free and must not allocate. */
  int (*read_state)(rhal_ctx *, rhal_state *out, uint64_t deadline_ns);
  int (*write_command)(rhal_ctx *, const rhal_command *, uint64_t deadline_ns);

  /* Must be callable from a signal handler and from a failing process. */
  int (*emergency_stop)(rhal_ctx *);

  int (*describe)(rhal_ctx *, char *json_out, size_t len);   /* the embodiment descriptor */
  int (*read_sensor)(rhal_ctx *, const char *name, rhal_frame *out);
  int (*extension)(rhal_ctx *, const char *token, const void *in, void *out, size_t len);
} rhal_driver_v1;
```

Four properties make it a *contract* rather than an interface:

* **`abi_version` is checked at load and mismatches refuse.** A driver built against v1 never
  silently runs on v2. This is the whole Treble idea in one field: the OS ships weekly, the
  driver ships when the vendor has something to say, and neither blocks the other.
* **`read_state`/`write_command` are wait-free and non-allocating.** They run inside the real-time
  path. A driver that takes a mutex held by a non-RT thread is a driver that causes a priority
  inversion at 1 kHz, and `rhal.state.rate` in the bench suite measures the worst-case gap
  precisely to catch it.
* **`emergency_stop` must work from a failing process.** It is called from a signal handler on a
  crashing runtime. A driver implementing it as "publish a ROS message and wait for an ack" fails
  the test that matters most.
* **`describe` returns the embodiment descriptor.** The body description is not a file somebody
  maintains in a wiki — it comes out of the driver, so it cannot drift from the hardware it
  describes without the driver being wrong.

`extension` is the escape hatch, guarded by a token so a policy using a vendor feature declares
the dependency rather than reaching for it. It mirrors `StandardRobotInterface.extension()` in
`lib/skynet/robots/interface.ts`, which already exists on the Hub side.

**Reference drivers ship with the OS** for ROS 2 (`ros2_control` resource manager), plain
`ros2_control`, EtherCAT (SOEM), CANopen, MuJoCo and Isaac Sim (simulator drivers are how the sim
suite runs at all), and — see §16 — LeRobot.

---

## 5. The Embodiment Descriptor

*Implemented: `lib/skynet/os/embodiment.ts`.*

The body, made explicit and typed: joints with vendor wire names, canonical roles, per-joint
limits and accepted control modes; end effectors; sensors with modality and frame; control modes,
rate bounds and **command latency**; the safety envelope; the IK solvers actually resident on the
robot; and the **capability tokens this body and its adapter provide**. Digested with sha256 and
signed by the vendor exactly as any other Skynet artifact, so `embodiment.descriptor.signed` in SCTS
is a real check rather than a checkbox.

The digest and the signature answer different questions, and the robot reports them separately: the
digest identifies **which document this is**, and the signature identifies **who stands behind it**.
A console that showed only the digest would let a number that is a fact about a file imply a
provenance it does not carry. The signature is detached — `state/embodiment.sig.json` beside the
descriptor (§28) — because the digest is taken over the descriptor's canonical form, so a signature
written inside the document would change the digest it is a signature of.

### 5.0 Why `capabilities` is here and not somewhere else

§8 defines a package's side of the permission model completely — what it declares it needs, what it
requests, what the operator grants — and left the robot's side assumed. `admitPackage()` takes a
`capabilities` list described as "the tokens the robot plus its adapter actually provide", and until
this section nothing said what determined it. The consequence was concrete: admission could not be
wired onto the install path at all, because the only two ways to supply that set were to invent a
mapping from body parts to tokens, or to leave it empty and refuse every package on a working robot.

It belongs in the descriptor for two reasons.

**The descriptor is produced by the adapter, for the body**, so "the robot plus its adapter" is
exactly the question a driver's `describe()` is in a position to answer. Nothing else on the robot
sees both halves.

**§11's invariant is that no decision on a robot requires a round trip.** Fetching the set from the
Hub at install time would make a package installable only while connected, on a machine the design
expects to spend a week offline. The Hub keeps its own copy in the registry — that is what the
compatibility engine answers "will this run?" from, before anybody downloads forty gigabytes — and
the two are the same fact recorded in the two places that need it.

**Empty is a claim, not an absence**, and this is the one place the descriptor's usual rule inverts.
Everywhere else absent means "nobody declared it" and the consumer reports unavailability. Here a
missing list would leave admission choosing between refusing every package and admitting every
package, and there is no honest third option — so an absent list reads as *provides nothing*, and
the vendor is required to write it down. The failure direction is refusal, which is visible, names
the tokens, and is fixed by declaring them.

The token vocabulary is §8's, and `extensions` carries the ones nobody standardised — the same
extensibility rule §5.1 applies to roles, for the same reason: a vendor token must be expressible
without being mistaken for a canonical one.

### 5.1 The canonical role vocabulary

This is the mechanism that makes cross-body work possible at all, and it is deliberately modest.

```
role  := chain "." link [ "." axis ] [ "#" index ]
chain ∈ left_arm right_arm left_leg right_leg left_hand right_hand
        torso neck head base tail aux
axis  ∈ pitch yaw roll slide flex abduct rotate grip

left_arm.shoulder.pitch    right_hand.finger.flex#2    base.wheel#0
```

Wire names stay vendor-specific; the *semantics* become shared. This is the USB descriptor move,
and it is the smallest change that lets two vendors' naming schemes be related mechanically
instead of by an engineer reading two URDFs side by side.

`aux` exists so a body plan nobody anticipated — a snake robot's segments, a second mast, a
sensor boom — can still declare roles that parse, rather than being pushed into a wrong-but-close
chain because the vocabulary had no honest slot for it.

**The extensibility rule is the important part:** `parseRole()` returns `null` for anything it
does not recognise, and a joint with an unparseable role can only ever be matched by *exact
name*. An unrecognised role never silently maps to something plausible. A body the vocabulary
does not describe stays usable — it just cannot participate in remapping, which is truthful,
because nobody has established what its joints mean.

### 5.2 Why `command_latency_ms` is in the descriptor

Almost nobody publishes it, and it is the number that decides whether a policy trained at 50 Hz
is stable on this body. Requiring it costs a vendor one measurement (`rhal.command.latency` in the
bench suite measures it for them) and turns "it feels sluggish on our robot" into an arithmetic
check performed before download.

---

## 6. Embodiment Transport

*Implemented: `resolveTransport()` in `lib/skynet/os/embodiment.ts`.*

The heart of the system. For each channel of the policy's action space, what does this body do
with it?

| Method | Meaning |
|---|---|
| `direct` | The joint name matches exactly. |
| `remap` | Different wire name, same canonical role. |
| `retarget` | Different kinematics — driven through task space by an IK solver. |
| `synthesize` | No comparable structure, but the command's *meaning* transfers (base velocity from a legged policy onto a wheeled base). |
| `drop` | No equivalent, and the publisher declared the channel optional. |
| `blocked` | No equivalent, and the policy requires it. |

The plan's verdict is the worst binding in it: `refused` › `degraded` › `retargeted` › `direct`.
One blocked required channel refuses the whole plan, because a policy missing an actuator it
depends on does not do most of its job — it does something nobody has characterised.

### 6.1 Four decisions worth defending

**Retargeting is a chain-level decision, never per-joint.** You cannot send an elbow through task
space while its shoulder goes direct. `decideRetargetChains()` therefore resolves whole chains
first, and every channel on a retargeted chain is marked together.

**Retargeting is off by default** (`allowRetarget`). It changes what the policy is doing. That
should be a decision somebody made, not a fallback that happens quietly — and if the body has no
solver installed, the request is refused rather than promised and discovered at the first command.

**Scaling never guesses.** When a policy declares it peaked at 180 Nm and the target joint is
rated for 80, `scaleFor()` returns 0.44 and raises a `scaled_down` warning naming the joint. When
*either* number is missing it returns 1 and warns `unscalable_channel`. Inferring the missing side
would be inventing a safety-relevant figure, which this codebase refuses to do anywhere —
consistent with `skynet_robots.limits` being nullable rather than defaulted.

**Control rate clamps down, never up.** Running a policy faster than it was trained changes its
dynamics. And `rate_below_watchdog` is the warning that matters most: a loop slower than the
controller's timeout faults *while the robot is moving*, which is worse than refusing to start.

### 6.2 Why the planner is pure, and runs in two places

`resolveTransport()` has no I/O, no clock and no database. It runs:

* **on the Hub**, so a repository page answers "does this run on my G1, and how well?" *before*
  anyone downloads forty gigabytes — served at
  `GET /repos/{ns}/{name}/versions/{v}/transport?robot=`;
* **on the robot**, at install time, with no network at all.

Both compute the same function over the same declared facts, so neither is authoritative and they
cannot disagree. This is the pattern `lib/skynet/loading.ts` already established for load plans,
extended to bodies.

> **Corrected in §23.2.** The implementation here is TypeScript, which a robot cannot run — so as
> written this section assumes a second implementation on the robot, and two implementations can
> drift. The contracts move to a Rust crate consumed as WASM by the Hub; the TypeScript stays as
> the reference and differential-test oracle. One implementation compiled twice cannot disagree
> with itself, which is a stronger guarantee than the one this section claims.

### 6.3 What it deliberately does not do

It does not *perform* the retargeting. Solving IK is the runtime's job, in the loop, in a
real-time context. This module decides **what is possible and under what ceilings**, and hands the
ceilings to the Safety Kernel — which is the only component that can enforce them. Deciding is
complex and runs once; enforcing is trivial and runs at 1 kHz. Separating them is what makes both
tractable.

---

## 7. The Safety Kernel

*Implemented: `lib/skynet/os/safety.ts`.*

> **The Safety Kernel is the only component with write access to an actuator.**

A policy does not command a joint. It writes a *desired* command into a ring buffer the kernel
reads. This is not a stylistic preference. The Hub's artifact scanner already reports that most
published checkpoints are pickles, and unpickling executes code by construction. Any design where
loading a model can move an arm makes every published model a remote code execution vulnerability
bolted to a humanoid. Making the kernel the sole writer is what lets Skynet run untrusted policies
*at all* — and running untrusted policies is the entire point of having a catalogue.

### 7.1 The guards, in evaluation order

Order is part of the specification, not an implementation detail.

| # | Guard | Checks | Response on trip |
|---|---|---|---|
| 1 | `watchdog` | A command arrived inside the window and its sequence advanced | `stop_cat2` |
| 2 | `envelope` | Per-joint position, velocity, torque vs. rating and transport clamp | `clamp` |
| 3 | `rate_limit` | Change per cycle vs. the jerk ceiling | `clamp` |
| 4 | `workspace` | Commanded pose vs. operator-defined allowed volumes, in the frames of §44 | `hold` |
| 5 | `self_collision` | Commanded configuration vs. the body's collision model, one step lookahead | `hold` |
| 6 | `balance` | Centre of pressure inside the support polygon; fall-arrest reflex takes the loop | `stop_cat2` |
| 7 | `proximity` | Commanded speed vs. distance to the nearest detected person (speed and separation monitoring) | `stop_cat1` |
| 8 | `mode_gate` | The command's capability is one the operator granted this package | `hold` |

**The watchdog runs first** because a stale command must be *discarded*, never clamped and
forwarded — clamping it would keep a dead producer's last intention alive at a legal magnitude,
which is exactly the failure the watchdog exists to prevent.

**Rate limiting runs after magnitude clamping** because clamping creates step changes of its own.
Getting this order wrong yields a kernel that passes every unit test and still whipcracks an arm
the first time it clamps.

Responses are graduated — `clamp` › `hold` › `stop_cat2` › `stop_cat1` › `stop_cat0`, following
IEC 60204-1 — because a kernel whose only response is a hard stop is a kernel operators disable.
A humanoid that drops a crate whenever a velocity estimate blips is worse than no policy at all.

**The column is what a guard demands when it trips, not everything it does.** `proximity` is the one
row where the difference bites: inside the stop distance it demands `stop_cat1`, and *between* the
two distances it scales the command and lets it through, which is a `clamp`. A verdict deriving its
response from the row rather than from what the guard demanded that cycle reports a stop on a cycle
the robot kept moving through, and a black box recording that is a black box recording something
that did not happen. A cycle's response is therefore the strongest response actually demanded, and
a `hold` or stronger means the command does not go out at all.

### 7.2 Guards that cannot run say so

> **Extended in §45.5.** An *expired calibration* is a cause of unavailability alongside a missing
> input — a guard built on extrinsics nobody has re-measured since the robot hit something is not a
> guard, and it names the expired parameter set rather than assuming the numbers still hold.

`resolveLimits()` partitions the guard set into `active_guards` and `unavailable_guards`, each
with the specific input that is missing — no IMU, no collision geometry, no person-detecting sensor,
no configured jerk ceiling. A guard whose inputs are absent is reported as **unavailable**, never
as passing. And the `balance` guard on a wheeled base reports "not applicable to this body" rather
than "missing an IMU", so nobody reads a shape difference as a defect.

**The evidence is asked for, never the claim.** An earlier form of this asked the descriptor for a
boolean — "does this body have a collision model?" — which a descriptor could answer `true` with no
geometry in it, producing a guard reported as running that had nothing to evaluate. The pose guards
ask the frame graph instead: `workspace` needs volumes *and* a tree that reaches every frame they
name, `self_collision` needs two or more frames carrying a shape the kernel can place. Both are
facts about the document rather than assertions in it, and both name the missing frame (§44.5).

### 7.3 Limits compose by minimum, always

Three sources of ceilings — the robot's declared envelope, the transport plan's per-channel
limits, and the operator's profile — and the rule between them is always the *narrowest*, never a
precedence order. No source can raise what another lowered, including a mistaken or malicious
operator profile, which is why the profile is folded in here rather than applied afterwards. An
attempt to widen is recorded as a `profile_widened` notice and then discarded by the minimum:
silently ignoring it would leave an operator believing a limit they set is in force.

**A workspace volume is not a number, and the minimum rule does not apply to it.** Layers of allowed
volumes (§38.2) compose by **conjunction**: a frame must lie inside at least one volume of *every*
layer that names it. That is the same guarantee expressed as a predicate rather than as geometry —
intersecting a box with a sphere is a shape that is neither, and a composition rule producing a
third shape would be one no operator could check by reading their own configuration. A layer says
nothing about frames it does not name, so a site rule about one arm does not silently confine the
other.

### 7.4 What the certified tier is and is not

```
SAFETY_CLAIM_RULE:
  Skynet OS supplies evidence for a safety case. It does not certify a machine. Conformity
  assessment against ISO 10218, ISO 13482 or any other standard is performed by a notified
  body against a specific robot in a specific application.
```

This constant lives in the code and is rendered on every surface mentioning the certified tier.
ISO 10218:2025 brought humanoids partly into scope (it no longer excludes them outright, though
it does not govern their mobility hazards) and made functional-safety requirements explicit; ISO
13482 covers service robots. Neither is something software confers. The tier is an architecture
that makes a safety case *possible* — a stop path that survives the application processor dying,
a kernel small enough to argue about, an audit trail. It is not a certificate, and shortening the
sentence would be the most damaging thing this product could do.

### 7.5 Reference implementation and parity

The kernel on the robot is C, allocation-free, running against a preallocated frame. The
TypeScript in `safety.ts` is the *reference*: the specification the C is written against, the
thing the Hub runs to show an operator what will be enforced before they deploy, and one half of
the `safety.kernel.parity` conformance test — which feeds identical cycle vectors to both and
requires identical verdicts. Two implementations of one specification, held to each other by the
suite.

### 7.6 Resolving on a robot, and the tier that is not one

`resolveLimits()` composes three sources into the one set the kernel enforces (§7.3), and for a
long time nothing on a machine ever called it. Its callers were the kernel's own suite, the
corpus, and the Hub's renderer — all of which hand it a document. The robot-side composition is
`skynetd-io::limits`, served at `GET /limits` and printed by `skynet limits`, and it is worth
stating what it reads and what it cannot.

**The tier is an input, and the robot does not have one.** §15 awards a tier from bench evidence a
human witnessed and §11 would deliver the award in desired state; neither path is written, so
nothing on a robot decides its own tier and a module that chose one would be inventing the fact the
whole arrangement exists to prevent. The daemon therefore resolves at **guest** and says so, which
is defensible only because of a property of `resolveLimits()` itself:

> The tier changes what is done with the limits. It never changes what they are.

Every ceiling, every active guard and every unavailable guard is identical at all three tiers. What
the tier decides is the enforcement mode and two notices — `advisory_only` and
`no_independent_stop`. That is asserted by a test rather than assumed, and the day it fails is the
day the assumption has to be replaced by an awarded tier rather than defended.

**Two of the operator profile's fields had no delivery path, and now have one.** `resolveLimits()`
accepts seven, and §11's desired state carried four ceilings and §55.2's cell. `proximity` and
`self_collision_margin_m` are on the profile too, so both arrive under the profile's own signature
rather than through a second path (§38.1).

They land differently, and the difference is worth stating. `self_collision_margin_m` changes what
the machine does: §7.1's fifth guard holds at a distance instead of waiting for two surfaces to meet,
and `no_collision_margin` stops being emitted on every robot alive. `proximity` changes what the
machine *says* — it also needs `available.people_detection`, so the guard stays unavailable and now
names the sensor it has not got rather than a profile field that could not be delivered. That is the
difference between *"this platform cannot do it"* and *"this robot has not got one"*, and only the
second is a fact about the body.

**One thing to know about the margin**, because it is the only figure in the block that behaves this
way: it is a **floor**. Every other limit here is a ceiling, where a smaller number is stricter; a
larger margin keeps two bodies further apart. §33.4's asymmetry — *narrowings apply immediately,
widenings wait* — is computed by a classifier that assumed smaller-is-stricter, and running a margin
through it would have classified halving one as a narrowing and applied it under a running policy.
Two thresholds of `proximity` are floors for the same reason, and its `slow_factor` is a ceiling.

**`people_detection` is the third and it is a different kind of absence**: §5 declares nine sensing
modalities and none of them is a person detector. A camera is not one — detection is a perception
component, not a sensor — and inferring one from `rgb` would be exactly the
claim-about-the-body-that-nothing-checks §7.2 removed two fields for.

**A guard can be active and have nothing to compare against.** `envelope` has no missing input, so
it is active on every robot; the ceiling it applies to a joint comes from the transport plan's
`per_channel` (§6.3), and a robot running no policy has no plan in force. The whole-body velocity
and torque figures are deliberately not applied per channel, because nothing at this layer knows
whether a given channel carries radians per second or newton-metres. So a robot can report
`envelope` active, hold a declared envelope, and clamp nothing — and reading "active" as "enforcing
a ceiling" is the wrong reading and the natural one. The route says so in a sentence rather than
leaving it to be derived from two fields.

#### It crosses §17, and it is relayed rather than recomputed

§17's check-in carries a `limits` block: the ceilings with their provenance, the guard partition
with every missing input named, and two fields a consumer must not collapse. `enforcement` is what
the tier permits; `enforced` is whether anything on the robot applies them, which on an edition with
no policy runtime is false at every tier. A fleet page holding only the first would render a green
guard list over a robot that enforces nothing.

The block belongs on the check-in rather than on a route of its own because §34.4 collapses
check-ins on the grounds that *every field in one is a statement about now* — and these are: the
limits in force this pass, not a count, a delta or a history. It is about five hundred bytes, which
is what a robot that has been offline for a week should reconnect saying exactly one of.

The Hub **relays it and recomputes nothing**, and the restraint is load-bearing rather than
fastidious. The Hub holds a copy of the descriptor and a mirror of `resolveLimits()`, so a computed
table would typecheck — and would report `balance` on a fixed base as a missing IMU, where §7.2
requires *"not applicable to this body"*. It would also report the limits a robot *should* be
running rather than the ones it is, which is the failure §17's own rule is written against.

Notices cross as **codes and subjects, never sentences**. Both sides hold the same catalogue, and a
robot shipping English prose would put the translation into thirty-five locales (§32.3) on the side
that cannot do it. `guard_unavailable` is carried even though the guard table beside it says the
same thing: the wire is evidence and a renderer decides what to show.

---

## 8. Packages and permissions

*Implemented: `lib/skynet/os/package.ts`.*

A `.sky` package reuses everything the Hub already proves about bytes — content addressing, the
version digest, the Ed25519 release signature that is never stored unverified, the scan verdict —
and adds a declaration: capability tokens needed, embodiment requirements, a **resource budget**,
and the permissions it asks the operator to grant.

### 8.1 The budget is mandatory, and that is the point

A phone that runs out of memory kills an app. A robot that runs out of memory mid-stride falls
over. `admitPackage()` refuses a package the robot cannot feed rather than admitting it and
discovering the shortfall at 60 kg of angular momentum. This is the one place the format is
stricter than an APK and the reason is physics.

`inference_budget_ms` is declared by the publisher and *measured* by the conformance suite, so a
package that overruns its own claim on real hardware can be refused before it is trusted with a
loop.

### 8.2 Permissions are the Android runtime-permission moment

`manipulation.grasp` is a permission, not a feature flag. An operator grants it, per package, per
robot, revocably, and every grant is audit-logged. The `mode_gate` guard enforces it at every
cycle — so revocation takes effect on the next command rather than on the next restart.

Two rules keep it from decaying into "grant everything":

* **A package cannot request what it did not declare a need for.** `requests` must be a subset of
  `embodiment.capabilities`, or the install fails with `over_request`. This closes the
  "declare a little, request a lot" path.
* **Ungranted is a warning, not an error.** The package installs and simply cannot use the
  capability until the operator says yes. Refusing the install would train operators to
  pre-approve everything to make the error go away.

And `admitPackage()` returns *all* findings rather than the first, because fixing four problems
one deployment at a time is precisely how people end up granting everything.

### 8.3 Isolation has no override

Only a `driver` may run `native`. A `policy` is always sandboxed — no flag, no override, no
`--privileged`. An escape hatch here would quietly undo the invariant §7 is built on, so there
isn't one.

---

## 9. skynetd — the system

> **Expanded in §33.** This section is a sketch; `skynetd` is the largest component in the system
> and Part IV specifies it properly — the not-safety-critical property that licenses its
> complexity, the level-triggered reconciler, reconcile ordering and the safe-state barrier,
> restart and crash-loop policy, the liveness/readiness distinction, and its own failure modes.

One supervisor, owning:

* **lifecycle** — start, stop, restart and crash policy per package, with the kernel always
  outliving whatever it supervises;
* **reconciliation** — the loop in §11;
* **health** — package liveness, budget accounting, thermal and battery state, guard trip rates;
* **the black box** — §13;
* **OTA** — §12;
* **the local API** — a Unix socket carrying the same REST shapes as the Hub, so the CLI and SDK
  work identically against a robot and against the cloud. `skynet ps`, `skynet logs`, `skynet
  install`, `skynet estop` are the same verbs in both places.

Units are systemd units; `skynetd` supervises above them rather than replacing them. Reinventing
process supervision would be a year of work to reach parity with something already on the device.

---

## 10. Identity, pairing and attestation

The Hub already has the right credential class. `skynet_devices` issues `skd_` tokens that
**cannot read repositories or claim jobs** — deliberately, because a credential physically
present on a robot in the field is the easiest one in the system to lose. Skynet OS keeps that
and adds two things:

* **Hardware binding.** Where a TPM or secure element exists, the device key is generated in it
  and never leaves. `security.identity.attested` checks this.
* **Image attestation.** Every check-in carries the digest of the running OS image, the running
  policy and the embodiment descriptor. The Hub records not "robot X is online" but "robot X is
  running image `sha256:…`, policy `sha256:…`, against body `sha256:…`" — which is the sentence an
  incident investigation needs and nobody can currently produce.

Pairing is a one-time operator action: `skynet pair <code>` on the robot, confirmed in the Hub UI,
minting a device token shown exactly once.

---

## 11. Desired state and reconciliation

**The Hub stores intent. The robot reconciles.** Never push.

> **Mechanism in §33.3–33.4.** The loop is level-triggered rather than edge-triggered, which is
> what lets a robot converge from any state including ones nobody anticipated, and the actions it
> emits are ordered behind a safe-state barrier.

```
GET /devices/{id}/desired  →  { os: {channel, version}, packages: [...],
                                policy: {digest, grants}, profile: {...}, generation: 41 }

  robot: compare against local state → resolve transport → admit → stage → apply at safe state
  robot: POST /devices/{id}/checkin { generation: 41, applied: true, health, attestation }
```

Pull-and-reconcile rather than push, for one reason: a robot offline for a week must converge on
reconnect without anyone intervening. Push assumes a link robots do not have — in a basement, in a
warehouse aisle, on a construction site. The `generation` counter makes it idempotent, and
`offline.reconcile.resumes` in the bench suite measures the week-offline case directly.

**The invariant this rests on:** *no control decision may require a network round trip.* The
transport plan, the clamps, the packages and the grants are all resident. The Hub is for
provisioning and reporting; it is never in the loop. `offline.control.independent` removes the
network mid-task and requires the robot to finish.

---

## 12. OTA

A/B slots managed by **RAUC** — chosen over SWUpdate for its breadth of flash backends, and over
OSTree because A/B's storage cost buys a genuinely unattended recovery path, which is the only
property that matters when the device is in a warehouse at 3 a.m. Bundles are signed with the same
Ed25519 machinery the Hub already uses for release signatures, so there is one signing story
across the product rather than two.

Three rules that are not standard practice:

1. **Never apply mid-task.** An update issued while the robot is holding something is staged and
   applied at a declared safe state. `ota.safe_state.respected` measures it.
2. **Health-gated rollback.** The new slot must pass a health check within a boot budget or the
   bootloader returns to the previous slot unattended. `ota.rollback.works` stages a deliberately
   broken image and requires unattended recovery.
3. **OTA is not a route around certification.** An update that changes the Safety Kernel or the
   envelope is **refused** without a fresh bench pass. Without this rule, every guarantee in §7
   and §15 is one silent update away from being void — so `ota.safety_gate` tests it, and it is
   the single most important test in the suite.

Rollout is staged through deployment rings (canary → ring 1 → fleet) with automatic halt on a
rise in guard trip rates, which the black box already measures.

Getting the bytes is §75, which was written after building both halves found that this section
gates *applying* an image and nothing anywhere fetched one.

---

## 13. The black box

Every control cycle, appended to an on-disk ring buffer:

```
sequence · tick · monotonic ns · observation hash · policy digest · policy output
     · kernel verdict (guards tripped, clamps applied) · final command · RHAL result
     · capability token · nearest-person distance
```

Sealed and uploaded on any fault, estop, or operator request. **The seal is taken by the raise and
the bytes do not cross yet** — see §77, which was written when the two halves were built and the
incident turned out to be a document with no window attached to it.

> **Amended by the implementation.** Three additions, each of which the first version of the record
> made impossible rather than merely awkward.
>
> **The last two fields exist so that replay can reach all eight guards.** The mode gate evaluates
> the capability the command exercised and the proximity guard evaluates the distance to the nearest
> person (§7.1); neither is derivable from the other fields, so a recording without them cannot
> recompute a verdict that turned on either, and a replay would report a divergence on correct
> behaviour. A capability is recorded as an index into a table in the ring header, alongside the
> policy digests, because both are fixed for the life of a lease and neither survives being written
> into a record the next wrap overwrites.
>
> **Every record carries a sequence number and a CRC.** The sequence numbers are dense and
> increasing, which is what makes `telemetry.blackbox.complete` a check rather than a claim: a hole
> inside the window is visible as a hole. The CRC is what makes a write interrupted by power loss
> report itself as damaged instead of deserialising into a plausible cycle.
>
> **"Roughly an hour" is a consequence, not a setting.** The ring is sized by a **byte budget**, and
> what that buys is computed and reported. The conversion is not a detail: a 30-joint humanoid
> writes about 936 bytes a cycle, so an hour at a 1 kHz joint rate is roughly **3.4 GB**, and a
> 512 MB ring holds about ten minutes of it. An operator who asked for an hour and got ten minutes
> should be told ten minutes; a constant named `ONE_HOUR` in front of a ring that holds a fraction
> of one is the class of invented number this specification exists to prevent. Three things this makes possible
that are impossible today:

* **Deterministic replay.** Given the same policy digest, observation stream and embodiment
  digest, replay reproduces the same actions. `telemetry.replay.deterministic` requires it. This
  turns "the robot did something strange on Tuesday" from an anecdote into a regression test.
* **Field conformance.** The ratio of cycles the kernel did not have to intervene in, computed by
  `fieldConformance()`, aggregated per robot per fleet — the input that can suspend a
  certification (§15).
* **Insurability and regulatory answerability.** An underwriter or an inspector can be shown
  exactly what was commanded, what was enforced, and why. This is a genuine commercial wedge and
  not a feature anybody currently ships.

Note the deliberate choice in `fieldConformance()`: a robot reporting zero cycles returns
`undefined`, not a flattering 1.0. Measuring "violations detected" would reward a broken detector.

---

## 14. Fleet

Fleet management is the same reconciliation loop, addressed to a set. A fleet is a saved query
over devices (by robot model, site, tier, tag). Rollouts, profiles and grants apply to the set;
each robot still pulls and reconciles independently, so a fleet operation is not an outage
waiting to happen and there is no orchestrator whose failure stops a hundred robots.

The console shows, per robot: tier and enforcement mode, running digests, guard trip rates, field
conformance, certification state, OTA slot and generation lag. Nothing on that screen is inferred
— every field is either reported by a check-in or explicitly absent.

---

## 15. SCTS — the Skynet Compatibility Test Suite

*Implemented: `lib/skynet/os/conformance.ts`.*

The mechanism that decides whether "universal" is a claim or a fact. Three suites, because they
measure different kinds of truth:

| Suite | Where | Cost | Proves |
|---|---|---|---|
| `sim` | CI, on every commit | free | **the contract** — the driver answers RHAL correctly, the descriptor parses, the kernel reaches the specified verdict |
| `bench` | the physical robot, once, with a human present | a day | **the machine** — the estop actually cuts power within the claimed latency, the joint rated for 80 Nm refuses 90 |
| `field` | continuous telemetry, forever | free | **that it stayed true** |

The third is the one nobody else does, and it is where certification stops being a document and
becomes a live property.

36 tests across 9 areas, deliberately short. A suite that tests three hundred things is a suite
nobody runs and every vendor games; one that tests the three dozen properties the rest of the system
actually depends on is one that can be *required*. Every test exists because some other part of
Skynet OS would be unsound without it.

The count grew because §52 collected the descriptor fields Parts IV and V added and named a test
for each, and five of those tests did not exist. A schema obligation whose “measured by” column
points at nothing is a field a vendor may fill in with anything, so the five were written:
`embodiment.frames.complete`, `safety.command_loss.behaviour`, `safety.homing.envelope`,
`rhal.sensor.latency`, and `power.safe_state.reachable` — which is the whole of the ninth area,
because the energy a robot keeps back to stop with is not a property of any of the other eight.

`embodiment.capabilities.declared` is the thirty-sixth, and it arrived *with* the field it measures
(§5.0) rather than five sessions after it. That is the rule the expansion above was meant to
establish: a descriptor field and its measure are one change.

The measurements that matter most are the ones taken **at the joint rather than reported by the
driver**: `safety.estop.latency`, `safety.envelope.clamps`, `embodiment.limits.honest`. A driver
returning success is not evidence.

### 15.1 Who runs which half, and why nothing on a robot signs

§15's three suites have three runners, and for a long time two of them existed.

* **`sim`, driver-facing** — `skynet driver check`, against a built shared object, with no hardware.
* **`sim`, runtime-facing** — eight tests that are properties of the *runtime* rather than of a
  driver, which that command lists by name as **not covered** because a driver can neither pass nor
  fail them. `GET /conformance` on the robot is the runner for those: it executes them against that
  robot's own installed code — the same `admitPackage` its installer runs, the same `admitBundle`
  its updater runs — and reports the rest as `skipped` with a sentence naming what is missing and
  whose it is.
* **`bench`** — a human, a physical robot and a day. There is no substitute and this does not
  pretend to be one.

**A robot does not sign its own results, and that is a rule rather than a gap.** §19 is explicit
that a robot verifies and never signs; a certificate a machine issued about itself would be worth
exactly what it cost to produce. So what `GET /conformance` serves is a *report*: unsigned, carrying
no bench and no field evidence, and therefore always at or below what a real award would be. The
Hub awards, from evidence a person witnessed.

The runner's own first rule is §15's, applied to itself with more force than to anything it runs: a
conformance runner that reported `pass` for a test it did not perform would be the most damaging
component this platform could ship. So every entry is either a check that executed or a named
absence, and there is no third state.

**One consequence worth stating out loud.** The first robot to run this failed
`package.signature.required` — a guest-tier test — because §37.1's `admit` stage *reports* rather
than gates for every package kind but a driver, and an unsigned package installs. That is a known
and deliberate position (nothing on a robot can produce a signature, and refusing every locally
built package would refuse the golden path), and reporting it as `skipped` would have hidden a
decision behind an absence. It is a `fail`, it caps the evidence below guest, and a platform whose
own suite says so about it is a platform whose suite means something.

### Award rules

* **Absence is not a pass.** A required test with no result is `missing`, never a silent success.
* **The evidence caps the award, not the claim.** A vendor claiming `certified` whose independent
  stop path was never measured is awarded `host`, with the reason recorded — rather than refused.
  Downgrading tells the truth and keeps them in the programme; refusing sends them off to write
  "ROS 2 compatible" on a web page instead, which helps nobody.
* **Field conformance suspends rather than downgrades.** Below 0.999 the award is suspended
  pending re-measurement, because a robot whose commands have been leaving its envelope is not a
  lower-tier robot — it is one whose certificate no longer describes it. The floor is 0.999 rather
  than 1.0 because a hundred robots at 100 Hz produce tens of millions of cycles a day, and
  demanding literal perfection would suspend every honest vendor on sensor noise.
* **Bench results lapse after a year**, carried in between by field conformance.
* **`not_applicable` passes.** The balance guard on a wheeled base is genuinely inapplicable, and
  forcing a fail there would push vendors to fake a result rather than declare a shape.

---

## 16. LeRobot

Hugging Face's LeRobot is the closest thing the field has to a shared robotics stack: a `Robot`
class abstracting `connect / get_observation / send_action`, the `LeRobotDataset` format (MP4 for
vision, Parquet for state and action), a policy zoo (ACT, Diffusion, π0, GR00T, SmolVLA), a
teleoperator abstraction, and a CLI. Native support spans SO-100/101, Koch, LeKiwi, HopeJR,
Reachy 2, Unitree G1 and others, with community packages for xArm, UR5e, Franka and AgileX.

It is **complementary, not competitive**, and it is worth being precise about where the line falls.
LeRobot is a *framework you write a program with*. Skynet OS is *the thing that runs programs*. It
has no safety kernel, no package format, no permission model, no OTA, no fleet reconciliation, no
conformance suite, no cross-embodiment transport as a service — because those are operating-system
concerns and LeRobot is not trying to be an operating system.

Three concrete integrations, in the order they pay off:

1. **A LeRobot RHAL driver.** `Robot.connect/get_observation/send_action` is very nearly the
   simplest possible RHAL. Wrapping it means **every robot LeRobot supports becomes a Guest-tier
   Skynet robot on day one**, for the cost of one driver. This is the single fastest path from
   zero to a real device fleet, and it should be built first.
2. **`LeRobotDataset` as a first-class episode format.** `lib/skynet/loading.ts` already lists
   `lerobot` as both a loader kind and a dataset reader, so the Hub anticipated this. The device
   episode ingest path (`POST /episodes`) should emit LeRobotDataset natively, which makes every
   hour of Skynet teleoperation immediately trainable with existing tooling.
3. **LeRobot policies as the seed catalogue**, imported with their real provenance and run through
   the transport planner — which is where Skynet adds something LeRobot cannot: an ACT policy
   trained on an SO-101 gets a *typed, per-channel* answer about whether it runs on a G1, instead
   of a fork of the repository.

The honest framing for the ecosystem: Skynet OS should be the best place to *run* a LeRobot
policy, and should say so.

---

## 17. The Hub contract

Almost everything needed already exists. What the Hub has today that the OS consumes directly:
content-addressed artifacts, version digests, verified Ed25519 signatures, scan verdicts, the
load plan (`GET /versions/{v}/load`), the compatibility engine, `skynet_robots.limits`, device
tokens, episode ingest, deployments pinned by digest that only a runtime check-in can promote,
namespace secrets, and the audit log.

New surface, and it is deliberately small:

```
GET  /repos/{ns}/{name}/versions/{v}/transport?robot=   transport plan (same pure fn as the robot)
GET  /robots/{slug}/embodiment                          the signed embodiment descriptor
POST /robots/{slug}/embodiment                          vendor publishes one
GET  /devices/{id}/desired                              desired state; the robot reconciles to it
POST /devices/{id}/checkin                              generation, health, attestation
POST /devices/{id}/incidents                            sealed black box upload
POST /devices/{id}/conformance                          signed SCTS results
GET  /os/releases?channel=&tier=&arch=                  OTA bundle manifest
GET  /os/releases/{version}/bundle?arch=                the image itself (added by §75)
GET  /certifications/{robot_slug}                       award, evidence, expiry, field conformance
```

New tables follow the existing `skynet_*` convention: `skynet_embodiments`,
`skynet_device_state` (desired + reported + generation), `skynet_conformance_results`,
`skynet_certifications`, `skynet_incidents`, `skynet_os_releases`. RLS on every one, and
`scripts/skynet-verify-rls.mjs` extended to probe them by observation as it does the rest.

Two rules carried over unchanged from the Hub, because they matter more here, not less: **the
platform never claims a robot is running something it has not heard from** (a deployment is
`pending` until a check-in), and **a stopped deployment stays stopped** (a robot reporting in does
not undo an operator's stop).

---

## 18. Developer experience

```bash
skynet init my-policy --robot unitree-g1        # scaffold with a real embodiment descriptor
skynet transport ./ --robot spot                # what would happen on a body you do not own
skynet run ./ --sim mujoco                      # simulator RHAL driver; same OS, no hardware
skynet pair ABCD-1234                           # bind this robot to a namespace
skynet install openrobot/grasp-v2@1.4.0         # admission findings printed in full
skynet grants add manipulation.grasp            # operator grant, audit-logged
skynet ps · logs · estop · blackbox seal
```

Two properties are worth designing for explicitly:

* **`skynet transport` needs no hardware.** Because the planner is pure and descriptors are
  published, a developer can ask "would this run on a robot I do not own?" and get a per-channel
  answer. This is the loop that makes cross-body publishing normal instead of heroic.
* **Simulator-first is not a lesser path.** Isaac Sim, MuJoCo and Gazebo are RHAL drivers like any
  other, so the *same* OS, packages, kernel and grants run in simulation. The sim conformance
  suite depends on this being true, and it removes the usual gap where sim code and robot code
  diverge.

---

## 19. Security posture

Inherits the Hub's posture (§7 of `docs/skynet-architecture.md`) and adds what being on the robot
requires:

* **Untrusted policies are the design assumption, not an edge case.** Sandboxed, no actuator
  descriptor, declared budget enforced, capabilities gated per cycle.
* **Secure boot to the kernel.** Bootloader verifies the OS image; the OS verifies the Safety
  Kernel and RHAL drivers before loading. A driver runs native and must therefore be signed by a
  key the operator has trusted.
* **A device credential is assumed lost.** It cannot read repositories, claim jobs, or write to
  another namespace, and `security.credential.scope` tests exactly that.
* **"A key the operator has trusted" is a file, and it has no default.** The phrase appears in §37.1
  and in the bullet above, and it needs a referent or it is decoration. That referent is
  `/data/state/trust.json`: a list of Ed25519 public keys with the operator's own label for each.

  Four rules, and each of them is the same rule §39.2 applies to credentials, applied to keys:

  | | |
  |---|---|
  | No file | This robot trusts **nobody**. Every signature reports `untrusted`, which is true and names the file to write |
  | A file that does not parse | Also nobody, and it says *that* rather than "no keys" — one is a robot nobody provisioned and the other is a file somebody broke |
  | One malformed entry | Skipped and reported; the rest still work. A key that fails to parse can only ever *remove* trust, so skipping it is safe in the only direction that matters, while refusing the file would turn one typo into a robot that installs nothing |
  | A small-order key | **Refused outright.** Those eight points verify signatures nobody produced, and an operator must not be able to trust one by pasting it in |

  The **label** is why this is not a list of hex strings. A report that says *"signed by
  `d75a980182b10ab7…`"* has told an operator nothing they can act on.

  Keys are read on the pass that needs them rather than held at start-up, so a key an operator adds
  takes effect on the next reconcile rather than at the next reboot.

  **The store is an audited surface, not a file people edit.** `skynetd trust list`, `add` and
  `remove` are the way in, each change carries the provenance §38.3 asks for, and each is written to
  the audit log beside the store. Editing the JSON by hand still works and still produces a robot
  that trusts the key — the entry simply reports its provenance as `unrecorded`, which is true.

  Two refusals belong to the command rather than to the file. **The same key twice** is refused
  naming the label already holding it, because silently relabelling rewrites a decision somebody
  else recorded and hides the case worth seeing — one key arriving under two names. **The same label
  twice** is refused naming the key, because two keys under one name make every report ambiguous
  about which one signed, which is the one thing a label exists to prevent.

  And `add` refuses outright on a store that does not parse. Writing over it would destroy the keys
  it still holds, and the robot cannot tell which those are; the remedy there is a person.
* **The verifier is strict, and the strictness is a compatibility property.** Non-canonical `y` in a
  key or a commitment, a scalar at or above `L`, and a small-order key are all refused, and the
  group equation is checked cofactorless. Every one of those is a case where two implementations
  otherwise disagree about *the same bytes* — and a disagreement about whether a package is signed
  is worse than either answer, because it means two robots in a fleet running different software.
* **The blast radius of a compromised policy is bounded by the kernel**, not by the policy's good
  behaviour. This is the property that makes the whole marketplace model defensible.
* **Every grant, stop, install, OTA and certification change is audit-logged** to the Hub's
  existing `skynet_audit_log`.

---

## 20. Distribution

The Android structure, because it is the one that worked:

* **Skynet OS Core** — open source, Apache 2.0. The kernel, RHAL, transport, package format, the
  reference drivers, the CLI. Anyone may build it, fork it and ship it. A vendor interface nobody
  can inspect is a vendor interface nobody adopts.
* **Skynet Certified** — the trademark and the badge, gated on SCTS. Free for individuals and
  research; commercial for fleets. This is CTS, and it is what makes the compatibility claim mean
  something.
* **Skynet Services** — the GMS analogue, per robot per month: Hub sync, hosted OTA, the fleet
  console, black box retention, marketplace install and paid-package entitlement.

The flywheel: more certified robots → more policies target them → more robots want certification.
For a vendor the pitch is one sentence — *implement one driver, and the entire catalogue works on
your robot on day one* — which is the same pitch that got silicon vendors to implement a HAL.

---

## 21. Hub and OS, side by side

The original framing, answered:

| | **Skynet Hub** | **Skynet OS** |
|---|---|---|
| Where it runs | Cloud | On the robot |
| Models | Store, version, sign | Load and execute under a resolved transport plan |
| Datasets | Store and version | Streams episodes out; stores no corpus |
| Skills | Publish and discover | Install and execute as `.sky` packages |
| Robots | Registry bound to `botindex` | Drives the machine through RHAL |
| Compatibility | *Whether* X works with Y | **The transport plan that makes X work with Y** |
| Benchmarks | Leaderboards | Runs evaluations on the robot and in the sim driver |
| Deployment | Sends intent | Pulls desired state and reconciles to it |
| Versioning | Central history | Manages installed versions and A/B slots |
| Safety | Policies, provenance, envelopes | **Real-time enforcement; sole actuator writer** |
| OTA | Manages releases and rings | Installs, health-gates, rolls back |
| Telemetry | Stores and analyses | Black box every cycle; seals on fault |
| Marketplace | Buy and sell | Install and run purchased software under grants |
| Community | Stars, forks, discussions | None — deliberately |
| Hardware | Metadata | **Drivers, HAL, device management** |

---

## 22. Build order

**Buildable now, no robot required** — all pure, all testable, all shared with the Hub:

* the Embodiment Descriptor, canonical roles, and `resolveTransport()` — *done*
* the Safety Kernel reference implementation and limit resolution — *done*
* the `.sky` manifest and `admitPackage()` — *done*
* SCTS definitions and `awardCertification()` — *done*
* the transport endpoint on the Hub, the embodiment tables, the descriptor publish path
* the simulator RHAL driver (MuJoCo), which makes the sim suite runnable end to end
* the **LeRobot RHAL driver**, which is the fastest path to a real device (§16)

**Needs hardware, and will report honestly that it is waiting for it** — exactly as the Hub's
training and simulation surfaces already do rather than displaying invented results:

* the C Safety Kernel and its parity corpus
* RHAL drivers for EtherCAT and CANopen
* the bench conformance suite
* OTA on a real A/B device
* the safety MCU and the certified tier

**The order matters.** Transport and the kernel are the parts that must be right before anything
touches hardware, and they are the parts that can be written and tested without any. The
temptation is to start with a driver because it produces visible motion; the result of starting
there is a system whose safety argument is retrofitted, which is the failure mode this entire
document is arranged to avoid.

---
---

# Part II — Implementation stack

Part I designs the system. This part commits to what it is built out of, and then closes the
gaps Part I left open. Section 23 exists because a reader asked the obvious question — *what
language is this actually written in?* — and Part I never answered it, which was a real omission
rather than a deferral.

---

## 23. Languages and toolchain

> **Superseded in Part III.** This section argues for a single implementation. That position was
> overruled: Skynet OS ships as multiple language editions (§31), each with a tier ceiling set by
> whether its runtime can promise a deadline, and all of them held to one conformance corpus.
> Rust remains the reference edition, and everything below still describes it.

One rule generates most of this table: **Skynet writes Rust; C is a boundary, not a language.**
Every component Skynet owns is Rust. C appears exactly once, as the ABI at the vendor boundary,
because a stable ABI is a compatibility requirement and C is the only one every language can both
produce and consume. Python appears exactly once, where policies actually live.

| Component | Language | Why this one |
|---|---|---|
| **Safety Kernel** | **Rust**, `#![no_std]`, no `alloc` | §23.1 — the decision the rest hangs on |
| **RHAL — the interface** | **C ABI** (not a language choice) | Rust's ABI is unstable and C++'s is compiler-dependent. A vendor writing C, C++ or Rust must link against the same symbols in ten years. |
| **RHAL — drivers we ship** | Rust | Same safety argument as the kernel; they run in the RT path. |
| **RHAL — vendor drivers** | Their choice | We ship a C header, a C++ wrapper and a Rust crate. The vendor's existing motor code is C or C++ and rewriting it is not a precondition for adoption. |
| **`skynetd`** | Rust | Long-running, must not crash, parses untrusted network input and verifies signatures. No GC, real error handling, and it shares the kernel's crates. |
| **Policy Runtime** | Rust host + two backends | §23.3 |
| **Shared contracts** (transport, admission, conformance) | **Rust core → WASM for the Hub** | §23.2 — this is a correction to Part I |
| **On-robot CLI / agent** | Rust, single static binary | It must work on a robot with no Node, no Python and a broken network. |
| **Python SDK** | Python | Already exists at `sdk/python/skynet`. It is how researchers work and that is not negotiable. |
| **Hub** | TypeScript / Next.js | Unchanged. The Hub is a web application and rewriting it would be vandalism. |
| **Existing Node CLI** | TypeScript | Stays for workstations; converges onto the Rust CLI's command surface rather than being maintained twice forever. |

### 23.1 Why the Safety Kernel is Rust and not C

Four requirements, and only one language satisfies all of them:

1. **One source, two targets.** The same kernel must run as a Linux RT thread (host tier) and on a
   bare-metal Cortex-M or R-series safety MCU (certified tier). `#![no_std]` Rust compiles for
   both from one codebase. Two implementations of a safety kernel is two safety kernels, and the
   one that gets less attention is the one on the MCU.
2. **No heap, provably.** Omitting `alloc` entirely makes "this never allocates" a compile-time
   property rather than a code-review promise.
3. **Memory safety in the one place it is worth the most.** A buffer overrun in a web server
   returns a 500. A buffer overrun between a policy's output buffer and a torque command is a
   moving humanoid.
4. **A qualified toolchain must exist.** This used to be the decisive objection to Rust and no
   longer is: **Ferrocene** is TÜV SÜD-qualified for ISO 26262 **ASIL D**, IEC 61508 **SIL 3** and
   IEC 62304 Class C, with a certified subset of `core` at SIL 2 / ASIL B.

Two honest qualifications on that last point, because this is a safety claim and precision is the
whole product:

* Ferrocene is qualified to **SIL 3, not SIL 4**. Ferrous Systems supports customer qualification
  efforts toward SIL 4; the toolchain itself has not achieved it. SIL 3 is above what a robot
  safety function of this shape typically requires — ISO 10218:2025 lands around PL d / SIL 2 with
  hardware fault tolerance of 1 — so this is sufficient, and overstating it would be exactly the
  kind of lie `SAFETY_CLAIM_RULE` exists to prevent.
* A qualified compiler is one input to a safety case. It is not a safety case.

**The case for C**, stated fairly because it is not weak: MISRA C is the well-trodden path, every
assessor has reviewed it, and every MCU vendor ships a C toolchain and a BSP. Choosing Rust means
occasionally fighting a vendor's C-only ecosystem. The trade is accepted because the kernel is
roughly two thousand lines that must be correct forever, which is precisely the shape of program
where memory safety is worth an integration cost. **Ada/SPARK** would prove more and was rejected
on talent pool alone.

The C ABI is still exported (`skynet_sk_*` symbols), so a vendor may call the kernel from C
without knowing Rust is underneath.

### 23.2 A correction: the shared contracts belong in Rust

Part I says the transport planner, package admission and certification logic are pure functions
that run identically on the Hub and on the robot, and that this is what makes the two unable to
disagree. Part I then implemented them in TypeScript.

**A robot cannot run TypeScript.** As shipped, the design implies a second implementation on the
robot — and two implementations of "will this policy destroy this arm" can drift, which is the
exact failure the shared-function argument was introduced to prevent.

The resolution:

* The contracts move to a Rust crate, `skynet-contracts`, as the single implementation.
* The Hub consumes it as **WASM** from a Next.js route handler. One implementation, compiled
  twice, cannot disagree with itself — which is a stronger guarantee than the one Part I claimed.
* **The existing TypeScript is kept, and its role changes.** It becomes the reference and the
  differential-test oracle: the same corpus of cases runs against both, and they must agree. This
  is the role `safety.ts` was already designed to play against the C kernel, applied consistently.

Keeping the TypeScript is not sentiment. A specification written in a language the domain experts
read, tested against the implementation, is worth more than a comment block — and it is what makes
`safety.kernel.parity` meaningful.

> **Amended by §31.** With language editions, "one implementation compiled twice" is no longer
> available as a guarantee. What replaces it is stronger in one way and weaker in another: the
> contracts are implemented per edition and held to a byte-identical conformance corpus, so
> divergence becomes *detected* rather than *impossible*. The WASM build remains how the Hub
> consumes the reference edition.

### 23.3 The Policy Runtime is two backends behind one supervisor

A Rust supervisor process owns the loop, the observation assembly and the ring buffer. What it
hosts depends on the loader the Hub's load plan already identified from the *bytes* — the split
falls out of `LOADER_KINDS` in `lib/skynet/loading.ts` rather than being invented here:

| Load plan says | Backend | Notes |
|---|---|---|
| `safetensors`, `onnx`, `torchscript` | **In-process**, ONNX Runtime or LibTorch via Rust bindings | The production path. No Python, no GIL, no interpreter startup, bounded memory. |
| `pickle`, `custom`, `lerobot` | **Sandboxed CPython subprocess** over shared memory | The compatibility path — and, not coincidentally, exactly the loaders the scanner flags `executes_on_load`. It is sandboxed hardest because it is the one that runs arbitrary code. |

This is why the load plan is computed server-side from file headers rather than extensions: the
runtime picks a backend from what the bytes actually are. `.pt` written by `torch.save` is a
pickle and takes the subprocess path; `.pt` written by `torch.jit.save` is TorchScript and runs
in-process. The extension cannot tell them apart and the header can.

**Inference is never in the real-time path.** The kernel runs at the joint rate whatever the
policy is doing; a slow forward pass produces a stale command, the watchdog catches it, and the
robot stops in a controlled way. A design where a 40 ms inference spike delays a torque command
is a design that falls over.

### 23.4 The rest of the stack

| Layer | Choice | Why |
|---|---|---|
| Kernel | **Linux, PREEMPT_RT**, mainline since 6.12 | §23.5 |
| Image build | **Yocto** + `meta-rauc`, `meta-tegra`/vendor BSP layers | Reproducible, minimal, and every SoC vendor already ships a Yocto BSP. Debian is easier and gives up reproducibility; Buildroot is simpler and has a thinner BSP ecosystem. |
| Guest tier | An OCI container on whatever is already there | Guest explicitly runs beside somebody else's stack; requiring an image would defeat the tier. |
| Init | systemd, with `skynetd` supervising above it | Reinventing process supervision is a year spent reaching parity with what is already installed. |
| Policy sandbox | OCI runtime (`crun`/`youki`) + seccomp, no `CAP_*`, no device nodes | The sandbox's defining property is negative: there is no path to an actuator file descriptor. |
| Hot-path IPC | **Lock-free SPSC ring in shared memory** (`memfd` + `mmap`), fixed-size frames | §25 |
| Control-plane IPC | Unix socket, JSON, same shapes as the Hub REST API | One command surface for the CLI whether it is talking to a robot or the cloud. |
| OTA | **RAUC**, A/B slots, Ed25519 bundle signatures | Reuses the Hub's existing signing machinery. Widest flash-backend support of the open engines. |
| Black box | **MCAP** | §26 |
| Simulation | Isaac Sim, MuJoCo and Gazebo as **RHAL drivers** | The same OS, kernel, packages and grants run in sim. This is what makes the sim conformance suite meaningful instead of a parallel universe. |
| Safety MCU (certified) | Bare-metal Rust, or Zephyr where a scheduler is needed | Not Linux. §23.5 |

### 23.5 Is Linux the right answer?

**On the application processor, yes, and it is not close.** Nothing else has the GPU drivers, the
ML runtimes, the camera stacks, the networking and the package ecosystem, and PREEMPT_RT has been
mainline since 6.12 — real-time is a kernel config now, not a patch stack to rebase forever.
Typical latencies of 1–50 µs with worst case in the low hundreds are comfortably inside what a
100 Hz–1 kHz whole-body loop needs.

**On the safety MCU, no.** The certified tier's whole argument is a stop path that survives the
application processor hanging, and running that path on the same kernel would defeat it. Bare-metal
Rust, or Zephyr where a scheduler is genuinely needed.

**Below the RHAL, also usually no.** Joint-level current control at 10–20 kHz stays on the vendor's
motor controller, where it already is. Skynet does not want that loop and should not pretend it
could take it.

Tuning that is part of the spec rather than an operator's problem: `isolcpus` + `nohz_full` for the
RT core, `SCHED_FIFO` for the kernel thread, `mlockall(MCL_CURRENT|MCL_FUTURE)`, no swap, and the
inference process pinned away from the RT core. `rhal.state.rate` in the bench suite measures the
worst-case gap, which is what catches a driver that takes a lock held by a non-RT thread.

---

## 24. The RHAL data structures

Part I sketched the vtable and left the structs undefined, which left the most latency-sensitive
part of the system unspecified. Three properties matter: **no allocation**, **timestamps at
acquisition**, and **an explicit hold mask**.

```c
#define RHAL_ABI_VERSION 1

/* flags */
#define RHAL_ESTOP     (1u << 0)   /* an emergency stop is asserted */
#define RHAL_FAULT     (1u << 1)   /* the drive has faulted; commands are being ignored */
#define RHAL_DEGRADED  (1u << 2)   /* running, but something is wrong — thermal, a dead encoder */

typedef struct {
  uint64_t stamp_ns;        /* CLOCK_MONOTONIC_RAW at ACQUISITION, never at return */
  uint32_t seq;
  uint16_t joint_count;
  uint16_t flags;
  float   *position_rad;    /* [joint_count] — caller-allocated, driver fills */
  float   *velocity_rad_s;  /* NaN where the joint does not measure it */
  float   *effort_nm;
  float    base_pose[7];    /* x y z qx qy qz qw; NaN when the base is not localised */
  float    base_twist[6];
  uint32_t fault_code;      /* vendor-specific, surfaced verbatim to the black box */
} rhal_state;

typedef struct {
  uint64_t stamp_ns;        /* when the command was produced, not when it was sent */
  uint32_t seq;             /* MUST advance; the watchdog treats a repeat as a stalled producer */
  uint8_t  mode;            /* RHAL_MODE_POSITION | _VELOCITY | _TORQUE */
  uint8_t  _pad[3];
  uint16_t joint_count;
  const float   *values;    /* [joint_count]; units follow `mode` */
  const uint8_t *mask;      /* [joint_count]; 0 = hold this joint at its current setpoint */
} rhal_command;
```

Every buffer is caller-allocated and reused across cycles, so the hot path never touches an
allocator. `stamp_ns` is taken at acquisition because a timestamp applied on return folds the
driver's own latency into the measurement and quietly corrupts every downstream latency budget.

**The `mask` is where the transport plan meets the wire.** A `drop` binding — a channel the policy
emits that this body has no equivalent for — becomes a zero in the mask. Without it, a dropped
channel would have to be encoded as some value, and every value is a command. The mask is how the
OS says *nothing* rather than saying *zero*, which on a torque-controlled joint is the difference
between holding position and going limp.

Errors are negative `errno`-style. `write_command` returning `-EPERM` because the drive is faulted
is a different fact from returning `-ETIMEDOUT`, and the black box records which.

---

## 25. The command path, and what crosses it

```
Policy backend                       Safety Kernel                     RHAL
──────────────                       ─────────────                     ────
observation frame  ──assembler──▶
                   ◀── inference ──
desired[]  ──▶ ┌──────────────────┐
               │ SPSC ring, shm   │ ──▶ 8 guards ──▶ rhal_command ──▶ write_command()
               │ fixed 4 KiB slots│      (§7.1)
               └──────────────────┘
```

Single-producer / single-consumer, lock-free, fixed-size slots in a `memfd`-backed mapping. No
serialisation, no sockets, no DDS, no allocation in the hot path.

Two properties are load-bearing:

* **The ring is the only channel from the policy to the kernel.** Not a convention — a
  consequence of the sandbox, which holds no actuator descriptor and no other writable mapping.
  The policy cannot reach the kernel except through the ring, and cannot reach the RHAL at all.
* **The kernel never blocks on the producer.** If no fresh frame is ready, it does not wait. It
  runs its cycle on what it has, the watchdog observes a non-advancing sequence, and the robot
  stops in a controlled way. A kernel that waits for a policy is a kernel whose real-time
  guarantee is the policy's tail latency.

### 25.1 Observation assembly and time

This was missing from Part I, and it is one of the harder parts of the whole system.

A policy is trained on synchronised frames: image and joint state as of one instant. A real robot
has three cameras at 30 Hz with 20–60 ms of pipeline latency, joint state at 1 kHz with 1 ms, and
an IMU at 200 Hz — none of them aligned, none of them stamped the same way. Feed a policy the most
recent value from each and it receives a frame that never existed, in which the arm is where it is
*now* and the image shows where it was two frames ago. Policies degrade in ways that look like bad
training.

The design:

* **One time base**: `CLOCK_MONOTONIC_RAW`, with PTP or gPTP where the hardware supports it. Every
  sample carries an acquisition stamp, never an arrival stamp.
* **Declared per-sensor latency** in the embodiment descriptor, so the assembler knows how far back
  a camera's "now" actually is. This is why `command_latency_ms` is a descriptor field and why the
  bench suite measures it rather than trusting it.
* **The assembler targets an instant**, not a set of latest values: it picks the sample nearest the
  target for each stream and reports the worst skew in the frame.
* **A staleness budget** per stream, from the policy's declared observation requirements. Exceeding
  it is a `hold`, not a silently older frame.
* The frame's **skew and per-stream ages go into the black box**, so "the policy behaved oddly" can
  be checked against "the frame it was given was 80 ms skewed" months later.

This does not make an unsynchronised robot into a synchronised one. It makes the skew *measured
and visible* instead of silently folded into the policy's behaviour.

### 25.2 Two packages, one arm

Also missing from Part I. Capability grants say a package *may* use the arm; they do not say what
happens when two packages hold that grant simultaneously.

**Capabilities are leased exclusively.** A running package holds a lease on each capability token
it commands, `skynetd` is the arbiter, and a second package requesting a held token is refused
rather than interleaved. Two policies alternating commands to the same joint produce a trajectory
neither was trained on and neither can be held responsible for.

Leases carry a priority, and exactly one pre-emption is allowed: a package registered as a **safety
behaviour** (a fall-arrest reflex, a retreat) may pre-empt a lease. It is a distinct package kind,
it is granted separately, and every pre-emption is a black box event. Sensing capabilities are
shared freely — two policies reading the same camera is normal; two policies driving the same
elbow is not.

---

## 26. Black box format

**MCAP**, which is already the ROS 2 default bag format since Iron and is what the robotics
tooling ecosystem reads. Choosing it means Foxglove opens an incident recording with no exporter,
and it means `lib/skynet/loading.ts` — which already lists `mcap` among its dataset readers — can
ingest a sealed recording as training data without a conversion step. Inventing a format here
would have bought nothing and cost the entire existing toolchain.

Channels, each with an embedded schema so the file stays readable without the code that wrote it:

| Channel | Rate | Carries |
|---|---|---|
| `/sk/state` | joint rate | `rhal_state` verbatim |
| `/sk/desired` | policy rate | what the policy asked for |
| `/sk/verdict` | joint rate | guards tripped, clamps applied, response |
| `/sk/command` | joint rate | what actually went to `write_command`, and its return |
| `/sk/frame` | policy rate | observation hashes, per-stream ages, worst skew |
| `/sk/event` | on change | lease grants, pre-emptions, estop, OTA, grant changes, configuration changes, admission verdicts |

Observation *hashes* rather than observation bytes: an hour of three-camera video is hundreds of
gigabytes and the robot has neither the disk nor the uplink. The hash proves which frame was seen;
full-fidelity capture is a separate, explicitly enabled recording mode for data collection.

### 26.1 The ring is not itself an MCAP file

Written in during implementation, because it was tried the other way first and does not work. MCAP
is a linear container — a magic, a header, records, a data-end, a summary computed over everything
before it, a footer — and a ring overwrites its oldest bytes, so the file's own framing is the first
casualty of the second hour. A black box whose format breaks precisely when it wraps works only
during the incidents short enough not to need it.

So the two roles are separated:

* **The ring is the recording.** Fixed-size records, one slot each, a sequence number that only ever
  increases, and no structure a wrap can invalidate. A record is [§13's line] laid out to a byte
  offset. The size is a function of the body's joint and channel counts, so a ring is created for a
  particular shape of robot and re-created — after sealing what it holds — when that shape changes.
* **MCAP is the artifact.** Produced once, at seal, from whatever the ring holds at that instant.
  §26 chose MCAP so that an incident opens in Foxglove with no exporter, and that is a property of
  the file handed to an investigator rather than of the bytes being written a thousand times a
  second.

Three consequences worth naming:

**One cycle becomes four messages.** The table above splits a control cycle across `/sk/state`,
`/sk/desired`, `/sk/verdict` and `/sk/command`; the ring holds it as one record. That is not a
redundancy to collapse. The ring is optimised for writing at joint rate and the file for somebody
asking a question, and "what did the hardware report" and "what did the policy want" are different
questions.

**Every message carries `mono_ns` as well as MCAP's `log_time`.** `log_time` is nanoseconds since
the epoch because that is what a general reader can render; every control decision was made against
`CLOCK_MONOTONIC_RAW` (§35.1). Carrying both means a wall-clock step is *visible* in the file — the
two disagree across it — rather than silently displacing an hour of the timeline.

**Message bodies are canonical JSON, with a JSON Schema on each channel.** §31.2 requires a black
box sealed by one edition to be readable by every other, and canonical JSON is already the one
serialisation every edition implements identically and the corpus already tests (§31.4). A CDR body
would need a second format specified and held identical across editions, and would need a generator
and an IDL to be anything but bytes. The compactness argument for CDR applies to a bag written at
line rate; this file is written once, after an incident, from a bounded ring.

**Gaps appear on `/sk/event`.** §47.4 requires a dropped frame to be recorded as a drop. A gap
consumes a sequence number of its own, so the numbering stays dense and a reader can distinguish
"the ring wrapped past these" — normal; the window moved on — from "these were never written", which
is not.

---

## 27. Boot, and what happens when things break

> **Extended in §48.** A power-on self-test and an `UNHOMED` state precede this sequence: a robot
> with incremental encoders does not know its joint positions at power-on, so every pose-dependent
> guard is unavailable until it is homed, and a failed self-test goes to `SAFE` rather than
> `DEGRADED`.

```
bootloader (verifies slot signature)
  → kernel + initramfs
    → RHAL driver load, ABI check, describe() → embodiment descriptor
      → Safety Kernel start, limits resolved, guards partitioned
        → skynetd: verify installed packages, reconcile against last desired state
          → policy runtime: transport plan resolved, leases granted
            → RUNNING
```

The kernel starts **before** any policy and outlives all of them. A robot with no policy running is
a robot that holds position safely; a robot with a policy and no kernel must not exist, so the
runtime refuses to start if the kernel is not live.

| State | Entered when | Behaviour |
|---|---|---|
| `HOLD` | No policy, or a guard `hold` | Setpoints held, brakes as configured. The safe resting state. |
| `RUNNING` | Lease granted, plan resolved, kernel live | Normal operation. |
| `DEGRADED` | A guard is unavailable, or the RHAL reports `RHAL_DEGRADED` | Runs, reports, and refuses any package whose grants depend on the failed guard. |
| `STOPPING` | Watchdog, proximity, balance, or an operator stop | Controlled ramp per the stop category. |
| `SAFE` | Stop complete, or the kernel lost the RHAL | Drives disabled. **Requires an explicit human action to leave.** |

`SAFE` is deliberately sticky. A robot that recovers itself from a safety stop is a robot that
resumes moving while somebody is standing next to it working out what happened.

---

## 28. Storage and minimum hardware

```
/boot        A/B, verified by the bootloader
/rootfs.a    /rootfs.b     read-only squashfs, RAUC slots
/data        persistent, survives updates
  /packages    content-addressed .sky blobs, shared by digest across policies
  /blackbox    ring buffer, preallocated at install so a full disk cannot silence it
  /state       desired-state cache, grants, leases, operator profiles, the embodiment descriptor,
               the credentials (§39.2) and the trusted keys (§19)
  /identity    device key (TPM/secure-element-backed where present)
```

Three files in `/state` are pinned by name, for the reason §37.1 pins the package layout — two
editions that disagree about where something is written are two editions where a robot provisioned
by one looks unprovisioned to the other:

| | |
|---|---|
| `state/embodiment.json` | the descriptor (§5) |
| `state/embodiment.sig.json` | the vendor's detached signature over its digest. **Detached**, because the digest is over the descriptor's canonical form and a signature written inside it would change the digest it signs |
| `state/trust.json` | the keys this robot trusts (§19) |

`/blackbox` is preallocated on purpose: a black box that stops recording because a log filled the
disk is a black box that fails exactly when something is going wrong.

**The embodiment descriptor is `/data/state/embodiment.json`, and the path is pinned** for the
reason §37.1 pins the package layout. Two editions that disagree about where the body is written are
two editions where a robot provisioned by one is a robot the other believes has no body — and a
robot with no body records a zero embodiment digest, cannot answer the capability half of an
admission decision, and reports both pose guards unavailable. That is a large amount of wrong
behaviour to inherit from a filename, and none of it announces itself.

One file, not a directory. A robot is one machine, and a directory here would invite a second
descriptor whose relationship to the first nothing defines — *which* of two bodies the kernel is
guarding is not a question this system should ever have to answer at run time.

It sits beside the profiles rather than under them because it is not one. A profile is what an
operator may narrow (§38.2); the descriptor is what the vendor says the machine *is*, and the
layering rule has nothing to say about a joint count.

**It arrives as a file, and on the supervisor's side it has to.** §4 gives the driver a `describe()`
returning exactly this document, and §33.1 forbids `skynetd` from loading a driver to call it —
doing so would hand the supervisor a vtable of function pointers into vendor code, which is the
whole of what that rule prevents. So the descriptor is written at provisioning (§50), by `skynet
descriptor from-urdf`, or by vendor tooling, and the daemon reads it the way it reads everything
else about the world.

**Absent and malformed are different states**, and every consumer is required to keep them apart.
"Nobody has told this robot what it is" is a provisioning step that did not run; "somebody did, and
the answer does not parse" is a generator emitting something wrong. They imply different work, and a
surface that renders both as an empty panel hides the only fact that decides which.

Targets, not measurements — these are what the design is aimed at, and the bench suite is what will
say whether they hold:

| Tier | CPU | RAM | Accelerator | Notes |
|---|---|---|---|---|
| Guest | any x86-64 / arm64 | 2 GB | none needed | Runs beside the vendor stack; may sit off-robot entirely. |
| Host | 4 cores, one isolated for RT | 8 GB | optional | Enough for ONNX policies up to roughly 1 B parameters. |
| Host + VLA | 8 cores | 16 GB | 16 GB unified or discrete | A 7 B VLA at int8 needs ~8 GB of weights plus activations and camera buffers. Jetson Orin NX 16 GB is the reference point. |
| Certified | as above | as above | as above | Plus a Cortex-M/R safety MCU on the drive-enable line. |

---

## 29. Two things Part I referenced and never defined

**The collision model.** `self_collision` and `workspace` need geometry that the descriptor did not
carry. It is an optional block: convex hulls or capsules per link, a URDF or USD reference for the
full mesh, and an allowed-pairs list (adjacent links touch by design and flagging them would make
the guard unusable). Absent, the guard reports unavailable — it does not silently pass.

**Teleoperation.** The episode schema already knows about `teleop_vr`, `teleop_mocap`,
`teleop_gamepad` and `teleop_keyboard`, so the path exists on the Hub side. On the OS: a
teleoperator is a **package kind**, holding the same capability leases and passing through the same
eight guards as any policy. A human operator is not exempt from the envelope — an operator can
command an arm through a wall as easily as a policy can, and the clamp is the same clamp. Intervention
counts, which the episode schema already records, come from lease pre-emptions in the black box.

---

## 30. What is still open

The architecture is complete: every subsystem has a shape, a contract and a reason. The
engineering specification is not, and these are the parts that are genuinely undecided rather than
merely unwritten.

| Open question | Why it is not settled |
|---|---|
| **Retargeting fidelity** | The transport plan says a chain *can* go through task space. It does not say the resulting behaviour is any good. Whether a manipulation policy survives 7-DoF → 6-DoF retargeting is an empirical question per policy, and the honest answer today is a `retargeted` verdict plus a benchmark result, not a promise. Cross-embodiment VLA work is moving fast here and the planner should be able to consume its output rather than guess ahead of it. |
| **Role vocabulary governance** | Twelve chains and eight axes cover humanoids, arms, quadrupeds and wheeled bases. They do not cover soft robots, continuum arms or anything with non-rigid kinematics, which fall to `aux` and exact-name matching. Who extends the vocabulary, and how a body plan is added without breaking existing descriptors, needs a process — probably versioned vocabularies pinned by descriptor, but that is unproven. |
| **Kernel formal verification** | `#![no_std]` Rust plus a qualified toolchain is strong. Proving the guard set never emits a command outside the envelope — as opposed to testing that it does not — is a Kani or Creusot project that has not been scoped. |
| **Field conformance privacy** | Continuous telemetry from a customer's robots to a vendor's certification badge is a data-governance question with a real conflict in it. Aggregating on-robot and reporting only conformance ratios is the likely answer; it has not been designed. |
| **Multi-robot coordination** | Deliberately out of scope. Skynet OS runs one robot. Fleet coordination — shared maps, traffic, task allocation — is a different system and pretending otherwise would produce a bad version of both. |
| **The certified tier is unbuilt** | Everything above the MCU is designed and testable. The MCU itself, its interlock wiring and the safety case around them need hardware, a partner and a notified body. No amount of further design substitutes for that, and the tier should keep saying so until it exists. |

---
---

# Part III — Language editions and the interface

Part II committed to Rust and argued that reimplementation should be forbidden above the vendor
boundary. That argument was heard and overruled: **Skynet OS ships as multiple language editions,
so an adopter can choose the one their team already writes.**

This part designs that properly. The concern behind the original position was real — N
implementations of a safety kernel is N sets of latent bugs — so the answer is not to drop the
concern but to adopt the mechanism that other multi-implementation systems use to survive it.
Protocol Buffers has implementations in twenty languages, WebAssembly has a dozen runtimes, SQL
has hundreds of engines. None of them stays coherent by good intentions. They stay coherent
because **the specification and its conformance corpus are the artifact, and every implementation
is measured against it rather than against the others.**

---

## 31. Language editions

### 31.1 The shape of the decision

The normative artifact stops being a codebase and becomes a pair:

* **The specification** — Parts I and II of this document, plus the machine-readable schemas.
* **The conformance corpus** — golden input/output vectors that every edition must reproduce
  exactly (§31.4).

An edition is then any implementation that passes the corpus and SCTS. Rust remains the
**reference edition**, which is a narrower claim than "the real one": it means that where the
specification is ambiguous and two editions disagree, Rust's behaviour is what the specification
is amended to say. It does not mean an edition is second class.

### 31.2 What is per-edition, and what can never be

Some things are *formats*, and a format with two implementations of its own definition is not a
format. These stay single-source no matter how many editions exist:

| Single-source, always | Why |
|---|---|
| The RHAL C ABI (`bindings/c/skynet_rhal.h`) | It is the ABI. Two definitions of a struct layout is a segfault. |
| The `.sky` package format | A package installed by one edition must install identically under another. |
| The embodiment descriptor schema | The body description must mean one thing. |
| MCAP channel schemas | A black box sealed by one edition must be readable by every other. |
| The Hub wire protocol | One API, many clients. |
| **The conformance corpus** | The thing all editions are measured against cannot itself be per-edition. |
| The kernel's `sin`, `cos` and `sqrt` | §44.2's forward kinematics needs them and no platform library is required to round the way another one does. |

Everything else is per-edition: the Safety Kernel, `skynetd`, the policy runtime, the contract
implementations (transport, admission, certification), the CLI and the SDK.

The last row is the newest and the least obvious, so the reasoning is worth stating. ECMAScript
permits `Math.sin` an implementation-defined error, every C library rounds its own way, and a
Cortex-M without double-precision hardware runs a third routine again — so two editions computing a
pose through their own libraries diverge on the first revolute joint, and the divergence presents as
a specification disagreement rather than as what it is. Worse, §7.4's certified tier mirrors this
kernel onto a *separate processor with a different toolchain*, where a libm difference makes the
redundant channel disagree with the primary about a clamp: the exact event the redundancy exists to
detect, fired by correct behaviour, which is the fastest way to have redundancy switched off. So the
specification defines the three functions as an explicit sequence of IEEE-754 binary64 operations —
addition, subtraction, multiplication and division are correctly rounded everywhere, so the same
sequence yields the same bits everywhere — and the corpus pins the poses, not only the verdicts.

### 31.3 The edition matrix

The honest part, and the one that makes this design hold together: **not every language can serve
every tier.** A garbage-collected runtime cannot promise a clamp inside a 1 ms deadline, and an
edition that shipped an "enforcing" kernel on one would be making exactly the claim the tier model
exists to prevent. So each edition declares a **tier ceiling**, enforced the same way
`resolveEnforcement()` enforces it today — derived from the edition, never chosen by it.

| Edition | Ceiling | Kernel | Standing | Why the ceiling is there |
|---|---|---|---|---|
| **Rust** | Certified | Yes | **Reference** | `no_std`, no GC, Ferrocene qualified to ASIL D / SIL 3. |
| **C** | Certified | Yes | Full | MISRA C is the most trodden certification path in the industry and the smallest MCU footprint. Strongest candidate for the safety-MCU build specifically. |
| **C++** | Certified | Yes | Full | The largest population in robotics; ROS 2 is C++ and every vendor SDK is. Certified tier requires a MISRA C++:2023 or AUTOSAR C++14 subset — no exceptions, no RTTI, no dynamic allocation after init. |
| **Zig** | Host | Yes | Community | Genuine systems language with a native C ABI story. No qualified toolchain exists, so it cannot reach the certified tier — a limitation of the ecosystem, not the language. |
| **Go** | Guest | No | Full | GC pauses. Excellent for `skynetd`-equivalent fleet services *off* the robot, and for tooling. |
| **Python** | Guest | No | Full | GIL and GC. Cannot host an enforcing kernel and should never claim to. Remains the best edition for simulator drivers, bench harnesses, research and the SDK. |
| **Java / Kotlin** | Guest | No | Community | GC. Industrial automation shops with existing JVM stacks. |
| **C# / .NET** | Guest | No | Community | GC. Windows-based cells and existing industrial tooling. |
| **TypeScript** | Off-robot | No | Full | The Hub, the workstation CLI, and the WASM host for the contracts. Never runs on a robot. |

Three rules keep the matrix honest:

1. **An edition below its ceiling reports advisory enforcement**, exactly as the guest tier does
   today. A Python edition is not a weaker kernel; it is an *observer*, and it says so in those
   words on every surface.
2. **An edition may not raise its own ceiling.** The ceiling is a property of the edition, checked
   at build and reported at check-in, not a configuration flag.
3. **A ceiling is not a judgement about the language.** Python's ceiling is about a GC, not about
   Python. The matrix reads as a capability statement, never as a ranking.

### 31.4 The conformance corpus — how N implementations stay one system

This is the load-bearing addition, and without it the whole plan is unsound.

The corpus is a set of versioned, signed test vectors — plain data, no code — living beside the
specification:

| Corpus | Input | Expected output |
|---|---|---|
| `cycle/` | Cycle vector + resolved limits + envelope | The exact `CycleVerdict`: guards tripped, clamps applied, response |
| `transport/` | Source embodiment + descriptor + options | The exact `TransportPlan`: every binding, method, scale, limit, warning |
| `admission/` | Package manifest + device capacity + flags | The exact finding list, in order, with severities |
| `certification/` | SCTS results + field conformance + bench age | The exact award, failures, missing, reasons |
| `roles/` | Role strings, valid and malformed | Parse result or null |
| `readiness/` | Registry subject | Attainable tier and blocker list |

Four rules govern it:

* **Byte-identical output, not equivalent output.** Serialised to canonical JSON and compared
  exactly. "Equivalent" is where drift hides — a warning in a different order today is a different
  clamp tomorrow.
* **Differential fuzzing in CI.** Generated inputs run through every edition and diffed on every
  commit. A divergence is either a specification ambiguity or an implementation bug, and it blocks
  the release of the edition that diverged — never of the others.
* **A new case lands in the corpus before the fix lands in any edition.** This is the rule that
  keeps the corpus ahead of the implementations rather than a retrospective description of them.
* **`safety.kernel.parity` generalises.** The SCTS test that today holds a C kernel to the
  TypeScript reference becomes "this edition reproduces the corpus", and every edition sits the
  same exam.

An edition that has not passed the corpus is not an edition. It cannot carry the name, appear in
the matrix, or be installed by `skynetd` — which is the same mechanism SCTS applies to robots,
pointed at implementations.

### 31.6 The wire fixtures — the same mechanism, pointed at the boundary

The corpus holds N editions to one specification. It cannot see the **boundary**: the Hub is a
different repository with a different language and a different test runner, and §17's documents cross
between them.

That gap has a shape, and §76 is the instance that found it. Both sides of the episode path were
built, both were tested, and neither had ever put one document through the other — they did not
agree about a single field name. Each suite was correct and each was about half a boundary: one
asserted what the robot sends, the other asserted what the Hub accepts.

So a second frozen set, `reference/wire/`, built on the same idea and with three properties that
make it worth having:

1. **Produced by the code, never typed.** Each file is the exact canonical body the daemon puts on
   the wire for one route, written by its own suite from the real producer. A fixture somebody wrote
   beside the test that reads it is a record of what its author believed.
2. **Read by the far side's real contract.** The Hub pushes each document through the receiving
   route's validation and asserts the facts survive — not through a parser written for the test.
3. **Pinned in both repositories**, so a fixture that moves without the other side being resynced
   fails in CI with one checkout, which is what CI has.

The result is that a change to a producer fails in the OS suite with the command that re-freezes it,
and a change to a route that stops reading a field fails in the Hub's. The two then land together,
which is the property neither side could have alone.

It is deliberately **not** a second corpus. Nothing is frozen here that no producer emits — a fixture
for a document nothing writes freezes somebody's guess, and hides exactly the gap this set exists to
find.

### 31.5 The cost, stated plainly

Because it should be visible rather than discovered:

* **N× maintenance.** Every spec change is N implementations, N reviews, N releases.
* **N× security surface.** A memory-safety bug in the C or C++ kernel is a bug the Rust edition
  does not have, and adopters of that edition carry it.
* **The corpus is permanent work.** It must grow with every feature, and a feature without corpus
  coverage is a feature where editions may already disagree. §31.6's wire fixtures are the same
  standing cost pointed at the boundary, and the same rule applies: a document that crosses §17
  without one is a document the two repositories may already disagree about.
* **Certification is per edition, not per product.** A vendor certifying on the C++ edition has
  certified the C++ edition. The badge names it.

The mitigation is scope discipline: the kernel is roughly two thousand lines. Editions are viable
precisely because the component that must be identical is small. Should the kernel grow, the
edition count must shrink — and that trade should be made deliberately rather than discovered
three editions later.

---

## 32. The interface

Two interfaces, and the split is not cosmetic. When the network is down the Hub is unreachable,
and that is exactly the moment somebody needs to see the robot.

### 32.1 Hub-side (`/skynet/os/*`)

| Route | Shows | Needs |
|---|---|---|
| `/skynet/os` | Tier model, registry readiness, fleet-wide blockers | Nothing new — runs on the existing registry |
| `/skynet/os/editions` | The §31.3 matrix: ceiling, kernel, standing, corpus status and downloads per edition | `skynet_os_editions`, `skynet_conformance_results` |
| `/skynet/os/devices` | Fleet: tier, enforcement, running digests, generation lag, guard trip rate | `skynet_device_state` |
| `/skynet/os/devices/[id]` | One robot: active and unavailable guards with the missing input named, resolved limits and their provenance, current transport plan, black box index | `skynet_device_state`, `skynet_incidents` |
| `/skynet/os/transport` | Interactive: pick a policy version and a robot, get the per-channel plan before downloading anything | The transport endpoint |
| `/skynet/os/certification/[slug]` | Award, the evidence behind it, expiry, live field conformance | `skynet_certifications` |
| `/skynet/robots/[slug]` | An OS-readiness panel added to the page that already exists | Nothing new |

**Registry readiness is the one to build first** because it needs no new tables and answers a
question the ecosystem actually has. Its vocabulary is deliberately distinct from certification:
it reports an **attainable** tier — the paperwork is in order, the measurements may now be taken —
where SCTS reports an **awarded** one. Collapsing the two would put "host tier" over a robot
nobody has ever measured, which is the precise failure this platform exists to avoid.

Its most useful output is the fleet-wide blocker count, because "twelve robots have no published
adapter" and "twelve robots have no sourced datasheet" describe completely different problems and
imply completely different work.

### 32.2 The on-robot console

Served by `skynetd` over the LAN. Small, static, no build step, no CDN — it has to render on a
robot with no internet, which rules out every fashionable way to build it.

| Panel | Contents |
|---|---|
| **Status** | Tier, enforcement mode, state machine position (`HOLD` / `RUNNING` / `DEGRADED` / `STOPPING` / `SAFE`), uptime, edition and version |
| **Stop** | A large, always-reachable software stop. Never below the fold, never behind a confirm dialog on a page whose purpose is stopping a robot. |
| **Guards** | Live: which are active, which are unavailable and the exact input missing, trip counts this session |
| **Plan** | The resolved transport plan and the clamps in force, with provenance for every number |
| **Packages** | Installed packages, their grants, and grant/revoke — revocation taking effect on the next command, not the next restart |
| **Black box** | Recent events, seal-and-download, upload state |

Two constraints that are design decisions rather than details: **it must work with no network**,
and **it must never show a value it did not receive**. A dashboard that renders a stale torque
reading as a live one is worse than a blank panel, so every field carries its age and a stale
field reads as stale.

### 32.3 Constraints inherited from the project

* Every string translatable — a key in `messages/en.json`, then `i18n-catalog.mjs sync`. Both
  interfaces, all 35 locales.
* Device and fleet routes are dashboard-shaped and take the `noindex` treatment in
  `next.config.ts`, alongside `/dashboard` and `/settings`.
* The public pages (`/skynet/os`, `/editions`) are SSR with canonical and hreflang via
  `buildSkynetMetadata()`, like every other Skynet page.
* Empty states say what is absent. "No robot has checked in" — never a demo fleet, never a
  placeholder chart. The Hub already holds this line for training and simulation and the OS
  surfaces hold it too.

### 32.4 Order

1. `/skynet/os` — readiness over the live registry. No new tables.
2. `/skynet/os/editions` — as the first edition ships, and needed the moment there is more than one.
3. The OS-readiness panel on the existing robot page — small, and puts the answer where people
   already look.
4. `/skynet/os/transport` — once the transport endpoint exists.
5. Devices, certification, and the on-robot console — these follow their tables and their first
   real device, and until then they would be showing invented state.

**The console was built ahead of its place here, and the reason is worth recording** because it
narrows what item 5 is actually about.

Items 1 to 4 are Hub routes, and they are ordered by the tables they need. The console is not a Hub
route — `skynetd` serves it from the machine it is describing, so it needs no table and no fleet,
and the only thing it was waiting on was the risk in the last clause: *it would be showing invented
state*.

That risk is real and it is also the one thing §32.2 already forbids. "It must never show a value it
did not receive" is not a quality bar to be met later; it is the property that decides whether the
panel is worth having at all. A console built to that rule on a robot with no kernel does not invent
a state machine position — it names §27 and says which unwritten thing would supply one. It does not
draw eight green guards; it lists the eight and, for each, the input this robot does not have. The
emptier the robot, the more the page is worth, because what it is reporting is exactly the gap.

So the ordering constraint on the console was never the tables. It was whether the discipline would
hold when there was nothing to show, and the honest answer is that a console which cannot be built
without inventing state is a console that would have invented state on a full robot too.

The other two in item 5 stay where they are: `/skynet/os/devices` and `/skynet/os/certification`
are Hub routes over `skynet_device_state` and `skynet_certifications`, and those tables do not
exist.

---
---

# Part IV — The rest of the operating system

Parts I to III designed the parts that are interesting: the kernel, transport, packages,
conformance, editions. An audit of the document then found the obvious problem with that — of
thirty-three sections, sixteen ran under 280 words, and the thinnest of all was `skynetd` at 129,
which is the single largest component of the system. The safety-critical spine was specified and
the plumbing was gestured at.

Plumbing is what decides whether a robot works in a warehouse in February. This part closes it.

The kernel is not revisited; §7, §23.1 and §25 stand.

---

## 33. skynetd

### 33.1 The property that makes everything else possible

> **`skynetd` is not safety-critical, and it is built that way on purpose.**

It never touches an actuator. It supervises the policy runtime, which posts desired commands into
a ring the Safety Kernel reads, and the kernel is what writes. So `skynetd` may crash, hang, leak,
be killed by the OOM reaper, or be stopped by an operator, and the robot does not become dangerous
— the kernel keeps running, sees no fresh commands, and holds.

That single property is what licenses everything else in this section. `skynetd` can be a large,
complex, allocating, network-facing Rust program with a JSON parser and an HTTP server in it,
because none of that is in the path between a policy and a joint. Had the supervisor been on that
path, every feature below would have had to be argued through a safety case, and the honest
outcome would have been an OS with no fleet management.

Two consequences are worth stating explicitly, because they are testable:

* **The kernel and the RHAL driver survive a `skynetd` restart.** They are separate processes with
  separate lifetimes; the kernel is started by systemd before `skynetd` and does not depend on it.
* **A robot with no `skynetd` at all is a robot that holds position safely.** Not one that keeps
  executing the last policy, and not one that goes limp.

### 33.2 Structure

One process, several long-lived tasks, none of them blocking another:

```
skynetd
├── reconciler      desired → actual, level-triggered, one loop (§33.3)
├── supervisor      process lifecycle, restart policy, crash-loop detection (§33.5)
├── health          liveness and readiness aggregation (§33.6)
├── blackbox        ring-buffer writer, sealing, upload (§13, §26)
├── ota             stage, verify, apply at safe state, roll back (§12)
├── connectivity    bearer selection, backoff, the outbound queue (§34)
└── api             Unix socket (local tools) + LAN HTTP (the console) (§33.7)
```

### 33.3 The reconciler is level-triggered, and that is the whole design

The reconciler compares a **desired state** against an **observed actual state** and emits actions
to close the gap. It does this on a timer and on any event, and it is **level-triggered rather
than edge-triggered**.

That distinction is the most important sentence in this section. An edge-triggered supervisor acts
on transitions — "a new package appeared, install it". Miss one edge, because the process was
restarting or the disk was full or the network dropped mid-message, and the system stays wrong
forever, because nothing will ever re-deliver that edge. A level-triggered supervisor asks "what
is true, what should be true, what is the difference" and converges from *any* starting state,
including states nobody anticipated. It is the lesson every large-scale orchestrator learned
expensively, and a robot that spends days offline is exactly the case that punishes getting it
wrong.

So there is one loop, it is idempotent, and running it twice changes nothing the first run
already did:

```
loop:
  desired ← cached desired state on disk        (refreshed from the Hub when reachable)
  actual  ← observe(): installed packages, running units, image slot, grants, leases
  plan    ← diff(desired, actual)               ordered, §33.4
  for action in plan:
      if action.requires_safe_state and not at_safe_state(): defer(action); continue
      apply(action); observe(); record()
  report(generation, actual, health, attestation)
```

**Observation is never assumed.** `actual` is read from the system — the package store, systemd,
the RAUC slot, the kernel's lease table — never from a memory of what `skynetd` believes it did
earlier. A supervisor that trusts its own record of the past is a supervisor that disagrees with
the machine it is supervising, and on a robot that disagreement is discovered by a person.

**Convergence is one plan being empty, not one pass succeeding.** The loop computes a single plan
from a single observation, so an action that *enables* a later one — activating a package the next
stage would start — does not put that later action in the same pass. A supervisor that reported
"converged" because every action it applied succeeded would stamp the generation on a robot that
has not started the policy yet, which is precisely the claim §17 forbids the platform from making.
The test is the level-triggered question asked again: given what is true now, is there anything
left to do?

Several passes may therefore be needed, bounded by the six stages below, and running them back to
back rather than one per timer tick is a faster loop rather than a different one.

### 33.4 Reconcile ordering, and the safe-state barrier

Actions are not independent, so the plan is ordered rather than a set:

| # | Stage | Requires safe state | Note |
|---|---|---|---|
| 1 | OS image | Yes, to *apply* | Staging is always safe; applying reboots |
| 2 | RHAL drivers | Yes | Changing the driver changes the body |
| 3 | Package installs and upgrades | No to stage, yes to activate | Fetching and verifying is free |
| 4 | Profiles and grants | No | A *narrowing* takes effect immediately; a widening waits for a restart of the affected package |
| 5 | Policy start and stop | Stop, no. Start, yes | Stopping is always allowed |
| 6 | Leases | No | Follows from 5 |

Two rules govern the barrier:

* **Anything that changes what the robot is capable of waits for a safe state.** Swapping a driver
  under a running policy would change the meaning of every command in flight.
* **Anything that reduces what the robot may do applies immediately.** Revoking a grant, narrowing
  a limit, and stopping a policy never wait, because making a robot less capable is never the
  dangerous direction. This asymmetry appears throughout the design and it is deliberate.

A deferred action is reported as deferred, with the reason, so an operator sees "waiting for a safe
state" rather than an update that silently never happened.

One consequence is worth stating because leaving it implicit produced a plan that deferred for ever.
**An upgrade to a package that is running emits a stop for it.** Activation waits for the package to
stop — so that `current` never points at a version other than the one executing, which would make
every report of what a robot is running a lie — and nothing else in the plan would ever stop a
package the desired state wants running. §33.5 is what makes that affordable: a restart is never a
motion event, because the kernel holds throughout.

A *grant* being widened is the opposite case and correctly waits without forcing anything: §33.4's
table says a widening "waits for a restart of the affected package", so the action stays deferred,
visibly, until something else restarts it.

### 33.5 Supervision and crash policy

Every package runs as a transient systemd unit that `skynetd` owns. Restart policy is per package
kind, and the numbers are defaults an operator profile may narrow:

| Kind | On exit | Backoff | Crash loop |
|---|---|---|---|
| `policy` | Restart | 1 s, ×2, jittered, capped at 60 s | 5 failures in 10 min → `failed`, stop retrying, report |
| `service` | Restart | as above | as above |
| `driver` | **Do not restart automatically** | — | A driver that crashed left hardware in an unknown state. Recovery is a deliberate act. |

Three properties:

* **The kernel holds through every restart.** A restarting policy stops posting commands, the
  watchdog observes it, and the robot comes to a controlled stop. Restart is therefore never a
  motion event, and this is the reason the restart policy can be liberal.
* **Crash loops stop.** Infinite restart is how a robot spends a night power-cycling a driver and
  logs ten million lines. After the threshold the package is marked `failed` and left alone.
* **Every restart is a black box event** with the exit status and the last verdict before exit, so
  a crash that correlates with a particular command is findable afterwards.

### 33.6 Liveness is not readiness

Two distinct health signals, and conflating them is a classic robotics failure:

* **Liveness** — the process exists. Cheap, and nearly worthless on its own.
* **Readiness** — the process is producing commands at its declared rate, with an advancing
  sequence number.

A policy that is alive but not ready is **worse** than one that has died, because a dead process
is obvious and a live-but-stalled one looks fine on every dashboard while the robot stands still.
The watchdog catches it in the control path; readiness is what catches it in the *reporting* path,
so an operator sees "running, not producing" rather than a green tick.

Aggregate health rolls up: per-package liveness and readiness, budget headroom, guard trip rates,
thermal and battery state (§36), storage headroom, and generation lag against the Hub. It is
reported at check-in and rendered on the console. Every field carries its age.

### 33.7 The local API

A Unix socket at `/run/skynetd.sock` and an HTTP listener on the LAN, both carrying **the same
REST shapes as the Hub**. That is not tidiness for its own sake: it means `skynet ps`, `skynet
logs`, `skynet install` and `skynet grants` are the same verbs whether they are pointed at a robot
or at the cloud, and it means the on-robot console and the Hub console can share a client.

The socket is root-and-`skynet`-group only. The LAN listener is subject to §39.

### 33.8 When `skynetd` itself fails

| Failure | Behaviour |
|---|---|
| Crash | systemd restarts it. It re-observes and reconciles from the on-disk desired state. Nothing is lost, because nothing important lived only in its memory. |
| Hang | `WatchdogSec` in the unit file. `skynetd` pings it from the reconcile loop, so a hung loop — not merely a hung process — trips it. |
| Disk full | The black box is preallocated (§28) and keeps recording. Package installs fail loudly and GC (§37.3) runs. Reconcile continues for everything that does not need disk. |
| Corrupt desired state | Falls back to the last-known-good copy, then to "run nothing", which is the safe default. It never invents a desired state. |
| Cannot reach the Hub | Not a failure. §34. |

---

## 34. Connectivity

### 34.1 The robot is assumed offline

Network is opportunistic, never assumed, and never in the control path — the invariant from §11.
Everything here is about making the *reporting* path survive a link that is down most of the time.

### 34.2 Outbound only

**The robot never accepts an inbound connection from the Hub.** Every exchange is initiated by the
robot: pull desired state, push check-ins, push telemetry, push incidents.

This is a security decision before it is a networking one. It means no listening port exposed to
the internet, no inbound firewall rule, no port forwarding, and no problem behind carrier-grade NAT
on a 5G modem — which is where a large share of field robots actually live. It is what the
pull-and-reconcile model in §11 buys, and it is worth more than the latency it costs.

The LAN console (§39) is the sole listener, and it is bound to link-local interfaces only.

### 34.3 Bearers and cost

A robot may have ethernet on a dock, Wi-Fi in a building and 5G outdoors, and they are not
interchangeable:

| Bearer | Preference | Metered | Used for |
|---|---|---|---|
| Ethernet | 1 | No | Everything, including bulk upload and OS images |
| Wi-Fi | 2 | Configurable | Everything unless marked metered |
| Cellular | 3 | Assume yes | Check-ins and incidents only, by default |

**Bulk transfer respects metering.** A sealed black box is tens of megabytes and an OS image is
hundreds; pushing those over a metered link because it happened to be the only one up is how a
fleet generates a surprise invoice. Bulk classes wait for an unmetered bearer, and the wait is
visible rather than silent.

### 34.4 The outbound queue

Bounded, on disk, survives reboot, and — because it is bounded — has an explicit **drop policy**.
Something must be discarded when a robot has been offline for three weeks, and deciding what
*after* the disk fills means deciding it badly:

| Class | Priority | Dropped |
|---|---|---|
| Incidents (sealed black box on a fault) | 1 | **Never.** Disk is reserved for them at install. |
| Check-ins | 2 | Superseded — only the newest matters, so older ones are collapsed rather than queued |
| Episodes | 3 | Oldest first, once above the episode quota |
| Telemetry and metrics | 4 | Oldest first, aggressively |

Check-in collapsing is worth noting: a week offline should produce one current check-in on
reconnect, not ten thousand historical ones. State is level-triggered here too.

Backoff on failure is exponential with jitter, capped at fifteen minutes. Jitter matters at fleet
scale — a hundred robots that lost the same access point must not retry in lockstep and take down
the endpoint the moment it returns.

#### Four policies, not one policy with four thresholds

That distinction is what makes the table worth having. Telemetry is a stream where the newest is
worth most and any single item is worth nothing. A check-in is *level-triggered state*, so
enqueueing one removes the last. An episode is data somebody collected and paid for, so it goes
oldest-first and only above a quota. An incident is the entire reason the black box exists.

Two rules fall out and both are load-bearing:

* **Nothing is ever evicted for something less important than itself.** Filling a disk with
  telemetry must not be able to take out an episode. When the only items left outrank the newcomer,
  the enqueue is **refused** rather than allowed to evict upwards.
* **The reserve is a floor for incidents, not a ceiling for everybody else.** Incidents draw on the
  whole budget; every other class stops at the reserve line. So a queue packed to its shared limit
  still takes an incident, and takes it *without discarding anything* — the reserve is not made of
  other people's items.

An incident that does not fit even in its reserve is an **error naming a full disk**, not a licence
to discard the previous incident. There is nothing in this queue worth less than an incident.

#### A drop is recorded, always

§47.4's rule applied to a queue rather than to a ring: *a gap annotated as a gap is honest; a gap
that looks like a quiet period is not.* Every discard increments a counter that lives with the queue
and survives a reboot, so a fleet console can say "this robot dropped forty thousand telemetry items
last month" instead of showing a suspiciously tidy series.

Collapsing a check-in is **not** a drop and is counted apart from one. Nothing was lost: the newer
item carries the same level-triggered state, which is the whole reason the older one can go.

#### The budget lives with the queue

*"Disk is reserved for them at install"*, taken literally: the budget and the reserve are written
into the queue's own directory the first time it is opened, and every later opener adopts them. Two
callers disagreeing about the budget would enforce two drop policies over one directory, and a
console would report a limit nothing was applying. Re-sizing an existing queue is deliberately not a
side effect of opening it — it changes retrospectively which items would have been dropped.

#### Full jitter, not a perturbation

The delay is uniform over `[0, exponential]` rather than `exponential ± 10%`. The narrow form leaves
a hundred robots that lost the same access point retrying inside the same tenth of a window, which
is the thundering herd this rule exists to prevent, smeared slightly rather than broken up. Its cost
is that a robot occasionally retries sooner than the curve suggests, which is harmless. The jitter is
**supplied** rather than drawn, for §35.2's reason: a function that reaches for a random number
cannot be tested, and an untested backoff is how a fleet discovers the cap was off by a factor of
sixty.

#### What is queued today, and what sends it

§38.1's alarm, which had been recorded as unraised for two sessions for want of anywhere to put it:
a profile signature that **fails** — as against one merely absent or from an untrusted key — is an
incident, because a signature that verifies against nothing was made over *other bytes*, which is
what "somebody editing limits on a robot's disk" leaves behind. `absent` and `untrusted` raise
nothing, for the same reason §37.1 keeps them apart from `failed`: both are administrative gaps,
both are visible on every surface, and an incident for either trains an operator to close incidents
without reading them.

Incidents and check-ins send. §34.2's outbound path and §10's pairing are written, and each of those
two classes addresses a route in §17.

Telemetry had none until §74 added one, which is why the drop policy above was for a long time the
whole of what happened to it: items accumulated under it and the surfaces said so rather than
presenting a queue that only grows. Episodes are still in that state — `Class::Episode` has a route
in §17 and nothing on a robot produces one.

---

## 35. Time

### 35.1 Two clocks, and control reads only one

| Clock | Property | Used for |
|---|---|---|
| `CLOCK_MONOTONIC_RAW` | Never adjusted, never steps, unaffected by NTP | **Every control decision.** Watchdog, deadlines, rate limits, black box ordering |
| `CLOCK_REALTIME` | Wall time, may step, may go backwards | Logging, scheduling, correlation with the Hub |

**No control decision ever reads wall time.** An NTP step, a leap second or an operator correcting
the date must not be able to trip a watchdog or widen a deadline. Both clocks are recorded in the
black box, so a wall-time jump is visible in replay rather than being a mystery in the timeline.

### 35.2 The kernel takes no clock at all

The Safety Kernel does not call any clock function. Its tick is **supplied** — `now_ms` is a
parameter, which is why `evaluateCycle()` in the reference implementation takes one rather than
reading a clock itself.

Three things fall out of that, and the third is the reason for the design:

1. The kernel is trivially testable — a cycle vector includes its own time.
2. Deterministic replay works, because replay supplies the recorded timestamps.
3. **Simulation works without a special case.**

### 35.3 Sim time

A simulator RHAL driver supplies its own clock, and simulation rarely runs at wall speed — Isaac
Sim may run a 200 Hz control loop at a fifth of real time on a busy workstation, or ten times real
time headless.

Because the kernel's time is supplied by the RHAL rather than read from the OS, **sim time simply
is the kernel's time**, and everything downstream follows: watchdogs, rate limits and deadlines are
all measured in the same units the simulator is producing. Without this, every simulated run would
trip the watchdog within a few cycles, and the usual workaround — disabling the watchdog in
simulation — would mean the thing being tested is not the thing that ships.

The rule that keeps it honest: **a simulator driver must declare itself one.** The tier is forced
to `guest`, the enforcement mode to advisory-with-simulation noted, and check-ins carry the flag,
so no simulated run can ever be presented as bench evidence in SCTS.

The field is **`simulator`** on the embodiment descriptor: a boolean whose absence means `false`,
because a real robot should not have to declare what it is not and the obligation to declare sits on
the simulator driver, where this section puts it. `awardCertification()` reads it from the
descriptor rather than from the driver's declared ceiling, because a simulator driver written in C++
has no language ceiling to fall back on — the Python one is capped twice and that is a coincidence
of the first two we happen to have. The cap is applied *after* the evidence walk, so the verdict
still records which tier the tests would have supported: "the evidence supports host; the body is
simulated, so guest" is a more useful sentence to a vendor than a silent guest.

### 35.4 Synchronisation

NTP when a network exists, for wall time only. PTP or gPTP where the hardware supports it, which
is what makes the multi-sensor alignment in §25.1 achievable rather than approximate. Where neither
exists, the observation assembler reports larger skew, which is the honest degradation.

---

## 36. Power and thermal

> **Amended by the implementation.** This section was four subsections of prose for eleven parts of
> this document, and §73 listed it as the one row that was *unwritten* rather than merely
> unfinished. Writing it settled five things the prose left open, and each is recorded below beside
> the sentence it interprets rather than in a changelog — because the next person to read §36.1's
> table needs to know that "approaching reserve" is 2.25 × the reserve and why, not that a session
> once decided it.
>
> Built as `crates/skynet-contracts/src/power.rs` and `reference/typescript/power.ts`, held to each
> other by `corpus/power/` and `corpus/vocabulary/closed-sets.json`.

### 36.1 Low battery is a safety event

A laptop at 3% shows a warning. A humanoid at 3% falls over. Power is therefore handled in the
same register as the safety envelope, not as a user-experience concern.

| State | Trigger | Behaviour |
|---|---|---|
| `normal` | Above reserve + margin | Full operation |
| `low` | Approaching reserve | No new task leases granted; running task may finish; operator notified |
| `reserve` | At the reserve budget | Controlled stop, then move to a safe state — park, sit, or dock |
| `critical` | Below reserve | Immediate controlled stop. Drives disabled once at rest. |
| `unmetered` | This body's energy is not metered | Full operation, and the report says why |

**The bands, and why they are multiples of the reserve.** "Approaching" and "at" are a shape rather
than a number, and the numbers are `reserve` up to **1.25 ×** the reserve and `low` up to **2.25 ×**.
Both are multiples of the reserve rather than percentages of the pack, because the reserve is the
only energy scale the body declares and a percentage would mean something different on every robot.

The two widths are chosen for different reasons and neither is round-number convenience:

* The `reserve` band is **a quarter of a reserve wide** so that a gauge reporting within a percent
  of the boundary does not chatter between *begin a controlled stop* and *stop immediately*, on a
  robot that is already mid-ramp.
* The `low` band is **one whole reserve wide**, because `low`'s only behaviour is that a running
  task may finish and no new one may start. A warning that arrives with less than one further safe
  stop in hand is a warning the robot cannot act on, and the row's promise would be one it cannot
  keep.

**`unmetered` is the fifth state, and §36.1's original four assumed a battery.** An arm bolted to a
bench and run off mains has no state of charge, and forcing it into one of the four would either
immobilise it or claim a headroom nobody measured. It is not a rung of the same ladder, which is why
it is listed last rather than beneath `critical`: it grants leases, and a body reaching it is
usually behaving correctly.

**§36.1's `low` row is a rule, and §54.4's `acquiring` stage is where it is enforced.** A skill that
reaches `acquiring` on a robot that is withholding is refused there — terminal, aborted,
`power_withheld` — and §25.2's table is **not consulted**: a refusal that had taken two of three
tokens first would be holding an arm for an invocation that is about to abort.

It is refused rather than parked. An invocation waiting in `acquiring` for somebody to bring a
charger burns §54.4's timeout and then aborts with the wrong reason on it, and "no new task leases
granted" is a refusal in the row's own words.

`power_withheld` is its own lifecycle reason rather than a second `lease_denied`, for the reason
that enumeration has nineteen entries: only the reason tells an operator whether to call the
publisher, find the package holding the arm, or **put the robot on a charger**. Three errands, and
a shared word sends two of them to the wrong one.

And the row's other half — *a running task may finish* — falls out of where the check sits. A skill
already past `acquiring` holds its leases and is not disturbed when the battery falls, because a
policy dropped mid-motion is the failure a controlled stop exists to avoid.

**Two bodies reach `unmetered` and they are not the same robot.** One declares no pack. One declares
a pack whose gauge has stopped answering. The distinction is a *fault* rather than a state —
`gauge_silent` — because it is a fact about whether the number can be believed rather than about how
much energy is left, and an operator needs both. A silent gauge is deliberately **not** `critical`:
§36.4 overrides the gauge on a *positive* observation from the bus, missing evidence is never a
positive finding anywhere else in this system, and a robot that stopped dead on a failed I²C read
would be a robot that stops dead on a failed I²C read. It withholds new leases and does not take
away the work already running — which is `low`'s behaviour, arrived at from the other direction.

### 36.2 The reserve budget

**The robot always retains enough energy to reach a safe state**, and how much that is depends on
the body: a quadruped needs a second to sit, a humanoid holding a payload needs longer and needs
more, an arm needs almost none.

So the embodiment descriptor carries it: `time_to_safe_state_s` and `safe_stop_energy_wh`,
declared by the vendor and — like every other declared figure in this system — measured by the
bench suite rather than trusted. A body that declares two seconds and takes six has a reserve
budget that is a third of what it needs, and that is exactly the kind of number that is discovered
in an incident report if nobody measures it first.

The reserve is not a fixed percentage. It is computed from the declared figures, the current
payload, and the distance to the nearest safe state where the robot knows one.

**One of those three terms exists on this platform, and the report says so.** There is no payload
mass anywhere on the descriptor and no distance-to-safe-state anywhere in the observation, so the
reserve computed today is `safe_stop_energy_wh` and nothing else. Scaling it by a plausible payload
factor would have been one line of arithmetic and a number nobody could defend — and this is the
number that decides when a two-metre humanoid stops walking.

So the reserve is reported with an `unaccounted` list naming §36.2's missing terms, and a reserve
with a non-empty list is a **floor** rather than the reserve: every missing term can only add
energy, so the figure is an under-estimate and every state derived from it is optimistic by an
unknown amount. Under-estimating is the direction that runs a robot past the point where it can
still sit down, which is why it is named rather than rounded up to something comfortable.

### 36.2.1 The descriptor's `power` block

```json
"power": {
  "time_to_safe_state_s": 2.0,
  "safe_stop_energy_wh": 20.0,
  "pack_capacity_wh": 500.0,
  "thermal_throttle_c": 70.0,
  "thermal_critical_c": 90.0,
  "brownout_fault_codes": [4097]
}
```

`pack_capacity_wh` is here so that a gauge reporting a *fraction* can be compared against a reserve
in watt-hours. A gauge that reports `energy_remaining_wh` directly is preferred and needs no
capacity — whatever told it the capacity is closer to the pack than a descriptor field is — and a
body with neither has a state of charge and no way to turn it into energy, which is reported rather
than approximated.

Absent and all-null are the same body to every consumer. Unlike `frames`, this block carries no list
whose emptiness is a claim, so there is nothing for the two spellings to disagree about.

### 36.3 Thermal, and the degradation order

Under thermal or power pressure, things are given up in a fixed order, weakest claim first:

```
telemetry  →  OTA  →  black box compression  →  policy inference rate
           →  non-essential sensors  →  ...  and never: RHAL, kernel, control loop
```

**Inference is throttled before control.** A policy running at 20 Hz instead of 50 is degraded and
says so; a control loop running late is a fault. And the kernel is never throttled at all — it is
a few microseconds of arithmetic per cycle, and there is no thermal situation in which giving up
the safety envelope is the right trade.

Sustained thermal throttling raises `DEGRADED`, which is reported and which blocks any package
whose declared `inference_budget_ms` can no longer be met. A policy quietly missing its own
declared deadline is precisely what the budget exists to prevent.

**How deep the ladder is cut, which the order alone does not say.** Two thresholds, both declared by
the body as `thermal_throttle_c` and `thermal_critical_c`, and the depth is the meaningful part:

| Pressure | Given up |
|---|---|
| `nominal` | Nothing |
| `throttling` | The first **three** rungs — telemetry, OTA, black box compression |
| `critical` | All five, inference and non-essential sensors included |

The line between three and four is where housekeeping ends. Telemetry, OTA and black-box
compression cost the robot nothing it was hired to do; rungs four and five change what it can *do*,
and are held back for the temperature at which the alternative is damage. `RHAL`, `kernel` and
`control_loop` appear in no row of that table, at any pressure, and are frozen as data so that
"never" is something a test checks rather than a word in a paragraph.

**"Sustained" is thirty seconds.** A threshold of one cycle makes every heavy motion a degraded
robot; a threshold of ten minutes makes `DEGRADED` a state a thermally limited machine never reaches
while spending its afternoon sitting at the throttle point.

**A temperature with no declared threshold sheds nothing, and says so.** Nobody has stated what hot
is on that body, and 85 °C is right for a CPU package and fifty degrees wrong for a harmonic drive.
It is a named gap, not a silent pass and not a guess.

**The budget is enforced against the measurement, not against the weather.** `inference_budget_ms`
blocks a package when the *observed* loop time exceeds the declared one — thermal throttling being
the usual cause and not the only one. A robot that refused a package for being warm while its loop
sat comfortably inside budget would be enforcing a proxy instead of the thing itself. And a package
is never blocked on an absence: no declaration or no measurement means the question cannot be asked,
which is not the same as failing it.

### 36.4 Brownout

Voltage sag is reported by the RHAL as `RHAL_DEGRADED` with a vendor fault code. `skynetd` treats
it as `critical` regardless of what the fuel gauge claims, because a gauge that disagrees with the
bus is a gauge that is wrong.

**Which is a rule this OS could not follow as written.** §24 makes `fault_code` *"vendor-specific,
surfaced verbatim to the black box"* — that is, the OS is told never to interpret one — and this
paragraph asks it to act on exactly one meaning. An OS that guessed which codes meant sag would be
interpreting a vendor's private vocabulary in the one place where being wrong stops a working
machine, or fails to stop a falling one.

So the vendor who defined the codes names the ones that mean sag, in the descriptor's `power` block:

```json
"brownout_fault_codes": [4097, 4098]
```

The OS then **matches** a number against a list rather than interpreting anything, and §24's rule is
kept intact. `RHAL_DEGRADED` carrying a code that is not on the list is not a sag — it is the dead
encoder or the thermal fault that flag also covers.

**An empty list is not "this body cannot brown out".** It is *nobody has said*, and it is reported
as `brownout_undeclarable` at the moment it matters — when the RHAL is actually degraded and
nothing on the robot can say why. The robot keeps working: a hole in a vendor's declaration is a
reason to say so, not a reason to stop a machine that is running.

### 36.5 Where the numbers come from, on a machine

| Field | Source |
|---|---|
| `energy_remaining_wh` | `/sys/class/power_supply/<pack>/energy_now`, µWh |
| `state_of_charge` | `<pack>/capacity`, per cent — only when there is no `energy_now` |
| `temperature_c` | the hottest `/sys/class/thermal/thermal_zone*/temp` |
| `throttled_for_ms` | the daemon, from the instant the temperature first crossed the threshold |
| `rhal_degraded`, `fault_code` | **nothing** — see below |

**Which supply is the pack is a question and which zone is hottest is not.** Every
`/sys/class/power_supply` entry declares its own `type`, so the batteries are found rather than
guessed at; what remains is the genuine ambiguity, and a machine with two batteries has not said
which drives the legs. Summing them would be a guess about whether the second is a backup cell, a
tool battery or half of one pack — and §36.2's reserve is compared against the answer. So: no
battery reads nothing, one battery is the pack, and more than one reads nothing and names them,
until `state/power.json` says which. That is §34.3's shape for bearers, for §34.3's reason.

The thermal zones need no declaration, because the maximum over every zone the kernel exposes *is*
"the hottest thermal zone", whatever each zone measures. There is no question there to get wrong.

`charge_now` × `voltage_now` is **not** converted to watt-hours, and that is deliberate: the product
is an energy computed from a voltage that sags under load, so it under-reads exactly when the robot
is working hardest. A named gap beats a number whose error moves with the thing it is measuring.

**§36.4's override has no input on this edition.** `RHAL_DEGRADED` and its fault code come from the
RHAL, and §33.1 keeps `skynetd` off that path — the daemon does not depend on the RHAL crate and CI
asserts the edge is absent. So a robot can report `normal` while its bus is sagging, and `GET /power`
says so in as many words rather than letting the word `normal` carry a claim it cannot make.

---

## 37. Package lifecycle

Distinct from OS updates (§12): packages are content-addressed blobs installed into a store, not
filesystem images, and they move far more often.

### 37.1 Install

```
resolve   the desired state names id, version and digest — already resolved, §37.4
fetch     by sha256, skipping any blob already present (dedupe is free, §2 of Part I)
verify    digest, then Ed25519 signature against a key the operator trusts
admit     admitPackage() — signature state, budget, capabilities, grants, dependencies, scan verdict
transport resolveTransport() against this body; a `refused` verdict fails the install
stage     unpack into /data/packages/<digest>/, still inert
activate  atomic symlink swap, then register with the supervisor
```

#### The `transport` stage, which for a long time was the only one that never ran

Five of the six stages above have had an implementation since the store was written. `transport`
had none, and the shape of the gap is one this document keeps finding: `resolveTransport()` had
three callers — the Hub's repository page, `skynet transport`, and the conformance corpus — and not
one of them was an install. `admitPackage()` has taken a `transport_verdict` since it was written,
and on a robot it was always absent, with a comment saying there was no source embodiment to plan
from. **A consumer with no producer.**

There is one now. The package manifest carries `source_embodiment` — the body the policy was
trained on and the action space it emits — which is the half the robot could not otherwise have.
The other half is the descriptor it already holds (§28).

Two options are decided at the install rather than declared by the publisher:

* **`requested_hz` is the package's own `budget.control_hz`**, the rate it says it will run its loop
  at on this robot — not the source embodiment's `control_hz`, which is the rate it was *trained*
  at. A publisher may declare both and they may differ; the plan is made against the one the loop
  will use, because a plan that cleared a rate the package never intends to run is a plan about a
  different robot.
* **`allow_retarget` is the operator's**, and §6.1 is why it cannot be the publisher's:
  *"retargeting changes what the policy is doing. That should be a decision somebody made, not a
  fallback that happens quietly."* So it is a flag on `skynetd stage` — the person running it is the
  somebody — and a reconcile loop, which has no such person, passes `false`.

Two ways there is nothing to plan, and neither is a pass:

* **The package declares no action space.** A driver, an environment, a dataset: none of them
  command a joint, and a transport verdict about them would answer a question nobody asked.
* **This robot has no usable descriptor.** §7.2's rule applies — the absence is reported by the
  surfaces that report descriptors, not converted into a verdict here. A `refused` invented from a
  missing body would fail every install on an unprovisioned robot with a sentence about the policy.

And the refusal names the channel. *"The policy's action space cannot be transported onto this
body"* is true and tells nobody which joint to look at, so the report carries every binding that
came out anything but `direct`, with its role, its method and its reason (§64).

#### The verdict is recorded, not only printed

Admission decides whether code may run on this machine, which makes it exactly the thing an incident
review asks about — and for several sessions it was printed to whoever ran the command and kept
nowhere. Every stage now writes its verdict to the audit log (§38.3), from which `skynetd` mirrors it
onto §26's event channel as an `admission` event.

**On every path out of a successful stage**, including the two where admission reached no verdict at
all: "nothing checked this package, and here is why" is a fact about what is now on the disk, and
omitting it would make an unexamined blob indistinguishable from one nobody staged.

What lands on the event channel is the *decision* — the signature state and the count of errors and
warnings — rather than the findings, because §41.1 keeps the black box out of the logging business
and a ring full of prose is a ring holding fewer control cycles. The findings stay in the report an
operator reads.

#### The signature has five outcomes, not two

`verify` was written as though it answered yes or no, and `admitPackage()` took a boolean. Four
different facts were collapsed into `false`, and they have different owners and different remedies:

| State | What happened | Whose job |
|---|---|---|
| `verified` | Checked against a key the operator trusts | — |
| `failed` | A signature was present and did not verify | **Stop.** The bytes are attested to somebody other than the manifest claims, which is what tampering looks like |
| `untrusted` | It verified, against a key this robot has not been told to believe | The operator's. Add the key, or find out why a stranger signed this |
| `absent` | Nobody signed it | The publisher's |
| `unverifiable` | This platform cannot check a signature at all | The operating system's |

`absent` and `unverifiable` are the pair the original collapse hurt most. An operator told `unsigned`
by a robot whose OS has no Ed25519 will go and ask a publisher to sign a package the publisher
already signed — and the finding was worded as a fact about the *bytes* ("nobody has attested to who
produced them") when it was a fact about the *reader*. `signature_unverifiable` puts the remedy on
the right desk.

`failed` earns its own code for the opposite reason: it is the one state where something went wrong
rather than something is missing, and it should not sit in the same bucket as a package nobody got
round to signing.

`untrusted` is the state a verifier discovers the moment there is a trust store to check against,
and it did not exist while nothing could verify anything. A signature can be entirely valid and made
by somebody this robot has never heard of — which is neither an attack nor a publisher's oversight,
but a decision nobody has made. It is an **error** rather than a warning, because *somebody* attested
to these bytes and it was nobody here.

This is §7.2's rule — a check that could not run reports what was missing, rather than returning a
verdict — applied to the one field where the missing thing might be an attack.

#### A signature travels with the key that made it

An Ed25519 signature does not name its signer. A verifier holding a set of trusted keys and a
signature that verifies under none of them therefore cannot tell `failed` from `untrusted` — an
attack from an administrative gap, which are the two states furthest apart in this table. So the
signature arrives as a pair:

```json
"signature": { "key": "<64 hex public key>", "value": "<128 hex signature>" }
```

and the robot asks two questions in that order: does it verify under the key it names, and is that
key one the operator trusts. Asking them the other way round collapses the distinction the pair
exists to preserve.

The **message signed is domain-separated**, because §38.1 signs operator profiles with the same
machinery and §5 has the vendor sign the descriptor. Without a tag, a signature over one document's
digest would verify over another's:

| Signed thing | Message |
|---|---|
| a package version | `skynet.package.v1\n` + the version digest |
| an operator profile | `skynet.profile.v1\n` + the profile digest |
| an embodiment descriptor | `skynet.embodiment.v1\n` + the descriptor digest |

#### What gating on the signature actually refuses

§19 is specific: *a driver runs native and must therefore be signed by a key the operator has
trusted.* So the gate is on **drivers**, at activation:

* a driver whose signature is anything but `verified` does not activate. The blob stays staged and
  inert, the refusal names the state and the remedy, and it is reported as a decision rather than as
  a deferral, because the same bytes and the same trust store give the same answer every pass;
* every other kind installs carrying its findings, and says `unsigned` in every report that mentions
  it.

That asymmetry is a real gap rather than a principle. Nothing on a robot can *produce* a signature —
the Hub signs releases — so a developer staging a package they built five minutes ago has nothing to
sign it with, and refusing those would refuse every local install and turn the gate into a flag
somebody removes. Closing it needs a way for an operator to sign locally, which is a design question
about developer keys rather than a missing primitive.

At activation rather than at fetch, because a staged blob is inert and the symlink swap is the
consequential moment.

**Activation is a symlink swap**, so a crash at any point before it leaves the previous version
running and a crash after it leaves the new one running. There is no window in which the robot has
half a policy. The swap is `symlink` to a temporary name and then `rename` over the live one,
because `rename` is atomic and unlink-then-symlink is not: between those two calls the package has
no current version at all.

Note the ordering: **transport is resolved at install, not at start.** A policy that cannot reach
this body should fail while somebody is watching, not at the moment it is first asked to move.

#### The on-disk shape, pinned

An edition that laid a package out differently would install packages the others could not read, so
the layout is part of the contract rather than an implementation choice:

```
/data/packages
  /blobs/<digest>/…                    the package, content-addressed
    manifest.json                      the .sky manifest — the Hub's `manifest` column
    …                                  every other file is a version file — the Hub's artifacts
  /active/<ns>/<name>/current          symlink -> ../../../blobs/<digest>
  /active/<ns>/<name>/prev.1           the version a rollback returns to (§37.2)
  /active/<ns>/<name>/prev.2           and the one before that
```

Four rules, each of which is a way two editions could otherwise disagree about the same bytes:

* **`<digest>` is the Hub's version digest**, bare lowercase hex, computed by `versionDigest()` over
  the manifest as a *value* and the remaining files as a `(path, sha256)` list. A manifest that also
  appeared in that list would be hashed twice and no version the Hub published would verify.

  > **Open, and demonstrated rather than argued: `manifest.digest` cannot be this package's own
  > version digest.** The shared package contract documents the field as *"the version digest from
  > the Hub, the identity that matters"*, and the rule above hashes the manifest as a value — so the
  > field is a fixed point of SHA-256, and writing the digest into it changes the digest.
  >
  > On the robot's staging path one of two things therefore happens, always. Either the field is
  > absent — the common case, since no publisher can compute a satisfying value — and the manifest
  > does not decode, so **admission does not run at all** and `verify → admit → stage` loses its
  > middle step in silence; or the field is present with any value and `digest_mismatch` fires, on
  > every package, for ever.
  >
  > Nothing caught it because the admission corpus uses `"sha256:pkg"` on both sides, a placeholder
  > that makes them trivially equal and that also violates this section's own rule that a digest is
  > 64 lowercase hex characters with no prefix. A corpus case is only as sharp as the value it pins.
  >
  > Two defensible fixes and the choice is a contract decision: exclude `digest` from the hashed
  > manifest, or remove it from the manifest entirely and let the identity travel in the desired
  > state, which is where §33.4 already puts it. Either changes the identity of every package across
  > three implementations and the corpus, so it is recorded here rather than guessed.
* **The links are the rollback set.** §37.3 says garbage collection never touches it, and the
  cheapest way to guarantee that is to have no second record of what "previous" means: `prev.N` are
  links like `current`, collection removes a blob nothing points at, and rollback reads `prev.1`. A
  history file kept beside the links is a history file that can disagree with them.
* **The links live in a directory named for the package**, not beside it as `<name>.prev.1`, because
  a package legitimately named `grasp.prev.1` would otherwise have its blobs collected when `grasp`
  is upgraded.
* **Exactly one file is executable: the one `runtime.entrypoint` names.** Everything else is `0644`.
  A file's mode is not part of the digest — the Hub stores artifacts as bytes and a `(path, sha256)`
  pair carries no permission — so a store that preserved whatever mode it found would let two robots
  holding one digest disagree about which files may be executed. Deriving the mode from the manifest
  makes it a function of the digest, because the manifest is digested, and it is the smaller
  privilege besides. A package needing a second executable cannot express that today; that is a real
  limit and it is recorded as one.

#### Identifiers are path components, and therefore a security boundary

A desired state arrives over the network and names packages and digests, and both become path
components under `/data`. A package id is `namespace/name` with each segment matching
`[a-z0-9][a-z0-9._-]*`; a digest is 64 lowercase hex characters and nothing else — no `sha256:`
prefix, because a second accepted spelling is a string comparison that answers "different" about
identical bytes. Neither is normalised into legality: a validator that repairs its input is one
whose callers stop believing the value was ever checked.

### 37.2 Upgrade and rollback

The new version is installed alongside the old and activated by swapping the symlink. The old blobs
stay. Rollback is therefore the same swap in reverse and completes in milliseconds without a
network — which matters, because the moment you most need to roll back is the moment the new
version broke something.

The running version and the previous **N** (default 2) are **pinned** and never collected.

### 37.3 Garbage collection

The store is refcounted by digest. A blob with no referring version, not pinned, and older than a
grace period is collectable. GC runs on a timer, on disk pressure, and after any activation.

**GC never touches the rollback set**, even under disk pressure. A robot that frees space by
deleting the version it would roll back to has optimised for the wrong failure.

Under genuine pressure the order is: unreferenced blobs, then old episodes not yet uploaded (which
are also droppable per §34.4), then a loud failure. Never the black box reserve, never the
rollback set.

### 37.4 Dependency resolution happens on the Hub

The desired state carries a **flat, fully-pinned list** of packages with exact digests. The robot
does no version solving.

This is deliberate. A SAT solver on a robot is a source of unbounded work and non-reproducible
outcomes — two robots with the same desired state could resolve differently, and neither operator
could explain why. Resolving centrally makes the resolution reproducible, auditable, reviewable
before rollout, and identical across a fleet.

The robot's job is narrow: verify, admit, and install exactly what it was given, or refuse and say
which check failed.

---

## 38. Configuration

### 38.1 Safety configuration is signed

Operator profiles carry the limits the kernel enforces (§7.3), so they are **signed with the same
Ed25519 machinery as releases**, versioned, and delivered through desired state. An unsigned
profile is not applied. A profile that fails signature verification is not applied *and* raises an
incident, because the interesting case is not a corrupt file, it is somebody editing limits on a
robot's disk.

#### The robot checks; it does not take the document's word

That last sentence names the attacker, and for a while the implementation handed them the door. The
desired state carried a boolean — `"verified": true` — and the robot applied the limits when it saw
one. But **the desired state is itself a file on the robot's disk**, so anybody who could edit the
limits could edit the assertion that somebody had checked them, in the same pass, with the kernel's
ceilings on the other side of it.

So the document carries the *signature* and never the verdict:

```json
"profile": { "name": "site-a", "digest": "…",
             "signature": { "key": "…", "value": "…" },
             "limits": { … } }
```

and the decoder sets `verified` to false whatever the document says. The only thing that can make it
true is a signature over `skynet.profile.v1` and the profile's digest, checked on this robot against
`/data/state/trust.json` (§19). A document that could assert its own verification is not a document
anybody has verified.

The incident for a signature that *fails* — as against one that is merely missing — is not yet
raised: there is no outbound incident path (§34.4). The profile is not applied either way, so the
limits are safe; what is missing is the alarm.

Non-safety configuration — log level, telemetry sampling, console theme — is unsigned and local.
The line between the two is drawn by whether the value can reach the kernel.

### 38.2 Layering, and it only ever narrows

```
vendor defaults  →  fleet profile  →  site profile  →  cell profile  →  session override
```

Each layer may only **narrow** what the layer above it allows, enforced by the same `narrowest()`
rule as §7.3 rather than by convention. A site cannot raise a fleet ceiling; a session cannot raise
a cell's. Attempted widenings are recorded and discarded, so an operator who believes they have
raised a limit is told they have not.

Workspace volumes (§7.1's fourth guard) layer the same way and by the rule §7.3 states for them: a
frame must satisfy every layer that names it. The layers reach the kernel intact rather than being
merged into one, because merging is what would require a geometry the operator did not draw.

Session overrides are time-boxed and expire — a "maintenance mode, half speed" that outlives the
maintenance is a hazard that looks like a setting.

### 38.3 Where it lives

`/data/state/profiles/`, each with its signature and its provenance. Every change is a black box
event and an audit-log entry, carrying who, when, which layer, and the previous value.

**The same rule applies to the trust store** (§19), and for the same reason: which keys a robot
believes is a decision somebody made that changes what may run on the machine. Each entry carries
`added_at`, `added_by` and a free `note`, all of them optional — a store written by hand before any
of this existed has none, and the honest report of such an entry is **`unrecorded`** rather than a
fabricated date. A key nobody recorded still verifies; the gap is in the audit and not in the
security, so it is a notice and never a refusal.

Provenance is *recorded*, never inferred. `skynetd trust add` without `--by` writes **unattributed**
rather than the invoking user's name: an environment variable is not an identity, and a provenance
nobody typed is exactly the confidently-wrong record this section exists to prevent.

A change made through `skynetd trust` rewrites the store, which raises a hazard worth stating: an
entry the parser could not read must survive the rewrite. It is kept verbatim, in place, because
that entry is the thing an operator came to the command to find — a rewrite that dropped it would
make adding a key a way to delete the bug report.

#### The audit log, and why it is not the black box

`/data/state/audit.jsonl`, one canonical JSON object per line, append-only.

The two records this section asks for are not redundant. The black box is a fixed-size ring that
**wraps**: it holds the last few minutes at full fidelity, which is what an incident needs and
precisely the wrong shape for *"when did this robot start trusting that key"*, a question whose
answer is eleven months old.

JSON Lines rather than an array because **an append must not require rewriting the file**. An array
has to be read, parsed, extended and written back for every entry, so every append is an opportunity
to lose the history — and rewriting an audit log is the operation an audit log exists to make
difficult. It degrades in the right direction too: a line truncated by a power cut costs that line,
not the file. A line that cannot be read is *reported*, never skipped, because a log with invisible
gaps is worse than no log — somebody is relying on it.

Wall time, per §35.1's third job for `CLOCK_REALTIME`. Nothing here is read to make a decision, so a
stepped clock spoils a record and cannot spoil a verdict; a monotonic reading would mean nothing
after a reboot and could not be compared with the date on anybody's ticket.

#### The black box half is mirrored, not written twice

The ring has exactly one writer by construction, and configuration changes are made by *other
processes* — `skynetd trust add` runs while the daemon is up. Two writers to a preallocated file at
computed offsets is corruption rather than a race that occasionally loses a record. So each audit
entry carries a sequence number, whoever makes the change writes the log, and `skynetd` copies
anything past its cursor onto §26's event channel on its next reconcile pass.

Three consequences, each stated rather than left to be discovered:

* A black box event for a configuration change is **delayed by up to one reconcile period**, and on
  a robot where `skynetd` is not running it does not arrive at all. The audit entry always does,
  which is the right direction for the record whose job is to survive.
* **The first pass records a baseline rather than the history.** A robot provisioned a year ago has
  hundreds of audit entries and a ring that holds minutes; replaying the log would throw away the
  incident window in order to record a year of configuration history, most of it about keys since
  removed. So the first pass writes one event saying how many earlier changes exist and where to
  read them, and the cursor jumps to the end.
* **A burst is capped per pass**, so a provisioning script that trusts six keys between two
  reconciles cannot wrap the recording either.

---

## 39. Local access control

### 39.1 Stop is never privileged

> **The stop control on the on-robot console requires no authentication.**

This inverts the usual rule, and it is correct. An authentication prompt between a person and a
stop button is a hazard. The threat model — somebody on the robot's LAN maliciously stopping it —
is a nuisance; the threat model it prevents — somebody watching a robot do something dangerous
while hunting for a password — is an injury.

Everything else requires a credential.

| Role | May |
|---|---|
| *(unauthenticated)* | Stop. See status. |
| Observer | Read health, guards, plan, packages, black box index |
| Operator | Start and stop packages, grant and revoke capabilities, seal and download the black box, apply a staged update |
| Admin | Pair and unpair with the Hub, install drivers, change profiles, factory reset |

### 39.2 Provisioning

Local credentials are issued at pairing (§10) and stored hashed. **There is no default password**,
at any point in the lifecycle. An unprovisioned robot's console offers status and stop and nothing
else, so the failure mode of never provisioning it is a useless console rather than an open one.

Bootstrapping uses physical presence — a chassis button held during pairing, or a QR code printed
on the robot — because possession of the machine is the only credential that means anything before
any other credential exists.

The LAN listener binds to link-local interfaces only and is never routed. Reaching a robot's
console from outside its network is not a feature and is not a configuration option.

---

## 40. Resource enforcement

### 40.1 The declared budget becomes the cgroup limit

A package declares `ram_mb`, `vram_mb`, `cpu_cores` and `disk_mb` (§8.1), and those exact numbers
become its cgroup v2 limits. This is what makes the declaration mean something: a package that
under-declares to get admitted is killed at the number it declared, on the robot, in front of the
person who installed it — rather than being admitted on a fiction and discovered later by
exhausting a machine that is holding sixty kilograms.

| Control | Setting | Why |
|---|---|---|
| `memory.max` | Declared `ram_mb` | **`max`, not `high`.** A policy thrashing at the high watermark misses its deadline every cycle while looking alive. A hard kill and a restart is a cleaner failure, and the kernel holds throughout. |
| `cpu.max` | Declared `cpu_cores` | Quota, not shares — shares are relative and give no guarantee under load |
| `pids.max` | Small, fixed | A fork bomb in a policy is a denial of service against the robot |
| `io.max` | Class-based | Telemetry upload must not starve the black box writer |
| `cpuset.cpus` | **Never the RT core** | §40.2 |

### 40.2 The RT core is not shareable

The core running the kernel and the RHAL is isolated at boot (`isolcpus`, `nohz_full`) and no
package's cpuset may include it. This is not a tunable. A policy that could be scheduled onto the
core running the safety kernel could delay the safety kernel, and the entire real-time argument
in §23.5 would be void.

### 40.3 Accelerators

Where the GPU supports partitioning (MIG) or arbitration (MPS), `vram_mb` maps onto it. Where it
does not — which is most embedded accelerators — **one policy holds the accelerator at a time**,
and the second is refused at admission with that as the stated reason. Time-slicing an accelerator
between two policies produces two policies that both miss their declared inference budgets, which
is worse than one that runs and one that is honestly refused.

### 40.4 On breach

OOM kill, restart with backoff (§33.5), a black box event carrying the declared and observed
figures. Repeated breach trips the crash-loop threshold and the package is marked `failed` — which
is a more useful signal to a publisher than a robot that limps.

---

## 41. Logs and diagnostics

### 41.1 The black box is not a log file

Two streams that must never be conflated:

| | Black box | Logs |
|---|---|---|
| Format | MCAP, structured, schema'd | journald, human-readable text |
| Content | Every control cycle (§26) | Whatever the code chose to print |
| Retention | Preallocated ring, sealed on fault | Rotated by size and age |
| Purpose | Reconstructing what the robot did | Understanding what the software thought |
| Guarantee | Complete, or the run is not conformant | Best effort |

Debugging by grepping the black box is a category error, and letting ordinary logging write into it
would break the completeness guarantee that `telemetry.blackbox.complete` tests. They stay
separate.

### 41.2 Remote access

Off by default. When enabled it is **time-boxed, operator-approved on the robot, and audit-logged
at both ends**, and it expires on its own rather than on somebody remembering. A permanently
enabled remote shell on a fleet of robots is a fleet-wide vulnerability with a friendly name.

### 41.3 `skynet doctor`

One command producing one bundle: versions and edition, image slot and generation lag, active and
unavailable guards with their missing inputs, resolved limits and provenance, package list with
budgets versus observed usage, the last N black box events, connectivity and queue depth, thermal
and battery history, and the last reconcile plan including anything deferred and why.

It is the first thing to ask for in a support conversation, and it exists so that the answer to
"what is wrong with this robot" is a file rather than an interview.

---

## 42. Amendment to §31.3 — edition levels

Part III presented nine language editions as though they were peers. They are not, and the table
understated the cost of the ones with a kernel by roughly an order of magnitude. The contracts
alone are 2,430 lines measured; a full edition — kernel, `skynetd`, policy runtime, CLI, SDK — is
closer to 40,000–60,000, and Part IV is a large part of why.

So "edition" decomposes into levels, and an implementation declares which one it reaches:

| Level | Scope | Rough size | Realistic count |
|---|---|---|---|
| **L1 — Bindings / SDK** | Write drivers and policies, call the local API, against an OS already running | ~2–3k | **All of them.** Cheap, and where nearly all demand actually is. |
| **L2 — Contracts** | Transport, admission, certification and readiness computed natively, corpus-verified | ~2.4k | **3–4** |
| **L3 — Runtime** | `skynetd` and the policy runtime — everything in Part IV | ~35k | **1**, for a long time |
| **L4 — Kernel** | The Safety Kernel | ~2k lines, disproportionate assurance cost | **2 at most** |

The correction that matters for planning: **almost everyone asking for their language wants L1.**
A ROS 2 shop wants to write a driver and a policy in C++. They do not want to maintain a C++
implementation of OTA rollback, and offering them one as though it were the same request would
commit both sides to work neither wants.

The tier ceilings in §31.3 are unchanged and apply to L3 and L4 only — a Go L1 binding is entirely
unremarkable and carries no ceiling, because it is not hosting anything.

A second L4 edition roughly doubles the certification bill and should be driven by a named
customer's safety case rather than by preference.

---

## 43. What Part IV leaves open

| Open | Why |
|---|---|
| **Multiple robots on one compute** | One Jetson driving two arms is common in cells and genuinely awkward: two RHAL drivers, two kernels or one, two black boxes, and a shared accelerator. Deliberately deferred rather than half-designed. |
| **Safe-state discovery** | §36.2 assumes the robot knows where a safe state is. For an arm that is trivially true; for a mobile robot it is a navigation problem, and the reserve budget depends on solving it. |
| **Package-level A/B for drivers** | Drivers do not auto-restart (§33.5), so a bad driver update needs a person. Whether drivers should get their own A/B slot is unresolved. |
| **Console over the internet** | §39.2 forbids it, which is right for security and wrong for a robot in a customer's building with nobody on site. The answer is probably a Hub-brokered relay the robot dials out to, and it has not been designed. |
| **Fleet-wide config convergence** | Profiles layer cleanly on one robot (§38.2). Rolling a fleet profile across a hundred robots with staged rings and automatic halt is the same problem as OTA and should probably share its machinery. |

---
---

# Part V — The hardware-facing layer

A second audit, prompted by asking whether the plan was complete, found that it was not. The
platform layer — supervision, packaging, updates, fleet, security, conformance, editions — was
thorough. The layer between the RHAL and the policy had holes, and three of them changed what the
guards in §7.1 can actually promise:

* `calibration` appeared **zero** times in the document.
* `transform`, `tf2` and "coordinate frame" appeared **zero** times, while the `workspace` guard
  checks poses that only mean something in a frame.
* All sixteen occurrences of "watchdog" were software. The hardware watchdog — the thing that
  catches the entire application processor wedging — was absent, while §7.4's certified-tier
  argument quietly leaned on it.

This part closes those, then the five functional and commercial gaps behind them.

---

## 44. Coordinate frames

### 44.1 The problem the guards had

`workspace` checks "commanded end-effector pose against operator-defined allowed volumes" and
`self_collision` checks "commanded configuration against the body's collision model". Both operate
on poses, and a pose without a frame is a number. Nothing in Parts I–IV said who resolves
`head.camera_rgb` into the base frame, or where the end effector *is* given a joint vector.

### 44.2 Two graphs, not one

The important decision, and it is a split rather than a system:

| | **Safety frame set** | **Perception tree** |
|---|---|---|
| Who computes it | The Safety Kernel, in-cycle | The policy runtime |
| From what | Joint state, by forward kinematics | Everything, including estimated and filtered poses |
| Contents | Link poses, end effectors, collision bodies, declared workspace volumes | The above plus map, odometry, detected objects, sensor frames |
| Rate | Every control cycle | Whatever it manages |
| Staleness | Zero by construction — computed from the state it just read | Bounded at best |
| Delivery | None; it never leaves the kernel | Published, lossy, subscribable |

**The kernel computes its own forward kinematics and subscribes to nothing.** This is the whole
point. A transform tree distributed over a message bus — ROS `tf2` being the canonical example —
has unbounded staleness and no delivery guarantee, and a `workspace` guard evaluating a 200 ms old
transform is not a degraded guard, it is a confidently wrong one. The kernel already holds the
joint vector it just read; FK over a fixed chain is a few hundred flops and is deterministic.

The perception tree is allowed to be lossy precisely because nothing safety-critical reads it.

### 44.3 The frame graph in the descriptor

The embodiment descriptor gains a `frames` block: a tree of named frames, each with a parent and
either a **static** transform or a declaration that it is **dynamic** (derived from a named joint).

```
base_link                              root, by convention
├── torso.link              dynamic ← torso.yaw
│   ├── left_arm.link0      dynamic ← left_arm.shoulder.pitch
│   │   └── … left_arm.tool0
│   └── head.link           dynamic ← neck.pitch
│       └── head.camera_rgb static, from calibration (§45)
└── base.footprint          static
```

Static transforms have two sources, and the distinction matters: those from the **descriptor** are
nominal design values signed by the vendor, and those from **calibration** are measured values for
this individual machine. Where both exist the calibrated one wins, and the frame records which it
used — a workspace guard running on nominal extrinsics when a calibration exists is a guard using
the wrong number, and one running on *expired* calibration is worse.

**The shape, pinned.** Two generators emit this block — `skynet descriptor from-urdf` and the MuJoCo
driver — and the kernel will read it. So the field names belong to the specification rather than to
whichever tool happened to write one first. A second edition emitting a different spelling of the
same tree is a divergence that no test catches, precisely because nothing consumes `frames` yet and
there is therefore nothing to fail.

```json
"frames": {
  "root": "base_link",
  "convention": "rep103",
  "generator": "urdf" | "mjcf" | "vendor" | null,
  "nodes": [
    {
      "name": "left_arm.link0",
      "parent": "torso.link",
      "kind": "root" | "static" | "dynamic",
      "translation": [x, y, z],
      "rotation": [x, y, z, w],
      "source": "descriptor" | "calibration",
      "joint": "left_arm.shoulder.pitch",
      "joint_type": "revolute" | "continuous" | "prismatic" | "ball" | "floating",
      "axis": [x, y, z],
      "mimics": "left_hand.finger.flex#0",
      "mimic_multiplier": -1,
      "mimic_offset": 0,
      "collision": true,
      "collision_shape": {
        "kind": "capsule",
        "a": [0, 0, 0],
        "b": [0, 0, -0.28],
        "radius_m": 0.06
      },
      "collision_ignore": ["torso.link"],
      "mass_kg": 1.42
    }
  ]
}
```

| Field | Rule |
|---|---|
| `root` | Names the node whose `parent` is null. `base_link` by convention (§44.4), not by requirement. |
| `convention` | `rep103` today. The field exists so a body that is *not* REP-103 has to say so rather than be silently misread. |
| `generator` | What produced the tree. Provenance, never a licence to read the other fields differently. |
| `translation`, `rotation` | The transform from the parent frame: metres, and a quaternion ordered `xyzw` (§44.4). Both absent on the root, which has nothing to be relative to. |
| `source` | Which of the two sources above this transform came from. Absent on the root. |
| `joint`, `joint_type`, `axis` | Present exactly when `kind` is `dynamic`. The axis is expressed in this frame, before the joint is applied. |
| `mimics` | The joint this one follows, where it is a linkage rather than a degree of freedom. Optional. |
| `mimic_multiplier`, `mimic_offset` | `value = multiplier × driver + offset`, URDF's semantics. Absent means 1 and 0. |
| `collision` | Whether the frame carries collision geometry **at all**. |
| `collision_shape` | That geometry as one sphere-swept segment, where it is one the kernel can evaluate. Nullable. |
| `collision_ignore` | Frames this one is permitted to touch. Symmetric: written once, meant both ways. |
| `mass_kg` | What the geometry declares, for anyone estimating dynamics. Nullable — a frame with no geometry has no mass. |

**`mimics` carries a ratio, and the reason is a gripper.** The commonest linkage in robotics is a
parallel gripper whose second finger runs at −1, and a field that could only record "follows" would
place that finger on the wrong side of the object — inside the guard whose entire job is knowing
where things are. URDF has carried the multiplier and the offset since the beginning; this now
carries them too.

**`collision` and `collision_shape` are two different facts and neither implies the other.** A
vendor's model is usually meshes, and no honest reduction of a mesh to a primitive exists without
reading the mesh. So a frame may say "there is collision geometry here" and offer no shape, and the
`self_collision` guard then reports itself unavailable naming those frames — rather than testing a
body with limbs quietly missing from it. A shape without `collision: true` is two statements that
disagree and is refused.

**The shape is one primitive: a sphere-swept segment.** It is a sphere when `a` equals `b`, it
bounds a cylinder exactly and a link's real geometry conservatively, and the distance between two of
them is a closed form with no branching beyond four clamps. Conservative is the only direction of
error a collision bound may have — the shape contains what it stands for, so the guard trips early
rather than late. A guard built on meshes would need a broadphase, a solver and an allocator, none
of which belong inside a control cycle.

**`collision_ignore` is not an optimisation.** Every real body has pairs that overlap by design — a
humanoid's pelvis and torso — and URDF cannot say so, which is why SRDF's `disable_collisions`
exists. Without the field the guard trips on the robot standing still, and a guard that does that is
one an operator switches off: the failure §7.1 spends a paragraph avoiding. Adjacent frames, a
parent and its child, are excluded without being declared, because they touch at the joint by
construction.

**A node carries at most one joint.** That is what keeps forward kinematics a walk rather than a
solver, and it is also a real restriction: URDF cannot express anything else, MJCF can, and a body
with two joints on one link therefore has no representable frame graph. Such a body emits
`frames: null` rather than a tree with a hinge missing from it, and §44.5 already says what that
means — the pose guards report unavailable with the reason named. Whether a node should instead
carry an ordered *list* of joints is open, and it is a change to every consumer of a field no
consumer reads yet.

**Absent, null and empty are three different bodies.** Absent means the vendor's schema predates the
field. `null` means the generator could not express this body's tree and said so. An empty `nodes`
array is a body with no frames at all, which is a claim nobody should be making. All three leave the
pose guards unavailable; only the middle one names a reason.

### 44.4 Conventions, stated because a sign error here moves a robot the wrong way

Right-handed throughout. **REP-103 axes**: x forward, y left, z up. Metres, radians, seconds, and
kilograms. Quaternions ordered `xyzw`. Rotations are active, applied to points, not frames.
`base_link` is the root and is defined by the vendor.

These match ROS's conventions deliberately: a bridge to an existing ROS stack should be mechanical
rather than a source of transposed axes, and the largest population of existing robot code assumes
exactly these.

### 44.5 What becomes possible

`workspace` and `self_collision` move from "specified but underspecified" to implementable, and
both gain an honest failure mode: a body whose descriptor declares no frame graph, or whose graph
does not reach the end effector, reports those guards **unavailable** with the missing frame named
— rather than evaluating them against a frame it guessed.

"Does not reach" is a **static** property and is answered when the lease is granted, not once per
cycle. A frame below a ball or floating joint can never be placed from one scalar per joint, so the
guard reports it unavailable then and there. The alternative — an active guard that holds the robot
on every cycle for the life of the lease — is the same outcome with the reason removed.

Three limits, stated here rather than discovered:

**A volume is in the robot's own root frame, and for a mobile base that is not the cell.** §44.2
gives the kernel a frame set rooted at `base_link` and no world pose; localisation lives in the
perception tree, which the kernel deliberately does not subscribe to. For a fixed-base arm the
distinction is empty and an operator's "stay inside this region" means what they think it means. For
a mobile base the volume travels with the robot. Fixing this needs a pose the kernel would have to
trust from outside itself, which is the thing §44.2 exists to refuse; a cell boundary for a mobile
base is a different mechanism and is not designed here.

**The commanded configuration exists only on a position-commanded body.** §7.1 specifies both guards
one step ahead, and on a body commanded in velocity or torque there is no next configuration to
compute — so what runs is a check of where the robot is now. That is a real check and a weaker
claim, and the verdict carries which of the two was performed rather than letting a log imply the
stronger one.

**The cost is quadratic and it is the first part of this kernel where that matters.** Forty collision
bodies is 780 pairs and roughly fifty thousand floating-point operations per cycle: comfortable on
an application processor, and not obviously comfortable on a safety MCU without double-precision
hardware, where each of those is a software routine. The number is reported rather than assumed —
`skynet frames` prints it — and §7.4's certified tier has to answer it before mirroring this guard
onto a second processor.

---

## 45. Calibration

### 45.1 Why this is a safety subsystem and not a setup chore

Calibration is the only state in the system that is **safety-relevant, individual to one machine,
and drifts over time**. The descriptor is signed and static. Profiles are signed and operator-set.
Calibration is neither: it is measured, it decays, and several guards are built on it.

A camera extrinsic off by four centimetres means the `proximity` guard measures separation to the
wrong place while reporting perfectly healthy. That is the failure this section exists to prevent,
and it is invisible without an expiry policy.

### 45.2 What is calibrated, and what it feeds

| Parameter set | Drifts with | Feeds | Guard affected |
|---|---|---|---|
| Joint zero offsets | Re-homing, mechanical work, collisions | Forward kinematics, every pose | `workspace`, `self_collision` |
| Camera intrinsics | Rarely; lens changes | Projection | `proximity` (indirectly) |
| Camera extrinsics | Vibration, thermal cycling, knocks | Sensor → base transform | `proximity`, `workspace` |
| Force/torque bias | **Temperature**, continuously | Force readings | `envelope` in torque mode, force control |
| IMU bias | Temperature, ageing | Attitude, centre of pressure | `balance` |
| Tool / TCP offset | Every tool change | End-effector pose | `workspace` |
| Gravity vector | Base tilt | Gravity compensation | `balance`, torque envelope |

That table is the deliverable. It converts "we should calibrate the cameras sometime" into "an
expired extrinsic disables the proximity guard", which is a statement an operator can act on.

### 45.3 The record

Each parameter set is stored as a signed record in `/data/state/calibration/`, carrying:

* the **values**, and the **method** that produced them (which procedure, which target, which pose set);
* a **residual** — reprojection error, fit RMS, whatever the method yields;
* **conditions**: timestamp, temperature, tool fitted, payload;
* the **operator** who ran it;
* a **validity policy** (§45.4).

Signed for the same reason profiles are: it can reach the kernel. And provenance is mandatory for
the same reason `limits.source` is — a calibration whose method nobody recorded is a rumour with
six decimal places.

**The residual is thresholded, not decorative.** A hand-eye calibration with twelve millimetres of
reprojection error is a failed calibration, and accepting it silently produces a workspace guard
that is precisely twelve millimetres wrong forever.

### 45.4 Validity is a policy, not a date

A calibration expires on whichever comes first:

| Condition | Typical for |
|---|---|
| Elapsed time | Everything; the backstop |
| Joint travel since calibration | Joint zeros, tool offsets |
| Temperature delta from calibration conditions | F/T bias, IMU bias |
| **An event** | A detected collision, an estop under load, a tool change, a driver replacement, a mainboard swap |

The event triggers are the ones that matter. A robot that has just hit something has, by
definition, possibly moved its cameras, and continuing to trust yesterday's extrinsics is exactly
the wrong response to an impact. So a collision invalidates the calibrations the impact could
plausibly have disturbed, and the affected guards go unavailable until re-measured.

### 45.5 Expiry degrades; it never silently continues

An expired calibration follows the rule already established in §7.2: **the guards that depend on it
report unavailable, naming the expired parameter set.** Not "assume it is probably still fine",
which is what every system without an expiry policy does by omission.

The robot keeps running. It runs with fewer guards, says so on every surface, and the operator
decides whether that is acceptable for the task — which is the same bargain the tier model offers
everywhere else.

### 45.6 Procedures ship as packages

A calibration *procedure* is a package of kind `service` with a `calibration` role: it holds the
capability lease, moves the robot through a pose set under a restricted envelope, computes the
parameters, and writes a record. The *results* are state, not a package.

This means a vendor can ship an improved hand-eye routine without an OS release, and a third party
can publish a better one — which is the marketplace working as intended on a problem that is
usually solved with a PDF and a spreadsheet.

### 45.7 Conformance additions

* `calibration.provenance` — every stored record carries a method and a residual, or it is refused.
* `calibration.expiry_enforced` — a deliberately expired record disables its dependent guards, and
  the guards report the parameter set by name.
* `calibration.event_invalidation` — a simulated collision invalidates the affected sets.

These are **one corpus**, not three, and the reason is worth writing down: they are one question
asked of one input — *given these records and this moment, what is believed and which guards lose
their input*. Split, an edition could pass `expiry_enforced` while disagreeing about which guard the
expired set feeds, and the guard is the half that matters to whoever is standing next to the robot.

### 45.8 Five states, and the two absences that are not the same absence

A record is `valid`, `failed`, `expired`, `absent` or `unusable`, and the last two are the ones that
took a session to separate.

| State | Meaning | Whose job |
|---|---|---|
| `valid` | Believed, and the report says **what was actually checked** | — |
| `failed` | The residual exceeds the method's own threshold (§45.3). It was never a calibration, so it does not become one at any age and does not "expire" | Whoever ran it |
| `expired` | It was one, and §45.4 has since caught up with it | Schedule a re-measurement |
| `absent` | Nobody ever measured this set on this robot | Schedule a first measurement |
| `unusable` | A record exists and cannot be judged — no method, no residual, or filed under one set and declaring another | Somebody has to go and look |

`valid` carries the list of conditions it was tested against, and that list is **empty for a record
with no policy**. A console must be able to tell *still valid* from *nothing was checked*, and a
state alone cannot: a record that expires on nothing is not evidence of anything.

`unusable` is separate from `failed` for the reason `untrusted` is separate from `failed` in §37.1 —
nothing says the measurement is bad, and nothing says it is good either. A record filed as
`camera_extrinsics` that declares itself `joint_zeros` is the sharpest case: whichever half is wrong,
some set is being judged by another set's policy, and the guards that lose their input are the wrong
guards.

### 45.9 A missing reading expires; it does not extend

The rule that keeps an expiry policy from switching itself off. If a policy names a temperature delta
and the robot cannot say what temperature it is, the record **expires**, with the reason naming the
missing input.

The tempting alternative — skip the condition you cannot evaluate — inverts the policy on exactly the
robot it was written for: a machine whose thermal sensor has failed is precisely the machine whose
force/torque bias should not be trusted.

This has a loud consequence in this edition and it is the correct one. §47's sensor pipeline and the
policy runtime are unwritten, so nothing on a robot produces a temperature or a cumulative travel
count, and every record with one of those policies reports expired on day one, naming the reading
nothing supplies. That is the honest state of the machine. Every surface that shows it also carries
the sentence that keeps it from being read as a to-do list: **every guard here is already unavailable
for a prior reason** — §27's kernel is unwritten, so nothing evaluates a cycle at all — and what
calibration reports is what would *still* be missing once it is.

### 45.10 Which sets apply, and the base kind that decides

A robot is judged only on the parameter sets its own body can hold, derived from the capability
tokens and the base kind the descriptor already carries rather than from a new field.

Two absences, treated differently on purpose:

* **Undeclared capabilities** mean nobody said what this body senses, so every sensing-derived set is
  included. Over-reporting what is missing is the conservative direction.
* **A declared `fixed` base is not an absence at all.** It is a positive statement that this body
  cannot fall over, so the IMU bias and gravity vector are not measurements it is missing — they are
  measurements it will never take. §45.2 says the gravity vector drifts with *base tilt*, and a fixed
  base does not tilt. Listing `balance` unavailable on a bolted-down arm is a permanent red mark
  nobody can ever clear, which is how an operator learns to ignore the panel.

An *unknown* base is mobile, because that is the first kind of absence again, going the other way. A
fixed body that does declare an IMU keeps both sets, since it evidently has one.

---

## 46. Watchdogs, all four of them

### 46.1 The chain

Each layer catches a failure the one above it cannot see:

| # | Watchdog | Catches | Response | Tier |
|---|---|---|---|---|
| 1 | Kernel `watchdog` guard (§7.1) | The policy stopped producing commands | `stop_cat2` | All |
| 2 | systemd `WatchdogSec` on `skynetd` | The supervisor's reconcile loop hung | Restart `skynetd` | All |
| 3 | **SoC hardware WDT** | The application processor wedged entirely | **Reset the SoC** | Host, Certified |
| 4 | **Safety MCU independent timer** | The AP wedged *and* the reset did not recover it | **Cut drive enable** | Certified |

Layer 3 is petted by the **Safety Kernel**, not by `skynetd`. That is what makes it meaningful: the
WDT is satisfied only while the thing that actually enforces the envelope is running its cycle. A
watchdog petted by a supervisor tells you the supervisor is alive, which is not the question.

### 46.2 A reset is not a stop, and this has consequences

The uncomfortable part. When the SoC resets, the drives lose their command source — and what they
then do is a property of the **hardware**, not of anything in this document. Some hold. Some engage
brakes. Some fault and disable. **Some coast.**

So the descriptor gains a required field:

```
on_command_loss:  "hold" | "brake" | "fault" | "coast"
```

And it produces a finding that was not visible before Part V:

> **A robot whose drives coast on command loss cannot safely reach the Host tier**, because the
> hardware watchdog's own recovery path — reset the processor — is, on that machine, a fall.

For such a body the only sound configuration is the Certified tier, where layer 4 cuts drive enable
independently of the processor being reset. This is a genuine constraint that falls out of taking
the watchdog chain seriously, and it belongs in the compatibility report rather than in an
integration engineer's memory.

`on_command_loss` is measured by the bench suite, not trusted: `safety.command_loss.behaviour`
stops commands and observes what the joint actually does.

### 46.3 Window nesting

The windows must nest, and getting the order wrong makes an upper layer unreachable:

```
kernel cycle worst case  <  envelope.watchdog_ms  <  SoC WDT window  <  drive command timeout
```

If the SoC WDT is shorter than the kernel's own watchdog window, the processor resets before the
kernel ever gets to execute a controlled stop — the harsher recovery pre-empts the gentler one.
If it is longer than the drive's timeout, the drive faults first and the WDT never matters.

The nesting is computed at boot from the descriptor and the resolved limits, and a body whose
declared figures cannot be nested is reported at startup rather than discovered in the field.

#### Where the four windows come from

| Window | Declared by |
|---|---|
| `kernel_cycle_worst_case_ms` | The kernel, which measures its own worst case |
| `envelope_watchdog_ms` | `envelope.watchdog_ms` on the descriptor |
| `soc_wdt_ms` | `watchdog.soc_wdt_ms` on the descriptor — a fact about the board |
| `drive_command_timeout_ms` | `watchdog.drive_command_timeout_ms` — a fact about the drive |

The last two are on the descriptor because they are the only place they can come from: nothing in
this operating system can read a hardware watchdog's window off a board.

**An undeclared window is not a passing one.** It is reported as missing and the walk *steps over*
it — the pair on either side of a gap still inverts, and that inversion is real whatever sits
between them. So the report carries two booleans rather than one: `ordered` means nothing
contradicts, `complete` means every window is declared as well. A single boolean would have to lie
about one of them, and the lie it tells — "sound" over a chain nobody has measured — is the one this
section exists to prevent.

**The comparison is strict.** Two watchdogs with the same window is a race whose winner is decided
by interrupt latency, and a chain whose order depends on which timer fires first is not a chain.

**Adjacent pairs only.** Comparing every pair would report one inversion up to three times and bury
the one an operator has to fix; the transitive consequences follow from the adjacent ones anyway.

#### The refusal is host-shaped, and that is unusual

§46.2 refuses **the host tier** for a coasting body, and the shape is worth stating because a tier
ladder normally caps downward:

* **Guest** is fine — enforcement is advisory and nothing relies on a hardware watchdog.
* **Host** is the configuration where layer 3 is the last line and its recovery is resetting the
  processor, which on a coasting body is a fall.
* **Certified** is fine — layer 4 cuts drive enable independently of the processor being reset.

The middle rung is the unsafe one. Rewriting that rule as a ceiling would silently permit exactly
the configuration it exists to refuse.

**Silence is not `hold`.** A body that has declared no `on_command_loss` is refused host for the
same reason a coasting one is: the recovery path has not been characterised, and treating an
absence as the most convenient value is inventing an answer to the only question layer 3 turns on.
A value outside the vocabulary gets its own sentence — *"this body says `freewheel`"* and *"this
body says nothing"* are different facts with different remedies.

#### What the chain reports about *itself*

A page listing four layers over a machine that has one of them reads as four running watchdogs. So
every surface reports the chain **and** which layers exist here: layer 1 unavailable while §27's
kernel is unwritten, layer 2 armed, layers 3 and 4 unobserved because nothing in this edition can
see a hardware timer.

---

## 47. The sensor and media pipeline

### 47.1 What §25.1 assumed

The observation assembler takes streams with acquisition timestamps and builds a frame at a target
instant. It never said where those streams come from, how they are buffered, or who timestamps
them — which is most of the work.

### 47.2 Capture

| Backend | For |
|---|---|
| V4L2 | USB and MIPI cameras — the common case |
| GenICam / GigE Vision | Industrial cameras |
| Vendor SDK | RealSense, ZED, and depth cameras with proprietary pipelines |
| RHAL `read_sensor` | Anything only the driver can reach — joint-mounted tactile, in-hand sensors on a proprietary bus |

**Timestamps come from the sensor, not from arrival.** V4L2 offers
`V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC` and it is used; GenICam offers a device timestamp and it is
used. Where a backend genuinely cannot supply one, the pipeline records an *estimated* latency and
the frame carries a larger uncertainty, which flows into the skew the assembler already reports
(§25.1). Estimating is acceptable. Silently substituting arrival time is not, because it folds a
variable queue delay into a number the policy treats as ground truth.

### 47.3 Zero copy

Capture buffers are `dmabuf` handles passed by reference from the capture device to the GPU and to
the hardware encoder. Three 1080p30 streams copied through userspace is roughly a core of memcpy
and several milliseconds of added latency, spent achieving nothing.

### 47.4 Two consumers, and they must not share a queue

The design decision in this section:

| Consumer | Wants | Gets |
|---|---|---|
| **Policy** | The freshest frame; an old frame is worthless | A **latest-wins slot**. New frames overwrite unconsumed ones. |
| **Black box** | Every frame; completeness is the guarantee | A **bounded queue** feeding the hardware encoder |

A single queue serving both is the classic error, and it fails in the direction nobody wants: the
policy ends up reading queued frames, so a momentary encoder stall becomes permanent added latency
in the control path. A queue in front of a policy is latency wearing the costume of reliability.

Backpressure is resolved separately for each. If the encoder falls behind, recording frames are
dropped **and the drop is recorded** — a gap in the black box that is annotated as a gap is
honest; a gap that looks like a quiet period is not. Capture is never stalled, because stalling
capture starves the policy to protect the recording, which is precisely backwards.

### 47.5 Accounting

Capture buffers are OS-owned rather than charged to a package's `vram_mb`, since several packages
may read one camera. They are bounded, declared per stream, and surfaced in `skynet doctor` — an
unbounded capture pool is a slow memory leak that presents as a policy OOM three hours in.

---

## 48. Homing and power-on self-test

### 48.1 A robot may not know where it is

With incremental encoders — still common on arms — joint positions at power-on are unknown.
Forward kinematics is therefore meaningless, which means **every pose-dependent guard is
unavailable**, which means the robot must not be permitted to run a policy.

The state machine from §27 gains a state before `HOLD`:

```
boot → POST → UNHOMED → HOLD → RUNNING …
                 ↓ (POST failure)
               SAFE
```

### 48.2 Power-on self-test

Before anything moves:

| Check | Failure means |
|---|---|
| RHAL ABI version matches | Wrong driver for this OS |
| `describe()` parses; joint count matches the descriptor | Driver and descriptor disagree about the body |
| Estop asserted and observed | The stop path is not proven. **Nothing runs.** |
| Encoder sanity — plausible values, no NaNs | Sensor or bus fault |
| Calibration expiry evaluated (§45) | Guards that will be unavailable are known *before* start |
| Black box writable, reserve intact | The recorder cannot record |
| Kernel corpus self-check (a sample of §31.4) | The kernel on this machine is not the kernel that was certified |
| Watchdog window nesting valid (§46.3) | A layer of the chain is unreachable |

**A failed POST goes to `SAFE`, not `DEGRADED`.** `SAFE` is sticky and requires a human. A robot
that failed its own self-test should not be talked into running by a reconcile loop, and
`DEGRADED` is a state the system will happily operate in.

The estop check is the one worth insisting on: asserting the stop and *observing* that the RHAL
reported it is the only evidence the stop path works today, as opposed to having worked when it
was installed.

### 48.3 Homing

Homing **moves the robot**, and it is the only motion permitted before `RUNNING`.

* It is a **driver-provided procedure**, invoked via the RHAL extension channel, because only the
  vendor knows the sequence for their hardware.
* It runs under a **restricted homing envelope** declared in the descriptor — typically a small
  fraction of rated velocity — enforced by the kernel like any other envelope. Homing is not an
  exemption from the safety kernel; it is a narrower envelope.
* It requires operator confirmation on any body where the homing sweep can leave the parked volume.
* Completing it writes or confirms the joint-zero calibration record (§45.2), so homing and
  calibration are the same fact rather than two.

A body that needs no homing declares so and the state is skipped. A body that needs it and cannot
complete it stays `UNHOMED`, which is a reported state, not a hang.

### 48.4 Four outcomes, because two of them are not failures

§48.2 gives a check two answers. It needs four, and the three that are not `pass` send a person to
three different places:

| | |
|---|---|
| `pass` | The check ran and this robot is good on this point |
| `fail` | The check ran and it is not. §48.2's `SAFE`: sticky, needs a human |
| `unavailable` | The **input does not exist**. Nothing is broken and nothing was established |
| `elsewhere` | This check is **not `skynetd`'s to run** — see below |

`may_run` is true only when **every** check passed. An unavailable check has established nothing and
a check nobody could run has not been run, so neither is a licence — and the verdict for that state
is `post_incomplete` rather than `safe`, because a robot whose checks could not run does not need a
human. It needs the rest of the operating system.

Collapsing `fail` and `unavailable` would produce the two worst sentences available: *"self-test
passed"* on a robot that tested almost nothing, or *"self-test failed"* on a robot with nothing
wrong with it.

**An expired calibration does not fail POST.** §45.5 degrades rather than refuses, and a self-test
that refused to boot on a stale camera extrinsic would be a robot nobody could get into a state
where they could recalibrate it. What the row does is make the list of guards that will start
unavailable known *at power-on* rather than at the first command, which is what §48.2 asks of it.

**An unreachable watchdog layer does fail it.** §46.3's inversions mean a layer of the chain cannot
fire, and a robot booting past that is exactly what "before anything moves" is for. An *undeclared*
window does not fail: nothing contradicts, and a body nobody has measured is not a body that is
wrong.

### 48.5 Half the table is not the supervisor's to answer

This falls out of §33.1 and it is worth stating rather than discovering. `skynetd` never touches an
actuator — that single property licenses everything else about the daemon, and CI asserts it cannot
even take a dependency on the RHAL. Four of §48.2's eight checks need precisely what that denies:

| Check | Owner |
|---|---|
| RHAL ABI version matches | The kernel, which loads the driver |
| `describe()` parses; joint count matches | The kernel |
| Estop asserted **and observed** | The kernel, through the RHAL |
| Encoder sanity | The kernel, through the RHAL |
| Kernel corpus self-check | The kernel |
| Calibration expiry (§45) | **`skynetd`** |
| Black box writable, reserve intact | **`skynetd`** |
| Watchdog window nesting (§46.3) | **`skynetd`** |

So the supervisor runs three and reports the other five as `elsewhere` — *not* as unavailable, which
would be a different and wrong sentence. A POST that silently omitted them would be a supervisor
claiming to have proved a stop path it is forbidden from touching, and one that called them
unavailable would suggest they might become available here. They will not.

The homing procedure itself (§48.3) is in the same position: it moves the robot, through the RHAL
extension channel, which is the one thing this process may not do.

---

## 49. Audio

Lower priority than everything above it, and included because one part of it is safety-relevant.

### 49.1 The warning path belongs to the OS

Standards for robots operating around people expect an audible indication before autonomous
motion. That makes the warning tone a **system function, not an application feature**: `skynetd`
owns it, it sounds on transition into `RUNNING` and on resumption after a stop, and **a package
cannot suppress it.** A policy that could silence its own warning would be a policy that could
remove a control the operator's risk assessment relied on.

Packages may use the speaker for anything else only with an explicit `hri.speech` grant, and that
grant never includes the warning channel.

### 49.2 Microphones are a privacy surface

`sensing.audio` is a capability token gated like any other, and it is one of the tokens an operator
profile is most likely to deny outright. A microphone on a robot in a workplace records people who
did not consent to a robot being there, let alone to it listening, so the default is denied and the
grant is explicit and audit-logged.

The genuinely useful non-HRI application is anomaly detection — a failing bearing sounds different
long before it fails — and that runs equally well on a spectrum with speech discarded at the
source, which is the configuration to prefer.

---

## 50. Factory provisioning

### 50.1 Pairing assumes things a factory does not have

§10 pairs a robot to a namespace with an operator, a Hub and a network. A robot on a production
line has none of them, and it needs an identity before it has an owner.

### 50.2 The birth certificate

At manufacture:

1. A device keypair is generated **inside the secure element** and the private key never leaves it.
2. The vendor signs a **birth certificate** over the public key, binding: model, serial number, the
   embodiment descriptor digest, hardware revision, manufacture date, and the factory calibration
   records.
3. The robot ships with the certificate and **no namespace**. It belongs to nobody yet.

At the customer, first boot: the birth certificate proves **what it is**; pairing (§10) binds it to
**whose it is**. Two separate facts, established by two separate parties, which is correct — the
vendor is the only party who can attest to the first and has no business asserting the second.

### 50.3 What this closes

Without it, "this is a Unitree G1 with these joint ratings" is a claim the robot makes about
itself, and the entire safety envelope rests on believing it. The vendor key is registered with the
Hub using the existing Ed25519 machinery, so a robot presenting an unsigned or unknown descriptor
is visible as exactly that.

It also gives the factory calibration a provenance chain: the extrinsics measured on the line are
signed by the party that measured them.

### 50.4 Repair

The identity is bound to the secure element, so replacing a mainboard produces a robot whose
birth certificate no longer matches its hardware. That is the correct behaviour and not a bug —
it is the same property that makes a phone's secure enclave meaningful. Re-issue is a
vendor-signed operation, recorded, and it invalidates the calibrations a board swap could have
disturbed (§45.4).

---

## 51. Offline entitlement

### 51.1 One principle decides the whole design

> **A licence check must never stop a running robot.**

A billing system that can halt a humanoid mid-task has invented a safety hazard to protect
revenue. Everything below follows from refusing that.

### 51.2 Checked at install, not at run

Entitlement is a signed, time-limited token delivered with the desired state and cached on disk. It
is verified when a package is **installed or upgraded** — never in the loop, never at start, never
periodically against a server the robot may not be able to reach.

| Situation | Behaviour |
|---|---|
| Valid entitlement | Install, upgrade and run |
| Expired, inside grace | Runs. Upgrades allowed. Reported as lapsed. |
| Expired, past grace | **Keeps running.** New installs and upgrades of that package refused. |
| Revoked | **Keeps running.** Reinstall refused, incident raised. |

Grace is deliberately generous — ninety days by default — because the failure it guards against
(a robot in a basement losing a feature because a token expired) is worse than the leakage it
permits.

### 51.3 Enforcement is commercial, not physical

A revoked entitlement blocks distribution, not operation. The lever against genuine abuse is the
Hub: no new installs, no upgrades, no support, and a record. Not a robot that stops moving in
somebody's warehouse because a payment failed.

This is a narrower enforcement posture than most commercial software takes, and it is the correct
one for software that moves mass.

---

## 52. Descriptor additions from Parts IV and V

The embodiment descriptor accumulated fields across these two parts. Collected, because they are a
schema change vendors must implement and the bench suite must measure:

| Field | From | Measured by |
|---|---|---|
| `capabilities`, `extensions` — what this body provides (§5.0, §8) | Part I | `embodiment.capabilities.declared` |
| `frames` — the frame graph (§44.3) | Part V | `embodiment.frames.complete` |
| `frames[].collision_shape`, `collision_ignore` (§44.3) | Part V | `embodiment.frames.complete` |
| `frames[].mimic_multiplier`, `mimic_offset` (§44.3) | Part V | `embodiment.frames.complete` |
| `simulator` — the body is simulated (§35.3, §70) | Part IV | `embodiment.descriptor.valid` |
| `on_command_loss` — hold / brake / fault / coast (§46.2) | Part V | `safety.command_loss.behaviour` |
| `homing` — required, procedure token, restricted envelope (§48.3) | Part V | `safety.homing.envelope` |
| `power.time_to_safe_state_s`, `power.safe_stop_energy_wh` (§36.2) | Part IV | `power.safe_state.reachable` |
| `power.pack_capacity_wh`, `power.thermal_throttle_c`, `power.thermal_critical_c` (§36.2.1, §36.3) | Part IV | `power.safe_state.reachable` |
| `power.brownout_fault_codes` — the vendor's own codes that mean sag (§36.4) | Part IV | *unmeasured* — see §36.4 |
| Per-sensor `latency_ms` (§25.2, §47.2) | Parts II, V | `rhal.sensor.latency` |
| `command_latency_ms` (§5.2) | Part I | `rhal.command.latency` |

Every one is nullable, and every one absent produces a named unavailability rather than an
assumption — the rule from §7.2 applied consistently, which is what keeps the descriptor honest as
it grows.

`capabilities` and `simulator` are the two exceptions, and each for its own reason.

`capabilities` absent means **the empty set** — §5.0 explains why the usual rule cannot apply to a
list admission has to compare against.

`simulator` absent means `false`. Made nullable it
would produce "nobody said whether this robot is real", which is a question about a physical machine
that the machine's own existence answers. The obligation is asymmetric because the risk is —
a simulator presented as a robot is the failure; a robot presented as a robot is not.

---

## 53. What Part V leaves open

| Open | Why |
|---|---|
| **Multi-camera calibration at fleet scale** | Calibrating one robot is a solved procedure. Keeping two hundred robots' extrinsics valid, detecting drift from field data rather than from a schedule, and deciding which need a technician is a data problem nobody has designed here. |
| **Automatic drift detection** | §45.4 expires calibration on time, travel, temperature and events. Detecting that an extrinsic has drifted *from the images themselves* would be far better and is a research problem, not a design one. |
| **Audio beyond warning and anomaly** | Speech interaction, source localisation and noise-robust command recognition are an application domain, and treating them as one is the honest scope. |
| **`on_command_loss` for mixed drivetrains** | A humanoid may brake on the legs and coast on the arms. The field is currently one value per body and probably needs to be per chain. |
| **Entitlement for the OS itself** | §51 covers packages. Whether a Skynet Services subscription lapsing should change anything about a running robot is a commercial decision, and the answer implied by §51.1 is "no". |

---
---

# Part VI — Skills, environments and on-robot evaluation

A third audit, prompted by asking whether the OS is compatible with the Hub, found the largest
remaining gap by counting how the document treats each of the Hub's five repository kinds:

| Repo kind | Mentions in Parts I–V | State |
|---|---|---|
| `model` | 18 | Designed throughout — it is what transport, packages and the kernel are built around |
| `dataset` | 6 | Designed via episodes and the black box |
| `benchmark` | 2 | Named in a comparison table |
| `skill` | **2** | **Both occurrences are the word in a list** |
| `environment` | **0** | **Absent** |

That is a compatibility gap, not an omission of detail. The Hub *sells* skills; `SkillManifest`
declares `preconditions` as "preconditions that must hold before the skill is safe to run", and
nothing in the operating system consumed them. §21 promised "Runs evaluations on the robot" and
nothing designed it.

---

## 54. Skills

### 54.1 A skill is not a policy

The distinction the OS had been eliding:

| | **Policy** | **Skill** |
|---|---|---|
| Shape | Observations → actions, continuously | Invoked with arguments, terminates |
| Duration | As long as it holds the lease | Bounded, declared |
| Outcome | None; it just runs | Succeeded, failed or aborted |
| Guards | Envelope applies | Envelope applies, **plus preconditions** |
| Composition | Runs alone | May wrap a policy, and may call other skills |

`StandardRobotInterface.execute({ skill, arguments, timeout_s })` already existed on the Hub side
as the invocation point. Nothing routed it.

### 54.2 Preconditions become machine-evaluable, and the OS evaluates them

The decision that matters most in this part.

`preconditions` in `SkillManifest` is a list of strings — human prose describing what must hold
before the skill is safe. Prose is documentation, and documentation does not stop a robot. So
preconditions become **predicates**, and they are evaluated by the OS before the skill is allowed
to start.

This needs no new machinery: the Hub already has a pure predicate engine at
`lib/skynet/compatibility/predicate.ts` with `and / or / not / eq / gte / lte / includes /
includes_all / exists / semver_satisfies`. The compatibility engine evaluates predicates over a
static context assembled from manifests; the skill runtime evaluates the *same predicate shape*
over a **live** context:

```
robot.state.*        joint positions, gripper, battery percent, base pose
robot.guards.active  which guards are actually running (§7.2)
robot.calibration.*  which parameter sets are valid, and their age (§45)
robot.frames.*       whether the frames the skill needs resolve (§44)
robot.tier           guest | host | certified
skill.grants.*       capability tokens the operator granted this package
env.*                operator-declared facts about the cell — fixture present, door state
```

Three rules:

* **A precondition that cannot be evaluated refuses the skill**, naming the path that was
  unresolvable. Not "assume true", which is what free-text preconditions do by construction.
* **Preconditions are re-checked, not just entry-checked.** A subset marked `hold` in the manifest
  is evaluated every cycle; battery dropping below reserve mid-skill should stop the skill, not be
  discovered at the end.
* **The operator may add preconditions; the skill may not remove them.** Same narrowing-only rule
  as profiles (§38.2). A site can require "door closed" on a skill whose publisher never imagined
  that cell.

`safety_notes` stops being decorative too: it is displayed at grant time and acknowledged once per
skill per robot, recorded in the audit log. A safety note nobody has read is a safety note that
does not exist.

Two refinements from building it, both of which the plain reading permits and neither of which it
implies:

* **The acknowledgement is bound to the digest of the notes**, not to the package name. A publisher
  who edits a warning has written a new warning, and the alternative lets a package acquire its
  notes *after* an operator agreed to a blank one — which is the version of this rule that would be
  worth nothing.
* **The refusal carries the notes.** *Displayed* is the word the rule uses, and the only place the
  robot can be sure a person sees them is in the answer to the request they just made. A refusal
  that named the rule and not the text would send an operator to look them up on a Hub the robot may
  not be able to reach.

It gates a **widening** and nothing else. Revocation, stopping and every other narrowing path is
untouched, for §39.1's reason: the safe direction never waits.

### 54.3 Success is judged by the OS, not by the skill

`success_criteria` is evaluated the same way, over post-state, by the runtime rather than by the
skill's own code. **A skill that judges its own success is a skill that always succeeds** — and on
a platform where success rates feed a marketplace, self-reported outcomes are worth nothing.

Where a criterion is not machine-evaluable (many genuinely are not — "the cup is upright" needs
perception), the outcome is `unlabeled` rather than `success`, which is exactly the vocabulary the
episode schema already carries. Honest ignorance beats an invented pass.

### 54.4 Lifecycle

```
idle → preconditions → acquiring → running → verifying → succeeded | failed | aborted
          ↓ refused        ↓ denied     ↓ timeout / guard trip / precondition lost
```

| Stage | What happens |
|---|---|
| `preconditions` | Predicates evaluated. A failure names the predicate and refuses. |
| `acquiring` | Capability leases taken for `required_capabilities` (§25.2). Denied if held. |
| `running` | The skill's policy runs under the transport plan and the resolved clamps, exactly like any other policy — a skill is not an exemption from the kernel |
| `verifying` | `success_criteria` evaluated over post-state |
| terminal | Leases released, outcome recorded, episode emitted |

**Timeout is enforced by the OS.** `expected_duration_seconds` from the manifest, times a declared
overrun factor, and past it the skill is stopped under a controlled stop and recorded as `aborted`
— matching the Hub's `EpisodeOutcome` vocabulary, so an aborted skill becomes a labelled training
example rather than a gap.

Every transition is a black box event, and one episode is emitted per invocation, carrying the
skill reference, arguments, outcome, duration and intervention count. Skills therefore generate
training data as a side effect of being used, which is the flywheel the Hub was built for.

**The document is `POST /episodes`'s, not one of the robot's own** — see §76, which was written
after the two halves were compared for the first time and did not agree about a single field name.

### 54.5 Composition

`SkillManifest.composition` names a model, dataset, environment and benchmark. On the OS only the
model matters at runtime: it is the policy the skill runs, resolved at install, transport-planned
against this body, and refused if the plan is refused. A skill whose model cannot reach the body is
a skill that cannot be installed, and finding that out at install is the point.

Skill-to-skill calls are permitted with three limits, because a composition system without them
becomes a way to exhaust a robot:

* **Depth-limited** (default 4) and **acyclic** — a cycle is refused at install by walking the
  declared graph, not discovered at runtime. Both halves of that sentence are load-bearing and the
  second is the one that was missing for six sessions: the runtime validated the **live ancestry**
  and refused the call that would close a cycle, which is correct and is exactly the discovery this
  rule forbids being the only one — it fires on a robot, with leases held and a deadline running.
  The declared graph is walked at install, reported to whoever ran the command, and enforced at the
  symlink swap where a staged blob becomes usable (§37.1).
* **Leases are held by the root invocation**, not re-acquired per call. Nested acquisition is how
  you get a skill deadlocking against its own child.
* **The root's timeout bounds the whole tree.** A child cannot extend its parent's budget.

### 54.6 Conformance additions

* `skill.preconditions.enforced` — a skill whose precondition is false is refused, with the
  predicate named.
* `skill.preconditions.rechecked` — a `hold` precondition falsified mid-run stops the skill.
* `skill.timeout.enforced` — an overrunning skill is stopped and recorded `aborted`.
* `skill.success.independent` — a skill claiming success against a false criterion is recorded as
  failed.

---

## 55. Environments, and what they are on a real robot

### 55.1 An environment is a simulation artifact

`EnvironmentManifest` carries simulators, a scene, assets, tasks, randomisation, `physics_hz` and
`render_hz`. All of it describes a simulated world.

So the rule is simple and worth stating because the alternative is confusing: **environments are
loaded only by a simulator RHAL driver.** Installed onto a robot whose driver is physical, an
environment package is inert, and the OS says so rather than silently accepting it. Its
`required_capabilities` are still checked at admission, since an environment that needs a gripper
is not runnable by a robot without one in sim either.

### 55.2 The physical counterpart is the cell, and it is not a repo kind

The real-world analogue of an environment — where the fixtures are, which volumes are forbidden,
where the safe parking pose is, what the operator has declared about the space — is a **cell
descriptor**, and it belongs in the operator profile (§38), not in the repository catalogue.

The reason is ownership. An environment is published, versioned and shared; a cell is
site-specific, changes when somebody moves a table, and is nobody's intellectual property. Making
it a repo kind would invite people to publish their factory layout to a marketplace, and would put
safety-relevant workspace volumes behind a fork-and-star workflow.

The cell descriptor supplies `env.*` to the precondition context (§54.2) and the allowed volumes to
the `workspace` guard, and is signed like every other safety-relevant profile.

---

## 56. On-robot evaluation

### 56.1 The problem that decides the design

§21 promises the OS "runs evaluations on the robot and in the sim driver", and the Hub publishes
leaderboards from the results. **A leaderboard fed by self-reported on-robot numbers is trivially
gameable** — a publisher evaluating their own policy on their own robot, choosing their own seeds,
and reporting their own score is not producing evidence.

So an on-robot evaluation result is only worth as much as its attestation.

### 56.2 Signed results, and a provenance class

A run produces a bundle the **device** signs with its secure-element key (§10, §50):

```
benchmark digest · policy digest · embodiment digest · OS image digest
seeds used (supplied by the benchmark, never chosen by the robot)
per-episode outcomes and metrics
black box segment hash
tier and enforcement mode at run time
simulator flag (§35.3)
```

The Hub verifies the signature against the registered device and assigns a **provenance class**,
which is what a leaderboard filters on:

| Class | Means | Leaderboard |
|---|---|---|
| `field` | A real robot, valid birth certificate, attested image, non-simulator driver | Eligible |
| `simulated` | A simulator RHAL driver, which must declare itself | Eligible, separately ranked |
| `unattested` | No device signature, or a device the Hub does not know | **Accepted and never ranked** |

`unattested` is accepted rather than refused because a researcher on a bench robot has a legitimate
result worth recording — and it is never ranked, because it is not evidence.

### 56.3 Seeds come from the benchmark

`BenchmarkManifest.seeds` is supplied to the robot, never generated by it. A robot that picks its
own seeds picks easy ones, and it need not do so dishonestly — an autocorrelated PRNG and a
convenient starting pose is enough.

### 56.4 The black box is what makes it checkable

The bundle carries a hash of the black box segment covering the run, and the segment is uploadable
on request. A disputed result is therefore **replayable**: the same policy digest, embodiment
digest and recorded observation stream must reproduce the same actions
(`telemetry.replay.deterministic`, §15).

Nobody in robotics can currently do this, and it is the strongest argument for the whole black box
subsystem existing. A benchmark number with a replayable recording behind it is a different class
of claim from a number in a table.

### 56.5 Scoring is computed twice

Roll-up per `BenchmarkManifest.scoring` runs on the robot — so an offline robot sees its own score —
and again on the Hub from the raw per-episode outcomes. **The Hub's number is authoritative.** They
must agree, and a disagreement is a defect worth investigating rather than a rounding difference to
paper over; it means the shared contract implementations diverged, which is precisely what the
conformance corpus (§31.4) exists to catch.

### 56.6 What the Hub does with the class, and the axis it did *not* replace

§56.2 says the Hub assigns the class, and that makes `provenance` the one shared contract where the
Hub is the **consumer** rather than a second opinion — the robot's edition exists so a robot can
predict its own class offline. That inversion argues for sharing the code more strongly, not less:
two implementations of an anti-gaming rule that disagree is an anti-gaming rule with a hole, and a
hole in this one always points the same way, towards admitting something to a leaderboard.

Building it produced one finding worth recording, because it looked like a reconciliation and was
not. `skynet_evaluations` already carried two things that resemble a provenance class:

```
verified   boolean                                — a registered platform worker measured it
source     platform | self_reported | imported    — who submitted it
provenance field | simulated | unattested         — what it is evidence of          (new)
```

The first two are about the **submission**. §56.2's classes are about the **run**. A platform worker
can measure a simulated run, and a robot can produce an embodied one nobody verified — so these are
two axes, and collapsing them would have destroyed the distinction §56 exists to draw. `provenance`
is therefore a third column beside them rather than a replacement for either.

#### NULL is not `unattested`

The sharpest line in the schema. `unattested` is a **verdict**: §56.2 reached it by looking at a run
bundle and finding nothing to attribute. NULL is the absence of a verdict — no bundle was ever
submitted, so §56 has not classified the run and cannot.

Every result on the platform when this landed is in the second state, and backfilling them to
`unattested` would have been the platform asserting a §56 verdict about runs it holds no bundle for.
That is §15's *"absence is not a pass"* pointed the other way: absence is not a failure either.

#### The class is part of the leaderboard key

Not a badge on a row. `unattested` never reaches a board at all — *"accepted and never ranked,
because it is not evidence"* — and `simulated` and `field` are keyed separately, so one policy on
one robot can hold a record on each. Collapsing them would let a simulated number displace an
embodied one, which is the single outcome §56 exists to prevent.

#### An eighth domain tag, and the broadest collision surface on the platform

A run bundle binds a **policy digest, an embodiment descriptor digest and an OS image digest** —
three hex strings each already signed under one of the existing tags. A bundle is therefore the one
document here that contains other documents' subjects wholesale, and without `skynet.bundle.v1` a
device signature over a descriptor could be presented as a device signature over the run that names
it. §56.2 would then class as `field` a result nobody attested.

#### Every fact the class turns on comes from the Hub

A bundle arrives carrying claims about itself, and four of the facts §56.2 decides on are ones the
Hub already holds:

| Fact | Where the Hub gets it |
|---|---|
| `device_signature` | Decided here, and only against **this device's own key** |
| `device_known` | Whether the second credential resolved to a device row |
| `birth_attested` | §50's certificate, recomputed against what the Hub observes of the machine |
| `image_attested` | §10's check-in attestation |

`simulator` is the one claim taken at face value, and §52 already settled why: the obligation to
declare sits on the simulator driver. A robot that lies the other way demotes its own result.

`birth_attested` is the input that did not exist until §50 was built. Before it, the Hub could not
evaluate §56.2's second condition at all — and a `field` class would have been three conditions
wearing the name of four.

---

---

## 57. The Hub surface, revised

§17 listed eight new endpoints. Parts IV–VI added requirements, and most of them fold into surface
that already exists rather than extending it — which is the useful finding:

| Need | Resolution |
|---|---|
| Transport plan | New — `GET /repos/{ns}/{name}/versions/{v}/transport` |
| Embodiment descriptor | New — `GET|POST /robots/{slug}/embodiment` |
| Desired state and check-in | New — `GET /devices/{id}/desired`, `POST /devices/{id}/checkin` |
| Incidents | New — `POST /devices/{id}/incidents` |
| Conformance and certification | New — `POST /devices/{id}/conformance`, `GET /certifications/{slug}` |
| OS releases | New — `GET /os/releases` |
| **Calibration records** (§45) | New — `GET|POST /devices/{id}/calibration` |
| **Vendor keys for birth certificates** (§50) | **Existing** — `POST /namespaces/{handle}/keys` already registers Ed25519 public keys |
| **Birth certificate** (§50) | New — `POST /robots/{slug}/attestations` |
| **Entitlement tokens** (§51) | **Existing** — a field in the desired-state document, not an endpoint |
| **POST results** (§48.2) | **Existing** — part of the check-in payload |
| **Skill invocation outcomes** (§54.4) | **Existing** — `POST /episodes`, unchanged |
| **Attested evaluation results** (§56) | **Existing shape** — `POST /evaluations` with a device credential and a signature field. Built: the device credential arrives in `X-Skynet-Device` beside the submitter's own, because the two answer different questions (§56.6) |
| **Telemetry** (§34.4) | New — `POST /devices/{id}/telemetry`. **Added by §74**; §34.4 gave the class a priority, a drop policy and a counter, and neither this table nor §17 gave it a route |
| **OS bundle bytes** (§12) | New — `GET /os/releases/{version}/bundle`. **Added by §75**; §12 gates applying an image and neither this table nor §17 gave a route to fetch one, so a robot could admit a release it had no way to obtain |

Eleven new endpoints in total, four things that need only a new field, and no breaking change to any
of the sixty routes that exist. The reason the delta is this small is that the OS was designed
against the Hub's object model from the start rather than beside it.

New tables follow the `skynet_*` convention: `skynet_embodiments`, `skynet_device_state`,
`skynet_calibration_records`, `skynet_conformance_results`, `skynet_certifications`,
`skynet_incidents`, `skynet_os_releases`, `skynet_os_editions`, `skynet_attestations`.

### 57.1 All of them exist

Twelve endpoints, not ten: the count above was written before §74 gave telemetry a route and §75
gave the bundle one, and the table's own last rows say so. Written across six sessions on the Hub
side.

Recorded here rather than only in a build log, because the useful fact about this table is no
longer what it asks for — it is **which of the eleven carry evidence and which carry assertions**,
and that distinction is invisible from a list of routes that all return 200.

| Endpoint | Evidence, or assertion |
|---|---|
| `GET /repos/…/transport` | Computed from two signed documents. Evidence. |
| `GET\|POST /robots/{slug}/embodiment` | Signature state decided against a registered key. Evidence when `verified`, and it says which. |
| `GET /devices/{id}/desired` | The Hub's own intent. Not a claim about a robot at all. |
| `POST /devices/{id}/checkin` | The robot's word, and §17 forbids the Hub restating it as its own. |
| `POST /devices/{id}/incidents` | The robot's word. The sealed recording behind it does not cross (§13). |
| `POST /devices/{id}/conformance` | **Assertion.** §57 calls these signed and no runner on any robot signs one. |
| `GET /certifications/{slug}` | Recomputed from the above, so it inherits the above. |
| `GET /os/releases` | Signed bundles, admitted by the mirrored contract. Evidence. |
| `GET\|POST /devices/{id}/calibration` | Half. Age and events are in the record; joint travel and temperature are properties of the robot at the moment of asking, and the answer names which rules it could not check. |
| `POST /devices/{id}/telemetry` | Counted windows, with the denominator being what was **observed**. Evidence about the interval it covers and nothing about the intervals it does not. |
| `POST /robots/{slug}/attestations` | Evidence, and the only one on this list where a *vendor* is the attesting party. |
| `GET /os/releases/{version}/bundle` | Bytes, and the only row on this list that is neither. What makes them evidence is the digest in the signed manifest they are checked against — which is why the route needs no signature of its own (§75.1). |

Two rows are the ones to read together. Conformance is asserted and certification is
computed from it, which means §12's gate — the one deciding whether an OS update may change the
safety kernel — currently reads evidence nobody signed. A birth certificate is the first document
on the platform that is signed by the party who actually knows, and it closes the smaller half of
that gap: it establishes **what the machine is**, which is the input every other measurement is
about. It does not establish that the measurements were taken.

---

## 58. Compatibility, answered

### 58.1 With the Hub

| Hub concept | OS treatment |
|---|---|
| `model` | Loaded via the load plan, transported onto the body, run under the kernel |
| `dataset` | Produced — episodes from skills and teleoperation, in LeRobotDataset form (§16) |
| `skill` | Installed as a `.sky` package; preconditions and success criteria enforced by the OS (§54) |
| `environment` | Loaded only by a simulator driver; the physical counterpart is the cell descriptor (§55) |
| `benchmark` | Run on the robot, results signed and provenance-classed (§56) |
| Artifacts, digests, signatures, scan verdicts | Reused unchanged — the OS adds no parallel format |
| Devices, episodes, deployments, check-in, rollback | Reused unchanged |
| Compatibility engine | Reused, and its predicate engine is reused again for skill preconditions |
| Namespaces, teams, org membership, audit | Reused unchanged |

### 58.2 With the robot

The best-covered axis, and the one with the fewest open questions: the RHAL C ABI and its structs
(§4, §24), the embodiment descriptor including frames, envelope, `on_command_loss` and homing
(§5, §44, §46, §48, §52), capability tokens and extensions, transport (§6), the tier model (§1),
and a conformance suite that measures rather than trusts (§15).

### 58.3 What is still not designed

| | Status |
|---|---|
| Multi-robot on one compute | Open (§43) |
| Safe-state discovery for mobile robots | Open (§43) |
| `on_command_loss` per chain rather than per body | Open (§53) |
| Fleet-scale calibration drift detection | Open (§53) |
| Perception as an OS service — detection, segmentation, pose estimation | **Deliberately out of scope.** These are packages, not operating system. The OS supplies frames, calibration, synchronised observations and capability grants; what a policy does with them is the policy's business. |
| Motion planning as an OS service | **Deliberately out of scope**, for the same reason. IK solvers are declared by the body because the kernel needs them for retargeting and the `workspace` guard; a full planner is a package. |

The last two are worth stating explicitly rather than leaving as absences. An operating system that
also shipped perception and planning would be a robotics *framework*, and the market already has
several. The line Skynet OS draws is: **the OS owns everything that must be correct for the robot
to be safe, and nothing that is merely useful.**

---
---

# Part VII — Limits

Everything above describes what the system does when it works. This part is the honest answer to
two questions a buyer will ask, and the answer to both is qualified rather than yes.

§1 already refuses the strong claim — "there is no binary that boots on a Unitree G1, an ABB arm,
a Roomba and a warehouse AMR" — but the word *universal* invites the strong reading anyway, and a
document that only refuses it once, six hundred lines from the end, has not really refused it.

---

## 59. Will a robot work once this is installed?

**No, not on its own, and not on every robot.** Six reasons, in descending order of how often they
will bite.

### 59.1 Nothing runs without an RHAL driver for that specific robot

The universality claim is precisely: *any robot for which somebody has written a conformant
driver*. That is a much weaker sentence than "any robot", and it is the true one. A driver needs
the vendor's protocol, their timing behaviour and usually their SDK. For a closed robot with no
documented interface — most consumer robots, plenty of industrial ones — no driver can be written
without reverse engineering, and Skynet OS does nothing for it at all.

### 59.2 A large class of robots cannot be commanded at joint level, and the design assumes they can

The gap this part was written to record. "Joint" appears forty-three times in this document.
`URScript`, `KRL`, `RAPID`, `Fanuc`, `KUKA` and "teach pendant" appear zero times. The whole design
assumes it can send joint positions, velocities or torques at 50–1000 Hz.

**Most of the world's installed industrial robots do not offer that.** Behind a Fanuc, KUKA, ABB or
older Universal Robots controller you upload a *program* — the controller does the interpolation,
owns the servo loop, and enforces its own safety. For those machines:

* **Transport has almost nothing to map.** There are no joint channels to bind; the action space is
  a set of named motions with parameters.
* **The Safety Kernel cannot clamp**, because it never sees the commands. The vendor controller
  generates them internally, and it is the safety-rated component — often genuinely certified,
  which Skynet is not.
* Only the `execute({ skill, arguments })` path applies. Skills, packaging, grants, fleet
  reconciliation, OTA and the black box all still work; per-joint envelope enforcement does not.

These robots are **Guest tier permanently**, and not because of a missing feature — because the
architecture that makes them safe is somebody else's and is already there. That is a legitimate
and useful product (fleet management, packaging, provenance, an audit trail for a cell that has
none), and it is not what the rest of this document describes.

### 59.3 Most robots have no sourced safety envelope

The design refuses to invent limits, correctly. The consequence is that a robot whose vendor has
never published joint torque ratings cannot reach Host tier, and lands at Guest with advisory
enforcement. Honest, and not the same thing as safe.

### 59.4 Transport makes a policy *execute*, not *succeed*

A `retargeted` verdict means the commands are geometrically valid and inside the envelope. It says
nothing about whether the policy still does the task. Retargeting fidelity is listed as open in
§53 and it is the widest gap between "runs" and "works" — a policy transported onto a body with
different link lengths may execute flawlessly and fail every attempt.

### 59.5 None of it is built

This is a design with its contracts implemented: roughly 2,430 lines of TypeScript covering
transport, admission, certification and readiness, plus the RHAL headers. The Safety Kernel,
`skynetd`, the policy runtime, every driver, OTA, the console and the conformance corpus are zero
lines. The realistic figure is several engineer-years before a robot moves under this.

### 59.6 Commissioning is human work

Homing, calibration, the cell descriptor, workspace volumes, operator profiles, capability grants.
Per robot, per site, by a person. "Upload the OS" is not a step that exists at Host tier — that
tier is a vendor porting project — and at Guest tier the upload is the easy part.

### 59.7 What actually works, by robot class

| Robot class | Best tier | What Skynet OS delivers |
|---|---|---|
| Open real-time interface — Franka FCI, UR RTDE, `ros2_control`, Unitree SDK, most LeRobot-supported arms | Host; Certified with a safety MCU | Everything in this document |
| High-level API only — Spot SDK, Fanuc/KUKA/ABB controllers, most cobots in program mode | **Guest, permanently** | Skills, packaging, grants, fleet, OTA, black box. **No transport, no clamping.** |
| No documented interface — consumer appliances, closed platforms | None | Nothing, absent reverse engineering |
| No onboard compute | Guest, from external compute | Observation and skill dispatch over the link, at link latency |

---

## 60. Compatibility with existing robot software

### 60.1 Where it is genuinely free

* **MCAP** (§26) is already the ROS 2 default bag format, so Foxglove opens a sealed black box with
  no exporter and no conversion.
* **REP-103 frame conventions** (§44.4) were adopted so a bridge to existing ROS code is mechanical
  rather than a source of transposed axes.
* **ROS 2 as an RHAL transport** — a `ros2_control` bridge driver is a very achievable first driver.
* **LeRobot** (§16) — its `Robot` class is close enough to the RHAL that wrapping it is a small job
  and brings a large fleet of supported arms with it.
* **LeRobotDataset** — episodes emit in a format the existing training ecosystem already reads.

### 60.2 The conflict, stated plainly

> **At Host tier, Skynet OS replaces the thing it is said to be compatible with.**

`ros2_control` and the Safety Kernel both want to be the component that writes to actuators. They
cannot both be. At Host tier ROS 2 becomes a **client** of Skynet — a source of perception and
planning that posts desired commands — rather than a peer, and porting an existing ROS 2
application means every node that publishes to a controller must be restructured to go through the
kernel. That is not a bridge. It is a rewrite of the application's control layer, and anyone
evaluating adoption should be told so in the first conversation rather than the third month.

Two smaller frictions in the same direction:

* **Discovery.** ROS 2's DDS is multicast, peer-to-peer, with inbound connections everywhere.
  §34.2 is outbound-only. Running both means the ROS side keeps its own network posture, and the
  security property Skynet claims is true only of Skynet's own traffic.
* **Timing.** Most ROS 2 stacks, Isaac ROS included, are not hard real-time. That is fine here —
  they sit above the kernel as the policy runtime, which is exactly where §23.3 puts inference —
  but it means "we run ROS 2" and "we are real-time" remain two separate claims after adoption,
  just as they were before.

### 60.3 The honest one-line statement

**Skynet OS is compatible with the existing ecosystem as a consumer of its drivers and a producer
of its data formats. It is not compatible with it as a peer at the control layer, because that
layer is the one it exists to own.** At Guest tier it coexists with anything. At Host tier it
supplants what was there.

Every stack that has tried to own the control layer has faced this, and the ones that pretended
otherwise were discovered to have pretended during integration. Saying it here costs a little
enthusiasm early and saves a great deal of trust later.

---

## 61. What would have to be true for the strong claim to hold

Not a roadmap — a list of what "any robot works" would actually require, so the distance is
visible:

| Requirement | Today |
|---|---|
| A conformant driver for the robot | One per robot, written by somebody. The LeRobot bridge is the cheapest way to many at once. |
| A signed embodiment descriptor with sourced limits | Vendors do not publish these. SCTS is the lever; it has no adoption yet. |
| Joint-level command access at rate | Unavailable on a large installed base (§59.2), and not fixable in software |
| A policy that transfers to that body | An open research problem (§53), not an engineering one |
| Commissioning | Human, per robot, per site |
| The OS itself | Several engineer-years, unstarted |

The first, second and fifth are work. The third is a property of hardware somebody else shipped.
The fourth is a research result nobody has yet. A design that claimed to have solved all six would
be the kind of document this project exists to be an alternative to.

---
---

# Part VIII — Drivers and the developer path

Two corrections and one gap, all found by asking who actually writes the drivers and how hard the
first day is.

The correction: Parts I–VII consistently implied that **vendors** write RHAL drivers, by analogy
with Android's HAL. That understates what is achievable badly, and if believed it turns adoption
into a waiting game for vendor goodwill. Most of the drivers that matter can be written by whoever
is building this, from public documentation.

The gap: §18 "Developer experience" is 185 words, and `driver template`, `mock robot`,
`driver SDK` and `conformance harness` appear zero times in the document. The single largest
practical barrier to this platform existing is that writing the first driver looks hard, and
nothing was designed to make it easy.

---

## 62. Who writes the drivers

### 62.1 Four generic drivers cover a large fraction of everything

The leverage is in drivers that are not robot-specific at all:

| Driver | Covers | Effort |
|---|---|---|
| **LeRobot bridge** | SO-100/101, Koch, LeKiwi, HopeJR, Reachy 2, Unitree G1, plus community packages for xArm, UR5e, Franka and AgileX Piper | Small — its `Robot` class is nearly the RHAL already (§16) |
| **`ros2_control` bridge** | Every robot with a `hardware_interface` plugin, which is most of the ROS-supported industrial and research fleet | Medium — one adapter over the resource manager |
| **EtherCAT (SOEM)** | CiA402 servo drives directly, which is most industrial servo hardware | Medium |
| **CANopen / CiA402** | The same device class over a different bus, common on smaller robots | Medium |

Four drivers, and the addressable set is already large. This is the same shape as a printer driver
model where a handful of page-description languages cover most of the market and the long tail is
handled individually.

Simulator drivers — MuJoCo and Isaac Sim — are written regardless, because the sim conformance
suite (§15) cannot run without them.

### 62.2 Vendor SDK drivers are written from public documentation

Unitree SDK2, Boston Dynamics Spot SDK, `libfranka` and the Franka Control Interface, Universal
Robots RTDE — all publicly documented, all usable without a commercial agreement. Each is a
contained project, days to weeks, and each is written by whoever wants that robot supported.

**The vendor only *has* to write the driver when the interface is undocumented and closed.** That
is a real category, and it is the long tail rather than the head.

### 62.3 The honest ladder

```
4 generic drivers        →  a large share of the addressable fleet     (we write these)
~6–10 vendor SDK drivers →  the named robots people actually ask for   (we write these)
long tail                →  vendor or community                        (SCTS is the incentive)
```

Which reframes SCTS: it is not the mechanism that *gets* drivers written, it is the mechanism that
keeps them honest once they exist. Adoption does not wait on vendor goodwill.

---

## 63. Bootstrapping a driver

The path that decides whether this platform has an ecosystem.

### 63.1 Scaffold

```
skynet driver new my-arm --lang rust|c|cpp|python
```

Produces a building, loading, failing-for-the-right-reasons driver: the vtable stubbed, the ABI
version wired, a build file, and a descriptor skeleton. It fails conformance on day one and the
failures are the to-do list.

### 63.2 The mock robot

A simulated body that answers the RHAL, shipped with the SDK. **A developer writes and tests a
driver against it with no hardware at all** — and, more importantly, tests the *failure* paths
that are impossible to provoke safely on a real robot: a stalled command stream, a joint reporting
NaN, an estop during motion, a bus timeout mid-cycle.

Being able to develop the dangerous cases without a robot present is the difference between a
driver that has been exercised and one that has merely been run.

### 63.3 Local conformance before hardware

```
skynet driver check ./my-arm
```

Runs the **sim half of SCTS** against the driver locally — ABI version handling, state
completeness, command rejection, descriptor validity, kernel parity. It reports which tests fail
and why, in the terms of §15, so the bench session is spent on the things only hardware can prove:
estop latency, watchdog behaviour under load, real joint ratings.

Turning up to a bench session with the sim suite already green is what makes bench time cheap.

### 63.4 Generate the descriptor from the URDF

The highest-leverage tool in this part, because **the work is largely already done and sitting in
a file the robot already has.**

```
skynet descriptor from-urdf robot.urdf > embodiment.json
```

A URDF already contains the joint list and types, position, velocity and effort limits, the link
tree, and collision geometry. That is most of an embodiment descriptor:

| Descriptor field | From URDF |
|---|---|
| `joints[].name`, `.type` | Directly |
| `joints[].limits.position_*`, `.velocity_max_rad_s`, `.effort_max_nm` | Directly, **as design values** — see below |
| `frames` (§44.3) | Directly from the link tree |
| Collision model (§29) | Directly |
| `dof` | Counted |
| **`joints[].role`** | **Left blank.** URDF carries no semantics; the tool *suggests* from name heuristics and a human confirms. |
| **`envelope`** | **Left null.** |

The last two rows are the design holding its own line. A URDF `effort` limit is a **design value
from a CAD model**, not a datasheet safety rating, and copying it into `envelope.joint_torque_max_nm`
would auto-populate a safety limit from a number nobody verified — precisely the "invent a
plausible figure" failure that §5 of the safety envelope migration exists to prevent. The
generator therefore emits `envelope: null` with a comment saying where sourced figures must come
from, and the robot renders "no safety envelope declared" until a person supplies one.

Roles are suggested rather than assigned for the same reason at lower stakes: `shoulder_pitch_joint`
is *probably* `left_arm.shoulder.pitch`, and probably is not a basis for cross-body remapping.

### 63.5 Time to each milestone

Targets, stated so they can be measured and missed visibly:

| Milestone | Target |
|---|---|
| Scaffold builds and loads | 5 minutes |
| Descriptor drafted from URDF | 15 minutes |
| Mock robot moving under the driver | 1 hour |
| Sim conformance green | 1 day |
| Real hardware, one joint under the kernel | 1 afternoon |
| Full sim + bench conformance on real hardware | 1 week |

If the first three do not hold, the platform has an adoption problem regardless of how good
anything above this part is.

---

## 64. Errors are a design surface

Every refusal in this system names the fix, and this is already how the implemented contracts
behave rather than an aspiration:

| Refusal | What it says |
|---|---|
| Transport refused | Which channel, its role, and what this body lacks |
| Package refused | **Every** finding at once, not the first — fixing four problems one deploy at a time is how people end up granting everything (§8.2) |
| Guard unavailable | The exact missing input, and "not applicable to this body" where that is the truth (§7.2) |
| Precondition failed | The predicate path that was false or unresolvable (§54.2) |
| Calibration expired | The parameter set, its age, and the guards now unavailable (§45.5) |
| Conformance failed | The test id, the measured value, and the threshold |

The rule behind them: **a refusal that does not tell you what to change is a bug report the user
has to write themselves.**

---

## 65. The golden path

What a developer reads, in order, and nothing else is required to get to a moving robot:

1. **Concepts** — tiers, the kernel invariant, what a driver is. One page.
2. **Write a driver** — scaffold, mock robot, `driver check`. §63.
3. **Describe the body** — URDF import, roles, and where sourced limits come from. §63.4.
4. **Run a policy** — install a package, read the transport plan, understand the clamps.
5. **Go to bench** — what SCTS measures and how to prepare for it.

Everything else in this document is reference. A vendor engineer should reach a moving robot
without reading Parts II through VII, and if they cannot, the fault is in the golden path rather
than in their patience.

---
---

# Part IX — The driver set, and the ecosystem around it

Part VIII established that most drivers can be written without waiting for vendors. This part
designs them, because "we can write a `ros2_control` bridge" is a sentence and not a plan.

Each driver is specified by what it maps onto the RHAL, the part that is genuinely hard, and the
**tier ceiling it can reach** — which is a property of the driver, not only of the robot, and is
derived rather than chosen for the same reason edition ceilings are (§31.3).

---

## 66. The set

| Driver | Reaches | Ceiling | Why the ceiling |
|---|---|---|---|
| **LeRobot bridge** | SO-100/101, Koch, LeKiwi, HopeJR, Reachy 2, Unitree G1, plus community xArm, UR5e, Franka, AgileX Piper | **Guest** | Python. GIL and GC cannot hold an RT path (§31.3). |
| **`ros2_control` bridge** | Every robot with a `hardware_interface` plugin | **Host** | Runs inside the controller manager's RT loop, on PREEMPT_RT |
| **EtherCAT / CiA402** | Industrial servo drives directly | **Certified** | Distributed clocks, and STO over FSoE is a stop path off the application processor |
| **CANopen / CiA402** | The same drives on a slower bus | **Host** | Bus bandwidth caps the joint count × rate product |
| **MuJoCo** | Simulation | **Guest**, flagged simulated | §35.3 forces it |
| **Isaac Sim** | Simulation | **Guest**, flagged simulated | §35.3 forces it |
| **Unitree SDK2** | G1, H1, Go2 | **Host** | Low-level mode gives joint access at rate |
| **`libfranka` / FCI** | Franka Research 3 | **Host** | 1 kHz torque control; among the best fits available |
| **UR RTDE** | UR3/5/10/16e | **Host** | 500 Hz servo interface |
| **Spot SDK** | Spot | **Guest** | gRPC, command-level; §59.2 applies |

Two of these — the `ros2_control` bridge and EtherCAT — are worth more than the rest combined,
because neither is robot-specific.

---

## 67. The LeRobot bridge

### 67.1 Mapping

LeRobot's `Robot` is nearly the RHAL already:

| RHAL | LeRobot |
|---|---|
| `open(config)` | `robot.connect()` |
| `read_state` | `robot.get_observation()` — proprioception split from camera frames |
| `write_command` | `robot.send_action()` |
| `close` | `robot.disconnect()` |
| `describe` | Built from `observation_features` / `action_features` plus the motor bus config |
| `read_sensor` | The camera entries of `get_observation()` |
| `emergency_stop` | Torque-disable on the motor bus |

`get_observation()` returns one dict mixing joint positions and camera frames; the bridge splits it,
routing proprioception to `rhal_state` and images to `read_sensor`, and timestamps at read since
LeRobot does not supply acquisition times (§47.2 — the estimated-latency path, reported honestly).

### 67.2 Roles map cleanly, which is the point

SO-100/101 motor names line up with the canonical vocabulary almost one to one:

```
shoulder_pan   → right_arm.shoulder.yaw      wrist_flex  → right_arm.wrist.flex
shoulder_lift  → right_arm.shoulder.pitch    wrist_roll  → right_arm.wrist.roll
elbow_flex     → right_arm.elbow.flex        gripper     → right_hand.gripper.grip
```

A hand-written mapping table per LeRobot robot class is a few dozen lines and unlocks
cross-embodiment transport for the whole fleet. This is the cheapest large win available.

### 67.3 The ceiling, stated plainly

Python caps this at **Guest**. The bridge cannot host an enforcing kernel and should never claim
to. What it still delivers is substantial and is most of why anyone would adopt early: the package
system, skills with enforced preconditions, capability grants, fleet reconciliation, the black box,
episode capture in LeRobotDataset form, and advisory violation reporting that tells an operator
what a policy *would* have commanded before it is trusted with anything.

A Rust or C++ reimplementation reaching Host tier is possible for specific LeRobot-supported arms
by talking to their motor buses directly — Feetech and Dynamixel protocols are documented — and is
a per-robot project rather than a bridge.

---

## 68. The `ros2_control` bridge

### 68.1 It is a controller, not a node

The decision that makes this driver viable.

The obvious implementation — a ROS 2 node subscribing to joint states and publishing commands — is
wrong. It puts DDS in the control path, inherits discovery latency and jitter, and gives up any
real-time guarantee, which caps the driver at Guest for no good reason.

Instead the bridge is a **`controller_interface::ControllerInterface` plugin**, loaded into the
controller manager like any other controller:

* `state_interface_configuration()` claims the position, velocity and effort state interfaces.
* `command_interface_configuration()` claims the command interfaces **exclusively** — `ros2_control`
  already enforces exclusive claims, so no other controller can write the same joint. That is
  exactly the Safety Kernel's sole-writer invariant (§7), enforced by machinery that already exists.
* `update(time, period)` runs in the controller manager's real-time loop, with direct memory access
  to the interface handles. No messages, no serialisation, no DDS.

### 68.2 Whose clock

The controller manager owns the update rate, not Skynet. This would be a conflict except that
§35.2 already resolved it: **the kernel takes no clock — its tick is supplied by the RHAL.** So the
controller manager's `time` and `period` become the kernel's tick, and everything downstream —
watchdog, rate limits, deadlines — is measured in the loop that is actually running.

The property designed for simulation turns out to be exactly what is needed to sit inside somebody
else's control loop. That is the design paying rent.

### 68.3 Describe from the URDF that is already there

`ros2_control` robots ship a URDF with `<ros2_control>` tags declaring interfaces and limits, so
`describe()` is the §63.4 generator running in-process — joints, types, limits, frames and
collision geometry directly, roles suggested, **envelope null** because a URDF effort limit is a
design value and not a safety rating.

### 68.4 What it does not fix

ROS 2 above the bridge keeps its own network posture (§60.2), and the application's nodes that
previously wrote to controllers must now post to the kernel instead. The bridge makes the *hardware*
reachable; it does not make an existing ROS application Skynet-shaped.

---

## 69. EtherCAT and CANopen — the CiA402 drivers

### 69.1 One profile, two transports

Both speak **CiA402**, the drive profile the industrial servo world standardised on, so the two
drivers share a core and differ in transport.

Modes of operation map directly onto the RHAL:

| CiA402 mode | RHAL |
|---|---|
| 8 — Cyclic Synchronous Position (CSP) | `SKR_MODE_POSITION` |
| 9 — Cyclic Synchronous Velocity (CSV) | `SKR_MODE_VELOCITY` |
| 10 — Cyclic Synchronous Torque (CST) | `SKR_MODE_TORQUE` |

The drive state machine — Switch On Disabled → Ready to Switch On → Switched On → Operation
Enabled, with Fault and Quick Stop — is driven by the driver's `open()` and torn down by `close()`.
`emergency_stop()` triggers Quick Stop, which is a *drive-level* reaction that does not depend on
the application processor still being healthy.

### 69.2 Where `on_command_loss` gets real values

§46.2 introduced `on_command_loss` and noted it decides whether a robot can safely reach Host tier.
CiA402 is where the field stops being abstract: the drive's fault reaction is **configurable**, per
drive, in its object dictionary. The driver reads it during `open()` and reports it in `describe()`
rather than asking a human to type it in — so the descriptor's most safety-relevant field is
populated from the hardware that will actually perform the behaviour.

### 69.3 Distributed clocks

EtherCAT's DC mechanism synchronises slaves to sub-microsecond, which does two things nothing else
in this document can: it gives the observation assembler (§25.1) a genuinely common time base
across joints, and it makes the cycle jitter that `rhal.state.rate` measures a property of the bus
rather than of Linux scheduling.

### 69.4 The route to Certified

EtherCAT is the only driver here with a credible path to the certified tier, because **Safe Torque
Off over FSoE** is a stop path wired into the drive's power stage and reached over a
safety-rated protocol — independent of the application processor by construction, which is exactly
what §1's certified tier requires. Skynet supplies the evidence; the safety case is still somebody
else's to make (`SAFETY_CLAIM_RULE`).

### 69.5 CANopen's ceiling

Same profile, and bandwidth is the limit: a 1 Mbit/s bus carrying PDOs for many joints runs out
before a whole-body humanoid rate is reached. The driver computes the achievable rate from joint
count and PDO size at `open()` and declares it as `control.rate_hz_max`, so the transport planner
clamps the loop honestly (§6.1) rather than a robot discovering it mid-motion.

---

## 70. Simulator drivers

MuJoCo and Isaac Sim, and they are not a lesser path — the sim half of SCTS cannot run without
them, so they are load-bearing infrastructure.

* **They declare `simulator: true`**, which forces the tier to Guest and stamps every check-in and
  every evaluation result (§35.3, §56.2). No simulated run can be presented as bench evidence.
* **They supply the clock**, so a simulation running at a fifth of wall speed or ten times it is
  simply the kernel's time (§35.3).
* **Descriptors come from the scene**: MJCF for MuJoCo, USD for Isaac — the same generator shape as
  §63.4, with the same refusal to populate an envelope from model values.
* They are what makes the **mock robot** of §63.2 more than a stub, and what lets a driver author
  reach green sim conformance before touching hardware.

---

## 71. Vendor SDK drivers

Written from public documentation, each a contained project:

| Driver | Interface | Notes |
|---|---|---|
| **`libfranka` / FCI** | 1 kHz torque and position control over UDP | The best fit available — real torque control, published joint limits, a documented fault reaction. The reference Host-tier driver. |
| **UR RTDE** | 500 Hz servo interface | `servoj` for position streaming. URScript program mode is the §59.2 case and is Guest-only; RTDE is not. |
| **Unitree SDK2** | DDS-based, high-level and low-level modes | Low-level gives joint access at rate. High-level mode is command-level and Guest-only, and the driver must report which mode it opened. |
| **Spot SDK** | gRPC, command-level | Guest tier by §59.2 — the controller owns the servo loop and its own safety. Skills, packaging, fleet and black box all still apply. |

The pattern worth noting: **several of these robots have two interfaces**, one command-level and
one joint-level, reaching different tiers. The driver must report which it opened at check-in, and
`describe()` must reflect that mode — a robot running in high-level mode is a Guest-tier robot
whatever it is capable of in principle.

---

## 72. Repository and ecosystem structure

Recorded here because it was decided in discussion and belongs in the design.

### 72.1 Skynet OS lives in its own repository

Not in the BotMarket repository. Four reasons, one decisive:

1. **Licensing history.** §20 proposes Apache-2.0 for Core. A subdirectory of a private commercial
   repository cannot be cleanly open-sourced later — it needs a history-scrubbing split, or it
   publishes commit history that leaks marketplace internals. Git history is permanent, so the
   licence boundary must exist before the code does.
2. **Vendors must read the RHAL header** to implement against it.
3. **The toolchains do not overlap** — a Rust workspace, C and C++ headers, Yocto layers and
   cross-compilation against a Next.js application's CI.
4. **Release cadence differs** — versioned A/B images against deploy-on-push.

It begins **private and becomes public later**, which is a repository setting rather than a
migration. That sequencing works only if the repository is separate from the outset.

### 72.2 The split

| Repository | Holds |
|---|---|
| **`skynet-os`** (new, private → public) | Spec, RHAL bindings, the conformance corpus, every edition, contracts (Rust plus the WASM build and the TypeScript reference oracle), drivers, CLI, Yocto layers, the on-robot console |
| **`Botmarket`** (existing, private) | Hub API routes, Skynet OS UI pages, migrations, registry readiness — consuming the contracts as a published package |

One repository rather than one per edition: §31.4's differential fuzzing needs every edition
testable against one corpus in one CI run, and splitting them makes that a cross-repository problem
on day one.

### 72.3 Timing

**Before the first Rust lands.** Moving a design document and a few contract files is minutes;
moving that plus a 40,000-line workspace, a Yocto layer and a CI matrix is a week. The C header in
`bindings/` is already vendor-facing code sitting in a private repository — the problem in
miniature, and currently trivial to fix.

---

## 73. What version one covers

> **Amended, and the amendment is the point of the table.** This began as a list of what had been
> *designed*, written when the implementation was 2,430 lines of TypeScript. It went stale in the
> ordinary way — nobody updates a summary while they are building the thing it summarises — and a
> state table that says "Designed" about a subsystem with 45 tests is worse than no table: it is the
> first thing a vendor reads, and it was understating the platform by about eighty thousand lines.
>
> Three states, and the third is the one worth having. **Designed** is prose. **Built** is code with
> a suite. **Built on both sides** means the robot half and the Hub half exist and a document has
> been put through one into the other (§31.6). The last column says what is *not* true yet, per row,
> because a table of green ticks is the failure this whole document is written against.

| Area | Where | State | What is still missing |
|---|---|---|---|
| Thesis, tiers, the honest meaning of universal | §1–3 | Designed | — |
| RHAL — ABI, structs, drivers | §4, §24, §66–71 | **Built** — C, C++ and Python headers; mock, MuJoCo and LeRobot drivers | No vendor has written one |
| Embodiment descriptor and transport | §5–6, §44, §52 | **Built on both sides** — and, since §37.1's `transport` stage, resolved at install on the robot; §6.3's hand-off performed at `GET /limits?package=` | The ceilings reach `resolveLimits()` and stop there: no policy runtime exists to carry a command as far as a guard, so a robot running nothing has no plan in force and `envelope` is active with nothing to clamp (§7.6) |
| Safety Kernel — guards, limits, tiers | §7, §23.1, §25, §46 | **Built on both sides** — `no_std`, no allocator, 101 tests; §7.6's resolution runs on the robot at `GET /limits` and crosses §17 to the device page | Nothing has driven a physical joint, and the tier it resolves at is assumed rather than awarded (§7.6) |
| Packages, permissions, admission | §8, §37 | **Built on both sides** — and §78 closed the case where the gate ran and checked nothing: the Hub refuses the manifest at publish, a robot refuses it at install with `embodiment_undeclared`, and three corpus cases hold both editions to it | Nothing on a robot can *produce* a signature (§19). The two spellings of a package's requirements still both exist; §78.2 is why neither could simply be deleted |
| skynetd — reconciliation, supervision, health | §9, §33 | **Built** — level-triggered, six ordered stages | — |
| Identity, pairing, attestation, factory provisioning | §10, §50 | **Built on both sides** | `security.identity.attested` needs a TPM; no key is generated on a robot |
| Desired state, fleet, OTA | §11–12, §14, §33.3 | **Built on both sides** — including §75's bundle fetch | No robot has yet applied an image from this Hub |
| Black box, telemetry, replay | §13, §26, §41 | **Built** — ring, MCAP, sealing, deterministic replay, and a robot now replays a window against its own resolved limits (`telemetry.replay.deterministic`); §74's telemetry on both sides, with all eight counters sourced — the five that looked like they needed a kernel are counted off this ring (§74.6) | The sealed bytes do not cross §17 (§77). No recording of a *policy* has ever been replayed, because none has run — and the same absence is why the five counters report unmeasured on a shipping robot rather than zero: a ring with no action channels has no cycles to count, which the window says rather than implies |
| Conformance and certification | §15, §31.4, §56 | **Built on both sides** — and a robot now runs §15's eight runtime tests on itself (`GET /conformance`); §15's third suite has its numerator and denominator, counted off §13's ring (§74.6) | Three of the eight execute; the other five name what they wait for. Nothing signs a result and nothing will on a robot — §19 makes that a rule rather than a gap (§15.1). The `field` figure is measurable rather than measured: it needs a robot that runs cycles |
| LeRobot | §16, §67 | **Built** — the bridge and its self-test | — |
| Hub contract | §17, §57 | **Built** — twelve endpoints, no breaking change; four of them frozen as wire fixtures produced by the real producer (§31.6) | Two of them carry assertions rather than evidence (§57.1) |
| Developer experience, driver authoring, URDF import | §18, §63–65 | **Built** — 48 CLI commands, the golden path checked mechanically | — |
| Security, local access control | §19, §39 | **Built** — Ed25519, the trust store, five signature states, four roles | — |
| Distribution and business model | §20, §72 | Designed | — |
| Languages, editions, levels | §23, §31, §42 | **Built** — one edition, held to a corpus of 68 files and 973 cases | The second edition, which is what the corpus is for |
| IPC, observation assembly, arbitration | §25 | **Built** — the shared-memory ring and the command frame | — |
| Boot, faults, POST, homing | §27, §48 | **Built** — §27's machine, POST's eight checks | Five of the eight are the kernel's, through a path §33.1 denies the daemon |
| Networking and time | §34–35 | **Built** — bearers, metering, the outbound queue, backoff, two clocks kept apart | — |
| Power and thermal | §36 | **Built** — the four states plus `unmetered`, the reserve, §36.3's ladder, §36.4's override; the gauge and the thermal zones read from `/sys`; enforced at §54.4's `acquiring`; `GET /power` and `skynet power` | §36.4's override has no input: the RHAL flag and its fault code reach this daemon through a path §33.1 denies it, so no robot can report a brownout. §36.1's `reserve` and `critical` rows ask for a controlled stop, and nothing performs one — the stop path is the kernel's |
| Configuration, calibration | §38, §45 | **Built on both sides** — the ceilings a profile resolves to reach the Hub, and §11's profile now delivers `self_collision_margin_m` and `proximity` as well, so six of `resolveLimits()`'s seven inputs have a path (§7.6) | The seventh is `people_detection`, which is not a thing §5 lets a body declare — so `proximity` is delivered and its guard is still unavailable, naming the sensor rather than the profile |
| Resource enforcement | §40 | **Built** — cgroup limits, the RT core | — |
| Frames | §44 | **Built** — the graph, forward kinematics, both pose guards | — |
| Sensor and media pipeline | §47 | **Built** — capture, gaps annotated as gaps | — |
| Audio | §49 | Designed | — |
| Entitlement | §51 | **Built** — offline verification | — |
| Skills, environments, on-robot evaluation | §54–56 | **Built** — §54.4's lifecycle, composition, safety notes, and an episode's `intervention_count` read off §13's ring rather than defaulted away (§74.6) | Nothing on this edition can run a policy, so no skill has ever executed. The count is attributed only while one invocation is in `running`: the ring counts an interval and the recording knows which policy was commanding by digest, which nothing reads yet |
| Interface — Hub-side and on-robot | §32 | **Built on both sides** — the console, the fleet page, the device page; §7.6's guards and limits cross §17 and are rendered on both, relayed rather than recomputed, and §32.2's four console panels all draw something measured | The transport plan does not cross §17, and should not until a policy runtime exists to be running one — the robot serves it per package at `GET /limits?package=`, for staged packages as well as active ones (§6.2). §32.1's guard-trip column now has its source and is frozen against a document a robot produces (§74.6); what it does not yet have is a robot that trips a guard |
| Limits, and what is not true | §59–61 | Stated | — |

**Built** means running, tested code: **93,500 lines of Rust** across fifteen crates with 1,346
tests over a corpus of 73 files and 1,020 cases, **13,300 lines** of the TypeScript reference edition
that the corpus holds it to, **5,400 lines** of drivers, and **8,200 lines** in the Hub's mirror of
the shared contracts. The Hub's own suite is 1,499 tests.

Numbers go stale; the shape does not. What the table is for is the **last column**, and the honest
summary of it is that every subsystem that can be built without hardware has been, every subsystem
that needs a physical machine is waiting for one, and the three things that are neither — a signing
key on a robot, a conformance runner, and the bytes of a sealed recording — are named in it rather
than absent from it.

Open questions are collected in §43, §53, §58.3 and §61 rather than closed prematurely. The largest
are not engineering: retargeting fidelity is a research result nobody has, joint-level access is a
property of hardware other people already shipped, and the certified tier needs a notified body.

Version one ends here. Part X is what building the other half found.

---
---

# Part X — Amendments from the Hub

Part IX ended version one. This part exists because building the **other side** of the Hub contract
found things that reading either side alone could not: a producer that had a fact and no consumer,
and a consumer specified with no producer. Both are invisible from either end, which is why they
survived twenty-four sessions of a repository that finds something every time somebody opens a page.

---

## 74. Amendment to §57 — telemetry has a route

§34.4 gives telemetry a priority, a drop policy, a counter and a paragraph of its own. §17 lists
eight endpoints and §57 lists nine, and **none of them is a route telemetry can be sent to.** Three
sessions recorded that as the one place the two documents do not line up, and it stayed open because
from the robot's side it looks like a queue that fills, and from the Hub's side it looks like
nothing at all.

Two things upstream of it turn out to depend on it, which is what settles the question:

* **§15's third suite.** Field conformance is *"continuous telemetry, forever"*, and it is the suite
  §15 calls the one nobody else does — the mechanism by which certification *"stops being a document
  and becomes a live property."* Without a route it is permanently unmeasured, and a Hub that
  defaulted it to 1.0 would be awarding on evidence nobody collected.
* **§32.1's fleet page.** Guard trip rate is a column on it. §33.6 lists `guard_trip_rate` as an
  aggregate health field, and a check-in *cannot* carry it: a rate is a count over a window, and
  §34.4 collapses check-ins precisely because every field in one is a statement about **now**. A
  rate in a check-in would make the collapse lossy.

So there is a tenth endpoint, and its shape follows from the second bullet rather than from
convenience.

```
POST /devices/{id}/telemetry     windowed counters; the class §34.4 drops first
```

### 74.1 Telemetry is windows, not samples

A telemetry item covers a **half-open interval** and carries counts over it. It is not a time series
of readings, and it is not a running total.

Each of the three alternatives fails against a rule that already exists:

| Shape | Why not |
|---|---|
| A series of samples | A week offline produces ten thousand items, which is the state §34.4 collapses check-ins to avoid. |
| A running total since boot | A drop becomes invisible: the next total absorbs it and the series stays tidy, which is exactly what §47.4 forbids. |
| Counts since the last successful send | Makes delivery load-bearing for correctness. A retry that succeeded twice would double-count, and a robot cannot know which. |

Counts over a self-describing window have none of those properties. Dropping one loses that window
and nothing else, and the loss is **visible**, because the windows no longer tile: a Hub that
receives `[10:00, 10:05)` and `[10:15, 10:20)` knows precisely what it does not have. That is
§47.4's rule — *a gap annotated as a gap is honest; a gap that looks like a quiet period is not* —
obtained from the shape of the data rather than from anybody remembering to annotate it.

### 74.2 Two clocks, and both of them

A window carries wall-clock bounds **and** a monotonic duration.

§35.1 already assigns the two: `CLOCK_REALTIME` is for *"correlation with the Hub"* and
`CLOCK_MONOTONIC_RAW` is for anything measured. A window needs both, because it does both jobs — it
has to be placed on a timeline a Hub shares with other robots, and it has to be divided into to
produce a rate.

Using wall time for the duration would let an NTP step change a rate: a clock that jumps forward
five minutes mid-window turns four hundred guard trips into a plausible number instead of an alarming
one, and nothing anywhere would look wrong. Using monotonic time for the bounds would make the
windows of two robots incomparable.

**Their disagreement is reported rather than reconciled.** `(to − from) − monotonic` is the clock
skew over the window, and a large one is a fact about the robot — an NTP correction, a clock nobody
set, a suspended machine — that is worth one line on a fleet page and is not a reason to discard the
counts.

### 74.3 Every counter says whether it was measured

The rule §33.6 applies to health, applied here for the same reason and with the same failure in
mind: **zero and unmeasured are opposite facts.** No measurement of guard trips and no guard trips
render identically as `0`, and the second is the number an operator relaxes on.

So a counter is present-and-null when nothing measured it, and a window with no measured counters is
still a valid window — it says the robot was up for five minutes and counted nothing, which is a
fact, and it is the fact every robot this edition runs will report until a kernel is running.

### 74.4 What it does not carry

**No time series, no per-cycle data, and no observations.** Those are §13's black box, which is
sealed and uploaded on a fault (§17's incident route) rather than streamed. Telemetry is the class
§34.4 drops *aggressively and first*, and anything a robot would mind losing does not belong in the
class designed to be lost.

**No control-relevant state.** §11's rule is unaffected: nothing here is read back by the robot, and
a fleet that lost every telemetry window would be exactly as safe and exactly as capable.

### 74.5 Consequences elsewhere

* **§57's count becomes ten.** No breaking change to any existing route.
* **§15's `field` suite becomes measurable.** Field conformance is `interventions / cycles` over the
  union of the windows a Hub holds — and where the windows do not tile, the union is smaller than
  the elapsed time and the Hub can say so rather than dividing by a period it did not observe.
* **§32.1's guard trip rate column has a source**, and it is the only one it could have had.
* **New table**, following the `skynet_*` convention: `skynet_telemetry_windows`.

---

### 74.6 Where the counters come from, and why five of them were not missing

§74 named eight counters and this document, and the code under it, recorded five as having *"no
source"* — `cycles`, `interventions`, `guard_trips`, `clamps` and `estops`, on the grounds that four
of them **need a kernel** and the fifth needs teleoperation. The first half of that is true and the
conclusion does not follow.

> They need a kernel to **happen**. They do not need one to be **counted**.

§13's ring is the record of what the kernel did, cycle by cycle: `Cycle` carries the guards that
tripped and the channels that were clamped, `Cycle::intervened` has computed the numerator §15
divides by since the ring was written, and §26 puts an estop on the event channel beside them. And
`skynetd` **holds that ring** — it opens it before the first reconcile pass, writes §26's events into
it, seals it on a fault and serves it over §33.7. The producer and the consumer were in one process
for twenty sessions and nothing had walked from one to the other.

The fifth was wrong for a different reason. `interventions` was recorded as needing
**teleoperation**, and §15 defines field conformance as *"the ratio of cycles the kernel did not have
to intervene in"* — a kernel intervention, which `Cycle::intervened` has computed and named as
`field_conformance`'s input since the ring was written.

Nothing in this specification asks any producer for a count of human takeovers. There is **one**
meaning of the word here, and §54.4's `intervention_count` on an episode is the same kernel number
over a narrower window — §54.4's own lifecycle ends `running` on a *guard trip*. So a comment had
invented a second meaning in order to explain why the first was impossible, which is a more expensive
mistake than a wrong word: it made a counter look like somebody else's problem.

#### A window of the ring, and the two ways it can say nothing

A telemetry window is the interval between two reconcile passes; the ring holds minutes or years. So
the window is taken **from a sequence number** — the ring's next sequence at the instant the window
opened — and covers every record at or after it. Nothing new is stored: the boundary is one integer
and the counting is a walk.

Two conditions make the five unreportable, and they are separate facts that must not be collapsed:

* **The ring has no action channels.** That is a robot with no command path — the policy runtime and
  observation assembly (§25) are unwritten — so §26's four cycle channels have no producer and
  neither has the safety path that asserts a stop. Zero here is the absence of a writer, not a
  measurement, and rendering it would put a green guard column over a robot that has never commanded
  a joint. This is a property of the **ring**, not of the edition: the day a control loop opens a
  ring with channels in it, the counters begin reporting and nothing else changes.
* **The window is no longer whole in the ring** — it wrapped, or it holds a gap, or a slot is torn.
  The counts still exist and are a **floor**, and a floor presented as a count is exactly §47.4's
  *"gap that looks like a quiet period"*, in the one place §74.1 chose its whole shape to avoid it. A
  robot logging at 1 kHz into a ring that holds seventy seconds overwrites a five-minute window four
  times over. The window is reported with the five absent, and because §74.1 made the windows tile,
  the Hub sees a hole it can name rather than a small number it believes.

They arrive **all five or none**: they come from one walk of one recording, so a window carrying some
of them would describe a ring that was partly readable, which is not a state a ring has — and would
leave a consumer dividing a measured numerator by an absent denominator.

#### Two fixtures, because one of them freezes the wrong case

§31.6 freezes the exact document a robot puts on the wire. There are now **two** for this route.
`telemetry` is what every robot alive sends: no command path, so the five are absent. `telemetry-
counted` is what a robot that commands a body sends, produced through a real ring at a real shape by
the real counting — the cycles are the only synthetic ingredient, which is the same and only one
`telemetry.replay.deterministic` has.

Freezing only the first would leave the Hub's guard-trip column and §15's field figure exercised
against nulls for as long as no robot commands anything, which is a column nobody has seen work.
Freezing only the second would freeze a document no robot can currently produce.

#### `guard_trips` and `interventions` are not the same number

A cycle in which three guards fire is **one** intervention and **three** trips, and `clamps` counts
**channels** whose command was altered. Each counter is at the granularity of the thing its name
names. Collapsing the first two would put one line on a console twice and would compute §15's ratio
from the wrong numerator.

---

## 75. Amendment to §12 and §57 — the bundle has a route

§12 is four pages about *applying* an image: A/B slots, the safe-state barrier, health-gated
rollback, and the gate it calls the single most important test in the suite. It says nothing about
**getting** one, because getting one looks like an implementation detail.

It is not, and the shape of the gap is exactly §74's. From the robot's side everything worked: the
manifest arrived, the signature verified, the compatibility list matched, the bench pass gated the
kernel change, and the action reported — correctly, for twenty-five sessions — that there was no
bundle and no way to ask for one. From the Hub's side there was a catalogue of manifests that
nobody had ever asked for bytes from. Neither end was wrong. There was no route.

So there is an eleventh endpoint:

```
GET /os/releases/{version}/bundle?arch=        the bytes of one image; a redirect, and resumable
```

### 75.1 A location is not evidence

The manifest names a digest and **not** a URL, and the robot derives the location from the endpoint
§10 paired it to.

This is worth stating because the opposite is the obvious design. A signed manifest carrying a
download URL would let a publisher point at a mirror, and it reads as the more secure option because
the URL is inside a signature. It is the weaker one. The bytes are checked against a digest that is
already signed, so a hostile mirror serving the wrong image produces a mismatch and a deleted file —
the location cannot lie about the content. What a signed URL *does* buy is a manifest that goes stale
the day a hostname changes, and a signature over a fact that did not need one.

It also keeps §57's narrow manifest narrow. The Hub's release row carries an artifact id; the robot
copies the manifest's own fields out of it and deliberately not that one, so no edition can start
depending on the Hub's storage layout.

### 75.2 A slice, not a transfer

An image is hundreds of megabytes and §33.2's loop is one thread: it reconciles, serves the local
API and pings the watchdog, in that order, for ever. A pass that blocked for the twenty minutes a
transfer takes over a warehouse link would be a supervisor that had stopped supervising — which is
the rule §34.4's drain already follows, and it applies here with more force because the payload is
two orders of magnitude larger.

So each reconcile pass moves what it can inside a bounded budget and the next one resumes.
**Resume is what makes a bounded slice free**, and resume is why the partial is keyed to the digest
rather than to the version: a partial keyed by version would let a resume append one release's tail
to another's head, and the failure would surface after the whole transfer rather than before it.

Two consequences that are not obvious until the budget exists:

* **A slice that runs out of budget is not a failure**, and must not back off. The distinction the
  robot needs is *did the file grow*, not *did the process exit zero* — a link that moved forty
  megabytes and then dropped has made progress, and doubling a delay for it makes a poor link
  exponentially poorer.
* **A server that ignores the range is caught at the response, not at the digest.** A `200` to a
  resume means the whole file was appended to what was already there. The digest would catch it,
  hours later; the status code catches it immediately and throws the partial away.

### 75.3 The bytes are checked before they are installed, however they arrived

Writing the fetcher found a hole that had been open since §12 was built, and it was not in the part
that was missing. **Every gate §12 applies is bound to the manifest's digest — and nothing compared
that digest against the bytes handed to the bootloader.** A bundle placed on a robot's disk by hand,
which was the only way to update one before this route existed, was installed unchecked.

The rule is therefore stated where it can be applied rather than assumed: an image is hashed against
the manifest immediately before it is installed, whatever put it there, and a mismatch deletes it. An
unverified bundle left on a disk is an artifact some future code path finds and treats as an image.

### 75.4 §34.3 applies, and the wait is visible

An OS image is the largest bulk transfer this system has, so it waits for an unmetered bearer. The
wait is a **state a console shows**, not a silent skip: a robot on cellular that is admitted to
update and is not updating is behaving correctly, and an operator who cannot see why will conclude
the fleet is broken.

That state is also not a failure, which turns out to need saying in the reconciler's own vocabulary:
§34.1 assumes the robot is offline, so *no link* and *a metered link* are the ordinary condition of
a fleet rather than faults in a machine. A supervisor that reported them as failures would put a red
line on a console every ten seconds for a robot doing exactly what it was designed to do.

### 75.5 Consequences elsewhere

* **§57's count becomes eleven**, and no existing route changes.
* **§12's `stage_image` becomes reachable**: it assesses, fetches, checks and installs, in that
  order. The assessment stays first, so a refusal costs no bytes.
* **A manifest published with no artifact is a state the Hub can now name.** It validates, it lists,
  and every robot on the channel holds it, gates it, admits it and then waits for an image nobody
  uploaded. The route answers that with its own refusal rather than a generic *not found*.
* **The manifest declares a size**, added in the session after this one and recorded here rather
  than in a build log because it is the field two of the rules above depend on. *A partial cannot be
  larger than what it is part of* needs something to compare against, and a progress line is a
  fraction only when the denominator came from the document a publisher signed — a size the server
  claimed describes whatever that server is serving. It is **absent, not zero**, on a manifest that
  says nothing: a zero would be a publisher declaring an empty image, which would have every robot
  on the channel discard its partial on the first pass.

  Its one cost is worth naming. The manifest's canonical bytes changed, so a robot holding the
  five-field version of a release refuses the six-field one as *"the Hub is serving a different
  manifest for release X"* — which is the immutability rule working exactly as §12 intends. Free
  today, when no robot holds one; a new version number after a fleet exists.

---

## 76. Amendment to §54.4 — the episode is the route's document

§54.4 says one episode is emitted per invocation, *"carrying the skill reference, arguments,
outcome, duration and intervention count"*, and §57 routes it to `POST /episodes`. Both halves were
built. Neither had ever put one document through the other.

The robot sent `skill`, `duration_ms` and `intervention_count`. The route reads `task`,
`duration_seconds` and `interventions`, ignores fields it does not know, and answers `201`. Every
episode a robot produced would have been stored with **no task, no duration and none of the skill** —
successfully, quietly, and with each side's own suite passing, because one asserted what the robot
sends and the other asserted what the Hub accepts.

Three rules follow, and none of them is about episodes specifically. §31.6 is the mechanism that
enforces them, added in the same session: the documents that cross §17 are frozen from their real
producers and read by the far side's real contract.

### 76.1 The producer writes the consumer's field names

A document that crosses §17 is the receiving route's shape, not a shape the producer chose and the
receiver is expected to adapt to. The alternative — a translation layer on one side — is the same
mistake wearing a coat: it is a second definition of the document, and §31.2 already gives the
reason two definitions of one structure is a defect rather than a convenience.

Where the two disagree, the route wins, because the route is the one with a table under it.

### 76.2 What the schema has no column for goes in `metadata`, not into a column that nearly fits

§54.4's list is longer than the episode table's columns: the invocation id, the digest of the bytes
that ran, the arguments, the lifecycle reason, whether it was a root invocation. All of it matters to
an incident investigation and none of it has a column.

The temptation is to put the skill reference in `task` and let the rest go. Both halves of that are
wrong: `task` is what the robot was doing, which for a skill invocation *is* the skill, so it belongs
there — and the remaining facts belong in the `jsonb` column that exists for facts a schema does not
model. Dropping them is the option that leaves an investigation with a row saying a skill failed and
no way to find which bytes were running.

### 76.3 Unmeasured has to survive the crossing

The robot omits `intervention_count` when nothing on it counts interventions, and says why in the
code that omits it. `POST /episodes` defaulted the absence to `0`, and the column was
`integer not null default 0`.

So the robot's careful absence became a zero at the boundary — and a zero here is not a neutral
value. It is *"this run needed no human"*, which is the number a marketplace quotes. §74.3 states
the rule for telemetry counters and it is general: **zero and unmeasured are opposite facts, and the
one that gets misread is always the one that looks like good news.** A schema that cannot express the
absence forces every producer to assert something false, and the producer that is most careful about
it is the one that gets misread first.

The column is nullable, the validation preserves the absence, and any aggregate over it carries the
count of rows that actually measured — because a total over four hundred episodes of which twelve
counted is a smaller number presented as a better one.

---

## 77. Amendment to §13 — the incident names its window, and does not carry it

§13 says a recording is *"sealed and uploaded on any fault, estop, or operator request"*, and §17
lists `POST /devices/{id}/incidents` as a **sealed black box upload**. Neither half was true.

The sealing existed and nothing called it: a fault produced a document, the ring kept turning, and
the window that described the fault was overwritten in the ordinary course of a robot running.
`skynet_incidents.artifact_id` was NULL on every row the platform could hold, and an investigator
reading one had no way to know a recording had ever existed.

So the seal is now taken **by the raise**, not by the caller, and the incident carries the window's
identity: name, size, digest, record count, damaged count, and `uploaded: false`.

### 77.1 Sealing is not uploading, and the document says which

A sealed window is tens to hundreds of megabytes. §34.3 puts it in the bulk class, which wants
resume, progress and metering on an **upload** — the mirror of §75's fetch, and unwritten.

Between "send it" and "say nothing" there is a third option that is honest and cheap: send the
window's identity. A Hub can then say *"this incident has a 4 MB recording on the robot, digest
`sha256:…`, that nothing has collected"*, which is a true statement somebody can act on. Saying
nothing is what leaves an operator concluding no recording was taken — the worse of the two errors,
because it closes an investigation rather than opening one.

The digest is what makes it worth carrying. When the bytes do cross, they are checked against a
digest that was recorded before the transfer existed, by the party that produced them.

### 77.2 Three states, not a field that is sometimes there

`recording` is always present and always the same shape, discriminated by `sealed`:

* **sealed** — the window, named.
* **not sealed**, with the reason: no ring, or a ring that would not open. A fact about the machine.
* **absent** — an older edition, or a producer that says nothing. **Not** evidence of either above.

A consumer that had to switch on a JSON type before reading a field would get it wrong on the robot
where it matters, which on this edition is the common case: a development root has no ring at all.

### 77.3 A seal that fails does not swallow the incident

The failure is written **into** the document and the incident is queued anyway. §34.4 makes this the
one class that is never dropped, and a robot that suppressed an alarm because its own recorder was
broken would be losing the report and the recording together — which are precisely the two things
that fail at the same moment.

---

## 78. Amendment to §8 — one document, two spellings, and an admission gate that checked nothing

§8.2 is unambiguous about which field a robot's admission gate is computed from:

> `requests` must be a subset of **`embodiment.capabilities`**, or the install fails with
> `over_request`.

Everything in `admitPackage` reads that block: the unmet-capability check, the reserved-token check,
the over-request rule. It is the field.

The Hub's card schema does not have it. It validates **`required_capabilities`** and
**`required_extensions`** at the top level of the same document, because its compatibility engine —
*"can this policy run on that robot"*, answered before anything is installed — wanted a flat list to
match against a robot's declared capabilities.

And it is the *same document*. §37.1's store pins the on-disk shape: `manifest.json` at a staged
package's root **is** the Hub's `manifest` column, and `computeVersionDigest` depends on it, hashing
the manifest as a value and the files as a list so that the two are never counted twice.

So one JSON object has two places a publisher can declare what a package needs, each with exactly one
consumer, and nothing had ever compared them.

### 78.1 The direction that matters, and what it actually produces

A publisher who fills in the Hub's fields — the ones its schema validates, its forms present and its
documentation describes — and leaves `embodiment` empty publishes a package that a robot admits with
**every check passing and nothing checked**:

* no unmet capability, because the required set is empty;
* no over-request, because `requests` is a subset of nothing only if it is also empty — and a
  publisher using the card fields has no reason to have written `requests` either;
* an operator reading a clean admission report.

That is not a stricter reading of a grey area. It is §8's entire admission gate silently doing
nothing, on a document whose publisher believed they had declared their requirements, and reporting
success.

The other direction is milder and still wrong: a publisher who fills in only `embodiment` gets a
compatibility answer computed from an empty requirement list — *"runs on anything"* — for a package
every robot would refuse at install. Nobody can act on that answer.

### 78.2 The fix is not to pick a winner

The obvious repair is to make one field authoritative and delete the other. Both halves of that are
wrong today. `embodiment.capabilities` is normative and cannot move; `required_capabilities` is on
every card this Hub has ever stored and in every compatibility rule it evaluates. A migration that
rewrote published manifests would rewrite documents whose digests are cited by benchmark results.

So the rule is stated in terms of the two consumers instead:

* **Reading is a union.** The Hub resolves a version's requirements from `embodiment` *and* the card
  field, deduplicated. The failure to avoid on this side is **under**-reporting — saying compatible
  where a robot refuses — and a union cannot under-report.
* **Publishing refuses a disagreement in one direction.** A card that declares what its own
  `embodiment` block does not is refused at publish, because that is the manifest whose gate does
  nothing. A card that declares *less* is not refused: that package is enforced correctly and only
  described incompletely, which the union above already fixes.
* **Admission refuses the same shape**, with `embodiment_undeclared` — the seventeenth rejection
  code. This is not redundant with the publish check. The publish check protects versions published
  from now on; every version already published carries whatever it carries, and the robot is the
  party that has to decline to install one it cannot check. An error rather than a warning, because
  a warning here is a package that installs having been checked against nothing.

`required_capabilities` and `required_extensions` are therefore fields on the package manifest **that
nothing enforces from**. They are read on the robot for one purpose: to notice that they disagree
with the block that is enforced. Reading them as a second source of requirements would make the
duplicate spelling load-bearing on both sides, which is how a duplication becomes permanent.

### 78.3 Consequences elsewhere

* **`PACKAGE_REJECTION_CODES` becomes seventeen**, in both editions and in
  `corpus/vocabulary/closed-sets.json`.
* **Three corpus cases**, and the first is the one worth reading: a manifest that used to admit
  `true` with an empty findings list now refuses. That diff is the defect, stated as data.
* **§55.1's environments are read at last.** *"Its `required_capabilities` are still checked at
  admission, since an environment that needs a gripper is not runnable by a robot without one in sim
  either"* — the Hub's schema has accepted the field for environments all along and its resolver
  discarded it, because the resolver was written for models and skills and the sentence was in a
  different section.
