# `drivers/reachy_mini` — a Pollen Robotics Reachy Mini over its daemon's REST interface

`DRY-COVERAGE-PLAN.md` item 12.E.12. Reachy Mini is Pollen Robotics' open-source desktop robot: a head on a Stewart platform (six degrees of freedom), a body that turns, and two antennas. Its software is
one program, `reachy-mini-daemon`, which fronts the Wireless robot (a Raspberry Pi inside), the Lite robot (a USB cable to your computer) and a MuJoCo simulation behind **one REST interface**. This driver
speaks that interface. The daemon is the maker's (`reachy_mini`, Apache-2.0), so this driver's routes and models were read out of the maker's own source, held to it by `layout_check.py`, and its live block
runs against **the maker's own daemon in the maker's own simulator**: F118's rung 2. It is not a robot.

```bash
make selftest          # 168 checks: the shapes and every refusal, against a simulated daemon written here
make check             # and the sim half of SCTS through the real ABI (34 passed)
make chaos             # the link-chaos contract: the daemon's one TCP port through a relay that delays, stalls and blackholes it (30 checks)
make check-layout REACHY_MINI=/path/to/reachy_mini    # routes, models and limits against the maker's source, and the maker's own pydantic models run (30 checks)
make check-sim-docker  # all of it against the maker's own daemon running its MuJoCo simulator, in a container
```

## What is here

| file | what it is |
|---|---|
| `skynet_reachy_mini/frames.py` | the daemon's routes, its pose and state models, the motor modes, the documented ranges, and the encoding of a target |
| `skynet_reachy_mini/bridge.py` | the driver |
| `skynet_reachy_mini/daemon_sim.py` | a simulated daemon written here: the dry substrate, with a robot that follows its target at the speed each joint has |
| `skynet_reachy_mini/__main__.py` | the entry `drivers/python-host` serves, with `SKYNET_REACHY_SIM=1` for `skynet driver check` with no robot and no container |
| `selftest.py` | the driver's own suite |
| `layout_check.py` | this driver's routes, models and limits against the maker's source, and the maker's own models run |
| `live.py`, `sim/Dockerfile`, `sim/run-sim.sh` | the substrate image (the maker's repository at a pinned commit, installed with its MuJoCo extra **and its Git LFS meshes**) and the live block |
| `chaos.py` | the subject for `tools/faultproxy/chaos.py` |

## The body, and what the daemon does not say

The daemon reports whether it is a simulation, its state and the error it holds, and where the head, the body and the antennas are. It documents **head pitch and roll ±40°, head yaw ±180°, body yaw ±160°,
and the head and body yaw within 65° of each other**, and it says **nothing about the head's translation, the antennas' range, or how fast anything may move**. So:

```json
{"host": "reachy-mini.local", "port": 8000, "stop_mode": "hold",
 "limits": {"head_x": [-0.01, 0.01], "head_y": [-0.01, 0.01], "head_z": [-0.01, 0.01], "antenna_right": [-1.0, 1.0], "antenna_left": [-1.0, 1.0]},
 "speeds": {"head_x": 0.05, ..., "antenna_left": 1.0}}
```

* **`limits`** — the head's x, y and z and both antennas' ranges are **required** (§5.2: a driver that made one up would bound the robot with a number nobody measured); a documented range may be narrowed and never widened.
* **`speeds`** — every joint's ceiling, **required**: a step the ceiling does not allow in one command period is refused, never clipped.
* **`stop_mode`** — **no default**. `hold` stops the stream and holds the head where it is, stiff; `limp` takes the motors' torque away, which on a head the motors hold up is a fall. What stopping means is a person's decision.
* `simulator` is the daemon's word; a configuration can only make it more cautious. `rate_hz`, `http_timeout_ms`, `stale_ms`, `open_timeout_ms`, `roles`. A key the driver does not read is refused, naming the keys it does.

It is a Python driver (`drivers/python-host` runs it), its tier ceiling is Guest (§31.3), it declares one control mode, **position**, and its base is `fixed`. Nine scalar joints: the head's x, y and z (prismatic)
and roll, pitch and yaw, the body's yaw and the two antennas (revolute), in that order, the antennas right then left as the maker's own comment has them.

## What it refuses to bring up, and what a command does

**It refuses** a daemon that is not running or holds an error, motors that are not enabled (`disabled`, or `gravity_compensation`, where somebody may have a hand on the head: this driver changes neither), a move that
somebody else started (the daemon ignores `set_target` while one runs), **and a robot that is already outside the ranges the session was given**. That last one was found by running against the maker's own daemon
(F246, below).

**A command is one `POST /api/move/set_target`** with the whole body. Every setpoint is checked before any request goes out: finite, inside the joint's range, inside the head-and-body yaw difference, and no
further from the last setpoint than the joint's speed allows in one period. A refusal is **whole** (nothing is sent) and **says which joint and why**, once per run of the same cause (F247).

**The stop** latches and returns, and a thread of its own tells the daemon: cancel every move that is running, then `hold` (a target at the present pose) or `limp` (`set_mode disabled`), asked again on a
lost request. It is a **software stop**: the robot's own safety is the motors' firmware and the person at the desk.

## What is measured, and how far

| rung | what | result |
|---|---|---|
| 4 | `selftest.py`: the driver against `SimulatedDaemon`, a daemon written here | 168 checks |
| 4 | `skynet driver check` through python-host against it | 34 passed |
| 4 | defect seeding: `tools/seed-defects/seeds/reachy_mini.json`, 73 seeded defects and one control: **73 of 73 noticed** (it was 61 of 71 when first run, and the ten that survived are listed below) | `docs/dry-test-matrix.md` |
| 4 | the link-chaos contract (`make chaos`): the daemon's port through delay, jitter, a slow link, a stall and a blackhole | 30 checks |
| 2 | `layout_check.py` in the image: this driver's routes, models and limits against the maker's source, and the maker's own pydantic models run | 30 checks |
| 2 | `live.py` against `reachy-mini-daemon --sim`: the driver's session, commands, refusals and stop, with the maker's models, physics and web server's access log as the second opinion | **39 of 39**, and the sim half of SCTS through the real ABI **34 of 34** (`make check-sim-docker`) |

The substrate is **rung 2**: the maker's own program and the maker's own simulator are on the other end. The robot's firmware, motors, heat and cables are not.

## What running it found

* **F245 — the maker's daemon cannot be asked what it was told to go to.** `GET /api/state/full?with_target_head_pose=true` looks as if it can. Before any command it answers **500** (`assert target_pose is not None`
  in `routers/state.py`), and after one it returns the state with **no target in it**: the route builds `target_head_pose` and the response model, `FullState`, has no such field, so the value is dropped. The WebSocket
  does the same. The live block used it as its second opinion for what the driver sent and had never once had data; it uses the maker's own access log instead (`REACHY_DAEMON_LOG`). Not the driver's, not filed (vendor
  reports stay local).
