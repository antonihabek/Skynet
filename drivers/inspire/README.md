# `drivers/inspire` — an Inspire RH56 dexterous hand over its private serial protocol

`DRY-COVERAGE-PLAN.md` item 12.E.8. Inspire Robots' RH56 hands (the series manual's DF and BF builds, and the RH56DFTP with tactile sensors) have six actuators, one per finger and two for the thumb, and one register map
that can be reached three ways: the **RS-485 private protocol** (`EB 90` frames, a byte checksum), Modbus RTU and Modbus TCP. Inspire publish a manual for each of the two families and a PC tool. They publish **no SDK, no
simulator and no client**. This driver speaks the private protocol, links nothing of the maker's, and was written from the two manuals (the series manual `PRJ-02-TS-U-001` V1.0.9 and the DFTP manual `PRJ-02-TS-U-010` V1.0.0,
downloaded from the maker's own store links; `vocab_gen.py` checks their SHA-256). It is held to **the manuals' own four worked frames** (pinned as literals in `selftest.py`) and to **two open-source clients of the same
protocol that other people wrote from the same manuals** (`pyrh56` 0.3.0 and `inspire-rh56dftp` 0.1.0, MIT, PyPI). The hand on the other end is written in this repository. It is not a hand, and nothing the maker has
written is on the other end of the line, so the stage is **alpha**.

```bash
make selftest          # 491 checks: the manuals' worked frames, the register map, every refusal, the exchange cycle, the stop, the faults, the motions and the line, against a simulated hand on a pseudo-terminal (needs Linux)
make check             # and the sim half of SCTS through the real ABI (37 passed)
make check-layout INSPIRE_PYTHON=/path/to/venv/bin/python   # this driver's bytes against pyrh56 and inspire-rh56dftp, which are run (43 checks); pip install --require-hashes -r requirements-layout.txt
make chaos             # the link-chaos contract: the hand's serial line through a relay that delays, stalls, loses, truncates and corrupts it (51 checks)
make faultvocab        # the maker's own error bits and status codes, injected and held to the class the table gives each (96 checks over 24 codes)
make vocab INSPIRE_SERIES_PDF=… INSPIRE_DFTP_PDF=…   # prints the `inspire` entry of drivers/fault-vocabulary.json from the two manuals' own tables
```

## What is here

| file | what it is |
|---|---|
| `skynet_inspire/frames.py` | the wire: the frames, the checksum, the register map, the two models' profiles, the status codes and error bits. Pure, every constant cited to a manual section |
| `skynet_inspire/link.py` | the line: a serial port or a TCP socket (an RS-485 gateway that forwards the line raw), with the echo of a half-duplex adapter and stale bytes skipped |
| `skynet_inspire/bridge.py` | the driver |
| `skynet_inspire/hand_sim.py` | a simulated hand written here: the dry substrate. Finger motion at the manual's speed, objects that stop a closing finger at its force limit, the faults, the force calibration, the clear-error semantics, and a lever for every way a line can go wrong |
| `skynet_inspire/__main__.py` | the entry `drivers/python-host` serves, with `SKYNET_INSPIRE_SIM=1` for `skynet driver check` with no hand |
| `selftest.py` | the driver's own suite |
| `layout_check.py` | this driver's bytes against the two independent clients, which are run, and against the manuals' own frames |
| `chaos.py`, `faultvocab.py` | the subjects for `tools/faultproxy/chaos.py` and `tools/fault-vocab/contract.py` |
| `vocab_gen.py` | reads the two manuals' error and status tables (`pdftotext`), checks they agree, and writes the `inspire` entry of `drivers/fault-vocabulary.json` |
| `requirements-layout.txt` | the three wheels `layout_check.py` imports, with their hashes |

## The body

Six revolute joints, **`pinky`, `ring`, `middle`, `index`, `thumb_bend`, `thumb_rotation`**, in the order of every register group of the manuals. A position is the angle the hand's ANGLE registers stand for, in radians,
between the two ends of the range the model's manual gives. **Nothing the hand says identifies its model**, and the two manuals give different ranges, speed and force for it (F295), so `model` is the operator's claim and
has no default: `RH56`, `RH56DFTP`, or `custom` with a `profile` somebody measured. It takes a position, one control mode, and its tier ceiling is Guest (§31.3).

```json
{"device": "/dev/ttyUSB0", "baud": 115200, "hand_id": 1, "protocol": "private", "model": "RH56", "stop_mode": "hold", "speed": 0.5, "force_g": 300, "simulator": false,
 "limits": {"index": [0.4, 2.0]}, "roles": {"index": "right_hand.finger.flex#0"}, "rate_hz": 20}
```

