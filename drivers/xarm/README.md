# `drivers/xarm` — a UFACTORY xArm over the controller's own wire

`DRY-COVERAGE-PLAN.md` Phase 10 item 10.5: *"xArm on UFACTORY's controller simulator — a driver first,
then its substrate."* No xArm driver existed in this tree. UFACTORY publish what one needs: their
Python SDK's **source** (BSD-3-Clause), and a simulator that runs the controller's own firmware. So
the wire in this driver was read out of the vendor's files, and it is measured against the vendor's
controller — F118's rung 1, the maker's controller software on the other end of the wire.

```bash
make selftest          # 98 checks: the wire and every refusal, against a simulated controller
make check             # and the sim half of SCTS through the real ABI (34 passed)
make check-sim-docker  # all of it against UFACTORY's controller firmware, and the vendor's own client
make check-sim-models  # and again for each of the five arms the firmware image carries
make check-layout XARM_SDK=/path/to/xArm-Python-SDK
```

## What is here

| file | what it is |
|---|---|
| `skynet_xarm/frames.py` | the wire: ports, registers, the envelope, the report layouts — every constant with its vendor file beside it |
| `skynet_xarm/bridge.py` | the driver |
| `skynet_xarm/controller.py` | a simulated controller written here — the dry substrate, with the behaviour measured on the firmware written into it |
| `selftest.py` | the driver's own suite, and its `--live` block for a real controller's firmware |
| `layout_check.py` | this tree's constants against the vendor's own SDK source — **42 checks** |
| `sim/` | the client image and the run script `make check-sim-docker` uses beside UFACTORY's simulator |

## The evidence, and why it is not one fake agreeing with one driver

Three independent measurements, and they fail in different ways:

* **`layout_check`** opens the vendor's SDK and requires each constant to still be what the driver
  transcribed: the three ports, all nine registers, the protocol identifier, the bits of a reply's
  state byte, the bytes `BytesData.from_fp32_list` makes for a `MOVE_SERVOJ`, and the offset ctypes
  computes for every field of the report structures the driver parses. A request is compared **byte for
  byte** with what the vendor's `send_modbus_request` writes.
* **`selftest`** — the driver against a controller written in this repository. Almost every check is a
  refusal beside the input that succeeds.
* **`--live`** — the same driver against **UFACTORY's own controller firmware** (`xarm6-type6-controller`
  2.4.0, in `danielwang123321/uf-ubuntu-docker`, the image UFACTORY's own support article names),
  with the vendor's `XArmAPI` reading the same controller as a second opinion on every state the driver
  claims: it agrees on the position and on the stop's state 4.

## What is read off the controller

**Nothing about the arm is typed into a configuration file but what a person must assert.** The rich
report (port 30002) says which arm it is (`robot_type`, `robot_axis`), the firmware version, the joint
ranges it holds, its joint speed maximum, its error and warning codes, which motors are on, and — the
field a CANopen drive has no way to give — **whether the controller says it is a simulation**. The
descriptor's `simulator` is that field, and a configuration can only make it more cautious.

Travel is the controller's own configured range. The firmware rounds what it reports to a milliradian
(F176): joint two's minimum reads −2.059 rad where the vendor's table says −2.059488, and joint three's
reads −3.927 where the table says −3.92699, ten microradians *wider*. The suite holds the two to
agreement within a milliradian, and the arm's own table is the reference, not a source.

Joint **names and roles** are configuration, because a role is a human's assertion (§63.4):

```json
{"host": "192.168.1.100",
 "joints": [{"name": "joint1", "role": "right_arm.shoulder.yaw"}, …]}
```

`host` is required — there is no scan, and a guessed address would command whichever arm answered
first. The joint count must equal the axes the controller reports.

## F175 — the controller acknowledges a target it is about to fault on

Measured on the 2.4.0 firmware, from a home pose, one joint at a time. A servo target beyond a joint's
range gets a **clean reply** (state byte `0x00`), the arm starts to move toward it, and the controller
stops with an error and state 4 a moment later: joint two toward 3.0 rad (range 2.094) moved 0.5 rad,
joint three toward 1.0 (range 0.192) moved 0.14 rad, joint five toward 3.6 (range 3.142) moved 1.3
rad. The error was 22, which the vendor's own table titles *Self-Collision Error*. A driver that
believed the reply would report a command that was bad as one that was taken, after the arm had moved.

So `write_command` **refuses a setpoint outside a joint's range before it sends anything** — `EINVAL`, no
frame — and the suite holds it two ways: in-process, that the controller *heard nothing* from the
refused command; and live, that the firmware never raised its error after one.

