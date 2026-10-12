# skynet-lerobot — the LeRobot bridge

§16 and §67. `Robot.connect / get_observation / send_action` is very nearly the simplest possible
RHAL, so wrapping it means **every robot LeRobot supports becomes a Guest-tier Skynet robot for the
cost of one driver**.

```bash
python selftest.py          # sim conformance against a fake robot — 128 checks, no hardware
make check-wire PYTHON=...  # LeRobot's own SO-101 (Feetech) and Koch and OpenMANIPULATOR-X (Dynamixel) classes on emulated buses — 55 and 76 checks
                            # (and the emulators' own 18 and 27 tests), needs `lerobot[feetech,dynamixel]`
make check-reachy2 PYTHON=...  # LeRobot's own Reachy2Robot against Pollen's own core in its mock-hardware mode — 50 checks, needs Docker and `lerobot[reachy2]`
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

## Reachy 2: LeRobot's own class against Pollen's own core

`selftest_wire_reachy2.py` is the first run of this bridge against **software its maker wrote**. The robot is Pollen Robotics' Reachy 2, through LeRobot's own `Reachy2Robot` and Pollen's own `reachy2-sdk` (gRPC), and the
other end is Pollen's own core in its mock-hardware mode (`fake:=true`): the ROS 2 stack and the SDK server of the open image `pollenrobotics/reachy2` (Docker Hub, no login, about 4.5 GB, pinned by digest in `sim/reachy2_core.py`).
Nothing of Pollen's is in this repository.

```bash
make check-reachy2 PYTHON=~/lerobot-venv/bin/python     # or: make check-reachy2-venv, which makes the venv (`lerobot[reachy2]`); needs Docker, and port 50051 of the loopback free
```

**50 checks** hold the bridge to the real class and the real server: the 21 joints in LeRobot's order with the roles the table gives them, the SDK's degrees read as radians and commanded as degrees (the grippers' included), a masked joint held at the
goal it had, a command for the neck, an elbow, a wrist, a gripper and an antenna together, the refusals, the stop (5.9 ms), a core **frozen with the container's cgroup freezer** mid-session, and a second session on the same core. `skynet driver check`
through the real ABI against it: **34 passed**, 0 failed (the roles cover every joint; the descriptor is unsigned, as any local one is). It is **rung 2** (`docs/robot-support.md` §3): the maker's server answering the maker's client. The mock hardware has no motor,
no physics, no camera and **no limit** (a goal of 500 degrees reads back as 500, and `GetJointsLimits` answers with nothing), so what it shows is that the bridge speaks to a Reachy 2 core as Pollen's software expects, not that an arm moves, holds a load or stays
inside its travel. What it found:

| | |
|---|---|
| **F329** | **The image does not stay up in a container with no USB bus.** The video server's camera search runs `lsusb`, which exits 1 where there is no bus (Docker Desktop on Windows gives a container none), the server dies on the `CalledProcessError`, and `reachy.launch.py` treats a dead video server as a critical node and shuts the whole core down with it, the SDK server among the nodes. `sim/reachy2_core.py` puts a `lsusb` that finds nothing first on the PATH of the one process tree (the search is for a camera the mock mode has none of); nothing in the image is changed. |
| **F330** | **A core that stopped answering held the bridge for as long as TCP kept the connection.** LeRobot's `Reachy2Robot.get_observation()` reads the mobile base's odometry once for each of its three velocities, and each read is a gRPC call (`GetOdometry`) that Pollen's SDK makes with no deadline: a run against a frozen core waited in `mobile_base.py:125 odometry`, called from `robot_reachy2.py:169 _get_state`, for ever. `read_state` and `emergency_stop` are the calls the RHAL says may not wait. A robot class marked `bounded` in `roles.ROBOT_DEFAULTS` (the Reachy 2) now has its calls made on a thread of the bridge's own, with a deadline of 100 ms (40 ms for the stop's freeze): a call that does not return fails the read or the write with `ETIMEDOUT`, the class is not called again until it has returned (two threads in one gRPC stub are a race), and the stop returns at once with the latch set and says that the freeze was not sent. |
| **F331** | **A stream that had stopped left a cache that read as alive.** Pollen's SDK keeps its state in a cache that a gRPC stream fills; when the stream stalls the cache stays readable, and the bridge stamped each read with the time of the read and counted the sequence per call, so nothing said so (§24: a repeated sequence is a stalled producer). The SDK keeps the timestamp of the last state its stream delivered (`get_update_timestamp()`), and a class with such a clock (`update_clock`) is read the Unitree way: a read waits, no longer than 100 ms, for the clock to move, stamps the state with when the bridge first saw it, counts one sequence number for each state, and when none comes returns the last one again under its stamp and its sequence. |

Both are shown with a fake that has the SDK's clock and can be made to hang (`selftest.py`: 128 checks, of which 17 are new; 9 of them fail against the bridge as it was). What the bridge **does not** do for a Reachy 2 yet: it declares no joint limits (the maker's URDF names a range for each joint
and the descriptor does not carry them, so the Safety Kernel has nothing to clamp a command to), it stops by freezing the joints in place and does not cut the drives (the class has no torque-disable; Pollen's `turn_off_smoothly`, which lets the arms down, is not used), and it does not read the
robot's own audit, so a motor in error is not seen and the fault word is always 0. The mobile base is held still and the cameras are not read.

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
