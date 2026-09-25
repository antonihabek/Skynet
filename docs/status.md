# Status

What is built, how far it goes, and what exists only as specification — crate by crate. The
[README](../README.md) is the short version; this is the long one, and it is kept honest the same
way: where a figure here can be measured, a CI check measures it.

## What is in this repository today

| | State |
|---|---|
| `docs/` — the full specification, nine parts | Complete |
| `bindings/c`, `bindings/cpp` — the RHAL ABI | Written, compiling, layout-checked from both sides |
| `crates/skynet-kernel` — the Safety Kernel | `no_std`, no `alloc`, 8 guards, forward kinematics and its own arithmetic, 101 tests |
| `crates/skynet-contracts` — transport, admission, certification, readiness, observation, entitlement, birth, provenance, operating state, the live precondition context, OTA, identity, the check-in, §11's pull | Ported from the reference; §25.1's observation assembler; §51's offline entitlement, whose enumeration has no variant meaning *stop*; §50's birth certificate, where a board swap is an expected finding with a remedy rather than a failure; §56's provenance classes, where a declared simulator can never reach the field leaderboard; §27's operating state machine, which four other sections had been describing in prose; §54.4's skill lifecycle and §54.5's two runtime limits; §54.2's context, where a fact the robot cannot observe is **absent** rather than defaulted, so the precondition naming it is refused with the path named; §12's certification gate, where an unrecognised component is gated for the same reason the safety kernel is and a bench pass is bound to the digest it was taken against; §10's identity, where a device credential's scopes are an allow-list because §19 assumes the credential is lost, and an attestation with a hole in it is still a sentence; §17's check-in, where `applied` is derived from what is left to do rather than set, the generation on the wire is the one the robot applied, and a health reading nobody can date is not reported; §11's pull, where a Hub that reuses a generation is refused because adopting it would be invisible and one that goes backwards is followed because a fleet nobody can recover is worse; §7.6's `limits` block on the check-in, carrying the ceilings with their provenance and the guard partition with every missing input named; §47.6's frame pool, where the released slot a publish takes is the whole mechanism and the reference had it wrong; §5's `detects_people`, the seventh `available` input and the last one that had no path; §64's error-code catalog, where every machine-readable refusal has a meaning and an action; 525 tests |
| `crates/skynet-json` — canonical JSON | 16 tests |
| `crates/skynet-crash` — what a dead process left behind (§64) | The panic report both hosts write and the supervisor reads, the four-state previous exit, and the clean-stop marker. 8 tests |
| `crates/skynet-rhal` — the Rust view of the ABI | 7 tests |
| `crates/skynet-sha256` — SHA-256 | Held to the FIPS 180-4 vectors, 5 tests |
| `crates/skynet-ed25519` — Ed25519 verification (§19) | SHA-512, the field, the curve; held to RFC 8032's vectors and to the encodings that divide implementations. Verification only — signing is a test feature. 41 tests |
| `crates/skynet-urdf` — URDF import (§63.4) | XML reader, model, role suggestion, descriptor, collision geometry, 45 tests |
| `crates/skynet-cli` — the `skynet` command | descriptor, transport, frames, role, driver new/check, plus the robot commands over the local socket, `blackbox`, `skill run` / `skill stop` / `skills`, `ota`, §10's `pair` / `unpair` / `identity`, `checkin`, `intent`, `cell`, `power`, `conformance`, `readiness` and §7.6's `limits` — which carries §37.1's transport plan for a named package, staged or active; and §54's `skill check`, which holds a skill package against the contract on a workstation — the manifest decodes, the digest computes, nothing is over-requested, the timeout has an input, and no precondition names a path §54.2 publishes on no robot; 67 commands, 85 tests. `driver check --descriptor <file>` writes the body the driver describes, canonically, which is how a robot with no vendor tooling is provisioned at §28's pinned path |
| `crates/skynetd` — the supervisor's brain (§33) | Reconciler, supervision, health, leases — including §54.4's all-or-nothing acquisition; pure, 45 tests, and CI asserts it stays pure |
| `crates/skynetd-io` — the I/O half, and the `skynetd` binary | Package store, systemd launcher, observation, local API, the on-robot console (§32.2), the black box on disk, the trust store (§19), §34.3's bearers and the outbound sender, §51's cached entitlement tokens, §50's birth certificate, §27's state observed once a pass and §33.4's barrier read off it — and since 2026-09-07 the machine's **`kernel` and `rhal` conditions**, read off §13's ring rather than off systemd — a turning loop records a cycle every pass, and a hung kernel is an active unit. §24's `RHAL_FAULT` is deliberately not a loss: a faulted drive still answers reads, **§54's skill runtime** — the loop that drives the lifecycle, assembles the precondition context, takes the leases, records every transition and emits one episode per invocation — **§12's OTA half**: the signed release manifest and bench pass, the RAUC updater behind a trait, and the boot that confirms or condemns its own slot, plus **§75's fetch** — the bytes themselves, one bounded slice a pass, resumed from the partial's own length and held to the manifest's digest before the updater is shown them; **§10's identity**: two files so the identity is public and the credential is not, a pairing exchange behind a trait whose one HTTPS request never puts the code or the credential on a command line, and the attestation assembled from what this machine can actually measure; **§34.2's outbound path**: one function that makes every HTTPS request this daemon makes, with the credential in curl's config on stdin and never on a command line, and the queue drained to the Hub a robot was paired with; **§17's check-in**, built every reconcile pass and collapsed by §34.4 to exactly one; **§74's telemetry window**, whose eight counters are three from this loop and five walked off §13's ring, each saying whether it was measured; **§11's pull**, which decodes before it writes so a proxy's answer can never overwrite the intent a robot is reconciling against; **§7.6's resolved limits**, the first caller of `resolve_limits` that is a robot — and the block §17's check-in carries to the Hub's device page — the descriptor's envelope, the signed profile's ceilings, §55.2's cell and §6.3's transport clamp composed into the one set the kernel would enforce, at a tier the robot is honest about not having; **§79's upload**, the mirror of §75's fetch — the probe that asks the receiver for an offset the robot cannot read off its own disk, a resumable `PUT` bounded to one twenty-second slice a pass, and a receipt written only against the digest recorded at the seal; **§49.1's warning channel**, declared like a bearer because which output is the klaxon is a property of the machine, sounded before the state is committed, and raising an incident when it cannot be; and **§15's `safety.policy.isolation`**, which reads a running unit's confinement back off systemd rather than asserting the argv that asked for it — the difference between what was requested and what landed; 742 tests plus an end-to-end run against real systemd |
| `crates/skynet-blackbox` — the black box (§13, §26) | The preallocated ring, the MCAP writer, sealing, deterministic replay, and — since 2026-09-09 — **reading a sealed file back and putting its cycles through the kernel again**: `read` opens one, `assemble` turns the four messages each cycle is written as back into a cycle, and `replay` evaluates them. `no_std`, 55 tests. Ring format **2**: a frame records the capture paths that produced nothing, and an unmeasurable skew rather than a zero one. **Record schema 2** (2026-09-07): a cycle says what `read_state` returned, not only what the write did — before it, a recording of a driver that had stopped answering was byte-comparable with a healthy one, because every field describing the body is a fill value on a failed read. The schema byte had been written and never read since the format existed, which is what let a zero mean two things |
| `crates/skynet-ring` — the SPSC command ring (§25) | The only channel from a policy to the kernel: 4 KiB slots, a seqlock, latest-wins, never blocks. `no_std`, no `alloc`, no `unsafe`; 18 tests including a two-thread torture run |
| `crates/skynet-ring-shm` — the `memfd` under it | Creates, seals and maps the anonymous file; the consumer's mapping carries no `PROT_WRITE`. 9 tests, one of which re-executes itself so a command crosses two real processes |
| `crates/skynet-capture` — the capture-side split (§47.4) | A latest-wins slot for the policy and a bounded queue for the black box, which must never be one queue. Moves buffer handles, never pixels; every displaced or refused frame is handed back, and a run of drops is one annotated gap. `no_std`, no `alloc`, no `unsafe`; 17 tests |
| §47.6's frame pool — in `skynet-contracts`, not here | The **loan**, which is what crosses a sandbox boundary: a handle names one generation of one slot, a slot a reader holds is never reused under it, and when every slot is held the frame is dropped and counted rather than stalling capture. Ten scripted corpus cases, which every edition reproduces. Writing the corpus first found the reference taking the lowest-index free slot where §47.6 says oldest — a pool of any capacity using one slot, refusing a reader its frame on nearly every cycle (§47.6a) |
| `crates/skynet-dl` — the driver loader | `dlopen`/`LoadLibraryW`, the vtable check, and a safe wrapper around the four calls a control loop makes. The workspace's only loader `unsafe`, in one crate with two callers |
| `crates/skynet-kernel-host` — the control loop, and the `skynetk` binary (§25) | The consumer end of the command ring, running: a real driver opened, a cycle turned at a declared rate, the kernel's verdict written, a `Cycle` recorded every pass. A hold is a mask of zeros; a controlled stop ramps at the declared jerk ceiling; a ramp with no rate escalates and says so; an advisory verdict is not enforced. Records every cycle into §13's ring behind an exclusive writer lock (§26.2), seals the window on exit, and writes the **limits it ran under beside it** — without which nothing off a bench could be replayed, because `skynetd` resolves its own from a descriptor and a plan rather than from this loop's flags; `skynetk blackbox <dir>` reads one back and reports §74's five counters off it. 22 tests, plus three against the mock robot at 200 Hz with no overruns |
| `corpus/` — the conformance corpus | 1,466 cases, 93 files, and 91 closed string sets every edition must spell identically |
| `crates/skynet-corpus` — the corpus runner | Reproduces all 1,466 byte for byte |
| `reference/typescript/` — the oracle | Vendored from the Hub; generates the corpus |
| `bindings/python` — the Python RHAL | Driver protocol plus a ctypes ABI view, offset-asserted |
| `drivers/mock` — the mock robot | 31 sim-conformance checks of its own, and **a shared object a robot can actually load**: `make lib` builds it, `skynet driver check` runs the driver-facing suite against it (31 passed, 0 failed, 2026-09-24) and it declares `simulator: true`, so §35.3 caps anything built on it at guest tier by mechanism |
| `packaging/` — the installation | The two systemd units, and the two scripts that had been missing under them: `install.sh` puts the OS on a machine (§28's layout, §39.2's issued credential, `--check` re-running every postcondition, `--uninstall` that never touches `/data`), and `virtual-robot.sh` turns a machine with no robot attached into one — real binaries, real units in §33.1's order, real kernel, real guards, with the mock body underneath. Both run on every push: the `package` workflow builds the release bundle, installs it with `get.sh`, and brings a virtual robot up from it |
| `drivers/python-host` — the Python driver host | Runs a Python driver out of process, so its stop path is real |
| `drivers/lerobot` — the LeRobot bridge (§67) | Role tables, 75 checks against a fake; Guest tier |
| `drivers/mujoco` — the MuJoCo simulator driver (§70) | 105 checks against real physics with no GL context (the camera checks are skipped, not passed); Guest tier, twice over |
| `drivers/apollo` — an Apptronik Apollo in MuJoCo | A body from DeepMind's menagerie, pinned by commit and bytes, through `drivers/mujoco`: 81 checks, 18 in the layout cross-check, 33 passed through the real ABI. A simulator row by nature — no Apptronik interface is public |
| `drivers/pendant` — the other kind of robot (§59.2) | A body that takes a *program*, not a setpoint. 43 checks, almost all of them refusals, because for that class of machine the refusals are the interface; 31 passed through the real ABI |
| `drivers/canopen`, `drivers/canopen-rs` — CiA 402 over CANopen | Python: 78 checks, 31 passed through the real ABI, against an in-process bus and simulated drives. Rust port: 20 unit and vector tests, and a live test over a kernel `vcan` bus with the Python drives in another process — which needs a kernel that will create `vcan` |
| `drivers/ethercat`, `drivers/ethercat-rs` — CiA 402 over EtherCAT | Python master: 91 checks, 31 passed through the real ABI, against simulated slaves; cross-checked against SOEM, an independent master, over a veth pair. Rust port: its tests and 31 passed through the real ABI. `cia402-rs` is the state machine the two Rust ports share |
| `drivers/ur_rtde`, `drivers/ur_rtde-rs` — Universal Robots RTDE | Against **URSim** — UR's own controller software in a container (URSoftware 5.26.1 on 2026-09-24): Python 82 checks, Rust a live test; 31 passed through the real ABI each. The wire format is measured against the thing that speaks it; servo timing is not |
| `drivers/ros2_control` — a body behind `ros2_control` | 296 checks against `mock_components` over thirteen vendor descriptions, and two live bodies: a UR5e behind URSim through `ur_robot_driver`, and a Doosan M1013 behind **DRCF** — Doosan's controller emulator — through Doosan's own `dsr_hardware2` plugin (21 checks, 31 passed through the real ABI). Hosts a `hardware_interface::ResourceManager` in-process rather than talking to a `controller_manager`, because §11 forbids a network in the control loop |
| `drivers/franka`, `drivers/franka-rs` — a Franka arm over the FCI | Python: 93 checks against a fake Control box; both drivers against **franka-sim**, an independent implementation of the FCI with physics (17 checks), 31 passed through the real ABI each. The Rust port is the host-tier one: a Python-paced 1 kHz stream crosses the FCI's discontinuity limits on timing alone (F124) |
| `drivers/unitree` — Go2, H1, G1, H1-2 over unitree_sdk2 | The vendor's SDK on both sides of a fake (243 checks across four bodies), and Unitree's own simulator, `unitree_mujoco`, unmodified in a pinned image (75 checks: stand, hold, stop measured as a collapse); 31 passed through the real ABI per body (2026-09-24, thirtieth session) |
| `drivers/booster` — a Booster T1 | booster_robotics_sdk on both sides of a fake: 61 checks, 29 passed through the real ABI |
| `drivers/fourier` — a Fourier GR-1 over RoCS | 90 checks against a fake RoCS server, 16 with the vendor's own client against it; 31 passed through the real ABI, roles left to an operator |
| `drivers/x1` — an AgiBot X1 over its ROS 2 topics | 81 checks against a fake on the vendor's message types, and the vendor's own `agibot_x1_infer` MuJoCo deployment for the motion proof; 31 passed through the real ABI, roles left to an operator |
| `drivers/lite3` — a Deep Robotics Lite3 over MotionSDK | 122 checks, 13 with the vendor's own `Sender` and `Receiver` against the fake, a layout check against their headers; 32 passed through the real ABI |
| `drivers/spot` — Boston Dynamics Spot | Command-level (§59.2): 74 checks, 25 with the vendor's `bosdyn-client` against faked services; 32 passed through the real ABI. No simulator of any kind exists for Spot |
| `drivers/fairino` — a FAIRINO FR3 | The vendor's SDK source on the other side of a fake controller: 174 checks, 44 in the layout check, 25 with the vendor's own `FRRobot`; 29 passed through the real ABI |
| `drivers/pudu` — a Pudu service robot over its cloud API | 63 checks against a fake cloud, the signing held to the vendor's own construction; 32 passed through the real ABI. A developer account is what would meet the real cloud (an owner action) |

