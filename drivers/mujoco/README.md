# skynet-mujoco

The **MuJoCo simulator RHAL driver** — §70. A scene file becomes a Skynet body: a descriptor, a
clock, a command path and a camera, with no hardware anywhere.

```bash
pip install mujoco
make check                     # 119 sim-conformance checks against real physics
```

§70 is explicit that this is not a lesser path:

> MuJoCo and Isaac Sim, and they are not a lesser path — the sim half of SCTS cannot run without
> them, so they are **load-bearing infrastructure**.

## Running it

The driver is served to the OS by [`drivers/python-host`](../python-host), which is what dlopens
into `skynet driver check`, `skynetd` and anything else that speaks the RHAL:

```bash
cd ../python-host && make
SKYNET_PYTHON_DRIVER=skynet_mujoco \
SKYNET_MUJOCO_SCENE=$PWD/../mujoco/scenes/so_arm_shaped.xml \
PYTHONPATH=../../bindings/python:../mujoco \
  skynet driver check ./libskynet_python_host.so
```

or through the Makefile, which spells the same thing:

```bash
cd ../python-host && make check-mujoco
```

The scene normally arrives in the driver configuration the OS hands to `open()`:

```json
{"scene": "/opt/scenes/so_arm_shaped.xml",
 "rate_hz": 100,
 "base": "wheeled",
 "role_overrides": {"shoulder_pan": "right_arm.shoulder.yaw"},
 "render": {"width": 128, "height": 96},
 "keyframe": "home"}
```

`SKYNET_MUJOCO_SCENE` is the other way in, and exists because `skynet driver check` opens a driver
with an empty configuration. **There is no default scene.** A simulator driver with no scene has no
body, and inventing one would describe a robot nobody asked for and then report conformance results
about it.

## The clock, which is the whole design

§35.3 makes the simulator's clock *the kernel's* clock, so a simulation running at a fifth of wall
speed or ten times it is simply the time the whole system is measured in — watchdogs, rate limits
and deadlines included. Otherwise every simulated run trips the watchdog in a few cycles, and the
usual workaround (disable the watchdog in simulation) means the thing being tested is not the thing
that ships.

**`read_state` is what advances it.** Each call integrates one control period with whatever `ctrl`
currently holds and then reports the state that resulted. Not `write_command`, and the difference
matters:

* a **stalled policy still has time passing**, the last command keeps being applied, and the
  watchdog fires exactly as it would on hardware. Had the world advanced only on a command, a
  stalled policy would freeze the universe and `safety.watchdog.fires` would be untestable in
  simulation — which is half the reason for having a simulator driver;
* it is what a real robot does: `read_state` returns a stamp that has moved whether or not anybody
  commanded anything.

One consequence, stated so nobody has to derive it from a surprising number: a command written at
cycle *N* is integrated during cycle *N+1*, so **`command_latency_ms` is exactly one control
period**. It is the one descriptor field a simulator fills in with more confidence than any hardware
vendor, and it is filled in by construction rather than by measurement.

## What the descriptor says, and what it refuses to

| Field | Value | Why |
|---|---|---|
| `simulator` | **`true`** | §35.3, §70. Caps the certification award at guest, and stamps every check-in and evaluation result so no simulated run is presented as bench evidence. |
| `joints[].limits` | From the scene | In a simulator **the model is the body** — a joint range in the file is exactly what this body does, not a design value that might be wrong about some hardware. |
| `envelope` | **`null`. Always.** | A safety envelope bounds what a machine may do to the people near it. There is no machine and there are no people. |
| `joints[].role` | `null` unless configured | This generator does not suggest roles. See below. |
| `digest`, `verified` | `null`, `false` | Nobody has signed a scene file. |
| `end_effectors` | `[]` | MJCF has no notion of one; a gripper is a body and a joint like any other. |
| `frames` | The body tree (§44.3) | Every body a frame, dynamic ones naming their joint and axis — a complete FK specification, which is what the kernel needs since it computes its own and subscribes to nothing. |
| `sensors` | The scene's cameras, **if they can be rendered** | See "Cameras". |
| `base` | Operator-declared when the root is free | A free-jointed root says the machine moves and says nothing about how. |

### Roles: none, on purpose

A body from this driver arrives with **no canonical roles**, and `skynet driver check` reports
`embodiment.roles.coverage` at 0% for it. That is the designed outcome rather than an oversight.

Role suggestion already exists, once, in `skynet descriptor from-urdf`, with the human-accept step
§63.4 requires. A second implementation of "which joint is this, really" is a second answer to one
question, and the whole apparatus of the conformance corpus exists because two answers to one
question is the failure mode this project is arranged around.

The fix is one of:

* `role_overrides` in the driver configuration — an operator asserting the mapping, recorded in the
  descriptor's provenance as an assertion;
