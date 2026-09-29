# skynet-spot — a Boston Dynamics Spot over its gRPC SDK

`DRY-COVERAGE-PLAN.md`'s Phase 7 quadruped row: `bosdyn-client` 5.2.0 is public on PyPI and
there is no vendor simulator (community Gazebo only), so the substrate is faked gRPC services
with the vendor's client unmodified — the Pudu shape. Spot is §59.2's genuine class: the
vendor controller owns the servo loop, the API is command-level, and Guest is the ceiling.

```bash
python3 -m venv .venv && .venv/bin/pip install 'bosdyn-client==5.2.0'   # repo root, once
make check                                                            # here
```

The driver is Python and runs under `drivers/python-host`. The wire is gRPC over TLS and no
stdlib speaks either, so this row is the tree's first that needs third-party packages — and
the environment rule is the licence boundary made executable: the SDK is proprietary
(Development Kit Licence), nothing of it is copied here, the venv is untracked by design
(`.venv/` in `.gitignore`, outside every line count and every CI scan), and every gate
refuses without it. CI installs `bosdyn-client` into a temporary environment on the runner each time
(`.github/workflows/ci.yml`, "Spot, the vendor's client against simulated services"), so the row runs on
every push and nothing of the SDK is copied into this tree. (This line said "CI has no network and no
SDK, so CI does not run this row" until 2026-09-29; the runner has both, and the row had gone unrun.)

## The fake is a Spot, and it verifies the discipline

`skynet_spot/fake_spot.py` serves the eight services the driver speaks — Auth, Directory,
Estop, Lease, RobotState, RobotCommand, Power, TimeSync — on one TLS port, the way one port
serves them on the robot, distinguished by authority. On every call but the token mint it
verifies the `authorization: Bearer` token the SDK attaches; a wrong one is UNAUTHENTICATED.
What it enforces: lease discipline (no live lease, no power or motion), power discipline (no
power, no motion), timesync discipline (no clock, no motion), and the estop challenge echo —
the response is the challenge's bitwise negation, and a forgery is refused and counted.

The joints are scripted, not vendor figures: stand and sit poses distinct enough that a
motion reads unambiguously, a body height that follows them, power transitions completing at
once where a real Spot takes seconds. Three simplifications, all stated in the module: one
lease holder, estop resolved per endpoint with last check-in winning, no behaviour faults.
TLS runs the real path with a test trust root: `certs/` carries a CA (whose key was deleted
after signing) and a server certificate with every listed authority, and the SDK is pointed
at the CA explicitly. Nothing disables verification.

The deadman is modelled, because the driver's close path leans on it: a registered endpoint
that goes silent past its timeout estops the fake and cuts its power, and config surgery
while powered is refused — both the real semantics. That modelling caught a real defect
while being built: the timeout's nanos were unread, so a 0.5 s deadman silently ran as five
seconds. The suite's short-deadman cycle (open, power on, close dirty, sleep, assert the
stop) pins both halves of the duration now.

## The wire cross-check, and what the smoke caught

`cross_check.py` imports the vendor's `bosdyn-client` unmodified and walks it through the
fake: authenticate, directory, estop setup, acquire, time sync, power, stand/sit/stop/
selfright — then forges a token, a lease, a clockless command and a challenge response, and
asserts every refusal. If the fake runs the vendor's flows and refuses the vendor's
forgeries, the wire the driver speaks is Spot's.

Building that argument caught two more defects. The lease verdict compared sequences for
equality, and the SDK's keepalive retains with a sublease (`[1, 0]` against `[1]`) — the
sequence is a path, and the fake renders the prefix rule now. And every response header must
carry `error.code = CODE_OK`: the SDK reads it before any per-RPC status, and an unset code
fails the call no matter what the RPC answered.

## The driver: bring-up order, twelve joints, six motions

`open()` runs the bring-up a Spot demands, in the order it demands it: authenticate, sync
the directory, an estop endpoint with a keepalive, the body lease with a keepalive, time
sync (before the command client exists — one born without an endpoint refuses everything),
and one state read whose joint names the descriptor is checked against. A twelve-joint body
answering eleven is refused as a different body; extra joints (an arm's) are ignored and the
driver says so, because this driver is the quadruped base.

`read_state` maps by name into the vendor's enumeration, front-left to hind-right. Position
and velocity are radians; effort is NaN (`load` has no documented unit, §24). The base pose
is the vision-frame body edge or NaN; the twist is the published body velocity. The fault
word counts active system and behaviour faults with `FLAG_FAULT`; the estop flag rides when
the latch or the robot says stopped.

`write_command` refuses everything past the shape checks — there is no joint-level command
access and §59.2 says software cannot add it. The six motions under `vendor.spot.task` take
no arguments, the builders' defaults being the vendor's:

```
power_on / power_off    motors power, feedback polled once
stand / sit             synchronized mobility commands
stop / selfright        full-body stop / self-right
```

A motion while unpowered is EPERM with the remedy (`power_on first`), not EIO: the state
forbids it. Faults are reported and never cleared — clearing is an operator act. The estop
is real: `emergency_stop` CUTs, stops the keepalives, and latches permanently for the
instance; reads continue and report the stop in the flags. It is declared as `safety.estop`
and not as independent — the CUT travels the same TLS the commands do.

`open()` takes `host` (required), `port` (default 443), `username`/`password` (required),
`ca` (a PEM path for a test trust root; unset means the SDK's own robot certificate — a real
robot's), `estop_timeout` (seconds, default 5), and `simulator` (default true).
