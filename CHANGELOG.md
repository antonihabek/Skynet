# Changelog

All notable changes to Skynet OS, in Keep a Changelog form. Versions are the
workspace version in `Cargo.toml`, tagged `v<version>`; `package.yml` refuses a tag
the workspace disagrees with, so the changelog, the tag and `skynet version` cannot
drift apart. Dates are release dates — entries land under Unreleased first.

## [Unreleased]

### Added

- **mjbots moteus servo controllers over CAN-FD** (`drivers/moteus`, `DRY-COVERAGE-PLAN.md` item 12.E.10's third driver), **alpha**: one Skynet joint per controller on one CAN-FD bus, joint-level, in position, velocity and torque with mixed per-joint modes. `stop_mode` (zero velocity, brake or stopped) is required and has no default; `release_stop` is honest only from the first two. The travel is the controller's own, read over its diagnostic channel and narrowed, never widened, by the operator's `limits`; every axis is read back in position mode before anything is commanded, and a controller in a fault, in timeout, in another master's mode, outside its travel or that does not answer refuses the open by name. 379 checks against simulated controllers written here (392 on a kernel CAN-FD `vcan`), the registers, modes, fault codes, scales, frames and identifier held to mjbots's own Python client and firmware headers at a pinned commit (182 checks), the fault-vocabulary contract (264 checks over 66 injected codes), the link-chaos contract (40 checks) and 35 passed through the real ABI. 292 of 292 seeded defects noticed (the first campaign noticed 247 of 291; among the 39 checks nobody had written was every wait the driver makes, which a bus that does not say at once that nothing is coming can observe and the in-process one cannot). **F285**: mjbots's register reference lists the limiting codes in the fault register as 96 to 103, and its firmware defines and sends 104 and 105 too; the driver's table is the firmware's. `tools/faultproxy`'s CAN relay learned CAN-FD (`CanRelay(fd=True)`, a `canfd` flow in `chaos.py`, `faultproxy.py --fd`); its self-test is 133 checks.
- **AgileX Piper** (`drivers/piper`, item 12.E.10's second driver), **alpha**: 330 checks against a simulated arm (341 on a kernel `vcan`), the layouts held to AgileX's own SDK codec at a pinned commit (72 checks; the encoder was looser than the maker's on a gripper torque past 5000, now fixed, and **F284** pins a disagreement between the maker's own sources about "all motors"), the fault-vocabulary contract (280 checks over 70 injected codes), the link-chaos contract (40) and 31 passed through the real ABI. 169 of 169 seeded defects noticed (the first campaign noticed 144 of 171 and found 27 checks nobody had written). `Bus.backlog()` was added to the CANopen driver's bus classes.
- **Seeding reaches the last two drivers that command a robot, and finds four defects** (item 12.C.3). `tools/seed-defects` runs a compiled driver's suite in a container (`skynet-ros2-control-mock`, new; `skynet-x1-sim`), each with `--network none`. **`ros2_control`**: 71 of 71 seeded defects noticed (the first campaign noticed 36 of 72 and found 28 checks nobody had written); **F281** a command refused for a non-finite setpoint had already stored the joints before it, **F283** the bridge found a joint's type and limits by searching for the literal text of one spelling of the URDF, so a URDF that wrote `type` before `name` declared no limits. **`x1`** (F282): the driver published a non-finite setpoint to the maker's simulator as it was, and none of the five compiled drivers refused one (only `x1` was fixed); the first clean campaign noticed 74 of 77 and found three checks nobody had written (`release_stop` had none). **F286**: the X1's two earlier campaign reports measured four containers hearing each other over DDS multicast and not the suite — a control "killed" by an unrelated check was the symptom — and the containers now have no network. `unitree` 94 of 94, `lite3` 60 of 60, `fairino` 85 of 85 and `booster` 55 of 55 were re-run with F282's seeds in them.
- The fault-vocabulary contract (`tools/fault-vocab/contract.py`, `DRY-COVERAGE-PLAN.md` item 12.C.6) and the link-chaos contract (`tools/faultproxy/chaos.py`, item 12.C.4) bind **ODrive**, and its first defect-seeding campaign is run (item 12.C.3): 76 checks over the maker's 13-entry `Axis_Error` table, generated from the DBC and held against `enums.py`, plus 6 undefined bits that escalate and never read as healthy; 40 checks with the bridge on one `vcan` and the simulated axes on another through a CAN relay, clean on the first run (the stop returns in 0.1 ms against a blackholed link and the body is renewed by re-opening, the ENOSYS path by design); 37 of 37 seeded defects noticed with 0 survived, the first campaign's one mislabelled control re-triaged. Both contracts are shown red first (a fault-blind read fails 36 checks, an unlatched stop fails the latch check). Reading the firmware to write the table found the ignored subsystem flags correct: every encoder, sensorless and controller error sets its axis bit or refuses closed loop, and the motor errors that do neither are boot-latched advisories or surface as unarmed — the axis word plus the armed reading reports every fault the firmware stops for.
- **ODrive servo axes over CANSimple** (`drivers/odrive`, `DRY-COVERAGE-PLAN.md` item 12.E.10's first driver), **alpha**: one Skynet joint per ODrive axis on one CAN bus, joint-level, in position, velocity and torque with mixed per-joint modes. 173 checks against simulated axes written here on an in-process bus (181 on a kernel `vcan` against axes in another process) and 32 passed through the real ABI at guest; the frame ids, payload layouts and enumerations are held to ODrive's own firmware, CAN database and enums module at a pinned commit (`layout_check.py`, 96 checks). Health is the heartbeat: a body whose axes do not send one does not come up. Errors are refused, never cleared, and the stop latches `ESTOP_REQUESTED` in the firmware, so there is no software release (`release_stop` is ENOSYS and a new session returns the body, the Spot precedent). The wire publishes no rating for the motor on an axis, so the joints are continuous with no limits and the envelope is null, and effort reads NaN. The layout check pins two disagreements between the maker's own sources (F280): the DBC models the motor error as 32 bits while the firmware sends 64, and its input-velocity units are swapped. Not run: an ODrive, any motor, or the maker's firmware.
- The fault-vocabulary contract (`tools/fault-vocab/contract.py`, `DRY-COVERAGE-PLAN.md` item 12.C.6) binds **LeRobot**: the five error bits of Feetech's `Status` register (the `ERRBIT_*` of `scservo_sdk`) and the five of Dynamixel's `Hardware_Error_Status` (ROBOTIS's e-manual), each named by its maker, and codes outside both tables, injected into the emulated buses behind LeRobot's own SO-101 and Koch classes and held to FAULT+DEGRADED, refused commands, the word `servo id << 16 | bits` and recovery (68 checks, 17 injected; bit 7 of the Feetech byte is unstageable — the maker's own SDK discards any reply whose error byte exceeds 0x7F as a framing error — and is reported per code, never skipped). Shown red first: the first run failed two checks on that probe, and a bridge that reported DEGRADED without FAULT fails seven. The two seeded defects the last campaigns had left alive — the count of samples a session had returned, kept by the next session in the Unitree and Booster drivers — are killed by a check each that times the new session's first read (0 ms fixed, the whole 20 ms window under the seed): Unitree's suites went from 730 to 737 checks, Booster's from 100 to 101.
- The link-chaos contract (`tools/faultproxy/chaos.py`, `DRY-COVERAGE-PLAN.md` item 12.C.4) reaches **DDS**, and found the same defect in three drivers (F277–F279), each fixed test first. DDS names its peers in
  discovery messages and the FCI's state stream goes to a UDP port the driver announces, so no relay with an address of its own can stand in front of them: `tools/faultproxy/relays.py` gains `NetemRelay`, the
  kernel's own link emulation (`tc netem`) on the loopback of an unprivileged network namespace that the driver and the substrate share, which carries delay, jitter, loss, bursts of loss, duplicates, reordering,
  one-bit corruption and a blackhole window, **refuses what a kernel queue cannot do** (a stall, a truncation, a reset, a fault in one direction) instead of running it as something else, and refuses to touch a
  machine's own network (`faultproxy.py netem` is the same relay on the command line). Its self-test went from 92 to 128 checks, and ten defects put into a copy of the relay were each noticed. **Unitree** (F277) and
  **Booster** (F278): a robot whose link had been cut kept reading as fresh for ever, because `read_state` stamped the last state with the time of the read (§24: *at acquisition, never at return*) and advanced the
  sequence with the call (§24: a repeat is a stalled producer); a read now returns the state with the stamp it arrived with and a sequence that counts samples, waits for the next one for the watchdog window, and
  holds both when it does not come (the rule `lite3` and `ur_rtde` already followed). **Franka** (F279): the Python driver returned its cached state as a successful read when the stream had gone quiet, where the
  Rust port answers `EIO`. Unitree's suites went from 653 to 730 checks, Booster's from 89 to 100, Franka's from 135 to 140; the contract reads 56 of 56 on all four drivers it was run on (Unitree, Booster, Franka and
  its Rust port). Seeded defects noticed: Unitree 91 of 91, Booster 52 of 52, Franka 43 of 43 (the first campaign for the Python driver), each with 0 survived.
- The link-chaos contract (`tools/faultproxy/chaos.py`, `DRY-COVERAGE-PLAN.md` item 12.C.4) reaches its first two C++ drivers, FAIRINO's FR3 and Deep Robotics' Lite3, gains a sixth question, and found three defects, each
  fixed test first. **FAIRINO** (F274, F275): a servo command waited out the controller's answer, up to the driver's whole two-second RPC timeout (1,497 ms under a stall and 2,028 ms under a blackhole, against the
  control loop's quarter of a second); a whole command, the calls that open a servo window included, is now given 200 ms and is `EIO` at once beyond it, the connection and each wait for the reply bounded by what is
  left of it. A failure that goes on is said once and not on every period (a refused connection said nothing at all). The stop wrote the vendor's frame with a blocking send, which a controller that had stopped reading
  its port could hold for 2,026 ms against the RHAL's 50 ms, and it never waits for room now. **Lite3** (F276): the driver re-took control right only when its own sends had been quiet, and a link outage is not that: a
  control loop that goes on writing through a link that is down is never quiet, the motion host takes the legs back after a second without a command it received, and after a stall of 1.5 s every command was accepted by
  the socket, ignored by the robot and answered `OK`. The state stream going quiet for longer than the reclaim guard is the driver's evidence that the link was down, and the next command takes control right again, once,
  and says so. The sixth question (*effective*: once the link is healed, does the substrate act on the driver's commands?) is what found it, because a datagram nothing answers is `OK` as soon as the socket has it. Seeded
  defects noticed: Fairino 82 of 82 (73 before; the first run on the new seeds noticed 81, and the survivor was a test with too little margin), Lite3 58 of 58. The C++ subjects load the driver through
  `tools/faultproxy/rhal_ctypes.py`; the arm's true position comes from a shared object of the fake the suite runs (Fairino) or from a tap the fake sends (Lite3).
- The link-chaos contract (`tools/faultproxy/chaos.py`, `DRY-COVERAGE-PLAN.md` item 12.C.4) reaches the UR bridge (Python and Rust), the Python EtherCAT master and LeRobot's SO-101, and found defects in every driver it
  reached, none of them in the two Rust ports' stops (already fire-and-forget), each fixed test first. **UR** (F264–F266, F272, F273): `emergency_stop` asked the dashboard for `stop` in the caller and returned after 5,005 ms against a blackholed link (50 ms wanted), `read_state` raised
  `RtdeError` out of the driver when the controller closed the stream, the same sentence was printed on every period (98 and 188 identical lines in one run), a setpoint's send could wait the socket's five seconds, and the
  Rust port read a closed stream as a protocol error and not as a link that had gone; the stop's dashboard request now runs from its own thread, a read is `EIO`, a send is bounded and refuses the next setpoint by name once
  one did not finish, and `ports` in the configuration lets a relay stand between the driver and a controller. **EtherCAT** (F269): a raw socket's `TimeoutError` escaped every entry point of the Python master, whose stop also
  waited for the ring's answer (50.2 ms); the master turns an OS error into a `MasterError` and its stop is one frame written, as the Rust port's always was. **LeRobot** (F270, F271): the stop wrote torque-off one servo at
  a time and waited for every reply (351.8 ms against a line that carried nothing) and is one broadcast `SYNC_WRITE` that waits for nobody now, with the next health poll finding a servo that did not take it and
  sending the stop again; and a late reply to an earlier request was read as error bits that differ every time, each a fault with every command refused, where a reading is a fault now when the servos say the same thing
  twice. The runner learned that a fault's onset is the same instant on every flow (a flow that carries nothing until a stop asks for it was never tested: F267), that an entry point that raises is a failed check and
  not the end of the run, that a held sequence is §24's stalled producer, and that its relay's threads should not die with a traceback when a socket is closed under them (F268); `exception-escapes` is a seed class. Seeded
  defects noticed: UR 50 of 50 (Rust 89 of 89), EtherCAT 36 of 36 (Rust 77 of 77), LeRobot 46 of 46.
- A Robotiq 2F-85 or Hand-E gripper over Modbus RTU (`DRY-COVERAGE-PLAN.md` item 12.E.8, `drivers/robotiq`, beta): one prismatic joint in metres, claimed from the maker's SDK and not run on a gripper. `model`, `speed`, `effort`, `stop_mode`
  and `simulator` have no default, `stop_mode` accepts only `halt` (rATR, the maker's emergency release, opens or closes the fingers all the way and leaves a major fault, and whether that is the safer stop is the integrator's
  risk assessment), and activation and the reset after a major fault are motions a person runs, never something `open` does. Its bytes are held to the maker's own client, which is run (43 checks), and its live block runs against
  **Robotiq's own simulated gripper and client**, built unmodified in a container: 56 of 56, and 37 passed through the real ABI. 343 checks against a simulated gripper, the link-chaos contract (51), the fault-vocabulary contract
  (124 checks, 31 codes injected) and 200 of 200 seeded defects noticed (the first campaign noticed 191: each of the nine survivors was a check nobody had written). F262 (a gripper power-cycled behind a live driver would have
  been activated by the driver's own command image) and F263 (a command waiting when the image is made again would have been reported carried).
- A SCHUNK EGU, EGK or EZU gripper over its module's web interface (`DRY-COVERAGE-PLAN.md` item 12.E.9, `drivers/schunk`, beta): one prismatic joint in metres from the module's own travel, which a configuration
  may narrow and never widen. `stop_mode`, `speed` and `simulator` have no default, a PROFINET module is refused by name, and a module that comes up in its fast stop opens with the stop latched and is released only
  by an acknowledge that is verified to have cleared the error. Its two 16-byte frames are held to the maker's own client, which is run (21 checks), and its live block runs against **the maker's own protocol
  simulator** (an EGU 60 and an EGK 25 on EtherNet/IP and an EGU 60 on PROFINET, in a container) with the maker's own client as the second opinion and a record of every frame the maker's server was handed:
  83 of 83, and 37 passed through the real ABI. 204 checks against a simulated module, 130 of 130 seeded defects noticed (109 on the first run: each of the 21 that survived had no check and is one now, shown red against its seed, and so is the one a later
  run found, a stop landing while a command waits for the module's toggle), the link-chaos contract (30). F249–F256: the maker's client and its own PROFINET dump disagree on where the error byte is, its client imports `pytest` without declaring it, its simulator comes up in
  a fast stop, clears *ready for operation* on every write and neither stops a motion nor replaces one; and the driver's own, a grip or a release that left the "already issued" target in place (F253) and a command
  that could take 336 ms against a control loop's quarter of a second (F256).
- The link-chaos contract (`tools/faultproxy/chaos.py`) reaches three more drivers, Fourier GR-1, Pudu and Spot, and found four defects in them, each fixed test first: a reply cut off half way raised
  `http.client.IncompleteRead` out of `read_state`, `emergency_stop` and a motion (an HTTPException is not an OSError; Fourier and Pudu, F257); a read of a robot that had gone quiet waited out the whole request
  timeout on every joint (six joints at three seconds: 18 s) and Fourier's stop was a synchronous HTTP call that waited the same (F258); Pudu's read waited out ten seconds (F259); and Spot's read waited ten seconds,
  a motion waited twice over and `emergency_stop` returned after 29.9 seconds (F260). A read now has a bounded budget, a stop sets the latch and is told to the robot from a thread of its own (retried, and
  waited for by `close`), and a request that creates something keeps its long timeout, because a timeout does not say it was not made. The runner's stop check could not fail for a stop made of one request, since a
  profile's onset counts from the first message after it is set (found on Fourier, F261); it now starts the clock first. It also takes a serial relay, an `attach` hook and a subject's declared `read_bound_s`.
  A campaign no longer counts a step the shell could not start (exit 126 or 127) as a kill: the SCHUNK run had counted 24 seeds killed by a deleted binary.
- A Pollen Robotics Reachy Mini over its daemon's REST interface (`DRY-COVERAGE-PLAN.md` item 12.E.12, `drivers/reachy_mini`, beta): nine scalar joints (the head's six degrees of freedom, the body's yaw and two
  antennas), Wireless and Lite, claimed from the maker's README and the daemon's own source. The daemon documents four ranges and says nothing of the head's translation, the antennas or any speed, so the operator
  gives those (a missing one is a refusal), `stop_mode` has no default, and `simulator` is the daemon's word. Its routes, models and limits are held to the maker's own source and run through its own pydantic
  models (30 checks), and its live block runs against **the maker's own `reachy-mini-daemon --sim`**, headless in a container, with the maker's models, physics and web server's access log as the second opinion:
  39 of 39, and 34 of 34 through the real ABI. 168 checks against a simulated daemon, 73 of 73 seeded defects noticed (it was 61 of 71 when first run), the link-chaos contract (30). The maker's simulator does not
  model motor torque, so `limp` is held to the request its server logged. F245 (the daemon cannot be asked what it was told to go to), F246 (a robot found outside its ranges was accepted and then every command was
  refused, in silence: the maker's daemon starts asleep), F247 (every refusal said nothing, and a transport failure said itself every period) and F248 (the substrate's image installed Git LFS pointers for the
  maker's meshes).
- A robot arm behind a ROS-Industrial `simple_message` server (`DRY-COVERAGE-PLAN.md` item 12.E.6, `drivers/ros_industrial`, beta): the protocol that Motoman's MotoROS, `abb_driver`'s RAPID server, `fanuc_driver`
  and others speak on the controller. The protocol carries a trajectory point in and a feedback and a status out, and no joint limits, joint count, speed ceiling, model or word on whether the controller is a
  simulation, so the driver takes the arm's own limits in its configuration (a missing one is a refusal), gives `simulator` no default, and takes the byte order and real width of the controller's build as the
  maker's own `BYTE_SWAPPING` and `FLOAT64` switches. Its bytes are held to ROS-Industrial's own library **built from source and run**, in both directions and in the library's four builds (118 checks), and its live
  block runs against a controller's server built from that library: 35 of 35 and 34 of 34 through the real ABI on each build. 146 checks against a simulated controller, 69 of 69 seeded defects noticed, the link-chaos contract (38).
  F241 (the maker's own server accepts the next client five seconds late, which the driver took for a server that does not answer), F242 and F243.
- A submarine through the MAVLink driver (`DRY-COVERAGE-PLAN.md` item 12.E.4): a vehicle running ArduSub (`MAV_TYPE_SUBMARINE`) is driven instead of refused, and declares `base: "submerged"` (§97). What differs
  was read out of ArduSub's own source at the pinned commit and held against the firmware built from it: the stop is the operator's decision, so `stop_mode` has no default and a configuration without it is
  refused at open; a motion is a stream of `MANUAL_CONTROL` sticks that the vehicle holds for `RC_OVERRIDE_TIME`, so `drive` and every stop end in neutral frames; there is no takeoff, land or return, and the way out
  is `surface`, a mode ArduSub leaves by itself at the surface, which the driver reports as where the vehicle is. `MANUAL_CONTROL` is the nineteenth message in `frames.py`, held to `pymavlink`'s bytes. Measured:
  78 offline checks, a 242-check layout check, and 61 of 61 live checks against ArduSub's own firmware with `pymavlink` reading the vehicle's RC channels on a second port; F239 and F240.
- A ROS 2 mobile-base driver (`DRY-COVERAGE-PLAN.md` item 12.E.3, `drivers/ros2_base`, beta): a base whose interface is a `Twist` or `TwistStamped` in and `nav_msgs/Odometry` out, held by a stream with a lease, a
  refusal to open on a connection that reaches nobody, odometry that must be current, and a stop that is repeated until the base is seen at rest. Measured against `ros2_control`'s own `diff_drive_controller` on
  Jazzy and Humble (47 and 46 live checks) and **ROBOTIS's own TurtleBot3 simulation (28)**, each with the sim half of SCTS through the real ABI; 58 of 58 seeded defects noticed; F235 to F238 (two were the
  driver's: a turn through the half turn reported as a circle the other way, and Humble's `cmd_vel_unstamped` kept in the slug). The robots are claimed from their makers' documentation except the TurtleBot3, which ran in its maker's simulation.
- The MAVLink driver under the link-chaos contract (`drivers/mavlink/chaos.py`, nightly in `faults.yml`): the five questions asked through `tools/faultproxy`, 58 checks.
- Dynamixel through LeRobot's own bus code (the wire level of `drivers/lerobot`) and the stop that survives a servo's alert: F232 and F233. A stop written to `drivers/python-host` while an extension runs is
  read at once (`stop_during_extension`, F234).

- PX4 through the MAVLink driver (`DRY-COVERAGE-PLAN.md` item 12.E.1): a multirotor, a fixed wing and a ground rover running PX4 are driven
  through `drivers/mavlink`, which had refused PX4 by name. What differs from ArduPilot is a second motion set (`motions.py`; the bridge's
  autopilot-specific half moved there, shown first to leave every ArduPilot check as it was): a mode is a packed word asked for in two parameters,
  an integer parameter arrives as its own bytes in the float field, a takeoff is above sea level, a goto is a `DO_REPOSITION` over
  `COMMAND_INT`, and a velocity is a stream of bounded length that hands the vehicle to hold before it stops (PX4 answers a stream that simply
  stops by flying home). Whether a PX4 vehicle is a simulation is read from its airframe where only the simulation build has it and asserted
  by the configuration otherwise, with no default. A signed link is no longer refused on the flag alone: PX4's own build signs every frame and
  acts on unsigned commands, so the driver asks the vehicle whether it does. Measured against PX4's published SIH image (`make check-px4-docker`,
  nightly as `px4-sih`); eight findings, F224 to F231, in the plan. The last is a defect in the driver found by repeating the live block under loss: a mode request is one
  datagram, and a lost request for hold left PX4 in offboard with nothing streaming, which it answers by flying home; a request is now asked again every second until
  the heartbeat shows the mode.
- Two base kinds for water (§97, F178; `DRY-COVERAGE-PLAN.md` item 12.G.1): `surface` and
  `submerged` join `fixed`, `wheeled`, `tracked`, `legged`, `aerial` and `rail`, and both are
  mobile, so a hull that declares no safe-state basis is refused host as a wheeled base is. The
  contracts, the importer, the TypeScript reference and all eight editions carry them, and
  `mobile_bases` is now a corpus vocabulary of its own. `drivers/mavlink` drives a boat — ArduRover
  built with a boat frame, `base: "surface"` — instead of refusing it; the submarine, refused
  at first for a different reason (ArduSub has its own modes and its stop is a person's
  decision), is driven too (item 12.E.4, above).
- `flex` and `pitch` are one hinge on a link with one bend (§98, F177): §6's role step binds a
  channel whose own role the body lacks to the same role under the other word on an `elbow`,
  `wrist`, `knee`, `ankle`, `finger` or `knuckle`, so a policy written for an SO-101 reaches a
  Universal Robots arm the importer named from its axes. The binding is a `remap` whose reason
  names the equivalence; it keeps the ordinal, stops at a shoulder and a hip, yields when the
  policy declares both words, and does not touch sensors. Rust, the TypeScript reference (and the
  Hub's mirror of it) and the Python, C++ and Go editions plan it; the corpus is 1,510 cases.
- The RHAL at ABI v2 (§4, §24): the vtable appends `release_stop`, which clears the session's
  software stop latch without closing it, so power-on self-test can prove the stop path and hand
  the loop a commandable session. It never clears a latch the hardware keeps; a driver whose stop
  is latched only by hardware leaves the pointer NULL and the OS renews the session instead.
  Appending is still a break — a v1 table is one pointer shorter — so this one bumps. Every
  driver, the C/C++/Python bindings, `skynet driver check` and the scaffold carry it.
- The kernel's half of POST (§48.2, §48.5): `skynetk` runs the five kernel checks at start — ABI,
  `describe()` against the descriptor, the estop asserted, observed and then released, encoder
  sanity, and the corpus self-check — and records each verdict in §13's ring beside its measured
  worst-case cycle. `post::run` reads them instead of reporting `elsewhere`; a recorded verdict is
  pass or fail, and only an unrecorded check is `unavailable`. A robot whose eight checks all pass
  leaves `post`. Two defects fell on the way: the nesting window had no source at all (F155), and
  the black-box check read `ring.bin`, a file nothing has ever written (F156).
- A skill judged `success` (§54.3): `sandbox.sh` section 7 carries a success criterion on the
  elbow's recorded position and a control with the same motion and an unreachable criterion,
  judged `failure`. The specification is amended in both copies.
- Package fetch (§91): a robot told to run a package it does not hold fetches it from its Hub.
  `FetchPackage` reads `GET /devices/{id}/packages/{digest}` with the device credential, refuses
  a document whose id, kind, version or digest is not the desired state's or whose paths could
  leave the package, fetches each file in bounded slices that resume (§75.2), checks each against
  its sha256 and length, and stages the whole only if it hashes to the version digest. File
  requests carry no credential. A refusal or bad bytes hold the next attempt on §34.4's curve,
  and a metered or absent link waits (§34.3). The specification gains §91 in both copies.
- The sandbox (§23.4): every non-driver unit gets a read-only tree, no devices, a syscall
  allow-list, no namespaces, no real-time and no capabilities. A package that may command a
  joint also has no network and no socket family but `AF_UNIX`. In the system manager each unit
  runs as a user of its own. `skynetk --policy-socket-group` shares §25's socket with one group
  at mode 0660, only the commanding package is in it, and `install.sh` creates it.
  `safety.policy.isolation` reads every directive back and asks the kernel for the uid, the
  seccomp mode and the network namespace. A package declaring an accelerator is refused by
  name. `crates/skynetd-io/sandbox.sh` measures it as root.
- The body, off the newest recorded cycle: §54.2's `robot.state.joints`, `velocity`, `effort`,
  `estop`, `braked`, `degraded`, `homing` and `fault_code`, refused on a failed read or a joint
  count the descriptor disagrees with. `GET /state` and `skynet state` show the joints.
- The fleet sweep (`fleet/`, `DRY-COVERAGE-PLAN.md` Phase 10): every public robot description in
  `robot_descriptions` and DeepMind's menagerie, with this repository's own vendor descriptions —
  pinned by upstream commit and by bytes in `fleet/catalogue.json` — run through Skynet's URDF
  importer, the MuJoCo driver behind the C host, `skynet driver check`, `skynetk` with a policy
  asking past every rating and a replay of the sealed window, a check of both ends of every
  joint's travel, and `skynet transport` of a skill trained on another robot of the class.
  `reference/reports/fleet.json` is the result; `tools/check-fleet.mjs` holds it to the catalogue,
  to `drivers/models.json` and to the figures `docs/status.md` and the README quote, and refuses
  a release stage in it; `.github/workflows/fleet.yml` re-runs it nightly and fails on a row that
  got worse.
- `skynetk run --source <policy.json>`: the policy's source embodiment, resolved against the
  body's own descriptor by the planner `skynet transport` runs. Channels, each joint's ceiling in
  its own mode's units and the modes come from the plan, so the envelope guard has something to
  hold (F158). A refused plan, or one that binds a channel anywhere but the joint at its index, is
  refused. `skynetk`'s summary now names the first failed write — cycle, code and mode.
- `drivers/mujoco`: `hold_gains` in the configuration, so a `<motor>`-driven joint holds position
  on a zero mask (§25) with the operator's stiffness, and is still refused without one (F162).
- Proposed amendments `docs/issues/travel-has-two-ends.md` (F159),
  `the-plan-never-reaches-the-kernel.md` (F158) and `four-legs-in-a-two-leg-vocabulary.md` (F163),
  since ratified as §§92–94 (see above).

- Remote-policy provenance (§90.6): the recording marks which cycles ran on a remote policy's
  proposals, with the session id and the model digest. `skynetp --backend remote` writes each
  session fact (opened, first proposal, last proposal) into a queue beside its bridge socket;
  `skynetd` carries them into the ring as `remote_session` events (a new §26 event code) on each
  reconcile pass, and a reader joins them to cycles by the executed command's stamp. The
  daemon's recording index and `skynet blackbox` list each session with its digest, its commands
  and the cycles it drove. Facts the policy host could not queue are recorded as a gap.
- Two §74 telemetry counters, `remote_proposals` and `remote_stale` (§74.7): what a remote
  policy proposed, and the turns it stayed quiet because its newest chunk was older than its
  deadline, carried from the same queue as deltas. Unmeasured on a robot with no remote policy,
  and in a window where a policy host stopped without writing its final count. Every edition's
  vocabulary and the corpus carry them.

- Driver release stages: every driver that commands a physical robot carries a stage in
  `drivers/models.json` — `alpha` (tested only against counterparts written in this
  repository) or `beta` (also against the maker's own controller software or simulator, or
  an independent implementation of its protocol) — with what the stage rests on, per body
  for `ros2_control`. `skynet setup --robot` prints it before installing and on every
  refusal, and says first that the driver has never run on a physical robot. `stable` is
  reserved for a driver that has, and none has; `tools/check-release-stages.mjs` holds the
  manifest, `docs/status.md` and the nightly workflow to each other in CI.
- Nightly substrate workflow (`.github/workflows/substrates.yml`): every beta driver runs
  against its maker's software each night at the pinned versions and again at the makers'
  newest — URSim for both RTDE drivers and the UR5e behind `ros2_control`, DRCF for the
  Doosan M1013, Unitree's MuJoCo simulator, AgiBot's own X1 deployment, franka-sim, and the
  EGM client — so a vendor release that breaks a driver is found the night it ships. The
  UR5e and X1 substrates are new pinned images (`ros2_control/ur/Dockerfile`,
  `x1/sim/Dockerfile`); before them each could be run on one machine only.

- Supervisor watchdog: `skynetd.service` is `Type=notify` with `WatchdogSec=120s`;
  the daemon sends `READY=1` once serving and `WATCHDOG=1` from the loop body, so a
  hung loop is restarted within two minutes. `install.sh --check` pins both the
  watchdog and the deliberate asymmetry — `skynetk` must *not* be watchdogged, since
  a hung control loop holds rather than restarting into unknown hardware state.
- Pass overrun warnings and `GET /metrics`: each reconcile pass is timed against its
  interval, overruns warn per pass, and the route serves the daemon's counters
  (passes, overruns, requests, uptime) beside the live queue and unit counters —
  rendered as the console's Supervisor panel and the `passes` row of `skynet doctor`
  when overruns exist.
- Machine-readable API error codes: every §33.7 refusal carries a `code` beside its
  detail — the daemon's `api.*` vocabulary plus the pairing and skill codes passed
  through — all catalogued for `skynet explain` (51 new entries, `docs/error-codes.md`
  regenerated), with the suite scanning the emission sites against the table. The CLI
  prints the code and the lookup on every refused request.
- Kernel crash parity: `skynetk --crash-file` (wired in the unit file) writes panic
  reports in the shared `skynet-crash` format; the supervisor's every pass takes the
  file, logs it at error, records `kernel_crashed`, and serves it as `kernel_exit` —
  the `kernel` row of `skynet doctor` and a console banner. Hook writes are atomic
  (sibling + rename) so a per-pass reader never catches a half-written report.
- Four-state previous exit: a clean-stop marker (a `Drop` guard that writes nothing
  while unwinding) joins the crash file, so `GET /health` reports `clean`, `crashed`,
  `unclean` (killed, OOM, abort, or power loss — the journal names which) or `unknown`
  (first boot) instead of guessing. Doctor and the console banner the two abnormal
  states; `docs/troubleshooting.md` gained both rows.
- Release profile unwinds: `panic = "unwind"`, because the crash-report hook only runs
  on unwind and every release-build crash — every crash on a robot — previously read
  as an unrecorded kill. Safe for the control loop (the RHAL ABI has no callbacks
  into Rust), and pinned by a CI step since no test could catch a revert.
- Failing console panels name their code: the page printed the refusal's sentence
  without the `code` beside it, so nothing on screen could be looked up. Every
  failing panel now prints `sentence [api.*]`, and the suite pins the three fields
  on all four refusal shapes (401, 403, 404, 405).
- The installer narrates: `install.sh` numbers and times its steps (`✓ [3] (+45ms)`),
  mirrors every line to `--log-file` (same bytes, no color codes), and gives a
  sentence to the failures that used to exit silently — a `run` wrapper carrying the
  command's own words for the systemctl calls, and an exit trap naming the step any
  other death stopped after. Pinned by `packaging/test-install-output.sh` in CI.
- `skynet setup`: the install wizard, running on the PC. Host and driver are the whole
  of the required input — the body is derived from the driver and the bundle fetched
  for the robot's architecture (checksummed, `get.sh`'s scheme) when either is
  missing, each named in the plan. It copies a stage over SSH (one tar pipe,
  byte-count verified), runs the installer remotely while streaming its numbered
  steps, verifies with `--check`, and keeps a local `skynet-setup-<host>.log` —
  refusing before the copy when the robot is unreachable, the wrong architecture
  (read from the ELF headers, the one failure the installer cannot catch), or not
  writable by the SSH user. `--dry-run` prints the plan and every ssh invocation
  exactly. Covered by 25 CLI unit tests and `packaging/test-setup-wizard.sh`
  (stub ssh, real tar, real fetch over file://) in CI.
- The wizard takes `--robot <model>` (`drivers/models.json` maps all twenty-four: built
  libraries resolve from the bundle or the checkout, the rest refuse typed — missing
  SDK, simulation-only, Python-under-a-host, unmapped), offers to start the units and
  run the first health check after installing (both best-effort; `--yes` prints the
  commands instead), verifies downloads with `certutil` on Windows, and asks one fewer
  question (the SSH user defaults silently and shows at the confirmation). The
  developer guide's step 8 is now the one documented install path.
- `skynet doctor` reads fourteen sections: `metrics` (the counters behind health) and
  `audit` (who changed what, when) join the bundle; the architecture's section count
  is updated in both copies and re-pinned.
- JSON log format and operation IDs: `skynetd --log-format json` prints one
  JSON object per stderr line (service, wall timestamp, level, operation,
  message) for fleet log consumers; start-up, each reconcile pass and each
  served request run inside an operation (`boot`, `pass-N`, `req-N`) carried on
  every line. The text default is byte-identical.
- Crash reports (§64): `skynetd` installs a panic hook writing
  `state/last-crash.json`; the next boot logs it at error, records a
  `supervisor_crashed` black-box event, and carries it on `GET /health` as
  `previous_exit` — bannered on the console health panel and the `previous` row
  of `skynet doctor`.
- Troubleshooting guide (`docs/troubleshooting.md`): symptom-first reference from
  install failures to fleet mysteries, held to the same mechanical checks as the
  golden path — every command it shows is one the CLI dispatches, every `make`
  target exists, every link resolves.
- Versioned documentation: every `v*` tag publishes the checked documentation set
  to GitHub Pages under `versions/<tag>/` with a `latest/` copy and an index
  (`.github/workflows/docs.yml`, `tools/build-docs-site.mjs`).
- Apptronik Apollo simulator driver (`drivers/apollo`): the menagerie MJCF at a
  pinned commit through `skynet_mujoco` — 81-check suite, layout cross-check,
  sim conformance, Guest tier.
- RobotStudio runbook (`docs/robotstudio-runbook.md`): the Windows-side procedure
  for the ABB-live EGM substrate, plus the no-RobotStudio fake-controller route.
- KUKA KR 6 R900 over RSI behind `ros2_control` (`make -C drivers/ros2_control
  check-rsi-docker`): KUKA's own `KukaRSIHardwareInterface` against KUKA's own
  `kuka_rsi_simulator`, in a pinned image at the release set ROS Jazzy ships — the pose
  read, an axis moved and brought back, every closed session ended by RSI's stop flag as
  the simulator's own log counts it. The body is `beta` and runs in the nightly substrate
  workflow at the pins and at KUKA's `master` — there with the description KUKA's `master`
  builds, since KUKA moves its plugin and its descriptions together.
- AUBO i5 against ARCS behind `ros2_control` (`make -C drivers/ros2_control
  check-arcs-docker`): AUBO's own `AuboHardwareInterface` against ARCS, AUBO's controller
  software, installed in a container from AUBO's own installer with AUBO's simulated arm —
  a session against the arm powered off refused rather than reported done, the arm powered
  on by AUBO's own SDK example, the elbow moved and brought back on the controller's
  readback, and the controller out of servo mode after every closed session. The body is
  `beta` and runs in the nightly substrate workflow at the pins and at AUBO's newest
  installer and branches. One line of AUBO's plugin is patched to build on Jazzy; ARCS has
  no model for the i5S, which stays `alpha`.
- Ratified amendments §§92–96 (owner instruction *Decisions* 21, 2026-09-28): two-ended travel
  (§92, F159), the plan's route to the kernel (§93, F158), quadruped leg roles (§94, F163),
  refusal-before-framing for oversize frames (§95, F172), and the service-network switch (§96,
  F146). The specification carries all five in both copies.
- Quadruped roles (§94): the leg-chain index counts legs front-to-back per side and the
  suggester reads corner words, so the Go2 reference policy binds 14 quadruped descriptions
  the fleet could not touch before and `transport` binds 18. Seven new corpus cases (1493).
- Frame transport (§95): a sensor frame that does not fit its pipe is refused `EINVAL` before
  framing instead of tearing the stream, and the MuJoCo driver clamps its scenes to fit.
  `size_checks` in the driver selftest, `check-frame` on the host side.
- The service-network switch (§96): services start sealed, and the console grants one its
  network per service as the capability token `platform.network` — `POST
  /packages/{id}/network`, durable in the desired state, landed by a restart. e2e section 4b
  and `sandbox.sh` section 6b prove it, the latter by attempting a TCP connection from inside
  the sandbox.
- SAFE clears locally (F149): `POST /state/clear` serves the robot's own socket only and
  refuses the LAN with `state.clear.remote`.

- `drivers/xarm` — a UFACTORY xArm over the controller's own wire (`DRY-COVERAGE-PLAN.md` item
  10.5). The wire was read out of UFACTORY's SDK source and is held to it byte for byte by
  `layout_check` (40 checks); 89 checks run against a controller written here, and 20 against
  UFACTORY's own controller firmware (`xarm6-type6-controller` 2.4.0, in the simulator image their
  support article names) with the vendor's `XArmAPI` reading the same controller as a second
  opinion. The descriptor's `simulator` is the firmware's own `is_simulation_robot`. `beta`, against
  the firmware started as each of the five arms UFACTORY's image carries (xArm 5, 6, Lite 6, 850, 7:
  `make check-sim-models`). Run nightly by `substrates.yml`.
- Both CANopen drivers are `beta`: they run against `ros2_canopen`'s CiA 402 slave on Lely's CANopen
  stack, on a kernel `vcan` bus (`make -C drivers/canopen check-lely-docker`, item 10.7) — an
  implementation of CiA 301 and 402 this repository did not write. Python 108 checks and 34 passed
  through the real ABI per driver, with each slave's own log agreeing on every stop and every close.
  It is a protocol counterpart and not a vendor's drive. Run nightly.
- Nine Doosan arms behind DRCF (item 10.6): the M0609, M0617, M1013, M1509, A0509, A0912, E0509, H2017
  and H2515 — every arm DRCF emulates that Doosan's own description covers — each against an emulator
  started as that arm, whose startup line the target waits for, so a description cannot pass against
  the wrong arm (`make -C drivers/ros2_control check-drcf-models`, 468 checks and 34 passed each).
  `doosan/generate.py` writes the URDFs and provisioned descriptors from the vendor's own xacro;
  `check-doosan-urdfs` says when a committed file is not what it writes. Every URSim model was run the
  same way before (`check-ursim-models`).
- The fleet sweep gains a `roles` stage (item 10.8, F165): one accepted role table per arm family
  in `fleet/roles.py`, in LeRobot's own spelling, applied to a body only where the axes MuJoCo computed
  for it at the zero pose agree with the relations the table asserts, and refused by name where they
  do not. A skill trained on one robot now binds to 85 of the 94 arm descriptions, 102 descriptions in
  all (from 18). `fleet/test_roles.py` is the refusals, run in `invariants`. The fleet is 224
  descriptions of 177 robots, with the eight Doosan descriptions and the xArm routes added.
- Proposed amendment `docs/issues/flex-and-pitch-are-one-joint.md` (F177): the vocabulary has `flex`
  and `pitch` for one hinge and §6 matches by exact key.
- `drivers/mavlink` — an ArduPilot vehicle over MAVLink, the first driver for aerial and ground
  vehicles (`DRY-COVERAGE-PLAN.md` Phase 11, item 11.1). Command-level, Guest tier: the body has no
  joints and is moved by named motions (`arm`, `mode`, `takeoff`, `goto`, `velocity`, `hold`, `rtl`,
  `land`) under `vendor.mavlink.task`, in REP-103 frames. `beta` against **ArduPilot's own flight code
  built for x86**, pinned by commit: a multirotor, a rover and a fixed wing armed, flown and stopped
  through the real ABI (88 live checks, 34 passed through the ABI each) with `pymavlink` reading the
  same vehicle on a second serial port, the link impaired with delay, jitter and loss, and a boat and a
  submarine refused by name. The codec is held to the reference implementation (193 checks, six
  transcription errors caught on its first run). Run nightly by `substrates.yml`.
- `docs/robot-support.md`, generated from `drivers/models.json` and the new `drivers/robots.json` by
  `tools/robot-support.mjs` and held by `--check` in CI: which robots have a driver, how far each was
  tested, what its counterpart has not shown, what a dry test can and cannot see, and every claim that a
  driver cannot be written, re-checked against the public record.
- Proposed amendment `docs/issues/no-base-kind-for-water.md` (F178): §5's base kinds have no word for
  a hull, so the MAVLink driver refuses a boat and a submarine by name.

- The fault-vocabulary contract (`DRY-COVERAGE-PLAN.md` 12.C.6, the dry proxy for layer 9): `drivers/fault-vocabulary.json`
  says what each code a maker defines is *for the robot* — healthy, degraded, a fault, or a stop — and what a code in no table
  escalates to, and `tools/fault-vocab/contract.py` injects each one through a driver's own read and write and holds it to the
  flags §27 reads, a command refused while the controller is ignoring it, the fault word, and recovery. The xArm, Franka and
  CANopen tables are generated from the makers' own sources (UFACTORY's SDK, `libfranka-common`, CANopenNode), UR's from names
  read out of the maker's controller binary, and Spot's from Boston Dynamics' `robot_state.proto` and the SDK's compiled descriptors.
  Bound to `ur_rtde`, `xarm`, `franka`, `canopen`, `ethercat`, `mavlink` and `spot` and the four
  Rust ports (a body moved by motions says which stay available in a failsafe, `keeps`, and the contract holds the driver to
  answering them), with a static check (`tools/check-fault-vocab.mjs`) that refuses a table the contract cannot hold, a driver with no
  entry and no reason, and a bound driver that is not wired into CI, the confidence matrix and the seeded campaign.
- Proposed amendment `docs/issues/fault-flag-is-not-read.md` (F197): §27 reads `RHAL_DEGRADED` and `RHAL_ESTOP` and nothing reads
  `RHAL_FAULT`, so a drive that has faulted and ignores commands leaves the operating state at `present`.

### Fixed

- **The first run of `e2e.sh` and `sandbox.sh` under a real systemd, and six defects it found** (`DRY-COVERAGE-PLAN.md` F287–F293; neither script had ever run on a machine with a system manager outside GitHub's runners, where neither had been reached). **A robot's power-on verdicts were written over when the black box wrapped**: `skynetk` records its five §48.2 verdicts once and `skynetd` reads them off the ring every pass, so at 1 kHz and the default 64 MiB a robot that had passed its self-test read as `post_incomplete` and commanded nothing after four and a half minutes. `Recorder::restate_post` writes the verdicts and the worst case again each quarter of the ring (shown red first: the verdicts are gone from a ring that has wrapped three times). **A daemon pass cost eight seconds in a debug build**: the POST read decoded and sorted the whole ring to find five events and the empty-slot test was a byte loop; `Ring::events` reads the kind byte first and `decode` compares a chunk at a time, so a pass is 1.2 to 1.5 s there, and `e2e.sh` against the debug binaries `ci.yml` builds passes in 139 s where CI's run had failed twelve checks after eleven minutes. **The workspace did not compile for Windows** (`skynet-policy-host`'s bridge and its test module, and two functions only the Unix loop calls) and had not since §90 was built. **`sandbox.sh`'s control skill declared no action space**, was granted no channels and exited before commanding anything, so its `failure` verdict said nothing about the judge; it now declares the skill's own and replaces it. **`e2e.sh`'s open-chain scenario had been unreachable** since the self-test learned to release a stop; the mock gains `fault=chain_open`, a circuit that opens after a healthy start and survives the release. **GitHub skipped what the local runner ran**: 25 steps of `ci.yml` had no `if: ${{ !cancelled() }}`, so a failure hid the four `rust` and seventeen `abi` steps after it; `tools/check-ci-steps.mjs` is an `invariants` step and holds the file to the runner's claim. Nothing has run on `main` since 2026-10-06 12:23: the two newest runs ended in seconds with no runner assigned, which is an account matter and not a code one.

- `drivers/xarm` took a joint's range from the controller alone. UFACTORY's simulator reports the xArm 6's
  ranges whatever arm it starts as, so on four of the five models the driver would have allowed a wider
  range than the vendor's own SDK gives that arm (an xArm 5's wrist bend landed on the wrong joint). Travel
  is now the narrower of the controller's range and the vendor's per-arm table, said aloud when they
  disagree (F181).
- `drivers/mavlink` sent a velocity setpoint with a yaw rate of zero where none was given, which
  ArduRover reads as "drive straight along your heading": told to go north it drove southeast and
  never turned (F179). The yaw rate is ignored unless the caller supplies one.
- Every robot booted into a `SAFE` it could not leave: `skynetd` judged §27 before opening the
  recording and read an unobservable RHAL as lost. §27 is now first judged one interval after
  the recording opens, over cycles this process watched. The proximity reading, which counted
  an always-empty interval, shares that interval now.
- A skill started after any other policy had run was aborted `guard_tripped` for the kernel's
  watchdog holding an idle arm. A skill is now charged only with cycles in which a guard judged
  a command.
- The mock arm treated a position setpoint as a rate and ran past it. A position-mode joint now
  moves toward its setpoint.
- `skynet driver check` probed "accepts a well-formed command" in position mode on every body, so
  a body that declares torque alone — every quadruped in DeepMind's menagerie — reached no tier for
  refusing a mode it never offered. The probe now uses a declared mode, gentlest first (F161).
- `skynet descriptor from-urdf` read a joint's axis in its own rotated frame, calling every joint
  of a Universal Robots arm a yaw; it now reads the axis in the body's frame at the zero pose (F166).
  It also copied the sentinels URDFs write for "unlimited" — `effort="1e30"`, `lower="-1e16"`,
  `effort="-1"` — into descriptors as ratings; they are now left out and named (F168).
- `drivers/mujoco` ignored actuator gear: a geared servo settled at its command divided by the
  gear, a geared motor applied gear times the torque, and every force read back and every force
  ceiling was in the actuator's units rather than the joint's. A `<motor>` bounded by `ctrlrange`
  declared no rating at all. Both are carried through the gear now (F167). A moving base declared
  for a scene whose root is fixed to the world is refused rather than written into a descriptor
  whose own provenance said the base was fixed (F164).

- `drivers/ros2_control` reads and writes its hardware plugins in turn — one write after
  each read, as `controller_manager` does — instead of when its caller does. A
  request/reply plugin (KUKA's RSI client) wedged on the first read's second read and
  then reported its first telegram's pose as live under fresh stamps; a command with no
  read waiting now goes with the next turn's write, and the first read no longer sleeps
  when it already holds a sample.
- `drivers/ros2_control`'s `close()` deactivates its hardware plugins before shutting
  them down, so a plugin's own stop path runs: KUKA's sends RSI's stop flag in
  `on_deactivate`, and the controller of every closed session had been left to find out
  by timing out.
- `drivers/ros2_control`'s `open()` refuses a body whose hardware plugin failed to load or
  to initialize. The resource manager reports no component at all after either failure, so
  the check that every component came up active had nothing to refuse, and `open()`
  succeeded with no plugin behind the body — as it did for KUKA's plugin at `master`, handed
  the URDF of KUKA's last description release. The check now walks the components the URDF
  declares.
- `drivers/ros2_control` keeps reporting a hardware plugin that failed. A plugin's failed
  read or write — or its asking to be deactivated — takes its component down for the
  session, and the resource manager then answers every later read and write OK, so the
  bridge reported the one failing call and then a healthy state with every position NaN,
  and after a deactivation accepted every command and sent it nowhere. Every read now
  reports the fault and every command is refused until the body is reopened, pinned in the
  default suite by `ros2_control`'s own test component (`make check` now needs
  `ros-jazzy-hardware-interface-testing`).

- `skynet driver check` crashed on a driver built against another ABI: it took a reference to the
  whole vtable before reading `abi_version`, reported the mismatch, and carried on calling through a
  table whose layout it had just said it did not know — under v2 a slot a v1 table does not have. It
  now reads the version through the pointer and ends the run at `rhal.abi.version` with nothing
  called (F173).
- Both CANopen drivers refused a healthy drive that took time to change state: they wrote a
  controlword and read the statusword once, straight after. A drive spends seconds after power-up in
  `not ready to switch on`, and one that evaluates its controlword on its control cycle acknowledges
  the write before its state machine runs. They now write the controlword on every pass for a bounded
  second and stop at once on a latched fault (F174).
- `drivers/xarm` refuses a servo target beyond a joint's range before sending it: the firmware
  acknowledges one with a clean reply, moves toward it, and faults afterwards (F175).
- Five drivers reported a fault in a flag the OS does not read. §27 leaves `present` only on `RHAL_DEGRADED`, so `drivers/franka`,
  `drivers/ethercat` and their ports, and `drivers/xarm` for a fault that does not stop the arm, raised `FAULT` alone and an arm
  that had faulted read as healthy. They raise `DEGRADED` beside it (F197).
- `drivers/ur_rtde` and its port answered `SKR_OK` for a servo target written to an arm in a protective stop, a safeguard stop,
  a violation or a fault, because a register write succeeds whatever the arm is doing, and read all of those but two as merely
  degraded. A stopped arm is now `FAULT` and its commands are refused; a safety status the driver has never heard of — the
  maker's controller names twenty-three and the table numbered nine — is read as the stop chain not known to be closed, and
  not as degraded (F198, F199).
- `drivers/canopen`, `drivers/ethercat` and `drivers/franka` and their ports answered `SKR_OK` for a command a faulted drive, or an
  arm in guiding, recovery or a person's stop, drops. A command is refused whole while any drive of the body holds a fault: the
  other axes are no longer walked on towards a target (F198).
- Three drivers read a maker's state and said nothing. `drivers/canopen` and its port discarded the emergency object — the
  drive's own account of why it faulted, and every warning it raised while it ran on — and put the node, which is not the maker's
  code, in the fault word; the word is now `node << 16 | code`. `drivers/xarm` ignored the controller's warning code, and
  `drivers/franka` every robot mode but two (F200).
- `drivers/mavlink` read three of `MAV_STATE`'s nine values, as `FAULT` alone, so a vehicle flying its own failsafe read as
  healthy to the OS; a vehicle powering down, one still booting and a state the dialect does not define read as nothing. It
  reads all nine now, a failsafe as `FAULT` and `DEGRADED`, the autopilot ending the flight as `ESTOP`, and answers the way out
  of a failsafe (`hold`, `rtl`, `land`, `disarm`) while refusing a motion that takes the vehicle somewhere new (F197, F199,
  F200, F202).
- `drivers/spot` raised `FAULT` alone for any fault, which §27 does not read, and put a *count* of faults in the fault word where the
  maker's own `code` belongs; read an informational fault ("No hardware problem") as a fault, a hardware estop slot in any state but
  "stopped" as released, and motors in the error state as healthy; and sent `stand` to a robot reporting a critical fault. It reads
  the four enumerations Boston Dynamics defines in four classes now, carries a system fault's `code` (a behaviour fault's cause, a
  motor state, under a mark), refuses `power_on`, `stand` and `selfright` while the robot is not doing what it is told, and every
  motion while an estop is not released, while `stop`, `sit` and `power_off` stay answered (F197, F198, F199, F200).
- `drivers/unitree` and `drivers/booster` reported a joint in trouble (a non-zero `lost` on the Go2, H1 and T1, `motorstate` on the G1
  and H1-2) with a fault word that was the joint's ordinal alone, under a comment that called it the vendor's word verbatim, and
  nothing tested it: the fake published zero for ever. The word now carries the joint (1-based, high half) and the vendor's own number
  (low sixteen bits), and a second fake per body reports one joint in trouble so the suite holds it (F205).
- `drivers/fairino` carried the controller's error number in the fault word and raised no flag for it, so by §27 a controller reporting
  an error read as healthy for as long as the error stood, and a command was sent to it anyway. A non-zero `main_code` now reads
  `FAULT` and `DEGRADED`, `write_command` refuses it and names the error, and a zero code reads healthy again (F206; the suite's
  new checks fail against the driver as committed before).
- `drivers/mavlink`'s own suite went red one run in three on a machine with every core busy, on the same two checks each time — a
  retry after a lost acknowledgement, with a 200 ms deadline for the reply — and its seeded-defect campaign refuses to start from a
  suite that is not green three times running. The checks that are about what the vehicle says, and about the retry, now run on a
  rig with a two-second deadline and a rig each; the one that is about the deadline keeps 200 ms (F207).
- `skynetd`: `robot.guards.active` was left absent on every robot on the argument that the daemon has no channel to the loop. The loop writes `active_guards` in its own limits report; the daemon now publishes that list, and only while its reading of the
  recording shows the loop turning, so a report left by a dead loop is not a claim and an empty list is the loop's word and not a default (F204).
- `skynetd`: `robot.state.base_pose` was left absent on every robot, though the loop records the driver's pose in every cycle (all `NaN` for
  a base nothing localises, §24) and the daemon already reads the joints and flags from that record. It is published now when all seven numbers
  are finite and the quaternion is a rotation, and otherwise absent: a driver that wrote zeros where §24 asks for `NaN` would have had the robot
  reported at the origin facing nowhere. `BodyReading::base_pose` is the rule, and a cycle whose read failed is no pose (§26.4).
- `skynet skill check` could not tell a path §54.2's grammar has from a path a robot supplies: it built its context with every branch populated,
  so `robot.state.gripper` and `robot.tier`, which the daemon never fills, passed. `skynet_contracts::live::DAEMON_NEVER_FILLS` is now the
  daemon's own list of the branches it never fills, each with the reason, a test in `skynetd-io` fails when the daemon starts filling one, and
  the new check `skill.context.supplied` fails a package whose precondition or success criterion reads one — saying the skill is refused on
  every robot and that the fix is a source for the branch, not a weaker clause. The second new check, `skill.context.on_body` (with `--body`),
  names each joint a clause reads that the body does not have, beside the joints it does. Of the Hub's 24 skills, 21 fail the first (they read
  `robot.state.gripper`), and the 23 that name a joint resolve on one of the five bodies they were listed as verified on, not five (item 12.F.3;
  `docs/issues/skill-clauses-name-one-bodys-joints.md`, `docs/issues/gripper-has-no-open-end.md`).
