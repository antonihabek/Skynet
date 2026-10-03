# `drivers/mavlink` — an ArduPilot or PX4 vehicle over MAVLink

`DRY-COVERAGE-PLAN.md` Phase 11 item 11.1. Multirotors, fixed wings and ground vehicles were a class of robot
this tree had no driver for: the fleet sweep found four aerial descriptions and refused all four (*"the file has
no joint a servo could drive"*), and the plan's only aerial sentence was that. But the class has the best
counterpart any class has. **ArduPilot's flight code builds for x86** — `arducopter`, `ardurover`, `arduplane`,
`ardusub` — with the board's physics and sensors simulated and everything above the hardware abstraction layer
the vehicle's own: the same parameter tree, arming checks, EKF, failsafes and MAVLink stack that fly. So this
driver is measured against the code that runs on the flight controller, which is F118's rung 1, and its codec
against the reference MAVLink implementation.

Item 12.E.1 added the other autopilot. **PX4 publishes the same thing**: `px4io/px4-sitl-sih`, its flight stack with the physics simulated
in-process, as an image. It speaks the same protocol with a different vocabulary — a packed mode word, parameters that arrive as bytes, a takeoff
above sea level, a velocity that is a stream — so the bridge's autopilot-specific half moved into `motions.py` (a change shown first to leave every
ArduPilot check, offline and against the firmware, as it was) and PX4 is the second motion set. See *PX4* below for what differs and what it found.

```bash
make selftest          # the wire, the frames and every refusal, against a simulated vehicle (ArduPilot's and PX4's; it prints its count)
make check             # and the sim half of SCTS through the real ABI, for each autopilot (34 passed)
make check-layout PYMAVLINK_PYTHON=.venv/bin/python   # 242 checks: this driver's MAVLink against pymavlink's
make check-sim-docker  # all of it against ArduPilot's own flight code, a vehicle of each family
make check-px4-docker  # and against PX4's own: a multirotor, a fixed wing and a rover (`sim/run-px4.sh`)
make chaos             # the driver's UDP link to a simulated vehicle through a relay that delays, drops, duplicates, reorders, truncates and corrupts
```

## What is here

| file | what it is |
|---|---|
| `skynet_mavlink/frames.py` | the wire: MAVLink 1 and 2 framing, the X.25 checksum, nineteen messages, the enums and ArduPilot's mode numbers for four vehicle families — every constant held against the reference |
| `skynet_mavlink/geometry.py` | NED and FRD into ENU and FLU (§44.4), and the arithmetic between them |
| `skynet_mavlink/link.py` | `tcp:`, `udpin:`, `udpout:` and `serial:` in the stdlib |
| `skynet_mavlink/bridge.py` | the driver |
| `skynet_mavlink/autopilot.py` | a simulated vehicle written here — for the refusals no firmware produces on demand |
| `selftest.py` | the driver's own suite |
| `layout_check.py` | this tree's MAVLink against `pymavlink` 2.4.49 — **242 checks** |
| `live.py` | the driver against the firmware, with the reference reading the same vehicle |
| `sim/` | the image that builds ArduPilot's SITL at a pinned commit, and the script `make check-sim-docker` runs in it |

## The evidence, and why it is not one fake agreeing with one driver

Four measurements, and they fail in different ways.

* **`layout_check`** opens the reference implementation and requires every message's id, checksum seed, field
  order and widths, every enum, and every mode of each of the four vehicle families to be what is written here. The
  seeds are *recomputed* from the field lists as MAVLink's own generator does, which proves the
  non-extension fields are the reference's in name, type and order. Each message is encoded here and by the
  reference from the same values and the bytes must be identical; every frame the reference builds must decode
  here to the values it was built from. **It caught six transcription errors on its first run**, all from memory
  that was wrong: `MAV_RESULT_CANCELLED` is absent from the pinned dialect, the VTOL types were renamed
  between releases, the AHRS sensor bit has a different constant name, and four modes (`turtle`, `autoland`,
  `motordetect`, `surftrak`) had been left out.
* **`selftest`** — the driver against a vehicle written in this repository. Nearly everything asserted is a
  refusal or a boundary, beside the input that succeeds: a frame with one bit flipped, a heartbeat that never
  comes, two vehicles on a link, PX4, a submarine that is not told what stopping it means, a signed link, an arm refused for a reason the vehicle states
  in words, an acknowledgement lost on the wire and one that never arrives, a stop that must survive a write
  in progress.
