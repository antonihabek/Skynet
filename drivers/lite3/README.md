# skynet-lite3 — a Jueying Lite3 quadruped over MotionSDK's wire

`DRY-COVERAGE-PLAN.md`'s Phase 7, quadruped row. F92 specified it and left it unbuilt: *"Lite3
MotionSDK public MIT, joint-level UDP, no vendor sim or fake, no X20 SDK; Fourier-shaped, SDK
client unmodified against a fake motion host."* This is that row.

```bash
git clone --depth 1 https://github.com/DeepRoboticsLab/lite3_motionsdk /tmp/lite3_motionsdk
make check LITE3_SDK=/tmp/lite3_motionsdk
make suite                       # the driver's own suite alone; needs no SDK at all
```

The driver is C++ and links nothing of the vendor's. The wire is three words and a struct, and
`lite3_wire.hpp` carries it with the vendor's own file named beside every constant.

## Why the vendor's library is on the other end rather than under the driver

MotionSDK's `Sender`/`Receiver` pair is a demo's shape, not a driver's: `Receiver`'s port is fixed
inside the prebuilt library, `RobotStateInit()` moves every joint to zero and then sleeps seven
seconds, `ControlGet(ROBOT)` sleeps two more, and none of it can be called from `emergency_stop()`,
which §4 requires to work from a signal handler and from a dying process. A driver built on those
entry points would move a robot at `open()` and block in its stop.

So the vendor's code is put where it is worth most — on the *other* side of the substrate — and the
transcription is held to it twice:

| | |
|---|---|
| `layout_check.cpp` | compiles the vendor's `robot_types.h` and `command.h` beside this tree's header and static-asserts every size, every offset and the packing of the `{ type : 8; count : 24 }` bitfield. If it compiles, the two layouts are one layout |
| `cross_check.cpp` | the vendor's own `Sender` and `Receiver`, **unmodified**, against `fake_motion_host`: their client's datagrams parsed by this tree's fake, this tree's state push parsed by their `Receiver` and read back through their own per-leg views |

That second one is what makes the command codes trustworthy. MotionSDK documents exactly one —
`0x0906`, the state push — and the rest (`0x0111` a joint command, `0x0114` and `0x0113` control
right, `0x31010c05` all joints to zero) were **read out of the disassembly of the prebuilt
library**, which is weaker evidence than a document and is labelled as such in `lite3_wire.hpp`. The
cross-check is the corroboration: the vendor's own `Sender` drives the fake, and a code this tree
had wrong would land as `unknown_code` in the fake's report. It lands as zero.

## The three things the mapping has to get right

§24 commands a body in one mode, and `JointCmd` is not a mode. It is an impedance command — every
joint carries `position`, `velocity`, `torque`, `kp` and `kd` at once — and the drive computes, at
20 kHz,

    T = kp * (pos_goal - pos_real) + kd * (vel_goal - vel_real) + t_ff

**1. Zero gains are this wire's "say nothing", and they are also a limp leg.** The Unitree row needs
sentinel values because a zero written into `q` is a command to go to zero radians; here the same
protection falls out of the arithmetic, since `kp = kd = t_ff = 0` makes `T` zero whatever the
setpoints say. What does not fall out is §24's mask. The datagram is all twelve joints or nothing,
so a masked joint cannot be left out and has to become bytes — and §24 says which: *"hold this joint
at its current setpoint"*. A masked joint carries its last position setpoint, or, when it has none,
the position measured when the hold began, latched once rather than re-read, with the operator's
`kp` and `kd`. With no gains configured, a command that masks a joint is refused rather than sent as
a slack leg.

Until 2026-09-24 this driver read the mask the other way — limp by default, damped at a configured
`hold_kd` — and called the hold *"a command the OS never issued"* (F102). §24's "current setpoint"
answers that, and Unitree's own simulator measured what the limp reading costs: the kernel writes an
all-zero mask every cycle it has nothing to forward, and the first one folded a standing Go2 from
0.385 m to 0.106 m (`DRY-COVERAGE-PLAN.md` F121, carried here as 8.13). `hold_kd` is now refused by
name, so a configuration written for the old reading does not silently get the new one, and the fake
motion host counts any slack joint under the conformance run as a defect (it was 24 a run; it is 0).

**2. `kp` and `kd` are the operator's, and are not guessed.** A position command with no gains
configured is refused, and a velocity command with no `kd`; a torque command needs neither.
§63.4's rule arriving through a gain rather than a rating, the same refusal `drivers/unitree` makes
about the same kind of drive.

**3. Control right expires, and a latch cannot track it.** One second without a command and the
underlying controller takes the legs back (MotionSDK README §2). A driver that took control at
`open()` and remembered it would, after any pause, be writing datagrams a robot had stopped
listening to — every send succeeding, every command ignored, nothing anywhere saying so. This driver
takes control in the same call as the first command it is for, and **re-takes it whenever it has
been quiet for longer than half the reclaim window**. The suite holds it: a command inside the window
is one datagram, a command after it is two.

## What the descriptor declares, and what it will not

Joint names, travel, velocity and torque are all Deep Robotics' own, from `Lite3_rl_deploy`:
`run_policy/lite3_test_policy_runner_onnx.cpp` for the names, and
`state_machine/parameters/lite3_control_parameters.cpp` for

```
fl_joint_lower_ << -0.530, -3.50, 0.349;   fl_joint_upper_ << 0.530, 0.320, 2.80;
joint_vel_limit_ << 30, 30, 20;            torque_limit_ << 40, 40, 65;
```