* `skynet descriptor from-urdf robot.urdf --accept-suggested-roles`, if the robot has a URDF, which
  most simulated ones do.

Without either, a policy channel can still bind by exact joint name. It cannot participate in
cross-body remapping, which is truthful: nobody has said what these joints are.

### Cameras: declared only if they can be rendered

MuJoCo renders through OpenGL, and a headless machine may have no context. So the driver **probes at
open** and declares the scene's cameras only if a frame actually came back; otherwise it declares no
sensors and `read_sensor` reports `SKR_ENOSYS`, with the reason on stderr.

The alternative — declaring the cameras and failing the reads — was tried by accident in the LeRobot
bridge and caught by `skynet driver check` with the sentence that is now the rule: *a declared stream
that cannot be read binds a policy's observation to nothing.*

```bash
MUJOCO_GL=osmesa make check      # or =egl, with the matching library installed
```

A failed probe is remembered for the life of the process and never retried, which was found the hard
way: MuJoCo's GL context is process-global, the first attempt with no context raises, and the
*second* aborts the interpreter from inside the native library — no exception to catch.

### Cameras: rendered off the control thread, and declared `concurrent_read`

Every declared camera carries `"concurrent_read": true` (§89.1): `read_sensor` may be called on
another thread while `read_state` and `write_command` run, and it does not make them wait. The
promise rests on three arrangements in `skynet_mujoco/render.py`, and the selftest holds each one:

* **`read_state` publishes a snapshot** — `qpos`, the mocap poses and the clock, copied after every
  step — by replacing one reference. It takes no lock and reads nothing back.
* **A render thread of the driver's own** owns the GL context from creation to release and has an
  `MjData` of its own. A frame is rendered from the newest snapshot through that data, so rendering
  never reads the arrays the physics is writing, and the frame is stamped with the state it shows.
* **A read that does not come back is bounded.** Five seconds, then `SKR_EIO` for the life of the
  body, because a read that never returns would stall the runtime's capture thread (§89.3).

`DRY-COVERAGE-PLAN.md` F24 is what this is for. Rendered on the control thread through a software
rasteriser, the wrist camera cost 18.2 ms of a 20 ms period and the loop missed it on every frame;
§89.2 now suspends such a stream after its third miss. Declared, the same render on the same machine
runs beside the loop: `make -C ../python-host check-loop-mujoco` reports **0 overruns** over 250
cycles with 51 of 51 frames read on the runtime's capture thread.

The honest limit is Python's. Two threads in one interpreter do not run bytecode at once, so the
declaration is true only because a render spends its time in MuJoCo's native code with the GIL
released. Measured on the machine above: a 14.4 ms render on the render thread raised the slowest
ten-substep step burst on the control thread from 0.49 ms to 2.1 ms. A binding that held the GIL
through a render would make the declaration false, and the loop's zero-overrun assertion is what
would say so.

## What the RHAL cannot carry, and what happens to it

Each of these is a real thing a modeller writes, and each is named on stderr and left out of
`joints[]` rather than dropped quietly — a body with fewer joints than the scene has is a surprise
best delivered at open time.

| In the scene | Why it is not an RHAL joint |
|---|---|
| An actuator on a tendon, a site or a body | There is no joint index to give it |
| An actuator with internal state (`intvelocity`, a muscle, a user gain) | `ctrl` is then a setpoint for an integrator, not radians, rad·s⁻¹ or Nm |
| A ball or free joint | Three or six degrees of freedom behind one name; the RHAL's arrays carry one scalar per joint |
| Two joints on one body | The body drives; the **frame graph** is omitted, because a frame node carries one joint and a tree with a hinge missing from it is worse than none (§44.5) |

## The mask holds, and holds *there*

§24: a zero in the mask is *"hold this joint at its current setpoint"*, and the kernel sends an
all-masked frame every cycle it has nothing to forward. A position actuator's `ctrl` is its
setpoint, so a masked joint is held at the target latched when the hold began — its last position
setpoint, or where it was when it never had one — and not re-read. Until 2026-09-24 this driver
wrote the measured position back into `ctrl` on every masked cycle, which zeroes the servo's
stiffness term: a thousand hold frames let `so_arm_shaped`'s loaded shoulder sag 0.65 rad, and three
hundred put `drivers/apollo`'s standing humanoid on the floor. Both are checks now, and both fail
against the old code (`DRY-COVERAGE-PLAN.md` F121, carried as 8.13). A velocity body holds by
commanding zero; a torque body holds with the operator's `hold_gains` and refuses a mask without them (below).

**A torque joint holds with the operator's gains.** §25 says a zero mask holds position on a
torque joint too, and a torque joint holds position the way an impedance drive does: a stiffness
about the target latched when the hold began. `hold_gains` in the configuration is that stiffness —
one `{"kp", "kd"}` for every torque joint or a map from joint name to one — and it is the
operator's, as `drivers/unitree`'s gains are, never a default. With none, a masked torque joint is
refused, as it always was. Every quadruped in DeepMind's menagerie is `<motor>`-driven, and before
this none of them could take the kernel's hold frame at all (`DRY-COVERAGE-PLAN.md` F162).

