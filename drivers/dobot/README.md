# `drivers/dobot` — a Dobot V4 arm (CR, CRA, E6 and Nova) over the controller's TCP interface

`DRY-COVERAGE-PLAN.md` item 12.E.13. A Dobot V4 controller (the CR series, the CRA, the CRV, the Nova and the Magician E6) is a box with two TCP ports that matter here: **the dashboard on 29999**, a text protocol that takes one
command at a time and answers each with `ErrorID,{value},Command(params);`, and **the real-time feedback on 30004**, a 1440-byte packet every 8 ms. Both are in the maker's TCP/IP remote control interface document
(V4.6.6), and the maker publishes a Python client for both (`Dobot-Arm/TCP-IP-Python-V4`, MIT). This driver links nothing of Dobot's: it speaks the protocol, and **the maker's client is run** against a controller written in
this repository, which is the stage's independent evidence (F118's rung 3).

```bash
make selftest          # 750 checks: the frames, the two links, every refusal, how the arm is opened, the state, the commands, the stop in both modes, the faults, the motions and the lines, against a simulated controller on loopback ports (needs Linux)
make check             # and the sim half of SCTS through the real ABI (37 passed)
make fetch-sdk         # Dobot's client (MIT) at a pinned commit, checked against its SHA-256 (DOBOT_VENDOR=where it goes); never committed
make check-layout      # this driver's text and packets against the maker's own client, which is run (344 checks; DOBOT_PYTHON=a python with requirements-layout.txt installed by hash)
make chaos             # the link-chaos contract: the arm's two TCP connections through a relay that delays, stalls and blackholes them (30 checks)
make faultvocab        # the interface document's RobotMode values and the maker's own 341-alarm table, injected and held to the class the table gives each (1436 checks over 359 codes)
make check-sdk-docker  # the client half in a container that installs numpy at DOBOT_NUMPY (2.5.3 pinned by hash; `latest` in the nightly's second run)
```

## What is here

| file | what it is |
|---|---|
| `skynet_dobot/frames.py` | the wire: the command builders, the reply parser, the error codes and RobotMode values of the document, and the feedback packet's layout, decoded and encoded. Pure |
| `skynet_dobot/link.py` | the two connections: a dashboard that matches each reply to its command by the command it echoes, and a feedback stream that reads the newest whole packet and finds its place again by length and self-test field |
| `skynet_dobot/bridge.py` | the driver |
| `skynet_dobot/controller_sim.py` | a simulated controller written here: the dry substrate. Both ports on loopback, the dashboard's state-gated commands, a feedback packet every 8 ms, an arm that follows a `ServoJ` target or a `MovJ` at a speed, alarms, a collision, a software emergency stop, a control mode that is not TCP, and levers for a line that drops, delays, garbles, goes quiet or splits a packet |
| `skynet_dobot/__main__.py` | the entry `drivers/python-host` serves, with `SKYNET_DOBOT_SIM=1` for `skynet driver check` with no arm |
| `selftest.py` | the driver's own suite |
| `layout_check.py` | this driver's text and packets against the maker's client, run, with the client and the driver each reading what the other wrote |
| `chaos.py`, `faultvocab.py` | the subjects for `tools/faultproxy/chaos.py` and `tools/fault-vocab/contract.py` |
| `sim/fetch-sdk.sh`, `sim/Dockerfile`, `sim/run-sdk.sh` | fetch the maker's client (pinned), and run the client half in a container |

## The body

Six revolute joints, `j1` to `j6`, in radians (the controller speaks degrees). It takes a position, one control mode, and its tier ceiling is Guest (§31.3). A position command is one **`ServoJ`**: the controller's own
dynamic-follow call for streaming a target, which "is not affected by the global speed factor but by the speed limits", so how fast the arm follows is the controller's limit and the operator's `servo` parameters, not a
speed this driver invents. The state is the packet's `QActual`, not its `QTarget`; a velocity and an effort are not reported, because the document gives their fields no unit.

```json
{"model": "CR5", "host": "192.168.5.1", "stop_mode": "stop", "simulator": false, "payload_kg": 1.5,
 "limits": [[-6.0, 6.0], [-3.0, 3.0], [-2.5, 2.5], [-6.0, 6.0], [-6.0, 6.0], [-6.0, 6.0]], "servo": {"t": 0.1, "aheadtime": 50, "gain": 500}, "rate_hz": 33}
```

* **`model`** — **required**: one of the document's robot types (`CR5`, `Nova 5`, `Magician E6`, …). The controller reports its own type and the open checks the two agree: a joint's range and a payload's limit depend on
  the arm, so a mismatch is refused by name and nothing is commanded.