* **`live`** — the same driver against ArduPilot's own firmware in real time, 107 checks. A multirotor (50), a rover
  (20), a boat (20), a fixed wing (16) and a submarine (61) are armed, moved and stopped, with `pymavlink` on the vehicle's *second serial port* reading the same
  vehicle as a second opinion: every mode the driver sets is read back off the vehicle's own heartbeat, the
  position it reports agrees with the reference's to half a metre, the yaw agrees through the whole quaternion
  path, and the velocity the driver reports is the velocity the reference reads.
* **Conformance** — `skynet driver check` through the real ABI, against the simulated vehicle and against the
  firmware's multirotor, rover and fixed wing: 34 passed each, guest.

## What is read off the vehicle

| From | Used for |
|---|---|
| `HEARTBEAT.type`, `.autopilot` | which vehicle, and whether it is one this driver knows |
| `AUTOPILOT_VERSION` | the descriptor's slug (`ardupilot-copter-4.7.1`), and the build hash: the live block requires it to be the pinned commit's |
| the parameter table: `SIM_SPEEDUP` | **whether it is a simulation.** A SITL build has `SIM_*` parameters and a flight controller's build does not; the descriptor's `simulator` is that, and a configuration can only make it more cautious |
| `LOCAL_POSITION_NED`, `ATTITUDE_QUATERNION` (else `ATTITUDE`) | `base_pose` in REP-103: ENU position, `xyzw` orientation of FLU in ENU |
| the same two | `base_twist`: linear in the world frame, angular in the body frame |
| `SYS_STATUS`, `HEARTBEAT.system_status` | the fault word, and whether the state is degraded (an unhealthy sensor the vehicle has and uses; a battery at or under 10%; a pose that stopped arriving) |
| `STATUSTEXT` | the vehicle's own words for a refusal, attached to the error |
| the link-loss failsafe parameter | reported, and warned about when off. **Never written.** |

The pose is NaN, not zeros, until the EKF has one (a rover takes about half a minute to have one). A pose that
stops arriving goes back to NaN with the state degraded: a vehicle that is not localised must not read as one that
is at the origin.

## What it makes of the vehicle's own state

The heartbeat's `system_status` (`MAV_STATE`) is the autopilot's account of what it is doing, and the one place a failsafe shows. It was read for three
of its nine values, as FAULT alone — which §27 does not read (its `rhal` condition leaves `present` on DEGRADED), so a vehicle flying its own failsafe read as
healthy to the OS — and a vehicle that had begun to power down, one still booting and a state the dialect does not define read as nothing at all. It is read as
four classes now: UNINIT, BOOT and CALIBRATING are **degraded** (not flight-ready: DEGRADED, motions taken as far as the vehicle allows); STANDBY and ACTIVE are
well; CRITICAL, EMERGENCY and POWEROFF are a **fault** (FAULT and DEGRADED); FLIGHT_TERMINATION, the autopilot cutting the flight off, is a **stop** (ESTOP and
DEGRADED); and a state in no row is a fault. The fault word is the state, verbatim.

A vehicle that is moved by motions and not by joint commands cannot be held to "a faulted body refuses commands" without taking away what a person most wants in a
failsafe, so **the way out stays open**: while the vehicle reports a failsafe the motions that take it somewhere new (`arm`, `takeoff`, `goto`, `velocity`, `mode`)
are refused, and `hold`, `rtl`, `land`, `disarm` (with `force=1` in the air, as before), `status` and `param` are answered. While it reports a flight termination
nothing but `status` and `param` is. `drivers/fault-vocabulary.json` states this (`mav_state`, its `keeps`) and `make faultvocab` holds the driver to it against the
simulated vehicle (`tools/fault-vocab/`, `DRY-COVERAGE-PLAN.md` item 12.C.6). It says what the driver does with a state, never which one a real firmware reports when:
no firmware here was made to report CRITICAL or EMERGENCY.

## What it refuses to bring up

* **A link with no vehicle**, and **a link with two**, unless `system_id` names one: a driver that picked would
  command whichever vehicle spoke first.
