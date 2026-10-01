# `drivers/fairino` — a FAIRINO FR3 over the vendor's own wires

`DRY-COVERAGE-PLAN.md` Phase 7's cobot class. The plan closed that class with two rows unbuilt —
*"Fairino and JAKA close the class unbuilt (unlicensed, and JAKA ships no hardware interface)"* —
and F92 recorded Fairino as blocked because every `package.xml` in `FAIR-INNOVATION/frcobot_ros2`
carries `TODO: License declaration`.

That is still true, and it was never what route D needed. **`FAIR-INNOVATION/fairino-cpp-sdk` is
Apache-2.0 and ships its source.** The ROS 2 wrapper being unlicensed blocks the `ros2_control`
bridge — route R — and says nothing about writing a driver against the vendor's published wire.
The class was blocked on the wrong artefact, and this row is the correction.

## What is here

| file | what it is |
|---|---|
| `fairino_bridge.cpp` | the driver. Speaks XML-RPC on 20003 and CNDE on 20005; links nothing of the vendor's |
| `fairino_wire.hpp` | the wire truth the three share, with the vendor's file and symbol named beside every constant |
| `fairino_xmlrpc.hpp` | the subset of XML-RPC this wire uses, and the parts it refuses rather than guesses |
| `fake_controller.hpp` | the substrate: an FR controller's three sockets with a plant behind them |
| `fake_controller.cpp` | the same thing as a process, for the cross-check and the conformance run |
| `selftest.cpp` | the driver's own suite — **236 checks**, running the controller in-process |
| `layout_check.cpp` | this tree's field sizes against the vendor's struct, and its ports and its stop frame against the vendor's own source — **44 checks** |
| `cross_check.cpp` | the vendor's own `FRRobot`, unmodified, against the fake — **25 checks** |

```
git clone --depth 1 -b v2.3.9-robot3.9.9 \
  https://github.com/FAIR-INNOVATION/fairino-cpp-sdk /tmp/fairino-cpp-sdk
make check FAIRINO_SDK=/tmp/fairino-cpp-sdk
```

`make suite` is the half that needs no checkout.

## The evidence, and why it is better than the row before it

The Lite3 row had to read four command codes out of a prebuilt library's disassembly. Nothing here
was guessed: FAIR INNOVATION publish the SDK's **source**, so every port, every verb name, every
field name and the emergency stop's thirty-three characters were read out of their own files, and
`layout_check` opens those files and requires the text to still be there.

Three independent measurements, and they fail in different ways:

* **`layout_check`** — compile-time `static_assert`s that the eighteen fields this driver asks the
  stream for are the size the vendor's own `ROBOT_STATE_PKG` gives them, plus runtime assertions
  that `robot.cpp` still defines 20005, 8080, 20007 and 20003, that `FRCNDEClient.cpp` still knows
  all eighteen field names, and that the stop frame is in the vendor's source character for
  character.
* **`selftest`** — the driver against the controller, in one process, so what is asserted is the
  six doubles inside the `ServoJ` that arrived rather than a return code.
* **`cross_check`** — the vendor's `FRRobot::RPC()` against the fake: it configures the stream with
  **its own default seventy-six fields**, starts it, drives both servo windows, and reads state
  back through `GetRobotRealTimeState` into its own struct. A field name or a size that were wrong
  here would put the vendor's own reader at the wrong offset, and the check that the six drive
  temperatures arrive as 30…35 °C is 823 bytes into that struct.

## The three things the mapping had to get right

**The stop is the vendor's own bytes.** `FRRobot::StopMotion()` is not an XML-RPC verb: it writes
the literal frame `/f/bIII44III102III4IIISTOPIII/b/f` to port 8080. Because that is a constant, the
driver holds the socket and the bytes from `open()` and `send()`s them from a signal handler, which
§4 requires and which an XML-RPC round trip could not do.

**This wire has no per-joint mode.** `ServoJ` takes six positions, `ServoJV` six velocities and
`ServoJT` six torques, each in its own window, and no verb mixes them. The descriptor declares
`per_joint_modes: false` and a command whose joints disagree is refused rather than split into
three commands at three instants.

