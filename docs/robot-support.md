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
* **The one figure this page does give is a detection rate, and it is about the test and not the robot.** For each
  driver it says how many of a set of defects that have really happened to a driver in this tree — a sign, a unit, a
  byte order, a latch that outlives its session, a hold that goes limp, a fault in a flag the OS does not read — the
  strongest dry suite notices when one is seeded in (section 3's matrix, `tools/seed-defects/`). It is always read
  with its denominator and its rung, it says a *class* of defect would have been caught by the counterpart the suite
  runs against and not that none is left, and a defect known to be undetectable is in every set and must survive, so
  that the figure can fall. It replaces the percentage and does not stand in for it: nothing here turns it into a
  chance that a driver works (*Decisions* 25).
* **A `beta` driver's first physical run should get through open, read, hold, one commanded motion and stop**,
  because those were measured against software that speaks the robot's own interface. It is not evidence for
  the servo loop under load, for timing at rate, for the robot's own protective reactions, or for anything a
  person will do with the machine afterwards.
* **The distance from here to "works" is a commissioning project per robot and a bench day per tier claim**
  (§59.6, §15). That does not shrink with more dry testing; the dry testing shrinks the part of it that is
  the driver's fault.

## 1. Installable today

<!-- generated:summary -->
**60 robot rows across 23 drivers that command a physical robot: 40 beta, 20 alpha, 0 stable.** (A row is one robot or one robot family a driver was built for or run against; a driver that hosts other vendors' plugins counts each body.)
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
| Unitree B2 | `unitree` | joint-level | **beta** | 2 |
| Unitree H1 | `unitree` | joint-level | **beta** | 2 |
| Unitree G1 | `unitree` | joint-level | **beta** | 2 |
| Unitree H1-2 | `unitree` | joint-level | **beta** | 2 |
| Unitree H2 | `unitree` | joint-level | **beta** | 2 |
| Unitree R1 | `unitree` | joint-level | **beta** | 2 |
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
| SO-100 and SO-101 (Feetech), Koch v1.0 and v1.1 and OpenMANIPULATOR-X (Dynamixel) — each class run under the bridge — and the other robots LeRobot's Robot class supports | `lerobot` | joint-level | **alpha** | 4 |
| Pudu service robots, through the vendor's cloud OpenAPI (delivery, cruise, call) | `pudu` | command-level | **alpha** | 4 |
| any CiA 402 servo drive on CAN that exposes the objects the driver reads (encoder resolution, gear ratio, limits, speed) | `canopen` | joint-level | **beta** | 3 |
| the same CiA 402 drives, from the Rust port | `canopen-rs` | joint-level | **beta** | 3 |
| UFACTORY xArm 5 | `xarm` | joint-level | **beta** | 1 |
| UFACTORY xArm 6 | `xarm` | joint-level | **beta** | 1 |
| UFACTORY Lite 6 | `xarm` | joint-level | **beta** | 1 |
| UFACTORY 850 | `xarm` | joint-level | **beta** | 1 |
| UFACTORY xArm 7 (each row against UFACTORY's firmware started as that arm) | `xarm` | joint-level | **beta** | 1 |
| ArduPilot multirotors, fixed wings, ground rovers, surface boats and submarines or ROVs (ArduSub), and PX4 multirotors, fixed wings and ground rovers (the driver reads the autopilot and the airframe from the heartbeat) | `mavlink` | command-level | **beta** | 1 |
| TurtleBot3 Burger, in ROBOTIS's own Gazebo simulation, and iRobot Create 3, in iRobot's own (neither robot was run); and any ROS 2 mobile base that takes a geometry_msgs Twist or TwistStamped and publishes nav_msgs Odometry: Clearpath Husky and Jackal, TurtleBot 4, AgileX bases, Hello Robot Stretch's base, PAL's TIAGo base, anything on ros2_control's diff_drive_controller or mecanum_drive_controller. Claimed from each maker's documentation: only the two simulations and ros2_control's own controller were run | `ros2_base` | command-level | **beta** | 3 |
| any robot arm whose controller runs a ROS-Industrial simple_message server: Yaskawa Motoman (MotoROS), ABB (abb_driver's RAPID server), FANUC (fanuc_driver), Staubli and others, up to ten joints. Claimed from ROS-Industrial's and each maker's documentation: no controller was run, only a server built from ROS-Industrial's own library | `ros_industrial` | joint-level | **beta** | 3 |
| Pollen Robotics Reachy Mini, Wireless and Lite: the maker's README says one daemon fronts both and its simulation. Claimed from that README and the daemon's own source; the simulation was run, neither robot was | `reachy_mini` | joint-level | **beta** | 2 |
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
| `booster` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 45/45 (100%) at rung 4 | not re-run |
| `fairino` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 73/73 (100%) at rung 4 | not re-run |
| `lite3` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 55/55 (100%) at rung 4 | not re-run |
| `unitree` | beta | 2 | 2 | 2 | 2 | 2 | 2 | · | 2 | · | · | 83/83 (100%) at rung 4 | not re-run |
| `x1` | beta | 2 | 2 | 2 | 2 | 2 | 2 | · | 2 | · | · | — | not re-run |
| `ros2_control` | per body | 1 | 1 | 1 | 1 | 1 | · | · | 1 | · | · | — | not re-run |
| `lerobot` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | 4 | · | · | 40/40 (100%) at rung 4 | 2026-10-03 |
| `pudu` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 29/29 (100%) at rung 4 | 2026-09-30 |
| `canopen` | beta | 3 | 3 | 3 | 3 | 3 | · | · | 3 | · | · | 46/46 (100%) at rung 4 | 2026-10-01 |
| `canopen-rs` | beta | 3 | 3 | 3 | 3 | 3 | · | · | 3 | · | · | 74/74 (100%) at rung 4 | 2026-09-30 |
| `xarm` | beta | 1 | 1 | 1 | 1 | 1 | · | · | 1 | · | · | 41/41 (100%) at rung 4 | 2026-10-01 |
| `mavlink` | beta | 1 | 1 | 1 | 1 | 1 | · | · | 1 | · | · | 108/108 (100%) at rung 4 | 2026-10-03 |
| `ros2_base` | beta | 2 | 2 | 2 | 2 | 2 | 2 | · | 2 | · | · | 58/58 (100%) at rung 4 | not re-run |
| `ros_industrial` | beta | 3 | 3 | 3 | 3 | 3 | · | · | 3 | · | · | 69/69 (100%) at rung 4 | not re-run |
| `reachy_mini` | beta | 2 | 2 | 2 | 2 | 2 | · | · | 2 | · | · | 73/73 (100%) at rung 4 | not re-run |
| `ethercat` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 33/33 (100%) at rung 4 | 2026-10-01 |
| `ethercat-rs` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 76/76 (100%) at rung 4 | 2026-09-30 |
| `fourier` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 28/28 (100%) at rung 4 | 2026-09-30 |
| `franka` | beta | 3 | 3 | 3 | 3 | 3 | 3 | · | 3 | · | · | 41/41 (100%) at rung 4 | 2026-10-01 |
| `franka-rs` | beta | 3 | 3 | 3 | 3 | 3 | 3 | · | 3 | · | · | 87/87 (100%) at rung 4 | 2026-09-30 |
| `spot` | alpha | 4 | 4 | 4 | 4 | 4 | · | · | · | · | · | 105/105 (100%) at rung 4 | 2026-10-01 |
| `ur_rtde` | beta | 1 | 1 | 1 | 1 | 1 | · | · | 1 | · | · | 38/38 (100%) at rung 4 | 2026-10-01 |
| `ur_rtde-rs` | beta | 1 | 1 | 1 | 1 | 1 | · | · | 1 | · | · | 82/82 (100%) at rung 1 | 2026-09-30 |
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
| **Industrial arms** | UR, ABB IRB 1200 (EGM), KUKA KR 6 (RSI), Doosan ×9, AUBO, Kawasaki, DENSO, Mitsubishi, Stäubli, FANUC LR Mate, Elite, Kinova Gen3, Techman, xArm, Franka; and any arm whose controller runs a ROS-Industrial `simple_message` server (Yaskawa Motoman's MotoROS, ABB's `abb_driver`, FANUC's `fanuc_driver`: `drivers/ros_industrial`, beta, run against a server built from the maker's own library; no controller was run) | Yaskawa, Epson, Comau, Hyundai, JAKA, Dobot, Rokae, Flexiv, Neuromeka, Rainbow, Kassow, KUKA iiwa, Kinova Gen2/JACO2, Baxter/Sawyer | section 5, row by row. Most have a documented protocol; most lack a free controller emulator |
| **Cobots** | as above | as above | as above |
| **Educational and research arms** | SO-100/101, Koch and OpenMANIPULATOR-X, and the robots LeRobot's `Robot` class supports (alpha) | Interbotix ViperX/WidowX, AgileX Piper, ARX | the LeRobot ones are run as shipped on emulated servo buses (rung 4); the others speak Dynamixel through ROS or CAN, section 5 |
| **Quadrupeds** | Unitree Go2, Unitree B2, Deep Robotics Lite3, Boston Dynamics Spot | ANYmal, Ghost Vision 60, Xiaomi CyberDog, Unitree Go1/A1, Mini Cheetah/Solo-12 | Ghost is access-gated; ANYbotics sells to customers; Go1/A1 use the older `unitree_legged_sdk`; the wheeled B2-W and Go2-W (SDK2, in Unitree's own simulator) wait for a rule for a wheel joint |
| **Humanoids** | Unitree G1/H1/H1-2/H2/R1, Booster T1, Fourier GR-1, AgiBot X1 | Digit, Cassie, Optimus, Figure, 1X NEO, Apollo (sim only), Kepler, UBTech, Reachy 2, Atlas, Robonaut, iCub | section 5 |
| **Aerial** | ArduPilot and PX4 multirotors and fixed wings | DJI, Parrot, Crazyflie, Tello, Skydio | PX4 is the second motion set of the same driver, run against PX4's own firmware (rung 1); the others have vendor SDKs, and section 5 says which have a simulator that speaks them |
| **Ground vehicles** | ArduPilot and PX4 rovers; any ROS 2 base that takes a `Twist` or `TwistStamped` and publishes `Odometry` (`drivers/ros2_base`, beta: run against `ros2_control`'s own controller in Jazzy and Humble, and the TurtleBot3 Burger and the Create 3 in their makers' own simulations) | MiR, AgileX Scout/Hunter, Clearpath, TurtleBot, Create 3, Hello Robot Stretch, Boston Dynamics Stretch, Locus, TIAGo | the named bases speak the same two topics by their makers' documentation, and none of them was run: the driver is built for the class and held to the standard controller, which is not any base maker's software (`drivers/ros2_base/README.md`); MiR is REST |
| **Marine** | ArduPilot surface boats and submarines (ArduSub) | ArduPilot submarines, BlueROV2 | §5 had no base kind for water ([`issues/no-base-kind-for-water.md`](issues/no-base-kind-for-water.md), F178); §97 carried it on 2026-10-02 (`DRY-COVERAGE-PLAN.md` item 12.G.1), the driver drove a boat that day and ArduSub's own firmware on 2026-10-03 (item 12.E.4, rung 1): moved by sticks, with a stop the operator names and no default. No named ROV was run, and the simulated submarine's depth is not a measurement |
| **Hands and grippers** | via the arm's driver, where the gripper is a joint; SCHUNK EGU and EGK grippers over their module's web interface (`drivers/schunk`, beta: run against the maker's own protocol simulator and client, with no gripper) | Robotiq, OnRobot, Inspire, Allegro, LEAP Hand, Shadow, Barrett, tendon-driven MJCF grippers | the MuJoCo driver carries joint-driven actuators only (`drivers/mujoco/README.md`: a tendon, site or body actuator has no joint index to give it); the others are protocols |
| **Servo drives** | any CiA 402 drive on CAN (`canopen`) or EtherCAT (`ethercat`); Feetech and Dynamixel through LeRobot's robot classes (rung 4) | moteus, ODrive, VESC, Modbus servo drives, Maxon EPOS2 | each is a protocol; the CiA 402 profile covers most industrial drives, and one emulator per bus would cover the hobby and research ones |
| **Service robots** | Pudu (cloud API); Pollen Robotics Reachy Mini, Wireless and Lite (`drivers/reachy_mini`, beta: run against the maker's own daemon in the maker's own MuJoCo simulator, with no robot) | Temi, Keenon, Bear Robotics | each is a vendor cloud or an Android SDK; section 5 |
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
| **LeRobot bridge** (the library has been run under it for the SO-100/101, the Koch and the OpenMANIPULATOR-X) | **built for those three (rung 4)** | LeRobot's own `SO101Follower`, `KochFollower` and `OmxFollower`, their own `FeetechMotorsBus` and `DynamixelMotorsBus` and the makers' own `scservo_sdk` and `dynamixel_sdk` run under the bridge on stateful pty emulators of six STS3215 and six Dynamixel X-series servos (`drivers/lerobot/sim/feetech_bus.py` and `dynamixel_bus.py`, their control tables read out of LeRobot's `tables.py`): 44 and 68 checks, found F219–F222, F232 (a servo that has shut itself down on an overload read as healthy) and F233 (the stop aborted at the first servo that carried an alert, leaving the rest under power) (`make -C drivers/lerobot check-wire`; nightly in `substrates.yml`, `lerobot-wire`, at the pinned release and at PyPI's newest). `tests/mocks` is not in the pip package. LeKiwi, HopeJR and Reachy 2 have role tables and have not been run. |
| **EtherCAT** (every slave was written here) | route found | KickCAT (CeCILL-C) is an independent software slave and network emulator *(web)*; whether SOEM or the IgH master can attach to it is unverified, and the effort is high. |
| **The ten mock-only `ros2_control` bodies and the IRB 1200** (the vendor's plugin has not run) | owner, route found for one | Stäubli: a ROS-Industrial `simple_message` server is an independent server (rung 3, no account), and `drivers/ros_industrial` now drives one built from `industrial_core`'s own library (2026-10-03, item 12.E.6; the driver is for the protocol and the robot is not run). FANUC, Kawasaki, DENSO, Mitsubishi, Techman and ABB need an account or a licence for rung 1; each is listed in `DRY-COVERAGE-PLAN.md` item 12.H. |
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
| **Koch, OpenMANIPULATOR-X** | built (rung 4) | LeRobot's own `KochFollower` and `OmxFollower` run under the bridge on a stateful Dynamixel protocol 2.0 emulator (`drivers/lerobot/sim/dynamixel_bus.py`: 27 tests against ROBOTIS's own `dynamixel_sdk`, 68 checks of the bridge; item 12.E.5, 2026-10-03). The Koch is opened calibrated and in degrees, the OpenMANIPULATOR-X as it ships, so LeRobot's own `calibrate()` writes its calibration. Still alpha: the servo is a counterpart written here. |
| **Interbotix ViperX/WidowX** | route found | All Dynamixel, through ROS rather than LeRobot; Interbotix's own `xs_sdk_sim` is rung 2 at the ROS level *(web)*, and the emulator above could stand behind its `xs_sdk` for rung 4. |
| **AgileX Piper, ARX** | route found (rung 4) | SocketCAN protocols with MIT-licensed SDKs (`piper_sdk`, `arx5-sdk`); no emulator, so a vcan stand-in *(web)*. |
| **ANYmal** | owner | The SDK is documented through ANYbotics' support and a Siemens subscription; no public simulator *(web)*. Four ANYmal descriptions already run in the fleet sweep. |
| **Ghost Vision 60** | stands | Access-gated (2026-09-22, F116) and nothing public found *(web)*. |
| **Xiaomi CyberDog** | route found, low value | `cyberdog_ros2` is on ROS 2 Galactic (end of life); the simulator is Gazebo plus LCM and does not exercise the high-level API *(web)*. |
| **Unitree Go1/A1** ("the older SDK") | route found (rung 4) | `unitree_legged_sdk`'s UDP protocol is public in its headers, and `free-dog-sdk` is an independent client to cross-check the codec against; `unitree_mujoco` does not cover them *(web)*. |
| **Unitree B2** | built (rung 2) | The claim put it with the older SDK, and the plan's Phase 7 table called a B2 "built" (`DRY-COVERAGE-PLAN.md` :5556) while `drivers/unitree` had no B2. It is on SDK2, and since the forty-third session `model=b2` is a row of that driver: the Go2's twelve slots and leg order (the vendor's `b2_stand_example.cpp` and its `b2.xml` agree on them), the model's own travel, and the maker's page figure "About 360N.m" as a ceiling with no footnote to quote *(web)*. It passes the driver's suite against a fake and, in `unitree_mujoco`, stands from 0.126 m to 0.625 m, holds under the all-zero mask and folds on the stop. That simulator also lists b2w, go2w, h2, a2, as2 and r1: the wheeled ones wait for a wheel-joint rule, the H2 (`hg`, 31 joints) and the standard R1 (`hg`, 26 motors in sparse slots, no published torque) were built next, in the same session; the A2 is not, because the SDK has no low-level example for it to be held to. |
| **Mini Cheetah/Solo-12** | route found, low value | Mini Cheetah's stack talks LCM and shared memory, not a network API; Solo-12's ODRI master board uses raw MAC frames and no simulator speaks that wire *(web)*. |
| **Digit** | route found (rung 1), owner for provenance | Agility's `ar-control` simulator binary is re-hosted at `GTLIDAR/digit_controller` (public; proprietary licence, API "separate access") *(web)*. Run under the owner's ruling of 2026-09-29; Agility's own copy is the owner's to ask for. |
| **Cassie** | route found (rung 2) | `cassie-mujoco-sim` speaks a UDP protocol and needs no key; ports and packet layout were not read *(web)*. Discontinued robot. |
| **Optimus, Figure, 1X NEO, Kepler** | stands | No public SDK or interface (F115, 2026-09-22; searched again 2026-09-29) *(web)*. Would change with a maker publishing one. |
| **Apollo (sim only)** | narrowed | The *model* is in the tree (`drivers/apollo`, F113); Apptronik's hardware interface is not public. |
| **UBTech** | route found for one | The Yanshee SDK is public (`Yanshee-Raspi-SDK`); Walker and UGOT interfaces were not found *(web)*. |
| **Reachy 2** | narrowed | §66 already lists Reachy 2 under the LeRobot bridge. Pollen ships a Docker simulator and `reachy2_mujoco` *(web)*. |
| **Reachy Mini** | built (rung 2) | `drivers/reachy_mini` drives Pollen Robotics' own `reachy-mini-daemon --sim` (the maker's package at commit `a4ee4267`, its MuJoCo model with its Git LFS meshes, headless, in a container): 39 of 39 live checks and 34 of 34 through the real ABI, with the maker's own pydantic models, physics and web server's access log as the second opinion (item 12.E.12, 2026-10-04). Neither the Wireless nor the Lite robot was run, and the maker's simulator does not model motor torque, so `limp` is held to the request its server logged. |
| **Atlas, Robonaut** | stands | No public interface *(web)*. |
| **iCub** | route found | YARP with `gazebo-yarp-plugins` "same API as the real robot" (rung 2) *(web)*. |
| **PX4** | built (rung 1) | `drivers/mavlink` drives PX4's own SIH image (`px4io/px4-sitl-sih`, pinned by digest): a multirotor (58 of 58 live checks on each of three clean runs), a fixed wing (16 of 16) and a rover (19 of 19), each 34 of 34 through the real ABI, with `pymavlink` reading the same vehicle on PX4's second MAVLink instance (item 12.E.1, 2026-10-03). A PX4 boat, submarine or VTOL is not driven. |
| **DJI** | owner | The Onboard and Payload SDKs' simulator needs the flight controller on USB and a developer account *(web)*. Would change with either. |
| **Parrot** | route found (rung 1) | Sphinx runs the real firmware, free, but needs a GPU and Ubuntu 20.04/22.04; headless is unverified *(web)*. |
| **Crazyflie** | route found (rung 1) | `CrazySim` (GPL-3.0) runs modified real firmware behind cflib's UDP link, no account *(web)*. |
| **Tello** | route found (rung 3) | Four independent emulators speak the UDP SDK on port 8889 *(web)*. |
| **Skydio** | owner | A cloud REST API needing an account and token, and no simulator *(web)*. |
| **MiR** | route found (rung 4) | A REST API and an `osrf` client; no maker simulator *(web)*. |
| **AgileX Scout/Hunter** | class built (rung 3) over ROS; the robot is not run | AgileX's ROS packages are documented as taking a `Twist` and publishing odometry *(web, not checked)*, which `drivers/ros2_base` would drive; `ugv_sdk` itself speaks CAN, and a Gazebo simulator exists over ROS topics (rung 2) *(web)*. |
| **Clearpath** | class built (rung 3); the robot is not run | The Husky and Jackal are documented as taking a `Twist` and publishing odometry *(web, not checked)*, which `drivers/ros2_base` would drive; the free ROS 2 Gazebo simulation (no account; how many models it covers today is unclear) is rung 2 and is not run under it *(web)*. |
| **TurtleBot, Create 3** | **TurtleBot3 Burger and Create 3: built (rung 2), in their makers' own simulations**; TurtleBot 4: class built (rung 3), not run | `drivers/ros2_base` drives ROBOTIS's own `turtlebot3_gazebo` (Gazebo's server, the burger's differential-drive plugin, its bridge): 28 of 28 live checks, with Gazebo's own pose of the model as the second opinion (the driver's displacement is the physics' to 2 mm, its yaw to 0.001 rad), and 34 of 34 through the real ABI (item 12.E.3, 2026-10-03). It is the maker's simulation and not the robot. The plugin holds its last command for ever, where `ros2_control`'s controller zeroes it after half a second, so a velocity's closing zero and the stop are what bring the base to rest. **The Create 3** was run in iRobot's own simulation (`irobot_create_gz_bringup`: Gazebo under a virtual display, the maker's `motion_control` safety node between the command topic and `ros2_control`'s controller with the Create 3's own parameters): 29 of 29 live checks, with Gazebo's own pose as the second opinion, and 34 of 34 through the real ABI (item 12.E.3, 2026-10-03); the simulation runs at about a fifth of real time on a CPU renderer, so the checks are held in its own time. It is the maker's simulation and not the robot. TurtleBot 4 speaks the same two topics by its maker's documentation *(web, not checked)*. |
| **Hello Robot Stretch** | route found (rung 2) | `stretch_mujoco`'s API is "portable between sim and physical" *(web)*. |
| **Boston Dynamics Stretch, Locus** | stands | No public interface found *(web)*. |
| **TIAGo** | class built (rung 3) for the base; the robot is not run | The base is documented as taking a `Twist` and publishing odometry *(web, not checked)*, which `drivers/ros2_base` would drive. Public ROS 2 Gazebo simulation, Docker image, no account (rung 2) is not run under it *(web)*. |
| **ArduPilot submarines, BlueROV2** | built (rung 1) | `drivers/mavlink` drives ArduSub's own firmware, built at a pinned commit (item 12.E.4, 2026-10-03): `base: submerged` (§97), moved by `drive`, a stream of sticks that always ends in neutral, and stopped by a mode the configuration names (`stop_mode` has no default: keep the depth, keep the place, stop the thrusters, or come up). 61 of 61 live checks with `pymavlink` reading the same vehicle, including ArduSub's own RC channels for each stick; the boat, which shares Rover's modes, was built the day before. BlueROV2 itself was not run, and the simulated submarine's depth is not a measurement (F240). |
| **Robotiq, OnRobot, Inspire** | route found (rung 4, rung 3 transport) | Modbus RTU/TCP; `pymodbus` and `diagslave` serve the transport, the register maps are the vendors' *(web)*. |
| **Schunk** | built (rung 2) | `drivers/schunk` drives SCHUNK's own `schunk_gripper_dummy` (three copies: an EGU 60 and an EGK 25 on EtherNet/IP and an EGU 60 on PROFINET; the maker's repository at commit `76df296c`, GPL-3.0, nothing of it in this tree) with the maker's own `schunk_gripper_library` as the second opinion and a record of every frame its server was handed: 83 of 83 live checks and 37 passed through the real ABI (item 12.E.9, 2026-10-04). No gripper, EZU or EtherCAT module was run; a PROFINET module is refused by name (the maker's client and the maker's own dump disagree on where the error byte is, F249); and the maker's simulator neither stops a motion nor replaces one (F255) and clears *ready for operation* on every write (F254), so a stop is held to the frames the module was handed and not to the jaw. |
| **Allegro, LEAP Hand** | route found (rung 4) | SocketCAN (Allegro) and Dynamixel (LEAP); no emulator *(web)*. |
| **Shadow** | stands | A closed EtherCAT profile with a ROS 1 interface *(web)*. |
| **Barrett** | stands, low value | `libbarrett` on ROS 1, no simulator *(web)*. |
| **Tendon-driven MJCF grippers** | narrowed | The limit is in the MuJoCo *driver* (a tendon actuator has no joint index), not in the robot; `drivers/mujoco/README.md`. Would change with a spec amendment for non-joint actuators. |
| **Dynamixel, Feetech** | both built (rung 4) | Protocol 2.0 is published; no maintained third-party emulator, and LeRobot's mocks match canned packets rather than hold register state. A stateful Feetech STS emulator (protocol 0: ping, read, write, sync read and write, the EEPROM lock, travel limits, the homing offset, a position loop) is `drivers/lerobot/sim/feetech_bus.py`, and a stateful Dynamixel X-series one (protocol 2.0: CRC-16 and byte stuffing held to the SDK's own functions, the EEPROM lock while torque is on, the homing offset with the opposite sign to Feetech's, extended-position mode, the Shutdown register) is `drivers/lerobot/sim/dynamixel_bus.py`; each is held to its maker's own SDK and to LeRobot's own bus class. |
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

**beta.** Unitree's own simulator, unitree_mujoco, unmodified, for the Go2, B2, H1, G1, H1-2, H2 and R1: stand, motion, hold and stop in physics

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

**alpha.** LeRobot's own SO-101 (Feetech STS3215), Koch and OpenMANIPULATOR-X (Dynamixel X-series) classes, each as shipped with its maker's own SDK and LeRobot's own bus code, on buses of servos emulated in this repository; and a simulated robot written here behind LeRobot's interface. The servo is a counterpart written here, not the maker's firmware, and no other LeRobot robot has been run

Not shown by that counterpart:

* a servo: LeRobot's own SO-101, Koch and OpenMANIPULATOR-X classes and the makers' SDKs have been run under this bridge (44 checks on Feetech, 68 on Dynamixel), against buses of servos emulated here from LeRobot's control tables. That is a counterpart written in this repository, not the makers' firmware
* any LeRobot robot but those three: LeKiwi, HopeJR and Reachy 2 have role tables and have not been run, and OpenArm and reBot have no table
* what a servo's error bits mean: the bridge asks each servo's error register and torque every 100 ms and a bit is a fault (F232), but only what the emulators can raise has been injected (an overload on a Dynamixel, any bit on a Feetech), and the makers' lists of what each bit is are not in this tree, so `drivers/fault-vocabulary.json` does not bind them

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

#### `mavlink` — aerial, ground and surface vehicles, command-level

**beta.** ArduPilot's own flight code built for x86 (SITL, 4.7.1 at a pinned commit): a multirotor, a rover, a surface boat, a submarine (ArduSub, moved by sticks, with a stop its configuration names) and a fixed wing armed, moved and stopped through the real ABI, with the reference MAVLink implementation reading the same vehicle on a second serial port and the link impaired with delay, jitter and loss; and PX4's own published SIH image (a pinned digest): a multirotor, a fixed wing and a rover armed, moved and stopped through the real ABI, with the reference reading the same vehicle on PX4's second MAVLink instance. Both are the firmware on a simulated board and not a flight controller: no motors, no sensor noise, no radio, and a boat's stop is HOLD, which stops the thrust and not the hull. The submarine's depth is that simulator's weakest part (its pressure sensor reads negative absolute pressure and its depth swings by metres in seconds with nothing commanded), so its vertical axis is checked by what the firmware reads off the stick and not by where the vehicle goes

Not shown by that counterpart:

* a real flight controller: SITL is the firmware on a simulated board, with no sensor noise or failure, no motors and no radio link
* a real ROV: its thrusters, tether, depth sensor and DVL. The simulated submarine's depth is not a measurement (its pressure sensor reads negative absolute pressure and its depth swings by metres in seconds with nothing commanded), so the vertical axis is held only to what the firmware reads off each stick, which is ArduSub's own RC channels in ArduSub's documented directions, and never to where the vehicle goes
* what a stop means under water: `stop_mode` is the operator's and has no default, the driver was measured to enter the named mode and send neutral sticks at once, and nothing was measured of what a vehicle then does in a current, on a tether or under ice
* a PX4 boat, submarine or VTOL (refused by name: nothing here has driven one)
* a boat's stop: HOLD stops the thrust and not the hull, which has no brake, so the SITL boat's time to rest is a simulated drag and not a vessel's
* the vehicle's failsafes in the field, the EKF on real sensors, and everything about a real battery

#### `ros2_base` — ground bases, command-level

**beta.** ros2_control's own diff_drive_controller on its own mock hardware, in ROS 2 Jazzy (which takes a TwistStamped) and Humble (which takes a plain Twist), with a second reader of the same base written without this repository's code and ROS's own command-line tools reading the graph, and the sim half of SCTS through the real ABI; and ROBOTIS's own TurtleBot3 simulation (Gazebo, the burger's differential-drive plugin, its bridge) and iRobot's own Create 3 simulation (Gazebo, the maker's `motion_control` safety node and controller) with Gazebo's own pose of the model as the second opinion, which are the makers' simulations of robots and not the robots. The controller is the standard implementation of the two topics every wheeled-base stack speaks, and not any base maker's software: Husky, TurtleBot 4, Stretch and the rest are claimed from their documentation and not run, and the hardware is mock_components/GenericSystem, which is never late, never stalled and has no inertia or slip

Not shown by that counterpart:

* any named base but the TurtleBot3 Burger and the Create 3, and those only as their makers' simulations: each publishes its odometry with its own QoS, covariance and frames, and some take commands on another topic or through a multiplexer; the driver reads QoS compatibly and refuses to open when either topic is missing or unconnected, which is the right failure and not a measurement of that robot
* a base on a floor: mock_components/GenericSystem is never late, never stalled and has no inertia, and Gazebo's differential-drive plugin has an acceleration limit and no slip, a battery or a bumper, so layers 7, 9 and 10 are not reached and 6 only as far as a simulator's physics goes, and a base's bumpers, cliff sensors, e-stop chain and hardware command timeout are the base's
* navigation: there is no goto, because odometry drifts and a goal-seeking loop on dead reckoning would be a motion nobody asked for; Nav2 would be another driver

#### `ros_industrial` — industrial arms, joint-level

**beta.** ROS-Industrial's own simple_message library (industrial_core, built from source at a pinned commit, without ROS) as a controller's server, in each of the four ways a controller's build can spell an int and a real (little- or big-endian, 4- or 8-byte reals): the library's sockets, framing, codecs, message manager, ping handler and reconnect are the maker's, and the arm behind them is a straight-line interpolator written here. The driver's bytes are held to the library's, built and run, in both directions (118 checks), and the sim half of SCTS passes through the real ABI against each build. It is an independent implementation of the protocol written by the people who wrote it and not any robot maker's controller: Yaskawa Motoman, ABB, Fanuc, Staubli and the rest are claimed from ROS-Industrial's and each maker's documentation and not run, the protocol carries no joint limits or model so the arm's own go in the configuration, and what a controller does with a trajectory point is its vendor's

Not shown by that counterpart:

* any named robot: which controllers have a server, which ports it listens on, whether it answers a ping, what it does when a second client connects, what it does with a point it cannot reach and with a stop trajectory (the maker's header says a server 'should' stop) are each vendor's, and the driver has no model table because the protocol has no model
* a robot's own faults: RobotStatus.error_code is each vendor's number and simple_message defines none, so the fault word carries it verbatim and drivers/fault-vocabulary.json has nothing to cover
* an arm on a floor: layers 6, 7, 9 and 10 are not reached; the arm behind the maker's sockets is a straight-line interpolator written here, and a controller's own planner, acceleration limits and safety circuit are the controller's
* the controller's e-stop input: the driver's stop is a stop trajectory over TCP, a software stop, and safety.estop.independent is a bench test

#### `reachy_mini` — service robot, joint-level

**beta.** Pollen Robotics' own daemon in its own simulator (`reachy-mini-daemon --sim`, the maker's package at a pinned commit installed with its MuJoCo extra and its Git LFS meshes, run headless in a container): the physics, the kinematics, the REST interface and the web server are the maker's. The driver's routes, models and limits are held to the maker's source and run through the maker's own pydantic models (30 checks); the driver's session, commands, refusals and stop run against that daemon with the maker's models, its physics and its web server's access log as the second opinion (39 checks); the sim half of SCTS passes through the real ABI against it (34 of 34). Not run: a robot, Wireless or Lite. The maker's simulator does not model motor torque (its `get_motor_control_mode` returns a constant), so `limp` is held to the request its server received and not to a head that went limp; the motors' firmware, heat, cables and the real antenna travel are not reached; the ranges of the head's translation and of the antennas are the operator's, because the maker documents none

Not shown by that counterpart:

* a robot: the motors' firmware, torque, heat, cables, the head's real dynamics and the antennas' real travel are not reached; the maker's simulator is MuJoCo with the daemon's own kinematics
* a limp head: the maker's simulator returns `enabled` whatever it is asked (`MujocoBackend.get_motor_control_mode` is a constant), so `limp` is held to the `set_mode/disabled` request its server logged, and what a real head does when its torque goes is the real robot's backend's to say
* the ranges of the head's translation and of the antennas: the maker documents none, so the operator gives them and the driver refuses a robot that starts outside them; only pitch, roll, head yaw and body yaw are the maker's documented ones
* the daemon's faults: it reports one error string and no code, so the fault word says there is one and drivers/fault-vocabulary.json has no table to bind
* the Wireless robot's own network, its Raspberry Pi and the maker's apps, which this driver never touches

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
