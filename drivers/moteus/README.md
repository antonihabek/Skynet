# skynet-moteus — mjbots moteus controllers over CAN-FD

`DRY-COVERAGE-PLAN.md` item 12.E.10's third driver. One Skynet joint per [moteus](https://github.com/mjbots/moteus) servo controller, on one CAN-FD bus,
joint-level, in position, velocity and torque with mixed per-joint modes. moteus is what mjbots's own quadruped and a good many research arms and legged
robots are built from, and it is the first driver in the vcan CAN family that needs **CAN-FD**: a command and its query do not fit eight bytes, and the frame that
asks for an answer has an identifier no 11-bit one holds.

```bash
make selftest                                      # the protocol, the simulated controllers and every refusal
make check                                         # and then the whole path through the real ABI
make check-vcan                                    # the same driver on a kernel CAN-FD bus, controllers in another process
make check-layout MOTEUS=/path/to/moteus           # this driver against mjbots's own client (run) and firmware headers
make chaos                                         # the link-chaos contract, across a CAN-FD relay
make faultvocab                                    # the fault-vocabulary contract: the mode and fault registers through the bridge
```

The driver is Python (the register protocol is arithmetic over byte strings and `socket` is stdlib) and runs under `drivers/python-host`. The bus underneath is the
CANopen driver's (`skynet_canopen.bus`, imported rather than rewritten: its loopback carries any frame, and a socket that speaks FD is `skynet_moteus.bus`), so
`models.json` stages both packages onto the robot.

## What is configured, and what has no default

```json
{
  "joints": [
    {"id": 1, "name": "J1", "role": "right_arm.shoulder.pitch"},
    {"id": 2, "name": "J2", "role": "right_arm.elbow.flex", "limits": [-1.2, 1.2]}
  ],
  "stop_mode": "zero_velocity",
  "watchdog_ms": 200,
  "max_torque_nm": 2.0,
  "interface": "can0"
}
```

* **`stop_mode` has no default.** What a stop does to a moteus axis is the integrator's risk assessment: `zero_velocity` is a derivative-only controller (it brings
  velocity to zero and holds nothing against gravity), `brake` shorts the phases, `stopped` lets the shaft coast, and **none of the three holds a loaded joint**. Which is
  right depends on the robot.
* `joints` has no discovery: an id says which controller on the bus, and nothing on the bus says which joint of which body it is. Roles are the operator's and are
  never inferred (§5.1).
* `watchdog_ms` is how long a controller keeps a command before it falls back to the behaviour its owner configured (mode 11, *timeout*). Left out, each controller's own
  default applies; **the driver never writes "no timeout"**, and a masked joint is *held* by a command every cycle and never skipped, because a joint that is skipped runs its
  watchdog down.
* `max_torque_nm` is a ceiling written with every command; left out, each controller's own.
* `limits` (radians, per joint) may narrow a travel and never widen it.

## What it reads, and what it will not invent

**Health is the mode register and the fault register together.** Mode 1 carries a fault code (32 to 50); mode 11 is the watchdog having run out; non-zero codes 96 to
105 in position mode are *which function is limiting the output* and not faults. In a session only *position* is the mode this driver put an axis in, so anything else —
stopped, a preparing phase, zero velocity, another master's voltage or current command — is something done to the axis by someone else, and it is **refused, never
repaired**: a driver that re-enabled an axis a person had stopped, by writing a position command into it, would be starting a machine whose reason for stopping nobody had
looked at. `open` is the person's act, and the only place an axis is brought up. The fault word carries the maker's own numbers, composed: the joint (from 1) in bits 16
and up, the mode in bits 8 to 15, the fault register in the low byte.

**Travel is the controller's own.** Over the diagnostic channel the driver asks each controller `conf get servopos.position_min` and `position_max`: the bounds the
controller itself enforces (it faults on starting position control outside them, and clamps its target to them), so the declared limits are not the operator's typing. A
controller with neither bound is a continuous joint. A controller that does not answer the channel, with no `limits` given, is refused.

**Effort is the controller's torque, at the output shaft, in newton metres** — a real number, where the drivers in this family whose wire reports amps have none.

**Homing is read, not claimed.** The home state is 0 *relative only*, 1 *rotor*, 2 *output*; only the last is a position referenced to the output shaft and reads as
homed. Anything else reads as pending, and the OS refuses to command a body whose positions are not trustworthy.

## The stop

`emergency_stop` writes the operator's stop mode to every controller and latches; commands are refused while it stands. From `zero_velocity` and `brake` the firmware
switches straight to another mode (`ISR_MaybeChangeMode`, read for this driver: the motor driver is never off in between), so `release_stop` clears the latch and the next
command takes the axes back with no interval in which they are limp. From `stopped` the controller re-enters through its calibrating phase with the driver off; a release that did
that would drop a loaded joint and report a clean return, so it is ENOSYS there and the session is renewed over axes a person has looked at (the Piper and ODrive precedent).
`close` sends nothing: a driver that stops talking leaves each controller to the fallback its owner configured.

## Where it stops

| Claim | Standing |
|---|---|
| **Guest tier** | Reached. The suite's checks (more on a kernel `vcan`), the layout check, the link-chaos run, the fault-vocabulary contract, and the sim half of SCTS through the real ABI — the numbers are in `docs/status.md` |
| Seeded defects | 292 of 292 noticed at rung 4 (100%), with 15 equivalent mutants left out for the reason written on each and 1 of 1 controls surviving as it must (`node tools/seed-defects/seed-defects.mjs --driver moteus`). The first campaign noticed 247 of 291 and found 39 checks nobody had written, among them every wait the driver makes: the in-process bus says at once that nothing is coming, which is why four timeouts had been written down as unobservable, and the suite's `Waiting` bus (it does not say so) with a stepping clock observes each |
| A real controller | None was on any bus this driver was measured against, and mjbots publishes no simulator of the controller. The simulated controllers answer the way the documentation and the firmware's mode machine say, and the wire is held to the maker's own client — but a simulation agreeing with its driver is rung 4, and the driver stays alpha |
| The plant | The simulated shaft is a second-order model written here: the real controller's gains, a payload, gravity, friction and a joint that stalls are unseen |
| What the stop does to a load | None of the three stops holds one (above); what each does to a moving or loaded joint was not measured |
| The multiplexed bus | A real CAN-FD bus carries a 5 Mbit data phase, arbitration among every controller on it, and a controller's own reply timing; `vcan` has none of those, so how many axes fit a millisecond is a question this suite cannot answer |
| The transports other than CAN-FD | mjbots's `fdcanusb`, the Pi3Hat and the RS485 multiplex bus are not spoken; `can_prefix` (a 13-bit routing prefix above the conversation id) is not used |

`make check-layout` reads mjbots's repository at the pin in `layout_check.py` and runs the maker's own Python client (its transports' third-party imports stubbed, which it
says) to hold every byte this driver sends and every conversion it makes to the maker's. It found that the register reference lists the limiting codes 96 to 103 and the
firmware sends 104 and 105 too (F285).
