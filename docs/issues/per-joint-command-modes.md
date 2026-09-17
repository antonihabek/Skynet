# Proposed amendment — §24 says what a body is commanded in; it cannot say what a *joint* is

**Status: an issue, not a change.** See [`README.md`](README.md).

It came out of `DRY-COVERAGE-PLAN.md` Phase 2's `ros2_control` row rather than from reading the
document, which is why it is worth filing: the driver is built, it passes, and the thing it cannot
do is a robot nobody would call unusual.

## The gap

§24's command carries one mode for the whole body:

```c
uint8_t  mode;            /* RHAL_MODE_POSITION | _VELOCITY | _TORQUE */
uint16_t joint_count;
const float   *values;    /* [joint_count]; units follow `mode` */
const uint8_t *mask;      /* [joint_count]; 0 = hold this joint at its current setpoint */
```

The mask is per joint and the mode is not. §24 explains the mask at length and for a good reason —
*"the mask is how the OS says nothing rather than saying zero"* — and the asymmetry is unremarked.

**A mixed-mode body therefore has no representation.** A mobile manipulator with the arm on position
and the wheels on velocity is one body, is written as one `<ros2_control>` element in every ROS 2
robot that has ever shipped, and cannot be expressed in one `rhal_command`. Two commands in
alternate cycles is not an answer: `seq` MUST advance and the watchdog reads a repeat as a stalled
producer, so a driver that split a frame in two would be sending the kernel two half-bodies at half
rate, each one holding the other half by mask.

## What it costs today

`drivers/ros2_control` declares the **intersection** — the modes every joint offers — because it is
the only truthful descriptor available:

| URDF | `control.modes` |
|---|---|
| both joints `position` and `velocity` | `["position", "velocity"]` |
| one `position`, one `velocity` | `[]` |

A body whose joints share no mode is reported with **no control modes at all**, every
`write_command` against it is refused, and `skynet driver check` reads it exactly as it reads
`drivers/pendant` (§59.2): the joints are reported, and there is no mode in which this driver
commands them. That is conservative, it is honest, and it means a perfectly ordinary robot is
uncommandable through this platform.

The alternative is what an integration reaches for, and is the reason this is an issue rather than a
note: pick the mode most joints offer, command those, and drop the rest. A body that accepted a
position command and silently moved two thirds of itself is the exact failure §24's shape exists to
make impossible, and it presents as a robot that *mostly* works — the worst available symptom,
because it survives a demonstration.

## Two amendments, and they are not equivalent

### A. Make the mode per joint

```c
typedef struct {
  uint64_t stamp_ns;
  uint32_t seq;
  uint8_t  mode;            /* the mode for every joint whose `modes` entry is RHAL_MODE_INHERIT */
  uint8_t  _pad[3];
  uint16_t joint_count;
  const float   *values;
  const uint8_t *mask;
  const uint8_t *modes;     /* [joint_count], or NULL — every joint takes `mode` */
} rhal_command;
```

`NULL` is the body-wide case and is what every driver written against ABI 1 already means, so the
field is additive in behaviour as well as in layout. `RHAL_MODE_INHERIT` (0) makes the per-joint
array sparse for the common body that is one mode with one joint held in another.

The cost is not the struct. It is that **every guard in §7.1 that reads units must now read them per
joint** — a velocity bound and a torque bound are different numbers against different descriptor
fields, and a guard that took `mode` once at the top of the cycle is wrong in a way that passes its
own tests on a single-mode body. That is the work, and it is where the corpus cases go.

### B. Declare that a §24 body is one mode, normatively

A mixed URDF is then two bodies, two drivers and two capability leases, and the amendment is a
sentence in §24 plus a refusal in §63.4 for a descriptor that declares joints disagreeing about
mode. Cheaper, and it is a real position rather than a dodge: two independently-commanded subsystems
*are* two bodies in every other sense the document cares about — §25.2's leases, §7.1's envelopes,
§15's tiers.

What it costs is that the two halves lose a common `seq`, so nothing downstream can say the arm and
the base were commanded from the same observation. For a mobile manipulator that is the property
that matters most.

**A is the one this issue recommends**, on the grounds that B moves the problem to whoever
integrates a robot rather than solving it, and that the frames-and-guards machinery is where the
knowledge about units already lives.

## Implementation order, once ratified

1. The contract first: `rhal_command.modes` in the corpus, with cases for `NULL`, for a uniform
   array, and for the mixed body — **before** any edition, for the reason §47.6 taught
   (`DRY-COVERAGE-PLAN.md` F11).
2. `bindings/c/skynet_rhal.h` and the Python and Rust views of it, whose layout assertions are the
   three-sided check that catches a field that moved.
3. §7.1's guards, per joint. This is the bulk of it.
4. `drivers/ros2_control` drops the intersection and declares the union, and its
   `mixed_interfaces.urdf` case changes from *"no mode, and refused"* to *"both modes, and
   commanded"*. That URDF is already in the suite and is already the regression test.
5. §15's `rhal.command.rejection` gains the case a mixed body makes available: a command whose
   `modes` entry names a mode that joint does not offer, refused by name.
