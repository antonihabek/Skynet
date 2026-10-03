# skynet-ros2-base — a ROS 2 mobile base over its velocity-command and odometry topics

`DRY-COVERAGE-PLAN.md` item 12.E.3. One driver for the class of bases whose ROS 2 interface is the one every wheeled-base stack has settled on: a velocity command in
(`geometry_msgs/Twist`, or `TwistStamped` since Jazzy's `diff_drive_controller`) and `nav_msgs/Odometry` out. That is Clearpath's Husky and Jackal, TurtleBot 3 and 4, iRobot's Create 3,
AgileX's bases, Hello Robot's Stretch base, PAL's TIAGo base, and anything built on `ros2_control`'s `diff_drive_controller` or `mecanum_drive_controller`.
**It measures none of those as robots.** It was held against `ros2_control`'s own controller on mock hardware, and against two makers' own simulations (ROBOTIS's TurtleBot3 and iRobot's Create 3);
the rest is the claim that the other bases speak the same two topics, and that is a claim from the documentation of each, not a measurement.

It is a Python driver (`drivers/python-host` runs it) because it is `rclpy`, which is a Python library, and its tier ceiling is Guest (§31.3): a base reached over DDS from a Python
process was never going to be a real-time path.

```
make selftest          the configuration, the graph it will and will not open on, the state, the motions and the stop,
                       against a simulated base in the same process (`sim.py`). Needs python3 and nothing else
make check             the above, then `skynet driver check` through python-host against the simulated base, once for a
                       controller that takes TwistStamped and once for one that takes Twist
make check-ros-docker  all of it against ros2_control's own diff_drive_controller (ROS 2 Jazzy) in a container, with a
                       second reader of the same base and ROS's own command-line tools for the facts about the graph
make check-ros-humble-docker   the same against Humble's controller, which takes a plain Twist
make check-tb3-docker  the driver against ROBOTIS's own TurtleBot3 simulation, Gazebo's own pose of the model as the second opinion
make check-create3-docker      the same against iRobot's own Create 3 simulation, with its `motion_control` safety node in the path
```

## The body

No joints the OS can see (`dof: 0`), `base: "wheeled"` (or `"tracked"`, the operator's word), so `write_command` refuses and the base is moved by named motions under
`vendor.ros2.base` (§84.1), the way `drivers/pudu` and `drivers/mavlink` are:

| motion | parameters | what it does |
|---|---|---|
| `status` | none | what the driver sees of the graph: the topics, the message type it chose, who is subscribed, the odometry's age and frames |
| `velocity` | `vx`, `vy` (holonomic bases only), `wz`, `duration` (0.1 to 5 s, required) | publishes that twist every `command_period_s` for `duration`, then tells the base to stop and returns when the odometry shows it at rest; the reply carries the displacement and yaw change the odometry saw |
| `hold` | none | publishes zero velocity until the base is seen at rest |

The configuration (`open`'s JSON) has two keys with **no default, on purpose**: `max_linear` (m/s) and `max_angular` (rad/s). How fast a base may be driven is the operator's
decision, and a velocity past either is refused, never clipped. Everything else has a default that is checked rather than believed: `command_topic` (`/cmd_vel`), `odometry_topic`
(`/odom`), `message_type` (`auto`), `holonomic` (false), `base` (`wheeled`), `simulator` (true: the descriptor says hardware only when the operator does), `frame_id`
(`base_link`, for stamped commands), `domain_id`, `command_period_s` (0.05), `stale_after_s` (1.0), `open_wait_s` (5), `stop_settle_s` (3), `rest_tolerance` (0.01 m/s and rad/s). A key the driver does not
read is refused, naming the keys it does.

## What a ROS 2 base makes true, and what the driver does about it

Each of these is something nothing in the graph will say, and each is held by a check.

* **A twist is a lease, not a speed.** `diff_drive_controller` zeroes its target when the newest command is older than `cmd_vel_timeout` (0.5 s); other bases hold the last command until they get
  another. So a velocity is a stream, republished every `command_period_s` for the whole `duration`, and it always ends with a zero command, repeated until the odometry shows the base at
  rest. A `command_period_s` past 0.2 s is refused, because it stutters a base with the usual timeout. If the process dies mid-stream, the first kind of base stops within its timeout and the
  second does not; which kind a base is, the driver cannot read off the graph, and `status` says so. Held by `live.py` against the real controller: one command moves the wheels and they stop by
  themselves 0.5 s later, which is the property the simulated base's timeout stands on.
* **A twist of the wrong type reaches nobody and fails nothing.** DDS matches publisher to subscriber by topic *and type*: a `Twist` published at Jazzy's `TwistStamped` subscriber is simply not
  connected. The driver reads the type off the subscribers the graph reports on the command topic (`auto`), or is told it and refuses when the graph disagrees, creates its publisher for that
  type, and **refuses to open until the publisher is connected to someone**. Two kinds of subscriber on the one topic are refused rather than guessed between.
* **Silence is not stillness.** A base whose odometry has stopped arriving is not a base at rest. `read_state` is `ETIMEDOUT` and DEGRADED once the newest sample is older than `stale_after_s`
  (and the stamp stays the sample's own, not the read's); a velocity is refused outright on a stale picture; a stream in flight ends the first period the odometry is stale, with the base told to
  stop; and "at rest" is only ever claimed from a current sample.
* **A stop that was said once is not a stop.** `emergency_stop` publishes zero immediately and keeps publishing it every period until released, serialised against every command that asks for
  motion, so none is ever sent after the latch (a publish in flight when the stop is asked for is waited for, not raced). It cannot brake: the wheels follow the controller's own deceleration, and a
  base told to stop that the odometry still shows moving `stop_settle_s` later reads **FAULT**, which nothing else in the state would say. ROS 2 defines no fault vocabulary for a base, so the fault
  word is 0 and `drivers/fault-vocabulary.json` says why.

## What is measured, and how far

| rung | what | result |
|---|---|---|
| 4 | `selftest.py`: the driver against `SimulatedBase`, a ROS 2 base written here that behaves as the real controller does where it matters (command timeout, type matching, lifecycle) | 226 checks |
| 4 | `skynet driver check` through python-host against it, for a `TwistStamped` and a `Twist` controller | see `make check` |
| 4 | defect seeding: 58 seeded defects and one control, `tools/seed-defects/seeds/ros2_base.json`: **58 of 58 noticed** (it was 52 of 58 when first run; the six that survived are listed below) | `docs/dry-test-matrix.md` |
| 2 | `live_tb3.py` against **ROBOTIS's own TurtleBot3 simulation** (Gazebo's server, the burger, its bridge): the driver's displacement is Gazebo's own pose of the model to 2 mm and its yaw to 0.001 rad, and the plugin holds its last command for ever, which a velocity's closing zero and the stop end. **28 of 28**, and the sim half of SCTS through the real ABI 34 of 34 | `make check-tb3-docker` |
| 2 | `live_create3.py` against **iRobot's own Create 3 simulation** (Gazebo's server under a virtual display, the maker's spawn and bridges, iRobot's `motion_control` safety node between the command topic and `ros2_control`'s controller with the Create 3's own parameters): the driver talks to `motion_control`, and its displacement is Gazebo's own pose of the model to 5 mm or 5 % (0.083 m both, in the run measured), its yaw change the physics' to 5 % and 0.01 rad (0.377 against 0.355 rad: the controller's odometry turns a little more than the physics does, and the driver reports the odometry); held in the simulation's own time, which here runs at about a fifth of real time. **29 of 29**, and the sim half of SCTS through the real ABI 34 of 34 | `make check-create3-docker` |
| 3 | `live.py` against `ros2_control`'s own `diff_drive_controller` on mock hardware, with a second reader of the same base: ROS 2 Jazzy (a `TwistStamped` command) **47 of 47**, and the sim half of SCTS through the real ABI 34 of 34; Humble (a plain `Twist` on `cmd_vel_unstamped`) **46 of 46** | `make check-ros-docker`, `make check-ros-humble-docker` |

The `ros2_control` rows are **rung 3**, not rung 1: the controller is the ROS project's own implementation of the interface, which is what a base's stack builds on, and it is not any base maker's software.
A rung-1 row would be Clearpath's or iRobot's own controller on the other end. The TurtleBot3 and Create 3 rows are **rung 2**: each maker's own simulator of the robot, whose physics, differential-drive plugin or controller and
bridge are the maker's, with a physics engine's idea of where the base is as the second opinion, which shares nothing with the odometry or this driver. They are the makers' simulations and not the robots.
The Create 3's adds the maker's own safety layer: the topic the driver publishes to is read by iRobot's `motion_control`, not by the controller.

`RclpyTransport` (`transport.py`) is the one module that imports ROS, and so the one part of the driver the offline suite never runs: it is held by `live.py` alone, and the seeding campaign,
which cannot drive a container, does not seed it beyond the one control (the odometry queue depth). That is a gap, and it is stated here and in `docs/robot-support.md`.

## What running it found

* **F235 — the substrate's mock hardware does not integrate a velocity into a position unless it is told to.** `mock_components/GenericSystem` echoes a velocity command back as the velocity *state*, and
  `diff_drive_controller` (with `open_loop: false`, which is its default) takes its odometry from the wheels' *positions*: so the odometry stayed at the origin through every motion and the driver
  reported a base that never moved, while the wheels' own velocity read exactly right. The first run of `live.py` against the real controller failed seven checks: one for this (a turn the wheels made read as a yaw
  change of 0.00 rad, and the check that the pose is the one a second subscriber reads had passed vacuously, both readings being zero), and six for the check's own reasons: two because it began before the second
  reader's subscription had matched the driver's fresh publisher (it heard 19 of 40 commands), three because they read a window of the second reader's samples that had not happened yet, and one
  because a graph refusal was given two seconds when a fresh node takes about 1.4 s to discover the graph. `sim/diffbot.urdf` sets `calculate_dynamics`; the odometry then reads 0.4 m for 0.2 m/s over two
  seconds and 1.0 rad for 0.5 rad/s, which is the arithmetic.
* **F236 — a turn through the half turn was reported as a circle the other way.** `yaw_change` was the difference of two headings, each in (-pi, pi]: a base at a heading of 3 rad that turned +0.47 rad was
  reported as -5.78 rad. Shown failing first (`selftest.py`, a base placed at 3 rad), and now the turn that was made. Found by writing the oracle for a seeded defect that had survived.
* **F237 — Humble's command topic is `cmd_vel_unstamped`, and the descriptor's slug kept the suffix** (`ros2-base-diff-drive-controller-cmd-vel-unstamped`). Found by the first live run against Humble.
* **F238 — six of the 58 seeded defects were not noticed.** A yaw read at half its value (the suite checked only the direction of a turn); a stop whose first zero is left to a thread it starts, a whole
  period late (the suite did not look at which thread said it); a stop that does not wait for a slow publish in flight, and a stream that checks the latch and publishes as two steps, so a command can
  land after the stop (the suite's slow publish was as slow as the stop's own, so the order they landed in never differed); a stream that does not read the latch before it reads the odometry, so a stop
  and a blind odometry in one period end as a blind stream and not as the stop; a fault remembered after the base is at rest (the base the test moved afterwards had stopped again before it was read).
  Each is a test now, and all six are noticed.

## What it cannot show

* **A base on a floor.** The hardware is `mock_components/GenericSystem`: never late, never stalled, no inertia, no wheel slip. What is measured is the arithmetic of the driver and the controller and the
  behaviour of the graph, not a motor, a battery, a bumper or a floor. Layers 6, 7, 9 and 10 stay unreached.
* **Any other base, and the TurtleBot3 and Create 3 themselves.** The TurtleBot3 Burger and the Create 3 were run in their makers' own simulations and never as the robots; Husky, TurtleBot 4 and the rest are claimed from their documentation, not run. Each publishes its odometry with its own QoS, covariance and frames, and some take their
  commands on a different topic or through a multiplexer; the driver reads the QoS compatibly (best effort matches either), reports the frames as published, and refuses to open when either
  topic is missing or unconnected, which is the right failure but not a measurement of that robot.
* **Navigation.** No `goto`: odometry drifts, the driver has no map, and a goal-seeking loop on dead reckoning with no obstacle information would be a motion the operator did not ask for. Nav2 is a
  separate driver, if it is ever one.
* **The base's own safety.** Bumpers, cliff sensors, the e-stop chain and a hardware command timeout are the base's. The driver's stop is a zero twist on a topic.
