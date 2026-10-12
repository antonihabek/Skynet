# skynet-x1 — an AgiBot X1 humanoid over its own ROS 2 topics

`DRY-COVERAGE-PLAN.md`'s Phase 7 item 3 names this row's substrate as *"genie_sim, the vendor's
own, open source"*. What is built instead is the vendor's other open deployment,
[`agibot_x1_infer`](https://github.com/AgibotTech/agibot_x1_infer): AimRT modules, a MuJoCo sim,
ONNX policies, ROS 2 topics — the whole stack open source, and it runs here. The driver is an
ordinary rclcpp node speaking the two topics the sim owns — `/joint_states` in, `/joint_cmd`
out — the way the vendor's own ControlModule does. Twenty-nine joints,
position/velocity/torque, discovered live off the first JointState.

```bash
make check-sim-docker    # the vendor's tree built in a pinned image, then all of `make check`

# or, against a vendor tree built by hand (below):
git clone https://github.com/AgibotTech/agibot_x1_infer /tmp/x1_infer
make check X1_INFER=/tmp/x1_infer ROS_ROOT=$CONDA_PREFIX
```

## Three facts shape the driver, and all three are the vendor's

Read off their code, not their docs:

**1. The sim steps once per incoming command.** `SimModule::CmdCallback`: write command, one
`mj_step`, publish states. No commands, no states — so `open()` against a silent sim primes a
zero-torque full-vector command until states arrive (opt-in: `"prime":true`). A read returns the
latest state and waits, for no longer than the watchdog window (100 ms), for one the last read
did not return; the sequence it reports counts the states that arrived, so a read that gets
nothing new repeats the sequence and the stamp of the one before, which is how §24 tells a
producer that has stalled from one that has not (it counted calls until F328, so a link that had
stopped carrying states read as fresh for ever). The sim says nothing unasked, so with `prime` a
read that finds nothing owed asks for a state by sending the last frame again, and a read that
follows a command waits for the state the command provokes. The sim also ignores everything for
its first three seconds, so the wait outlasts that window from any start.

**2. Every command must name every joint.** `WriteMotorCmd` builds its index map from the
command's own names and `operator[]` defaults a missing joint to index 0 — a partial command
would drive twenty-eight joints from the first joint's setpoint, silently. The driver always
sends all twenty-nine, and a masked joint is §24's hold: its last position setpoint, or the
position measured when the hold began, latched once and held with the operator's gains. With no
gains configured, a command that masks a joint is refused rather than sent as a free joint. Until
2026-09-24 the hold re-read the measured position every cycle, which zeroes the stiffness term and
lets a loaded joint drift (`DRY-COVERAGE-PLAN.md` F121, carried as 8.13). The suite asserts the full
vector on the wire and the fake counts deviations; the fake echoes every position it is sent, so it
shows the setpoint hold and the refusal, and the drift the latch prevents is the sim's to show.

**3. A command is an impedance frame.** Position, velocity, effort, stiffness, damping per
joint, applied as torque = effort + (pos-q)*kp + (vel-dq)*kd. Position and velocity need gains
the operator chose (Unitree's rule: no default stiffness, §63.4 — a position command without
`kp`/`kd` in the config is refused); torque needs none (kp = kd = 0, the vendor's own RL
shape). Per-joint modes (§86.1) select the lane per joint; every joint offers all three.

## The sim is the motion substrate, and the fake is the logic substrate

`make check` runs in two phases, in sequence:

* **Phase 1, the fake on DDS domain 42.** `fake_x1` speaks the same two topics in the vendor's
  own message types — four real X1 joints, scripted nonzero states, every command recorded —
  where the sim (domain 0) can neither hear it nor answer for it. The suite's 155 checks run
  against it (the discovery, the mapping read off the wire, the mask, the clamp, every
  refusal), then `skynet driver check` through the real ABI, then the fake's own count:
  malformed, out-of-range and partial commands must all be zero. Physics is the wrong oracle
  for refusals: a clamping check should not depend on MuJoCo stepping.
* **Phase 2, the vendor's sim on domain 0.** `run-check.sh` generates the substrate config
  from the vendor's own sim config at check time (the subscribe backend gains `ros2`, so
  SimModule hears `/joint_cmd` from outside AimRT; the ControlModule goes away, since its
  policy would fight the driver for the same joints), boots the sim headless under llvmpipe,
  and runs the motion proof: the left elbow displaced 0.3 rad from its live pose and asserted
  to have travelled — then `skynet driver check` against physics instead of the fake.

What the sim is **not** is a robot:

* the rate is nominal — one step per command, driven by the prober, not a control loop;
* the render thread is software rasterising with no window, and AimRT swallows SIGTERM once
  spinning (observed 2026-09-20), so the script KILLs what TERM leaves behind;
