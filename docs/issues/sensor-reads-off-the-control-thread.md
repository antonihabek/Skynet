# Proposed amendment — §24 never says whether a sensor may be read while a joint is

**Status: ratified 2026-09-18 as §89 of the specification, and built** — all six steps of the
implementation order below. Kept as the proposal it was. **Route A** and the suspension rule, which
this file recommends together, are both what was built.

Two places the built form departed from this file, each recorded where it was settled.

* **The MuJoCo driver takes no lock at all.** This file sketched *"`mjv_updateScene` under a lock,
  `mjr_render` outside it"*; a lock the control thread shares with the render path is a lock it can
  wait on, which is the thing §89.1's promise rules out. `drivers/mujoco` instead publishes a copied
  snapshot by replacing one reference, and renders on a thread that owns the GL context and an
  `MjData` of its own. Its selftest holds a frame mid-render and requires fifty steps to complete
  meanwhile — and the lock this file sketched fails that check (`DRY-COVERAGE-PLAN.md`, fourteenth
  session).
* **The host was not the only obstacle.** The list below names `drivers/python-host` as what stood
  between a Python driver and a declaration; that was true and is fixed (protocol 2), and F46 found
  the second half inside the drivers — the LeRobot bridge's frames are a by-product of its joint
  read, so it still cannot declare, for a reason only its own code can remove.

*"It should be the measurement that shows it closed"*, step 6 says. It is: on the machine that
reproduces F24 — a 14.4 ms software render against a 20 ms period — `check-loop-mujoco` fails with
three overruns and a suspended stream before the declaration and closes with **none** after it.

Written 2026-09-18 from `DRY-COVERAGE-PLAN.md` F24, which said in the eighth session that its fix
*"is an ABI question, not a patch … a specification change and therefore an issue"* and was never
filed. Like `per-joint-command-modes.md`, it came out of a body rather than out of reading the
document: nothing about it is visible until a sensor is slow.

## The gap

§47.2's fourth capture backend is RHAL `read_sensor` — *"anything only the driver can reach"*. The
driver is one C context, and the control loop holds it on every cycle. `bindings/c/skynet_rhal.h`
states three rules and none of them is about threads:

> 1. read_state() and write_command() are wait-free and allocate nothing. …
> 2. emergency_stop() must work from a signal handler and from a process that is dying. …
> 3. describe() is the source of truth for the body. …

Nothing says `read_sensor` may be called while `read_state` or `write_command` is running on another
thread. So `crates/skynet-kernel-host/src/capture.rs` does the only thing the ABI permits: it reads a
camera **inside** the control cycle, after the command is written and before the cycle is recorded,
and says why in its header — *"a capture thread with its own reference to that context would be a
second caller on an object whose thread-safety the ABI never promised."*

That is correct and it has a price the eighth session measured. With `BODY=mujoco`, a physics
simulator rendering its own wrist camera through `MUJOCO_GL=osmesa`:

```
cycles          250
overruns         73 — cycles that missed the 20.00 ms period
frames read      73 of 73 reads
```

**Exactly one overrun per frame read and none on any other cycle.** The render costs 18.2 ms; the
period is 20 ms; the physics step is 0.007 ms. §23.3 already refused this shape for inference —
*"a design where a 40 ms inference spike delays a torque command is a design that falls over"* —
and answered it with a second process, which is why `skynetp` reports **zero** overruns in the same
run. Capture has no such separation: it is a call on the control thread, by construction.

## What it costs today

* **A slow sensor delays §7's guards by exactly its own read time.** On the 50 Hz loop above that is
  a missed period per frame. On a 1 kHz loop it is any sensor read over a millisecond — which is
  most cameras on most buses.
* **Nothing refuses it.** The loop counts overruns, and F24's script attributes them to sensor reads.
  Neither stops the stream. A robot whose control period is being spent on a camera keeps running,
  late, for as long as the camera keeps rendering.
* **The honest drivers pay for it.** A driver that could read its sensor from another thread safely —
  a camera behind its own DMA engine, a tactile array on its own bus — has no way to say so, so it is
  read on the control thread with everybody else.

## Four routes

### A. The driver declares it, per sensor (recommended)

A descriptor field on each sensor:

```json
{ "name": "wrist", "modality": "rgb", "width": 128, "height": 96, "concurrent_read": true }
```