Figures are from runs on 2026-09-24 unless a row says otherwise; each driver's README has the
command that produced them and what its substrate cannot show. **Every one of these reaches guest
tier and stops there by mechanism** — §35.3 caps anything declaring `simulator: true`, and every
`bench` test in §15 is `missing` on every body, because each needs a physical machine and a person.

**Not built:** the safety MCU the Certified tier needs (§23.1 and §23.5 describe it; there is no firmware);
real-sensor capture beyond a driver-rendered stream, and the policy sandbox (the producer half of
the command path — below); and a single run on a physical machine. §59.5 is the sentence to read:
the OS is written and tested, **no robot has run any of it**, and the distance to one moving under
it is a commissioning project per robot and a bench day per tier claim.

This paragraph used to say EtherCAT, the vendor SDKs and the policy runtime were unbuilt, and before
that `ros2_control` and CANopen, and "several engineer-years" — each true when written, and each
corrected rather than quietly rewritten, because a stale *"not built"* is the one kind of wrongness
a status page cannot catch by being read. The drivers are all in the table above. **What has not
changed is the part that matters:** not one of them has driven a physical machine. They are
exercised against fakes, simulators and makers' own controller software — which shows the protocol
is right and is not evidence about a robot.

The policy runtime is the one to read carefully, because most of what that line used to cover has
moved. §25's command path has two ends, and the **consumer** end exists: `skynet-kernel-host` turns
the loop, evaluates the guards and writes to a driver. The **producer** was three things — inference,
observation assembly from real sensors, and the sandbox that would hold them. Inference is built
(`skynet-infer`, and `skynet-ort` for ONNX policies), and so is the assembler; the loop has closed on
a driver-rendered camera (all below). What is still missing is capture from real sensors (V4L2,
GenICam) and the sandbox.

