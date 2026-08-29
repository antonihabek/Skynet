# skynet-mujoco

The **MuJoCo simulator RHAL driver** — §70. A scene file becomes a Skynet body: a descriptor, a
clock, a command path and a camera, with no hardware anywhere.

```bash
pip install mujoco
make check                     # 102 sim-conformance checks against real physics
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

## Scenes

Five, in `scenes/`, and four exist to be refused in a particular way. All are written here, so
`make check` needs no downloads: a test suite that fetches an asset is a test suite that stops
working the day that URL moves.

| Scene | For |
|---|---|
| `so_arm_shaped.xml` | A six-joint arm with a gripper and a wrist camera, shaped like an SO-101. **Not a model of one** — the joint names match so the role path can be exercised; every dimension and gain was chosen to make a stable simulation. |
| `torque_arm.xml` | `<motor>` actuators, so the mask has no correct answer and must be refused |
| `free_base.xml` | A free-jointed root, so the driver refuses to open until `base` is declared |
| `two_joint_body.xml` | A pan-tilt head — one body, two joints — so `frames` is omitted |
| `unsupported_actuators.xml` | A tendon, an integrator and a ball joint, all named and excluded |

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
| macOS, Windows | Not exercised |
