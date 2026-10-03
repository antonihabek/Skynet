# `drivers/ros_industrial` — a robot arm behind a ROS-Industrial `simple_message` server

`DRY-COVERAGE-PLAN.md` item 12.E.6. ROS-Industrial's `simple_message` is the protocol the controller-side servers of a family of industrial robots speak: MotoROS on Yaskawa Motoman, the RAPID
server of `abb_driver`, `fanuc_driver`, Stäubli's and others each implement it on the controller, and `industrial_robot_client` (the ROS side) speaks it back. ROS-Industrial publish the protocol as
source (`industrial_core`, BSD-3-Clause) and it builds without ROS, so this driver's wire was read out of the maker's own files, every byte it makes is held to the maker's library, and its live
block runs against a server **built from that library**: F118's rung 3, an independent implementation of the protocol written by the people who wrote it. It is not any robot maker's controller.

```bash
make selftest          # 146 checks: the wire and every refusal, against a simulated controller
make check             # and the sim half of SCTS through the real ABI (34 passed)
make chaos             # the link-chaos contract: the driver's two sockets through a relay that delays, stalls, blackholes and resets them (38)
make check-layout INDUSTRIAL_CORE=/path/to/industrial_core    # constants and bytes against the maker's own library, built here: 118 checks
make check-sim-docker  # all of it against a server built from the maker's library, in each of its four builds
```

## What is here

