# `drivers/leap` — a LEAP Hand (sixteen Dynamixel servos on one bus) over protocol 2.0

`DRY-COVERAGE-PLAN.md` item 12.E. A LEAP Hand (Kenny Shaw, Deepak Pathak, CMU) is sixteen ROBOTIS Dynamixel servos in four fingers and nothing else: **the *Lite* has sixteen XL330-M288-T, the *Full* sixteen
XC330-M288-T, and neither has a controller of its own**. The host speaks Dynamixel protocol 2.0 to the servos through a USB serial adapter (4 Mbaud from the hand's authors) and every byte of "the hand" is a byte of
"the servos". So there are two kinds of fact in this driver and the code says which is which:

* **the servos' are ROBOTIS's**: the packet format (header, length, CRC-16 of polynomial 0x8005, byte stuffing), the instructions, the status packet's error byte, and the X-series control table. ROBOTIS publishes the
  client every program for these servos is built on, `dynamixel_sdk` (Apache-2.0, on PyPI), and **it is run** against a hand written in this repository: that is the stage's independent evidence, F118's rung 3.
* **the hand's are its authors'**: which servo is which joint (ids 0 to 15, the index finger first, each finger from its side-to-side MCP to its fingertip), the joint ranges, and the 3.14159 rad offset between the
  servo's angle and the hand's. They are physical facts about the hand, they are cited to `leap_hand_utils.py`, and **nothing independent holds them in CI** (see *The authors' own client*).

```bash
make selftest          # 703 checks: the frames, every refusal, how the hand is brought up, the exchange cycle, the stop, the faults, the motions and the line, against a simulated hand on a pseudo-terminal (needs Linux)
make check             # and the sim half of SCTS through the real ABI (37 passed)
make fetch-vendor      # LeRobot's X-series control table (Apache-2.0) at a pinned commit, checked against its SHA-256 (LEAP_VENDOR=where it goes); never committed
make check-layout      # this driver's wire and registers against ROBOTIS's own SDK, which is run (63 checks; LEAP_PYTHON=a python with requirements-layout.txt installed by hash)
make chaos             # the link-chaos contract: the hand's serial line through a relay that delays, stalls and blackholes it (51 checks)
make faultvocab        # the Hardware_Error_Status bits the e-manual defines, injected and held to the class the table gives each (48 checks over 12 codes)
make check-sdk-docker  # all of the SDK half in a container that installs ROBOTIS's SDK at DYNAMIXEL_SDK (4.1.0 pinned by hash; `latest` in the nightly's second run)
```

## What is here

| file | what it is |
|---|---|
| `skynet_leap/frames.py` | the wire and the registers: protocol 2.0 packets, the CRC and the stuffing, the control table, the hand's joint order, ranges and offset, the unit conversions. Pure |
| `skynet_leap/link.py` | the line: a serial port (a USB adapter, raw 8N1) or a TCP socket (a gateway that forwards the line), a transaction with a bound, the bytes of an earlier answer discarded |
| `skynet_leap/bridge.py` | the driver |
| `skynet_leap/hand_sim.py` | a simulated hand written here: the dry substrate. Sixteen servos on a pseudo-terminal that cross their range at a speed, an object that stops a closing finger and loads its motor, a hardware torque off, a power cycle that gives back the RAM, and a lever for every way a line can go wrong. It answers the instructions ROBOTIS's SDK sends, fast synchronised read included |
| `skynet_leap/__main__.py` | the entry `drivers/python-host` serves, with `SKYNET_LEAP_SIM=1` for `skynet driver check` with no hand |
| `selftest.py` | the driver's own suite |
| `layout_check.py` | this driver's bytes and registers against ROBOTIS's SDK, run, and against LeRobot's control table |
| `chaos.py`, `faultvocab.py` | the subjects for `tools/faultproxy/chaos.py` and `tools/fault-vocab/contract.py` |
| `sim/fetch-vendor.sh`, `sim/Dockerfile`, `sim/run-sdk.sh` | fetch LeRobot's table (pinned), and run the SDK half in a container |

## The body

Sixteen revolute joints in the authors' order: **`index_`, `middle_`, `ring_` and `thumb_`**, each `mcp_side`, `mcp_forward`, `pip`, `dip`. A position is the joint's angle in the authors' "sim" convention (0 is the
fully extended, allegro-like zero; closing is positive), in radians; the servo's own angle is that plus 3.14159. It takes a position, one control mode, and its tier ceiling is Guest (§31.3). The servos run in
**current-based position control (operating mode 5)**: a goal position, capped by a goal current that is the operator's.

```json
{"variant": "full", "device": "/dev/ttyUSB0", "baud": 4000000, "current_ma": 350, "gains": {"kp": 600, "ki": 0, "kd": 200}, "side_scale": 0.75, "stop_mode": "hold", "simulator": false, "rate_hz": 50}
```

* **`variant`** — `lite` or `full`, **required**. Nothing the bus says tells a hand's builder which it is, the maker suggests a different current for each, and a register can mean something else on another servo, so
  the operator says which it is and the open checks all sixteen servos report that variant's model number (1200 and 1240, ROBOTIS's bundle listings). A hand that disagrees is refused by name and nothing is commanded.
