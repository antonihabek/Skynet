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
| `drivers/lerobot` — the LeRobot bridge (§67) | **alpha**. Role tables, 111 checks against a simulated robot written here behind LeRobot's robot interface, 55 with LeRobot's own SO-101 class and Feetech's SDK on a bus of STS3215 servos emulated here, and 76 with its own Koch and OpenMANIPULATOR-X classes and ROBOTIS's SDK on a bus of Dynamixel X-series servos emulated here (`make check-wire`; it found F219–F222: the real class's name, the gripper's unit, an unregistered config class, a path given as text; F232: a servo that has shut itself down on an overload read as healthy, so the bridge now reads each servo's error register and torque; and F233: the stop aborted at the first servo carrying an alert and left the rest under power). 46 of 46 seeded defects are noticed, the fault-vocabulary contract (68 checks: every error bit of both servo families, through the bridge's read and write) holds the translation the wire suites built, and the link-chaos contract (42 checks: the SO-101's serial line through a relay) found F270 (the stop took 351.8 ms against a line that carried nothing: it is one broadcast write that waits for nobody now, and the next health poll finds a servo that did not take it and sends the stop again, so a stop that one servo does not obey is a fault in the next read and no longer an error from the stop) and F271 (a late reply to an earlier request was read as error bits that differ every time, each a fault with every command refused: a reading is a fault when the servos say the same thing twice). The servos are counterparts written here, not the makers' firmware. Guest tier |
| `drivers/mujoco` — the MuJoCo simulator driver (§70) | 119 checks against real physics with no GL context (the camera checks are skipped, not passed); Guest tier, twice over. The body behind the fleet sweep: a torque joint holds with operator gains, a command carries a mode per joint, and every value crosses the actuator's gear (2026-09-26) |
| `drivers/apollo` — an Apptronik Apollo in MuJoCo | A body from DeepMind's menagerie, pinned by commit and bytes, through `drivers/mujoco`: 81 checks, 18 in the layout cross-check, 33 passed through the real ABI. A simulator row by nature — no Apptronik interface is public |
| `drivers/pendant` — the other kind of robot (§59.2) | A body that takes a *program*, not a setpoint. 43 checks, almost all of them refusals, because for that class of machine the refusals are the interface; 31 passed through the real ABI |
| `drivers/canopen`, `drivers/canopen-rs` — CiA 402 over CANopen | **beta**, both. Against **`ros2_canopen`'s CiA 402 slave on Lely's CANopen stack** (0.3.4, in a pinned image; `make check-lely-docker`), which is an implementation of CiA 301 and 402 this repository did not write, on a kernel `vcan` bus: Python 108 checks — three drives brought up, their travel and speed ceiling derived from the drive's own objects, motion read back, the stop and its release seen by a second master in the drive's own state, a drive with no gear ratio and one with another PDO mapping each refused by name — and 34 passed through the real ABI per driver, with each slave's own log agreeing on every stop and every close. Rust port: 20 unit and vector tests and a live test against the Python drives on a kernel `vcan` bus. It is a protocol counterpart and **not a vendor's drive**: its motion is a mock (one step, in units of a thousand increments), its state machine leaves `not ready to switch on` on the first controlword it is sent, and no drive's own quirks, timing or bus load are in it. Running it found F173 and F174 |
| `drivers/ethercat`, `drivers/ethercat-rs` — CiA 402 over EtherCAT | **alpha**, both: every slave either has met was written here. Python master: 127 checks, 34 passed through the real ABI, against simulated slaves, and the link-chaos contract (48 checks, both ports: the ring through a relay that delays, stalls, loses, repeats, reorders, truncates and corrupts frames; it found the master's raw-socket `TimeoutError` escaping every entry point and its stop waiting for the ring's answer, F269); SOEM, an independent master, was run against those slaves over a veth pair, which checks the slaves and not this master. Rust port: its tests and 31 passed through the real ABI. `cia402-rs` is the state machine the two Rust ports share |
| `drivers/ur_rtde`, `drivers/ur_rtde-rs` — Universal Robots RTDE | **beta**, both. Against **URSim** — UR's own controller software in a container, pinned by digest in `drivers/ur_rtde/ursim.mk` (URControl 5.26.1.0, 2026-09-25): Python 82 checks — the arm powered on, a joint moved from −1.601 to −1.409 rad, the stop latched — Rust its live test and 16 vector and unit tests; 31 passed through the real ABI each. `make check-ursim-docker` starts and removes URSim itself, and the nightly substrate workflow runs it. The Python selftest alone is 224 checks with no controller, 50 of 50 seeded defects are noticed (Rust: 89 of 89), and the link-chaos contract (38 checks, both) runs the three sockets through relays against a controller written here: it found a stop that waited 5,005 ms on a blackholed dashboard (F264), a read that raised out of the driver when the stream closed (F265), a flood of identical messages (F266) and, in the Rust port, a closed stream read as a protocol error (F273). The wire format is measured against the thing that speaks it; servo timing is not |
| `drivers/ros2_control` — a body behind `ros2_control` | A stage per body, because each runs a different vendor's plugin. **beta**: a UR5e behind URSim through UR's own `ur_robot_driver` 3.9.0, in a pinned image (12 live checks, 31 passed through the real ABI), and nine Doosan arms — the M0609, M0617, M1013, M1509, A0509, A0912, E0509, H2017 and H2515, every one Doosan's own description covers — each behind **DRCF**, Doosan's controller emulator started as that arm (its own log line says which) and reached through Doosan's own `dsr_hardware2` plugin (21 checks, 34 passed each; 2026-09-29), and a KUKA KR 6 R900 sixx over RSI through KUKA's own `KukaRSIHardwareInterface` against KUKA's own `kuka_rsi_simulator` — the maker's code on both ends of the wire, with a scripted plant rather than a controller (17 checks, 31 passed; OfficeLite is paid), and an AUBO i5 against **ARCS** — AUBO's controller software, installed from AUBO's own installer, with AUBO's simulated arm — through AUBO's own `AuboHardwareInterface`, one line of it patched to build on Jazzy (25 checks against the arm powered on by AUBO's own SDK example, 5 against it powered off, 31 passed; ARCS has no model for the i5S, which stays alpha). **alpha**: an ABB IRB 1200 over EGM, PickNik's `abb_hardware_interface` against a controller written here from ABB's own `egm.proto` (17 checks, 31 passed; RobotStudio's virtual controller is an owner step), and the other ten vendor descriptions, which run against `ros2_control`'s own `mock_components` because their vendors' plugins have not run. 447 checks cover all twenty-one descriptions and `ros2_control`'s own test component failing on purpose (2026-09-29). Hosts a `hardware_interface::ResourceManager` in-process rather than talking to a `controller_manager`, because §11 forbids a network in the control loop |
| `drivers/franka`, `drivers/franka-rs` — a Franka arm over the FCI | **beta**, both. Python: 93 checks against a simulated Control box written here; both drivers against **franka-sim** 1.1.7, an independent implementation of the FCI with physics (17 checks, 2026-09-25), 31 passed through the real ABI each on 2026-09-25 (34 for the Python driver on 2026-09-29; the Rust port's count was not re-run). The Rust port is the host-tier one: a Python-paced 1 kHz stream crosses the FCI's discontinuity limits on timing alone (F124) |
| `drivers/unitree` — Go2, B2, H1, G1, H1-2, H2, R1 over unitree_sdk2 | **beta**. The vendor's SDK on both sides of a simulated robot written here (737 checks across seven bodies, 102/103/105/104/108/108/107, and 5 more per body against a simulated robot of the same kind that reports one joint in trouble), the link-chaos contract on the kernel's own link emulation (56 checks: DDS has no address to put a relay in front of, and it found a driver that served the last state as fresh for ever once the robot stopped publishing, F277), and Unitree's own simulator, `unitree_mujoco`, unmodified in a pinned image (131 checks over the seven bodies: stand or arm, hold, stop measured as a collapse); 34 passed through the real ABI per body (2026-10-05) |
| `drivers/booster` — a Booster T1 | **alpha**. booster_robotics_sdk on both sides of a simulated T1 written here: 101 checks, 5 more against a simulated T1 that reports one joint in trouble, 4 against ones that publish another body's layout, 32 passed through the real ABI, and the link-chaos contract on the kernel's own link emulation (56 checks; it found the same flaw as Unitree's, F278). Booster's own simulator is behind a login this repository has no account for |
| `drivers/fourier` — a Fourier GR-1 over RoCS | **alpha**. 110 checks against a simulated RoCS server written here, 16 with the vendor's own client against it; 34 passed through the real ABI (2026-10-05), roles left to an operator. Fourier's simulators drive the model directly and expose no RoCS |
| `drivers/x1` — an AgiBot X1 over its ROS 2 topics | **beta**. 81 checks against a simulated X1 written here on the vendor's message types, and AgiBot's own `agibot_x1_infer` MuJoCo deployment, built from a pinned commit in `x1/sim/Dockerfile` (2026-09-25): twenty-nine joints discovered live, the left elbow commanded 0.3 rad and moved 0.213. Through the real ABI against each: 31 passed and 1 failed — `embodiment.roles.coverage`, because the roles are left to an operator |
| `drivers/xarm` — a UFACTORY xArm over its controller's own wire | **beta**. The wire was read out of UFACTORY's SDK source and is held to it by `layout_check` (42 checks, a request compared byte for byte with the vendor's own, and the driver's copy of the vendor's per-arm joint table compared with the SDK's). 98 checks against a controller written here, and against **UFACTORY's own controller firmware** (2.4.0 in the simulator image their support article names; `make check-sim-models`, 2026-09-29), started as each of the five arms the image carries — the xArm 5, xArm 6, Lite 6, 850 and xArm 7: 23 live checks each — motion, the stop and its release seen over a fresh connection and by the vendor's own `XArmAPI`, the firmware's own `is_simulation_robot` carried into the descriptor — and 34 passed through the real ABI each. It found F175: the firmware acknowledges a servo target beyond a joint's range with a clean reply, moves the arm toward it and faults afterwards, so the driver refuses such a target before sending. And F181: on four of the five models the firmware reports joint ranges wider than UFACTORY's own SDK table for that arm (it reports the xArm 6's ranges on every model, so an xArm 5's wrist bend lands on the wrong joint), and two start with error 41 (*the friction file is invalid*), so the driver's travel is the narrower of the two sources. No motors, no timing at rate |
| `drivers/mavlink` — an ArduPilot or PX4 vehicle over MAVLink | **beta**. The first driver for aerial and ground vehicles, and command-level (§59.2): the flight controller owns every servo loop, the body has no joints, and it is moved by named motions. The codec is held to the reference implementation, `pymavlink` (`layout_check`, 235 checks: every message's seed *recomputed*, every frame byte for byte, every mode number of four vehicle families; it caught six transcription errors on its first run), and the driver's own suite is 314 checks against a vehicle written here (ArduPilot's and PX4's). The substrate is **ArduPilot's own flight code built for x86** at a pinned commit (`make check-sim-docker`, 2026-09-29): a multirotor, a rover, a boat and a fixed wing armed, moved and stopped through the real ABI with the reference reading the same vehicle on a second serial port, the link impaired with delay, jitter and loss, and a submarine refused by name; 34 passed through the real ABI for each of the four. The boat (2026-10-02) is ArduRover built with a boat frame and declares `base: surface` (§97); its stop is HOLD, which stops the thrust and not the hull. It found F179 (ArduRover reads a yaw rate of zero as "drive straight"). **PX4** (2026-10-03) is the second motion set, held to PX4's own published SIH image (`make check-px4-docker`, nightly as `px4-sih`): a multirotor passed 58 of 58 live checks on each of three clean runs, a fixed wing 16 of 16 and a rover 19 of 19, each with 34 of 34 through the real ABI at guest, the reference reading the same vehicle on PX4's second MAVLink instance. It found F224–F231, among them F231: a mode request is one datagram, and on a link that loses one in thirty a lost request for hold left PX4 in offboard with nothing streaming, which it answers by flying home. Submarines and a PX4 boat or VTOL are refused (F178 gave §5 a base kind for water, §97; ArduSub still has no driver). SITL is the firmware on a simulated board: no motors, no sensor noise, no radio |
| `drivers/ros_industrial` — a robot arm behind a ROS-Industrial `simple_message` server | **beta**. A Python driver under `drivers/python-host` for the arms whose controller runs the server ROS-Industrial's protocol defines: Yaskawa Motoman (MotoROS), ABB (`abb_driver`'s RAPID server), FANUC (`fanuc_driver`), Stäubli and others, up to ten joints, **claimed from ROS-Industrial's and each maker's documentation and not run**. Joint-level (§59.2): one trajectory point per command. The protocol carries no joint limits, joint count, speed ceiling, model or word on whether the controller is a simulation, so the arm's own go in the configuration (a missing one is a refusal) and `simulator` has no default. 146 checks against a simulated controller written here, 69 of 69 seeded defects noticed (one equivalent mutant left out) and a control, 34 through the real ABI, and the link-chaos contract (38); the driver's bytes are held to **ROS-Industrial's own library built from source and run**, in both directions and in its four builds (118 checks), and the live block runs against **a controller's server built from that library** (its sockets, framing, codecs, message manager and ping handler), 35 of 35 and 34 of 34 through the real ABI on each build (`make check-sim-docker`, nightly as `ros-industrial-sim`). It is an independent implementation of the protocol written by the people who wrote it, and no robot maker's controller: the arm behind the sockets is written here, `error_code` is each vendor's and nothing is mapped onto it. It found that the maker's own server accepts the next client up to five seconds late (F241: the driver refused a controller that was merely waiting), a transcription defect the cross-check caught (F242) and two traps in the library as a Linux server (F243) |
| `drivers/reachy_mini` — a Pollen Robotics Reachy Mini over its daemon's REST interface | **beta**. A Python driver under `drivers/python-host` for the Reachy Mini (the head's six degrees of freedom, the body's yaw and two antennas: nine scalar joints), Wireless and Lite, **claimed from the maker's README and the daemon's own source and not run on either robot**. Joint-level (§59.2): one `set_target` per command. The daemon documents pitch, roll and both yaws and says nothing of the head's translation, the antennas' range or any speed, so the operator gives those (a missing one is a refusal), `stop_mode` has no default, and `simulator` is the daemon's word. 168 checks against a simulated daemon written here, 73 of 73 seeded defects noticed and a control, 34 through the real ABI, and the link-chaos contract (30); the driver's routes, models and limits are held to **the maker's own source and run through its own pydantic models** (30 checks), and the live block runs against **the maker's own daemon in the maker's own MuJoCo simulator**, headless, with the maker's models, physics and web server's access log as the second opinion: 39 of 39 and 34 of 34 through the real ABI (`make check-sim-docker`, nightly as `reachy-mini-sim`). The maker's simulator does not model motor torque, so `limp` is held to the request its server logged and not to a head that went limp; the robot's firmware, heat, cables and the real antenna travel are not reached. It found that the maker's daemon cannot be asked what it was told to go to (F245: the route answers 500 before a command and returns no target after one), that a robot found outside the ranges it was given was accepted and then refused every command without a word (F246, the maker's daemon starts asleep), that every refusal said nothing (F247), and that the substrate's image had installed Git LFS pointer files for the maker's meshes (F248) |
| `drivers/schunk` — a SCHUNK EGU, EGK or EZU gripper over its module's web interface | **beta**. A Python driver under `drivers/python-host` for SCHUNK's EGU, EGK and EZU electric grippers, through the web interface their fieldbus module serves (two routes, two 16-byte frames), **claimed from the maker's client and simulator and not run on a gripper**. Joint-level (§59.2): one prismatic joint, `jaw`, in metres from the module's own travel, which a configuration may narrow and never widen; `stop_mode`, `speed` and `simulator` have no default, a PROFINET module is refused by name, and a module that comes up in its fast stop opens with the stop latched and is released only by an acknowledge that is verified to have cleared the error. The wire was read out of SCHUNK's own repository and is held to its client, which is run (`layout_check.py`, 21 checks); the live block (83 checks, 37 passed through the real ABI) runs against **the maker's own `schunk_gripper_dummy`** in a container with the maker's own client as the second opinion and a record of every frame its server was handed (rung 2). 204 checks against a simulated module written here, 130 of 130 seeded defects noticed (the first run noticed 109: each of the 21 that survived had no check and is one now, shown red against its seed, and so is the one a later run found, a stop landing while a command waits for the module's toggle, which the first test written for it passed for the wrong reason), the link-chaos contract (30). F249–F256: the maker's client and its own PROFINET dump disagree on where the error byte is, its simulator comes up in a fast stop, clears *ready for operation* on every write and neither stops a motion nor replaces one, and the driver's own: a grip or release left the "already issued" target in place (F253), and a command could take 336 ms against a control loop's quarter of a second (F256). A stop is held to the frames the module was handed and not to the jaw, and what a stop does to a held workpiece is in a manual behind a sign-in this repository does not use |
| `drivers/robotiq` — a Robotiq 2F-85 or Hand-E gripper over Modbus RTU | **beta**. A Python driver under `drivers/python-host` for Robotiq's 2F-85 and Hand-E electric grippers over their Modbus RTU line (a serial adapter, or a TCP gateway to one), **claimed from the maker's SDK and not run on a gripper**. Joint-level (§59.2): one prismatic joint in metres. `model`, `speed`, `effort`, `stop_mode` and `simulator` have no default, a 2F-140 (which the maker's SDK has no profile for) is `custom` with a profile the operator measured, and `stop_mode` accepts only `halt`, because rATR, the maker's emergency release, opens or closes the fingers all the way and leaves a major fault, and whether that is the safer stop is the integrator's risk assessment. Activation and the reset after a major fault are motions a person runs, never something `open` does (each is a calibration sweep of the fingers' full range). The driver sends one command image in every exchange (function 0x17), which is also the keep-alive for the gripper's one-second no-communication fault, and a command waits for the exchange that carried it, within a quarter of a second, and puts the image back if it did not. 343 checks against a simulated gripper written here, the driver's bytes held to the maker's own client, which is run (`layout_check.py`, 43 checks: 500 random commands as function 0x17 and as 0x10, 1,512 status blocks decoded by the maker's accessors, both fault tables against the maker's enumerations, the maker's whole `Gripper` driving the simulated one), a live block against **Robotiq's own simulated gripper and client** (the maker's SDK ships a register model behind its own Modbus RTU server), built unmodified from the maker's repository in a container (`make check-sim-docker`, nightly as `robotiq-sim`; 56 of 56, with a record of every request the maker's server was handed and a state staged in the maker's own register model), 37 passed through the real ABI at guest, the link-chaos contract (51), the fault-vocabulary contract (124 checks, 31 codes injected) and **200 of 200 seeded defects noticed** (one equivalent mutant left out, three controls that survive as they must; the first campaign noticed 191, and each of the nine survivors was a check nobody had written). F262: a gripper power-cycled behind a live driver would have been activated by the driver's own command image, a sweep nobody asked for; F263: a command waiting when the image is made again would have been reported carried. The maker's simulated gripper has no motion, object or fault of its own, so those are held to the simulated gripper written here, and a gripper power-cycled while the link stays up looks live to it. OnRobot and Inspire, whose register maps are their makers' and which have no simulator, are not driven |
| `drivers/ros2_base` — a ROS 2 mobile base over its velocity-command and odometry topics | **beta**. A Python driver under `drivers/python-host` for the class of bases whose ROS 2 interface is a `Twist` or `TwistStamped` in and `nav_msgs/Odometry` out: Husky, Jackal, TurtleBot, Create 3, Stretch, TIAGo and anything on `ros2_control`'s `diff_drive_controller`, **claimed from each maker's documentation and not run, except the TurtleBot3 Burger and the Create 3, which ran in their makers' simulations**. Command-level (§59.2): no joints, a `wheeled` base, moved by `status`, `velocity` and `hold`. 226 checks against a simulated base written here, 58 of 58 seeded defects noticed; against `ros2_control`'s own `diff_drive_controller` on its mock hardware, with a second reader of the same base written without this repository's code, ROS 2 Jazzy (a `TwistStamped` command; 47 of 47 live checks and 34 of 34 through the real ABI) and Humble (a plain `Twist`; 46 of 46) pass (`make check-ros-docker`, nightly as `ros2-base-sim` and `ros2-base-humble`); and **ROBOTIS's own TurtleBot3 simulation** (Gazebo, the burger's differential-drive plugin, its bridge; `make check-tb3-docker`, nightly as `ros2-base-turtlebot3`) passes 28 of 28 live checks and 34 of 34 through the real ABI, with Gazebo's own pose of the model as the second opinion: the driver's displacement is the physics' to 2 mm; and **iRobot's own Create 3 simulation** (Gazebo under a virtual display, the maker's `motion_control` safety node and controller; `make check-create3-docker`, nightly as `ros2-base-create3`) passes 29 of 29 and 34 of 34, held in the simulation's own time (it runs at about a fifth of real time on a CPU renderer). The controller is the ROS project's implementation of the interface and not any base maker's software, its hardware is never late and has no inertia or slip, and the TurtleBot3 and Create 3 runs are the makers' simulators, not the robots. It found that the mock hardware does not integrate a velocity into a position unless told to, a yaw change that wrapped at the half turn (-5.78 rad for a turn of +0.47), and Humble's topic name left in the descriptor's slug |
| `drivers/lite3` — a Deep Robotics Lite3 over MotionSDK | **alpha**. 179 checks, 13 with the vendor's own `Sender` and `Receiver` against a simulated motion host written here, a layout check against their headers; 32 passed through the real ABI. 58 of 58 seeded defects are noticed (two equivalent mutants left out, four controls that survive as they must), and the link-chaos contract (68 checks: the motion host's two UDP flows through relays that delay, lose, repeat, reorder, cut short, corrupt and blackhole them, with a sixth question, whether the host acts on the driver's commands once the link is back) found F276: the driver re-took control right only when its own sends had been quiet, so after a stall of 1.5 s every command was accepted by the socket, ignored by the robot and answered `OK`; a state stream that was quiet for longer than the reclaim guard now makes the next command take control right again. No Lite3 simulator exists |
| `drivers/spot` — Boston Dynamics Spot | **alpha**. Command-level (§59.2): 74 checks, 25 with the vendor's `bosdyn-client` against simulated services written here; 32 passed through the real ABI. No simulator of any kind exists for Spot |
| `drivers/fairino` — a FAIRINO FR3 | **alpha**. The vendor's SDK source on the other side of a simulated controller written here: 272 checks, 44 in the layout check, 25 with the vendor's own `FRRobot`; 29 passed through the real ABI. 82 of 82 seeded defects are noticed (two controls survive as they must), and the link-chaos contract (38 checks: the controller's three TCP sockets through relays, the first C++ subject) found F274 (a servo command waited out the controller's answer, 1,497 ms under a stall and 2,028 ms under a blackhole against a control loop's quarter of a second: a command, the calls that open a servo window included, is now given 200 ms in all) and F275 (the stop's blocking send could hold 2,026 ms into a full buffer: it never waits for room now). The maker's SimMachine is behind a registration wall |
| `drivers/pudu` — a Pudu service robot over its cloud API | **alpha**. 63 checks against a simulated cloud written here, the signing held to the vendor's own construction; 32 passed through the real ABI. A developer account is what would meet the real cloud (an owner action) |
| `drivers/odrive` — ODrive servo axes over CANSimple | **alpha**. 173 checks against simulated axes written here on an in-process bus (181 on a kernel `vcan` against axes in another process), the frame ids, payload layouts and enumerations held to ODrive's own firmware, CAN database and enums module at a pinned commit (`layout_check.py`, 96 checks, which pins two disagreements between the maker's own sources, F280); 32 passed through the real ABI (2026-10-05). 37 of 37 seeded defects noticed, the fault-vocabulary contract (76 checks: the DBC's 13 axis error words and 6 undefined bits, each held to the flags, the word, refusal and recovery) and the link-chaos contract (40 checks: the bridge on one `vcan`, the axes on another, a CAN relay across — clean on the first run, with the stop returning in 0.1 ms and the body renewed by re-opening, the ENOSYS path by design). No ODrive was on any bus it was measured against, and ODrive publishes no simulator |

**The fleet sweep** (`fleet/`, `reference/reports/fleet.json`, 2026-09-29): 224 public robot descriptions of 177 robots from 79 makers, pinned by commit and bytes, 174 robots through conformance and the kernel loop. These are descriptions run through the MuJoCo driver, not drivers; no row moves a release stage. The `travel` stage passes on all 134 descriptions with an asymmetric joint, since §92 (F159), and cross-robot `transport` binds on 102 (F165, F163, F177): 85 of the 94 arm descriptions, each under a role table an integrator accepted for its arm family and the sweep checks against the body's own zero-pose axes before applying it (item 10.8; 7 bind directly and 78 degraded, the joints a five-degree-of-freedom policy has no counterpart for being held), the 14 quadrupeds under the Go2 reference policy, and 3 humanoids. Nine arms are refused, each in its own words: three whose wrists are tilted 30° off what the table asserts, four with too few or too many joints for the policy, and two whose joints the tables do not name. **Binding is not working**: whether a policy trained on one arm does anything on another is §59.4's research result, which nobody has.

Figures are from runs on 2026-09-24 unless a row says otherwise; each driver's README has the
command that produced them and what its substrate cannot show. **Every one of these reaches guest
tier and stops there by mechanism** — §35.3 caps anything declaring `simulator: true`, and every
`bench` test in §15 is `missing` on every body, because each needs a physical machine and a person.

**Not built:** the safety MCU the Certified tier needs (§23.1 and §23.5 describe it; there is no firmware);
real-sensor capture beyond a driver-rendered stream (the producer half of the command path — below);
a run of `sandbox.sh` on a machine without systemd as PID 1 — the kernel's half of the power-on self-test
(§48.5) and the first skill judged `success` are built (DRY-COVERAGE-PLAN.md 9.7, F152–F156) and were held
against the real decoders and the system-manager CI job, and have not been run on this checkout's own
machine; and a single run on a physical machine. §59.5 is the sentence to read:
the OS is written and tested, **no robot has run any of it**, and the distance to one moving under
it is a commissioning project per robot and a bench day per tier claim.

This paragraph used to say EtherCAT, the vendor SDKs and the policy runtime were unbuilt, and before
that `ros2_control` and CANopen, and "several engineer-years" — each true when written, and each
corrected rather than quietly rewritten, because a stale *"not built"* is the one kind of wrongness
a status page cannot catch by being read. The drivers are all in the table above. **What has not
changed is the part that matters:** not one of them has driven a physical machine. They are
exercised against simulated counterparts written here, simulators and makers' own controller
software — which shows the protocol is right and is not evidence about a robot. That is what the
release stage in each row says: **alpha** when every counterpart the driver has met was written in
this repository, **beta** when it has also been run against software this repository did not write
that speaks the robot's own interface (the maker's controller software, the maker's simulator, or an
independent implementation of the protocol) and the nightly substrate workflow re-runs that at the
pinned versions and at the makers' latest. Neither means it works on a robot. **stable** is reserved
for a driver that has run on a physical robot, and none has; `drivers/models.json` carries each
stage and what it rests on, `skynet setup --robot` prints it before installing, and
`tools/check-release-stages.mjs` refuses a manifest, a workflow or this page that disagree
(DRY-COVERAGE-PLAN.md *Decisions* 19).

