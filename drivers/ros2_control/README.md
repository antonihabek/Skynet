# skynet-ros2-control — a body behind `ros2_control`

`DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this driver its substrate as a *"fake
`hardware_interface`"*, and `ros2_control` ships one: `mock_components/GenericSystem`, the fake its
own test suite runs against. So every line below is exercised against the real framework with no
robot, and against a fake nobody in this repository wrote — which is the only kind of dry run that
can be wrong in the project's favour.

```bash
make check              # 507 checks against mock_components and ros2_control's own test component
make check-mock-docker  # the same, in a Jazzy image built from mock/Dockerfile, for a machine with no ROS 2
make check-conformance  # the sim half of SCTS, through dlopen and the real §24 ABI
make lib                # libskynet_ros2_control.so on its own
make check-ursim-docker # the UR5e behind URSim, UR's controller software, through UR's own plugin
make check-drcf-docker  # the M1013 behind DRCF, Doosan's emulator, through Doosan's own plugin
make check-drcf-models  # the same block once for each of the nine arms Doosan's description covers
make check-egm-docker   # an ABB IRB 1200 over EGM, the open client against a fake built from egm.proto
make check-rsi-docker   # a KUKA KR 6 R900 over RSI, KUKA's own plugin against KUKA's own simulator
make check-arcs-docker  # an AUBO i5 against ARCS, AUBO's controller software, through AUBO's own plugin
```

`ROS_DISTRO` and `ROS_ROOT` override where it looks. With no `ros2_control` installed the Makefile
refuses and builds nothing: a driver that compiled without its framework would be a driver nobody
could load. `make check` also needs `ros-jazzy-hardware-interface-testing` — `ros2_control`'s own
test components, which fail on command — and refuses by name without it.

## It hosts a `ResourceManager`, and never talks to a `controller_manager`

The obvious shape is a ROS 2 node that publishes to `controller_manager`. This is not that, and §11
is the reason: *no control decision may require a network round trip*. A setpoint that crosses a DDS
domain on its way from the Safety Kernel to a drive is a control loop with a network in it, whatever
the network is made of — §25's ring exists precisely so that the loop does not have one.

`hardware_interface::ResourceManager` is the layer *underneath* `controller_manager`. It loads the
hardware plugins, owns their lifecycle, and hands out state and command interfaces. That is the
whole of what a §24 driver needs, it is in-process, and it works on a machine with no ROS graph
running at all — no daemon, no discovery, no domain ID.

What that costs is stated rather than absorbed: **a robot driven this way is not also driven by ROS 2
controllers.** `ros2_control` allows one claim on a command interface and this driver takes it. That
is the same exclusivity §25.2 gives a capability lease, arriving from the other side.

## The place the two models do not meet

§24's command carries **one mode for the whole body**. `ros2_control` declares command interfaces
**per joint**, and nothing stops a URDF giving joint A `position` and joint B `velocity`.
`urdf/mixed_interfaces.urdf` is exactly that body, and it is in the suite because it is not a corner
case — it is how a mobile manipulator is usually written.

There is no honest way to command such a body through §24 as it stands. So the descriptor declares
the **intersection**: the modes every joint offers.

| URDF | `control.modes` |
|---|---|
| both joints `position` + `velocity` | `["position", "velocity"]` |
| one `position`, one `velocity` | `[]` |

A body whose joints share no mode is reported with no control modes at all, and every
`write_command` against it is `SKR_EINVAL`. That is the truthful descriptor for it, and
`skynet driver check` reads it exactly as it reads `drivers/pendant` (§59.2): the joints are
reported, and there is no mode in which this driver commands them. The alternative — picking a
mode that works for most joints and quietly dropping the rest — is the failure §24 is arranged to
make impossible.

The mismatch itself is a specification question rather than a driver bug, and it is written down as
one: `DRY-COVERAGE-PLAN.md`'s F30, written out as `docs/issues/per-joint-command-modes.md` for a
maintainer to carry to both copies of the architecture.

## What it reads, and what it will not declare

Position and velocity limits come from the URDF's `<limit>` because they are what they say they are.
A **torque rating** does not. A `<limit effort="...">` is a design value from a CAD model, not a
datasheet safety rating — §63.4, and the same rule that makes `skynet-urdf` write a literal null
rather than copy one across. `urdf/two_joint_mock.urdf` carries `effort="80"` and the suite asserts
the descriptor does not: `effort_max_nm` is absent, `envelope` is null, and the guards that need one
report unavailable rather than guessing.

**Roles are the operator's and are never inferred.** A `ros2_control` URDF names joints and says
nothing about what they are; `shoulder` in one robot is a pitch and in the next a yaw. So §5.1's
role is `null` unless the configuration carries one:

```
urdf=/path/robot.urdf roles=shoulder:right_arm.shoulder.yaw;elbow:right_arm.elbow.flex
```

Without it `embodiment.roles.coverage` fails and the report names the joints nobody has claimed,
which is §63.4's behaviour — a descriptor that waits for a human beats one that invented a mapping
the Safety Kernel then bounds travel with.

**A state interface that is declared and cannot be read is NaN, never zero** (§24). The mock URDF
declares no `effort` interface, so effort reads NaN; a zero there is a joint reported as unloaded,
which is a specific and dangerous lie. `ros2_control` has no homing concept at all — a plugin either
reports a position or does not — so the two-bit homing field is `unreported` rather than either
answer.

## The second body: a UR5e nobody here wrote

`urdf/ur5e_mock.urdf` is generated from Universal_Robots_ROS2_Description — the header has the
commit and the command — with the vendor's own mock `hardware_interface`. So the fake is still
theirs and the driver is still ours, and the six joints and their limits are the vendor's too.
This is Phase 7's first substrate for this driver, and F69's body: the bridge hosts a body it can
name. The descriptor carries the URDF's own `<robot name>`, slugified the way `skynet descriptor
from-urdf` does (`ur5e` here, `two-joint-mock` there); the selftest and `skynet-urdf`'s suite pin
both halves of that agreement to the same inputs.

What the vendor file changes about the suite's answers: the union carries `torque`, because the
vendor's mock offers `effort`; and effort reads finite, because the vendor declares the state
interface. What it does not change: `<limit effort>` is still not read (§63.4), even though its
150/28 Nm happen to equal the vendor's published max joint torques.

`urdf/ur5e.provisioned.json` is that body's §28 provisioned descriptor: `driver check
--descriptor` wrote it and the integrator completed it with the transcribed ratings — the UR5e
tech sheet's axis-movement table plus UR's "Max. joint torques" support article, the same figures
as `drivers/ur_rtde`'s `MODEL_RATINGS`. `skynet readiness` on it reads guest attainable with two
host blockers left: `watchdog`, because a mock has no controller to fault, and `onboard_compute`,
because that is measured where the OS runs. Both fall to a real plugin and a deployment, not to
more transcription.

## The next eight bodies, and the two torque rows that refused

Phase 7's item 1 in obtainable order (F89): one driver, ten vendors, already written — the work is
one substrate each. Nine are in `urdf/`, every one generated from the vendor's own description with
the commit and the command in its header, every one named by the URDF's own `<robot name>`:

| Body | Source | Envelope |
|---|---|---|
| UR5e | Universal_Robots_ROS2_Description | provisioned: tech sheet + "Max. joint torques" article |
| ABB IRB 120 | ros-industrial `abb` | provisioned: product manual handling data |
| FANUC LR Mate 200iD | vendor ROS 2 driver | provisioned: published axis ratings |
| KUKA KR 6 R900 | `kuka_rsi_driver` description | provisioned: published axis ratings |
| Techman TM7S | `tm_description` | provisioned: published joint torques |
| AUBO i5S | vendor description | provisioned velocity-only: i5S datasheet axis table; joint torques not tabulated for this generation (an older i5 brochure tabulates moments — another generation's figures, declined) |
| Kinova Gen3 | `kinova_description` | provisioned: published actuator ratings |
| Doosan M1013 | `dsr_description2` (one `<plugin>` line to mock; `doosan_m1013_live.urdf` unedited, below) | provisioned velocity-only: M-series catalog joint table; no joint-torque ratings exist to transcribe |
| Elite CS520H | vendor description | provisioned velocity-only: the vendor driver's own joint-limits file (no standalone CS520H datasheet exists); no joint-torque ratings exist to transcribe |

The tenth and eleventh vendors close the class without substrates. Fairino ships a real
`SystemInterface` (`FairinoHardwareInterface`, position-only, verified in source) — but
`frcobot_ros2` carries no licence at all, so nothing generated from it can be vendored here. JAKA
ships no hardware interface: `jaka_driver` is a service-level node over the TCP SDK, and its repo
is unlicensed too. Both are recorded in the plan's session entry; neither is built, and a JAKA
driver belongs to item 5's long tail (route D, hardware-gated) if anyone asks.

## The last four bodies: item 5's confirmed rows, built

Phase 7's item 5 swept the long tail and left four vendors confirmed R-reachable and unbuilt
"because nobody asked". Asked, the four substrates are the same afternoon's work as the nine
above — generated from the vendor's own description, plugin line to mock where the vendor names
hardware, provisioned descriptors beside them:

| Body | Source | Envelope |
|---|---|---|
| Kawasaki RS007L | `khi_description` (Apache-2.0; KRNX covers only `khi_hardware`'s proprietary files) | provisioned velocity-only: RS007L-B axis table; wrist allowable moment is a load rating, not joint torques |
| Denso VS-060 | `denso_robot_descriptions` (Apache-2.0) | provisioned velocity-only: VS-050/060 table, VS-060 column; allowable moment is wrist-only |
| Mitsubishi RV-4FR | `melfa_description` (Apache-2.0) | provisioned velocity-only: RV-FR manual §2.1, RV-4FR column; allowable moment is wrist-only |
| Stäubli TX2-60L | `staubli_robot_description` (Apache-2.0, community) | provisioned velocity-only: TX2-60 range datasheet, L column; torques unpublished |

Two notes from the generation. Every `<limit effort>` in the four files is declined (§63.4), and
three of the four make the rule look easy: Kawasaki's is 1000000.0, Denso's is 1, Stäubli's is
0.0 — placeholders, a unit value, and nothing. And an XML declaration must be the first line of
the file, so Skynet's provenance comment sits below it: the first version put the comment first
and all four bodies refused to open, which the suite caught before anything else did.

## The live body: a UR5e behind URSim, moving

`urdf/ur5e_live.urdf` is the same vendors' description, generated for a simulator that answers,
inside `ur/Dockerfile`'s image — Universal Robots' `ur_robot_driver` 3.9.0 over `ur_client_library`
2.15.2 and `ur_description` 3.5.1, the release set ROS Jazzy ships, built from pinned commits. The
generation pins the topology `make check-ursim-docker` builds: the check runs inside URSim's network
namespace, so the controller and the driver's reverse server are both 127.0.0.1:

```
xacro $SHARE/ur_robot_driver/urdf/ur.urdf.xacro name:=ur5e_live ur_type:=ur5e \
  robot_ip:=127.0.0.1 reverse_ip:=127.0.0.1 headless_mode:=true \
  script_filename:=$SHARE/ur_client_library/resources/external_control.urscript \
  output_recipe_filename:=$SHARE/ur_robot_driver/resources/rtde_output_recipe.txt \
  input_recipe_filename:=$SHARE/ur_robot_driver/resources/rtde_input_recipe.txt