OTA is a partial entry rather than either column, and the half that is missing is one nobody can
close from here. §12's three rules are built and held to a corpus: an apply waits for a declared safe
state, a boot confirms or condemns its own slot within a budget, and an image that changes the Safety
Kernel or the envelope is refused without a bench pass bound to its digest. §75's fetch closed the half this paragraph
used to name. `stage_image` no longer reports `Unimplemented` for want of bytes: it derives the
location from the endpoint §10 paired this robot to, moves one bounded slice a pass, resumes from the
partial's own length, and hashes what arrived before RAUC is shown it. The hole it closed on the way
is the one worth naming — a bundle placed on the disk by hand, which was the *only* way to update a
robot before it, went to the updater without anything checking it was the release the manifest named.
What is left is not software. None of it has run against real RAUC on a real A/B device; the updater is a trait with a recording implementation, which
is what makes §12's unattended rollback testable at all and is not the same as having watched one.

§10 is built as far as one exchange goes, and the entry is worth splitting because the two halves of
"connected" are now in different states. A robot **pairs**: `skynet pair` takes the code an operator
read off the Hub, refuses a mistyped one before any network exists, refuses a credential pasted where
a code belongs, presents the code over one HTTPS request that puts neither the code nor the
credential on a command line, and refuses to store a credential §19 says a robot should not hold. It
then knows whose it is, and §10's sentence — *"this robot is running image `sha256:…`, policy
`sha256:…`, against body `sha256:…`"* — is assembled from what the machine can actually measure, with
every hole in it rendered as the hole it is.

