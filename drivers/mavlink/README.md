# `drivers/mavlink` — an ArduPilot vehicle over MAVLink

`DRY-COVERAGE-PLAN.md` Phase 11 item 11.1. Multirotors, fixed wings and ground vehicles were a class of robot
this tree had no driver for: the fleet sweep found four aerial descriptions and refused all four (*"the file has
no joint a servo could drive"*), and the plan's only aerial sentence was that. But the class has the best
counterpart any class has. **ArduPilot's flight code builds for x86** — `arducopter`, `ardurover`, `arduplane`,
`ardusub` — with the board's physics and sensors simulated and everything above the hardware abstraction layer
the vehicle's own: the same parameter tree, arming checks, EKF, failsafes and MAVLink stack that fly. So this
driver is measured against the code that runs on the flight controller, which is F118's rung 1, and its codec
against the reference MAVLink implementation.

```bash
make selftest          # 196 checks: the wire, the frames and every refusal, against a simulated vehicle
make check             # and the sim half of SCTS through the real ABI (34 passed)
make check-layout PYMAVLINK_PYTHON=.venv/bin/python   # 193 checks: this driver's MAVLink against pymavlink's
make check-sim-docker  # all of it against ArduPilot's own flight code, a vehicle of each family
```

## What is here

| file | what it is |
|---|---|
| `skynet_mavlink/frames.py` | the wire: MAVLink 1 and 2 framing, the X.25 checksum, eighteen messages, the enums and ArduPilot's mode numbers for four vehicle families — every constant held against the reference |
| `skynet_mavlink/geometry.py` | NED and FRD into ENU and FLU (§44.4), and the arithmetic between them |
| `skynet_mavlink/link.py` | `tcp:`, `udpin:`, `udpout:` and `serial:` in the stdlib |
| `skynet_mavlink/bridge.py` | the driver |
| `skynet_mavlink/autopilot.py` | a simulated vehicle written here — for the refusals no firmware produces on demand |
| `selftest.py` | the driver's own suite |
| `layout_check.py` | this tree's MAVLink against `pymavlink` 2.4.49 — **193 checks** |
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
  comes, two vehicles on a link, PX4, a boat, a signed link, an arm refused for a reason the vehicle states
  in words, an acknowledgement lost on the wire and one that never arrives, a stop that must survive a write
  in progress.
* **`live`** — the same driver against ArduPilot's own firmware in real time, 88 checks. A multirotor (50), a rover
  (20) and a fixed wing (16) are armed, flown and stopped, a boat and a submarine refused by name, with `pymavlink` on the vehicle's *second serial port* reading the same
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

## What it refuses to bring up

* **A link with no vehicle**, and **a link with two**, unless `system_id` names one: a driver that picked would
  command whichever vehicle spoke first.
* **PX4 and anything but ArduPilot.** PX4 packs a main mode and a sub mode into one word and wants a setpoint
  stream before it engages offboard; that is another driver, and no PX4 has been run.
* **A boat or a submarine**, by name. §5's base kinds are `fixed`, `wheeled`, `tracked`, `legged`, `aerial` and
  `rail`; none is a hull (F178, `docs/issues/no-base-kind-for-water.md`). The live block proves both refusals
  against ArduPilot's own boat and submarine.
* **A VTOL**, whose modes are a plane's and a copter's at once.
* **A signed link.** MAVLink 2 signing means the vehicle ignores an unsigned command; this driver holds no key.

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
| `hold` `rtl` | all | `hold` is the stop mode (BRAKE, HOLD or LOITER), without the latch; `rtl` is return-to-launch |
| `goto x= y= z=` or `lat= lon= alt=` | all | a position setpoint, local ENU or global |
| `velocity vx= vy= vz= [yaw_rate=]` | multirotor, rover | **one** setpoint: the vehicle stops following it at `GUID_TIMEOUT`, so a caller repeats it while it should continue. No thread of this driver keeps a vehicle moving after its caller is gone |

A setpoint sent to a disarmed vehicle, or to one that is not in guided, is **ignored by the firmware without a word**.
The driver checks both before sending and says so.

## The stop

`emergency_stop` latches, then puts the vehicle in the mode that stops it: BRAKE on a multirotor (LOITER if BRAKE
will not engage), HOLD on a rover, LOITER on a fixed wing — which circles, because a wing cannot stand still.
It does not kill the motors: a multirotor that loses its rotors in the air is a falling object, and that is a
decision for a person with a separate button. The command is resent every second until the vehicle's own
heartbeat says it took it, and it is sent with `try_send`: **it never waits behind a write in progress** and never
writes into the middle of one. It is a **software** stop over the same link as the commands; the descriptor
declares `safety.estop` and nothing that would claim the circuit. `close()` leaves an armed vehicle that this
session commanded braked, and changes nothing on one it only watched.

## Findings

* **F178 — §5 has no base kind for water** (`docs/issues/no-base-kind-for-water.md`, proposed). The firmware
  builds a boat and a submarine; the driver refuses both by name because calling a hull `wheeled` or leaving the
  kind out would each be wrong, in different directions.
* **F179 — a yaw rate of zero is not "no yaw rate", and ArduRover reads the difference.** The first velocity
  setpoint kept the yaw rate field active at zero, which a copter reads as "hold your heading" and a rover as
  "drive straight along it": the rover, told to go north, drove southeast at the requested speed and never
  turned. Found by the firmware; the yaw rate is now ignored unless the caller gives one, and the live block
  asks the rover for north and checks it turned.
* **The link-loss failsafe has a different name on a plane.** `FS_GCS_ENABLE` is a copter's and a rover's;
  ArduPlane's is `FS_GCS_ENABL`. The first live run read `null` for it on the plane.
* **A parameter name longer than sixteen bytes crashed the encoder** and would have taken the extension down
  with it. Found by the selftest; it is `EINVAL` now, and any request that does not fit the wire is.

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
* **PX4, boats, submarines, VTOLs.** Refused by name.
* **A fixed wing's takeoff under load, a rover's steering on a surface that is not flat, and the difference between
  the firmware's default tuning and a vehicle's.**