* **An autopilot that is neither ArduPilot nor PX4**, and a PX4 vehicle that is not a multirotor, a fixed wing or a ground
  rover: a PX4 boat, submarine or VTOL has not been run. (PX4 itself was refused by name until item 12.E.1; see *PX4*.)
* **A submarine whose configuration does not say what stopping it means.** ArduSub has no takeoff and no ground, and the stop that is right under the surface — keep the depth, keep
  the place, stop the thrusters, or come up — is a person's decision and not a table entry, so `stop_mode` has **no default** and a submarine opens only with one
  (`alt_hold`, `poshold`, `manual`, `stabilize` or `surface`; see *ArduSub* below). The refusal names the five and says the vehicle was not commanded.
  **A boat and a submarine are no longer refused** — see below.
* **A VTOL**, whose modes are a plane's and a copter's at once.
* **A signed link whose vehicle ignores an unsigned request.** MAVLink 2 signing means ArduPilot ignores an unsigned command, and this driver
  holds no key. The flag alone is not the property: PX4's own build signs every frame it sends and acts on unsigned commands (F228), so the
  driver asks the vehicle, with the version request it sends anyway, and refuses only when nothing answers.

At `open()` it **changes nothing on the vehicle**: it asks for telemetry rates (a per-link setting the vehicle
forgets) and reads parameters. It never writes one, and the suite requires that no `PARAM_SET` is ever sent.

## The motions, under `vendor.mavlink.task`

Command-level (§59.2): the flight controller owns every servo loop, the kernel never sees these commands, and
Guest is the ceiling. The descriptor declares zero joints, an `aerial` or `wheeled` base, and these motions
(§84.1); a request is text, `<motion> k=v k=v`, and every coordinate is REP-103 (ENU, FLU):

| Motion | Vehicles | Does |
|---|---|---|
| `status` `param name=` | all | reads: mode, armed, landed state, GPS, battery, the last messages; one parameter |
| `arm` `disarm [force=1]` | all | `MAV_CMD_COMPONENT_ARM_DISARM`, confirmed by the heartbeat. `force=1` is the value the protocol documents for disarming in the air |
| `mode name=` | all | a mode by name, from a list of those that do not fly from stick input; `allow_modes` widens it |
| `takeoff alt=` | multirotor | needs armed and guided; **does not change the mode as a side effect** |
| `land` | multirotor | the land mode |
| `hold` `rtl` | all | `hold` is the stop mode (BRAKE, HOLD or LOITER, or a submarine's `stop_mode`), without the latch; `rtl` is return-to-launch (not a submarine's) |
| `surface` | submarine | the surface mode: come up. ArduSub ends it itself the moment the vehicle is at the surface, so the answer says where the vehicle is |
| `drive surge= sway= heave= yaw= duration=` | submarine | **sticks**, each a fraction of a full one in [-1, 1] and not a speed (ArduSub scales them by its joystick gain): a stream of at most five seconds that always ends in neutral frames |
| `goto x= y= z=` or `lat= lon= alt=` | all | a position setpoint, local ENU or global |
| `velocity vx= vy= vz= [yaw_rate=]` | multirotor, rover | **one** setpoint: the vehicle stops following it at `GUID_TIMEOUT`, so a caller repeats it while it should continue. No thread of this driver keeps a vehicle moving after its caller is gone |

A setpoint sent to a disarmed vehicle, or to one that is not in guided, is **ignored by the firmware without a word**.
The driver checks both before sending and says so.

## The stop

`emergency_stop` latches, then puts the vehicle in the mode that stops it: BRAKE on a multirotor (LOITER if BRAKE
will not engage), HOLD on a rover or a boat, LOITER on a fixed wing — which circles, because a wing cannot stand still.
HOLD stops the thrust and not the hull: a boat has no brake, and comes to rest as the water and the wind allow.
It does not kill the motors: a multirotor that loses its rotors in the air is a falling object, and that is a
decision for a person with a separate button. The command is resent every second until the vehicle's own
heartbeat says it took it, and it is sent with `try_send`: **it never waits behind a write in progress** and never
writes into the middle of one. It is a **software** stop over the same link as the commands; the descriptor
declares `safety.estop` and nothing that would claim the circuit. `close()` leaves an armed vehicle that this
session commanded braked, and changes nothing on one it only watched.