A paired robot now also **reports**. Every reconcile pass builds §17's check-in — the generation
it has applied, what is left to do and whether that is waiting on a person, §33.6's health, and §10's
sentence — and puts it in §34.4's queue, where the newest supersedes the last. The queue drains over
HTTPS to the Hub the robot was paired with, presenting the device credential.

Three properties of that check-in are worth stating because each is a rule rather than a field.
**`applied` is derived, not set**: §33.3 says convergence is one plan being empty, so the daemon
cannot claim it, and there turn out to be four answers rather than two — applied, converging,
deferred behind §33.4's barrier, and a supervisor whose plan is empty while the generations disagree.
**The generation on the wire is the one the robot applied**, never the one it was asked for, which is
the single field §17's rule lives on. And **a health reading nobody can date is not reported**: §33.6
asks for an age on every field, and a battery reading from four hours ago renders on a fleet console
as current.

An **unpaired** robot queues nothing, deliberately. A check-in addressed to nobody would fill the
queue, be collapsed, and eventually be counted in §34.4's drop counters — so a console would show a
robot discarding its own state for want of a Hub it was never given.

**Telemetry** used to be the one class §34.4 names with nowhere to go — a priority, a drop policy
and a counter, and no endpoint on either side of the contract. §74 gave it one. A window is built
every reconcile pass and queued as the class that is dropped first and aggressively, and all eight of
§74's counters have a source: three from this loop, five off §13's ring, which is where the kernel's
verdicts already were. The five that looked like they were waiting for a kernel needed one to
*happen* and never needed one to be *counted* — and on a robot with no command path the ring says so,
which is why those five report unmeasured rather than zero.