* **`device` and `baud`** (a USB adapter, 4000000 from the hand's authors) or **`host` and `port`** (a gateway) — exactly one of the two. No default and no scan.
* **`current_ma`** — **required**: the goal current that caps how hard every servo pushes toward its position. Nothing in a position request says how hard the fingers may press. The most this driver takes is the
  maker's suggestion for the variant (350 mA for a Lite, 550 for a Full), and never more than a servo's own Current Limit register.
* **`gains`** — **required**: `kp`, `ki`, `kd` of the position loop (the authors' example sets 600, 0 and 200). `side_scale` is the fraction of P and D the three side-to-side MCP joints take (the example uses 0.75).
* **`stop_mode`** (only `hold`) and **`simulator`** — **required**. `limits` (may only narrow a joint, never widen it), `roles`, `rate_hz`, `timeout_ms`, `stale_ms`, `open_timeout_ms`. A key the driver does not read is
  refused, naming the keys it does.

## The stop

`stop_mode` accepts only `hold`: every servo's own present position, read fresh, written back as its goal, torque left on. A stop that cut torque would let a hand drop whatever it holds, and what a stop should do to a
held object is the integrator's risk assessment. It is a **software stop**: the servos have no stop input.

`emergency_stop` **latches and returns**. It never touches the line, because the RHAL says it never blocks (50 ms) and a line that is slow or dead would make a stop that wrote its own hold wait for it (Inspire's
driver was found to do that, F298). The reading thread writes the hold before anything else, between two of its own reads if it has to, and a call that comes while an earlier hold is being written is owed a hold
read after it (the defect Inspire's driver had, F309). Commands are refused from the instant of the latch, **whatever the line was doing**: a command that ran out of time waiting for the line while the hold was being
written is `EPERM`, the stop's, and not `EIO`, a dead line's (the race Inspire's and OnRobot's drivers had, F303). `release_stop` clears the latch only after an exchange shows the servos' own record of their goals equal to what the stop wrote.

## One cycle

Three synchronised reads: torque and hardware error (64 to 70), the gains and goal current (80 to 103), and the goal, motion and present state (116 to 135), checked servo by servo. `write_command` checks every
applied joint before it writes any (a command refused for one joint is refused whole, F281), makes one synchronised write of the goals it applies and only those, and is bounded by a quarter of a second *including the
wait for the line*. The servos' own record of their goals is read every cycle and compared with what was written: a goal acknowledged on the wire and then not taken is a command that did not happen (F175). A line that
was dead for three exchanges is not a hand that was left as this driver last asked (it may have been power-cycled, which gives the gains and the goal current back to their defaults), so the operator's values are
written again.

## Faults

The e-manual's `Hardware_Error_Status` byte: input voltage (bit 0), overheating (bit 2), electrical shock (bit 4) and overload (bit 5) (`fault-vocabulary.json`, `leap`). **Fail closed**: any bit, named or not, is `FAULT`
with `DEGRADED`, commands are refused, the fault word is `(servo + 1) << 16 | the byte` of the lowest-numbered servo, and the hand does not move until it is cleared. The `clear_errors` motion reboots the servos that
hold one, which drops their torque and their gains, and so needs `confirm=limp`; `enable_torque` then sets each limp servo's goal where it is *before* its torque goes on, so switching it on moves nothing (the class F121 names: a hold that means limp).

Motions (`extension`, token `vendor.leap.task`): `status`, `move <joint>=<rad> … [timeout]`, `enable_torque`, `clear_errors confirm=limp`.

## What is measured, and how far

| rung | what | result |
|---|---|---|
| 4 | `selftest.py`: the driver against `SimulatedHand`, written here, on a pseudo-terminal | 703 checks |
| 4 | `skynet driver check` through python-host against it | 37 passed, 0 failed, 1 not measured, 3 not applicable; the evidence supports guest |
| 4 | the link-chaos contract (`make chaos`): delay, jitter, a slow link, a stall, a blackhole, bursts of loss, truncated and corrupt packets | 51 checks |
| 4 | the fault-vocabulary contract (`make faultvocab`): the e-manual's bits, and codes outside them | 48 checks, 12 codes injected |
| 4 | defect seeding: `tools/seed-defects/seeds/leap.json`, 926 seeded defects: **890 of 890 noticed** (32 equivalent mutants left out, each with its reason; four controls that survive as they must) | `docs/dry-test-matrix.md` |
| 3 | `layout_check.py`: ROBOTIS's own SDK, run, against the simulated hand while this driver reads and writes the same one | 63 checks |

What `layout_check.py` compares, with `dynamixel_sdk` 4.1.0 on one side and `frames.py` on the other: the CRC and the byte stuffing on random buffers (with `FF FF FD` runs in them); every request this driver sends,
byte for byte, as the SDK builds it; every answer this driver reads, through the SDK's `rxPacket` and this driver's `parse_status`, field by field, with a bad CRC, a truncation and noise before the header; the register
map against LeRobot's `X_SERIES_CONTROL_TABLE`; and the two together: this driver brings the simulated hand up and commands it and the SDK reads back what it did to the registers, through `GroupSyncRead` and `fastSyncRead`;
then the SDK moves the hand with `GroupSyncWrite` and this driver reads where it went and does not move the hand it finds.

## The authors' own client, and the licence

The hand's authors' repository (`leap-hand/LEAP_Hand_API`) is **Creative Commons Attribution-NonCommercial 4.0**. **Nothing here fetches it, vendors it or runs it by default**, and nothing in CI does: `LEAP_API` is a
person's opt-in (`make check-layout LEAP_API=/path/to/checkout`), and with it `layout_check.py` runs the authors' `dynamixel_client.py` and `leap_hand_utils.py` against the simulated hand and holds the joint ranges, the offset
and the unit scales to them (the fifty-ninth session ran this locally once: 75 checks in all, 12 more than without). **Whether a non-commercial licence is acceptable in this repository's CI is the owner's decision**, not
an agent's, and until it is made the hand's joint order, ranges and offset have no independent check in the pipeline: they are the authors' numbers as this driver's author read them.

## What running it found

* **F304 — LeRobot's table has no XC330-M288**, the Full's servo (it has the XC330-T181 and XC330-T288). The Full's model number, 1240, is the e-manual's and ROBOTIS's bundle listing's, and nothing in this tree
  confirms it from a second source.
* **F305 — the authors' client cuts a goal to a whole tick toward zero** where this driver rounds to the nearest: a goal differs by at most one tick, and by one tick for some of the angles `layout_check.py` draws (it counts them).
* **F306 — the authors' client scales a Present_Current unit by 1.34 mA**, which is another servo's figure; the e-manual says 1.0 mA for the XC330-M288 and XL330-M288, and this driver reports 1.0.
* **F307 — ROBOTIS's Python SDK 4.1.0 sends `FF FF FD` in a packet's data unstuffed**: `txPacket` ignores what `addStuffing` returns, so a goal whose bytes contain that run goes out with the length unchanged and no
  stuffing byte, which the e-manual says must follow it. This driver stuffs it; `layout_check.py` pins the disagreement and the byte string that shows it.
* **F308 — the SDK's `removeStuffing` mis-reads data such as `FF FF FD FF FD FD`** after an earlier removal (it looks two bytes back in the buffer it is writing, not in the data it is reading). This driver reads such
  a block whole; the check counts the cases (195 of 3,000 random status packets) where the SDK does not.
* **F301 — `termios.error` is not an `OSError`**: a serial port the kernel will not configure raised out of `open` as an exception instead of returning a code (the same defect in three links). It is `EIO` and says
  what the kernel said.
* **The status parser skips what is not a status packet** (an echoed request, noise that starts `FF FF FD 00`) as the SDK does; a half-duplex adapter hears its own request.

## What it cannot show, and the assumptions it makes

* **A hand.** The servos' real control loop (a servo here moves toward its goal at a fixed speed), the current a real servo draws, its temperature, a finger's real dynamics, and what a closing finger does to an object
  are not reached. The simulated hand is accepted by ROBOTIS's SDK because it was made to be (F15), and agrees with whoever wrote it.
* **The hand's own mapping.** The joint order, the ranges and the 3.14159 offset are the authors' (see above) and no independent software holds them in CI.
* **Assumption: a servo within 12 ticks (about 1 degree) of its goal is there.** The e-manual gives no figure for how close a servo in current-based position control rests; the 12 is this driver's own.
* **Assumption: the 50 ms byte timeout, the 50 ms transaction and the 50 Hz period.** None is the maker's. A USB adapter's latency timer is commonly 16 ms and the hand's authors ask for 1 ms.
* **Firmware.** The control table is the e-manual's for the XC330-M288-T and XL330-M288-T at the firmware ROBOTIS documents; a servo with other firmware is a hand this driver would read by the same registers.
* **The Lite's current limit** in the simulator is its own figure (1750 mA); the e-manual's default for the XC330-M288 (2352 mA) is the Full's.