**A masked joint holds in its own mode, and a command can carry a mode per joint.** Only a joint a
command *applies* is asked to match the frame's mode; a held one holds the way its actuator holds.
The driver reads §86.1's per-joint array and declares `per_joint_modes`, which is what makes a
mobile manipulator — velocity wheels, position arm — commandable at all (F169).

**Every value crosses the actuator's gear.** A joint transmission acts on `gear · q`, so a position
or velocity setpoint is written as gear times itself, a torque as itself over the gear, and a force
read back or declared as a ceiling is gear times the actuator's. A `<motor>` bounded by `ctrlrange`
declares that bound as its rating. Until F167 every geared actuator was off by exactly its gear.

**A declared base must agree with the scene.** A moving `base` declared for a root the scene fixes
to the world is refused at open (F164).

The same change made the command all-or-nothing: every value is checked before any `ctrl` is
written, where the loop used to write joint by joint and refuse at the first non-finite value with
the joints before it already sent (F129).

## Scenes

Eight, in `scenes/`, and four exist to be refused in a particular way. All are written here, so
`make check` needs no downloads: a test suite that fetches an asset is a test suite that stops
working the day that URL moves.

| Scene | For |
|---|---|
| `so_arm_shaped.xml` | A six-joint arm with a gripper and a wrist camera, shaped like an SO-101. **Not a model of one** — the joint names match so the role path can be exercised; every dimension and gain was chosen to make a stable simulation. |
| `torque_arm.xml` | `<motor>` actuators, so the mask has no correct answer and must be refused |
| `free_base.xml` | A free-jointed root, so the driver refuses to open until `base` is declared |
| `two_joint_body.xml` | A pan-tilt head — one body, two joints — so `frames` is omitted |
| `unsupported_actuators.xml` | A tendon, an integrator and a ball joint, all named and excluded |
| `geared.xml`, `geared_motor.xml` | Servos and a motor behind gears, so every value crosses the gear in both directions (F167) |
| `mixed_modes.xml` | A velocity wheel and a position arm joint: one body, two modes (§86.1, F169) |

## What this is, and is not, evidence of

**Guest tier, twice over.** Python cannot hold a real-time path (§31.3), and a simulator is forced to
guest by §35.3. Neither cap is removable and the descriptor says both.

It *is* evidence that a policy, a package, a set of grants and a transport plan compose into
something that runs, against physics rather than against a stub. That is most of what a driver author
needs before there is hardware, and it is the difference §63.3 draws between turning up to a bench
session with the sim suite green and turning up to discover the shape of the problem.

It is **not** evidence about any machine. A simulated stop cuts nothing. `base_pose` is exact,
noiseless ground truth that no physical robot has, and a policy that quietly learns to depend on it
will find out on hardware. A simulated frame has no acquisition latency and no noise. The
descriptor's provenance block says each of these where anyone reading it will see it.

## Verified where

| | |
|---|---|
| Linux, MuJoCo 3.12, `MUJOCO_GL=osmesa` | 102 checks, and the whole chain through `drivers/python-host` into `skynet driver check`: 23 pass, 1 fail (roles, as designed), guest tier |
| Linux, no OpenGL | 95 checks, 1 skipped — the no-camera branch, which is what a container takes |
| WSL2 Ubuntu 26.04, MuJoCo 3.13, `MUJOCO_GL=egl` (llvmpipe) | 112 checks, 0 skipped; through `drivers/python-host`, 29 pass, 1 fail (roles, as designed), guest tier, and §89.4's 250 ms concurrent read passes. `check-loop-mujoco` closes with 0 overruns — with `POLICY=onnx` too |
| The same machine, no display and `MUJOCO_GL` unset | 102 checks, 1 skipped — the no-camera branch, with the held-render checks still run |
| The same machine, `MUJOCO_GL=osmesa` and no libOSMesa | `open` refuses with `ENODEV` and names the library: MuJoCo 3.13 loads the backend while it is imported, so this is not the probe's branch at all |
| Linux container, MuJoCo 3.14.0 from PyPI, `MUJOCO_GL=disable` (2026-09-24) | 105 checks, 1 skipped — the no-camera branch, with F121's two hold checks and F129's |
| Linux container, MuJoCo 3.14.0, no OpenGL (2026-09-26) | 119 checks, 1 skipped — with F162's torque hold, F164, F167's gear and F169's mixed modes, each shown failing against the driver before it; and every public robot description through the fleet sweep (`fleet/`, `reference/reports/fleet.json`) |
| macOS, Windows | Not exercised |