§11's other half is built too, so the loop closes: a paired robot **pulls** its desired state at the
top of every reconcile pass, and a fleet can change what a robot is told to run. The rule that half
turns on is the order — fetch, decode, assess, *then* write — because a Hub, a captive portal or a
proxy returning something that is not a desired state must never overwrite the one a robot is
reconciling against. §33.8's ladder is a recovery mechanism, not a licence to write first.

Two of the pull's four findings refuse the document, and the pair of them is the interesting part.
A Hub serving **two different documents under one generation** is refused, because §11 rests
idempotence on that counter — adopting would leave a robot running new software while reporting the
old generation as applied, and a fleet console would show it converged, which is invisible. A
generation that has gone **backwards** is followed, because that is a Hub restored from a backup and
a robot that refused to follow one would be a fleet nobody could recover without visiting every
machine. One produces a visible, actionable state; the other produces an invisible wrong one.

The **bundle** is fetched by the same loop, and it was its own piece of work rather than a second
call to the function that fetches a JSON document: an OS image wants resume, a slice small enough
that §33.2's single thread keeps supervising while it moves, and §34.3's metering. The partial's
length on the disk *is* the progress — there is no counter beside it, because a second record of a
fact the directory already holds is one that disagrees exactly once, after the crash that matters.

Two things §10 asks for are not built and are reported as unmeasured rather than assumed.
`security.identity.attested` wants a device key bound to a TPM or secure element, and nothing here
measures that — `/dev/tpm0` says a TPM exists, not that this key is in it — so every robot reports
the binding as unmeasured and no robot claims the property. And nothing generates a device key:
`skynet-ed25519` verifies, so the key a robot presents at pairing is the one its vendor put in §50's
birth certificate, and a robot without one pairs without a key.

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

