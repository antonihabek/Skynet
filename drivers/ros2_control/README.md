# skynet-ros2-control — a body behind `ros2_control`

`DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this driver its substrate as a *"fake
`hardware_interface`"*, and `ros2_control` ships one: `mock_components/GenericSystem`, the fake its
own test suite runs against. So every line below is exercised against the real framework with no
robot, and against a fake nobody in this repository wrote — which is the only kind of dry run that
can be wrong in the project's favour.

```bash
make check              # 219 checks against mock_components. Needs ROS 2 with ros2_control
make check-conformance  # the sim half of SCTS, through dlopen and the real §24 ABI
make lib                # libskynet_ros2_control.so on its own
```

`ROS_DISTRO` and `ROS_ROOT` override where it looks. With no `ros2_control` installed the Makefile
refuses and builds nothing: a driver that compiled without its framework would be a driver nobody
could load.

## It hosts a `ResourceManager`, and never talks to a `controller_manager`

The obvious shape is a ROS 2 node that publishes to `controller_manager`. This is not that, and §11
is the reason: *no control decision may require a network round trip*. A setpoint that crosses a DDS
domain on its way from the Safety Kernel to a drive is a control loop with a network in it, whatever
the network is made of — §25's ring exists precisely so that the loop does not have one.

`hardware_interface::ResourceManager` is the layer *underneath* `controller_manager`. It loads the
hardware plugins, owns their lifecycle, and hands out state and command interfaces. That is the
whole of what a §24 driver needs, it is in-process, and it works on a machine with no ROS graph
running at all — no daemon, no discovery, no domain ID.

What that costs is stated rather than absorbed: **a robot driven this way is not also driven by ROS 2
controllers.** `ros2_control` allows one claim on a command interface and this driver takes it. That
is the same exclusivity §25.2 gives a capability lease, arriving from the other side.

## The place the two models do not meet

§24's command carries **one mode for the whole body**. `ros2_control` declares command interfaces
**per joint**, and nothing stops a URDF giving joint A `position` and joint B `velocity`.
`urdf/mixed_interfaces.urdf` is exactly that body, and it is in the suite because it is not a corner
case — it is how a mobile manipulator is usually written.

There is no honest way to command such a body through §24 as it stands. So the descriptor declares
the **intersection**: the modes every joint offers.

| URDF | `control.modes` |
|---|---|
| both joints `position` + `velocity` | `["position", "velocity"]` |
| one `position`, one `velocity` | `[]` |

A body whose joints share no mode is reported with no control modes at all, and every
`write_command` against it is `SKR_EINVAL`. That is the truthful descriptor for it, and
`skynet driver check` reads it exactly as it reads `drivers/pendant` (§59.2): the joints are
reported, and there is no mode in which this driver commands them. The alternative — picking a
mode that works for most joints and quietly dropping the rest — is the failure §24 is arranged to
make impossible.

The mismatch itself is a specification question rather than a driver bug, and it is written down as
one: `DRY-COVERAGE-PLAN.md`'s F30, written out as `docs/issues/per-joint-command-modes.md` for a
maintainer to carry to both copies of the architecture.

## What it reads, and what it will not declare

Position and velocity limits come from the URDF's `<limit>` because they are what they say they are.
A **torque rating** does not. A `<limit effort="...">` is a design value from a CAD model, not a
datasheet safety rating — §63.4, and the same rule that makes `skynet-urdf` write a literal null
rather than copy one across. `urdf/two_joint_mock.urdf` carries `effort="80"` and the suite asserts
the descriptor does not: `effort_max_nm` is absent, `envelope` is null, and the guards that need one
report unavailable rather than guessing.

**Roles are the operator's and are never inferred.** A `ros2_control` URDF names joints and says
nothing about what they are; `shoulder` in one robot is a pitch and in the next a yaw. So §5.1's
role is `null` unless the configuration carries one:

```
urdf=/path/robot.urdf roles=shoulder:right_arm.shoulder.yaw;elbow:right_arm.elbow.flex
```

Without it `embodiment.roles.coverage` fails and the report names the joints nobody has claimed,
which is §63.4's behaviour — a descriptor that waits for a human beats one that invented a mapping
the Safety Kernel then bounds travel with.

**A state interface that is declared and cannot be read is NaN, never zero** (§24). The mock URDF
declares no `effort` interface, so effort reads NaN; a zero there is a joint reported as unloaded,
which is a specific and dangerous lie. `ros2_control` has no homing concept at all — a plugin either
reports a position or does not — so the two-bit homing field is `unreported` rather than either
answer.

## The second body: a UR5e nobody here wrote

`urdf/ur5e_mock.urdf` is generated from Universal_Robots_ROS2_Description — the header has the
commit and the command — with the vendor's own mock `hardware_interface`. So the fake is still
theirs and the driver is still ours, and the six joints and their limits are the vendor's too.
This is Phase 7's first substrate for this driver, and F69's body: the bridge hosts a body it can
name. The descriptor carries the URDF's own `<robot name>`, slugified the way `skynet descriptor
from-urdf` does (`ur5e` here, `two-joint-mock` there); the selftest and `skynet-urdf`'s suite pin
both halves of that agreement to the same inputs.

What the vendor file changes about the suite's answers: the union carries `torque`, because the
vendor's mock offers `effort`; and effort reads finite, because the vendor declares the state
interface. What it does not change: `<limit effort>` is still not read (§63.4), even though its
150/28 Nm happen to equal the vendor's published max joint torques.

`urdf/ur5e.provisioned.json` is that body's §28 provisioned descriptor: `driver check
--descriptor` wrote it and the integrator completed it with the transcribed ratings — the UR5e
tech sheet's axis-movement table plus UR's "Max. joint torques" support article, the same figures
as `drivers/ur_rtde`'s `MODEL_RATINGS`. `skynet readiness` on it reads guest attainable with two
host blockers left: `watchdog`, because a mock has no controller to fault, and `onboard_compute`,
because that is measured where the OS runs. Both fall to a real plugin and a deployment, not to
more transcription.

## The next eight bodies, and the two torque rows that refused

Phase 7's item 1 in obtainable order (F89): one driver, ten vendors, already written — the work is
one substrate each. Nine are in `urdf/`, every one generated from the vendor's own description with
the commit and the command in its header, every one named by the URDF's own `<robot name>`:

| Body | Source | Envelope |
|---|---|---|
| UR5e | Universal_Robots_ROS2_Description | provisioned: tech sheet + "Max. joint torques" article |
| ABB IRB 120 | ros-industrial `abb` | provisioned: product manual handling data |
| FANUC LR Mate 200iD | vendor ROS 2 driver | provisioned: published axis ratings |
| KUKA KR 6 R900 | `kuka_rsi_driver` description | provisioned: published axis ratings |
| Techman TM7S | `tm_description` | provisioned: published joint torques |
| AUBO i5 | vendor description | provisioned: published joint torques |
| Kinova Gen3 | `kinova_description` | provisioned: published actuator ratings |
| Doosan M1013 | `dsr_description2` (one `<plugin>` line to mock) | provisioned velocity-only: M-series catalog joint table; no joint-torque ratings exist to transcribe |
| Elite CS520H | vendor description | provisioned velocity-only: the vendor driver's own joint-limits file (no standalone CS520H datasheet exists); no joint-torque ratings exist to transcribe |

The tenth and eleventh vendors close the class without substrates. Fairino ships a real
`SystemInterface` (`FairinoHardwareInterface`, position-only, verified in source) — but
`frcobot_ros2` carries no licence at all, so nothing generated from it can be vendored here. JAKA
ships no hardware interface: `jaka_driver` is a service-level node over the TCP SDK, and its repo
is unlicensed too. Both are recorded in the plan's session entry; neither is built, and a JAKA
driver belongs to item 5's long tail (route D, hardware-gated) if anyone asks.

## The stop, and what it is not

`emergency_stop()` latches a flag, and every subsequent `write_command` is refused with
`SKR_EPERM`. Deactivating the hardware components is what a `ros2_control` integration would do, and
it allocates, takes locks and calls into a plugin — none of which is safe from the signal handler
and the dying process this entry point must survive.

**That is not a rated stop and this driver does not report it as one.** A rated stop cuts power
through a circuit; the plugin behind this may or may not have one, `ros2_control` has no way to say,
so the descriptor declares no `estop` rather than `true`, and §15's `safety.estop.independent` stays
a bench test nobody passes from here.

## What the substrate cannot show

`mock_components/GenericSystem` copies commands to states. It has no dynamics, no latency, no
communication failure and no drive that can fault. So this suite shows that the driver drives
`ros2_control` correctly and it shows nothing whatever about timing, about a plugin that blocks in
`read()`, or about what happens when a real fieldbus underneath it drops a cycle. Those are the rows
the plan's table leaves under *not dry-verifiable* for this driver, and they stay there.