`true` is the driver's promise that **`read_sensor` for this sensor may be called from a thread other
than the one calling `read_state` and `write_command`, while they run.** Absent means `false`, which
is exactly today's behaviour and what every driver written so far means. A runtime reads a declared
stream on a thread of its own and an undeclared one on the control thread.

It is §86's shape one section along: additive, carried in the descriptor rather than the vtable, so
**no ABI bump**, and the promise sits with the only party that can make it. A vendor who has not
thought about it has not declared it, and gets the conservative behaviour.

### B. Make it a fourth rule for every driver

*"`read_sensor` is callable concurrently with the real-time path."* Rejected: it binds every driver
already written to a promise none of them made, and a driver that breaks it corrupts state in a
robot's control path with no error anywhere. A rule that is true of some drivers is a declaration.

### C. A second context for sensors

`open_sensor(config) → skr_sensor_ctx`, a separate object with its own thread affinity. Rejected for
now, and for a mechanical reason: the vtable has no size field, so a member appended to `skr_driver`
is read past the end of every older driver's static struct. It needs ABI 2, which §24 reserves for a
breaking change, and A gets the same separation without one.

### D. Read sensors from a second process

Open the driver twice. Rejected: most devices are exclusive, and the ones that are not are
exactly the ones A already lets declare themselves.

## And the undeclared case, independently of A

A alone leaves an undeclared slow sensor where it is today. So the proposal pairs it with one rule
about the control thread, which does not need a vendor's promise:

**Capture yields to control.** A stream read on the control thread whose reads make the cycle miss
its period is **suspended** — no more reads, an annotated gap that says why, and `skynetk` says so
once on the terminal and in the black box. The policy then sees no fresh frame and holds for the
reason it already has a word for (`no_frame` / `frame_too_old`), which is the safe direction: vision
degrades, and the guards keep their period.

§47.4 says *"capture is never stalled, because stalling capture starves the policy to protect the
recording"*. That sentence is about the recording queue and it stands. This is the other trade — the
control period against a frame — and §7 decides it: a guard evaluated late is a guard evaluated on
a state that no longer exists.

## What each route needs from the existing drivers

None of the drivers in this repository could honestly declare `concurrent_read` today, and the
reason is worth writing down because it is what a vendor will meet:

* `drivers/python-host` serialises every call over one pipe to one interpreter. Two threads calling
  in would interleave on the pipe. It would need to multiplex requests before any Python driver
  behind it could declare anything.
* `drivers/mujoco` steps and renders one `mjData`. Rendering while another thread steps is a data
  race inside MuJoCo. It would need to render from a scene snapshot (`mjv_updateScene` under a lock,
  `mjr_render` outside it).
* `drivers/mock` and `drivers/pendant` have no sensors.

So A is a promise nobody here can yet make, and the suspension rule is what changes behaviour on the
day this is ratified.

## Implementation order, if ratified

1. **The oracle.** `concurrent_read` on `EmbodimentSensor`, validated under
   `embodiment.descriptor.valid` (a boolean or absent; anything else refused by name). A pure
   placement function — declared, measured read time, control period in; `control_thread`,
   `own_thread` or `suspended` out — so the rule is data and not a loop's side effect.
2. **Corpus cases**, before any edition: the undeclared fast stream, the undeclared slow stream (the
   F24 numbers), the declared slow stream, a non-boolean declaration.
3. **The editions.**
4. **`skynet-kernel-host`**: the suspension rule on the control thread first, because it needs no
   driver's cooperation; then a capture thread for declared streams, which needs `skynet-dl` to hand
   a second thread the context for `read_sensor` only — on the `unsafe` allow-list already, with the
   invariant written where the pointer crosses.
5. **`skynet driver check`**: a declared stream is read concurrently with `read_state` for a bounded
   interval and the results compared, so a false declaration is a conformance failure rather than a
   field report.
6. **`drivers/python-host`** multiplexing, then **`drivers/mujoco`** rendering from a snapshot, and
   `check-loop-mujoco` re-run: the measurement above is what the section is for, and it should be the
   measurement that shows it closed.

## Amends

§24 (the header's rules, and the sensor declaration), §47.2 (the RHAL backend's placement), §7 (the
control period against a capture read).