* **F246 — a robot found outside the ranges it was given was accepted at open, and then every command was refused and not a word said (the driver's).** The maker's daemon starts **asleep** (`--no-wake-up-on-start`, or
  after `goto_sleep`): the head 5 cm down and the antennas folded at ±3.05 rad. A session whose ranges are narrower than that can never command it, because a target must be in range and a step within the speed, both
  measured from where the robot is. It is refused at open now, naming the joints, where each is and the range it was given, and what to do (`POST /api/move/play/wake_up`, which the maker's own tools use, or give the
  range the robot truly has). The allowance is one step of the joint's own speed, so jitter at an edge is not a robot outside it.
* **F247 — the driver refused a command and said nothing.** Every refusal in `write_command` was a bare `EINVAL`, which against the maker's daemon was seen as 76 identical `-22`s. `CONTRIBUTING.md`: an error names
  its cause and the next action. Each refusal now names the joint, the value, the range or the speed, and is said once for a run of the same cause, because a stream at 20 Hz would otherwise say it twenty times a second.
* **F248 — the substrate's image installed 160-byte pointer files for the maker's meshes.** The maker's `.stl` files are Git LFS objects; a shallow `git fetch` of the pinned commit and a `pip install` packaged the
  pointers, and MuJoCo refused the first mesh ("number of faces should be between 1 and 200000 … perhaps this is an ASCII file?"), which the daemon reported as `state: error` after it was already serving. The image
  now fetches the objects (`git lfs pull`) and fails the build if a pointer is left. Found by the first run of `sim/run-sim.sh`.
* **The maker's simulator does not model torque.** `MujocoBackend.get_motor_control_mode` returns `Enabled` and `set_motor_control_mode` is `pass`, so `limp` cannot be seen as a state here: the live block holds it to the
  `POST /api/motors/set_mode/disabled` the maker's server logged, says so, and reads `enabled` through the stop on purpose, so that a simulator that learns to model torque is noticed.
* **What the seeding campaign found, and is not a finding of the driver's:** its first run, over 71 seeds (62 the previous session had written and never run, and nine written this session), left ten survivors, each a test that was missing or was
  passing for the wrong reason. A head roll widened to 90° (the check of the documented ranges read the pitch, the yaw and the body and was named for the roll as well); a pose that arrives as a matrix (still
  refused, by a `TypeError`'s words about list indices and not by the sentence that says a matrix is what `use_pose_matrix=true` asks for, which only the message tells apart); a configuration's `simulator: false`
  over a daemon that says it is a simulation, and mock-up mode described as hardware (two directions nobody had asked); motors that are limp not reported (the check passed on the flag the previous cause had left
  up, because the status is read every half second); silence read as stillness (the check passed on failed requests, and a daemon that answers every request, slowly, had never been tried); a stop told once (a delay
  cannot show a retry, because a slow request still arrives, where a lost one does not); the setpoint last issued not kept (every other test set it by hand); and the step after a release worked out from the last
  setpoint and not from where the head is. Each is a check now, and the simulator gained two knobs for them (`mockup_sim_enabled`, and `drop_posts`, which loses the next N requests). One of this session's own seeds, for the
  new say-once rule, survived the first run too, for the same reason as the rest: the test followed a different cause from the one it meant. Four seeds are noticed by a crash and not by a failed check (a joint count not
  checked, a target of eight values, speeds not required): the code that would run without the guard cannot, which is a noticing, and is named here so that it is not read as more.
* **A transport failure was said once per control period (the driver's).** The link-chaos run's blackhole printed "did not answer POST /api/move/set_target: timed out" eleven times in its window; at 20 Hz it
  would be twenty a second. Every failure of a command now goes through the same say-once as a refusal, and a different cause, or the same one after an accepted command, is said again (F247 covers both).

## What it cannot show

* **A robot, Wireless or Lite.** Motor firmware, torque, heat, cables, the head's real dynamics, the real antenna travel and what a real robot's backend does with `limp` are not reached. The maker's simulator is
  MuJoCo with the daemon's own kinematics.
* **Ranges the maker does not document.** The head's translation and the antennas have none, so the operator's are the driver's, and the live block's (±1 cm, ±1 rad) are the block's choice, not a measurement.
* **A fault vocabulary.** The daemon reports one error string and no code; the fault word says there is one. `drivers/fault-vocabulary.json` has nothing to bind.
* **Timing.** One REST request per period over loopback or a LAN is a path for a Guest-tier driver at a few tens of hertz; the daemon's own control loop and the motors' are the maker's, and no claim is made about them.