with the mirror across the body taken from the vendor's own `idle_state.hpp`. That makes this **the
first legged body in this tree whose `joint_ratings` readiness check is met**: every Unitree body,
the Fourier GR-1, the Booster T1 and the AgiBot X1 declare a torque ceiling and no joint velocity,
because their vendors publish none (F64). `skynet readiness` on this descriptor reads **guest**, with
`control_rate` the only host blocker left — and that one stays, because MotionSDK publishes no rate
and the demo's one-millisecond timer is the example's loop, not a figure about the body.

A position setpoint outside that travel is **refused, not clamped**: clamping would move the joint
somewhere nobody asked for, and sending it unchanged would make the descriptor a document the driver
itself does not honour.

`safety.estop` is claimed and `envelope.estop` is not. The first is §15's *software* stop and this
driver has one — damping, hand-back, and a latch that refuses every later command. The second is the
circuit, and there is no circuit here: what goes out is a datagram to a program on the robot's own
computer. `platform.onboard_compute` is claimed on the vendor's own documentation — the motion host
is a Linux computer the owner is told to SSH into (README §4.2), whose configuration the SDK edits
(§5), and which the SDK's CMake cross-compiles for (`-DBUILD_PLATFORM=arm`).

The IMU, the joint temperatures and the foot forces go out under `vendor.lite3.telemetry`, because
§24's `State` has no field for any of them and `read_sensor` is a frame channel for cameras. The same
reply carries what only the driver knows about the session: arrivals, drops, stale reads, the push
interval it measured, and how many times it had to take control right back.

## Control right across a link outage

The robot takes the legs back after a second without a command it **received** (MotionSDK README 2), and a command is a datagram nothing answers, so a driver's `OK` says the socket took it and nothing about the
machine. This driver used to re-take control right only when *its own* sends had been quiet, and a link outage is not that: a control loop that goes on writing through a link that is down is never quiet, the
robot reclaims the legs after a second of receiving nothing, and when the link came back every command was accepted by the socket, ignored by the robot and answered `OK`, for the rest of the session. The link-chaos
contract's sixth question found it (`make chaos`: **0 commands acted on in the 1.5 s after a 1.5 s stall**; DRY-COVERAGE-PLAN.md F276). The evidence the driver has that the link was down is **the state stream going
quiet for longer than the reclaim guard**: the receive thread notes a push that arrived later than that after the one before it, and the next command takes control right again, once, and says so with the
gap's length. An outage in the command direction only is not covered: the pushes keep arriving, nothing says the robot reclaimed, and the wire carries no control-state field to read.

`chaos.py` is the contract's subject: the motion host's two UDP flows (the commands, and the state the host pushes to the port it is told) each through a relay, with the fake started in the runner's `attach` hook
and its plant read through an optional sixth argument of `fake_motion_host`, a tap that sends the twelve joint positions and the count of commands the host acted on to a port the relays do not carry. **68 of 68**,
and 58 of 58 seeded defects are noticed. The host is the fake written here (F15): this shows the driver's own waits, staleness, stop and control right against a link that delays, loses, repeats, reorders, cuts
short, corrupts and blackholes datagrams, and nothing about Deep Robotics' motion host.

## Where it stops

| | |
|---|---|
| **Guest tier** | Reached. 179 checks in its own suite, 13 in the wire cross-check, 32/0 through the real ABI |
| **`rhal.state.rate`, `rhal.command.latency`** | Loopback UDP against a C++ driver is fast. A real Lite3's wire crosses Wi-Fi or an Ethernet port to the robot's own computer, and what that costs is a bench measurement |
| **`safety.estop.latency`** | A number in milliseconds, on a body, with a stopwatch on the drives. What this driver has is a datagram and a latch |
| **The X20** | Same vendor, same family, **no published SDK** (F92). `model` refuses it by name rather than assuming the wire carries over |
| **Contact, gravity, a body on the ground** | Not in the substrate at all, and a legged body is where that matters most. This fake cannot fall over and cannot stand up |
| **The 20 kHz current loop** | The fake integrates a torque into a velocity and nothing else. Real drives have dynamics, saturation and heat |

## Two things the vendor's own code does that are worth knowing

**The single-value path sends `12 + value` bytes and copies from a null pointer.** `CmdDone` reads
`EthCommand`'s union as a size for the `memcpy` and as a length for the `send`, whichever half was
written — so `SetCmd(code, value)` with a non-zero `value` copies `value` bytes from `nullptr`.
Every `Sender` method in the library happens to pass zero, which is why it has never been hit. This
driver does not use that path; the control-right datagrams it sends are the twelve-byte shape the
vendor's own methods produce.

**The receive path ignores the datagram's length.** `Receiver`'s callback copies a fixed
`sizeof(CommandMessage)` — 1036 bytes — out of a reused 64 KB buffer whatever arrived, so a short
push there is read as the tail of whatever arrived before it. This driver checks the length it got
against the length the header declares and drops what does not carry a whole `RobotData`, counting
it in the telemetry reply. You will also see `98 : Cannot bind the socket` from the vendor's
`Receiver` at startup: it binds 43897 twice and prints the second failure. Neither is this tree's.

## Licences

This directory is Apache-2.0. MotionSDK is MIT and `Lite3_rl_deploy` is Apache-2.0; neither is
vendored here, the driver links neither, and `make check` refuses rather than running half of
itself when the SDK is not named.