**And, now, the consumer end of it.** `skynet-kernel-host` is the process §3's diagram draws and the
repository did not have: it opens a real RHAL driver, turns a loop at a declared rate, reads the
ring, asks the kernel what may be sent, writes the verdict to the driver, and records a `Cycle` for
every pass. `skynetk run --driver ./mock_robot.so --tier host --velocity-max 2.0 --jerk 0.1
--watchdog-ms 50` drives the mock robot at 200 Hz with four guards enforcing and four naming the
input they lack, which is §7.2 on a real machine rather than in a test.

**And §13 now has cycles in it.** `--blackbox <dir>` records every pass into the ring §26 defines and
seals it on exit — the first recording of control cycles this repository has produced. The ring has
**one writer** and takes an exclusive lock, because `Ring` holds its next sequence in memory and two
writers over one file would encode two records into one slot, producing a recording that passes every
check a reader applies and is wrong in the interval that matters. §26.2 has the argument and §80 has
the fix, which is the sequence as an atomic in a mapped header.

Four of its decisions are worth reading in `cycle.rs` rather than here, because each is a place the
translation could be quietly wrong: a **hold is a mask of zeros and never a command of zeros** (§24 —
the other reading slams a position-controlled arm to its origin); a **controlled stop ramps at the
declared jerk ceiling**, because the bound on acceleration is a bound on deceleration and inventing
a second number would be inventing a safety figure; a ramp with **no declared rate escalates to
Cat 0 and records that it did**, since escalating silently would have an operator read the harder
stop as the one the guard demanded; and an **advisory verdict is not enforced**, because a guest tier
that withheld a command the kernel said to forward would be enforcing where it has never claimed to.

Still not built: most of the **producer** end. One capture backend fills a stream since
2026-09-16 — a sensor the driver renders, read through `read_sensor` by the control loop at a
declared rate and published into §47.6's pool — and V4L2 and GenICam, which the runtime would read
itself, do not exist. The pool is built in both halves: the **loan**, reproduced by three editions,
and the Linux **mapping** (§47.6c). Nothing records a frame, because there is no encoder.

§23.3's inference backend is no longer vacant, and it is worth being exact about which part of it is
not: `skynet-infer` is the **decision layer** — position and velocity per channel and a pooled frame
in, a setpoint per channel out, seven named refusals rather than a guess — behind a `Session` seam,
with a dependency-free feed-forward runner that reads its weights out of a JSON artefact. `skynetp
run --backend inference --policy <file>` selects it. So something does now turn a frame into a
command, and `Inference` is the first backend in the workspace for which §47's whole path changes
what a joint is told to do.

**And §23.3's production runner is now built too**, in `skynet-ort`: an ONNX Runtime session behind
the same seam, reached by `dlopen` rather than by a Cargo dependency, so `CONTRIBUTING.md`'s
no-third-party rule stays a check rather than being narrowed until it admits a transitive tree
(`DRY-COVERAGE-PLAN.md` *Decisions* 12). A policy file declaring `kind: "onnx"` is now run by it; the
file's own `kind` picks the runner, and each runner still refuses the other's bytes by name.

What that route costs is a transcription of the first 101 members of a C struct **whose field order is
its ABI**, and the honest thing to report is how much of that is checked rather than trusted. Four
things: the order was extracted mechanically from the pinned `v1.30.0` header rather than read off it;
the API version is requested by number, so a library that cannot serve it returns null instead of a
shifted struct; a `size_of` test fails if the transcribed prefix ever changes length; and every load
runs a status round-trip through ONNX Runtime's **error** path — the members a successful run never
touches, and which would otherwise be unproven at the exact moment something needed reporting.