* **`host`** (192.168.5.1 from the factory), **`port`** (29999) and **`feedback_port`** (30004; 30005 and 30006 are the same packet every 200 ms and every second). No default host and no scan.
* **`limits`** — **required**: six `[min, max]` pairs in radians. The interface document gives no joint range and the hardware manual's figures are not in anything read here, so the range is the operator's, for this arm
  and this cell. A command outside it is refused whole.
* **`payload_kg`** — **required**: the load on the flange, told to the arm when it is enabled (its gravity compensation and collision detection depend on it). **`stop_mode`** and **`simulator`** — **required**.
  `servo` (`t`, `aheadtime`, `gain`, each held to the document's range; a parameter not given is not sent and the controller's default applies), `roles`, `rate_hz`, `timeout_ms`, `stale_ms`, `open_timeout_ms`. A key the
  driver does not read is refused, naming the keys it does.

## What it never does on its own

It does not take the controller's control mode (`RequestControl()` takes it from whoever has it: a person's decision), it does not power the arm on (about ten seconds, with brakes releasing), it does not enable it
(that releases the brakes, and carries the payload the operator gave) and it does not clear an alarm. Each is a motion (`request_control`, `power_on`, `enable`, `clear_errors`) that a person asks for. A command is refused, naming
the state, until the arm is enabled and idle or running. The open reads `RobotMode()` and, for an arm that can answer it, `GetAngle()`, and refuses an arm whose `GetAngle()` and feedback differ by more than two
degrees: one of them is then in another unit, or the arm is moving fast.

## The stop

`stop_mode` is required and is one of two, because which is the safer is the integrator's risk assessment for this arm, this tool and this payload: **`stop`** is the controller's `Stop()`, which halts the queued motion
and leaves the arm enabled where it is; **`emergency`** is its software emergency stop, `EmergencyStop(1)`, after which the arm is disabled and in alarm and "must have the emergency stop released and the alarm cleared
before it can be enabled again". It is a **software stop**: the arm's own safety circuit is a hardware chain, and there is none on this wire.

`emergency_stop` **latches and returns**. It never touches the line (the RHAL says it never blocks; F298), and a pipe wakes the reading thread, which writes the stop before anything else, and writes it again if the line
did not carry it or the controller refused it. Commands are refused from the instant of the latch, **whatever the line was doing**: a command that ran out of time waiting for the line while the stop was written is
`EPERM`, the stop's, and not `EIO`, a dead line's (F303); a `ServoJ` is not written once the latch is set; and a stop called while an earlier one is being written is owed its own write (F309). `release_stop` clears
the latch only after a reading shows the arm not running (`stop`) or in its alarm state (`emergency`, which is then released with `EmergencyStop(0)` and `ClearError()` and left disabled).

## Faults

The state of the arm is read in every packet and the alarm list (`GetErrorID()`) is read at once when the arm first shows an alarm and twice a second while it stays so. **Fail closed**: an alarm (RobotMode 9, or the
packet's error flag), a collision (11, or the packet's collision flag) and any mode the document does not list are `FAULT` with `DEGRADED`; commands are refused; the fault word is the first alarm of the list (9 for an
alarm whose list has not been read, 11 for a collision, 65536 plus the mode for a mode the document does not list, above any alarm identifier of the maker's table). Three alarms are the maker's own words for a stop
(116 the emergency stop button, 132 the software emergency stop, 133 the user emergency stop) and are `ESTOP`. The modes in which an arm takes no command and is not in fault (initialising, brake open, powered off,
disabled, drag, paused) are `DEGRADED` with the command refused, naming the motion that changes the state.

Motions (`extension`, token `vendor.dobot.task`): `status`, `request_control`, `power_on`, `enable`, `disable`, `clear_errors`, `move <joint>=<rad> … [v=<percent>] [timeout=<s>]`.

## What is measured, and how far

| rung | what | result |
|---|---|---|
| 4 | `selftest.py`: the driver against `SimulatedController`, written here, on loopback ports | 750 checks |
| 4 | `skynet driver check` through python-host against it | 37 passed, 0 failed, 1 not measured, 3 not applicable; the evidence supports guest |
| 4 | the link-chaos contract (`make chaos`): delay, jitter, a slow link, a stall and a blackhole on both connections | 30 checks |
| 4 | the fault-vocabulary contract (`make faultvocab`): the document's RobotMode values and the maker's 341 alarms | 1436 checks, 359 codes injected |
| 3 | `layout_check.py`: the maker's own client, run, against the simulated controller while this driver reads and writes the same one | 344 checks |
| 4 | the seeded-defect campaign (`tools/seed-defects`) | 1,013 seeds authored (`tools/seed-defects/seeds/dobot.json`); the first campaign was still running and its figure is not recorded here yet |

What `layout_check.py` compares, with the maker's client on one side and `frames.py` on the other: every field of the feedback packet at the offset the client's numpy dtype gives it and the packet's size; random
packets made by the client's dtype read by this driver and random packets made by this driver read by the client's parser, field by field; every command this driver sends, byte for byte as the client writes it (random
angles and parameters); and the two together: the client connects to the simulated controller, takes control, clears, enables, moves it with `MovJ` and streams `ServoJ` at it while this driver, opened on the same
controller, reads where the client put it and moves it for the client to read.

## What running it found

* **F311 — a stop that woke the reading thread spent the period's slot** (found by a `release_stop` at 1 Hz in LEAP's suite, and fixed in OnRobot's, LEAP's and Inspire's; this driver's thread waits on the
  feedback socket and a pipe, and has no slot to spend): the cycle a stop kicked took the place of the next scheduled one, so the reading after a stop was a second or two away.