```

(`SHARE=/opt/ur_ws/install/share`; the image writes the result to `/opt/ur_ws/ur5e_live.urdf`,
and the body below the provenance comment is that file byte for byte. `script_filename` is the
`external_control.urscript` the driver ships — the stock one, unmodified. The kinematics come from
the description's `default_kinematics.yaml`, whose checksum is the simulator default; a real arm
needs its own extracted with `ur_calibration`, and the driver says so loudly when the checksum does
not match.) Until 2026-09-25 this body was generated from a RoboStack install on one machine, with
`host.docker.internal` and paths into that machine's conda environment baked in — nothing else
could run it (DRY-COVERAGE-PLAN.md F133).

```bash
make check-ursim-docker   # URSim at the pin in ../ur_rtde/ursim.mk, the suite's live block, SCTS
```

It builds the image, starts URSim, and runs `check-live` inside the image: the suite's live block,
which wakes the arm over the dashboard (power on, brake release), primes the control script with
thirty true-hold rounds, ramps the elbow +0.08 rad in 0.001 rad steps at 50 Hz, and ramps it back —
the readback is the assertion, and the arm ends where the block found it — then the sim half of
SCTS against the same body, position alone (F97). `check-live` on its own refuses without URSim or
without UR's plugin; it used to skip and pass. The nightly substrate workflow runs the same target at
the pins and again at URSim's `latest` and UR's branch heads.

Four of the session's findings changed the bridge, and all four are live-UR lessons. F93: the
declaration's 2 kHz negotiates RTDE at a rate URSim answers with silence, so `update_rate` is a
configuration now and the live body runs at 500. F94: `open()` used to succeed on hardware that
never started, so it checks activation now and a dead body refuses. F95: the first read returned
the manager's zero init, so the bridge settles on open. F96: the bridge claimed interfaces but
never switched the controller, so it switches manager-faithfully before the first command. Two
more findings stayed documentation, because the code they describe is not ours. F97: the vendor
plugin crashes on a controller mode switch (an unguarded stop path in 3.8.0), so the live body
commands pure position from a single start and never switches. F98 is the commanding contract,
measured against the running script rather than read in any document: every write preceded by a
read, rounds at 50 Hz, setpoints seeded from the live pose in 0.001 rad steps. A stale hold trips
the script's speed check and pops the pendant; a write gap trips the 40 ms receive timeout the
file bakes in and the script stops.

It stays a simulator=true guest. URSim moves the trajectory math, not a motor — the block proves
the driver commands a live `ros2_control` stack correctly, and nothing about the arm that stack
would drive.

## The Doosan body: an M1013 behind DRCF, through Doosan's own plugin

`DRY-COVERAGE-PLAN.md` Phase 8, item 8.3, and the second arm in this directory whose vendor plugin
runs, after the UR5e behind URSim. `urdf/doosan_m1013_live.urdf` is Doosan's own description, generated inside
`doosan/Dockerfile`'s image with `mode:=virtual` and nothing edited — the `<plugin>` is
`dsr_hardware2/DRHWInterface` — and DRCF is Doosan's controller emulator, the code that runs in the
robot's control box minus the motors, in the container Doosan publishes for it. That is rung 1 of
the plan's F118 ladder: the maker's controller software, not a fake of it.

```bash
make check-drcf-docker   # builds the image, starts DRCF, runs the suite's DRCF block and SCTS
```

The block opens the body twice. The first session is F126's evidence; the second is the body as
integrated — `update_rate=10 modes=position` — holding at ten hertz, taking a millisecond-spaced
burst that moves nothing, moving the elbow 0.12 rad out and back at the period, and latching the
stop. SCTS then runs through `dlopen` against the same emulator: 31 passed, guest, by §35.3.

What DRCF taught, all four found by reading Doosan's source and three confirmed against the running
emulator:

* **F125 — the period is the plugin's contract.** The bridge handed every `read` and `write` a
  period of one millisecond. `mock_components` ignores it; `DRHWInterface` skips, and returns OK
  for, every write whose period is outside 0.3–1.5 times its configured one. The bridge now passes
  the time since the previous call of the same kind, as `controller_manager` does. Shown failing
  without the fix — the elbow never leaves home, 1 of 303 — and passing with it. The burst check
  keeps the shape visible on purpose: writes a millisecond apart all say OK and move nothing, and
  the first write after a pause longer than 1.5 periods is dropped the same way, so a loop driving
  this plugin runs at its rate or not at all.
* **F126 — `velocity` here is not a velocity mode.** Every joint exports a `velocity` command
  interface, and the plugin commands motion only when the *position* command changes: velocity is
  a feed-forward in real mode and ignored in virtual mode. Two seconds of 0.2 rad/s on the elbow at
  the period — every write OK, the elbow still. Nothing in the URDF says so, so the integrator does:
  `modes=` narrows the modes the interface names offer (`modes=position`), the descriptor then
  offers only those, and a command in any other mode is refused. It narrows and never adds; a name
  outside §24's three, or one no joint offers, is refused at open.
* **F127 — velocity reads a standing zero.** In virtual mode `read` fills position from
  `GetCurrentPose` and never writes velocity, so the declared interface reads 0.0 on a moving joint
  — §24's "joint reported as still". Nothing this side of the plugin can tell that zero from a
  measurement; the block prints the value it saw on every run rather than asserting Doosan's
  behaviour.
* **F128 — the controller version is read inside `assert()`.** A Release build compiles the call
  out, logs `DRCF version = ` empty, and on a real version-3 controller would dial the real-time
  stream on `host` rather than `rt_host`. Unreachable in virtual mode; recorded for Doosan.

`urdf/doosan_m1013.provisioned.json` offers position alone for F126's reason. It stays a
`simulator: true` guest: DRCF moves the trajectory, not a motor, and the ten-hertz rate is the
emulator's recommendation rather than anything a real M1013 is held to.

### Every arm DRCF emulates and Doosan describes (Phase 10, item 10.6)

```bash
make check-drcf-models   # the block above, once per arm, each against an emulator started as that arm
```

Nine arms: the M0609, M0617, M1013 and M1509, the A0509 and A0912, the E0509, and the H2017 and
H2515. The block is the same moves on each, because all nine are six-axis arms with the same joint
vocabulary; what changes is the description the plugin is opened from and the emulator's own model.
A URDF alone would pass against any arm, so the target waits for DRCF's own startup line — `Robot
Model Number: H2017` — before it runs, and fails naming what the emulator did say if that line
never comes. The URDFs and provisioned descriptors of all but the M1013 (an older, hand-made set)
are what `doosan/generate.py` writes from the vendor's own `xacro` inside the image, and
`--check` says a committed file is not what the recipe writes.

DRCF 3.0.1 carries eleven more models — the A0307, the `f` and `s` variants of the A0509 and A0912,
the `a` variants of the four M-series arms, and the M0617's -120 and -160 configurations — and
`dsr_description2` has no description for any of them, so there is nothing to open a body from and
they stay unrun. The five provisioned descriptors written by the recipe cite the description's own
joint limits and nothing else: no Doosan catalogue was read for them, so they carry no torque and no
claim beyond what the vendor's file says.

## The ABB body: an IRB 1200 over EGM, with no RobotStudio

`DRY-COVERAGE-PLAN.md` Phase 8, item 8.4, and section 6 of `docs/robotstudio-runbook.md`. The
client is PickNik's `abb_hardware_interface` over ros-industrial's `abb_libegm`, built unmodified
at pinned commits in `abb/Dockerfile`; the description is `abb_ros2`'s own IRB 1200 5/90 (the arm
its RobotStudio example uses), generated unedited as `urdf/abb_irb1200_egm.urdf` with
`configure_via_rws:=false` so the client describes the arm from its joints instead of asking a
controller's Robot Web Services. The other end of the wire is `abb/fake_egm.py`: an EGM controller
written here from ABB's own `egm.proto` (BSD-3, compiled in the image), sending `EgmRobot` at 4 ms
and moving six joints toward each `EgmSensor` reference at a bounded speed.

```bash
make check-egm-docker   # the image, the fake, the suite's EGM block, then SCTS; the fake's counts after each
```

The block reads the fake's pose (degrees on the wire, radians here), holds at EGM's rate, moves the
elbow 0.2 rad out and back, refuses an out-of-travel setpoint and keeps the session, and latches
the stop; SCTS then passes 31. The fake counts malformed, mistyped, short, non-finite and
out-of-range references and fails the run on any.

Two things the client taught. `abb_libegm` refuses a six-axis controller message without a Cartesian
pose in both its feedback and its plan, so the fake sends a constant one — it has no kinematics,
and says so; nothing on this side reads the pose. And **F131**: PickNik's description gives the
client ranges that disagree with the arm's own `<limit>`s on every joint (joint 6 ±2.23 rad against
±2π; joint 2 ±2.41 against −1.745 to 2.269), and the client throws on a command outside its own. A
setpoint inside the `<limit>` and outside the block failed the component for the rest of the
session — every later write refused, back in range or not. So the bridge now declares the
**intersection** of `<limit>` and the position command interface's `min`/`max` as each joint's
travel, and refuses a position setpoint outside the declared travel before anything reaches the
plugin, rather than forwarding or clamping it.

What it is not: a controller. `EGMActJoint`'s gains and filters, what the speed reference means
(the RAPID program's `PosCorrGain` decides, which is why the block runs `modes=position`), the comm
timeout that stops EGM, the motors' own limits — none of that is in a fake written here. The
runbook's virtual controller is the rung above, and it stays an owner action.

## The KUKA body: a KR 6 R900 over RSI, through KUKA's own plugin

`DRY-COVERAGE-PLAN.md` Phase 8, item 8.14. Both ends of the wire are KUKA's: the client is
`kuka_rsi_driver/KukaRSIHardwareInterface` over KUKA's external-control SDK, and the other end is
`kuka_rsi_simulator`, from the same repository. `kuka/Dockerfile` builds them at the release set ROS
Jazzy ships (`kuka_drivers` 2.0.0, the SDK 2.0.0, the descriptions 2.0.2), and
`urdf/kuka_kr6r900_rsi.urdf` is KUKA's own `kr6_r900_sixx` description, generated unedited with every
argument at its default — RSI-only, listening on 59152.

```bash
make check-rsi-docker   # the image, the simulator, the suite's RSI block, then SCTS; what the simulator saw after each
```

The block reads the simulator's pose (degrees on the wire, radians here, A1 to A6 in order), holds,
moves A3 0.2 rad out and back, refuses A2 past KUKA's 45 degrees and keeps the session, and latches
the stop; SCTS then passes 31. `kuka/run-rsi.sh` reads the simulator's own log after each and fails
the run unless every closed session reached the controller as RSI's stop flag and none was
abandoned for missed replies. The simulator runs in a loop, because an RSI-only session ends with
the RSI program on the controller and KUKA leaves the next one to an operator; the loop is that
operator, which is what lets SCTS open its second session.

What KUKA's client taught, three bridge fixes, each shown failing first on this block:

* **F135 — the plugins are read and written in turn.** RSI is request/reply: the controller sends
  a telegram every cycle, and the client must answer it before it may receive the next. KUKA's SDK
  enforces that absolutely — a second receive with a telegram unanswered fails at once and keeps
  failing, and a reply with nothing to answer makes the plugin throw — and `controller_manager`
  never trips it, because it calls read, then write, once each, every cycle. This bridge called them
  when its caller did: the first read read twice, a refused command wrote nothing, and two commands
  wrote twice. Against KUKA's simulator the session wedged on its second read, and because KUKA's
  `read()` returns OK after a failed receive, every state from then on was the first telegram's,
  frozen, under a new stamp — the arm "held" perfectly and A3 never moved. So now a read still
  waiting for its answer is answered, with the last setpoint accepted, before the next read; a
  command with no read waiting stays in the command interfaces and goes with the next turn's write;
  and the first read no longer sleeps when it already holds a sample, since a controller-paced
  plugin queues telegrams behind a sleep.
* **F136 — `close()` deactivates before it shuts down.** `shutdown_components()` finalizes an
  active component straight from ACTIVE, and `on_deactivate` never runs. KUKA's plugin sends RSI's
  stop flag in `on_deactivate`, so the controller of every closed session was never told, and found
  out by timing out — on a KRC, a halted RSI program and an error for the operator. The simulator's
  own log is the measurement: no stop flag for the one session the block closed before the fix, one
  after, and two for SCTS's two.
* **F137 — a plugin that never initialized is refused.** When a plugin fails to load or fails its
  `on_init`, Jazzy's resource manager clears every component it had and its constructor carries on,
  so the manager reports no component at all — and the check that every component is active found
  nothing to refuse. KUKA's plugin at `master`, handed the committed URDF, FATALed in `on_init` and
  `open()` still returned OK. `open()` now checks the components the URDF declares, not the ones the
  manager reports; `urdf/no_such_plugin.urdf` and `urdf/init_fails.urdf` pin it in the default suite.

The nightly run at KUKA's `master` opens a different URDF from the pinned one. KUKA moves its plugin
and its descriptions together: `master`'s plugin requires `position`, `velocity` and `effort` on every
joint, and the 2.0.2 description declares `position` alone (F138). So that run builds the URDF from
KUKA's descriptions at the same refs inside the image and opens that one:

```bash
make check-rsi-docker RSI_IMAGE=skynet-kuka-rsi:maker-latest RSI_IMAGE_URDF=/opt/kuka_ws/kr6_r900_sixx_rsi.urdf DOCKER_BUILD_ARGS="--build-arg KUKA_DRIVERS_REF=master --build-arg KUKA_SDK_REF=master --build-arg KUKA_DESCRIPTIONS_REF=master"
```

What it is not: a controller. The simulator sends every 40 ms, a tenth of RSI's rate; a telegram
that goes unanswered is sent again with the same IPOC, where a KRC's IPOC advances every cycle
whatever the client does; it has no limits, no filters, no `Timeout` setting and no drives. So what
a KRC does with a late reply, with the first read's settle on a real 4 ms stream, or at the edge of
its correction limits, is unmeasured here. OfficeLite — KUKA's virtual controller — is the rung
above, and it is paid (8.8).

## The AUBO body: an i5 against ARCS, AUBO's controller software

`DRY-COVERAGE-PLAN.md` Phase 8, item 8.15. The other end of the wire is **ARCS** — the software that
runs in an AUBO control box — with AUBO's own simulated arm where the drives would be. AUBO documents
two ways to run it without a robot, a VMware image and its installer on Ubuntu; `aubo/arcs/Dockerfile`
is the second, in a container: AUBO's installer (`arcs_update-0.32.1-rc.54`), fetched from AUBO's
download server and checked against the md5 AUBO publishes, run in its documented `install` mode,
with AUBO's own `aubo_control_fake.conf` as the controller's configuration. No init runs in a
container, so the installer's `systemctl` and `udevadm` calls are stubs, and the controller is
started as its service unit starts it — with `--ulimit rtprio=99`, because ARCS puts its servo thread
on a real-time policy at start and, refused one, prints a line and waits for ever with only its
console port open (F139). `aubo/Dockerfile` builds the client — `AuboHardwareInterface`
from `AuboRobot/aubo_ros2_driver`, with the SDK its CMakeLists fetches — and `urdf/aubo_i5_arcs.urdf`
is AUBO's own xacro output for the i5.

```bash
make check-arcs-docker  # both images, ARCS, then: the unpowered arm, the operator, the suite's block, SCTS
```

`aubo/run-arcs.sh` runs four phases against one fresh controller and asks ARCS about itself after
each, through `aubo/arcs_state.cpp` — the one program in this row written here, and it only reads:

1. **The arm as ARCS starts it, powered off** (`check-arcs-unready`). The plugin activates anyway —
   it turns servo mode on and ignores the answer (F141) — and its first write fails. The session
   refuses that command with `SKR_EIO` and reports the fault on every read after it (F142, below);
   ARCS says afterwards that nothing reached the arm.
2. **The operator.** AUBO's own SDK example, `example_startup.cpp`, compiled unmodified, powers the
   arm on and releases its brakes — the pendant's job, which the plugin never does.
3. **The suite's ARCS block** (`check-arcs-suite`). AUBO's `velocity` interface first: a second of
   0.2 rad/s at 200 Hz, every write OK, the elbow still — the plugin sends positions to `servoJoint`
   and reads nothing else, F126's shape, so the body is integrated `modes=position`. Then the body
   as integrated: ARCS's own home pose read joint for joint, a hold, the elbow 0.2 rad out and back
   on ARCS's readback, the velocity read while it moved (about 0.21 rad/s — ARCS streams it, where
   Doosan's virtual mode reads a standing zero, F127), a setpoint past the declared travel refused
   with the session alive, and the stop.
4. **SCTS**, 31 passed. ARCS says servo mode is off after every closed session: the plugin's
   destructor turns it off, and `close()` gets there.

What AUBO's code taught:

* **F142 — a component that fails stays failed, and now says so.** Jazzy's resource manager takes a
  component whose `read` or `write` returned ERROR out of ACTIVE, and then reports every later read
  and write OK, skipping it. The bridge passed that on: `SKR_OK`, no fault, every position NaN, for
  the rest of the session — and a plugin that deactivated itself from `write` had every later
  command accepted and sent nowhere. After the first failure the bridge now asks the manager which
  declared components are still active, and while one is not, every read is `SKR_EIO` with the
  fault flag and every command is refused. `urdf/write_fails.urdf` pins it in the default suite
  with `ros2_control`'s own `test_system`, failing a read and deactivating itself on command.
* **F140 — AUBO's `<ros2_control>` block is PickNik's ABB one.** Three joints carry ABB IRB 1200's
  command ranges (±2.41, ±2.66, ±2.23 rad), three carry `{-2*pi}`, which xacro never expands, and the
  robot is named `aubo_robot` whatever the arm. AUBO's plugin reads none of it. F131's rule declares
  the intersection with the i5's `<limit>`s, so the descriptor's travel is ABB's on three joints —
  narrower than the arm and than ARCS's own limits, never wider — and the block checks a forearm
  setpoint at 2.5 rad refused although the arm would take it.
* **F143 — the arm ARCS simulates is the i5, not the i5S.** ARCS 0.32.1 ships no i5S model: configured
  as one, it logs that the type does not exist and dies. So this body is `aubo_i5`, and the i5S
  description stays on `mock_components`.
* **F144 — AUBO's plugin is written for Humble.** One line does not build on Jazzy: an include of
  `hardware_interface/visibility_control.h`, which Jazzy no longer ships and the plugin uses nothing
  from. `aubo/aubo-jazzy.patch` deletes that line and nothing else. The image builds the way AUBO's
  README does, with no build type, because that is the configuration whose SDK library AUBO's install
  rule copies.

What it is not: a robot. ARCS's simulated arm is kinematic — it tracks `servoJoint` and has no load,
no friction and no drive to fault — and ARCS runs here with the real-time policy it asks for but on
no real-time kernel, so timing, the servo loop at 5 ms under load, and every safety function that
reads a current are unmeasured. The i5's travel as the descriptor states it is ABB's on three joints
until AUBO's block is fixed (F140).

## The stop, and what it is not

`emergency_stop()` latches a flag, and every subsequent `write_command` is refused with
`SKR_EPERM`. Deactivating the hardware components is what a `ros2_control` integration would do, and
it allocates, takes locks and calls into a plugin — none of which is safe from the signal handler
and the dying process this entry point must survive. Between the stop and `close()` the plugins
are still read and written in turn (F135), and what each write carries is the last setpoint
accepted before the stop: a hold, which is what a plugin gets from `controller_manager` once every
controller has stopped. `close()` then deactivates them (F136), which is where a plugin such as
KUKA's tells its controller the session is over.

**That is not a rated stop and this driver does not report it as one.** A rated stop cuts power
through a circuit; the plugin behind this may or may not have one, `ros2_control` has no way to say,
so the descriptor declares no `estop` rather than `true`, and §15's `safety.estop.independent` stays
a bench test nobody passes from here.

## How it was measured, and what that found

The suite is held to a **defect-seeding campaign** (`tools/seed-defects/seeds/ros2_control.json`, `DRY-COVERAGE-PLAN.md`
item 12.C.3): 87 defects of the kinds that have really happened to a driver in this repository — a sign, a neighbour's
index, a zero where NaN belongs, a latch that outlives its session, a stop that releases nothing, a range taken wider than
the arm's own — each put into a copy of the bridge, built and run in the image `mock/Dockerfile` makes, one at a time.
**The first campaign noticed 36 of 72 (50%).** The suite, green until then, did not notice a first read that
never waits for the stream, a stamp that is never taken, a stop that cannot be released, a setpoint 0.05 rad past the travel,
a `modes=` with nothing after it, or `update_rate=0`. Each was a check nobody had written, and is one now; the second
campaign noticed 66 of 71 and the third **71 of 71**, with two equivalent mutants left out (the reason is on each seed) and
fourteen controls that survive as they must (run again on 2026-10-13, after the seeds were given no network, F286: the same, 71 of 71). The controls are the defects this rung cannot see because `mock_components`
cannot show them — the period a plugin is handed (F125), the mode switch a per-mode controller needs (F96), the request/reply
turn a socket keeps (F135 as KUKA's client has it), the deactivation a plugin acts on (F136) — which is what the rung-1
blocks above are for.

It found two defects in the driver itself, each shown red against the bridge as committed and fixed:

* **F281 — a command refused for a non-finite setpoint had already stored the joints before it.** The bridge validated modes
  and ranges before it claimed any interface and checked `isfinite` inside the claiming loop, so a NaN on the second joint was
  found after the first joint's setpoint was in its command interface — which is what the plugin is handed on the next write
  whatever the caller meant. Every applied setpoint is checked with the other refusals now, before anything is claimed.
* **F283 — a joint's type and limits were found by searching for the literal text of one spelling.** `<joint name="…"`,
  `type="` and `lower="` are one way to write a URDF and XML allows others: `type` first, a `<limit>` with `upper` ahead of
  `lower`, attributes on separate lines with spaces around the `=`, single quotes. A body written any of those ways opened and
  declared **no limits at all** — a revolute joint with no travel. `urdf/attribute_order.urdf` has one joint of each kind, and
  one `tag_attribute` reads them by name.

## A controller that stops answering, and plugins that do not say so (F332, F333)

`tools/faultproxy/chaos.py` puts the kernel's queue on a link; it cannot be used here, because URSim's and DRCF's own processes talk to each other over the loopback the plugin shares with them,
so a delay or a loss on it is a fault inside the controller and not on the line to it. The fault a `ros2_control` body most needs to survive is simpler and more usual: **the controller stops
answering and keeps its connections open** — it hung, it was paused for a debugger, a switch lost its power, a cable was pulled (a TCP peer that vanishes can stay silent for minutes before anything
resets, and a hung controller behind a live cable never resets at all). Docker's `pause` is that: the cgroup freezer stops every process of the controller at once, nothing is reset and nothing is sent.
`make -C drivers/ros2_control frozen-ursim-docker`, `frozen-drcf-docker` and `frozen-arcs-docker` open the UR5e behind URSim, an M1013 behind DRCF and the i5 behind ARCS through the makers' own plugins,
hold a pose for a second, freeze the controller for six seconds at 50 Hz, ask the contract's questions (`frozen.py`), thaw it and open the body again. KUKA's simulator shares the plugin's container,
so `frozen-rsi-docker` stops its processes with `SIGSTOP` instead, which the plugin cannot tell from a freeze. All four run nightly (`faults.yml`).

| | UR5e behind URSim | M1013 behind DRCF (virtual mode) | KR 6 R900 behind KUKA's RSI simulator | i5 behind ARCS |
|---|---|---|---|---|
| **bounded** — a read inside 2 s, a write inside 0.25 s | 297 rounds, the slowest read and write 0 ms | 298 rounds, the slowest read and write 0 ms | 8 rounds, **every read took 1.0 s** (the plugin's receive timeout) | 6 rounds, reads 0 ms, **every write took 1000 ms** |
| **outage reported** — a read says the controller is not answering | **never**: 297 reads, all OK, none flagged | **never**: 298 reads, all OK, none flagged | **never**: 8 reads, all OK, none flagged | **never**: 6 reads, all OK, none flagged |
| **stop** — `emergency_stop` inside 50 ms, and the latch holds | 0.0 ms; the command after it is refused | 0.0 ms; the command after it is refused | 0.0 ms; the command after it is refused | 0.0 ms; the command after it is refused |
| **recovers** — closed and opened again, it reads and is commanded | yes | yes | yes | yes |

**None of the four plugins says its controller has gone (F332).** UR's `read()` ends `// TODO(anyone): could not read from the driver --> return ERROR` and then returns OK, logging an error only
when `non_blocking_read` is off, which the live URDF turns on. Doosan's returns ERROR for a null pointer from the library and never for a link that is open and silent. KUKA's waits a second for a
telegram and then returns OK with the last one (F135 found that it does in the source; `frozen-rsi-docker` measured it). AUBO's copies what a thread last received from ARCS and returns OK. So for six
seconds the bridge reported the pose the controller held when it stopped, as a good sample, under a fresh stamp and a sequence that counted the reads — the flaw of F277, F278 and F328 in a fourth
body, and this one cannot be mended by looking at the values, because `ros2_control`'s `read()` has no way to say whether a sample arrived:

* **A value that does not change proves nothing.** A healthy arm holding a pose read **one** distinct pose in the second before the freeze (50 Hz, 49 and 50 reads), the same as the frozen
  one. A heuristic that calls "the same bits for N reads" an outage would fault every arm that stands still.
* **The state interfaces the plugin exports are cached too.** UR's `robot_mode`, `safety_mode` and `speed_scaling` are read from the same data package.
* **A call that waits is the one signal there is, and it catches two of the four.** KUKA's read waits its second and AUBO's write waits its second (below); UR's and Doosan's answer in under a
  millisecond from a cache. A bridge that flagged a plugin call longer than a few update periods as the controller not answering would catch KUKA and AUBO and miss UR and Doosan, and KUKA's
  healthy read already takes a whole simulator cycle (40 ms against a 4 ms period), so the threshold is a judgement per plugin. That is a design decision, not a patch; it is not made here.
* **The stop is not affected.** `emergency_stop` latches without calling the plugin and returns in 0.0 ms with the controller frozen; the command after it is refused. It is a latch in the
  bridge, not a stop the controller has been told about, and a controller that is not answering cannot be told: that is what a rated stop on its own circuit exists for (§15), which this
  body does not claim (above).

**AUBO's write waits for a controller that is not answering, and says nothing (F333).** `write()` sends the servo command by an RPC whose request timeout the plugin sets to 1000 ms
(`rpc_client_->setRequestTimeout(1000)`), inside `try { Servoj(...); } catch (const std::exception &e) { }`: the failure is swallowed and the call returns OK. With ARCS frozen every one of 6 writes
took 1000 ms, four times the 0.25 s the contract allows, and a caller that reads and writes in turn gets one command a second. KUKA's read does the same by waiting for a
telegram (above); UR's and Doosan's do not wait at all.