* there is no safety hardware behind it. The stop it observes is a zero-torque frame plus a
  latch — the vendor's RL zeros — and the descriptor declares no capability for it, because
  nothing here is rated.

## Building the vendor tree

The reproducible way is the image: `make check-sim-docker` builds `sim/Dockerfile` — ROS 2 Jazzy,
AgiBot's tree at a pinned commit, and the AimRT it names with `sim/aimrt-jazzy.patch` applied — and
runs all of `make check` inside it under Xvfb. Nothing is needed on the host but Docker, and it is
what the nightly substrate workflow runs, at the pin and at AgiBot's `main`. In it, the four patches
below come down to two things: DART's lodepng is Ubuntu's `libdart-external-lodepng-dev` rather than
a patch (the vendor links it and never defines it), and the three AimRT changes are the one patch
file. The substrate config also drops the vendor's JoyStickModule, which segfaults the sim when no
joystick is attached (DRY-COVERAGE-PLAN.md F132).

By hand, on a machine of your own: AimRT, protobuf and MuJoCo take a while, and a jazzy machine
needs four local patches — one in the vendor tree, three in the AimRT the configure step fetches
(0.9.1):

1. `src/module/sim_module/CMakeLists.txt`: upstream links `dart-external-lodepng` without
   defining it (their dev machines carry DART). Fetch lodepng beside the build (one header +
   one TU, zlib licence) and define the target, as the patch in a built tree shows.
2. `aimrt/src/plugins/ros2_plugin/ros2_channel_backend.cc`: drop the `<void>` template
   argument jazzy's `to_rcl_subscription_options` no longer takes.
3. `aimrt/src/plugins/ros2_plugin/ros2_adapter_subscription.h`: the `SubscriptionBase`
   constructor grew arguments, and jazzy added dynamic-type pure virtuals — declare them.
4. `aimrt/src/plugins/ros2_plugin/ros2_adapter_subscription.cc`: define those virtuals to
   throw, the way rclcpp's own subscription does (this adapter delivers typed and serialized
   messages only).

MuJoCo itself ships in the vendor tree (`sim_module/third_party/`); GLFW and SDL2 come from
the ROS environment. Then the vendor's own `./build.sh` with `-DXYBER_X1_INFER_SIMULATION=ON`.

## Where it stops

| | |
|---|---|
| **Guest tier** | Reached, on both substrates. 155 checks in the driver's own suite (measured 2026-10-06 in the seeding image, on the fake; F282's non-finite-setpoint checks and the fifty-seventh session's three — what the operator is told for a missing model, a joint's range read only to the end of its element, and `release_stop` — are in that number), 34 passed and 1 failed through the real ABI: the failure is `embodiment.roles.coverage`, because the roles are the operator's to give. 77 of 77 seeded defects are noticed (one equivalent mutant left out, three controls that survive as they must) |
| **`rhal.state.rate`, `rhal.command.latency`** | The row's *"joint access at rate"*. One step per command over loopback is fast; a real body's topics cross the network to the robot's own computer, and what that costs is a bench measurement |
| **`safety.estop.latency`** | A zero-torque frame plus a latch is what the vendor's RL stack commands, and it is not a power cut. A driver reporting `SKR_OK` for a frame it merely wrote would be claiming otherwise |
| **Contact, gravity, a body on the ground** | In the substrate, unmeasured: the motion proof moves an arm, not a leg, and asserts travel, not balance |

## What the descriptor declares, and what it will not

Travel per joint, from the sim's own MJCF (`model`, defaulting under `$X1_INFER`) — the sim's
design values, authoritative for clamping the sim and for nothing else, which is why §63.4
keeps them in the joints and out of the envelope. The envelope carries a **torque ceiling
only**, the Unitree hedge: 200 N·m is the PowerFlow actuator's published peak
(agibot.com/products/X1 — the knee/max, not a per-joint rating). AgiBot publishes no joint
velocity anywhere, so `joint_ratings` stays unmet and each joint carries position travel alone.
Watchdog 100 ms, driver design, stated as such. No control rate: the sim's step rate is not a
rating.

The base is `legged` and `base_pose` is **all NaN**: the free joint's pose exists in the
physics and nowhere on this wire — JointState carries joints, the IMU gives orientation, and a
driver that wrote either into a pose would be claiming a position it does not have (§24's rule
that an unmeasured quantity is NaN and never zero).

Roles wait for a human: `roles` maps joint names to §5.1 in the config, validated at open
(unknown joints and ungrammatical roles are refused, loudly), and the descriptor carries the
normalised form. Undescribed joints match by exact name only — which is truthful, and is also
the ceiling on what this body can run.

## Licences

This directory is Apache-2.0. It links the vendor's `agibot_x1_infer` tree (Mulan PSL v2),
MuJoCo (Apache-2.0), lodepng (zlib) and ROS 2 (Apache-2.0); none is vendored here and `make`
refuses rather than building half of itself when the vendor tree is not named.