The policy runtime is the one to read carefully, because most of what that line used to cover has
moved. §25's command path has two ends, and the **consumer** end exists: `skynet-kernel-host` turns
the loop, evaluates the guards and writes to a driver. The **producer** was three things — inference,
observation assembly from real sensors, and the sandbox that holds them. Inference is built
(`skynet-infer`, and `skynet-ort` for ONNX policies), and so is the assembler; the loop has closed on
a driver-rendered camera (all below). The sandbox is built (DRY-COVERAGE-PLAN.md 9.2). A package
that may command a joint runs with no network, no device nodes, a read-only tree, a syscall
allow-list and, in the system manager, a user of its own, and §25's socket is shared with one
group that only the commanding package is in. `sandbox.sh` measures six routes to an actuator
closed and the ring still arriving. What is still missing is capture from real sensors (V4L2,
GenICam).

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
the OS over post-state, every transition recorded and one episode emitted per invocation. And the
skill's own process is started with §25's rendezvous in its environment, so it can take the command
ring from the loop and command the body. `sandbox.sh` section 7 runs one that moves the mock arm's
elbow from inside the sandbox. The daemon reads the body off the ring it records, so a
precondition can name `robot.state.joints.J2`: the skill passes it on its first run and is refused
by it on its second, because the elbow moved. This paragraph said for several rounds that no
sandbox received a descriptor and nothing turned a frame into a command, which had stopped being
true (DRY-COVERAGE-PLAN.md F145). This paragraph then said that what was still missing was a verdict,
because five of §48.2's checks are the kernel's and nothing reported them; that stopped being true on
2026-09-26 (DRY-COVERAGE-PLAN.md 9.7, F152–F156). `skynetk` now runs those five checks at start and
records each verdict in §13's ring, `post::run` reads them, and a robot whose eight checks pass leaves
`post`. §54.3 has judged one skill `success` and its control `failure` (`sandbox.sh` section 7, on the
mock arm's elbow with `J2 >= 0.05`). That is the only verdict outside the unit tests: it is not a Hub
skill, it is not a physical joint, and F157 (the terminal row forgets which criterion decided) is open.

**Packages arrive by themselves** (§91). A robot told to run a package it does not hold fetches it
from its Hub. It reads §91's document, fetches each file in bounded slices that resume, checks each
against its sha256 and length, and stages the whole only if it hashes to the version digest the
desired state named. The Hub serves only a digest the device's desired state names, on the
standing of the person who set it. `e2e.sh` section 12 runs it against a stand-in Hub, and the
Hub's route has its own suite. `skynet install` still works for a robot with no Hub.

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
`make -C drivers/python-host check-loop` starts `skynetk` against the LeRobot bridge's simulated robot through
the real ABI — a C shim, a forked interpreter, `read_sensor` at 10 Hz — and `skynetp` beside it
running the inference backend, and requires the whole path to move: 51 frames pooled, 199 of 200
turns shown a frame, 199 setpoints published, 248 of 250 kernel cycles finding a command rather than
an empty ring, and **no overrun on either side**. Then the window `skynetk` seals on the way out is
read back and all 249 evaluated cycles reproduce their verdicts through the same kernel, with every
field the replay evaluates present on every cycle (§26.4).

That is a pipeline and not a robot. The body is simulated, the image is synthetic, and nothing in it is
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