- The suites of `drivers/fairino` and `drivers/lite3` asserted orderings a loaded machine does not promise, and failed one run in five to one
  in twenty-five with every core busy: a read was required to return a *new* frame inside the driver's own bound, so a fake that was a period
  late made a stalled-producer report (which §24 says it should) into a failure; the lite3 tests took "the last datagram once any has arrived"
  for the joint command when a write after the reclaim guard sends control right first; its stale-read count was an absolute number that a
  push in flight when the body was paused moved; and fairino's test of the stop frame raced the next XML-RPC call across two sockets. Each now
  states what the driver promises (fresh, or having waited its bound; the joint-command datagram itself; a count across the section that
  makes it certain; the stop taken before the release). A flaky base suite would have counted as a defect noticed under the seeding
  campaign's parallel workers (F210).
- `tools/flake-hunt.sh <runs> <command…>` runs a command N times quietly and N times with a busy loop on every core, prints each failing
  run with the eight lines before its FAIL, and exits 0 only if all are green: the procedure that found F210, so a suite's load-sensitivity is
  measured before its seeding campaign counts a red as a defect noticed.
- `packaging/rt-vm.sh timing` can run under KVM without the `kvm` group, through a container given `/dev/kvm` and nothing else (`RTVM_PREBUILT=1`
  uses the `skynetk` and mock body already built; the header has the command). Four such runs on this machine, itself a Hyper-V guest, put the
  real-time kernel's stress p99.99 at 3.6 to 16.0 ms and the stock kernel's at 2.1 to 12.6, in a different order each time, so the script now
  says when a hypervisor sits above it and that a failed budget row is then not a finding about the kernel; the assertions are unchanged, and
  a stall of known length was found eight times in eight (F211).
