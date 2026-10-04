# `drivers/robotiq` — a Robotiq 2F-85 or Hand-E gripper over Modbus RTU

`DRY-COVERAGE-PLAN.md` item 12.E.8. Robotiq's 2F-85, 2F-140 and Hand-E adaptive grippers speak **Modbus RTU** over RS-485: a six-byte command block at register 1000 and a six-byte status block at register 2000,
exchanged by one function 0x17 transaction (write the command, read the status) a cycle. Robotiq publish a C++ SDK for them with a fake gripper in it (`github.com/Robotiq/grippers`, BSD-3-Clause). This driver speaks that
wire. It links nothing of the maker's: the wire was read out of the SDK's headers, `layout_check.py` **runs the maker's own client** and holds this driver's bytes to it, and the live block runs the driver against **the maker's
own fake gripper**, served unmodified on a TCP port and a pseudo-terminal, with the maker's own client reading the same gripper: F118's rung 2. It is not a gripper.

```bash
make selftest          # 342 checks: the frames, every refusal, the exchange cycle, the stop, the faults, the motions and the line, against a simulated gripper on a pseudo-terminal written here (needs Linux)
make check             # and the sim half of SCTS through the real ABI (37 passed)
make build-sim ROBOTIQ_REPO=/path/to/grippers ROBOTIQ_SIM=/path/to/build    # the maker's SDK, unmodified, and four small programs against it
make check-layout ROBOTIQ_SIM=/path/to/build   # this driver's bytes against the maker's own client (43 checks)
make live ROBOTIQ_SIM=/path/to/build           # this driver against the maker's own fake gripper, the maker's own client looking on (56 checks)
make chaos             # the link-chaos contract: the gripper's serial line through a relay that delays, stalls, loses, truncates and corrupts it (51 checks)
make faultvocab        # the maker's own fault codes, injected and held to the class the table gives each (124 checks)
make check-sim-docker  # all of it, in a container that builds the maker's SDK at a pinned commit
```

## What is here

