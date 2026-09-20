# Proposed amendment — §59.2 says a large class of robots cannot be commanded at joint level, and the vendors have shipped that interface for years

**Status: proposed**, written 2026-09-20 by the twenty-first session of `DRY-COVERAGE-PLAN.md`,
from F85. It corrects a claim rather than settling a design question, which is why the proposed
text below is the whole of it.

## The gap

§59.2 is the section Part VII was written to record, and its conclusion is normative:

> Behind a Fanuc, KUKA, ABB or older Universal Robots controller you upload a *program* — the
> controller does the interpolation, owns the servo loop, and enforces its own safety. …
> **The Safety Kernel cannot clamp**, because it never sees the commands. …
> These robots are **Guest tier permanently**, and not because of a missing feature — because the
> architecture that makes them safe is somebody else's and is already there.

The evidence the section gives for it is an observation about *this document*:

> "Joint" appears forty-three times in this document. `URScript`, `KRL`, `RAPID`, `Fanuc`, `KUKA`
> and "teach pendant" appear zero times.

That is a true and useful catch about this design's vocabulary, and it is not evidence about
robots. The conclusion was never checked against what the vendors sell, and it does not survive
the check.

## What the vendors sell

Every major industrial vendor offers real-time external joint streaming on current controllers:

| Vendor | Interface | Rate | Availability |
|---|---|---|---|
| ABB | Externally Guided Motion (EGM) | 250 Hz over UDP, joint or Cartesian | Optional feature, IRC5 and OmniCore |
| FANUC | Stream Motion (J519) | up to 250 Hz, joint, position only | Paid option, R-30iB and later |
| KUKA | KUKA.RobotSensorInterface (RSI) | 250 Hz, 4 ms cycle | Software package |
| Universal Robots | RTDE | 500 Hz | Standard — and already `drivers/ur_rtde` |

250 Hz is inside the 50–1000 Hz this design assumes, at the low end of it.

The second half matters more here, because it means this is not even a driver-writing project.
**`ros2_control` hardware interfaces exist for all four**, including a first-party, actively
maintained FANUC ROS 2 Driver described by FANUC as a high-bandwidth streaming driver,
ros-industrial's ABB driver built directly on EGM, and `kuka_rsi_driver` published as a ROS 2
package. §62.1 already says the `ros2_control` bridge covers *"every robot with a
`hardware_interface` plugin, which is most of the ROS-supported industrial and research fleet"*,
and `drivers/ros2_control` is built and reaches guest — 51/0 in its own suite and SCTS 31/0 as
last recorded, against `mock_components` on a ROS machine and against no robot.

**So this platform's existing generic driver reaches these machines.** No new driver, no vendor
agreement, nothing that is not already in the tree.

The repository has never considered any of it. `EGM`, `RSI`, `FRI`, `Stream Motion`, `MotoPlus`,
`uniVAL` and `b-CAP` appear zero times across `docs/`, `crates/` and `drivers/` — which is §59.2's
own style of evidence, pointed the other way.

## What survives, and belongs in the replacement

Three things are true and are worth keeping. None of them is "the hardware will not let you."

1. **It is a licensed option, not a standard feature.** J519 is a paid option; EGM and RSI are
   purchased packages. Whether a given cell has one is a procurement fact, and a robot whose owner
   has not bought it is, that day, a program-level robot.

2. **There is a real legacy tail.** Controllers are typically vendor-supported for about ten years,
   robot mechanics run ten to twenty years and fifty thousand hours, and cells of fifteen to thirty
   years are ordinary in service. Those machines have no streaming option to buy. **The amendment
   should not put a fraction on this**: no measurement of the split between streaming-capable and
   legacy installed base was found, and §59's whole value is that it does not guess.

3. **The safety argument inverts rather than disappearing, and this is the part worth writing
   down.** ABB's own application manual warns that using EGM *"bypasses many safety features of the
   robot, including collision avoidance."* §59.2's reason — *"the architecture that makes them safe
   is somebody else's and is already there"* — is therefore backwards under EGM: part of that
   architecture is switched **off** to get in. That is simultaneously an argument for wanting the
   Safety Kernel's clamp on that machine and a safety-case conversation with whoever signed the cell
   off. It is a reason to be careful; it is not a reason the tier is unreachable.

And one sentence in §59.2 is now simply false. *"The Safety Kernel cannot clamp, because it never
sees the commands"* is true of a program upload and untrue of EGM, Stream Motion and RSI, where the
kernel is the thing that generates them.

## Proposed text — §59.2

Retitle to **"A large class of robots cannot be commanded at joint level, and it is smaller than it
looks"**, and replace the body from *"Most of the world's installed industrial robots do not offer
that"* to the end of the section with:

> **A great many installed industrial robots do not offer that, and fewer than this section once
> claimed.** Behind a Fanuc, KUKA, ABB or older Universal Robots controller the ordinary way to
> work is to upload a *program*: the controller interpolates, owns the servo loop, and enforces its
> own safety.
>
> But every one of those vendors also sells a real-time external motion interface on current
> controllers — ABB's Externally Guided Motion, FANUC's Stream Motion (J519), KUKA's
> RobotSensorInterface, Universal Robots' RTDE — each streaming joint references at 250 Hz or
> better, which is inside the rate this design assumes. Each has a `ros2_control` hardware
> interface, FANUC's written by FANUC, so §62.1's bridge reaches them with no new driver. **A
> modern industrial arm with the option licensed is not a program-level robot**, and this section
> said for several rounds that it was.
>
> Three things remain true of the class that is genuinely program-level:
>
> * **Transport has almost nothing to map.** There are no joint channels to bind; the action space
>   is a set of named motions with parameters (§84), and `execute({ skill, arguments })` is the
>   path (§88).
> * **The Safety Kernel cannot clamp**, because it never sees the commands. This is the sentence
>   that decides the tier, and it is true only of the program-upload case. Under an external motion
>   interface the kernel generates the commands and can clamp them.
> * Skills, packaging, grants, fleet reconciliation, OTA and the black box all still work.
>
> **Who is actually in that class**: a controller with no streaming option available — the legacy
> tail, which is large and which this document does not put a number on, because nobody has
> measured the split; a cell whose owner has not licensed the option; and a high-level-API-only
> platform such as the Spot SDK. Those robots are **Guest tier**, and not because of a missing
> feature — because the architecture that makes them safe is somebody else's and is already there.
> That is a legitimate and useful product, and it is not what the rest of this document describes.
>
> **One warning belongs with the good news.** ABB's own manual for EGM states that using it
> *"bypasses many safety features of the robot, including collision avoidance."* So reaching a
> robot through one of these interfaces does not mean adding Skynet's enforcement on top of the
> vendor's; it can mean adding it in place of some of the vendor's. That makes the clamp more
> valuable and the commissioning conversation harder, and §60.2's warning applies with more force
> here than anywhere else in this document: at Host tier Skynet replaces the thing it is said to be
> compatible with, and on these machines the thing being replaced is safety-rated.

## Proposed text — §59.7's table

| Robot class | Best tier | What Skynet OS delivers |
|---|---|---|
| Open real-time interface — Franka FCI, UR RTDE, `ros2_control`, Unitree SDK, most LeRobot-supported arms | Host; Certified with a safety MCU | Everything in this document |
| **Industrial controller with an external motion option licensed** — ABB EGM, FANUC Stream Motion, KUKA RSI | **Host, subject to §59.3 and the bench half** | Everything in this document, reached through §62.1's `ros2_control` bridge. Read §59.2's warning about bypassed vendor safety first |
| High-level API only — Spot SDK, an industrial controller with no streaming option available or licensed, most cobots in program mode | **Guest** | Skills, packaging, grants, fleet, OTA, black box. **No transport, no clamping.** |
| No documented interface — consumer appliances, closed platforms | None | Nothing, absent reverse engineering |
| No onboard compute | Guest, from external compute | Observation and skill dispatch over the link, at link latency |

Note the deleted word. The old second row said *"Guest, **permanently**"*, and permanence was the
claim that was wrong.

## Proposed text — §61's third row

| Requirement | Today |
|---|---|
| Joint-level command access at rate | Standard on open interfaces, and a licensed option on current ABB, FANUC, KUKA and UR controllers. Unavailable on the legacy installed base and on high-level-API platforms (§59.2), and there it is not fixable in software |

## What this does not change

* **§59.1 stands unchanged**, and is now the binding one: nothing runs without a driver for that
  specific robot, and a closed undocumented machine gets nothing.
* **§59.3 stands**, and becomes the tallest bar for exactly these robots. An ABB arm reached over
  EGM still needs a sourced envelope, and `DRY-COVERAGE-PLAN.md` F58 measured `envelope_source` as
  the most common Host blocker across the whole fleet.
* **The bench half is untouched.** Host needs sixteen bench tests, a human and a robot, per §15.
  This amendment moves a robot class from *"ineligible"* to *"eligible and unmeasured"*, which is
  the same distance Phase 6 measures for everything else.
* **Host's other gates are untouched** — PREEMPT_RT, an attested identity, a signed descriptor,
  a body that does not coast on command loss.
* **Guest is unaffected.** These robots already reached guest through the bridge; what was wrong
  was the ceiling above them.
* **§60.2 and §60.3 stand**, and read harder rather than softer, for the bypass reason above.

## Amends

§59.2 in full, §59.7's table, §61's third row. No contract change, no corpus case, no figure, no
edition work. §62.1's driver table is already correct and needs nothing — it was describing this
coverage accurately while §59.2 was denying it, which is §80's shape inside one document.

## Why this is an issue and not a patch

`CONTRIBUTING.md`: *"If you think the specification is wrong, open an issue rather than a pull
request against it — a maintainer carries the change to both copies."* §73's figures are
maintenance (`DRY-COVERAGE-PLAN.md` *Decisions* 1 and 10) because a check enforces them; this is a
normative claim about what tier a class of robot may reach, and it waits for the owner.

## Sources

* ABB, *Application manual — Externally Guided Motion*, OmniCore (3HAC073318) and RW7 — 250 Hz UDP,
  joint and Cartesian, and the bypassed-safety warning.
* FANUC, *Stream Motion (J519)*, FANUC America Tech Transfer — up to 250 Hz, joint, position only.
* FANUC, *FANUC ROS 2 Driver* documentation — first-party `ros2_control` streaming driver.
* KUKA, *KUKA.RobotSensorInterface* product page, and `kuka_rsi_driver` on the ROS 2 package index —
  250 Hz, 4 ms cycle.
* ros-industrial, `abb_robot_driver` — `ros_control` hardware interface over EGM.
* Southwest Research Institute, *Extending the Industrial Robot Life Cycle* — controller support
  horizon and mechanical service life, for the legacy-tail paragraph.
