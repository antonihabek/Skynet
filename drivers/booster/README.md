# skynet-booster — a Booster T1 or K1 over the vendor SDK's low-level channels

`DRY-COVERAGE-PLAN.md`'s Phase 7 item 3 names the T1 after the Unitrees: "then Fourier and
Booster". The interface is the T1's low-level DDS service (`rt/joint_ctrl` commands,
`rt/low_state` feedback), documented in the T1 Instruction Manual v1.1 pp55-56; the substrate is
*"SDK sim"* in the same honest reading as the Unitree row.

```bash
git clone --depth 1 https://github.com/BoosterRobotics/booster_robotics_sdk /tmp/booster_sdk
make check BOOSTER_SDK=/tmp/booster_sdk
```

## The vendor's SDK is on both sides, and that is the whole argument

`fake_robot.cpp` publishes `rt/low_state` and consumes `rt/joint_ctrl` using
**booster_robotics_sdk's own channel classes** (Apache-2.0), exactly as the driver does. So the CDR
encoding, the DDS discovery, the topic types and the message layouts are Booster's code at both
ends — nothing here can be wrong about the wire without being wrong in the way a real robot would
refuse.

What the fake is **not** is a robot:

* the joints integrate a first-order model with no dynamics, no contact and no gravity — a humanoid
  here never falls over;
* the rate is nominal, from a thread with `sleep_for`;
* there is no safety hardware behind it. The estop it observes is a message;
* it behaves as a T1 in CUSTOM mode (below), always.

What it does so that the suite can tell a defect from a coincidence (found by seeding the driver's
defects and reading what survived — `DRY-COVERAGE-PLAN.md` 12.C.3): every joint starts at a different
non-zero angle, and the velocity and torque estimates it reports carry a fixed offset per joint, so a
sign, a slot or a view slipped in the driver changes a number that is neither zero nor a neighbour's;
the *parallel* state vector carries other numbers than the serial one, so a driver that read the view
it does not command is seen; the impedance law is evaluated every step against the joint as it is, so
a held joint settles instead of running away at the speed its last command left it; a command with an
idle motor mode is counted; and `fake_robot <iface> <hz> <s> <joint>=<value> layout=<s>` publishes
twenty-two joints, as another body on the topic would, from the first sample or from a moment in.

## Custom mode is the operator's, and the robot enforces it

"The low level publish interface will only take effect when the robot is in custom mode" (manual
p55). Entering custom mode — through the high-level ChangeMode API or the joystick — is the
operator's setup, the way UR's dashboard setup is the operator's. The driver does not set modes and
does not gate on them: a driver commanding in WALK presents as a robot that ignores commands, which
is the robot's documented behaviour rather than a fight with the balance controller.

## Four things the mode mapping has to get right

§24 commands a body in one mode and Booster's `MotorCmd` is not a mode at all. It is an impedance
command: every motor carries `q`, `dq`, `tau`, `kp` and `kd` at once.

**1. There are no sentinels, so a zero gain is the ignore.** A velocity command sets `kp = 0` with
the `dq` setpoint; a torque command zeroes both gains with `tau`. That reading assumes the standard
impedance law behind the field set, which no obtainable Booster document states outright — it is
marked as what it is, and the bench confirms it. The fake implements exactly that law, so a driver
bug in the gains is still caught dry, and the suite reads every field off the wire. Held to a
control — sending PARALLEL fails the view check and the run exits non-zero.

**2. `kp` and `kd` are the operator's, and are not guessed.** A position or velocity command with no
gains in this driver's config is **refused** (§63.4 through a gain, the way the Unitree driver
refuses it). A torque command needs no gain and is accepted.

**3. The layout and the view are load-bearing.** The SDK header warns that a command with the wrong
layout "can address the wrong actuator or cause the robot to reject the message" — so the driver
sends exactly 23 motors and refuses a state that does not carry 23, and the fake drops and counts
anything else. SERIAL view everywhere, commands and state alike, per the manual's consistency rule;
the ankles read as pitch/roll series joints with the crank conversion inside the robot.