| file | what it is |
|---|---|
| `skynet_robotiq/frames.py` | the wire: the two blocks, the Modbus RTU frames, the faults and their severities, the two measured profiles and the maker's unit conversions. Pure, every constant cited |
| `skynet_robotiq/link.py` | the line: a serial port or a TCP socket (an RS-485 gateway that forwards the line raw) |
| `skynet_robotiq/bridge.py` | the driver |
| `skynet_robotiq/gripper_sim.py` | a simulated gripper written here: the dry substrate, with an activation that sweeps the fingers, a motion at the speed it is told, a workpiece, the manual's faults, the one-second no-communication fault and levers for the line |
| `skynet_robotiq/__main__.py` | the entry `drivers/python-host` serves, with `SKYNET_ROBOTIQ_SIM=1` for `skynet driver check` with no gripper and no container |
| `selftest.py` | the driver's own suite |
| `layout_check.py` | this driver's bytes against the maker's client, which is run |
| `live.py`, `sim/` | the live block, the substrate image (the maker's repository at a pinned commit and a toolchain), `sim/build.sh` (the maker's SDK and four harnesses), `server.cpp` (the maker's fake gripper on a TCP port and a pseudo-terminal), `codec_dump.cpp` (what the maker's client writes, reads and converts), `second_opinion.cpp` and `client_probe.cpp` (the maker's client reading a gripper, and driving one) |
| `chaos.py`, `faultvocab.py` | the subjects for `tools/faultproxy/chaos.py` and `tools/fault-vocab/contract.py` |

## The body

One prismatic joint, **`jaw`**: the opening between the fingers in metres, from the profile's `min_opening` to its `max_opening`. The profile is the maker's *measured* one (`device_profile.hpp`: the register a gripper
reads with nothing between its fingers at rPR 0 and at rPR 255) for the **2F-85** and the **Hand-E**. **Nothing the gripper says identifies its model**, so `model` is the operator's claim and has no default. Any other
gripper (a 2F-140, which the maker's SDK has no profile for, or either with other fingers, which move the measured endpoints) is `model: "custom"` with a `profile` the operator measured. It takes a position, one control
mode, and its tier ceiling is Guest (§31.3).

```json
{"device": "/dev/ttyUSB0", "baud": 115200, "slave": 9, "model": "2F-85", "stop_mode": "halt", "speed": 0.1, "effort": 0.5, "simulator": false,
 "limits": [0.0, 0.085], "roles": {"jaw": "right_hand.gripper.grip"}, "rate_hz": 50}
```

* **`device`** or **`host` and `port`** — the gripper's RS-485 adapter, or a gateway that forwards its line raw. There is no default and no scan: a driver that guessed a port would command whichever device answered first.
* **`stop_mode`** — **required, and only `halt` is accepted** (see below).
* **`speed`** (m/s, from the profile's range) and **`effort`** (0 to 1, the maker's `effortToRegister`) — **required**: nothing in a position request says how fast the fingers go or how hard they close on what they meet.
* **`simulator`** — **required**: nothing the gripper says tells a real one from a simulation, and a bench run described as one costs a tier it could have had.
* `limits` (narrower than the profile's, never wider), `roles`, `slave` (9 from the factory), `baud` (115200), `rate_hz` (50, at most 250: the maker's own figure for the line), `timeout_ms`, `stale_ms`, `open_timeout_ms`.
  A key the driver does not read is refused, naming the keys it does.

## One image, sent every period

The maker's own design, followed because Robotiq proved it on hardware: **one command image, sent in every exchange**, whatever has or has not changed. A gripper that hears nothing for a second raises fault `0x09`, so
the exchange cycle is also the keep-alive. A command is not a transaction of its own: `write_command` changes the image and waits for the exchange that carried it, in a quarter of a second at the most (the control loop's
bound, F256), and a command that was not carried is `EIO` **now and is put back** — it is not written when the line returns. The image is **seeded from the gripper's own echoes** (rACT, rGTO and rPR from gACT, gGTO and
gPR), so that connecting never disturbs a gripper that is already running, and **made again from them after a dead line**: a gripper that lost its power while nobody was talking to it is not told rACT is set (F263).

## What it refuses to do by itself

* **Activate.** Activation, and the reset after a major fault, is a **calibration sweep that moves the fingers through their full range** (the maker's own warning on `recoverFromFault`). A gripper that is not activated
  opens, reports `HOMING_PENDING`, and is refused commands, naming the `activate` motion; a major fault is cleared by the `recover` motion, the maker's unconditional handshake. `activate` leaves an activated gripper
  alone, refuses a major fault (the maker's `FaultLatched`), and waits for an activation already running.
* **Decide what a stop is.** `halt` clears rGTO and goes on exchanging, which takes the go-to away and leaves the gripper activated. It does **not** release a held workpiece. rATR is Robotiq's *emergency release*, which opens
  or closes the fingers all the way and leaves the gripper in a major fault until it is reset; whether a pinched finger or a held part makes release the safer stop is the integrator's risk assessment, and a configuration
  that asks for it is refused with that said, where a quiet `halt` would be a stop nobody chose. It is a **software stop**: the gripper's own e-stop is a circuit.

## The stop

`emergency_stop` sets the latch and clears rGTO in the image **under the lock `write_command` changes the image under**, so a command cannot put the go-to back after a stop; the loop is woken, and since the image is sent
every cycle a lost frame is sent again a period later. While latched no command is accepted and no motion runs (`activate` and `recover` included: a stop stops a calibration sweep too). `release_stop` clears the latch once
**an exchange after the call shows gGTO clear**: the gripper has been seen to take the halt. It does not wait for activation or for a fault to clear, which a latched stop would make impossible to run.

## Faults

The maker's own: `fault_status.hpp`'s two enumerations and `severity()`, generated into `drivers/fault-vocabulary.json`. A **warning** (action delayed, activation required) and a **minor** fault (over-temperature, no
communication for a second) are `DEGRADED`, and commands are accepted. A **major** one is `FAULT` with `DEGRADED`, and a command is refused naming the code and `recover`: it latches until rACT falls and rises again. The
optional controller's emergency stop input (kFLT `0x0C`) is `ESTOP` with `DEGRADED`. A code the maker does not name is Major, the maker's own rule. The fault word is the whole fault byte (the controller's nibble in the high
half), verbatim.

Motions (`extension`, token `vendor.robotiq.task`): `status`, `activate [timeout]`, `recover [timeout]`, `move position=… [speed] [effort]`, `grip effort=… [speed]`, `release [speed]`.

## What is measured, and how far

| rung | what | result |
|---|---|---|
| 4 | `selftest.py`: the driver against `SimulatedGripper`, written here | 342 checks |
| 4 | `skynet driver check` through python-host against it | 37 passed |
| 4 | the link-chaos contract (`make chaos`): delay, jitter, a slow link, a stall, a blackhole, bursts of loss, truncated and corrupt frames, over a serial relay | 51 checks |
| 4 | the fault-vocabulary contract (`make faultvocab`): the maker's 11 gripper codes and 6 controller codes, and codes outside every table | 124 checks, 31 codes injected, 1 not injectable |
| 4 | defect seeding: `tools/seed-defects/seeds/robotiq.json`, 204 seeded defects | `docs/dry-test-matrix.md` |
| 2 | `layout_check.py`: this driver's bytes against the maker's client, which is run (500 random commands in each of three functions, 1,512 status blocks, the replies, the conversions, the fault tables), and the maker's whole client driving the simulated gripper | 43 checks |
| 2 | `live.py` against the maker's own fake gripper, the maker's client reading the same gripper and a record of every request the maker's server was handed | 56 checks, and the sim half of SCTS through the real ABI 37 passed (`make check-sim-docker`) |

The substrate is **rung 2**: the maker's own fake gripper is on the other end, and the maker's own client is looking on. The maker's fake models **no motion, no object and no fault of its own** (activation completes
instantly and the fingers are wherever they were told), so the physical behaviour here is held to the simulated gripper written in this repository, and a state is staged in the maker's register model through its own
`status()` and `setStatus()` to see what the driver does about each severity.

## What running it found

* **F262 — a gripper power-cycled behind a live driver would be activated by the driver.** The first live run killed the maker's fake and started a fresh one on the same port: the new model, which is a gripper that
  lost its activation with the power, was *activated at once* by the driver's next exchange, because the image the driver held said rACT is set. A real gripper may well do the same, and the sweep it would run is a
  decision for a person. The image is now made again from the gripper's echoes after a line that has been dead, and this is shown by the live block (a fresh model on the same port reads as not activated and the driver
  asks for nothing), by the selftest (a gripper whose power was cut while the gateway was down) and by seeds.
* **F263 — a command that was waiting when the image was made again would have been reported carried.** Found by reading the fix for F262: the exchange that follows a new image carries a later sequence number than
  the waiting command's, so the command would have returned OK with its target overwritten by the echoes. The image carries an epoch, and a command that sees it move is `EIO` and says why.
* **The maker's fake models nothing that takes time**, which is its design and the plan's reason for the simulated gripper written here: with a gripper whose status shows a command a cycle late, or goes on saying activated
  for a while after a reset, a driver that judged a motion's end by gOBJ alone, or set rACT again as soon as it had cleared it, would pass against the maker's fake and fail on the wire. Both are levers on the simulated
  gripper (`echo_lag_s`, `reset_lag_s`) with a check each, and seeds that remove the guard.
* **Not a finding, a decision:** rATR (emergency release) is in the maker's header and in no motion of this driver, on purpose (see *What it refuses to do by itself*).

## What it cannot show

* **A gripper.** The fingers' real dynamics, the grip force (a fraction of a range this driver cannot see, never in newtons), a real workpiece, the motor's heat and the firmware are not reached.
* **That a gripper power-cycled while the link stays up is not activated by the driver.** The image says rACT is set from the first exchange after the power returns; whether the real firmware activates on a rising edge it never
  saw, or needs the falling edge first, is in the manual and not in the SDK. The driver cannot tell the two apart, and says so here.
* **What a stop does to a held workpiece.** Clearing rGTO is the go-to taken away; what the fingers do at that instant is the firmware's, and was not measured.
* **How gOBJ and the echoes lag on a real gripper.** A motion ends on the exchange that shows gGTO, gPR and gOBJ together; a real gripper may show them a cycle apart, and the simulated one has a lever for it (`echo_lag_s`).
* **A 2F-140, or any gripper with other fingers, without the operator's measured profile.** The maker's SDK has none.
* **Timing.** About four milliseconds an exchange on a real adapter is the maker's figure; nothing here measures it.
