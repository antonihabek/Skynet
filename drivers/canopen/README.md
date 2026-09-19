# skynet-canopen — CiA 402 drives on a CANopen bus

The best dry fit in `DRY-COVERAGE-PLAN.md`'s Phase 2 table, and this is why: almost all of CANopen is
arithmetic over eight-byte frames. The CiA 402 state machine, the object dictionary, expedited SDO
and the PDO layouts can all be exercised exactly, against a simulated drive, with no hardware —
and, here, with no CAN kernel module either.

```bash
make selftest    # the protocol and every refusal. Needs python3 and nothing else
make check       # the whole path: skynet driver check → python-host → this bridge → three drives
make check-vcan  # the same driver on a real SocketCAN interface, where the kernel has one
```

## What it reads off the drives rather than out of a file

| Object | For |
|---|---|
| `0x608F:1`, `0x608F:2` | encoder resolution — increments per motor revolution |
| `0x6091:1`, `0x6091:2` | gear ratio — motor revolutions per shaft revolution |
| `0x607D:1`, `0x607D:2` | software position limits → the descriptor's travel |
| `0x6080` | max motor speed → the descriptor's velocity ceiling |

The first two are the ones the driver **refuses a body over**. A position in increments means
nothing without them, and a driver that assumed the common 65536 counts per revolution and a direct
drive would report a joint angle wrong by the gearbox — a factor of fifty on an industrial axis, *in
radians, to a Safety Kernel that bounds travel with it*. §5.2's rule is that a limit with no
provenance is a rumour; a unit with no provenance is worse, because it is silently applied to every
number the body reports.

What it does **not** read is a torque rating. `0x6076` is the *motor's* rated torque — a design value
from a data sheet, not a joint safety rating (§63.4), the same distinction that makes `skynet-urdf`
write a literal null rather than a URDF's effort limit. So the descriptor declares no `effort_max_nm`
and `envelope: null`, and the guards that need one report unavailable.

## What it verifies before it commands anything

**The PDO mapping.** This driver speaks TPDO2 and RPDO2 under CiA 402's default mapping —
`statusword + position actual` out, `controlword + target position` in. A drive that has been
remapped reads a target position as something else entirely, and the frame is well-formed either
way. `open()` reads `0x1A01` and `0x1601` back and refuses a mapping it does not speak: assuming the
default is the industry's most common source of an axis that moves to a number nobody sent.

**A latched fault, before anything can clear it.** There is deliberately no `NMT reset node` in the
open path. A reset returns the state machine to `switch on disabled`, which clears a fault silently
— erasing, on every restart, the one piece of evidence about why a robot stopped. So the statusword
is read first and a faulted drive is refused, with the object to read for the code.

That ordering came out of the suite rather than the design: *"a drive that has latched a fault is
refused"* failed while a reset was still there, and it failed because the reset had already cleared
the fault the test had just set.

**The state, after each transition.** `shutdown → switch on → enable operation`, reading the
statusword back at each step. A master that trusted its own transitions would write setpoints into a
disabled drive, see them acknowledged on the bus, and see the axis stand still.

## The stop, and what it is not

`emergency_stop()` sends CiA 402 quick-stop to every axis and then NMT pre-operational, latches, and
refuses every later command. That stops the drives it can reach over the bus it has, and the
descriptor declares it as what it is — the `safety.estop` capability, a software stop. **It is not a
safety-rated stop.** A rated stop is an STO input wired to a relay and is not reachable from a CAN
frame, so the envelope carries none, and §15's `safety.estop.independent` is a bench test nobody
passes from here.

## The substrate, and its one hole

`make check` runs against `LoopbackBus`: the identical protocol over a Python list, with simulated
drives attached to it. The plan's table asks for `vcan`, which is a kernel module — `modprobe`, a
kernel with `CONFIG_CAN`, and the privilege to load it — and most containers have none of the three.
`SocketCanBus` is the same driver against the real thing, and refuses to construct on a kernel
without `AF_CAN` rather than pretending. `make check-vcan` runs the bridge on it, against simulated
drives in another process (`python3 -m skynet_canopen.drives`), on the machine's `vcan0` or on one
`vcan.sh` creates in an unprivileged network namespace — no root — and exits non-zero where it can
do neither, rather than reporting a pass it did not earn. Its first run found F71 in
`DRY-COVERAGE-PLAN.md`: the bridge's reads had only ever met a bus that answers inside the read.

The same drive server is what `drivers/canopen-rs` — this driver in Rust, Phase 6's port — is
measured against, so the two masters meet one substrate. Writing that port found F73: four paths
this driver had never run (a zero motor-side gear figure divided by, a command refused after its
first axes were sent, a second `open` stacking a second body, two joints on one node) — each with
a check here that fails without the fix, and the second of them the worst: a setpoint
past RPDO2's 32 bits was stored before its frame failed to pack, after which `emergency_stop`
raised instead of stopping.

**A loopback bus is not a bus.** No arbitration, no bit timing, no error frames, no bus-off, no
bandwidth. It shows that this driver encodes CiA 402 correctly and it shows nothing whatever about
whether eight axes fit in a millisecond at 1 Mbit. That is the one row the plan's table puts under
*not dry-verifiable* for this driver, and it stays there.

## No dependencies

`python-can` and `canopen` are both excellent and this driver uses neither, for the reason
`CONTRIBUTING.md` gives about the Rust workspace and `drivers/lerobot` demonstrates on the Python
side: a driver author should be able to clone this repository and reach a passing conformance run
with nothing installed. Expedited SDO is eight bytes with four fields in it.