**4. A masked joint is held, which is not the same as zero gains.** Because a zero gain is this
message's "ignore", §24's mask cannot be written as one: a zero in the mask is *"hold this joint at
its current setpoint"*, and the kernel writes an all-zero mask every cycle it has nothing to
forward. A masked joint carries its last position setpoint — or, with none, the position measured
when the hold began, latched once — with the operator's `kp` and `kd`. Until 2026-09-24 it carried
zero gains, the reading Unitree's own simulator showed folding a standing body
(`DRY-COVERAGE-PLAN.md` F121). With no gains configured a hold is refused.

Every command carries `weight = 1` on every joint: explicit full-user (manual p56). Moot in custom
mode, where the internal controller sends nothing — and still the contract, because anything less
would hand part of a joint to a controller the Safety Kernel cannot see.

## Where the names come from, and where they disagree

The manual names the arm joints shoulder pitch/roll/yaw + elbow (p1, p3); the SDK's `JointIndex`
enum names IDs 4/5 elbow pitch/yaw. The limits settle it — ±128° symmetric is a shoulder yaw,
+2°/-120° is an elbow that cannot hyperextend — so the names and roles follow the manual, and the
code says so where it matters. IDs and limits are unaffected either way.

## What the state says is wrong, and the word that says which

The robot's own report of trouble is one field per joint, `lost` on the serial view's `MotorState`. `read_state` takes the first joint
whose field is non-zero, sets `DEGRADED` — the bit §27 reads — and carries **both halves** in §24's one fault word: the joint's 1-based
ordinal in the high sixteen bits and the vendor's own number in the low sixteen. A word of zero stays "nothing reported".
(`DRY-COVERAGE-PLAN.md` F205: the word used to be the ordinal alone, under a comment that called it the vendor's word verbatim, so the
number the maker defined never reached the black box, and nothing tested it — the fake published `lost = 0` for ever.) `run-check.sh`
now runs a second fake with one joint reporting `70000`, a number that does not fit sixteen bits, and `selftest <joint>=<value>` holds
the word to the packing and to `DEGRADED`; the ordinary run holds a robot that reports nothing to a zero word.

What this does not say: whether `lost` is a count since boot (so `DEGRADED` would never clear) or a window is in nothing Booster
publishes that this tree holds. The driver reads a non-zero one as trouble and says so; it does not interpret it, and it sets neither
`FAULT` nor `ESTOP` for it.

## The robot has a clock of its own

The body publishes `LowState` when it likes, whatever the caller does, so what `read_state` returns is a *sample* with an arrival time, and the two facts §24 gives a state are the sample's: the stamp is when
it arrived (*"at acquisition, never at return"*) and the sequence counts samples, so that a repeat is the stalled producer the watchdog looks for. Until F278 — the Unitree driver's F277, found again — a read
stamped the last sample with the time of the read and advanced the sequence with every call, so a robot whose link had been cut kept reading as fresh. The link-chaos contract found it (below).

The delivery thread now stamps each sample as it arrives and counts it. A read waits for a sample the last read did not return, for the watchdog window (20 ms, or twice the interval the robot is measured
publishing at when it is slower than fifty hertz; never more than a tenth of a second), and then returns the last one with its sequence and its stamp as they were: `drivers/lite3`'s rule (F104) and
`drivers/ur_rtde`'s (F27), with the price they pay too — a read is held for the watchdog window on a robot that has stopped publishing. A quiet robot reads OK and says so by the sequence standing still and the
stamp getting older; it carries no flag, because `DEGRADED` is §27's word for a drive in trouble. The suite holds it against the fake, which `run-check.sh` hands to it by pid (`FAKE_PID`) so that it can stop
the robot (SIGSTOP) and start it again (SIGCONT): the sequence goes up by the states that went by and not by the calls, a read returns as the next state arrives, a quiet robot reads with the sequence and the
stamp it had and a stamp as old as the silence, the sequence advances when it publishes again, and a new session counts its own samples.

## What a bad link does

