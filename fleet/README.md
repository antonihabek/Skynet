# fleet — every public robot description, through the OS

`DRY-COVERAGE-PLAN.md` Phase 10. No robot will be bought (*Decisions* 18), and the owner asked for
every robot there is to be tested as far as technical testing reaches (*Decisions* 20). A driver
needs a robot's interface and most robots publish none; almost every robot publishes a
**description** — a URDF or an MJCF — and a description is enough to ask the OS every question that
does not need the robot's wire.

```bash
pip install -r fleet/requirements.txt
cargo build --release -p skynet-cli -p skynet-kernel-host -p skynet-policy-host
make -C drivers/python-host
python3 fleet/sweep.py                 # all of it, into reference/reports/fleet.json
node tools/check-fleet.mjs             # the report, the catalogue, the manifest and the prose agree
```

`fleet/catalogue.py` writes `catalogue.json` from `robots.py`; `sweep.py --only <id,id>` runs a
subset and `--keep <dir>` keeps each robot's working directory (its descriptor, its sealed window,
the kernel's own log). The first run clones every upstream repository at its pinned commit, about
12 GB.

## Where the robots come from

| Source | Pinned by | Rows |
|---|---|---|
| [`robot_descriptions`](https://github.com/robot-descriptions/robot_descriptions.py) 3.2.0 | its own per-repository commits, and each file's sha256 in `catalogue.json` | every description it lists |
| [DeepMind's menagerie](https://github.com/google-deepmind/mujoco_menagerie) | the same clone and commit | the models `robot_descriptions` does not list |
| `drivers/ros2_control/urdf/` | bytes | the vendor descriptions the bridge's bodies are built from |

`robots.py` says who makes each robot, what class it is, and which directory under `drivers/` would
speak to the real one — its **route**. A route is checked against `drivers/models.json`, and the
report never states a release stage: that lives in the manifest (*Decisions* 19), so nothing here
can promote a driver.

## What runs, per robot

| Stage | What runs | A pass means |
|---|---|---|
| `source` | the pinned file, fetched and hashed | the bytes are the catalogue's |
| `import` | `skynet descriptor from-urdf` (§63.4), with the class's base and, for a single arm, `--assume-side right` | Skynet's own importer reads the maker's file |
| `body` | MuJoCo, and `body.py` for a URDF | the description is a body physics can step |
| `roles` | an **accepted role table** for the arm's family (`roles.py`), tested against the joint axes MuJoCo computed for this body at its zero pose | the joints of a numbered arm carry the roles a LeRobot policy binds by, and each relation the table asserts held — or the table is refused, by name, and applied to nothing (item 10.8, F165) |
| `conformance` | `skynet driver check` through `drivers/python-host` and `drivers/mujoco` | the sim half of §15, at guest, through the real ABI |
| `loop` | `skynetk --source` and `skynetp` across the real ring, then `skynet blackbox replay` | a policy asking past every rating is clamped by the kernel, and the recording reproduces every verdict |
| `travel` | the same, asking past the **narrow** end of every asymmetric joint | the kernel holds both ends of travel (§92, F159): every command it judged was clamped, counted off `skynetk blackbox`'s `judged` rather than off what the policy posted, because §25's ring keeps only the newest command |
| `transport` | `skynet transport` of a skill trained on another robot of the class | the skill binds to this body (§6) |

An MJCF runs as written, with the menagerie's `scene.xml` when it includes the robot so a floating
body has a floor. A URDF has no actuators, so `body.py` gives each joint the same generated position
servo — critically damped at 8 Hz against the joint's own inertia, force-limited to the file's
`effort` — with gravity compensated, contacts off and the root fixed; each of those choices is on
every row it applies to. A `<motor>`-driven body holds with gains chosen the same way, which the
MuJoCo driver takes only as an operator's configuration (F162).

## The role tables (item 10.8)

A skill's channels bind to a body's joints by exact name or by canonical role (§6), and a role is a
human's assertion (§63.4). The importer names a joint from its words and its axis, and cannot name
`joint1` … `joint7` — which is how xArm, Kinova, FANUC, ABB, Franka and most arms are numbered — so
before this stage the SO-101 policy bound no arm but the SO-101. `roles.py` holds **one table per arm
family**, in the spelling LeRobot's own bridge uses (§67.2), written once instead of once per robot:
five patterns (`so5`, `four_no_roll`, `six_parallel_wrist`, `six_roll_mid`, `seven`) and one line per
description saying which pattern its joints follow, by the body's own joint names.

A table is applied only if it holds. Each pattern carries relations between its joints — the yaw's
axis is vertical, the elbow is parallel to the shoulder's pitch, the wrist's roll is perpendicular to
the joint before it — and each is tested against `data.xaxis`, MuJoCo's reading of the maker's file at
zero. A description whose body disagrees is `refused` in the report with the joint, the relation and
how many degrees off it was (the Jaco arms with a 60° wrist), and gets no roles from it.
`python3 fleet/test_roles.py` is those refusals and needs no MuJoCo.

What it is not: the maker's word, a driver's descriptor, or a claim that a policy does anything on the
arm it now binds to. `docs/issues/flex-and-pitch-are-one-joint.md` is the same gap at the layer where it
belongs.

## What a row is not

* **Not a driver test.** Every body here is `drivers/mujoco`, and the robot's own wire is untouched.
  The route column says which driver in this tree speaks it and `drivers/models.json` says how far
  that driver has been tested.
* **Not evidence about a machine.** Every body declares `simulator: true`, every award is guest
  (§35.3), and no row is bench evidence.
* **Not the vendor's controller.** A URDF body's servos are this directory's, not the robot's; a
  gain that holds a simulated joint is a number that worked in MuJoCo.
* **Not a balance test.** A floating body stands on its own servos with no balance controller, and
  a URDF humanoid is simulated on a fixed base.

What a pass does say: the OS accepts this robot's description, describes it through the real ABI,
bounds a policy that asks for too much by the robot's own ratings, and records and replays what it
did — which is the precondition for a driver being worth writing, and the part of "does it work on
this robot" that needs no robot.