* **F312 — the maker's client can never read an alarm list**: in `error_controller.parse_error_ids` a comment swallowed the line that assigns `start`, so every call is a `NameError` that the function catches and answers with
  an empty list. An arm in alarm is an arm with no alarms to the client's own error monitor. `layout_check.py` pins the behaviour on random lists and compares this driver's parser with the document.
* **F313 — RobotMode 11**: the document calls it a collision and the client calls it `JOG`, and the client adds a 0 `UNKNOWN` that the document does not have. This driver reads 11 as the document does, a fault.
* **F314 — the client cannot send a load of zero**: `EnableRobot(0.0)` becomes `EnableRobot()`, which leaves the controller's own payload. This driver sends the load it was given.
* **F315 — the client reads the robot type as a signed byte**, so the models numbered above 127 (the CRV series) are negative numbers to it. This driver reads it unsigned.
* **F316 — the client always sends `ServoJ`'s three optional parameters**, with the document's defaults, where this driver sends only the ones the operator set.
* **F317 — a RobotMode the document does not list was read as merely degraded** (found by the fault-vocabulary contract's probes, 0, 12 and 255): an unknown state of the arm is not a healthy one, so it is now a
  `FAULT` with commands refused, and its word is 65536 plus the mode, above any alarm the maker's table has.
* **F318 — an arm in alarm whose alarm list had not been read had a fault word of zero**, which is no fault: the word is now 9 (the mode) until the list is read, and the maker's alarm table starts at 16, so the two cannot be taken for each other.

## What it cannot show, and the assumptions it makes

* **An arm or a controller.** The simulated controller was written from the interface document and from what the client sends, and it is accepted by the client because it was made to be (F15). What it shows is the
  driver's own transaction budget, reading cycle, staleness bound and stop path, and the driver's bytes against the maker's client's, and nothing about Dobot's firmware. No Dobot arm and no Dobot controller software was run.
* **Which RobotMode a real stop leaves.** The document gives `Stop()` and `EmergencyStop` and says nothing of the mode after them beyond the alarm; the driver reads the arm's state and does not assume it.
* **The safety state byte.** The document gives the packet's `SafetyState` byte (offset 1420) no bit table in anything read here, so it is decoded and not used; a stop is read from the alarm list, where the maker names it.
* **The servo alarm table.** The maker's client carries a second table for the drives (`alarmServo.py`); whether `GetErrorID()` lists a servo's identifiers beside the controller's is not said, so an identifier that
  is in no table the driver reads is a fault (the probes), and which servo raised it is not known.
* **Units.** The document gives the feedback and `GetAngle()` in degrees; the open checks the two agree. The speed `ServoJ` follows at is the controller's own limit.
* **Assumption: the 33 Hz period, the 80 ms transaction, the 0.5 s staleness bound, the 0.5-degree arrival tolerance and the two-degree units check.** None is the maker's.
* **Licence.** The maker's client is MIT; the driver fetches it only to run it, pinned by commit and checked by SHA-256, and commits none of it.
* **The link-chaos `reset` profile is not run**, as in OnRobot's TCP link: a connection the controller drops is made again within half a second while the controller is still there, so the link carries data throughout and
  "outage reported" has no outage to report. `selftest.py` holds what the reset would: that a dropped connection is made again and the arm read and commanded again, that a controller which accepts and will not talk is
  retried at most twice a second, and that one which is not listening is reported as a line that could not be opened. A persistent outage is the blackhole's.