## F181 — the firmware reports the xArm 6's ranges on every model it starts

UFACTORY's simulator image carries five controller firmwares (`xarm5-type5`, `xarm6-type6`, `xarm6-type9` (the
Lite 6), `xarm6-type12` (the 850) and `xarm7-type7`), and `make check-sim-models` starts each the way their own
script does and runs the whole suite against it. Every one comes up as a body and passes the sim half of SCTS
(34, guest). Two things about them were not what the driver had assumed:

* **The joint ranges it reports are the xArm 6's, indexed as if every arm were one.** The rich report's
  `reduced_joint_limits` for the xArm 5 gives joint four a full turn and joint five the wrist bend (−1.693, π) —
  which is the xArm 6's layout; on the xArm 5 the wrist bend is joint four. The Lite 6 and the 850 report joint
  three and five ranges that differ from UFACTORY's own SDK table for those arms, and the xArm 7 reports a full
  turn on joints four and six where the table gives them (−0.192, 3.927) and (−1.693, π). Four of the five models
  disagree with the vendor's table by more than the milliradian F176 found the firmware rounds to. On a real arm
  configured as shipped they would presumably agree; nobody here has read one. A driver that believed the
  controller would have allowed an xArm 5's joint four ±360° where the vendor's client stops it at 180°, and
  the firmware acknowledges a target beyond a joint's range and faults afterwards (F175).
  **So travel is the narrower of the two**: `frames.VENDOR_JOINT_LIMITS` is a transcription of the SDK's
  `XCONF.Robot.JOINT_LIMITS` for every arm it lists, `layout_check` holds it to the SDK entry by entry, an
  operator's narrower reduced mode still narrows it, a disagreement is printed with the joints and both ranges,
  an arm the table does not list runs on the controller's word alone and says so, and two sources with nothing in
  common are a controller this driver has misread and will not command.
* **Two of them start with error 41**, *"The content of the friction file is invalid"* — the xArm 5 and the 850, for
  which the image carries no friction file. The driver refuses to come up over a latched error, on purpose (below),
  so the substrate does what a person does: `sim/clear_startup_error.py` reads the code, prints the vendor's own
  title and advice, clears it with UFACTORY's client and enables the motors, out loud, and a controller that still
  holds an error afterwards is a failure. A controller that starts clean is not touched.

## What it refuses to bring up

* **A latched error.** Cleaning it here would erase the one piece of evidence about why the arm
  stopped, on every restart. The operator clears it at the controller, after reading why.
* **An arm somebody else is moving.** State 1 at open is another client's motion.
* **Motors that stay off.** `MOTION_EN` is sent only when the enable byte says an axis is off — the
  simulator never answers it, and its byte is already full — and the byte is read back, not the reply
  trusted.
* **Two answers about one controller that disagree** — the version string on the control port against
  the rich report's.
* **No report stream, an arm of another axis count, a joint range that is not a range.**

## The stop

`emergency_stop` latches and sends `SET_STATE 4` on a socket of its own, opened at `open()` and never
blocking, so it does not wait for a command in flight and cannot land in the middle of one — the suite
holds the control socket's lock and shows the stop reaching the controller anyway. It is a **software**
stop. The controller's emergency-stop input is a circuit, and `safety.estop.independent` is a bench test
nobody passes from here.

`release_stop` runs the controller again with `SET_STATE 0` and **reads the state back off the stream**
before it answers OK; an error the controller itself holds is never cleared by it, and a failure
re-latches. `close` leaves the arm stopped where it is.

## What it cannot show

* **A real arm.** The firmware runs with no motors: no dynamics, no brakes, no thermal state. The 2.4.0
  controller's `is_simulation_robot` says so, and the descriptor declares it.
* **Timing at rate.** The simulator's clock is the host's. The descriptor declares 100 Hz because that
  is the fast report's rate, not because a servo stream was measured at it.
* **Velocity and torque.** The fast report carries neither, so both are NaN (§24). The rich report's
  `estimated_joint_torque` is an estimate from motor current, which a simulator has no motor to derive.
* **A real controller's ranges.** UFACTORY's simulator reports the xArm 6's ranges whatever arm it starts as
  (F181, below), so what a real xArm 5, Lite 6, 850 or xArm 7 reports has not been read; the driver takes the
  narrower of the report and the vendor's table for that arm, so the answer to a disagreement is the same.
* **The controller's other safety features** — collision detection, reduced mode, the safety boundary —
  are the controller's, and this driver neither configures nor relies on them.
