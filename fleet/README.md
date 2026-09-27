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
| `conformance` | `skynet driver check` through `drivers/python-host` and `drivers/mujoco` | the sim half of §15, at guest, through the real ABI |
| `loop` | `skynetk --source` and `skynetp` across the real ring, then `skynet blackbox replay` | a policy asking past every rating is clamped by the kernel, and the recording reproduces every verdict |
| `travel` | the same, asking past the **narrow** end of every asymmetric joint | the kernel holds both ends of travel — which today it does not (F159) |
| `transport` | `skynet transport` of a skill trained on another robot of the class | the skill binds to this body (§6) |

An MJCF runs as written, with the menagerie's `scene.xml` when it includes the robot so a floating
body has a floor. A URDF has no actuators, so `body.py` gives each joint the same generated position
servo — critically damped at 8 Hz against the joint's own inertia, force-limited to the file's
`effort` — with gravity compensated, contacts off and the root fixed; each of those choices is on
every row it applies to. A `<motor>`-driven body holds with gains chosen the same way, which the
MuJoCo driver takes only as an operator's configuration (F162).

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
