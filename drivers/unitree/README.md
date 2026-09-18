# skynet-unitree — a Unitree Go2 over `unitree_sdk2`'s low-level channels

`DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this row its substrate as *"SDK sim"*, its
dry-verifiable column as *"protocol, low-level mode mapping"*, and its **not** dry-verifiable column
as *"joint access at rate"*.

```bash
git clone --depth 1 https://github.com/unitreerobotics/unitree_sdk2 /tmp/unitree_sdk2
make check UNITREE_SDK2=/tmp/unitree_sdk2
```

## The vendor's SDK is on both sides, and that is the whole argument

`fake_robot.cpp` publishes `rt/lowstate` and consumes `rt/lowcmd` using **unitree_sdk2's own channel
classes**, exactly as the driver does. So the CDR encoding, the DDS discovery, the topic types and
the message layouts are Unitree's code at both ends — nothing here can be wrong about the wire
without being wrong in the way a real robot would refuse.

That is the strongest substrate of the three rows this session built. `drivers/franka` had to write
its own fake because there is no Franka simulator (F15's problem, answered there by compiling the
vendor's headers); this one did not.

What the fake is **not** is a robot:

* the joints integrate a first-order model with no dynamics, no contact and no gravity — a leg here
  never touches the ground;
* the rate is nominal, from a thread with `sleep_for`, and a real Go2 publishes at 500 Hz from its
  own onboard controller;
* there is no safety hardware behind it. The estop it observes is a message.

## Three things the mode mapping has to get right

§24 commands a body in one mode and Unitree's `MotorCmd_` is not a mode at all. It is an impedance
command: every motor carries `q`, `dq`, `tau`, `kp` and `kd` at once, and the drive computes
`tau_out = tau + kp*(q - q_measured) + kd*(dq - dq_measured)`.

**1. The sentinels are not zeros.** `PosStopF` (2.146e9) and `VelStopF` (16000) are Unitree's "ignore
this field" values. A driver writing `0.0` into `q` for a torque command is not saying nothing — it
is commanding the joint to zero radians, and with any `kp` at all that is a motion. This is the
defect the row exists to catch, and it is caught twice: the suite reads the message off the wire, and
`fake_robot` counts it. Held to a control — writing zeros there fails **five** checks and the run
exits non-zero.

**2. `kp` and `kd` are the operator's, and are not guessed.** They are not in a §24 command. A
position or velocity command with no gains in this driver's config is **refused**, because a default
would be a stiffness nobody chose in front of a Safety Kernel that cannot see it. That is §63.4's
rule arriving through a gain instead of through a rating, and it is the same refusal
`drivers/canopen` makes about a unit with no provenance. A torque command needs no gain and is
accepted.

**3. The CRC is load-bearing.** A `LowCmd_` whose CRC32 does not match is dropped by the robot,
silently, so a driver that omitted it would present as a robot that ignores commands. Unitree's CRC
is bit-reflected with polynomial `0x04C11DB7`, initial value all ones and **no final inversion** —
not any of the common parameterisations, which is why it is written out rather than reached for from
a library. The fake checks it.

## Twelve joints, and the message carries twenty

`LowCmd_` has twenty motor slots because the message is shared with larger bodies. A Go2 has twelve.
The driver reports twelve and fills all twenty — the eight it does not have get the sentinels and no
gain, because a slot left at the struct's zero is a well-formed command to a joint that is not
there, and on a body that *does* have twenty it is a motion.

## Where it stops

| | |
|---|---|
| **Guest tier** | Reached. 39 checks in its own suite, 26 through the real ABI |
| **`rhal.state.rate`, `rhal.command.latency`** | The row's *"joint access at rate"*. DDS over loopback against a Python-free C++ driver is fast; a real Go2's channel crosses Wi-Fi or a wired link to the robot's own computer, and what that costs is a bench measurement |
| **`safety.estop.latency`** | A Go2's stop is hardware on the robot. This driver publishes a message with every gain and every torque at zero, which on an impedance drive is the definition of commanding nothing — and it is not a power cut. A driver reporting `SKR_OK` for a message it merely wrote would be claiming otherwise |
| **Contact, gravity, a leg on the ground** | Not in the substrate at all, and a legged body is where that matters most |

## What the descriptor declares, and what it will not

Travel per joint, from Unitree's published limit table, because travel is a kinematic fact about the
machine. **No torque or velocity ceiling, and `envelope` is null** — §63.4, the same refusal
`drivers/franka` and `skynet-urdf` make.

The base is `legged` and `base_pose` is **all NaN**: nothing in `LowState_` localises a Go2. The IMU
gives orientation and nothing gives position, and a driver that wrote the IMU quaternion into a pose
would be claiming a position it does not have (§24's rule that an unmeasured quantity is NaN and
never zero).

## Licences

This directory is Apache-2.0. It links `unitree_sdk2` (BSD-3-Clause) and CycloneDDS (EPL-2.0 /
EDL-1.0); neither is vendored here and `make` refuses rather than building half of itself when the
SDK is not named.