A **submarine's** stop is the mode its configuration names, and it also sends neutral sticks at once and with every resend: the vehicle keeps the last stick it was given for about three
seconds (measured: it held its speed for 2.4 s), so a stop that changed the mode and left the sticks would be seconds of thrust after the latch.

## Findings

* **F178 — §5 has no base kind for water** (carried by §97). The firmware builds a boat and a submarine; the
  driver refused both by name because calling a hull `wheeled` or leaving the kind out would each be wrong, in
  different directions. §97 added `surface` and `submerged` to the closed set and made both mobile, and the driver
  now carries **a boat** — ArduRover built with a boat frame, which reports `MAV_TYPE_SURFACE_BOAT`, takes Rover's
  modes and motions, and declares `base: "surface"` — tested against ArduPilot's own boat firmware, and **a submarine**
  (`base: "submerged"`), whose own motion set is below.
* **F179 — a yaw rate of zero is not "no yaw rate", and ArduRover reads the difference.** The first velocity
  setpoint kept the yaw rate field active at zero, which a copter reads as "hold your heading" and a rover as
  "drive straight along it": the rover, told to go north, drove southeast at the requested speed and never
  turned. Found by the firmware; the yaw rate is now ignored unless the caller gives one, and the live block
  asks the rover for north and checks it turned.
* **The link-loss failsafe has a different name on a plane.** `FS_GCS_ENABLE` is a copter's and a rover's;
  ArduPlane's is `FS_GCS_ENABL`. The first live run read `null` for it on the plane.
* **A parameter name longer than sixteen bytes crashed the encoder** and would have taken the extension down
  with it. Found by the selftest; it is `EINVAL` now, and any request that does not fit the wire is.

## PX4 (item 12.E.1) — what differs, what it found, and how far it has been measured

A multirotor, a fixed wing and a ground rover running PX4 are driven by the second motion set in `skynet_mavlink/motions.py`. Each difference below was
read off PX4's own SIH image first (`px4io/px4-sitl-sih`, pinned by digest in `sim/run-px4.sh`) and is held by a check in `selftest.py`'s PX4 section.

| | ArduPilot | PX4 |
|---|---|---|
| a mode | a number | a packed word: the main mode in bits 16–23, the sub mode in 24–31; `DO_SET_MODE` carries the halves in `param2` and `param3` |
| a mode it will not enter | `FAILED` | **acknowledged as accepted and not made** (F224): the heartbeat is the only answer |
| an integer parameter | converted into the float field | **its four bytes copied there** (F225): 10040 is 1.4e-41 |
| takeoff altitude | above home | above sea level, so the driver adds home's, from the global position |
| a command to a disarmed vehicle | ignored | `NAV_TAKEOFF` is acknowledged and **enters takeoff mode** (F227), so the driver refuses takeoff, goto and velocity unless armed |
| goto | a setpoint in guided mode | a `DO_REPOSITION` over `COMMAND_INT` (integer latitude and longitude); a local goto is turned into one from the origin PX4 reports, in PX4's own projection |
| velocity | one setpoint, followed until it times out | a **stream** of bounded length (`duration=`, at most five seconds): offboard mode is entered only with one running and left for **return-to-launch with a critical state when it stops** (F226), so the driver hands the vehicle to hold *while still streaming* |
| whether it is a simulation | `SIM_*` parameters | the airframe, where only the simulation build has it (`SYS_AUTOSTART` 10040–10049); otherwise `simulator` in the configuration, with **no default** |
| a signed link | refused | PX4 signs every frame and acts on unsigned commands (F228): refused only if the version request goes unanswered |

