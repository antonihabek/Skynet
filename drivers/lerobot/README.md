# skynet-lerobot — the LeRobot bridge

§16 and §67. `Robot.connect / get_observation / send_action` is very nearly the simplest possible
RHAL, so wrapping it means **every robot LeRobot supports becomes a Guest-tier Skynet robot for the
cost of one driver**.

```bash
python selftest.py          # sim conformance against a fake robot — 111 checks, no hardware
make check-wire PYTHON=...  # LeRobot's own SO-101 (Feetech) and Koch and OpenMANIPULATOR-X (Dynamixel) classes on emulated buses — 44 and 68 checks
                            # (and the emulators' own 18 and 27 tests), needs `lerobot[feetech,dynamixel]`
make faultvocab PYTHON=...  # the fault-vocabulary contract: every error bit of both servo families through the bridge's read and write — 68 checks

# and through the OS, as a loadable driver:
cd ../python-host && make check
```

## The mapping (§67.1)

| RHAL | LeRobot |
|---|---|
| `open(config)` | `robot.connect()` |
| `read_state` | `robot.get_observation()` — proprioception split from camera frames |
| `write_command` | `robot.send_action()` |
| `close` | `robot.disconnect()` |
| `describe` | Built from `observation_features` / `action_features` |
| `read_sensor` | The camera entries of `get_observation()` |
| `emergency_stop` | Torque-disable on the motor bus |

## Roles are the point (§67.2)

The SO-100/101 motor names line up with the canonical vocabulary almost one to one, and a table per
robot class is a few dozen lines:

```
shoulder_pan   → right_arm.shoulder.yaw      wrist_flex  → right_arm.wrist.flex
shoulder_lift  → right_arm.shoulder.pitch    wrist_roll  → right_arm.wrist.roll
elbow_flex     → right_arm.elbow.flex        gripper     → right_hand.gripper.grip
```

Two things about that table are worth knowing before you trust it.

**The side is a convention, not a fact.** An SO-101 is a single arm; nothing about it is left or
right, and the specification's own worked example writes `right_arm`. On a bimanual rig one of the
two is wrong, and `role_overrides` in the driver configuration is how you fix it.

**An unmapped robot class gets no roles at all** — not plausible ones. A joint with no role can only
ever be matched by exact name, which is truthful: nobody has established what its joints mean. Add a
table, or supply overrides.

## Four things this bridge does not know, and says so rather than inventing

1. **Joint ratings.** LeRobot does not publish position, velocity or effort limits through the
   `Robot` interface — they live in the motor bus configuration and the calibration file, in raw
   encoder units. So `joints[].limits` is **empty**, and the transport planner reports every channel
   bound here as `unscalable_channel` rather than scaling against a guess. Understand this before it
   surprises you: a body with no ratings is a body a policy cannot be safely scaled onto.
2. **The safety envelope.** Null, always.
3. **Acquisition time.** `get_observation()` supplies none, so the bridge stamps at read (§47.2, the
   estimated-latency path). That stamp is *after* acquisition by an unknown amount, which inflates
   every latency budget in the safe direction. The descriptor's provenance records it.
4. **Units.** LeRobot's `.pos` values are normalised or in degrees depending on the robot and its
   calibration; the RHAL is radians. The bridge converts only when told (`"unit": "deg"`), and
   otherwise passes values through and **declares in the descriptor that it did**. Treating degrees
   as radians is a 57× error on every command, and it looks like a tuning problem for a day.

## The ceiling, stated plainly (§67.3)

Python caps this at **Guest**. The bridge cannot host an enforcing kernel and never claims to — the
host refuses to load a Python driver that announces anything else. What it still delivers is most of
why anyone would adopt early: the package system, skills with enforced preconditions, capability
grants, fleet reconciliation, the black box, episode capture, and advisory violation reporting that
tells an operator what a policy *would* have commanded before it is trusted with anything.

## Written from documentation — and now run against LeRobot itself

§62.2 is explicit that these drivers are written from public documentation. For a long time the consequence was that the bridge had
been exercised against a faithful fake (`skynet_lerobot/fake.py`) and **not** against LeRobot: `selftest.py` showed the adapter was
internally consistent and nothing about whether its field names matched the library's. That is the half `selftest_wire.py` closes
(`make check-wire`, DRY-COVERAGE-PLAN.md items 12.D and 12.E.5):

