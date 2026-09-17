# skynet-ros2-control — a body behind `ros2_control`

`DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this driver its substrate as a *"fake
`hardware_interface`"*, and `ros2_control` ships one: `mock_components/GenericSystem`, the fake its
own test suite runs against. So every line below is exercised against the real framework with no
robot, and against a fake nobody in this repository wrote — which is the only kind of dry run that
can be wrong in the project's favour.

```bash
make check              # 42 checks against mock_components. Needs ROS 2 with ros2_control
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