- `drivers/unitree` speaks the Unitree B2 (`model=b2`; item 12.E.2): the Go2's twelve slots and leg order (the vendor's B2 example and its
  `b2.xml` agree), the model's own travel, and the maker's page figure "About 360N.m" as a ceiling on every joint with its missing footnote
  said in the envelope. The suite passes all five bodies against a fake (90/91/93/92/96 checks), and in Unitree's own simulator the B2 stands
  from 0.126 m to 0.625 m, holds under the all-zero mask and folds on the stop, with every slot held to the actuator of the same name in
  `b2.xml`. The first B2 descriptor was not JSON: its envelope quotes the maker's words and the builder wrote the string between quotes as it
  stood; the driver's suite passed it and the conformance run's parse refused it, so the string is escaped (`json_escaped`) and the suite
  pins the escaped quotation (F212).
- `drivers/unitree` speaks the Unitree H2 (`model=h2`; item 12.E.2): thirty-one joints on the humanoid message, the vendor example's
  `H2JointIndex` for slots (contiguous, held to `h2_mujoco.xml`'s actuators by name), the model's own travel, roles for a three-joint waist and a
  two-joint head, and the maker's page figures (360 N·m for leg joints and 120 N·m for arm joints, with the page's footnote) applied to the hips,
  knees, shoulders and elbows only — the ankles, wrists, waist and head declare none. Its ankle is roll then pitch where the H1-2's is pitch then
  roll. The suite passes all six bodies against a fake (the H2 96 checks) and in Unitree's own simulator (19 checks). The simulator check's "arm
  went where it was sent" bound was a flat 0.15 rad and the H2's heavier arm stood 0.21 rad off its setpoint under 12.9 N·m at kp 60 — the drive's
  own law, load over stiffness — so the bound is now the larger of 0.15 and the measured load over `kp` plus 0.05, and the H2's arm gets three
  seconds to settle where the others get one (F213).