The four make targets therefore ask *outage reported* the other way round (`FROZEN_OUTAGE_REPORTED=no`), and AUBO's asks *bounded* for the write the other way round too
(`FROZEN_WRITE_BOUNDED=no`): each check **passes while the plugin behaves so and fails the day it does not**, so a gap cannot close without someone being told and the note being removed.
`frozen_selftest.py` (`make -C drivers/ros2_control frozen-selftest`, in `ci.yml`: no ROS, no container, ten seconds) asks `frozen.py`'s questions of a stand-in that is silent, says the controller has
gone, says it once and goes quiet, waits in its writes or does not, so that each of the eight cases can be seen to fail (against the script as it was before each gap was recorded, the cases that need
it fail). Doosan's is measured in `virtual` mode, which is what DRCF takes; its `real` mode reads `read_data_rt()` instead, and nothing here can show what that returns when the controller's real-time
stream stops. What would close F332 is the makers' to do (a read that returns ERROR when no packet has arrived in a few periods — UR's own TODO says as much) or an integrator's (a liveness
probe on a second connection, which makes the bridge know each vendor's protocol); neither is done here, and a report to a maker is a local draft, not filed anywhere.

## What the substrate cannot show

`mock_components/GenericSystem` copies commands to states. It has no dynamics, no latency, no
communication failure and no drive that can fault; `ros2_control`'s `test_system` fails only when it
is told to. So this suite shows that the driver drives
`ros2_control` correctly and it shows nothing whatever about timing, about a plugin that blocks in
`read()`, or about what happens when a real fieldbus underneath it drops a cycle. Those are the rows
the plan's table leaves under *not dry-verifiable* for this driver, and they stay there.
