# skynet-unitree — Unitree bodies over `unitree_sdk2`'s low-level channels

`DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this row its substrate as *"SDK sim"*, its
dry-verifiable column as *"protocol, low-level mode mapping"*, and its **not** dry-verifiable column
as *"joint access at rate"*. Phase 7 item 3 extends it from the Go2 it opened with to the buyable
humanoids: the G1, the H1, and the H1-2. One driver, `model` in the config selects the body
(`go2`, `h1`, `g1`, `h1_2`; default `go2`).

```bash
git clone --depth 1 https://github.com/unitreerobotics/unitree_sdk2 /tmp/unitree_sdk2
make check UNITREE_SDK2=/tmp/unitree_sdk2   # all four bodies, in sequence
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
  never touches the ground, and a humanoid here never falls over, which is the larger lie for the
  taller bodies;
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
`fake_robot` counts it. Held to a control — writing zeros there fails the torque-mapping checks and
the run exits non-zero, and shifting every joint one slot too high fails the placement check plus
seven more.

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

## Fewer joints than slots, on every body

Both message families are shared with larger bodies than any one of these four. The Go2 has twelve
joints in a twenty-slot message; the H1 has nineteen in the same twenty (slot 9 is the vendor's own
`kNotUsedJoint`); the G1 has twenty-three in a thirty-five-slot message, sparsely (slots 13, 14, 20,
21, 27, 28 are the 29-DoF body's); the H1-2 has twenty-seven in the same thirty-five. The driver
reports its joints and fills every slot — the ones the body does not have get the sentinels and no
gain, because a slot left at the struct's zero is a well-formed command to a joint that is not
there, and on a body that *does* have it it is a motion.

The joint-to-slot maps live in `unitree_models.hpp`, included by the driver, the fake and the suite
alike, so the three cannot disagree about the wire. Each map is transcribed from the vendor's
published low-level example and says which. The suite commands distinct values per joint and asserts
each landed in its mapped slot — the check a map disagreement fails.

Two different body plans share two message families: the Go2 and the H1 speak the `go2` family, the
G1 and the H1-2 the `hg` family, on the same topic names with different types. The H1 appears twice
because the product spans both: the original H1's example drives the `go2`-shaped message and the
H1-2's drives `hg`, and no firmware note says which interface the older generation speaks today.

## Where it stops

| | |
|---|---|
| **Guest tier** | Reached, on all four bodies. 211 checks across the four suites (50/53/52/56), 31 through the real ABI each |
| **`rhal.state.rate`, `rhal.command.latency`** | The row's *"joint access at rate"*. DDS over loopback against a Python-free C++ driver is fast; a real body's channel crosses Wi-Fi or a wired link to the robot's own computer, and what that costs is a bench measurement |
| **`safety.estop.latency`** | A Unitree stop is hardware on the robot. This driver publishes a message with every gain and every torque at zero, which on an impedance drive is the definition of commanding nothing — and it is not a power cut. A driver reporting `SKR_OK` for a message it merely wrote would be claiming otherwise |
| **Contact, gravity, a body on the ground** | Not in the substrate at all, and a legged body is where that matters most — a humanoid that cannot fall over is the larger lie |

## What the descriptor declares, and what it will not

Travel per joint, from the vendor's published models, because travel is a kinematic fact about the
machine. Each envelope carries a **torque ceiling only**, cited with the hedge: 45 Nm on the Go2
(the largest motor's peak), 120 Nm on the G1 (the brochure's max with footnote [1]), 360 Nm on the
H1 and H1-2 (the knee group peak, with the hip/ankle/arm groups beside it). Unitree publishes no
joint velocity anywhere, so `joint_ratings` stays unmet and each envelope says so in its `source`
rather than inventing one (F64). Watchdog 20 ms, ten DDS periods by driver design.

Ratings the vendor did not publish stay undeclared per joint rather than inheriting a neighbour's:
the H1's waist, and the H1-2's waist and wrists, carry no `effort_max_nm`. Control rates likewise:
the Go2's 500 Hz loop is documented, the humanoids' DDS rate is not, so their `rate_hz_max` is null
— the community's 500 Hz convention is not a vendor publication.

The base is `legged` and `base_pose` is **all NaN**: nothing in `LowState_` localises the body. The
IMU gives orientation and nothing gives position, and a driver that wrote the IMU quaternion into a
pose would be claiming a position it does not have (§24's rule that an unmeasured quantity is NaN and
never zero).

## Licences

This directory is Apache-2.0. It links `unitree_sdk2` (BSD-3-Clause) and CycloneDDS (EPL-2.0 /
EDL-1.0); neither is vendored here and `make` refuses rather than building half of itself when the
SDK is not named.