```bash
# an interpreter with `lerobot[feetech,dynamixel]` (LeRobot 0.6.1 is the release this was measured on)
make check-wire PYTHON=~/lerobot-venv/bin/python        # or: make check-wire-venv, which makes the venv
```

It opens **`lerobot.robots.so_follower.SO101Follower` as shipped**, the way production does (`open_bridge`: a JSON configuration, LeRobot's
own config registry, `make_robot_from_config`), with LeRobot's own `FeetechMotorsBus` and Feetech's own `scservo_sdk`, on a pseudo-terminal
that `sim/feetech_bus.py` answers as a bus of six STS3215 servos. Nothing on LeRobot's side is patched. `sim/test_feetech_bus.py` holds the
emulator to the maker's SDK first (framing, checksums, EEPROM lock, sign-magnitude, sync read and write), and then the bridge is held to
the real class: the calibration an arm already has, a command arriving on the servo as the raw step LeRobot's own normalisation says, the
stop cutting torque on all six, a corrupt reply retried by LeRobot's own retry, a servo that goes silent, a servo that reports an error. **44 checks.**

What it is: **rung 4** (`docs/robot-support.md` §3). The servo is a counterpart written here from the maker's control table, which is read out
of LeRobot's own `tables.py` rather than retyped; it is not Feetech's firmware, and a pass is not evidence an arm moves. What it found:

| | |
|---|---|
| **F219** | The real class names itself `so_follower`; the role table was keyed on `so101_follower`, the fake's name. LeRobot's own SO-101 got **no roles** and could not take part in any remapping. A check now fails for any robot class the installed LeRobot ships that is neither in the table nor listed as deliberately without one. |
| **F220** | One `unit` for the whole body converted the gripper, which LeRobot reports in **percent of its travel**, not degrees. `units` is per motor now; the gripper goes through unconverted and the descriptor says so. |
| **F221** | `open_bridge` could open no robot at all: LeRobot registers a configuration class when its package is imported and `lerobot.robots` imports none, so `get_choice_class("so101_follower")` raised `KeyError` for every type. |
| **F222** | `open_bridge` built the configuration from raw JSON, so a `calibration_dir` arrived as text where LeRobot calls `.mkdir()` on a `Path`. LeRobot's own parser (`draccus`) builds it now. |

The first two of those were wrong in a way a fake cannot show, because the fake had the name and the unit the table assumed. The
nightly (`substrates.yml`, `lerobot-wire`) runs this at the pinned release and at PyPI's newest, which is what the one admitted risk
above — "its field names match the library's on the day you run it" — needed someone to do.

## The Dynamixel half: LeRobot's own Koch and OpenMANIPULATOR-X

`selftest_wire_dynamixel.py` does the same for the other servo family LeRobot drives, and the two robot classes that use it: **`KochFollower`** and
**`OmxFollower`**, as shipped, through LeRobot's own `DynamixelMotorsBus` and ROBOTIS's own `dynamixel_sdk` (protocol 2.0), on a pseudo-terminal that
`sim/dynamixel_bus.py` answers as a bus of six X-series servos (XL430-W250 and XL330-M288). `sim/test_dynamixel_bus.py` holds that emulator to the SDK
first, **27 tests**: the CRC and the byte stuffing are compared with the SDK's own functions as well as run through its packets, the EEPROM is
read-only while torque is on (the SDK's own sentence for error 7), an operating mode an XL430 does not have is a data-range error, a reply whose parameters
hold `FF FF FD` is stuffed and unstuffed, a hardware error raises the alert flag and the Shutdown register takes the torque off. **68 checks** hold the bridge
to the real classes. The two arms start differently on purpose: the **Koch is calibrated** (servos holding offsets and travel, LeRobot's file saying the same,
`use_degrees` on), and the **OpenMANIPULATOR-X is opened as it ships** — no file, factory servos — so LeRobot's own `OmxFollower.calibrate()` runs and writes the
calibration itself, in LeRobot's default of -100 to 100.

It is **rung 4**, like the Feetech half: the servo is a counterpart written here from the maker's control table (read out of LeRobot's `tables.py`), not
ROBOTIS's firmware. What running the real classes found:

| | |
|---|---|
| **F232** | **A servo that has shut itself down reads as healthy.** LeRobot's observation is positions only and its bus code throws away the error byte of every reply, so a Dynamixel in overload (its factory Shutdown register tells it to let go, and an arm falls) or a Feetech reporting an error was a read that succeeded, a state with no flag and a command that was accepted. The bridge now asks each servo's own error register (`Hardware_Error_Status`, `Status`) every 100 ms through the robot's bus: any bit is FAULT and DEGRADED (§27 reads DEGRADED), the word is `servo id << 16 \| the servo's bits`, verbatim, and every command is refused until it clears. A servo whose **torque is off though nothing stopped it** (a reboot, a power dip) has no bit of its own and is a fault too (`0x100`). A robot with no bus the bridge can ask says so in its descriptor and keeps a fault word of 0. |
| **F233** | **The stop aborted at the first servo with an alert.** LeRobot's write path raises on a reply that carries the hardware-alert flag, after the write has been made, and `disable_torque()` writes servo after servo, so on a bus where servo 2 had an alert the stop ended there and servos 3 to 6 kept their torque — the stop failing at the moment it mattered. Every servo is tried now, whatever one says, and the torque register is read back: a stop is OK only if every servo reads off, and a servo that does not obey is an I/O error naming it, with the latch held all the same. |

One difference between the two families is worth knowing: LeRobot's `sync_read` makes **one try** unless a caller asks for more, and the SO class asks for three while the
Koch and the OpenMANIPULATOR-X do not, so a single reply that fails its CRC is an I/O error for the Dynamixel arms (never a position taken from a bad packet) and is
retried invisibly for the SO-101. Both are checked.

`SKYNET_LEROBOT_FAKE=1` selects the fake explicitly. It is opt-in rather than a fallback, because a
driver that silently substitutes a simulated body for a missing real one is a driver that will one
day report a successful grasp that never happened.

## Configuration

```json
{
  "robot": {"type": "so101_follower", "port": "/dev/ttyACM0", "id": "arm"},
  "unit": "deg",
  "robot_class": "so101_follower",
  "role_overrides": {"shoulder_pan": "left_arm.shoulder.yaw"},
  "robot_slug": "my-arm"
}
```

Every field is optional except `robot`, and an absent one means the bridge declares less rather than
assuming more.

## When the line is worse than a cable

```bash
make chaos PYTHON=~/lerobot-venv/bin/python     # LeRobot's own SO-101, the serial line through a relay that delays, stalls, loses and corrupts bytes
```

`chaos.py` puts `tools/faultproxy`'s serial relay between LeRobot's own `SO101Follower` and the emulated Feetech bus (`sim/feetech_bus.py`, which gained `attach` so it can serve on the relay's end) and asks the contract's
five questions. **The servos agree with whoever wrote them** (F15): it shows the bridge's own bounds, staleness and stop path, and nothing about a servo. `DRY-COVERAGE-PLAN.md` item 12.C.4's first run found two things:

* **F270 — the stop took 351.8 ms against a line that carried nothing.** It wrote torque-off one servo at a time, waited for each reply, and then read every servo back: the SDK's receive timeout seven times over, held in
  the stop. It is one broadcast `SYNC_WRITE` of `Torque_Enable` that waits for nobody now (0.1 ms on a dead line, and one frame that reaches every servo whatever one of them says, which is what F232's try-all-and-read-back
  was for). A stop cannot both return at once and verify, so the next health poll verifies: a servo that still reads torque on after the stop had a moment to take it is a FAULT whose word is its id and a new bit, `0x200`,
  and the stop is sent again every health period until every servo obeys. A stop that one servo does not obey used to be `EIO` from the stop itself, and is a fault in the next read.
* **F271 — a late reply read as a servo fault.** A servo's reply to the health poll is matched to the request by its id and a checksum and by nothing that says which register it answers, so a late reply to an earlier
  request (a position, on a line that stalled or dropped a frame) was read as the `Status` register: error bits that differ every time (`0x06`, `0xfd`, `0xf9` in one run), each a FAULT, with every command refused while it
  stood. A reading is a fault, or the end of one, when the servos say the same thing twice a period apart; a real fault does not change between two looks, and a corrupt reply could hide one as well as invent one, so the
  same rule ends it.

And the same message on every period (F266): 53 identical lines of `get_observation() failed` in one run, 47 of `refused: servo 1 …`. A failure that goes on is said once per run of the same cause.
`expect` in the subject says what a 10%-loss line can be asked for: a sync read is a request and six replies, seven messages that must all arrive, so 0.9⁷ is about 48% of reads whole before the SDK's retry.