- `drivers/unitree` speaks the standard Unitree R1 (`model=r1`; item 12.E.2): twenty-six motors in the vendor example's sparse slots of the
  humanoid message (0 to 13, 15 to 19, 22 to 26, 29, 30), the vendor model's travel, and **three things it declares nothing about** because
  nobody published them: the envelope's peak torque and every joint's rating (the page's arm line reads "About 2kg", a load), the head's travel
  (the vendor's model has no head), and a rate. The descriptor builder could only print a number for the envelope's peak and for each joint's
  travel, so it now prints `null` for an unpublished peak and leaves out an unpublished bound (F215). The simulator check excuses a joint from
  its by-name slot check only when the body's row lists it as unmodelled and its slot is beyond the model's actuators — the R1's two head
  joints — and says so in a note. The suite passes all seven bodies against a fake (the R1 95 checks) and in Unitree's own simulator (19).
- Every Unitree row now pins one travel figure inside one named joint's own limits, where it had pinned a bare substring that a different joint
  with the same figure could satisfy (the H2's two hip pitches); a seed for each row corrupts exactly that joint (F214).
- `tools/faultproxy/chaos.py` spelled the RHAL's flags `0x0004` and `0x0010`: DEGRADED's bit and the homing field's low bit, which
  every driver that answers `not_required` sets on every read, so its check that a driver says it is degraded could not fail
  for the xArm and CANopen drivers. It imports the ABI's now (F196).

## [0.1.0] — unreleased

No tags exist yet, so everything above is also everything below. The first `v0.1.0`
tag will exercise the release bundle, the installer-against-bundle test and the
documentation publish for the first time; until then this file describes the tree,
not a release.