* **`device`** or **`host` and `port`** — the hand's RS-485 adapter, or a gateway that forwards its line raw. No default and no scan.
* **`protocol`** — **required, and only `private` is accepted.** Modbus RTU and Modbus TCP are refused *by name*: the manuals say the registers are addressed by byte, and **give no byte order for the byte registers
  (ERROR, STATUS, TEMP) when they are carried in sixteen-bit Modbus registers**. A driver that read the fault bytes that way would be guessing at the one thing it must not guess.
* **`model`**, **`stop_mode`** (only `hold`), **`speed`** (a fraction above 0 and at most 1 of SPEED_SET 1000), **`force_g`** (grams, one number or six, up to the model's maximum) and **`simulator`** — **all required**:
  how fast the fingers close and how hard they hold is the operator's, and nothing in a position request says it.
* `limits` (narrower than the profile's, never wider), `roles`, `hand_id` (1 from the factory), `baud` (one of the manuals' four speeds, 115200 from the factory), `rate_hz` (20, at most 50), `timeout_ms`, `stale_ms`,
  `open_timeout_ms`. A key the driver does not read is refused, naming the keys it does.

## What it never sends

* **-1 in ANGLE_SET.** The manuals give it as "no response", and give thumb rotation a range of 0 to 1000 with none. A joint a command does not apply is not written: a write covers a run of applied joints and no more.
* **SAVE, RESET_PARA, HAND_ID, REDU_RATIO, the IP registers, the action sequences.** They write the hand's flash or change how it is reached: a person's, at the bench.
* **A stop the manual does not document.** A speed of zero, which both clients use, is in neither manual and is not offered.

## The stop

The manuals document no stop. **`hold`**: the hand's own ANGLE_ACT, read fresh and written back into ANGLE_SET, which the manual defines as "the finger will move to the preset angle", so it is a target where the finger
already is. It leaves every finger where it is and does not release a grasp; what a stop should do to a held part is the integrator's risk assessment. It is a **software stop**: the hand has no documented e-stop signal.

`emergency_stop` **latches and returns**. It never touches the line, because the RHAL says it never blocks (50 ms) and a line that is slow or dead would make a stop that wrote its own hold wait for it: the link-chaos
contract found 101 ms behind a blackhole (F298). The reading thread writes the hold before anything else, between two of its own reads if it has to, and as soon as the line carries it. Commands are refused from the instant
of the latch, and `status` says whether the hold has been delivered. `release_stop` clears the latch only after an exchange shows the hand's own record of its targets equal to what the stop wrote.

## One cycle

Three reads, whatever has or has not changed: ANGLE_ACT (12 bytes); the 48-byte span 1486..1533 (ANGLE_SET, FORCE_SET and SPEED_SET: the hand's **own record of what it was told**, which shows a write that was
acknowledged and not taken, and a power cycle that gave the speed and force back to their defaults); ERROR and STATUS (adjacent, so one read). The maker documents no keep-alive and the driver sends none. A hand whose
speed or force differ from the operator's is told them again. The image of the targets is **seeded from the hand's own ANGLE_SET** at open, so connecting never moves a hand that is already holding something.
`write_command` validates every applied joint before it sends any (a command refused for one joint is refused whole, F281), writes one transaction per run of neighbours, and is bounded by a quarter of a second *including
the wait for the line* (F299).

## Faults

The maker's own: the series manual's five error bits and seven status codes, generated by `vocab_gen.py` into `drivers/fault-vocabulary.json`. **Fail closed**: any error bit, a status of 5, 6 or 7 (the manual's
"locked rotor", "over-current" and "force-limit" stops), or a status or bit the tables do not name, is `FAULT` with `DEGRADED`, and commands are refused naming the joint and the code. The fault word is
`(joint + 1) << 16 | status << 8 | error` of the lowest-numbered faulted joint. The `clear_errors` motion writes CLEAR_ERROR, which the manual says clears all but the over-temperature error (that one clears when the
actuator cools). The DFTP manual has **no status-code table at all** (F295); its hand is read with the series manual's.

Motions (`extension`, token `vendor.inspire.task`): `status`, `move <joint>=<rad> [timeout]`, `clear_errors`, `calibrate_force confirm=unloaded` (the force calibration moves the fingers by itself for about six seconds, and
the hand must hold and touch nothing while it does, so it needs a person's word for it; a stop ends this driver's wait and not necessarily the calibration, which the response says).

## What is measured, and how far

| rung | what | result |
|---|---|---|
| 4 | `selftest.py`: the driver against `SimulatedHand`, written here | 491 checks |
| 4 | `skynet driver check` through python-host against it | 37 passed |
| 4 | the link-chaos contract (`make chaos`): delay, jitter, a slow link, a stall, a blackhole, bursts of loss, truncated and corrupt frames, over a serial relay | 51 checks |
| 4 | the fault-vocabulary contract (`make faultvocab`): the five error bits and seven status codes, and codes outside every table | 96 checks, 24 codes injected |
| independent | `layout_check.py`: 500 random reads, 500 batch writes and 200 byte writes byte for byte against `pyrh56`; the maker's four worked frames; mock answers through both clients' parsers; the register map against both; and `pyrh56`'s own `RH56Driver` driving the simulated hand while this driver reads the same hand | 43 checks |

The independent clients are **transcriptions of the manual by other people**, and they are what the substrate is, in place of the maker's. Where they agree with this driver the bytes are not one reading of the manual
(three readings of it agree); where they disagree with the manual, `layout_check.py` pins the disagreement as a finding and does not follow either.

## What running it found

* **F295 — the two manuals describe two different hands under one register map.** Ranges (a finger 19 to 176.7 degrees in one, 20 to 176 in the other; the thumb's bend -13 to 53.6 against -13 to 70), full-range time
  (0.8 s against 0.6 s), force (1000 g against 3000 g), the signedness of FORCE_ACT, and the DFTP manual has no status-code table. A driver with one table would be wrong on one of them, so `model` is required.
* **F296 — the DFTP client's default IP is not the manual's** (192.168.123.211 against 192.168.11.210). The driver speaks the private serial protocol and has no IP; pinned in `layout_check.py`.
* **F297 — `pyrh56` clamps the thumb's bend to 200..700 registers; neither manual says so.** The driver does not follow it and uses the manual's 0 to 1000; pinned in `layout_check.py`.
* **F298 — `emergency_stop` waited on the line** (101 ms behind a blackhole against the RHAL's 50 ms), found by the link-chaos contract the first time it ran. The stop latches and returns; the reading thread writes the hold.
* **F299 — a command waited on the line's lock past its own bound** (301 ms behind a blackhole against 250 ms): the reading thread holds the lock for a transaction, and a command that re-asserted the speed and force after
  a dead line waited for it twice. Every wait in a command is now bounded by the one deadline, and the contract holds.
* **A command that lost a race with the stop was `EIO` or `EPERM` by the scheduler's choice** (found as a flake in the busy-line check). The reading thread holds the line's lock while it writes the stop, and a command that
  ran out of its quarter of a second waiting for the lock was answered `EIO`, a dead line's, whether or not the stop was the reason. The latch decides now: a command that times out while the stop is latched is `EPERM`.
  The check is deterministic, with a line held busy while the stop is and is not latched, and was shown red without the fix; the same defect was in `drivers/onrobot` and is fixed there.
* **F301 — `termios.error` is not an `OSError`.** A serial port the kernel will not configure raised out of `open` as an exception instead of returning a code; the same `except` was in three links (this one, OnRobot's and
  Robotiq's). All three raise `OSError` now and the host's `open` returns `EIO`; each selftest has the check and ran red without the fix.
* **Manual typos, recorded and not followed:** ERROR(3) is labelled "thumb" in the series manual, ANGLE_SET is described as the "initial power-on angle", ANGLE_ACT points at "1.7DEFAULT_ANGLE_SET", and the DFTP manual
  gives two Modbus write limits (0x78 and 123).

## What it cannot show, and the assumptions it makes

* **A hand.** The fingers' real dynamics, the tactile sensors (the DFTP's are not read at all), the fingertip force, the motors' heat and the firmware are not reached.
* **Assumption: the register scale is linear between the two ends the manual gives.** The manuals call the registers dimensionless and give the physical angle at 0 and at 1000 only. A hand that is not linear is read
  with a systematic error in every angle, and the `status` motion says the mapping is assumed.
* **Assumption: register 1000 is the larger angle for every joint.** The maker states that fully open is 1000 for the fingers; the thumb's orientation is not stated, and was taken to follow the same order.
* **Assumption: homing is not required**, so the hand reports `HOMING_NOT_REQUIRED`. The manuals describe a force calibration and no homing.
* **Assumption: a finger that the status calls "arrived" is believed to be within 50 registers (5%) of its target.** The manuals give no figure; a hand that rests further off is a move that ends in a timeout naming
  the angles, never a success the angles do not show.
* **Modbus RTU and Modbus TCP** are refused (above), and so is the CAN bus the DFTP manual describes: nothing here speaks it.
* **Timing.** The 50 ms byte timeout and the 80 ms transaction are this driver's own; the maker gives neither.
