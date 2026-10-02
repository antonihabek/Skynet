# skynet-lerobot — the LeRobot bridge

§16 and §67. `Robot.connect / get_observation / send_action` is very nearly the simplest possible
RHAL, so wrapping it means **every robot LeRobot supports becomes a Guest-tier Skynet robot for the
cost of one driver**.

```bash
python selftest.py          # sim conformance against a fake robot — 110 checks, no hardware
make check-wire PYTHON=...  # LeRobot's own SO-101 class on an emulated Feetech bus — 33 checks, needs `lerobot[feetech]`

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
# an interpreter with `lerobot[feetech]` (LeRobot 0.6.1 is the release this was measured on)
make check-wire PYTHON=~/lerobot-venv/bin/python        # or: make check-wire-venv, which makes the venv
```

It opens **`lerobot.robots.so_follower.SO101Follower` as shipped**, the way production does (`open_bridge`: a JSON configuration, LeRobot's
own config registry, `make_robot_from_config`), with LeRobot's own `FeetechMotorsBus` and Feetech's own `scservo_sdk`, on a pseudo-terminal
that `sim/feetech_bus.py` answers as a bus of six STS3215 servos. Nothing on LeRobot's side is patched. `sim/test_feetech_bus.py` holds the
emulator to the maker's SDK first (framing, checksums, EEPROM lock, sign-magnitude, sync read and write), and then the bridge is held to
the real class: the calibration an arm already has, a command arriving on the servo as the raw step LeRobot's own normalisation says, the
stop cutting torque on all six, a corrupt reply retried by LeRobot's own retry, a servo that goes silent. **33 checks.**

What it is: **rung 4** (`docs/robot-support.md` §3). The servo is a counterpart written here from the maker's control table, which is read out
of LeRobot's own `tables.py` rather than retyped; it is not Feetech's firmware, and a pass is not evidence an arm moves. What it found:

| | |
|---|---|
| **F219** | The real class names itself `so_follower`; the role table was keyed on `so101_follower`, the fake's name. LeRobot's own SO-101 got **no roles** and could not take part in any remapping. A check now fails for any robot class the installed LeRobot ships that is neither in the table nor listed as deliberately without one. |
| **F220** | One `unit` for the whole body converted the gripper, which LeRobot reports in **percent of its travel**, not degrees. `units` is per motor now; the gripper goes through unconverted and the descriptor says so. |
| **F221** | `open_bridge` could open no robot at all: LeRobot registers a configuration class when its package is imported and `lerobot.robots` imports none, so `get_choice_class("so101_follower")` raised `KeyError` for every type. |
| **F222** | `open_bridge` built the configuration from raw JSON, so a `calibration_dir` arrived as text where LeRobot calls `.mkdir()` on a `Path`. LeRobot's own parser (`draccus`) builds it now. |

The first two of those were wrong in a way a fake cannot show, because the fake had the name and the unit the table assumed. The
nightly (`substrates.yml`, `lerobot-feetech`) runs this at the pinned release and at PyPI's newest, which is what the one admitted risk
above — "its field names match the library's on the day you run it" — needed someone to do.

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