| file | what it is |
|---|---|
| `skynet_ros_industrial/frames.py` | the wire: the envelope, the message types, the five message bodies, the two switches of the maker's build — every constant with its file beside it |
| `skynet_ros_industrial/bridge.py` | the driver |
| `skynet_ros_industrial/controller.py` | a simulated controller with a `simple_message` server on it, written here: the dry substrate, with every behaviour that is its own choice marked |
| `selftest.py` | the driver's own suite |
| `layout_check.py` | this tree's bytes against the maker's library, **built from source and run**: both directions, in four builds |
| `sim/codec_dump.cpp`, `sim/server.cpp`, `sim/build-reference.sh` | the maker's codec asked to encode and decode, and a controller's server built from the maker's sockets, framing and message manager with an arm behind it |
| `sim/Dockerfile`, `sim/run-sim.sh`, `live.py` | the substrate image (the maker's repository at a pinned commit, built in the image) and the live block |
| `chaos.py` | the subject for `tools/faultproxy/chaos.py` |

## The body, and what the wire does not say

The protocol carries a joint trajectory point (ten positions, a velocity *ratio*, a duration) to the controller, and a joint feedback and a status back. It carries **no joint limits, no joint count, no
speed ceiling, no model name, no joint names or roles and no word on whether the controller is a simulation**. A driver for it therefore cannot read the body off the controller the way
`drivers/xarm` does, and does not guess it: everything a descriptor needs that the wire does not say is the configuration's, and an absent one is a refusal.

```json
{"host": "192.168.1.31", "simulator": false, "robot": "motoman-gp7", "rate_hz": 10,
 "joints": [{"name": "joint_s", "role": "right_arm.shoulder.yaw", "min_rad": -2.97, "max_rad": 2.97, "max_speed_rad_s": 3.7}, ...]}
```

* **`joints`** — each joint's `name`, `role`, `min_rad`, `max_rad` and `max_speed_rad_s`. The limits are the arm's own, from its maker's datasheet or description package, and the operator asserts
  them (§5.2: a driver that made one up would bound the arm with a number nobody measured). Up to ten, the maker's own `JointData::MAX_NUM_JOINTS`.
* **`simulator`** — no default (the PX4 driver's rule): a bench run described as a simulation costs a tier it could have had, and a simulation described as a bench run is a false claim.
* **`byte_order`**, **`real_bytes`** — how the controller's build spells an int and a real: the maker's `BYTE_SWAPPING` and `FLOAT64` switches, for a controller whose CPU is big-endian and one whose
  `shared_real` is a double. The default is the maker's own (little-endian, 4-byte reals); `INT64` is not supported. Nothing on the wire says which a controller is, and a wrong guess is reported by the
  length prefix it reads (a big-endian controller read as little-endian announces a message of sixteen megabytes), not silently.
* `ports` (`motion` 11000 and `state` 11002, the maker's `StandardSocketPorts`), `rate_hz`, `point_duration_s`, `ping`, `ping_timeout_ms`, `stale_ms`, `open_timeout_ms`, `robot`. A key the driver does not read is refused,
  naming the keys it does.

It is a Python driver (`drivers/python-host` runs it), its tier ceiling is Guest (§31.3), it declares one control mode, **position**, and its base is `fixed`.

## What it refuses to bring up, and what a command does

**It refuses** a controller that says it is e-stopped, holds an error (the flag *or* the code), has its drives off, says motion is not possible, or is moving under somebody else's command. `STATUS` is a tri-state
per field (true, false, unknown) and an unknown field is read as neither: it is said aloud and the rest is held to. An error is never cleared here, since it is the evidence of why the arm stopped. It also
refuses a controller with no state (no feedback and status within the open timeout) and a motion port that does not answer a ping.

**A command is one `JOINT_TRAJ_PT`**, a service request: the target positions, a velocity ratio and a duration. A trajectory starts at sequence 0 and the next point is the previous plus one; a stop or a
failed point starts a new one. The driver works the speed out as the maker's own client does (`JointTrajectoryInterface::calc_velocity`): the largest `|step| / (duration × max_speed)` over the joints, and
**a step no joint can make in the time it was given is refused before it is sent**, never clipped, because the controller would slow the move and the loop above believes the arm is where it asked. A setpoint
outside a joint's range is refused too: a controller may acknowledge a target and fault when it gets there (the xArm's firmware does, F175). Replies are read on a thread of their own and counted; a point
answered with FAILURE is told to the next command, which refuses and starts a new trajectory, and a controller that stops answering is not queued behind past eight points.

**The stop** latches and sends `STOP_TRAJECTORY` (sequence −4, the maker's own number for "stop the current motion as soon as possible") under the motion socket's send lock with a bound of 50 ms, never waiting for a
reply, and drops the connection if that lock is stuck. It carries the arm's own positions where the maker's client sends zeros, so that a server that read it as a point would send the arm where it is. It is a **software
stop**: the controller's emergency-stop input is a circuit, `safety.estop.independent` is a bench test nobody passes from here, and what a controller does with a stop trajectory is its vendor's.

## What is measured, and how far

| rung | what | result |
|---|---|---|
| 4 | `selftest.py`: the driver against `SimulatedController`, a controller with a `simple_message` server written here | 146 checks |
| 4 | `skynet driver check` through python-host against it | 34 passed |
| 4 | defect seeding: `tools/seed-defects/seeds/ros_industrial.json`, 70 seeded defects and one control: **69 of 69 noticed**, one equivalent mutant left out (it was 64 of 70 when first run; the six that survived are listed below) | `docs/dry-test-matrix.md` |
| 4 | the link-chaos contract (`make chaos`): the two sockets through delay, jitter, a slow link, a stall, a blackhole and a reset | 38 checks |
| 3 | `layout_check.py`: this tree's constants against the maker's headers, and its bytes against the maker's **own library built from source**, both directions, in the default build and in `BYTE_SWAPPING`, `FLOAT64` and both | 118 checks |
| 3 | `live.py` against a server built from the maker's library (its sockets, framing, codecs, message manager, ping handler and reconnect), in each of the four builds, with the maker's own decoding of what the driver sent as the second opinion | **35 of 35 on each of the four**, and the sim half of SCTS through the real ABI **34 of 34 on each** (`make check-sim-docker`) |

The substrate is **rung 3**, not rung 1: ROS-Industrial wrote the protocol and its library, and no robot maker's controller is on the other end. A rung-1 row would be MotoROS on a Yaskawa controller or
`abb_driver`'s RAPID on an ABB one, which are the robots' own software and are not downloadable without the vendors' toolchains.

## What running it found

* **F241 — the maker's own server accepts the next client late, and the driver took that for a server that does not answer.** `MessageManager::spin` sleeps five seconds when its connection is down before it accepts the next
  client, so a session opened within five seconds of the last one's end is accepted late and pinged late. The first version waited the three seconds the connection gets and refused a controller that was merely waiting
  (SCTS's second session failed on it). The ping now has a wait of its own, `ping_timeout_ms`, default eight seconds, and the refusal names it. Found by the live block.
* **F242 — a transcription defect, caught before anything used it.** The older `JOINT` message was transcribed as the ten positions alone; the maker's `JointMessage` carries a sequence number first, which the cross-check found
  by reading the maker's own bytes. A defect of this kind is the reason `layout_check.py` builds and runs the maker's library and does not stop at reading its headers.
* **F243 — two traps in the maker's library as a server on Linux, recorded because any controller built on it meets them.** A process that writes to a client that has gone dies of SIGPIPE, because the maker's socket sends
  with plain `send()` (the live block's server exited with status 141 between two sessions until it ignored the signal); and `SimpleMessage::init` with an empty body leaves the body a message already had, so a ping made on a
  message that carried a status carries the status. Neither is the driver's; both are in `sim/server.cpp` and `sim/codec_dump.cpp` where they were met, with the reason.

* **What the seeding campaign found, and is not a finding of the driver's:** its first run left six survivors, each a test that was missing: a controller that says motion is not possible while it holds no error, an arm somebody else was moving at open, a range that the speed check was refusing in its place (the suite's steps were too big, so the speed refused what the range was meant to), a controller that holds an error or a stop while it says motion is possible, and one equivalent mutant (the stop's own reset of the sequence, which a release makes true again). Each is a check now.

## What it cannot show

* **A robot.** Every number here is the protocol's, against a server that agrees with the maker's library. What a Yaskawa, an ABB or a Fanuc controller does with a trajectory point — its planner, its acceleration limits, its
  alarm table, its safety circuit — is not measured: `RobotStatus.error_code` is each vendor's own number and the protocol defines none, so the fault word carries it verbatim and `drivers/fault-vocabulary.json` says there is
  nothing to cover.
* **Any named robot.** Motoman, ABB, Fanuc, Stäubli and the rest are claimed from ROS-Industrial's and each maker's documentation: which controllers have a server, which ports it listens on, whether it answers a ping, what
  it does when a second client connects. The driver has no model table because the protocol has no model.
* **The arm behind the sockets.** `server.cpp` and `controller.py` move a straight line in joint space at the speed a velocity ratio allows, and fault on a point outside the range. They are the author's, and what they
  do on a sequence gap or a full queue (FAILURE) is this author's choice, not the maker's header, which says only that a server "should" answer, and not any controller's measured behaviour (what MotoROS and the others do is from their documentation, *web, not checked*).
* **Timing.** One point per period over TCP is a path for a Guest-tier driver at a few tens of hertz. A controller's own interpolation between points is not the safety kernel's, and no claim is made about it.