**How far it has been measured, on 2026-10-03.** The offline suite (314 checks, ArduPilot's and PX4's), the fault-vocabulary contract (76) and the layout check
(235 against `pymavlink`, the two new messages and PX4's mode words included) pass. Against PX4's own firmware (`make check-px4-docker`, `sim/run-px4.sh`) the
multirotor's live block (`live_px4.py`) passed **58 of 58** on each of three clean runs, the fixed wing **16 of 16** and the rover **19 of 19**, and the sim half of SCTS
through the real ABI passed 34 of 34 at guest on each. *Clean* matters: the block is real time, and a run with other work on the same host failed its impaired-link checks
(a state read aged 1.76 s where it is normally 0.05 s), so it is run alone. The nightly job (`px4-sih`) has not yet run on a runner. The PX4 vehicles are at **rung 1**: the maker's
own flight stack, with a second implementation reading the same vehicle, on a simulated board with no sensor noise, motors or radio.

Repeating the block, and then repeating it under loss, found three more things, F229 to F231. Two were the harness's and one was the driver's:

| | |
|---|---|
| **F229** | PX4's second MAVLink instance relays the **driver's own** GCS heartbeat (system 255) to the port the reference listens on, so "the latest heartbeat" took the driver for the vehicle about half the time: the mode, the arming bit and the state it read were the driver's. The reference keeps only the vehicle's heartbeats now. |
| **F230** | The reference read stale data. Its pump stopped at the first datagram that yielded no message, so after a quiet spell it held a position seconds old, and a goto aimed at its idea of where the vehicle was went seven metres from where it was. PX4's second instance also sends position at 3 Hz and global position at 0.8 Hz whatever rate is asked (it has a data-rate budget of its own), so a target is taken only once the vehicle is at rest and a global position newer than that has arrived. And `pymavlink`'s `interpret_px4_mode` does not name PX4's offboard (the heartbeat is base `0x91`, main mode 6), so the check compares the main mode with `pymavlink`'s own constant. |
| **F231** | **A defect in the driver.** A mode request is one datagram that nothing acknowledges (PX4 acknowledges modes it will not enter, F224), and a velocity needs two of them, offboard in and hold out. On a link that loses 3% the live block failed about one run in two, and one failure was the dangerous kind: the request for hold was lost, the stream stopped, and PX4 stayed in offboard with nothing streaming, which is the state it answers by flying home. A request is asked again every second while the stream runs, until the heartbeat shows the mode, for the window the call already had. Shown failing first in `selftest.py` (a request lost on the way in, on the way out, and a link that loses every one, which is still an error), and held by two seeded defects. |

One finding was in the parameter decoder: PX4 copies an integer's four bytes into the float field, and the driver read them through a Python float, which refused a value whose bytes are a NaN (a signed `-1` is `0xFFFFFFFF`) and would quiet a signalling one. It reads the four bytes as they came now, held by `-1`, `-6000000` (a signalling NaN) and `2147483647`.

## ArduSub (item 12.E.4) — what differs, what it found, and how far it has been measured

A submarine or ROV running ArduSub (`MAV_TYPE_SUBMARINE`) is driven by `ArdusubMotions`. Each difference was read out of ArduSub's own source at the pinned commit (`dbe79216`:
`ArduSub/joystick.cpp`, `mode_guided.cpp`, `mode_surface.cpp`, `failsafe.cpp`, `defines.h`) and then held against the firmware, which was built here and run in `sim/run-sim.sh`.

| | the other ArduPilot vehicles | ArduSub |
|---|---|---|
| the stop | a table entry (BRAKE, HOLD, LOITER) | **the operator's decision, with no default**: `stop_mode` is `alt_hold` (keeps the depth, lets the vehicle drift), `poshold` (keeps the place; needs a position source), `manual` or `stabilize` (stops the thrusters; buoyancy decides whether it rises or sinks) or `surface` (ascends, which under ice or on a tether is not a stop). A configuration without one is refused at open |
| what moves it | a setpoint, followed in guided | **sticks**: `MANUAL_CONTROL`, x, y and r in -1000..1000 and z in 0..1000 with 500 neutral; the vehicle keeps the last one for `RC_OVERRIDE_TIME`, so `drive` always ends in neutral frames, and so does a stop |
| the signs | FLU to NED | FLU to the stick's own: forward is x, **left is a negative y** and a counter-clockwise turn **a negative r**, up is z above 500. Held against the firmware's own `RC_CHANNELS` (channel 5 forward, 6 lateral, 3 throttle up, 4 yaw clockwise, each above 1500 for the positive direction) |
| takeoff, land, return to launch | yes | none: there is no ground, and the way out is `surface` |
| `surface` | n/a | a mode ArduSub leaves by itself, in `alt_hold`, the moment the vehicle is at the surface (`ModeSurface::run`), so a vehicle that did what it was asked is never seen in it |
| guided, `poshold` | after the EKF has a position | after a position fix: about forty seconds after boot in this simulator, and refused with "requires position" until then |
| `MAV_STATE` | STANDBY disarmed, ACTIVE armed | CRITICAL for the first five seconds after boot, then the same: the driver reads CRITICAL as a failsafe and refuses a motion by it, so the live block retries |

