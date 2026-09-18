# The bench runbook

§15's second suite as a procedure: the sixteen bench tests a Host award requires, per body,
with the instrumentation each one needs named. Phase 6, item 6.7.

**Status: unrun.** Every procedure below is written, none has been executed — there is no robot
on this bench and no PREEMPT_RT kernel under it. The first execution will find defects in the
runbook itself, and finding them is part of the session: a step that cannot be performed as
written is a runbook defect, recorded in the result's `detail`, not an invitation to improvise
a passing measurement.

## What this is, and what it is not

This is the operator's half of §15: for each test, the preconditions (which declarations must
exist), the setup, the steps, what to record, and the instrument. It is not evidence, it does
not award anything, and a body that has merely read it is exactly where it was. The awarding
party is the Hub (§15.1), from records a person witnessed — `skynet bench record` binds each
result to the bytes it was taken against, and a rebuilt driver voids the record by arithmetic
rather than by anybody remembering.

Three bench tests are **not** in this runbook: `embodiment.limits.honest`,
`safety.estop.independent` and `telemetry.blackbox.survives` are required from `certified`,
and Certified is out of this phase's scope by the plan's own rule. They are named here so
that sixteen does not read as nineteen.

Two readings the runbook applies throughout, because §15.1 states them once for all tests:

* **Absence is not a pass.** A test that cannot run — no declaration to measure against, no
  instrument on the bench — is `missing`, never `skipped`, never absent from the record.
* **`not_applicable` passes, and only for shape.** A body with no sensors has no per-sensor
  latency to measure; a body that needs no homing has no homing envelope to watch. Genuine
  inapplicability is a declared property of the body, not a gap in the bench. Each test below
  states its `not_applicable` reading where one exists, and where none exists there is none.

## The sixteen tests

| # | Test | Quant | Unit |
|---|---|---|---|
| 1 | `rhal.state.rate` | yes | Hz |
| 2 | `rhal.command.latency` | yes | ms |
| 3 | `rhal.sensor.latency` | yes | ms |
| 4 | `safety.watchdog.fires` | yes | ms |
| 5 | `safety.envelope.clamps` | no | — |
| 6 | `safety.estop.latency` | yes | ms |
| 7 | `safety.ratelimit.enforced` | no | — |
| 8 | `safety.command_loss.behaviour` | no | — |
| 9 | `safety.homing.envelope` | no | — |
| 10 | `package.budget.enforced` | no | — |
| 11 | `ota.rollback.works` | no | — |
| 12 | `ota.safe_state.respected` | no | — |
| 13 | `telemetry.blackbox.complete` | no | — |
| 14 | `offline.control.independent` | no | — |
| 15 | `offline.reconcile.resumes` | no | — |
| 16 | `power.safe_state.reachable` | yes | s |

The order below is the run order, and it is not numeric: the week-long test goes first, the
destructive-adjacent tests go late, and the tests that share a rig run together. §15 prices
the suite at a human, a robot and a day; the week in test 15 is the exception §15 prices
without naming, and the runbook refuses to compress it (see test 15).

## Session setup — once, before test 1

**The bytes.** List every artefact the session runs against — the driver shared object, the
kernel image, the OS image under test — by path, and hash them. Every `skynet bench record`
invocation takes the same artefact list; `--append` refuses a result taken against other
bytes, which is what makes a mid-session rebuild visible rather than silent. Rebuild anything
and the session restarts: a pass against other bytes is `BenchPassStale`, not a pass.

**The witness.** One person, named, present for every test except where the test says a
machine may witness. The name goes in `--human` on every record. Unattended is not
unwitnessed: where a procedure says the robot acts on its own, the witness watches without
touching, and touching voids the test.

