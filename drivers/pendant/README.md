# skynet-pendant — a body behind a program-level controller

The teaching driver for the robots §59.2 is about.

`drivers/mock` is the teaching driver for the machines this design was written for: joints, a
setpoint every millisecond, a Safety Kernel between the policy and the drive. This is the teaching
driver for the other kind — and §59.2 is blunt about which is larger:

> "Joint" appears forty-three times in this document. `URScript`, `KRL`, `RAPID`, `Fanuc`, `KUKA`
> and "teach pendant" appear zero times. The whole design assumes it can send joint positions,
> velocities or torques at 50–1000 Hz. **Most of the world's installed industrial robots do not
> offer that.**

Behind a Fanuc, KUKA, ABB or older Universal Robots controller you upload a *program*. The
controller interpolates, owns the servo loop, and enforces its own safety — often with a
certification Skynet does not have and is not trying to replace.

```bash
make check              # the suite: thirty-seven checks, nearly all of them refusals
make lib                # libskynet_pendant.so
make check-conformance  # the sim half of SCTS against it
```

## What it answers, and what it refuses

| Entry point | Answer |
|---|---|
| `read_state` | works — this is the whole of what the design gets for free from such a machine |
| `write_command` | **`SKR_EPERM`**, always, on a well-formed command. `SKR_EINVAL` first for a malformed one |
| `extension` | the only route that moves it, under the vendor token `vendor.pendant.motion` |
| `emergency_stop` | works, and on a real one it drops the safety relay rather than asking the controller |
| `read_sensor` | `NULL` — the cell's cameras are not on this controller's interface |

`write_command` returning `EPERM` rather than `ENOSYS` is deliberate and is the distinction the
driver exists to make. `ENOSYS` would say *this driver does not implement joint commands*; what is
true is that the **machine** does not accept them, and the driver implements the refusal.

The shape is checked *before* the permission. A command declaring five joints against a six-joint
body is malformed whether or not the machine takes commands, and a driver answering `EPERM` to
everything would tell a caller "you may not" about a command that is also wrong — the caller fixes
its permissions, ships, and finds the array overrun on a machine that does take commands.

## The motions

Four, with their parameters and the controller's declared bounds:

| Motion | Canonical | Parameters |
|---|---|---|
| `move_l` | `move_linear` | `pose` (m, required), `speed` (m/s, 0.01–0.25) |
| `move_j` | `move_joint` | `pose` (rad, required), `speed` (rad/s, 0.01–1.0) |
| `close_gripper` | `grasp` | — |
| `home` | `home` | — |

One motion per call, as text — a mock that needed a JSON parser to be useful would be a mock with a
dependency:

```
move_l pose=0.42 speed=0.10
```

Every refusal names the cause and the next action, which is worth more here than almost anywhere
else in this repository: a real controller's rejection is typically a numeric code in a status word,
and an integrator's afternoon is spent turning it back into a sentence.

## What this driver is evidence of

Run `make check-conformance` and read the report. The body declares `control.modes: []` and every
joint `control_modes: []`, which is the honest shape of such a machine in the schema as it stands —
the joints exist, they are reported, and there is no mode in which they are commanded.

That made `rhal.command.rejection` **fail**, because one of its four assertions asks a driver to
*accept* a well-formed command. §15 requires that test from `guest`, so every robot §59.2 describes
reached no tier at all — while §59.2 itself says they are *"Guest tier permanently"*. Two sentences
in one document, disagreeing.

It is `not_applicable` now, which §15 reserves for *"cases where inapplicability is a declared
property of the body rather than a gap in the evidence"* — and a body declaring no control modes has
declared exactly that. The three **rejection** assertions still run: a driver that ignores a short
command reads past the end of the caller's array whether or not its machine takes commands.

What is still missing is the action space. §59.2 says it is *"a set of named motions with
parameters"*, the descriptor has no field for one, and so the table above is reachable only through
`extension()` under a vendor token. [`docs/issues/motion-action-space.md`](../../docs/issues/motion-action-space.md)
is the amendment that would change that, and this driver is what it would be tested against.

## What it is not

A real controller. `simulator` is `true` and §35.3 caps it at guest; §59.2 caps a real one at guest
too, for a different reason — the vendor controller owns the servo loop and is the safety-rated
component. Two independent reasons for one ceiling, and a reader should not take the first as
evidence for the second: a real pendant robot's driver declares `simulator: false` and is still
permanently Guest tier.