**How far it has been measured, on 2026-10-03.** The offline suite's ArduSub section is 78 checks (a stop that must be named, the signs, the neutral that ends a stream and a stop, a drive refused in a mode that ignores
sticks, the way out in a failsafe, `surface` from depth and at the surface) and `layout_check` holds the new message to `pymavlink` (242 checks, byte for byte). Against ArduSub's own firmware the live block
(`live.py sub`, with `pymavlink` on the vehicle's second serial port) is **61 of 61**: the vehicle goes the way the reference says it faces, to the left on a left sway, and turns counter-clockwise on a positive yaw;
what the vehicle reads on each RC channel is ArduSub's own for each stick; the neutral that ends a drive makes the vehicle slow at once where an unended override held its speed for 2.4 s; the named stop is the mode on the
reference's heartbeat; the drive ends at once on the stop; and the sim half of SCTS passes through the real ABI.

Two findings, and one limit:

* **F239 — `surface` is a mode ArduSub leaves by itself**, so a driver that waited for the heartbeat to show it answered an error for a vehicle that was already at the surface and did what it was asked: the live block's
  reference, reading the latest heartbeat a moment later, saw `alt_hold` and never `surface`. The answer is now where the vehicle is (`alt_hold`, `at_surface`) when that is where a surface request ends, and an error when the vehicle is
  at depth in `alt_hold` (a lost request leaves a vehicle where it was). Shown failing first in `selftest.py`.
* **F240 — the simulated submarine's depth is not a measurement.** Its pressure sensor reads *negative* absolute pressure and its depth estimate swings by metres in seconds with nothing commanded (4.5 m and back in a depth
  hold), so the vertical axis cannot be held to where the vehicle goes. It is held to what the firmware reads off the stick, which is ArduSub's own RC channels in ArduSub's own documented directions, and the offline suite holds
  the rest. No claim is made about a real vehicle's heave.
* **F244 — `surface` read a heartbeat that was sent before the vehicle answered.** After an acknowledged request for surface the driver took the heartbeat it already held, which on a vehicle that sends one a second is up to a second stale and still says `alt_hold`: a vehicle at depth that was ascending was
  answered with an error. It waits for a heartbeat that arrived after the answer now. Found by the seeding campaign (three PX4 seeds killed by this check, which is what a racy check does), not by the live block, because SITL's submarine is always near the surface, where `alt_hold` is a true answer.
* **Not found, but recorded:** a submarine that was opened within five seconds of boot is refused a motion by the driver's own failsafe class (CRITICAL), and arm among them; the firmware's pilot-input failsafe
  (`FS_PILOT_INPUT`, disarm by default) never fired in ten armed seconds with no sticks at all.

## What it cannot show

* **A flight controller.** SITL is the firmware on a simulated board: no sensor noise or failure, no vibration,
  no motors, no radio, no real battery. The descriptor declares `simulator: true` because the firmware says
  so, and §35.3 caps the tier at guest by mechanism.
* **The vehicle's failsafes in the field**, the EKF on real sensors, and anything geographic: a geofence, a
  home position that means something.
* **Timing at rate.** SITL's clock is the host's. The live block does put delay, jitter and loss on the link
  (`tc netem`, 80 ms ± 40 ms and 3% on a poor link, then 250 ms ± 120 ms and 40% on a very bad one) and requires
  that setpoints still arrive, reads stay fresh, an acknowledged command still lands, a stop still reaches the
  vehicle and no request hangs. That shows how the driver behaves on a bad link; it does not measure a
  real one.
* **A real ROV.** Its thrusters, tether, depth sensor and DVL; a stop under water in a current or under ice; what heave does to a vehicle that is not neutrally buoyant in a model.
* **VTOLs, and a PX4 boat, submarine or VTOL.** Refused by name.
* **A fixed wing's takeoff under load, a rover's steering on a surface that is not flat, and the difference between
  the firmware's default tuning and a vehicle's.**