Measured, not asserted: 20 tests, of which 11 run against a real ONNX Runtime 1.30.0 and assert
arithmetic the fixtures encode — `setpoint[0] = in[0] + 0.5`, `setpoint[1] = 2·in[1] − 1` — so a pass
means the right function was called and the numbers came back right. The suite's other half needs no
library and covers every refusal reachable before one is loaded. Both halves were held to controls: a
reordered call table is refused with *"nothing was dereferenced"*, and an inserted member fails the
size test with no library present.

**Still not built:** LibTorch, which §23.3 names as the alternative, and any provider but CPU. A CUDA
session is a later deliberate decision rather than a missing piece — a policy process that silently
picked a GPU would have timing that depends on what else is on the machine, which is the opposite of
what §23.3's rule about inference spikes is asking for.

**The loop has closed on a camera**, which is the sentence this section could not write before.
`make -C drivers/python-host check-loop` starts `skynetk` against the LeRobot bridge's fake through
the real ABI — a C shim, a forked interpreter, `read_sensor` at 10 Hz — and `skynetp` beside it
running the inference backend, and requires the whole path to move: 51 frames pooled, 199 of 200
turns shown a frame, 199 setpoints published, 248 of 250 kernel cycles finding a command rather than
an empty ring, and **no overrun on either side**. Then the window `skynetk` seals on the way out is
read back and all 249 evaluated cycles reproduce their verdicts through the same kernel, with every
field the replay evaluates present on every cycle (§26.4).

That is a pipeline and not a robot. The body is a fake, the image is synthetic, and nothing in it is
evidence about a machine — every bench test in §15 is still `missing`. What it does settle is that
no step between a driver's sensor and a guard's verdict is missing.

The ring the host reads is still empty until something is producing into it, and the kernel holds on
every cycle when it is — which is not a degenerate case being tolerated: it is exactly the state
§33.1 calls the property that makes the rest of the OS possible, and it can now be watched rather
than only argued about. The frames the assembler builds are still ones the corpus handed it.

The on-robot console has now been opened in a browser, which found two things no test was watching
for: `GET /plan` answered two different shapes and the reconcile panel had never once rendered, and
the health panel repeated the same thirty-word sentence five times. Both are fixed. Opening it again
for §10's panel found a third of the same kind: the attestation sentence read *"running image an
unmeasured OS image"*, because the template carried the label and the clause carried the noun. The
bytes were canonical, the corpus reproduced them, and the sentence was unreadable. And a fourth, on
the check-in panel: it listed every health field nobody had measured and never showed the one that
was — a health report that could say what it did not know and never that a robot was well.

The panel added for §11 exists for a state that is invisible everywhere else: a robot reconciling
against a **last-known-good** copy looks identical, on every other surface, to one reconciling
against a current desired state.
`cargo test -p skynetd-io -- --ignored console_fixture` writes the page and one file per route it
fetches, from the real router, together with the `fixture-serve.mjs` that replays each route's status
code — so anybody can serve the directory and look at it without a robot. Not a plain static server: a
503 replayed as a 200 renders an error object as data, which is the one thing §32.2 forbids.

**Two things are called a console, and this repository holds one of them.** *The on-robot console* is
the page above, served by `skynetd` over the LAN from the machine it describes (§32.2). *Skynet
Console* is the operator's **desktop application** (§83) — WinUI 3 on Windows, SwiftUI on macOS — and
it lives in the Hub repository under `desktop/`, not here. It is a shell rather than a client: §83.2
has it embed this page in a `WebView2` and own only what a browser cannot do — discovery, USB,
multi-gigabyte sealed windows, an always-reachable stop, several robots at once. Never bare "the
console" where both are in play.

§49's warning tone is built, and the shape of what was missing is worth a paragraph because it
is invisible from either half. `hri.warning` has been a **reserved** capability token since §8 was
built — no grant confers it, `CapabilityReserved` refuses the package that asks, and the corpus holds
both editions to the refusal. `warning_required` has decided exactly which transitions demand a tone,
phrased over "a policy is commanding this body" so that *into `RUNNING`* and *on resumption after a
stop* are one rule rather than two. Every rule about the tone was enforceable, and **nothing had ever
made a sound**: `Transition::warning` reached the daemon's observe step and was dropped.