**The rig every test shares.** The robot on the bench it will be measured on, the joint
feedback stream logged at the highest rate the body offers (per-body rates are in the
appendix), and a wall clock the log timestamps come from. Three of the sixteen tests are
explicit that the measurement is taken **at the joint rather than reported by the driver**
(§15's mantra); the rest take the joint feedback stream as their instrument, and each test
names what that stream is on each body so the provenance is on the record rather than
assumed.

**Read the resolved limits first.** `skynet limits` on the robot prints the ceilings the
kernel would enforce and which guards lack an input. Any test whose precondition names a
ceiling the robot reports missing stops before it starts: measuring against limits that are
not in force is rehearsal, not evidence.

---

## Test 15 (first): `offline.reconcile.resumes`

> *After a week offline the robot reconciles to desired state and uploads its backlog without
> operator action.*

**Why first.** It takes a calendar week and no bench time: sever the network on day one, run
the other fifteen, reconnect on day eight. Compressing the week — shorter outage, simulated
clock — voids the test, because the backlog it measures is a week's worth of sealed windows
and queued check-ins, and a smaller backlog is an easier test wearing the same id.

**Precondition.** A Hub the robot is paired with (§10), a desired state newer than the
robot's generation (so there is something to reconcile *to*), and a week of ordinary
operation with the network removed — the robot runs its tasks, seals its windows, queues its
uploads, and nobody touches it.

**Steps.** 1. Record the robot's generation and the Hub's desired generation. 2. Remove the
network (unplug, not firewall rules a process could re-add — the absence must be physical).
3. Leave it a week. The robot operates; the witness does not intervene. 4. Reconnect. 5.
Observe, without acting: the robot pulls desired state, applies it, drains its backlog, and
its next check-in carries the applied generation.

**Record.** `pass` when the check-in after reconnection shows the desired generation applied
and the Hub holds the week's sealed windows. `fail` when anything needed a hand — a retry
that required a reboot, a backlog item dropped silently, a generation adopted without being
applied (§17's refusal exists for exactly this). The `detail` carries the outage window and
the backlog size.

**Instrumentation.** The Hub's device page (generations, received windows) and the robot's
`skynet plan` (the reconcile pass, and anything deferred). No body-side instrument: this
test measures the platform, and it is the same procedure on all six bodies.

**Per body.** Identical — with one note for the bus bodies (`canopen`, `ethercat`): their
"ordinary operation" for the week is whatever the bench rig commands, and the rig's command
log is part of the record, so that a week of idleness cannot present as a week of operation.

---

## Test 14: `offline.control.independent`

> *The network is removed mid-task. The robot completes the task. No control decision may
> require a round trip.*

**Precondition.** A task the body can complete in minutes (per body, below) and a network the
robot is joined to — joined, not needed. This is the one bench test required from `guest`,
and the one *Decisions* 4 permits a machine to witness: it needs a real machine, not a
watched body. A human witness is still accepted and still recorded as one.

**Steps.** 1. Start the task with the network up. 2. Mid-task, remove the network physically.
3. The robot completes the task. 4. Restore the network.

**Record.** `pass` on completion with no control pause attributable to the network. The
black box of the window is the corroboration: no gap, no deferred decision, no policy
output missing where a round trip would have been. `fail` on any stall, abort, or safe-stop
that traces to the outage.

**Instrumentation.** The task's own completion signal plus the sealed window (`skynet
blackbox seal`, `skynet blackbox inspect` for continuity). The network removal is a hand on
a cable, witnessed and timed.

**Per body.** The task is the body's ordinary motion at its ordinary rate: the arms
(`franka`, `ur`) run a joint-space traverse mid-travel; the Go2 stands and weight-shifts
(see test 4's safety note — no locomotion on the bench); the bus bodies (`canopen`,
`ethercat`) run their axes through a scripted profile; `ros2_control` runs whatever the
URDF's body does ordinarily. The task is recorded in `detail`: a test whose task is unknown
is a pass nobody can weigh.

---

## Test 13: `telemetry.blackbox.complete`

> *Every cycle in a ten-minute run appears in the black box with observation hash, policy
> output, kernel verdict and final command.*

**Precondition.** A policy that posts — a window in which the policy never posts records
idle cycles, and replaying one found the watchdog tripping on a sequence that never advanced
(which is a true statement about an empty bench, not evidence about a robot). The run must
command the body for the full ten minutes.

**Steps.** 1. Start the recorder and the commanding policy together. 2. Ten minutes of
commanded motion (per-body motion below — ordinary, mid-travel, unhurried). 3. Seal the
window. 4. Inspect: `skynet blackbox inspect` for provenance and continuity, and assemble
the cycles to confirm the four fields on each.

**Record.** `pass` when every cycle of the ten minutes is present with all four fields and
`inspect` reports continuity with no gaps. `fail` on any gap, any cycle missing a field, or
a seal that does not verify. The `detail` carries the cycle count and the seal digest.

**Instrumentation.** `skynet blackbox seal` and `skynet blackbox inspect` on the sealed
file — no robot involved in the reading, which is the point: the machine that produced the
recording is the machine that is broken when somebody asks about it.

**Per body.** Identical procedure; the motion differs only in what "commanded" means: the
arms track a slow trajectory; the Go2 runs impedance holds with shifting setpoints (no
locomotion); the bus bodies follow a profile; `ros2_control` follows the URDF body's
ordinary commanding. The policy digest goes in `detail` — replay is against the same digest
or it is not replay.

---

## Test 1: `rhal.state.rate`

> *Sustained state-read rate over ten minutes, and the worst-case gap between two successive
> reads.*

**Precondition.** Declared `control.rate_hz_min` / `rate_hz_max` (all six bodies declare
these; the appendix lists them). The test measures the driver's read rate under full demand,
not the robot's stream rate under politeness.

**Steps.** 1. Open the driver against the robot. 2. Call `read_state` back-to-back for ten
minutes — no throttle, no sleep — logging `(seq, stamp_ns, arrival_ns)` per return, where
`arrival_ns` is the reader's own `CLOCK_MONOTONIC`. 3. Compute sustained rate (returns per
600 s) and the worst arrival gap.

**Record.** The `value` is the sustained rate in Hz; the worst gap in ms goes in `detail`
with the declared rate bounds. `pass` when the sustained rate meets the declared minimum
*and* the worst gap stays under the declared watchdog window — the derivation being that a
gap past the watchdog window is a trip the robot would take on its own stream, so a driver
that cannot read faster than its own watchdog cannot hold the tier. Where the Hub weighs
differently, the numbers are on the record to weigh.

**Instrumentation.** The read loop and its log. No external instrument exists for this test
and none is needed: the reader's clock is the measurement, and the log is the evidence.

**Per body.** Franka declares 1000/1000 — the loop must sustain a kilohertz for ten minutes,
and this is the test §31.3's language ceiling is about (a GC pause is a gap). UR declares
1–125: the minimum is loose enough to pass trivially, which the `detail` must not hide —
record the sustained figure honestly and let the Hub see a body whose floor is 1 Hz. Unitree
50–500 over DDS (wired for the session; Wi-Fi jitter is a measurement of the room, not the
robot). CANopen 1–1000 and EtherCAT 50–1000 at the bus rate the rig runs. `ros2_control` at
whatever the controller manager's update rate is, recorded in `detail`.

---

## Test 2: `rhal.command.latency`

> *Time from a command accepted at the RHAL boundary to measurable joint motion. The number
> the descriptor must declare.*

**Precondition.** Declared `control.command_latency_ms` — which **no body declares**. Every
body's session therefore starts with a provisional figure from engineering judgment, run as
the number under test: the run measures against the provisional, the vendor sets the
declaration from the measurement, and the recorded pass compares the run with the figure in
force during it. The provisional and its basis go in `detail`; a declaration that merely
repeats the measurement with no margin is the vendor's to defend, not the runbook's to set.

**Steps.** 1. Static baseline: log the target joint's feedback for 30 s at rest; the noise
floor is the largest excursion. 2. Issue a single joint command (per body, below) and stamp
its acceptance at the RHAL boundary. 3. Sample state at the body's maximum rate until the
joint moves past the noise floor. 4. Latency is acceptance to first past-floor sample. 5.
Repeat ten times; record the worst.

**Record.** The `value` is the worst of ten in ms. `pass` when the worst stays within the
declared (provisional, first session) figure.

**Instrumentation.** The joint feedback stream at full rate, per body: FCI `RobotState` at
1 kHz; RTDE `actual_q` at the negotiated rate; Go2 `LowState_` over DDS; CiA 402 object
`6064h` (position actual) sampled each PDO cycle; `ros2_control` joint states at the
controller rate. The acceptance stamp is the driver's: the runbook notes honestly that no
independent tap exists at the RHAL boundary on these bodies, and the feedback path's
provenance is the instrument's calibration.

**Per body.** One joint, mid-travel, against no load the body was not built for: Franka
joint 4 (elbow) 0.1 rad; UR wrist 0.1 rad at the controller's rate; Go2 one knee joint,
standing braced, 0.05 rad; bus bodies one axis, 5% of travel; `ros2_control` one joint of
the URDF body, 5% of its travel. Torque-mode bodies are measured in the mode the session
commands — the mode goes in `detail`, because latency through an impedance loop is not
latency through a position loop.

---

## Test 3: `rhal.sensor.latency`

> *Per sensor: time from the observed event to the timestamp the frame carries.*

**Not applicable, on all six bodies.** No fleet body declares a sensor, so there is no
per-sensor latency to measure — `not_applicable` with the reason recorded, which §15.1
counts as a pass. Two sentences of honesty accompany it: first, any body carrying
sensors its descriptor does not declare is a descriptor gap the test cannot measure its
way around — declaring them turns this test from `not_applicable` to `missing`, which
is the direction of more evidence rather than less, and the bench session verifies the
no-sensors claim against the hardware rather than trusting the descriptor about it.
Second, when a body does declare one, the procedure is: a timestamped stimulus (an LED
on a GPIO the bench drives, imaged by the sensor), ten trials, worst case recorded —
stamping arrival rather than capture fails, because a queue delay folded into a
timestamp is a number the policy treats as ground truth (§47.2).

---

## Test 4: `safety.watchdog.fires`

> *Commands are stopped mid-motion; time until the robot reaches a controlled stop, against
> the declared watchdog window.*

**Precondition.** Declared `envelope.watchdog_ms`: Franka 10, UR5e 80, Go2 20. CANopen,
EtherCAT, undeclared-model UR and `ros2_control` declare no envelope and therefore no
window — their sessions stop here with `missing` until the declaration exists, because
there is no window to measure against and inventing one at the bench is designing, not
measuring.

**Safety precondition, and it outranks the procedure.** Mid-motion silence on a legged
robot is a fall unless the robot is braced; on an arm it is a stop from speed. The Go2
runs this test standing, braced, moving one joint against gravity at walking pace — no
locomotion, no unbraced stance. The arms run mid-travel at 10% of rated velocity. The
witness holds the hardware stop for the whole test on every body. A session that cannot
meet these conditions does not run the test; it records `missing` with the reason.

**Steps.** 1. Start the per-body motion. 2. At a marked cycle, stop the command stream —
silence, not estop: close the commanding handle without any stop message, because the
watchdog is what answers silence and the test is whether it does. 3. Timestamp the cutoff
at the last command's send. 4. Watch the joint feedback until velocity is zero and stays
zero for one second: that is the controlled stop. 5. Time is cutoff to stop.

**Record.** The `value` in ms. `pass` when the stop lands inside the declared window with
the robot settling rather than tripping — a window the robot meets by faulting is a window
met by the wrong mechanism, and the `detail` says which mechanism stopped the robot.

**Instrumentation.** The command-stream cutoff marker (the commanding rig's log) and the
joint feedback trace at full rate. The nesting (§46.3) is the corroboration: the kernel's
worst case, the envelope window, the SoC window and the drive timeout must nest with the
measured stop inside the envelope window, and the four figures go in `detail`.

**Per body.** Franka: elbow drift at 0.26 rad/s, ten-millisecond window — the tightest in
the fleet, and the one most likely to fail honestly. UR: wrist drift, 80 ms window, at the
controller's rate. Go2: knee flexion against gravity, braced, 20 ms window. The bus bodies
and `ros2_control`: `missing` pending the declaration.

---

## Test 6: `safety.estop.latency`

> *Time from estop assertion to zero commanded torque. Measured at the actuator; a driver
> reporting success is not evidence.*

**Precondition.** The declared software stop path — `safety.estop` is declared on all six
bodies, and this test measures exactly that path, not the hardware button (which is
`safety.estop.independent`'s, certified, out of this runbook). Know which path that is per
body before the session: Franka `StopMove` over TCP; UR dashboard stop plus the cleared
gate; Go2 zero-gain zero-torque DDS message; CANopen and EtherCAT CiA 402 quick-stop
controlwords; `ros2_control` a pure latch — it refuses later setpoints and calls no
hardware, because deactivating components allocates and locks and is unsafe from a
signal handler. The latch-only stop is the honest extreme of what this test measures:
assertion to zero commanded torque where the driver commands nothing further.

**Safety precondition.** Same brace and speed rules as test 4 — mid-motion assertion from
10% velocity, hardware stop in the witness's hand, Go2 braced.

**Steps.** 1. Start the per-body motion. 2. Assert the software stop and stamp the
assertion. 3. Watch the last mile — the commands the driver emits after the assertion —
until commanded torque is zero, then watch the actuator until motion ceases. The measured
figure is assertion to zero *commanded* torque; the actuator's settling goes in `detail`
as corroboration, because a command of zero the actuator ignores is a finding, not a pass.

**Record.** The `value` in ms. `pass` when commanded torque reaches zero — there is no
declared figure to compare against (no body declares a stop latency), so the number stands
on its own for the Hub to weigh, with the mechanism named in `detail`.

**Instrumentation.** A tap on the driver's command output (the last mile: FCI
`RobotCommand` stream, RTDE setpoints, DDS `LowCmd`, PDO setpoints, controller commands)
plus the actuator-side trace: Franka link-side torque (measured, not reported — the one
body where the distinction is cheap), motor current where the drive exposes it, joint
velocity everywhere. The driver's return code is not an instrument.

**Per body.** As the stop paths above. Franka's assertion is one 12-byte send with no
reply waited for — the fastest path in the fleet to assert and the one whose latency is
most purely the box's. The Go2's is a DDS publish — the session runs wired, for test 1's
reason.

---

## Test 5: `safety.envelope.clamps`

> *A command at 150% of a joint rating is issued; the joint never exceeds its rating,
> measured at the joint rather than reported by the driver.*

**Precondition.** Declared joint ratings for the rating under test: Franka (both) and UR5e
(both) may run; the Go2 may run the torque half only (F64 — no published velocity, so the
velocity half is `missing`, not improvised); CANopen, EtherCAT, undeclared-model UR and
`ros2_control` are `missing` for both halves until their envelopes exist.

**Safety precondition.** 150% of a rating is asked of the kernel, not of the hardware —
the test passes when the joint *never sees it*. The witness holds the hardware stop, the
joint under test is mid-travel with room to settle in both directions, and the session
commands one joint at a time. If the joint exceeds its rating at any sample the test fails
*and stops*: the point is proven and further samples are risk without information.

**Steps.** 1. Pick the joint and the rating (per body, below). 2. Command 150% of the
rating — a velocity demand at 1.5× rated max, or a torque demand at 1.5× rated max — for
three seconds. 3. Sample the joint at full rate throughout. 4. The joint's measured peak
must not exceed the rating.

**Record.** Boolean. `pass` when no sample exceeds the rating in either half the body ran;
`fail` with the peak and the sample index when one does. The `detail` names the joint, the
rating, the halves run, and the peak measured.

**Instrumentation.** The joint feedback stream at full rate — and the runbook states the
gap plainly: on these bodies the highest-integrity joint measurement arrives *through the
driver*, so "measured at the joint" is satisfied by the feedback's provenance (link-side
torque on Franka, drive-reported velocity elsewhere) rather than by an independent
instrument. An external encoder or current probe where the bench has one strengthens the
record and goes in `detail`; its absence does not fail the test, but the record says it
was absent.

**Per body.** Franka: elbow velocity (1.5 × 2.62 rad/s demanded, 2.62 the ceiling) and
elbow torque (1.5 × 12 Nm, link-side measured). UR5e: wrist velocity and torque at 1.5×
the tech-sheet ratings. Go2: knee torque at 1.5 × 45 Nm, braced, velocity half `missing`.
One joint each; the appendix names the ratings.

---

## Test 7: `safety.ratelimit.enforced`

> *A single-cycle step from zero to full scale produces a ramp at the configured jerk
> ceiling, not a step.*

**Precondition.** A configured jerk ceiling, read off the robot (`skynet limits`) before
the session and recorded in `detail`. No ceiling in force, no test — an unconfigured
rate limiter cannot be observed to work.

**Steps.** 1. At rest, mid-travel, issue a single-cycle step from zero to full scale on
one joint (per body: what full scale means). 2. Sample the joint at full rate. 3. The
trace must show a ramp at the configured jerk ceiling — finite slope, matching the
ceiling within the sampling resolution — and never a step between two adjacent samples
larger than the ceiling permits.

**Record.** Boolean. `pass` on a ramp at the ceiling; `fail` on a step, with the two
samples quoted in `detail`.

**Instrumentation.** The joint feedback trace at the body's maximum rate. The ceiling
comes from the resolved limits, not from memory: print it, attach it, run.

**Per body.** Franka elbow: full scale is rated velocity (2.62 rad/s) demanded in one
cycle. UR wrist: the tech-sheet rating. Go2 knee: torque mode is not the test — velocity
to the (undeclared) rating cannot run, so the Go2 runs the position half only: a step to
5% of travel in one cycle, ramp observed. Bus bodies: one axis, 5% of travel in one
cycle. `ros2_control`: one joint, same shape. The position-half reading is the runbook's
where velocity ratings are absent, and it says so on each record.

---

## Test 8: `safety.command_loss.behaviour`

> *Commands stop and the drives are observed at the joint. What they do must be what the
> body declares.*

**Precondition.** Declared `on_command_loss` — "hold" | "brake" | "fault" | "coast" —
which **no body declares**. Every body's session starts by declaring it (a hardware
property, established by this test, not before it — the first run observes, the vendor
declares from the observation, the recorded pass compares a later run with the
declaration). A body observed to coast may not claim host (§46.2: the watchdog chain's
own recovery is, on that machine, a fall) — the test can still record the observation,
but the award caps below host whatever else passes.

**Steps.** 1. Mid-motion (test 4's motions and safety rules). 2. Stop the command stream
— silence, as in test 4. 3. Observe **every chain** at the joint: the measures text is
explicit that one leg is not evidence about an arm. 4. Classify each chain: hold (pose
kept, drives live), brake (brakes engaged, verified by the brake state where readable),
fault (drives disabled with a fault the driver surfaces), coast (unpowered drift).

**Record.** Boolean. `pass` when every chain does what the body declares (second run
onward). The `detail` classifies each chain separately with its trace reference. A body
declaring per-chain behaviour (`on_command_loss_by_chain`, §87.3) records per chain; a
body declaring both spellings is refused before measurement.

**Instrumentation.** Joint feedback per chain at full rate, plus the brake/fault state
where the body exposes it (CiA 402 statusword on the bus bodies; reflex bits on Franka;
controller state on UR; Go2 motor states). The classification is the witness's, from the
traces, named per chain.

**Per body.** The arms are single-chain bodies: all joints observed, one classification
(if a UR's wrist coasts while its shoulder holds, that is two classifications and a
per-chain declaration). The Go2 is four leg chains plus the body: each leg classified.
The bus bodies classify per axis. `ros2_control` classifies per the URDF body's chains.

---

## Test 9: `safety.homing.envelope`

> *The driver-provided homing procedure is run and the joints are watched. No joint
> exceeds the restricted homing envelope the descriptor declares (§48.3).*

**Precondition.** A homing declaration — which **no body makes**. Franka's joints are
absolute encoders with nothing to home: its declaration is needs-no-homing, and this
test is then `not_applicable` with that reason. Every other body must declare (needs
homing with a restricted envelope and a driver procedure, or needs none) before its
session; until it does, `missing`.

**Steps (bodies that home).** 1. Park the body. 2. Confirm with the operator where the
homing sweep travels — required on any body whose sweep can leave the parked volume
(§48.3), witnessed and minuted. 3. Invoke the driver homing procedure via the RHAL
extension channel. 4. Watch every joint against the restricted envelope (typically a
small fraction of rated velocity) for the whole procedure. 5. Confirm the joint-zero
calibration record is written (§45.2).

**Record.** Boolean, or `not_applicable` for needs-no-homing bodies. `pass` when no
joint leaves the restricted envelope and calibration lands. `fail` with the joint,
the sample and the envelope figure when one does.

**Instrumentation.** The joint trace overlaid with the restricted envelope, plus the
calibration record. The envelope under test is the *homing* envelope, not the motion
one — the runbook states it because the temptation is to watch the wrong lines.

**Per body.** Franka: `not_applicable` (absolute encoders), once declared. UR: per the
vendor's homing need — the controller homes on boot, and whether that counts as the
driver procedure is the vendor's declaration to make. Go2: per Unitree (joint zeros at
stand). Bus bodies and `ros2_control`: per drive / per URDF body. The declarations do
not exist today; the runbook does not write them.

---

## Test 10: `package.budget.enforced`

> *A package exceeding its declared memory budget is stopped rather than allowed to
> exhaust the robot.*

**Precondition.** The probe package: a `.sky` package declaring a small
`memory_max_mb` (64 MB) whose whole behaviour is to map past it — 64 MB declared, 256
MB touched, in 16 MB steps with a second between steps so the stop is attributable to
a step rather than to the attempt. The probe is built once, signed for the bench, and
its digest goes in every record that uses it.

**Steps.** 1. Install and start the probe beside a canary package (a trivial publisher
whose output proves the robot is otherwise healthy). 2. Let the probe allocate past its
budget. 3. Observe: the probe is stopped (cgroup `memory.max`, §40.1), the canary
publishes throughout, the robot stays up. 4. Confirm the stop is recorded (the
supervisor's account, not the probe's word — the probe is dead and testifies to
nothing).

**Record.** Boolean. `pass` when the probe dies contained: stopped at its budget, no
other package disturbed, no reboot. `fail` when the robot exhausts (canary gaps, OOM
outside the cgroup, any reboot) or when the probe survives past its budget.

**Instrumentation.** The supervisor's budget accounting and health (§40.1: cgroup
events), the canary's output log, and the robot's uptime across the test. The probe's
own log is not evidence.

**Per body.** Identical — this test measures the platform, not the body. The one
per-body sentence: on the Go2 the bench runs against the onboard computer (the
`platform.onboard_compute` body), because a budget enforced on a workstation proves
nothing about the robot.

---

## Test 11: `ota.rollback.works`

> *A deliberately broken image is staged; the robot boots it, fails its health check,
> and returns to the previous slot unattended.*

**Precondition.** RAUC A/B slots on the robot with a known-good image in the current
slot, and a deliberately broken image (same manifest shape, a kernel that panics on
boot — broken loudly, not subtly). Both digests recorded.

**Steps.** 1. Stage the broken image to the standby slot. 2. Reboot into it. 3. Watch
without touching: the new slot fails its health check within the boot budget, and the
bootloader returns to the previous slot. 4. Confirm the robot is back on the known-good
slot (`rauc status`, the generation on the wire, `skynet status` answering) — with no
hand laid on it between step 2 and this confirmation.

**Record.** Boolean. `pass` on unattended return to the previous slot with the robot
healthy. `fail` on any intervention, any boot loop, any return that needed a cable —
and on a broken image that *passes* its health check, which is the health check lying
rather than the rollback working.

**Instrumentation.** The boot log (serial where the board has it, RAUC status
otherwise), the slot table before and after, and the witness's minute-by-minute log of
not touching anything. The witness's inaction is the measurement; minute it.

**Per body.** Identical — the platform under test, not the body. The health check under
test is the robot's own (RAUC boot check plus `skynetd` reaching ready); its definition
goes in `detail` because a rollback gated on a check nobody wrote down is a pass
against an unknown bar.

---

## Test 12: `ota.safe_state.respected`

> *An update issued mid-task is staged and not applied until the robot reaches a
> declared safe state.*

**Precondition.** A declared safe state per body (below) and a staged update (a genuine
signed bundle, not the broken image — this test is about *when*, not *whether*).

**Steps.** 1. Start the per-body task. 2. Mid-task, issue the update. 3. Observe it
stage and wait — no apply, no reboot, no pause of the task attributable to the update.
4. Let the task reach the declared safe state. 5. Observe the apply happen there, on
the robot's own schedule.

**Record.** Boolean. `pass` when the update waits for the safe state and applies
there. `fail` on any mid-task apply, reboot, or task disturbance — and on an update
that never applies after the safe state is reached, which is staging that forgot its
purpose.

**Instrumentation.** The updater's log (staged/applied timestamps), the task log, and
the safe-state declaration the session ran against. The two timestamps with the
safe-state crossing between them are the whole evidence.

**Per body.** The task and the safe state, named: Franka — a pick-and-place traverse,
safe state parked with the gripper empty. UR — a scripted traverse, safe state the
program's end pose. Go2 — a standing weight-shift routine, safe state standing still
(no locomotion on the bench, for test 4's reason). Bus bodies — an axis profile, safe
state all axes at index. `ros2_control` — the URDF body's ordinary task, safe state as
that body declares it. "Holding something" (§12's example) is the general rule: the
safe state is where the body holds nothing and owes nothing.

---

## Test 16: `power.safe_state.reachable`

> *From full rated load, the robot is taken to its nearest safe state and timed,
> against the `time_to_safe_state_s` it declares (§36.2).*

**Precondition.** Declared `time_to_safe_state_s` — which **no body declares**. As in
test 2, the first session runs against a provisional figure: the vendor's engineering
judgment of how long the body needs, stated before the run, defended after it. The
reserve budget is computed from this figure, so a body that declares two seconds and
takes six has a third of the reserve it needs — the runbook states the arithmetic
because it is the reason the provisional must be honest rather than flattering.

**Safety precondition.** Full rated load is the most energy this test asks any body to
hold, and the safe state must be reachable *from* it. Rated payload, full extension,
controlled motion — and the hardware stop in the witness's hand, for the same reason as
tests 4 and 6.

**Steps.** 1. Load the body to full rated load (per body, below). 2. Command the
nearest safe state and stamp the command. 3. Time to arrival: the body at rest in the
safe state. 4. Repeat three times; record the worst.

**Record.** The `value` in seconds, worst of three. `pass` when the worst stays within
the declared (provisional, first session) figure.

**Instrumentation.** The joint trace (arrival is all joints at rest in the safe-state
pose) and the load manifest — what "full rated load" was, weighed or computed, in
`detail`. A load nobody wrote down is a test against an unknown bar.

**Per body.** Franka: rated payload at full extension, safe state parked. UR5e: rated
payload at full extension, safe state the program's end pose. Go2: standing with rated
payload aboard — the session runs it standing, braced, for test 4's reason — safe state
standing still. Bus bodies: rated torque held on all axes, safe state all axes at
index. `ros2_control`: the URDF body's rated load, safe state as declared. The rated
figures come from the vendor spec sheets at session time and go in `detail` with the
citation — a payload this document remembered would be a rumour, and the load manifest
is weighed, not trusted. The Go2's reading is the runbook's most
conservative, and it says so: a legged body at load is the case the reserve arithmetic
exists for, and the bench is not the place to discover the figure is wrong at speed.

---

## Appendix A: the six bodies' declared figures

Measured against `driver check --descriptor` output at Phase 6 (simulator-true
descriptors; the bench runs against the real bodies with `simulator: false`, and any
figure that differs there is re-verified, not carried over):

| Body | DoF | Modes | Rate (Hz) | Envelope | Watchdog | Latency decl | `on_command_loss` | Homing | `time_to_safe_state_s` | Sensors |
|---|---|---|---|---|---|---|---|---|---|---|
| `franka-fr3` | 7 | pos/vel/torque | 1000 | vel 2.62, tq 12 | 10 ms | — | — | — | — | 0 |
| `ur5e` (model declared) | 6 | position | 1–125 | vel π, tq 28 | 80 ms | — | — | — | — | 0 |
| `unitree-go2` | 12 | pos/vel/torque | 50–500 | tq 45 only (F64) | 20 ms | — | — | — | — | 0 |
| `canopen` axes | 3 | position | 1–1000 | null | — | — | — | — | — | 0 |
| `ethercat` segment | 3 | position | 50–1000 | null | — | — | — | — | — | 0 |
| `ros2_control` body | per URDF | union of joint modes | 1–1000 | null | — | — | — | — | — | per URDF |

**The shopping list.** Before any session runs, every body must declare
`command_latency_ms` (test 2), `on_command_loss` (test 8) and `time_to_safe_state_s`
(test 16), and a homing declaration (test 9). CANopen, EtherCAT, undeclared-model UR
and `ros2_control` additionally need envelopes before tests 4 and 5 can run. These are
declarations, not measurements — transcription and vendor engineering, weeks before the
bench, not the morning of.

## Appendix B: recording the session

One record per test, all bound to the same artefacts:

```bash
skynet bench record ./driver.so ./skynetk ./os-image \
  --what "the control loop and its driver" \
  --test safety.estop.latency --outcome pass --value 3.2 \
  --detail "wrist drift at 10% rated; assertion to zero commanded torque; FCI command tap + link-side torque" \
  --human "A. Operator" --out bench/estop.json

skynet bench record ./driver.so ./skynetk ./os-image \
  --what "the control loop and its driver" \
  --test rhal.sensor.latency --outcome not_applicable \
  --detail "body declares no sensors" \
  --human "A. Operator" --out bench/sensors.json --append
```

`--append` refuses bytes that moved: a mid-session rebuild voids the session by
arithmetic. Assemble the award with the sim halves when all sixteen are recorded:

```bash
skynet driver check ./driver.so --results bench/sim-driver.json
skynet conformance --results bench/sim-runtime.json   # on the robot
skynet bench award bench/*.json --claim host
```

## What the first execution owes back

Three things, all written down before the bench is struck:

1. **Runbook defects.** Every step that could not be performed as written — a missing
   tap, a mode the body has no name for, a precondition no declaration satisfies — is a
   defect in this document, filed against it with the test id. The runbook's second
   edition is written from these, not from this session's memory.
2. **Instrument gaps.** Every place the record says "no independent instrument exists"
   is a bench to build or a tap to add. Tests 2, 5 and 6 name the gaps; the session
   confirms or closes them.
3. **The provisional declarations.** Tests 2 and 16 ran against provisional figures.
   The vendors set the declarations from the measurements, with the basis stated, and
   the records say which runs were provisional — a declaration set from the run it
   passes is the vendor's to defend, and the defence is on the record.
