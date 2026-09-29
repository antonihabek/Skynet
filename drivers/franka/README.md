# skynet-franka — a Franka Robotics arm over the Franka Control Interface

`DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this driver its substrate as *"protocol fake from public
docs"*, its dry-verifiable column as *"encoding, state machine"*, and its **not** dry-verifiable
column as *"1 kHz torque, which is its entire claim"*. All three of those are exactly right and this
file is about the first one, because it is the row where the honesty is hardest to keep.

```bash
make selftest       # the wire, the state machine and every refusal. Needs python3 and nothing else
make check          # and everything above it, through the real ABI, against a fake Control box
make check-layout FRANKA_COMMON=<a libfranka-common checkout>   # the one that matters most
make check-franka-sim FRANKA_SIM=<a franka-sim virtualenv>      # against a server nobody here wrote
```

## The problem this row has that the others did not

Every other driver here was measured against somebody else's software. `drivers/ur_rtde` was written
byte by byte against URSim — Universal Robots' own controller in a container — and three of its
properties would not have been guessed (F27). `drivers/ros2_control` hosts the framework's own
`mock_components`. `drivers/canopen` drives a CiA 402 state machine that the profile defines.

**There is no Franka simulator** from Franka. The FCI talks to a Control box and to nothing else;
the arm's own simulation story is a URDF in somebody else's simulator, which is not the FCI. So the
fake in [`skynet_franka/sim.py`](skynet_franka/sim.py) is written here, in this repository, by the
same hand that wrote the client — and `DRY-COVERAGE-PLAN.md` F15 says exactly what that is worth:

> a protocol transcribed from memory and tested against a fake written from the same memory proves
> that two copies of one misunderstanding agree.

## Against franka-sim, which somebody else wrote

Since 2026-09-24 there is a second counterpart, and it is not this repository's (Phase 8, item 8.2,
F120). [`franka-sim`](https://pypi.org/project/franka-sim/) — Apache-2.0, a third party's — serves
libfranka's wire protocol at server version 10, the version this driver speaks, with MuJoCo physics
and Franka's FR3 model behind it, and it polices the FCI's motion-generator limits and lost command
cycles the way a Control box does. `make check-franka-sim` runs three phases, each against a fresh
server:

| | Result |
|---|---|
| [`franka_sim_check.py`](franka_sim_check.py), the Python driver motion by motion | 17 of 17: the version-10 handshake answered by an independent implementation, the state decoded from somebody else's encoder, the elbow sent 0.3 rad and arriving within 0.0001, nothing else moving, the all-zero-mask hold holding to 0.00000 rad, a velocity motion integrating to within 0.002 rad of its prediction, zero torque floating rather than dropping, and `StopMove` engaging the idle hold with the latch refusing what follows |
| `skynet driver check` on the Python driver | 31 passed, 0 failed |
| `skynet driver check` on the Rust port, `drivers/franka-rs` | 31 passed, 0 failed — the port's first run against a Control-box implementation that is not the Python fake it was measured against |

What the server flags is counted and printed rather than enforced. Every run of phase 1 so far has
drawn **two** motion-limit violations — a jerk near 6,400 rad/s³ against the FCI's 5,000 and an
acceleration step near 12 rad/s² against 10 — from nothing but the timing of a Python-paced 1 kHz
command stream. On a Control box each is a reflex that stops the arm. That is §31.3's reason for
capping Python at guest, measured rather than argued (F124), and it is why the Rust port is the one a
Host-tier Franka would run.

## What is done about it

**The layout is not transcribed.** Franka Robotics publishes `research_interface/robot/rbk_types.h`
and `service_types.h` under Apache-2.0 — the actual structs the actual Control box sends.
[`layout_probe.cpp`](layout_probe.cpp) compiles them and prints `sizeof` and `offsetof` for every
field this driver reads; [`layout.json`](layout.json) is that output, committed with the commit hash
and compiler it came from; and [`selftest.py`](selftest.py)'s first section asserts
`skynet_franka.protocol` against it on every run.

So the numbers are the vendor's compiler's, not anybody's reading. Held to a control: moving
`STATE_Q` by four bytes fails the suite with *"RobotState.q is at 596"*, which is the failure a
future release that moves a field will produce.

Two things that measurement caught which a careful reading would not have:

* **The state is floats and the command is doubles.** `RobotState` carries `float` arrays and
  `RobotCommand` carries `double` arrays — same seven joints, same protocol, adjacent structs in one
  header. A transcription that picked one for both is wrong by a factor of two in width on one side
  and the first field still parses.
* **`Connect::Response` is three bytes.** A `uint8_t` status with a `uint16_t` after it, and
  `#pragma pack(1)` means there is no padding byte between them. The natural C layout would be four.

**What it does not establish**, and the reason this README is long: that the Control box *behaves*
the way `sim.py` does. That `Move` is answered when this driver expects it, that a reflex arrives as
the bit this driver reads, that `StopMove` stops within any particular time. `franka-sim` narrows
that to what an independent reading of the protocol agrees with — which is more than one reading
agreeing with itself, and still not a Control box.

## Where it stops, and why the stopping point is the interesting part

| | |
|---|---|
| **Guest tier** | Reached. 34 of the sim suite's checks pass through the real ABI (`make check`, 2026-09-29; this line said 26) |
| **Host** | Needs the bench suite, and this driver's bench row is the hardest in the table |
| **`rhal.command.latency`, `rhal.state.rate`** | **The whole of Franka's claim.** `libfranka` is a 1 kHz torque loop; a driver that misses a tick is told so by the robot, in `control_command_success_rate`, and nothing in this repository can measure that against a Python bridge over a pipe (§31.3 caps Python at guest for this reason). The fake streams at a *nominal* 1 kHz from a Python thread, which exercises latest-wins reads and a message id that advances, and measures nothing about timing |
| **`safety.estop.latency`** | Franka's stop is a button wired into the Control box. This driver sends `StopMove`, which is the software path; the hardware path is not this driver's and cannot be |

## Three places the FCI's shape and §24's disagree

**A Franka is commanded only while a motion is running.** The UDP stream carries a `RobotCommand`
every tick and the box reads the field belonging to the motion generator that `Move` started; outside
a motion nothing in it is read. So the first command in a mode starts a motion, a command in a
different mode stops the running one and starts another, and a body with no motion refuses commands
rather than sending into a field nobody reads.

**The mask means something different per mode.** §24's zero is *hold this joint*. In position that is
the joint's own last reading; in velocity and torque it is zero, because on a torque joint holding
*is* going limp. The suite asserts both, beside each other.

**A latched error is refused, not cleared.** `drivers/canopen`'s F26 found a driver that reset every
drive at open and erased the fault that said why the robot stopped. The same rule is applied here
from the start: a faulted arm refuses commands, and `AutomaticErrorRecovery` is something an operator
asks for.

## What the descriptor declares, and what it will not

Joint **travel** is declared: it is a kinematic fact about the arm, published by the vendor, and
§5.2 asks for it. The **velocity and torque ratings are declared too**, transcribed from Franka's
own `franka_description` joint-limit table — the vendor's table, which their stack enforces, which
is §63.4's bar for a design value becoming a rating: the vendor saying it is one. The envelope
carries `basis: declared` (§5.3) and the watchdog is ten FCI periods by driver design. What stays
out is the hardware stop: Franka's is a button on the Control box this driver cannot press.

`effort_nm` is one of the few places in this repository where §24's array is a **measurement** rather
than NaN: a Franka reports link-side torque per joint, and the driver passes it through.