The channel is now declared in `state/audio.json` and sounded before the daemon commits the new
state. A robot with no declaration reports that it cannot warn rather than falling back to a
plausible default device — the fallback is the worse failure, because it produces a robot that
believes it is warning people while playing into a disconnected port.

The question §49 does not ask is what a robot does when a transition demands a tone and there is no
channel. It cannot refuse: §27's state is *derived from conditions* rather than granted, so there is
no point at which `skynetd` decides whether `RUNNING` may happen and nothing to withhold. So it
raises an incident — which §34.4 never drops — carrying the transition, the failure, and what this
robot believed its warning channel was. Refusing to command a body that cannot warn is the right rule
for a certified tier and it belongs in the kernel's interlock, where the transition is.

No robot has sounded one. None of §27's eight conditions are unobserved on this edition any
more — `homing` was the last, closed on 2026-09-07 by putting a two-bit field in twelve spare bits
of §24's flags word — but the machine still never enters a commanding state, because §25.2's lease
is the Safety Kernel's and the daemon cannot read it. The tone is in the same position as the black
box's cycle counters: the path exists, is tested against the real producer, and is gated on a
property rather than on a constant.

One thing found beside it and fixed: **a capability grant left no record at all.** §38.3 asks for a
black box event and an audit-log entry on every configuration change, and a grant is the change with
the largest safety consequence on the platform. The acknowledgement of a skill's safety notes *was*
recorded, which is the shape of the omission — this robot recorded that somebody read the warning and
not that it then handed over the capability the warning was about. `EventCode::GrantChanged` had
existed since the black box was written with exactly one producer, the reconciler, so the ring
recorded every grant a *Hub* made and none an operator made standing at the robot.

The black box is a partial entry rather than either column. The ring, the format, the seal and the
replay are written and held to the corpus; what a *supervisor* can put in one is §26's event channel
and no more, because the four cycle channels carry control cycles and there is no control loop to
produce them. `skynet blackbox` says so on every robot it is pointed at, rather than showing a
healthy-looking recording of a body that has never been commanded.

What is no longer partial is where a sealed window **goes**. §77 gave an incident the window's
identity — name, size, digest, `uploaded: false` — and said plainly that the upload itself was
unwritten, so a Hub could report a 4 MB recording that nothing would ever collect. §79 is that
upload: content-addressed at `PUT /devices/{id}/recordings/{sha256}`, resumable, one bounded slice
per reconcile pass, and a receipt written beside the recording only when the receiver names the
bytes by the digest this robot recorded at the seal. Three properties are worth stating because each
is a rule rather than a field.

**The offset is the receiver's and it is never evidence.** A download reads its own partial off its
own disk; an upload cannot, so there is a probe — and the answer places the next byte and never
concludes that a transfer succeeded. A receiver claiming more bytes than the window has is not
resumed from, and a probe that answers without a number moves nothing at all, because pushing from
zero on every failed probe would re-send the whole window for as long as the failure lasted.

**The alarm goes now and the bytes wait.** §34.3 puts a sealed window in the bulk class and §34.4
says there is nothing in the outbound queue worth less than an incident, and while sealing and
uploading were one act those two sentences could not both be obeyed — the code carried `Class::Incident`
over cellular with a comment conceding it meant tens of megabytes. It never did: that class has only
ever carried a document of a few hundred bytes. So an operator learns within seconds of a fault, on
a metered link, with the digest of the recording that describes it, and the fleet is not billed by
the megabyte for the evidence.

**`uploaded` on the incident stays false.** The incident is queued at the raise and the bytes cross
minutes or a week later; a robot that went back and rewrote a queued alarm to flip a flag would be
editing evidence after the fact. The Hub joins the two by digest, which is what content addressing
was for.

The Hub's half exists now. `PUT /devices/{id}/recordings/{sha256}` is served, and it has been driven
end to end against the real route, the real database and the real private bucket: a probe answering
404 that still carries `bytes: 0`, a slice offered at the wrong offset refused, and — the check worth
having — the stored object hashing to the digest the robot declared at the seal.

What has **not** happened is the two halves meeting. Each is exercised against a double: this robot
uploads to a test receiver and the Hub receives from a test client, and no window has yet crossed
from one into the other. Until one does, `skynet blackbox` and the on-robot console say which sealed windows
the Hub has acknowledged and which are on this robot and nowhere else, so the state is visible rather
than inferred.