`make chaos BOOSTER_SDK=...` asks the link-chaos contract (`tools/faultproxy/chaos.py`, DRY-COVERAGE-PLAN.md item 12.C.4) six questions of the driver against a link that goes wrong: is every call bounded, does the
stop return at once, is an outage reported, does the driver recover, is an OK read fresh, and does the robot act on its commands once the link is back. DDS names its peers in discovery messages, so there is
no address to put a relay in front of: the faults are the kernel's own link emulation (`tc netem`), on the loopback of a network namespace of its own that the driver and the fake robot share
(`tools/faultproxy/relays.py`, `NetemRelay`). Where the body really is and how many commands it has acted on come through a pipe (`fake_robot`'s `tap=<fd>`), because a socket on that interface would meet
the profile too. Eight profiles run: delay, jitter, a blackhole, loss, a burst of loss, duplicates, reordering and corruption — the ones a kernel queue can carry. **56 checks, 0 failed**; before F278 the
blackhole's *outage reported* failed with "0 of 10 reads not OK". **The fake agrees with whoever wrote it** (F15): this shows the driver's waits, staleness and stop path, and nothing about what a T1's
onboard computer does on a dead link. (Booster's SDK is Fast DDS and Unitree's is CycloneDDS: the same profiles meet both, and the blackhole stopped both streams.)

## The K1, and a simulator that is not ours

`model` in the configuration selects the body: `t1` (the default, 23 joints) or `k1` (22). A model the driver does not speak is refused by name, and so is a body whose state carries the other layout, **at open and
on every read, in both directions**: a K1 session on a T1's 23 serial joints, and a T1 session on a K1's 22, are `SKR_EIO` and are never indexed (the SDK header's "wrong actuator"). The K1 has no manual here. Its
layout is the SDK's own `JointIndexK1` (`b1_api_const.hpp`: no waist), and its names, order and travel are those of the maker's model of it, `BoosterRobotics/booster_assets` `robots/K1/K1_22dof.xml` (BSD-3-Clause),
which is also the file the simulator below loads. **The roles of its arm are the SDK's and the model's words** (`elbow.pitch`, `elbow.yaw`), where the T1's manual overruled the SDK's: the K1's limits (a symmetric
130° and a one-sided 140°) read like the T1's shoulder yaw and elbow, so those two per arm may be the same joints under the T1's names. A planner that binds by role should read them as the maker's names and not as a
statement of what the joint does. The K1's descriptor declares no hard emergency stop and no onboard compute (nothing read here says it has either), no torque or speed rating, and cites the model file.

Offline, `make check-k1` runs a K1 `fake_robot` (`model=k1`, 22 joints, the SDK on both ends): `selftest_k1.cpp` (**42 checks**), `skynet driver check` told `model: k1` (**34 passed, 0 failed**), the fake's own
count of what was sent, a K1 session against a T1's layout (1 check) and a K1 that starts publishing the T1's layout mid-session (3). This is the same fake agreeing with its author as the T1's, and says the same
small thing.

**What is not the fake: NUbots' `NUSim`** (`NUbots/NUSim`, MIT), a MuJoCo simulator of the K1 written by a team who wanted one to walk in, which "speaks the Booster SDK over FastDDS, the same protocol as the real
robot": it publishes `rt/low_state` (50 Hz), tracks `rt/joint_ctrl` with a PD law at 1 kHz against the maker's K1 model in CUSTOM mode, and answers the SDK's mode RPC. It is a third party's implementation of the
robot's side of the wire this driver speaks and knows nothing of this driver (the stage table's rung 3, and why the row can be beta).

```bash
make check-nusim-docker BOOSTER_SDK=/tmp/booster_sdk     # Docker; the first run builds Fast-DDS from source in the simulator's own image (minutes)
```