**A masked joint on a position wire is a held setpoint.** Holding is repeating *the setpoint this
driver last issued* — not the measured position, which sags under load and would walk the joint
downhill a little every cycle. The first command of a window initialises the held setpoints from the
measurement, and the suite asserts the difference. (The Lite3 row once refused to hold at all on its
impedance wire; F121 settled that §24's hold is a stiffness there too, and it holds now.) Under
velocity a masked joint is commanded zero. Under torque a hold has no value — zero is a joint that
gives way on an arm holding itself up — so a torque command that masks a joint is **refused**, where
it used to go out with zero on the masked joint and a notice at the window (F121, carried as 8.13).

## Where the numbers come from

**Travel is queried from the controller**, not transcribed: `GetJointSoftLimitDeg` returns the six
soft limits this arm is actually configured with, and the descriptor publishes those. No other
driver in this tree does that — everywhere else the limits are a datasheet's, and a datasheet
cannot know that this arm has a reduced wrist because of what is bolted to it. The fake answers
±90° for wrist 3 where the catalogue says ±175°, and the suite fails if the descriptor carries the
catalogue's number. When the query fails the driver falls back to the published figures and the
envelope's `source` says which of the two it used.

The velocity ceiling is FAIR INNOVATION's own (`frtech.fr/FR/5.html`, FR3: every axis ±180 °/s).
**There is no joint torque ceiling**, because they publish none — the product page gives payload,
reach, repeatability and per-axis speed and no torque at all. So `joint_torque_max_nm` is absent and
readiness's `joint_ratings` fails, exactly as it does for the ten velocity-only bodies F91
corrected. The wire *reports* joint torque, which is a measurement and not a rating.

`estop: true` **is** declared, and this is the first row in the tree where it is: an FR controller
streams the state of a physical emergency-stop button and of two independent safety-stop inputs,
and the vendor's own `GetSafetyCode()` refuses every servo verb while either input is asserted.
That is a stop path that does not run through the application processor, which is what the
`estop_hardware` check asks. It gates the certified tier and nothing else, and the certified tier
still needs a bench measurement nobody here has taken.

`platform.onboard_compute` is **not** declared. The controller runs the vendor's software and
accepts uploaded Lua programs; it is not a computer the OS could be deployed onto, and the vendor
documents no way to run one's own process there.

## What the controller's own error does

The state frame carries `main_code`, the controller's error number, and `sub_code`, which narrows it. A non-zero `main_code` reads
`FAULT` and `DEGRADED` (§27 reads the second; the first is the maker's "commands are being ignored" of §24) with the number in §24's
fault word and the sub code in the telemetry extension, and `write_command` refuses with `SKR_EPERM` and says which error the
controller holds, before a servo window is opened. A zero code reads healthy again at once: nothing here latches it.
(`DRY-COVERAGE-PLAN.md` F206: the number used to be carried and nothing was raised, so by §27 a controller reporting an error read
as healthy for as long as the error stood, and a command went to it anyway.) What this does **not** say is which of Fairino's codes
leave the arm able to move — their error list is not in this tree — so none is read as less than a fault. If some codes are
warnings on a healthy controller, that is the one place to say so, and a vocabulary for `drivers/fault-vocabulary.json` is what
would let the contract hold the rest.

## Where it stops

* **No FR cobot has run any of this.** The substrate has no kinematics, no collision model, no
  safety system and one integrator per joint. `rhal.state.rate`, `rhal.command.latency` and
  `safety.estop.latency` are bench numbers on a body and a loopback socket measures none of them.
* **One model.** FR5, FR10, FR16 and FR20 speak this exact wire and have different published
  ceilings, so `model` refuses anything but `fr3` rather than lending one model's numbers to
  another. Adding one is a transcription and a row in `CATALOGUE_TRAVEL`.
* **No control rate is claimed.** The stream's period is this driver's own (8 ms by default, the
  vendor's default too) and `ServoJ`'s `cmdT` is a recommendation in their header; neither is the
  loop rate of the body, which they do not publish. `rate_hz_max` stays null and `control_rate`
  blocks the host tier.
* **No watchdog window is claimed.** Nothing in 599 API entry points documents what the controller
  does when servo commands stop arriving, so `watchdog_ms` is null and that blocks host too.
* **The simulator FAIR INNOVATION do ship** — SimMachine — is a Windows virtual machine behind a
  registration wall. This substrate exists because that cannot be cloned in a pipeline, and it is
  not a replacement for it.

## Two defects found in the vendor's own SDK

Neither is exploited here and both are recorded because the row met them:

1. **`FRTcpClient::RecvPkg` cannot read the packet its own controller sends.** It is the reader for
   the older fixed-layout stream on port 20004, and its two stack buffers are
   `char[CNDE_MAX_PKG_SIZE]`'s smaller sibling — `uint8_t allRecvBuf[2048]` and
   `char tmpRecvBuf[2048]` — while `sizeof(ROBOT_STATE_PKG)` is **2075** at robot 3.9.9. A caller
   passing the packet size, which is the only sensible thing to pass, overflows both by 27 bytes.
   The function is unreferenced in the 2.3.9 source, so it is latent rather than live; this driver
   speaks 20005 and never calls it.
2. **`StopMotion`'s frame counter is computed and discarded.** `static int cnt = 0; cnt++;` sits
   beside a `sprintf` that writes the literal `44` into the count field, so every stop from every
   session goes out with the same number. Harmless, and worth knowing before anyone tries to
   correlate stops by that field. This driver sends the vendor's frame unchanged, count included:
   matching their bytes is worth more here than improving on them.
