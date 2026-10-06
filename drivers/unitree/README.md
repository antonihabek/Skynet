# skynet-unitree — Unitree bodies over `unitree_sdk2`'s low-level channels

`DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this row its substrate as *"SDK sim"*, its
dry-verifiable column as *"protocol, low-level mode mapping"*, and its **not** dry-verifiable column
as *"joint access at rate"*. Phase 7 item 3 extends it from the Go2 it opened with to the buyable
humanoids: the G1, the H1, and the H1-2, and Phase 12 (12.E) added the B2, the H2 and the R1. One driver,
`model` in the config selects the body (`go2`, `b2`, `h1`, `g1`, `h1_2`, `h2`, `r1`; default `go2`).

```bash
git clone --depth 1 https://github.com/unitreerobotics/unitree_sdk2 /tmp/unitree_sdk2
make check UNITREE_SDK2=/tmp/unitree_sdk2   # all seven bodies, in sequence
```

## The vendor's SDK is on both sides, and that is the whole argument

`fake_robot.cpp` publishes `rt/lowstate` and consumes `rt/lowcmd` using **unitree_sdk2's own channel
classes**, exactly as the driver does. So the CDR encoding, the DDS discovery, the topic types and
the message layouts are Unitree's code at both ends — nothing here can be wrong about the wire
without being wrong in the way a real robot would refuse.

That is the strongest substrate of the three rows this session built. `drivers/franka` had to write
its own fake because there is no Franka simulator (F15's problem, answered there by compiling the
vendor's headers); this one did not.

What the fake is **not** is a robot:

* the joints integrate a first-order model with no dynamics, no contact and no gravity — a leg here
  never touches the ground, and a humanoid here never falls over, which is the larger lie for the
  taller bodies;
* the rate is nominal, from a thread with `sleep_for`, and a real Go2 publishes at 500 Hz from its
  own onboard controller;
* there is no safety hardware behind it. The estop it observes is a message.

## Three things the mode mapping has to get right

§24 commands a body in one mode and Unitree's `MotorCmd_` is not a mode at all. It is an impedance
command: every motor carries `q`, `dq`, `tau`, `kp` and `kd` at once, and the drive computes
`tau_out = tau + kp*(q - q_measured) + kd*(dq - dq_measured)`.

**1. The sentinels are not zeros.** `PosStopF` (2.146e9) and `VelStopF` (16000) are Unitree's "ignore
this field" values. A driver writing `0.0` into `q` for a torque command is not saying nothing — it
is commanding the joint to zero radians, and with any `kp` at all that is a motion. This is the
defect the row exists to catch, and it is caught twice: the suite reads the message off the wire, and
`fake_robot` counts it. Held to a control — writing zeros there fails the torque-mapping checks and
the run exits non-zero, and shifting every joint one slot too high fails the placement check plus
seven more.

**2. `kp` and `kd` are the operator's, and are not guessed.** They are not in a §24 command. A
position or velocity command with no gains in this driver's config is **refused**, because a default
would be a stiffness nobody chose in front of a Safety Kernel that cannot see it. That is §63.4's
rule arriving through a gain instead of through a rating, and it is the same refusal
`drivers/canopen` makes about a unit with no provenance. A torque command needs no gain and is
accepted.

**3. The CRC is load-bearing.** A `LowCmd_` whose CRC32 does not match is dropped by the robot,
silently, so a driver that omitted it would present as a robot that ignores commands. Unitree's CRC
is bit-reflected with polynomial `0x04C11DB7`, initial value all ones and **no final inversion** —
not any of the common parameterisations, which is why it is written out rather than reached for from
a library. The fake checks it.

**4. A masked joint is held, and on this drive a hold is a stiffness.** §24 defines a zero in the
mask as *"hold this joint at its current setpoint"*, and the kernel writes an all-zero mask every
cycle it has nothing to forward. Until 2026-09-24 this driver read the mask as "say nothing" —
sentinels and zero gains, a free joint — and the vendor's simulator showed what that means: the first
hold folded a standing Go2 from 0.385 m to the ground, and every humanoid's raised arm fell
(`DRY-COVERAGE-PLAN.md` F121). A masked joint now carries its hold target with the operator's `kp` and
`kd`: its last position setpoint when it has one, otherwise the position measured when the hold
began, latched once rather than re-read every cycle (a target that follows the measurement holds
nothing). With no gains configured a hold is refused, loudly, rather than answered with a free body.

## Against the vendor's own simulator

`make check-sim-docker` runs the driver against `unitree_mujoco` — Unitree's own simulator, built
unmodified from a pinned commit in `sim/Dockerfile` and run headless under Xvfb. Where the fake
counts what a message *says*, the simulator applies Unitree's impedance law in physics, so a mapping
mistake shows up as motion. Per body, `sim_check.cpp`:

* holds every slot to the actuator of the same name in the vendor's model, and reads the model's
  sensor wiring before trusting it (F119, F123);
* stands the Go2 with the vendor's own stand pose and gains (`stand_go2.cpp`, kp 50 / kd 3.5), from
  0.117 m on the ground to 0.385 m, and the B2 with its example's (`b2_stand_example.cpp`, kp 1000 /
  kd 10), from 0.126 m to 0.625 m;
* raises a humanoid's arm sideways on the simulator's elastic band, with the vendor's example gains,
  and asserts the drive is carrying the arm's weight;
* holds with the kernel's all-zero mask for two seconds — a quadruped does not move (the B2 sags
  0.003 m), and no arm sags more than 0.01 rad;
* stops, and reads every actuator's force sensor at exactly zero, then watches the quadruped fold
  (the B2 from 0.624 m to 0.124 m) and the arm fall;
* then runs `skynet driver check` against the simulator as well.

It found three things the fake could not: the limp hold above (F121), a teardown race in `close()`
that aborted the process one conformance run in two (F122, `Link` in `unitree_bridge.cpp`), and six
torque sensors in the vendor's H1-2 model wired to the neighbouring joint, so that simulator
publishes the right hip's and the left shoulder's `tau_est` under the wrong motors (F123 — a vendor
defect, named on every run until it is fixed upstream).

## Fewer joints than slots, on every body

Both message families are shared with larger bodies than any one of these seven. The Go2 and the B2
have twelve joints each in a twenty-slot message; the H1 has nineteen in the same twenty (slot 9 is the vendor's own
`kNotUsedJoint`); the G1 has twenty-three in a thirty-five-slot message, sparsely (slots 13, 14, 20,
21, 27, 28 are the 29-DoF body's); the H1-2 has twenty-seven in the same thirty-five; the H2 has thirty-one, contiguous; the standard R1 has twenty-six, sparsely (0 to 13, 15 to 19, 22 to 26, 29 and 30). The driver
reports its joints and fills every slot — the ones the body does not have get the sentinels and no
gain, because a slot left at the struct's zero is a well-formed command to a joint that is not
there, and on a body that *does* have it it is a motion.

The joint-to-slot maps live in `unitree_models.hpp`, included by the driver, the fake and the suite
alike, so the three cannot disagree about the wire. Each map is transcribed from the vendor's
published low-level example and says which. The suite commands distinct values per joint and asserts
each landed in its mapped slot — the check a map disagreement fails.

Two different body plans share two message families: the Go2, the B2 and the H1 speak the `go2` family, the
G1, the H1-2, the H2 and the R1 the `hg` family, on the same topic names with different types. The H1 appears twice
because the product spans both: the original H1's example drives the `go2`-shaped message and the
H1-2's drives `hg`, and no firmware note says which interface the older generation speaks today.

## What the state says is wrong, and the word that says which

The robot's own report of trouble is one field per joint, and the field depends on the family: `lost` on the `go2` message and
`motorstate` on the `hg` one. `read_state` takes the first joint (in the driver's order) whose field is non-zero, sets `DEGRADED` —
the bit §27 reads — and carries **both halves** in §24's one fault word: the joint's 1-based ordinal in the high sixteen bits and the
vendor's own number in the low sixteen. A word of zero stays "nothing reported". (`DRY-COVERAGE-PLAN.md` F205: the word used to be the
ordinal alone, under a comment that called it the vendor's word verbatim, so a drive's own trouble code never reached the black box,
and nothing tested it — the fake published `lost = 0` and `motorstate = 0` for ever.) `run-check.sh` now runs a second fake per body
with one joint reporting `70000`, a number that does not fit sixteen bits, and `selftest <model> <joint>=<value>` holds the word to the
packing and to `DEGRADED`; the ordinary run holds a robot that reports nothing to a zero word.

What this does not say: whether `lost` is a count since boot (so `DEGRADED` would never clear) or a window, and what a `motorstate` value
means, are in nothing Unitree publishes that this tree holds. The driver reads a non-zero one as trouble and says so; it does not
interpret it, and it sets neither `FAULT` nor `ESTOP` for it.

## The robot has a clock of its own

The body publishes `LowState_` when it likes, whatever the caller does, so what `read_state` returns is a *sample* with an arrival time, and the two facts §24 gives a state are the sample's: the stamp is
when it arrived (*"at acquisition, never at return"*) and the sequence counts samples, so that a repeat is the stalled producer the watchdog looks for. Until F277 a read stamped the last sample with the
time of the read and advanced the sequence with every call, so a robot whose link had been cut kept reading as fresh: the link-chaos contract found it (below), with its blackhole profile.

The delivery thread now stamps each sample as it arrives and counts it. A read waits for a sample the last read did not return, for the watchdog window (20 ms, ten DDS periods — or twice the interval the
robot is measured publishing at, when it is slower than fifty hertz; never more than a tenth of a second), and then returns the last one with its sequence and its stamp as they were. It is `drivers/lite3`'s
rule (F104) and `drivers/ur_rtde`'s (F27), and the price they pay too: a read is held for the watchdog window on a robot that has stopped publishing. A quiet robot still reads OK, and says it by the
sequence standing still and the stamp getting older; it carries **no flag**, because `DEGRADED` is §27's word for a drive in trouble and a silent stream is the sequence's to report.

The suite holds all of it against the fake, which `run-check.sh` hands to it by pid (`FAKE_PID`): while the robot publishes, the sequence goes up by the states that went by and not by the calls, and a read
returns as the next state arrives and not after the window; with the robot stopped (SIGSTOP) a read waits the window, returns the last state with the sequence and the stamp it had, and the stamp is as old as
the silence; started again (SIGCONT), the sequence advances again; and a new session counts its own samples. Run by hand, without `FAKE_PID`, it says it skipped the stop.

## What a bad link does

`make chaos UNITREE_SDK2=...` asks the link-chaos contract (`tools/faultproxy/chaos.py`, DRY-COVERAGE-PLAN.md item 12.C.4) six questions of the driver against a link that goes wrong: is every call bounded, does
the stop return at once, is an outage reported, does the driver recover, is an OK read fresh, and does the robot act on its commands once the link is back. DDS names its peers in discovery messages and sends to
multicast groups and ports nobody configured, so there is no address to put a relay in front of: the faults are the kernel's own link emulation (`tc netem`), on the loopback of a network namespace of its own
that the driver and the fake robot share (`tools/faultproxy/relays.py`, `NetemRelay`). What the fake reports about itself — where the body really is, and how many commands it has acted on — comes through a
pipe (`fake_robot`'s `tap=<fd>`), because a socket on that interface would meet the profile too. Eight profiles run: delay, jitter, a blackhole, loss, a burst of loss, duplicates, reordering and corruption
(a kernel queue cannot hold packets and let them go at once, cut one short, reset a connection or fault one direction, so those are not asked). **56 checks, 0 failed**; before F277 the blackhole's *outage
reported* failed with "0 of 10 reads not OK", and the driver's own suite had no way to meet it. **The fake agrees with whoever wrote it** (F15): this shows the driver's waits, staleness and stop path, and
nothing about what a Go2's onboard computer does on a dead link.

## Where it stops

| | |
|---|---|
| **Guest tier** | Reached, on all seven bodies. 870 checks across the seven suites against the fake (go2 121, b2 122, h1 124, g1 123, h1_2 127, h2 127, r1 126, measured 2026-10-06 — 19 more per body than on 2026-10-05, the non-finite-setpoint checks F282 added), and 5 more per body against a fake that reports one joint in trouble and 131 in the vendor's simulator (18/18/19/19/19/19/19, last run 2026-10-01: the simulator was not run again after F277, so its reads have not been seen with the bounded wait), 34 passed through the real ABI per body |
| **`rhal.state.rate`, `rhal.command.latency`** | The row's *"joint access at rate"*. DDS over loopback against a Python-free C++ driver is fast; a real body's channel crosses Wi-Fi or a wired link to the robot's own computer, and what that costs is a bench measurement |
| **`safety.estop.latency`** | A Unitree stop is hardware on the robot. This driver publishes a message with every gain and every torque at zero, which on an impedance drive is the definition of commanding nothing — and it is not a power cut. The simulator shows that message zeroing every drive; it cannot show a power cut, because it has none |
| **Contact, gravity, a body on the ground** | In the vendor's simulator now, for the Go2 and the B2 standing on their own feet. The humanoids hang from the simulator's elastic band: nothing here stands one up, and the gains that hold a simulated arm are numbers that worked in MuJoCo, not on a robot |

## What the descriptor declares, and what it will not

Travel per joint, from the vendor's published models, because travel is a kinematic fact about the
machine. Each envelope carries a **torque ceiling only**, cited with the hedge: 45 Nm on the Go2
(the largest motor's peak), 360 Nm on the B2 (the maker's page says "About 360N.m" and nothing about which motors, so it is a ceiling on every joint and a claim for none), 120 Nm on the G1 (the brochure's max with footnote [1]), 360 Nm on the
H1 and H1-2 (the knee group peak, with the hip/ankle/arm groups beside it). Unitree publishes no
joint velocity anywhere, so `joint_ratings` stays unmet and each envelope says so in its `source`
rather than inventing one (F64). Watchdog 20 ms, ten DDS periods by driver design.

Ratings the vendor did not publish stay undeclared per joint rather than inheriting a neighbour's:
the H1's waist, and the H1-2's waist and wrists, carry no `effort_max_nm`. Control rates likewise:
the Go2's 500 Hz loop is documented, the B2's and the humanoids' DDS rate is not, so their `rate_hz_max` is null
— the community's 500 Hz convention is not a vendor publication.

The base is `legged` and `base_pose` is **all NaN**: nothing in `LowState_` localises the body. The
IMU gives orientation and nothing gives position, and a driver that wrote the IMU quaternion into a
pose would be claiming a position it does not have (§24's rule that an unmeasured quantity is NaN and
never zero).

## The B2, and what its row is made of

`model=b2` is the Go2's wire and a different machine. Slots, joint names and roles are the Go2's: the vendor's B2 example
(`example/b2/b2_stand_example.cpp`) addresses twelve motors in the same `go2` message, `unitree_mujoco`'s `b2.xml` lists its twelve
actuators in the Go2's leg order (front-right, front-left, rear-right, rear-left), and `sim_check` holds every slot to the actuator of
the same name in that file. What is the B2's own is the travel (the
vendor's `b2.xml`: hip ±0.87, thigh -0.94 to 4.69, calf -2.82 to -0.43 on all four legs) and the rating. The simulator's `ctrlrange`
(±200 on a hip or thigh, ±300 on a calf) is a CAD value for a simulated actuator and is not used as a rating (§63.4); the page's
"About 360N.m" is, as a ceiling, with its lack of a footnote said in the envelope's `source`.

The row was found wrong once before it was right: the envelope quotes the maker's words, and the descriptor builder put the string between
JSON quotes as it stood, so the first B2 descriptor was not JSON. The driver's own suite passed all 90 of its checks and the conformance run
refused it at the closing quote (`describe() wrote 3117 bytes and they do not parse`). The string now goes through `json_escaped`, the
suite pins the escaped quotation (`Expect::source_quote`), and the seeded defect that puts the raw string back is killed by that pin
(`uni-b2-source-unescaped`). The operator's `robot_slug` is the other text a descriptor takes from outside and is not escaped: the config
reader (`config_string`) has no notion of an escape either, so a slug that needed one cannot arrive in the first place.

Not built: the wheeled B2-W and Go2-W (a wheel is a joint with no travel, and the role grammar has no word for it yet), the A2 (the SDK has no
low-level example for it, only sport and audio, so its wire has no vendor source to be held to) and the R1's other two sizes (below).

## The H2, and what its row is made of

`model=h2` is the first body with thirty-one joints and the first whose vendor example gives **every joint its own gains**
(`example/h2/low_level/h2_ankle_swing_example.cpp`'s `Kp` and `Kd`); the driver takes one pair, so the simulator run uses the pair for the
joint it moves (the left shoulder roll, 60 and 1). Slots are the example's `H2JointIndex`, contiguous 0 to 30, and `h2_mujoco.xml` lists its
actuators in the same order, which `sim_check` holds the driver to by name. **The ankle is roll at slots 4 and 10 and pitch at 5 and 11, the other
way round from the H1-2's**, which is the mistake a table copied from a neighbour makes (`uni-h2-ankle-names-in-the-h1-2s-order` seeds it; the suite cannot see a name table, the simulator can). Roles for the waist and head follow the codebase's forms (`torso.waist.roll`,
`neck.neck.pitch`). Travel is the vendor's `h2_mujoco.xml`.

Ratings are the page's two figures, "Maximum Torque of Leg Joint [1]: 360N.m" and "Maximum Torque of Arm Joint [1]: 120N.m", with footnote [1] ("may
vary in different scenarios and configurations") quoted in the envelope's source, and taken no further than the H1-2's group figures are: 360 on the
hips and knees, 120 on the shoulders and elbows, nothing on the ankles, the wrists, the waist or the head. The vendor's model gives the ankles about a
fifth of the leg figure; that is a CAD value and not a rating (§63.4), but it is a reason not to let 360 stand for a joint a clamp would then wave through.

Two things in the simulator check had to learn about a heavier arm, and neither is the driver's: the "went where it was sent" bound was a flat 0.15 rad,
and the H2's shoulder stood 0.21 rad off its setpoint under 12.9 N·m at kp 60, which is the drive's own law (load over stiffness) and not a motion that
failed. The bound is now the larger of 0.15 and the measured load over `kp` plus 0.05, printed in the check, so no lighter body is held to less; and the
arm is given three seconds to settle before the hold is measured where the others get one, because at the vendor's gains it was still settling (0.056 rad
in the hold, 0.001 once given the time).

## The R1, and the three things its row says it does not have

`model=r1` is **the standard R1**: the 26 motors of `example/r1/low_level/r1_ankle_swing_example.cpp` (`R1_NUM_MOTOR`), whose `joint_idx_in_idl` puts them in the
`hg` message at 0 to 13, 15 to 19, 22 to 26, 29 and 30. The unused slots (14, 20, 21, 27, 28) are the 29-joint layout's waist pitch and wrist pitch and yaw, and the
vendor's model `R1_C++.xml` carries them as placeholder joints; the driver never addresses them, as it never addresses the G1's absent ones. The page also sells
a 20-joint **AIR** (no waist, no head, four-joint arms) and an **EDU** of 26 to 40: other bodies, not built. The ankle is pitch then roll (the H1-2's order and not
the H2's), the waist is **roll then yaw**, and the arms have five joints.

What the row **declares nothing** about, each said in the descriptor and not filled:

* **A peak torque and every joint's rating.** The page's "Maximum Torque of Arm Joint" line reads "About 2kg", which is a load, with a footnote that the arm's
  maximum load varies greatly with posture. The envelope's `joint_torque_max_nm` is `null` and the quote is in its `source`; no joint carries `effort_max_nm`.
  (The builder used to print a number for the envelope's peak whatever it was given; it prints `null` now when there is none, as it already did for the rate.)
* **The head's travel.** The vendor's model has no head joint and nothing else here gives a range, so `head_pitch` and `head_yaw` carry a `limits` block with no
  position bound at all (`NO_TRAVEL`, a NaN the descriptor leaves out), and the suite asserts that (`Expect::travel_omitted`).
* **A rate.** Unpublished, as for the humanoids.

The simulator cannot hold the head either: `sim_check` excuses a joint from its by-name slot check only if the body's row lists it as unmodelled **and** its slot is
beyond the model's actuators (`Body::unmodelled`, the R1's two head joints), prints a note for each, and says nothing about them below. So two joints of the 26
are checked by neither the suite nor the simulator for their names, and `uni-r1-head-names-swapped` is a control that says so. The other 24 are held to
`R1_C++.xml`'s actuators by name, the waist's roll-then-yaw order included.

## Licences

This directory is Apache-2.0. It links `unitree_sdk2` (BSD-3-Clause) and CycloneDDS (EPL-2.0 /
EDL-1.0); neither is vendored here and `make` refuses rather than building half of itself when the
SDK is not named.