`nusim/run-nusim.sh` fetches NUSim at the commit it pins by its SHA-256 (`NUSIM_REF=main` takes the tip of its default branch, which the nightly's second run does), builds it in the toolchain image its own
Dockerfile names, and runs it headless in one container; a second container holds the driver and the SDK on the same host network (Docker Desktop's `--network host` is the Docker VM's, not the machine a script runs on,
so the driver cannot run outside it), puts the simulator in CUSTOM mode through the SDK's `ChangeMode` (`nusim/mode_custom.cpp`), and runs two things, each against a simulator started fresh:

1. **The K1, joint by joint** (`nusim/nusim_live.py`, **32 checks**). The session opens on 22 serial joints and every one reads inside the maker's range; commanded to where it stands the body stays up (the worst
   drift was 0.003 rad); and then each of the 22 joints is moved on its own by 0.1 rad, **measured against a control** (the same body, held for the same time, with nothing moved, because a standing body answers a
   leg joint moved with other joints moving: a knee bent is a hip and an ankle moved), and the joint that was asked has to have moved the right way, by at least 20% of the step, and by at
   least 60% of what the most-moved other joint did. All 22 do (typically 95 to 104% of the step; the loaded hips and knees 37 to 81%). The same harness **fails a table slipped in the driver**: with every position command landing one slot along,
   22 of 22 fail; with the left hip pitch and knee exchanged, those two fail. Then the stop: the call returns, a command after it is refused, and a body commanded to do nothing does not hold itself up (its
   joints moved by up to about 2 rad in 1.2 s).
2. **The sim half of the conformance suite, through the real ABI, against the simulator**: `skynet driver check` told `model: k1`, **34 passed, 0 failed**, 1 not measured (the descriptor's signature, which a driver
   cannot make), 5 not applicable. It caps the tier at guest, as every simulated run does.

What it found: the SDK will not create its participant without an `interface` (here `lo`) or a FastDDS profile named `booster_dds`; the first `ChangeMode(kCustom)` sent to a fresh simulator is not answered (the
SDK's one-second RPC timeout, code 100: the simulator has not yet discovered the client) and the second is, so the helper tries up to ten times; and a simulator whose camera cannot make an OpenGL context says so and runs without it. What it cannot show: a K1. The simulator's servo is a PD law on a rigid
model with no motor, no backlash, no communication delay and no safety layer; the driver's one `kp` and `kd` for every joint (150 and 3 here) were chosen so that the body stays up and are not a K1's; the
simulator's idea of CUSTOM mode is its author's reading of the SDK; and nobody at Booster has looked at any of it. The T1 is still the fake, and nothing here says anything about it that the fake did not.

## Where it stops

| | |
|---|---|
| **Guest tier** | Reached. The T1: 123 checks in its own suite (101 before F282's and the fifty-sixth session's pose checks, 89 until F278; measured 2026-10-06), 5 more against a fake that reports one joint in trouble, 4 against ones that publish another body's layout, 32 through the real ABI. The K1: 42 checks against a K1 fake, 34 through the real ABI, and 32 checks and 34 through the real ABI against NUbots' simulator (above) |
| **`rhal.state.rate`, `rhal.command.latency`** | The row's *"joint access at rate"*: a bench measurement, as ever |
| **`safety.estop.latency`** | This driver publishes zero gains and zero torques, which under the impedance law is the definition of commanding nothing — and it is not the T1's hard emergency stop (manual p1), a button on the robot |
| **The impedance law itself** | Assumed standard, marked in the code, for the bench to confirm on hardware |
| **Contact, gravity, a body on the ground** | Not in the substrate at all |

## What the descriptor declares, and what it will not

Travel per joint, from the manual's limit table (pp3-4), because travel is a kinematic fact about
the machine. Everything else rated is absent: Booster publishes no joint torque, no joint speed and
no DDS rate, so per-joint `effort_max_nm` is missing on all 23 joints, the envelope's torque ceiling
is null, and the control rates are null (F64's shape: the envelope says what is missing in its
`source` rather than inventing it). Watchdog 20 ms, by driver design.

Two published hardware facts are declared: the hard emergency stop is fitted (manual pp1-2, the
envelope's `estop`), and the onboard compute is an AGX Orin (Basic) or i7 + AGX Orin (Standard) on a
body sold as "a humanoid robot development platform" — which earns `platform.onboard_compute`.

The base is `legged` and `base_pose` is **all NaN**: the IMU gives orientation, the T1's odometry
lives on a topic this driver does not read, and nothing gives position (§24's rule that an
unmeasured quantity is NaN and never zero).

## Licences

This directory is Apache-2.0. It links `booster_robotics_sdk` (Apache-2.0, FastDDS inside); it is
not vendored here and `make` refuses rather than building half of itself when the SDK is not named.
