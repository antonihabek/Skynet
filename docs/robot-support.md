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
  3 lists thirteen such substrates; each one found at least one defect that every counterpart written in this
  repository had agreed with. The finding that generalises is not any one defect but the rate: a driver
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
**50 robot rows across 20 drivers that command a physical robot: 30 beta, 20 alpha, 0 stable.** (A row is one robot or one robot family a driver was built for or run against; a driver that hosts other vendors' plugins counts each body.)
<!-- /generated:summary -->

"Installable" means `skynet setup --robot <driver>` finds the library, prints the stage and what it rests on,
and the driver passes `skynet driver check` (the sim half of the conformance suite, through the real ABI). It
does not mean the robot is commissioned: an operator still asserts roles (§63.4), provisions the envelope
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
| UFACTORY xArm 5, 6 and 7 (run against xArm 6 firmware only) | `xarm` | joint-level | **beta** | 1 |
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
| 1 | **Wire and framing** | a field one byte off; a checksum seeded wrong; a version the controller refuses | **yes, fully** | layout checks against the vendor's own source or the reference implementation (xArm 40, MAVLink 193, Fairino 44); a second client reading the same controller |
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
| UFACTORY's xArm firmware | a target the controller acknowledges and then faults on; ranges rounded to a milliradian | F175–F176 |
| ArduPilot's flight code | see [`drivers/mavlink/README.md`](../drivers/mavlink/README.md) | F178 and the MAVLink findings there |

Thirteen substrates, at least one finding each, and none of them found by a counterpart written here. **The
drivers that have not met one are the `alpha` rows, and the honest prior for each is that it carries a defect of
this kind**: Booster, FAIRINO, the Lite3, Fourier, Spot, Pudu, LeRobot, EtherCAT, and the ten `ros2_control`
bodies that run against `mock_components`.

## 4. By class of robot

| Class | Driven today | Not driven | Why, and what would change it |
|---|---|---|---|
| **Industrial arms** | UR, ABB IRB 1200 (EGM), KUKA KR 6 (RSI), Doosan ×9, AUBO, Kawasaki, DENSO, Mitsubishi, Stäubli, FANUC LR Mate, Elite, Kinova, Techman, xArm, Franka | Yaskawa, Epson, Comau, Hyundai, JAKA, Dobot, Rokae, Flexiv, Neuromeka, Rainbow, Kassow | section 5, row by row. Most have a documented protocol; most lack a free controller emulator |
| **Cobots** | as above | as above | as above |
| **Quadrupeds** | Unitree Go2, Deep Robotics Lite3, Boston Dynamics Spot | ANYmal, Ghost Vision 60, Xiaomi CyberDog, Unitree Go1/A1/B2 | Ghost is access-gated; ANYbotics sells to customers; Go1/A1 use the older `unitree_legged_sdk` |
| **Humanoids** | Unitree G1/H1/H1-2, Booster T1, Fourier GR-1, AgiBot X1 | Digit, Optimus, Figure, 1X NEO, Apollo (sim only), Kepler, UBTech, Pollen Reachy 2 | section 5 |
| **Aerial** | ArduPilot multirotors and fixed wings (this session) | PX4, DJI, Parrot, Crazyflie | PX4 is a second mode vocabulary; the others have vendor SDKs and no free simulator this tree has met |
| **Ground vehicles** | ArduPilot rovers (this session), Pudu (cloud) | MiR, AgileX, Clearpath, TurtleBot/Create 3, Boston Dynamics Stretch/Locus | no driver yet; ROS 2 bases would ride the bridge if a base were a `ros2_control` system |
| **Marine** | nothing | ArduPilot boats and subs are refused by name | §5 has no base kind for water: [`issues/no-base-kind-for-water.md`](issues/no-base-kind-for-water.md) (F178) |
| **Hands and grippers** | via the arm's driver, where the gripper is a joint | tendon-driven MJCF grippers (Robotiq 2F-85 in the fleet sweep); Robotiq/OnRobot as a device | the MuJoCo driver carries joint-driven actuators only (F179, below) |
| **Servo drives** | any CiA 402 drive on CAN (`canopen`) or EtherCAT (`ethercat`) | vendor-specific drives that do not speak CiA 402 | each is a protocol; the profile covers most industrial servo drives |
| **Exoskeletons, surgical, prosthetic** | nothing | everything | regulated devices with no public interface; not a driver problem |
| **Consumer and home robots** | nothing | vacuums, Sphero, Anki Vector, Tello, Petoi and the rest | open-source community protocol stacks exist for many; see the first row of section 5 |

## 5. The claims that a driver cannot be written, challenged

The specification and the plan say in several places that a robot *cannot* be reached. Each is re-checked below
against the public record on 2026-09-29. A verdict is one of: **overturned** (built, with the run), **overturned,
route found** (a way exists and is not built yet, with the evidence), **narrowed** (the claim was too broad),
**stands** (with dated evidence, and what would change it), or **owner** (blocked on an account, a licence or a
purchase this repository may not make).

*The table is filled in as each row is worked. Rows marked "not yet checked" are on the list and unverified.*

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

**beta.** UFACTORY's own controller firmware (xarm6-type6-controller 2.4.0, in the simulator image their support article names) for an xArm 6: motion, the refusal of a target the firmware acknowledges and then faults on, the stop and its release, with UFACTORY's own XArmAPI reading the same controller as a second opinion. The 5- and 7-axis paths have met only a simulated controller written in this repository

Not shown by that counterpart:

* motors: the firmware runs with none, so no dynamics, brakes or thermal state
* the 5- and 7-axis paths have met only a controller written here
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
