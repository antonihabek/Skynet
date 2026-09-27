# skynet-lerobot — the LeRobot bridge

§16 and §67. `Robot.connect / get_observation / send_action` is very nearly the simplest possible
RHAL, so wrapping it means **every robot LeRobot supports becomes a Guest-tier Skynet robot for the
cost of one driver**.

```bash
python selftest.py          # sim conformance against a fake robot — 75 checks, no hardware

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

## Written from documentation, not from a running robot

§62.2 is explicit that this is how these drivers get written. The consequence is worth stating
rather than burying: the bridge has been exercised against a faithful fake (`skynet_lerobot/fake.py`)
and **not** against LeRobot itself or any hardware. Passing `selftest.py` means the adapter is
internally consistent. It does not mean its field names match the library's on the day you run it,
and that claim is not made anywhere in this directory.

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
