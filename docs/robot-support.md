# Robot support — which robots can install Skynet OS today, and how far each has been tested

Written 2026-09-29, on the owner's instruction to say *how well* the robots are tested without one, to name
the robots that have a driver, and to challenge every claim that a driver cannot be written. The tables in
the two generated sections are produced from [`drivers/models.json`](../drivers/models.json) and
[`drivers/robots.json`](../drivers/robots.json) by `node tools/robot-support.mjs`, and CI fails when this file
disagrees with them. Everything else is prose, and is dated.

## 0. The answer to "will it work on the robot?"

**No robot has run any of this.** That is [`status.md`](status.md)'s sentence and §59.5's, and nothing below
weakens it. No driver is `stable`; `tools/check-release-stages.mjs` refuses the word until one has run on a
physical machine. What follows says what *has* been measured, against what, and what has not.

**There is no honest way to put 99% on a driver from here, and this page will not.** A number like that says
the residual risk is small and known. Ten kinds of thing can go wrong between a green run and a moving
machine (section 2); a dry test reaches four of them well, two partly and four not at all. What can be said
instead, and measured, is the following.

* **Where a driver has met the maker's own software, it has been changed by the meeting every time.** Section
  3 lists thirteen such first contacts, with ten distinct pieces of software behind them (URSim was met three
  times, and one row is PickNik's client against a controller written here); each one found at least one defect
  that every counterpart written in this repository had agreed with. The finding that generalises is not any one defect but the rate: a driver
  that has met only counterparts its own author wrote (`alpha`) should be presumed to carry defects of that
  kind, and the ones it carries are not knowable from here.
* **A `beta` driver's first physical run should get through open, read, hold, one commanded motion and stop**,
  because those were measured against software that speaks the robot's own interface. It is not evidence for
  the servo loop under load, for timing at rate, for the robot's own protective reactions, or for anything a
  person will do with the machine afterwards.
* **The distance from here to "works" is a commissioning project per robot and a bench day per tier claim**
  (§59.6, §15). That does not shrink with more dry testing; the dry testing shrinks the part of it that is
  the driver's fault.

## 1. Installable today

<!-- generated:summary -->
**54 robot rows across 20 drivers that command a physical robot: 34 beta, 20 alpha, 0 stable.** (A row is one robot or one robot family a driver was built for or run against; a driver that hosts other vendors' plugins counts each body.)
<!-- /generated:summary -->

"Installable" has two meanings, and the second is the narrower one that the table below does not yet separate.
The driver passes `skynet driver check` (the sim half of the conformance suite, through the real ABI, which
means it reaches guest tier and not that every check passed — the X1 exits 0 with `embodiment.roles.coverage`
failed, because its roles are an operator's to assert). Then either **one command** installs it —
`skynet setup --robot <driver>` finds the library, prints the stage and what it rests on, opens the control
loop's unit to the network the driver needs and no other (`--driver-address` names the controller), and prints
the site checklist. It works for every driver row: the Rust and C shared objects (`canopen-rs`, `ethercat-rs`,
`franka-rs`, `ur_rtde-rs`, and `booster`, `fairino`, `lite3`, `unitree`, `x1`, `ros2_control` where the vendor
SDK they link is on the machine that built them) and every Python driver (`lerobot`, `pudu`, `canopen`, `xarm`,
`mavlink`, `ethercat`, `fourier`, `franka`, `spot`, `ur_rtde`), which installs as python-host with its package,
the RHAL binding and the interpreter it needs, the distributions it imports checked on the robot first. The
release bundle ships every driver that needs no vendor SDK, read from the manifest
(`DRY-COVERAGE-PLAN.md` items 12.B.1, 12.B.2, 12.B.5; the one thing that measured the network claim is
`packaging/test-install-needs.sh`, on a real systemd). Neither
meaning says the robot is commissioned: an operator still asserts roles (§63.4), provisions the envelope
(§5.2), and a person stands next to the machine for every `bench` test in §15, none of which any robot has had.

`Rung` is F118's ladder for the strongest counterpart the driver has met: **1** the maker's controller software
run as a virtual controller, **2** the maker's simulator speaking the robot's own interface, **3** an
independent implementation of the maker's protocol, **4** a counterpart written in this repository.
`alpha` is rung 4; `beta` is rung 1 to 3, re-run every night at the pinned versions and at the maker's latest
by [`substrates.yml`](../.github/workflows/substrates.yml).

<!-- generated:installable -->
| Robot | Driver (`skynet setup --robot …`) | Interface | Stage | Rung |
|---|---|---|---|---|
| Booster T1 | `booster` | joint-level | **alpha** | 4 |
| FAIRINO FR3 (the FR series share the vendor's SDK; only an FR3 was built for) | `fairino` | joint-level | **alpha** | 4 |
| Deep Robotics Jueying Lite3 (the X20 is refused by name) | `lite3` | joint-level | **alpha** | 4 |
| Unitree Go2 | `unitree` | joint-level | **beta** | 2 |
| Unitree H1 | `unitree` | joint-level | **beta** | 2 |
| Unitree G1 | `unitree` | joint-level | **beta** | 2 |
| Unitree H1-2 | `unitree` | joint-level | **beta** | 2 |
| AgiBot X1 | `x1` | joint-level | **beta** | 2 |
| Universal Robots UR5e, through UR's own ur_robot_driver | `ros2_control` | hosted | **beta** | 1 |
| Doosan M0609 | `ros2_control` | hosted | **beta** | 1 |
| Doosan M0617 | `ros2_control` | hosted | **beta** | 1 |
| Doosan M1013 | `ros2_control` | hosted | **beta** | 1 |
| Doosan M1509 | `ros2_control` | hosted | **beta** | 1 |
| Doosan A0509 | `ros2_control` | hosted | **beta** | 1 |
| Doosan A0912 | `ros2_control` | hosted | **beta** | 1 |
| Doosan E0509 | `ros2_control` | hosted | **beta** | 1 |
| Doosan H2017 | `ros2_control` | hosted | **beta** | 1 |
| Doosan H2515 | `ros2_control` | hosted | **beta** | 1 |
| ABB IRB 1200, over EGM | `ros2_control` | hosted | **alpha** | 4 |
| ABB IRB 120 | `ros2_control` | hosted | **alpha** | 4 |
| AUBO i5, through AUBO's own plugin | `ros2_control` | hosted | **beta** | 1 |
| AUBO i5S | `ros2_control` | hosted | **alpha** | 4 |
| DENSO VS-060 | `ros2_control` | hosted | **alpha** | 4 |
| Elite CS520H | `ros2_control` | hosted | **alpha** | 4 |
| FANUC LR Mate 200iD | `ros2_control` | hosted | **alpha** | 4 |
| Kawasaki RS007L | `ros2_control` | hosted | **alpha** | 4 |
| Kinova Gen3 | `ros2_control` | hosted | **alpha** | 4 |
| KUKA KR 6 R900 sixx, over RSI | `ros2_control` | hosted | **beta** | 3 |
| Mitsubishi MELFA RV-4FR | `ros2_control` | hosted | **alpha** | 4 |
| Stäubli TX2-60L | `ros2_control` | hosted | **alpha** | 4 |
| Techman TM7S | `ros2_control` | hosted | **alpha** | 4 |
| SO-100 and SO-101 (role tables written), and the other robots LeRobot's Robot class supports | `lerobot` | joint-level | **alpha** | 4 |
| Pudu service robots, through the vendor's cloud OpenAPI (delivery, cruise, call) | `pudu` | command-level | **alpha** | 4 |
| any CiA 402 servo drive on CAN that exposes the objects the driver reads (encoder resolution, gear ratio, limits, speed) | `canopen` | joint-level | **beta** | 3 |
| the same CiA 402 drives, from the Rust port | `canopen-rs` | joint-level | **beta** | 3 |
| UFACTORY xArm 5 | `xarm` | joint-level | **beta** | 1 |
| UFACTORY xArm 6 | `xarm` | joint-level | **beta** | 1 |
| UFACTORY Lite 6 | `xarm` | joint-level | **beta** | 1 |
| UFACTORY 850 | `xarm` | joint-level | **beta** | 1 |
| UFACTORY xArm 7 (each row against UFACTORY's firmware started as that arm) | `xarm` | joint-level | **beta** | 1 |
| ArduPilot multirotors, fixed wings and ground rovers (the driver reads the airframe from the heartbeat) | `mavlink` | command-level | **beta** | 1 |
| any CiA 402 drive over EtherCAT (CoE) | `ethercat` | joint-level | **alpha** | 4 |
| the same drives, from the Rust port | `ethercat-rs` | joint-level | **alpha** | 4 |
| Fourier GR-1 | `fourier` | joint-level | **alpha** | 4 |
| Franka Robotics arms over the FCI (server version 10) | `franka` | joint-level | **beta** | 3 |
| Franka Robotics arms over the FCI, from the Rust port | `franka-rs` | joint-level | **beta** | 3 |
| Boston Dynamics Spot (the quadruped; its arm is not driven) | `spot` | command-level | **alpha** | 4 |
| Universal Robots UR3e | `ur_rtde` | joint-level | **beta** | 1 |
| UR5e | `ur_rtde` | joint-level | **beta** | 1 |
| UR10e | `ur_rtde` | joint-level | **beta** | 1 |
| UR16e | `ur_rtde` | joint-level | **beta** | 1 |
| UR20 | `ur_rtde` | joint-level | **beta** | 1 |
| UR30 | `ur_rtde` | joint-level | **beta** | 1 |
| the same UR arms, from the Rust port | `ur_rtde-rs` | joint-level | **beta** | 1 |
<!-- /generated:installable -->

Also installable, and not robots: `mock` and `pendant` (teaching drivers, §59.2), `python-host` (the generic
out-of-process Python host), `mujoco` and `apollo` (simulator bodies: guest by mechanism, no hardware claim),
and `cia402-rs` (the state machine the two Rust CiA 402 ports share). The fleet sweep
([`fleet/README.md`](../fleet/README.md)) runs 224 public robot *descriptions* through the MuJoCo driver; it
tests descriptions, not drivers, and no row of it moves a release stage.

## 2. What a dry test can and cannot see

| # | Layer | What goes wrong on a real robot | Reachable without one? | What reaches it here |
|---|---|---|---|---|
| 1 | **Wire and framing** | a field one byte off; a checksum seeded wrong; a version the controller refuses | **yes, fully** | layout checks against the vendor's own source or the reference implementation (xArm 42, MAVLink 193, Fairino 44); a second client reading the same controller |
| 2 | **Session and state machine** | bring-up order; power-on and brake release; a mode switch the controller ignores; recovering from a fault | **yes, where the maker's software exists** | URSim, DRCF, ARCS, xArm firmware, ArduPilot, Lely's slave: rung 1 to 3. Rung 4 shows only the author's reading |
| 3 | **Units, signs and conventions** | radians read as degrees; a north that is an east; a joint order the vendor documents wrong | **yes** | descriptions cross-checked against the vendor's tables (xArm's `JOINT_LIMITS`); explicit-matrix tests of every frame conversion, and a second implementation reading the same vehicle |
| 4 | **Refusal semantics** | a command acknowledged and then faulted on (F175); a setpoint ignored in the wrong mode without a word; a stop that is a request and not a fact | **yes, and it is where the findings are** | the vendor's software refusing on demand; the driver refusing before it sends what the controller would take badly |
| 5 | **The stop and the fault path** | a latch that outlives a session (F76); a stop sent once on a lossy link; a release that resumes motion | **the logic, yes; the latency, no** | `skynet driver check`'s second session; a stop measured as a collapse in a physics simulator (Unitree); the resend loop of a software stop |
| 6 | **Physics and gains** | a `kp` that stood a simulated joint and buckles a real one; a hold that goes limp (F121) | **partly** | only where the maker's simulator carries the robot's own control law and a real model (Unitree, franka-sim, AgiBot). Nothing tunes a gain |
| 7 | **Timing at rate, jitter** | a 1 kHz stream that arrives in bursts (F124); a servo that stalls on one late sample (F171) | **no** | a simulator's clock is the host's. Faults can be *injected* and refusals checked; a rate cannot be measured |
| 8 | **Version drift** | the maker ships a release and the driver stops working | **yes, by re-running** | the nightly substrate workflow at pinned and maker-latest versions |
| 9 | **Electrical, thermal, mechanical** | brakes, cables, temperature, wear, EMI, encoders that lose count | **no** | nothing |
| 10 | **The site** | payload and tool configuration, the safety configuration, network setup, calibration, the person | **no** | nothing; §59.6 calls it commissioning and it is human work |

Layers 1 to 5 and 8 are what a driver can be measured on without a robot, and they are where every finding
in section 3 came from. Layers 7 and 9 to 10 are out of reach by nature, and 6 is reachable for a handful of
bodies. **A first physical run can fail on layers this repository cannot see even when every layer it can see
is green** — which is why the release stages stop at `beta` and why `stable` is a record of a run and not a
word.

### The matrix, driver by driver

Each cell is the **strongest rung of evidence** a driver has at that layer (F118's: 1 the maker's controller software, 2 the maker's
simulator, 3 an independent implementation, 4 a counterpart written in this repository), and `·` is a layer nothing reaches, which
`drivers/confidence.json` must excuse in words. Layers 9 and 10 are `·` for every driver and always will be: they are the hardware
and the site, and `tools/check-confidence.mjs` fails if a row claims either. Layer 7 is `·` everywhere until `DRY-COVERAGE-PLAN.md`
item 12.C.5 measures the OS-side timing budget, which is a budget for this host and never a robot's latency. **A cell says which
evidence exists, not that it is good.** *Seeded defects noticed* is the seeding campaign's result for the driver's own suite at its
strongest measured rung ([`dry-test-matrix.md`](dry-test-matrix.md) has the classes, the survivors and how to reproduce it); it measures
the suite and not the robot. *Last seen green* is the most recent day any of a driver's suites exited 0 under `node tools/check-confidence.mjs --verify`, which
runs the suite's command and stamps the date itself, so nobody types it; a suite that failed keeps the date it had, and the nightly
substrate runs are recorded on the runner, not here, so a driver whose suites were not re-run says so.

<!-- generated:confidence -->
| Driver | Stage | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | Seeded defects noticed | Last seen green |
|---|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|---|---|
| `booster` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | — | not re-run |
| `fairino` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | — | not re-run |
| `lite3` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | — | not re-run |
| `unitree` | beta | 2 | 2 | 2 | 2 | 2 | 2 | · | 2 | · | · | — | not re-run |
| `x1` | beta | 2 | 2 | 2 | 2 | 2 | 2 | · | 2 | · | · | — | not re-run |
| `ros2_control` | per body | 1 | 1 | 1 | 1 | 1 | · | · | 1 | · | · | — | not re-run |
| `lerobot` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 27/27 (100%) at rung 4 | 2026-09-30 |
| `pudu` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 29/29 (100%) at rung 4 | 2026-09-30 |
| `canopen` | beta | 3 | 3 | 3 | 3 | 3 | · | · | 3 | · | · | 37/37 (100%) at rung 4 | 2026-09-30 |
| `canopen-rs` | beta | 3 | 3 | 3 | 3 | 3 | · | · | 3 | · | · | 68/68 (100%) at rung 4 | 2026-09-30 |
| `xarm` | beta | 1 | 1 | 1 | 1 | 1 | · | · | 1 | · | · | 36/36 (100%) at rung 4 | 2026-09-30 |
| `mavlink` | beta | 1 | 1 | 1 | 1 | 1 | · | · | 1 | · | · | 36/36 (100%) at rung 4 | 2026-09-30 |
| `ethercat` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 30/30 (100%) at rung 4 | 2026-09-30 |
| `ethercat-rs` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 73/73 (100%) at rung 4 | 2026-09-30 |
| `fourier` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 28/28 (100%) at rung 4 | 2026-09-30 |
| `franka` | beta | 3 | 3 | 3 | 3 | 3 | 3 | · | 3 | · | · | 34/34 (100%) at rung 4 | 2026-09-30 |
| `franka-rs` | beta | 3 | 3 | 3 | 3 | 3 | 3 | · | 3 | · | · | 82/82 (100%) at rung 4 | 2026-09-30 |
| `spot` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 80/80 (100%) at rung 4 | 2026-09-30 |
| `ur_rtde` | beta | 1 | 1 | 1 | 1 | 1 | · | · | 1 | · | · | 27/27 (100%) at rung 4 | 2026-09-30 |
| `ur_rtde-rs` | beta | 1 | 1 | 1 | 1 | 1 | · | · | 1 | · | · | 74/74 (100%) at rung 1 | 2026-09-30 |
<!-- /generated:confidence -->

## 3. The evidence that the ladder finds things

Each row is a first contact between a driver and software its author did not write, and what came out of it.
The counterparts written here (rung 4) agreed with the drivers every time.

| Substrate | First contact found | Findings |
|---|---|---|
| URSim (UR's controller software) with `ur_rtde` | three properties of the controller that would not have been guessed | F27 |
| URSim with the `ros2_control` bridge and UR's own plugin | four bridge defects and a plugin that crashes on a mode switch | F93–F98 |
| `unitree_mujoco` (Unitree's simulator) | the hold went limp and folded a standing Go2; a `close()` that aborted one run in two; a torque sensor wired to the neighbouring motor in the vendor's own H1-2 model | F121–F123 |
| `franka-sim` (an independent FCI server with physics) | a Python-paced 1 kHz stream crosses the FCI's discontinuity limits on timing alone | F124 |
| DRCF (Doosan's controller emulator) | the bridge handed every plugin a 1 ms period; a `velocity` interface that is not a velocity mode; a standing-zero velocity; a version read inside `assert()` | F125–F128 |
| EGM (PickNik's client, a controller built from ABB's `egm.proto`) | ABB's description gives ranges its own limits contradict | F131 |
| `agibot_x1_infer` (AgiBot's simulator) | a joystick module that segfaults the sim; the build needed patches nobody had committed | F132–F133 |
| KUKA's `kuka_rsi_simulator` | a request/reply protocol the bridge wedged on; a stop flag sent only in `on_deactivate`; `open()` succeeding on a plugin that never initialised; a plugin that refuses its own maker's descriptions | F135–F138 |
| ARCS (AUBO's controller software) | a controller that waits forever without real-time scheduling; a plugin that activates on a refused arm; a component that read OK for the rest of a session after failing once | F139–F144 |
| URSim's six models | a jitter period read as a stall | F171 |
| Lely's CiA 402 slave | a drive that takes time to change state was refused as broken | F174 |
| UFACTORY's xArm firmware | a target the controller acknowledges and then faults on; ranges rounded to a milliradian; joint ranges on four of five models wider than UFACTORY's own table, and two models that start with the friction file invalid | F175–F176, F181 |
| ArduPilot's flight code | a yaw rate of zero read as "drive straight" by ArduRover; see [`drivers/mavlink/README.md`](../drivers/mavlink/README.md) | F179 and the MAVLink findings there (F178 is a specification gap the same run exposed) |

Thirteen first contacts, at least one finding each, and none of them found by a counterpart written here. **The
drivers that have not met one are the `alpha` rows, and the honest prior for each is that it carries a defect of
this kind**: Booster, FAIRINO, the Lite3, Fourier, Spot, Pudu, LeRobot, EtherCAT, and the ten `ros2_control`
bodies that run against `mock_components`.

## 4. By class of robot

Scope, by the owner's ruling of 2026-09-29: arms, cobots, quadrupeds, humanoids, drones, ground bases, hands and
grippers, servo drives and service robots. Exoskeletons, surgical robots, prostheses, consumer and home robots
and machines that are not robots (CNC, 3D printers, PLC cells) are **deferred by that ruling, not ruled out**;
section 5 says what exists for each so the decision can be revisited with evidence.

Every name in the **Not driven** column below has a row in section 5, and `node tools/robot-support.mjs --check`
fails when one does not.

| Class | Driven today | Not driven | Why, and what would change it |
|---|---|---|---|
| **Industrial arms** | UR, ABB IRB 1200 (EGM), KUKA KR 6 (RSI), Doosan ×9, AUBO, Kawasaki, DENSO, Mitsubishi, Stäubli, FANUC LR Mate, Elite, Kinova Gen3, Techman, xArm, Franka | Yaskawa, Epson, Comau, Hyundai, JAKA, Dobot, Rokae, Flexiv, Neuromeka, Rainbow, Kassow, KUKA iiwa, Kinova Gen2/JACO2, Baxter/Sawyer | section 5, row by row. Most have a documented protocol; most lack a free controller emulator |
| **Cobots** | as above | as above | as above |
| **Educational and research arms** | SO-100/101 and the robots LeRobot's `Robot` class supports (alpha) | Koch, OpenMANIPULATOR, Interbotix ViperX/WidowX, AgileX Piper, ARX | most speak Dynamixel or CAN; section 5 |
| **Quadrupeds** | Unitree Go2, Deep Robotics Lite3, Boston Dynamics Spot | ANYmal, Ghost Vision 60, Xiaomi CyberDog, Unitree Go1/A1, Unitree B2, Mini Cheetah/Solo-12 | Ghost is access-gated; ANYbotics sells to customers; Go1/A1 use the older `unitree_legged_sdk`; the B2 is on SDK2 and in Unitree's own simulator |
| **Humanoids** | Unitree G1/H1/H1-2, Booster T1, Fourier GR-1, AgiBot X1 | Digit, Cassie, Optimus, Figure, 1X NEO, Apollo (sim only), Kepler, UBTech, Reachy 2, Reachy Mini, Atlas, Robonaut, iCub | section 5 |
| **Aerial** | ArduPilot multirotors and fixed wings | PX4, DJI, Parrot, Crazyflie, Tello, Skydio | PX4 is a second mode vocabulary on the same protocol; the others have vendor SDKs, and section 5 says which have a simulator that speaks them |
| **Ground vehicles** | ArduPilot rovers | MiR, AgileX Scout/Hunter, Clearpath, TurtleBot, Create 3, Hello Robot Stretch, Boston Dynamics Stretch, Locus, TIAGo | no driver yet; ROS 2 bases would ride one generic driver if a base were a `ros2_control` system |
| **Marine** | nothing | ArduPilot boats, ArduPilot submarines, BlueROV2 | §5 had no base kind for water ([`issues/no-base-kind-for-water.md`](issues/no-base-kind-for-water.md), F178); the owner ratified the amendment on 2026-09-29 and carrying it is `DRY-COVERAGE-PLAN.md` item 12.G.1 |
| **Hands and grippers** | via the arm's driver, where the gripper is a joint | Robotiq, OnRobot, Schunk, Inspire, Allegro, LEAP Hand, Shadow, Barrett, tendon-driven MJCF grippers | the MuJoCo driver carries joint-driven actuators only (`drivers/mujoco/README.md`: a tendon, site or body actuator has no joint index to give it); the others are protocols |
| **Servo drives** | any CiA 402 drive on CAN (`canopen`) or EtherCAT (`ethercat`) | Dynamixel, Feetech, moteus, ODrive, VESC, Modbus servo drives, Maxon EPOS2 | each is a protocol; the CiA 402 profile covers most industrial drives, and one emulator per bus would cover the hobby and research ones |
| **Service robots** | Pudu (cloud API) | Temi, Keenon, Bear Robotics | each is a vendor cloud or an Android SDK; section 5 |
| **Deferred by the owner** | nothing | exoskeletons, surgical robots, prostheses, vacuums, Sphero, Anki Vector, Misty, CNC, 3D printers, PLC cells | out of scope for now (2026-09-29); several have public interfaces and free simulators, so this is a scoping decision and not a technical limit |

## 5. The claims that a driver cannot be written, challenged

The specification and the plan say in several places that a robot *cannot* be reached, and the first version of
this page promised a table and never wrote one. This is that table. Each claim is re-checked against the public
record on 2026-09-29. A verdict is one of:

* **overturned** — the claim was wrong; the evidence is named;
* **route found** — a way exists and is not built yet; the row says which and at what rung of F118's ladder;
* **narrowed** — the claim was too broad;
* **stands** — with dated evidence, and what would change it;
* **owner** — blocked on an account, a licence or a purchase this repository may not make
  (`DRY-COVERAGE-PLAN.md` *Decisions* 23(c));
* **deferred** — outside the scope the owner set on 2026-09-29.

Evidence marked *(web)* was read from a public page on 2026-09-29 by a research pass that did not build or run
anything; it is a lead until a driver has met it, and a row is never promoted on it alone. Nothing here says a
driver works.

The first group is not about robots without a driver but about drivers that are `alpha`, because the claim that
no better counterpart exists is the same kind of claim.

| Robot or claim | Verdict | Evidence, and what would change it |
|---|---|---|
| **Booster T1** (its simulator is behind a Feishu login) | overturned, route found | The community mirror `socrob/booster_webots_sim` is a public Docker image with Webots and `sim_control`, no login (rung 2) *(web)*; its README says "internal use only". Run under the owner's ruling of 2026-09-29 (public means run), hash-pinned, in a container, never committed. The official wiki page does redirect to a login, and that route stays the owner's. |
| **Deep Robotics Lite3** (no simulator) | route found | Their `Lite3_rl_deploy` MuJoCo simulator speaks raw floats over UDP (ports 20001 and 30010), not MotionSDK; a translator between the two is small (rung 2−) *(web)*. Its byte layout was not read. |
| **Boston Dynamics Spot** (no simulator of any kind) | stands, for the gRPC API | Boston Dynamics ships none, and Isaac Lab's tracker (#1267) has no bridge to the high-level API *(web)*. The stand-in should be rebuilt from the public protos and validated with the real `bosdyn-client` and `spot_ros2`; it stays `alpha`. |
| **Fourier GR-1** (simulators expose no RoCS endpoint) | route found, unconfirmed | `fourier_aurora_sdk` lists a MuJoCo simulator and `fourier-grx-client` a server *(web)*; whether either speaks the transport this driver uses, and whether the GR-1 (not the GR-1P/GR-2) is covered, is not established. |
| **LeRobot bridge** (the library has never run under it) | route found | LeRobot ships `MockRobot` and mock Feetech and Dynamixel serial buses (`tests/mocks`; pty-based, so WSL or Docker) *(web)*: the real `Robot` classes under the bridge is interface-level rung 3, wire level stays rung 4 until an emulator with register state exists. |
| **EtherCAT** (every slave was written here) | route found | KickCAT (CeCILL-C) is an independent software slave and network emulator *(web)*; whether SOEM or the IgH master can attach to it is unverified, and the effort is high. |
| **The ten mock-only `ros2_control` bodies and the IRB 1200** (the vendor's plugin has not run) | owner, route found for one | Stäubli: a ROS-Industrial `simple_message` server (`industrial_core`, `simple_message_py`) is an independent server (rung 3, no account) *(web)*. FANUC, Kawasaki, DENSO, Mitsubishi, Techman and ABB need an account or a licence for rung 1; each is listed in `DRY-COVERAGE-PLAN.md` item 12.H. |
| **FAIRINO FR3** (SimMachine is behind a registration wall) | owner | Registration is the owner's. |
| **Pudu** (a developer account) | owner | The account is the owner's. |
| **FANUC, KUKA, ABB, older Universal Robots** ("always Guest") | overturned | §59.2 already says a licensed external-motion option makes them joint-level (F85); the README still said otherwise. FANUC's own documentation says its ROS 2 driver runs against ROBOGUIDE virtual robots on localhost (rung 1) *(web)*; the trial needs the owner's FANUC account. |
| **Legacy program-level controllers** (no streaming option) | narrowed | The class is real. ROS-Industrial's streaming (10 to 50 Hz) gives joint streaming without the licensed option *(web)*, which is below the 50 to 1000 Hz §59.2 assumes; the boundary between Guest and Host for those needs a note in the specification, not a driver. |

The rest are robots with no driver, in the order of section 4.

| Robot or claim | Verdict | Evidence, and what would change it |
|---|---|---|
| **Yaskawa** ("hardware-gated: controller and MotoPlus option") | overturned, route found | The claim (2026-09-20) was about MotoROS2 only. HSES over UDP 10040 is public, and `moto-hses-mock` (Rust, Apache-2.0, no account) is an independent server (rung 3) *(web)*. MotoSim keys come only from a regional request: owner. |
| **Epson** ("no manipulator ROS 2 driver found") | route found, owner for the option | Remote Control over TCP is documented; Epson's ROS 2 driver uses RTMC and needs the paid Real-time Motion Control option *(web)*. Whether the RC+ simulator serves either interface is unverified. |
| **Comau** ("blocked") | overturned, route found | F116 lifted the licence block on 2026-09-22. CRCOpen over UDP is documented under an agreement that is the owner's to sign; roboSim is by request *(web)*. |
| **Hyundai** | route found | Hi6 has an HTTP Open API and a BSD-3 ROS 2 driver (Hi6 and Hi7 only); HRSpace's virtual controller is a direct download, and whether it serves that API is unverified (rung 1?) *(web)*. F108 says to check the SDK and not the wrapper, and this is the SDK. |
| **JAKA** | stands, at wire level | The SDK is binary and the "virtual simulator" address decodes to a hosted host, so an account is likely *(web)*. Would change with a documented wire. |
| **Dobot** | route found (rung 4) | The TCP text protocol on ports 29999/30003/30004 is public and MIT-licensed; there is no mock server *(web)*. |
| **Rokae** | stands | `xCoreSDK` is proprietary and needs authorisation; ROS 2 is community-only *(web)*. Would change with a licence. |
| **Flexiv** | route found, owner for rung 1 | `flexiv_rdk` is Apache-2.0; Elements Studio runs the real control software, obtained by contacting Flexiv *(web)*. |
| **Neuromeka** | route found (rung 4) | IndyDCP3 is a public gRPC API; how to obtain the simulator is unclear *(web)*. |
| **Rainbow** | route found (rung 4) | Text over TCP 5000/5001 and an Apache-2.0 client; the "simulation" is a mode of a real controller, not an emulator *(web)*. |
| **Kassow** | stands | KORD is a UDP stream at 250 Hz, but its wire is not documented in the public repositories found and no simulator turned up *(web)*. |
| **KUKA iiwa** | route found, owner for rung 1 | FRI over UDP and protobuf; the client is bundled in `lbr_fri_ros2_stack` with a mock. The FRI server is Sunrise.FRI on the controller, so a stand-in is written from the bundled protobuf (rung 4) *(web)*. |
| **Kinova Gen2/JACO2** | stands | The API is closed and USB-only *(web)*. The Gen3 is driven (`ros2_control`, alpha); its Kortex gRPC API is public and a mock can be written from the `.proto`. |
| **Baxter/Sawyer** | narrowed | The vendor's Gazebo simulators are ROS 1 only and `ros1_bridge` does not run on Ubuntu 24.04 *(web)*. Rung 2 exists in a Noetic container; low value for discontinued robots. |
| **Koch, OpenMANIPULATOR, Interbotix ViperX/WidowX** | route found | All Dynamixel. One stateful Dynamixel protocol 2.0 emulator (rung 4, from LeRobot's packet builders) covers them and the LeRobot bridge; Interbotix's own `xs_sdk_sim` is rung 2 at the ROS level *(web)*. |
| **AgileX Piper, ARX** | route found (rung 4) | SocketCAN protocols with MIT-licensed SDKs (`piper_sdk`, `arx5-sdk`); no emulator, so a vcan stand-in *(web)*. |
| **ANYmal** | owner | The SDK is documented through ANYbotics' support and a Siemens subscription; no public simulator *(web)*. Four ANYmal descriptions already run in the fleet sweep. |
| **Ghost Vision 60** | stands | Access-gated (2026-09-22, F116) and nothing public found *(web)*. |
| **Xiaomi CyberDog** | route found, low value | `cyberdog_ros2` is on ROS 2 Galactic (end of life); the simulator is Gazebo plus LCM and does not exercise the high-level API *(web)*. |
| **Unitree Go1/A1** ("the older SDK") | route found (rung 4) | `unitree_legged_sdk`'s UDP protocol is public in its headers, and `free-dog-sdk` is an independent client to cross-check the codec against; `unitree_mujoco` does not cover them *(web)*. |
| **Unitree B2** | overturned | The claim put it with the older SDK, and the plan's Phase 7 table calls a B2 "built" (`DRY-COVERAGE-PLAN.md` :5556) while `drivers/unitree` has no B2. It is on SDK2: F119 already read the B2's wiring in `unitree_mujoco` and found it clean (:5907), and that simulator also lists b2w, go2w, h2 and as2 *(web)*. So these are rows for the existing driver at rung 2 once joint tables are added. |
| **Mini Cheetah/Solo-12** | route found, low value | Mini Cheetah's stack talks LCM and shared memory, not a network API; Solo-12's ODRI master board uses raw MAC frames and no simulator speaks that wire *(web)*. |
| **Digit** | route found (rung 1), owner for provenance | Agility's `ar-control` simulator binary is re-hosted at `GTLIDAR/digit_controller` (public; proprietary licence, API "separate access") *(web)*. Run under the owner's ruling of 2026-09-29; Agility's own copy is the owner's to ask for. |
| **Cassie** | route found (rung 2) | `cassie-mujoco-sim` speaks a UDP protocol and needs no key; ports and packet layout were not read *(web)*. Discontinued robot. |
| **Optimus, Figure, 1X NEO, Kepler** | stands | No public SDK or interface (F115, 2026-09-22; searched again 2026-09-29) *(web)*. Would change with a maker publishing one. |
| **Apollo (sim only)** | narrowed | The *model* is in the tree (`drivers/apollo`, F113); Apptronik's hardware interface is not public. |
| **UBTech** | route found for one | The Yanshee SDK is public (`Yanshee-Raspi-SDK`); Walker and UGOT interfaces were not found *(web)*. |
| **Reachy 2, Reachy Mini** | narrowed | §66 already lists Reachy 2 under the LeRobot bridge. Pollen ships a Docker simulator and `reachy2_mujoco`; Reachy Mini's daemon has a `--sim` MuJoCo mode behind its REST API (rung 1) *(web)*. |
| **Atlas, Robonaut** | stands | No public interface *(web)*. |
| **iCub** | route found | YARP with `gazebo-yarp-plugins` "same API as the real robot" (rung 2) *(web)*. |
| **PX4** | route found (rung 1) | SIH is headless and free (`make px4_sitl_sih`), and PX4 speaks MAVLink: the driver's codec and state machine are reused; differences are UDP 14540, the mode vocabulary and offboard's setpoint stream *(web)*. |
| **DJI** | owner | The Onboard and Payload SDKs' simulator needs the flight controller on USB and a developer account *(web)*. Would change with either. |
| **Parrot** | route found (rung 1) | Sphinx runs the real firmware, free, but needs a GPU and Ubuntu 20.04/22.04; headless is unverified *(web)*. |
| **Crazyflie** | route found (rung 1) | `CrazySim` (GPL-3.0) runs modified real firmware behind cflib's UDP link, no account *(web)*. |
| **Tello** | route found (rung 3) | Four independent emulators speak the UDP SDK on port 8889 *(web)*. |
| **Skydio** | owner | A cloud REST API needing an account and token, and no simulator *(web)*. |
| **MiR** | route found (rung 4) | A REST API and an `osrf` client; no maker simulator *(web)*. |
| **AgileX Scout/Hunter** | route found | `ugv_sdk` speaks CAN; a Gazebo simulator exists over ROS topics (rung 2) *(web)*. |
| **Clearpath** | route found (rung 2) | Free ROS 2 Gazebo simulation, no account; how many models it covers today is unclear *(web)*. |
| **TurtleBot, Create 3** | route found (rung 2) | Public ROS 2 simulators (`turtlebot3_simulations`, `create3_sim`); the real robots use Twist and odometry rather than `ros2_control` *(web)*. |
| **Hello Robot Stretch** | route found (rung 2) | `stretch_mujoco`'s API is "portable between sim and physical" *(web)*. |
| **Boston Dynamics Stretch, Locus** | stands | No public interface found *(web)*. |
| **TIAGo** | route found (rung 2) | Public ROS 2 Gazebo simulation, Docker image, no account *(web)*. |
| **ArduPilot boats, ArduPilot submarines, BlueROV2** | route found (rung 1), blocked on the specification | ArduSub and the boat frame run in SITL (`sim_vehicle.py`, free) *(web)*; the MAVLink driver already refuses both by name because §5 had no water base kind (F178, ratified 2026-09-29; carrying it is item 12.G.1). |
| **Robotiq, OnRobot, Inspire** | route found (rung 4, rung 3 transport) | Modbus RTU/TCP; `pymodbus` and `diagslave` serve the transport, the register maps are the vendors' *(web)*. |
| **Schunk** | route found (rung 2) | `schunk_gripper_dummy`, the maker's own protocol simulator for the EGU/EGK, in their ROS driver *(web)*; which interfaces it mimics is unverified. |
| **Allegro, LEAP Hand** | route found (rung 4) | SocketCAN (Allegro) and Dynamixel (LEAP); no emulator *(web)*. |
| **Shadow** | stands | A closed EtherCAT profile with a ROS 1 interface *(web)*. |
| **Barrett** | stands, low value | `libbarrett` on ROS 1, no simulator *(web)*. |
| **Tendon-driven MJCF grippers** | narrowed | The limit is in the MuJoCo *driver* (a tendon actuator has no joint index), not in the robot; `drivers/mujoco/README.md`. Would change with a spec amendment for non-joint actuators. |
| **Dynamixel, Feetech** | route found (rung 4) | Protocol 2.0 is published; no maintained third-party emulator, and LeRobot's mocks match canned packets rather than hold register state. `feetech-cli` has a stateful in-process register model *(web)*. |
| **moteus** | route found, unverified | mjbots compiled the firmware's control loop for the host against a motor model (2026-06) *(web)*; whether it exposes the CAN-FD interface is unknown. |
| **ODrive, VESC** | route found (rung 4) | CANSimple is public; neither has a firmware simulator on that interface *(web)*. |
| **Modbus servo drives** | route found (rung 3 transport) | `pymodbus`'s simulator and `diagslave` *(web)*; the register map is the drive's. |
| **Maxon EPOS2** | narrowed | Not CiA 402 compliant while the EPOS4 is *(web)*; a quirk to model, not a barrier. |
| **Temi, Keenon, Bear Robotics** | owner | Android or partner-portal SDKs with no off-robot simulator (2026-09-20 plan; `temi` on Android over `adb`) *(web)*. Nothing to build without an account or a unit (*Decisions* 18). |
| **Exoskeletons, surgical robots, prostheses** | deferred | The old claim "regulated devices with no public interface" was too broad: dVRK (ROS 2, CRTK, simulators) and Raven II run without hardware, and open exoskeleton and prosthesis projects have public code *(web)*. Deferred by the owner on 2026-09-29; a body-contact safety class the specification does not have would be needed first. |
| **Vacuums, Sphero, Anki Vector, Misty** | deferred | Community stacks exist (Valetudo has a built-in mock robot; `wire-pod`; Misty's API explorer) *(web)*. Deferred by the owner on 2026-09-29. |
| **CNC, 3D printers, PLC cells** | deferred | LinuxCNC, GRBL, Marlin and Klipper all ship simulators, and OpenPLC or an OPC UA server stands for a PLC *(web)*. Deferred by the owner on 2026-09-29; each can hurt a person and would need its own limits. |

**What the specification itself blocks**, as distinct from what nobody has written: water base kinds (F178) and
`flex`/`pitch` as one hinge (F177), both ratified on 2026-09-29 and not yet carried; scalar joints only (a ball or
floating joint, or a tendon, cannot be described); command-level bodies cap at Guest (§59.2); one robot per host;
simulators and Python drivers cap at Guest. None of these is a reason a driver cannot be written.

<!-- generated:details -->
#### `booster` — humanoid, joint-level

**alpha.** a simulated T1 written in this repository, with Booster's own SDK on both ends of the wire. Booster's simulator is behind a login this repository has no account for

Not shown by that counterpart:

* Booster's own simulator and controller: they sit behind a login this repository has no account for, so the DDS service the driver speaks has only ever been Booster's SDK talking to itself
* the joints' response, because the counterpart integrates a first-order model: gains, falls and CUSTOM-mode transitions are unseen
* timing at rate, and the robot's own safety layers

#### `fairino` — cobot arm, joint-level

**alpha.** a simulated FR controller written in this repository, driven by FAIR INNOVATION's own FRRobot client. The maker's SimMachine simulator is a Windows virtual machine behind a registration wall, and has not been run

Not shown by that counterpart:

* FAIR INNOVATION's own controller software: their SimMachine is a Windows virtual machine behind a registration wall and has not been run; the vendor's FRRobot client is a second opinion on the wire, not on the controller
* servo timing, and how the controller reacts to a collision or a protective stop

#### `lite3` — quadruped, joint-level

**alpha.** a simulated motion host written in this repository, driven by Deep Robotics' own Sender and Receiver. No Lite3 simulator exists, and four command codes are read out of the vendor's compiled library

Not shown by that counterpart:

* any Lite3 controller: Deep Robotics' own MuJoCo simulator speaks a private UDP layout, not MotionSDK's, so the motion host on the other end was written here
* four command codes were read out of the vendor's compiled library and have not been confirmed by the vendor
* physics, gains, and the vendor's motion-host modes

#### `unitree` — quadruped and humanoids, joint-level

**beta.** Unitree's own simulator, unitree_mujoco, unmodified, for the Go2, H1, G1 and H1-2: stand, motion, hold and stop in physics

Not shown by that counterpart:

* the real motor drivers: Unitree's simulator applies the impedance law in physics but does not check the CRC or the motor mode
* timing at 500 Hz, the robots' own protection layers, and how a real robot recovers from a fall

#### `x1` — humanoid, joint-level

**beta.** AgiBot's own agibot_x1_infer deployment and its MuJoCo sim, over the robot's own ROS 2 topics: a live elbow moved and conformance against physics

Not shown by that counterpart:

* roles are left to an operator, so the body binds no cross-robot policy until a person asserts them
* the real robot's actuator timing, and the drift a latched hold prevents (AgiBot's simulation was not rebuilt after that fix)

#### `ros2_control` — arms, hosted

Stage per body (each body runs a different vendor's plugin, so each has its own):

* **Universal Robots UR5e, through UR's own ur_robot_driver** — **beta**. Universal Robots' own ur_robot_driver plugin against URSim, UR's controller software: the arm woken, the elbow moved and brought back
* **Doosan M0609** — **beta**. Doosan's own dsr_hardware2 plugin against DRCF, Doosan's controller emulator, started as the M0609 (its own log says so): the elbow moved at the plugin's period and the stop latched
* **Doosan M0617** — **beta**. Doosan's own dsr_hardware2 plugin against DRCF, Doosan's controller emulator, started as the M0617 (its own log says so): the elbow moved at the plugin's period and the stop latched
* **Doosan M1013** — **beta**. Doosan's own dsr_hardware2 plugin against DRCF, Doosan's controller emulator, started as the M1013 (its own log says so): the elbow moved at the plugin's period and the stop latched
* **Doosan M1509** — **beta**. Doosan's own dsr_hardware2 plugin against DRCF, Doosan's controller emulator, started as the M1509 (its own log says so): the elbow moved at the plugin's period and the stop latched
* **Doosan A0509** — **beta**. Doosan's own dsr_hardware2 plugin against DRCF, Doosan's controller emulator, started as the A0509 (its own log says so): the elbow moved at the plugin's period and the stop latched
* **Doosan A0912** — **beta**. Doosan's own dsr_hardware2 plugin against DRCF, Doosan's controller emulator, started as the A0912 (its own log says so): the elbow moved at the plugin's period and the stop latched
* **Doosan E0509** — **beta**. Doosan's own dsr_hardware2 plugin against DRCF, Doosan's controller emulator, started as the E0509 (its own log says so): the elbow moved at the plugin's period and the stop latched
* **Doosan H2017** — **beta**. Doosan's own dsr_hardware2 plugin against DRCF, Doosan's controller emulator, started as the H2017 (its own log says so): the elbow moved at the plugin's period and the stop latched
* **Doosan H2515** — **beta**. Doosan's own dsr_hardware2 plugin against DRCF, Doosan's controller emulator, started as the H2515 (its own log says so): the elbow moved at the plugin's period and the stop latched
* **ABB IRB 1200, over EGM** — **alpha**. PickNik's abb_hardware_interface over EGM against a simulated controller written in this repository from ABB's own egm.proto. ABB's virtual controller (RobotStudio) has not been run
* **ABB IRB 120** — **alpha**. ros2_control's mock_components with the vendor's description; ABB's plugin has not run
* **AUBO i5, through AUBO's own plugin** — **beta**. AUBO's own AuboHardwareInterface plugin against ARCS, AUBO's controller software, with AUBO's simulated arm: the arm powered on by AUBO's own SDK example, the elbow moved and brought back, and a session against the unpowered arm refused rather than reported done. One line of the plugin is patched to build on Jazzy (an include Jazzy no longer ships)
* **AUBO i5S** — **alpha**. ros2_control's mock_components with the vendor's description; AUBO's plugin has run only against the i5 (ARCS has no i5S model)
* **DENSO VS-060** — **alpha**. ros2_control's mock_components with the vendor's description; DENSO's plugin has not run
* **Elite CS520H** — **alpha**. ros2_control's mock_components with the vendor's description; Elite's plugin has not run
* **FANUC LR Mate 200iD** — **alpha**. ros2_control's mock_components with the vendor's description; FANUC's plugin has not run
* **Kawasaki RS007L** — **alpha**. ros2_control's mock_components with the vendor's description; Kawasaki's plugin has not run
* **Kinova Gen3** — **alpha**. ros2_control's mock_components with the vendor's description; Kinova's plugin has not run
* **KUKA KR 6 R900 sixx, over RSI** — **beta**. KUKA's own KukaRSIHardwareInterface over RSI against KUKA's own kuka_rsi_simulator, which answers the wire from the controller's side with a scripted plant — every 40 ms it sends the axes and applies the reply's correction, and checks nothing else: the pose read in degrees, A3 moved and brought back, every session ended by RSI's stop flag. No KUKA controller (OfficeLite, a KRC) has been run
* **Mitsubishi MELFA RV-4FR** — **alpha**. ros2_control's mock_components with the vendor's description; Mitsubishi's plugin has not run
* **Stäubli TX2-60L** — **alpha**. ros2_control's mock_components with the vendor's description; Stäubli's plugin has not run
* **Techman TM7S** — **alpha**. ros2_control's mock_components with the vendor's description; Techman publishes no ros2_control plugin

Not shown by that counterpart:

* for each body whose stage is alpha, everything the vendor's plugin does: those run against ros2_control's mock components with the vendor's description, or (the ABB IRB 1200) against a controller written here
* for every body: timing at rate (the bridge hosts a ResourceManager in-process and the counterparts keep simulated time), and the controller's own safety reactions

#### `lerobot` — educational arms, joint-level

**alpha.** a simulated robot written in this repository behind LeRobot's robot interface; LeRobot's own library has not been run under this bridge

Not shown by that counterpart:

* LeRobot's own library has never been run under this bridge: the counterpart is a robot written here behind LeRobot's interface
* any motor bus (Feetech, Dynamixel): the bridge never touches one

#### `pudu` — service robot, command-level

**alpha.** a simulated Pudu cloud written in this repository, the request signing held to the vendor's own construction. Pudu's real cloud needs a developer account

Not shown by that counterpart:

* Pudu's real cloud: it needs a developer account, and the account is an owner action
* there is no cancel for a delivery in the API, so a stop cannot recall a task in flight (documented in the driver)

#### `canopen` — servo drives, joint-level

**beta.** ros2_canopen's CiA 402 slave on Lely's CANopen stack, an implementation of CiA 301 and 402 this repository did not write, on a kernel virtual CAN bus: units read off the drive, the enable walk, motion, the stop and its release, and the refusal of a drive with no gear ratio or another PDO mapping. It is a protocol counterpart and not a vendor's drive

Not shown by that counterpart:

* any vendor's drive: Lely's CiA 402 slave is a protocol counterpart with mock motion
* drive quirks, bus load and timing, homing methods, brake control

#### `canopen-rs` — servo drives, joint-level

**beta.** the same: ros2_canopen's CiA 402 slave on Lely's CANopen stack, on a kernel virtual CAN bus, through the conformance suite and the slave's own account of each stop

Not shown by that counterpart:

* any vendor's drive, as for the Python driver
* the kernel-bus half needs vcan; a machine without the module reports it not run

#### `xarm` — cobot arm, joint-level

**beta.** UFACTORY's own controller firmware (2.4.0, in the simulator image their support article names), started as each of the five arms the image carries — the xArm 5, xArm 6, Lite 6, 850 and xArm 7: motion, the refusal of a target the firmware acknowledges and then faults on, the stop and its release, with UFACTORY's own XArmAPI reading the same controller as a second opinion. The firmware's joint ranges are wider than the vendor's own table on four of the five, so the driver's travel is the narrower of the two

Not shown by that counterpart:

* motors: the firmware runs with none, so no dynamics, brakes or thermal state
* the firmware's joint ranges are wider than the vendor's own table on four of the five models, so travel is the narrower of the two: a real controller's ranges have not been read
* velocity and torque are not offered, and the controller's collision detection is neither configured nor relied on

#### `mavlink` — aerial and ground vehicles, command-level

**beta.** ArduPilot's own flight code built for x86 (SITL, 4.7.1 at a pinned commit): a multirotor, a rover and a fixed wing armed, flown and stopped through the real ABI, with the reference MAVLink implementation reading the same vehicle on a second serial port and the link impaired with delay, jitter and loss. It is the firmware on a simulated board and not a flight controller: no motors, no sensor noise, no radio. PX4, boats and submarines are refused by name

Not shown by that counterpart:

* a real flight controller: SITL is the firmware on a simulated board, with no sensor noise or failure, no motors and no radio link
* PX4 (refused by name), boats and submarines (refused by name: §5 has no base kind for water)
* the vehicle's failsafes in the field, the EKF on real sensors, and everything about a real battery

#### `ethercat` — servo drives, joint-level

**alpha.** simulated EtherCAT slaves written in this repository, over a loopback segment. SOEM, an independent master, was run against those slaves to check them, not this driver

Not shown by that counterpart:

* every slave either master has met was written here; SOEM, an independent master, was run against those slaves and not against this master
* distributed clocks, jitter, and a real network interface

#### `ethercat-rs` — servo drives, joint-level

**alpha.** the Python EtherCAT driver's simulated slaves, written in this repository, over a virtual Ethernet pair

Not shown by that counterpart:

* as for the Python driver: no slave or master this repository did not write

#### `fourier` — humanoid, joint-level

**alpha.** a simulated RoCS server written in this repository, also driven by Fourier's own client. Fourier's simulators drive the model directly and expose no RoCS

Not shown by that counterpart:

* Fourier's simulators drive the model directly and expose no RoCS endpoint, so the server is written here (Fourier's own client drives it)
* roles are left to an operator, and the real robot's response is unseen

#### `franka` — arm, joint-level

**beta.** franka-sim, an independent implementation of Franka's FCI server with MuJoCo physics: motion, hold, velocity, zero torque and the stop

Not shown by that counterpart:

* a real Control box: franka-sim is a third party's implementation of the FCI server with physics
* 1 kHz timing: a Python-paced stream crosses the FCI's discontinuity limits on timing alone (F124), which is why the Rust port is the Host-tier one

#### `franka-rs` — arm, joint-level

**beta.** franka-sim, an independent implementation of Franka's FCI server with MuJoCo physics, through the conformance suite

Not shown by that counterpart:

* a real Control box, and 1 kHz timing against one: franka-sim keeps simulated time

#### `spot` — quadruped, command-level

**alpha.** simulated Spot services written in this repository, driven by Boston Dynamics' own bosdyn-client. No Spot simulator of any kind exists

Not shown by that counterpart:

* no Spot simulator exists: the services are written here and driven by Boston Dynamics' own client
* lease, power and e-stop behaviour on a real robot, and its fault vocabulary

#### `ur_rtde` — cobot arm, joint-level

**beta.** URSim, Universal Robots' own controller software, as every model it emulates (UR3e, UR5e, UR10e, UR16e, UR20, UR30): every byte the driver sends, power-on, motion and the stop

Not shown by that counterpart:

* servo timing: URSim's clock is the host's (F171 found a stall at one jitter period)
* protective stops, the safety system, payload and TCP configuration, CB3 arms, and any URCap the site installs

#### `ur_rtde-rs` — cobot arm, joint-level

**beta.** URSim, Universal Robots' own controller software: the live test and the conformance suite

Not shown by that counterpart:

* as for the Python driver: servo timing, protective stops and the safety system
<!-- /generated:details -->
