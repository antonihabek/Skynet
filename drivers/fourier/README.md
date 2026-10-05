# skynet-fourier — a Fourier GR-1 humanoid over RoCS

`DRY-COVERAGE-PLAN.md`'s Phase 7 item 3: the buyable humanoids after the Unitrees. Fourier
publishes the interface as an MIT-licensed Python SDK (`fftai/rocs_client_py`): an HTTP+WebSocket
server on the robot, joint commands as WebSocket `move_joint` messages addressed by `(no,
orientation)`, per-motor position/velocity/current over HTTP. No simulator ships with it, so the
substrate is a loopback fake of the documented shapes — the Pudu arrangement, one class up:
joints this time, not tasks.

```bash
git clone --depth 1 https://github.com/fftai/rocs_client_py /tmp/rocs_client_py
make check FOURIER_SDK=/tmp/rocs_client_py WSCLIENT=/tmp/websocket-client-1.9.0
```

The driver is Python and runs under `drivers/python-host` (the wire is HTTP+WebSocket and
`urllib`/`socket` are stdlib; the one framing they lack is `wscodec.py`). Python's tier ceiling
is Guest and a humanoid commanded over a socket was never holding a real-time path either.

## The fake is a RoCS server, and it counts the clamps

`skynet_fourier/fake_rocs.py` serves the endpoints this driver speaks — limit/list, PVC, stop,
the `/ws` command channel — plus the doc-example shapes the cross-check's vendor client touches
(joint limits, joint states, stand, the states push). `move_joint` applies to the fake's motors
and clamps to the fake's ranges, counting every clamp: the vendor's client pre-clamps
(`_cover_param`), the driver pre-clamps to the live limits, so a run in which `clamped` is
nonzero is a run in which somebody sent what the robot would not take. That counter caught the
control it was built for: with the driver's clamp deleted, the suite fails on exactly the two
checks that assert the fake never clamped.

The wire argument has an independent half: `cross_check.py` imports the vendor's `rocs_client`
unmodified and drives the fake with it — their framing against this tree's codec, their Motor
and Human against this fake's endpoints, including a pushed debug state arriving on their
`on_message`. If the vendor's construction drives this fake, and the suite drives this driver
against the fake, the driver's wire matches the vendor's through two independent
implementations.

That cross-check caught two real defects while being built. The vendor's `Motor.__init__` takes
`port` and drops it — `super().__init__(ssl, host)` — so their Motor speaks only to 8001, and
the cross-check serves the fake there. And their `Camera`, constructed with every client, probes
`GET /control/camera_status`, which the fake now serves. Both are theirs, worked around without
touching their code.

## Joints are motors, discovered live

RoCS publishes no motor inventory and no name↔(no, orientation) map — the motor list arrives at
runtime off `/robot/motor/limit/list` — so the driver exposes what the robot reports, named by
motor address (`1-left`), with travel in radians from the live ranges. Nothing here names a
joint the robot did not report, and canonical roles (§5.1) wait for a human: the config takes a
`roles` map, validated against the grammar at open, and without one the joints match by exact
name only. That is the ceiling the roles test states, truthfully.

`move_joint` moves to angles and RoCS publishes no joint-velocity or joint-torque command, so
this body is position-only and other modes are refused, not emulated. Masked joints hold by
sending nothing for them. Commands are clamped to the live limits, the vendor client's rule.

## Where it stops

| | |
|---|---|
| **Guest tier** | Reached. 110 checks in its own suite, 34 through the real ABI (the roles test stays red: no operator, no map, no inference) |
| **The pipe carries per-joint modes now** | The first joint driver behind `python-host` caught the host dropping `modes[]` silently — a per-joint torque command crossed as body-mode position and read as success. The `C` line carries the array, both ends validate it, the Fourier suite pins it |
| **`safety.estop.latency`** | `POST /robot/stop` is the vendor's documented emergency path ("takes precedence", "in emergency situations") and the driver calls it — but it is software over HTTP to the onboard server, not an independent circuit, so the descriptor claims no `safety.estop` and the latch is what refuses later motion |
| **Velocities, efforts, base pose** | All NaN. The PVC velocity has no published unit, the current has no published torque conversion, and the pushed `basestate` is an undocumented frame — §24 throughout |
| **Contact, gravity, a body on the ground** | Not in the substrate at all, and a legged body is where that matters most — a humanoid that cannot fall over is the larger lie |

## What the descriptor declares, and what it will not

Travel per joint, from the live limit list, because that is what the machine reports about
itself. The envelope carries a **torque ceiling only**, the Unitree precedent with the same
hedge: 230 N·m is the GR series' published peak across the body (RoCS docs, "About GR series
robots"), not a per-joint rating, and no joint velocity is published anywhere — so
`joint_ratings` stays unmet and the `source` says so (F64) rather than inventing one. The
watchdog window is driver design, stated as such: the RoCS rate is unpublished.

Config: `host` (default 127.0.0.1), `port` (default 8001), `ssl`, `simulator`, and `roles`
(joint name → §5.1 role). `open()` probes the limit list and opens the command channel, so a
wrong address fails there rather than on a later call.

## Licences

This directory is this tree's licence. It speaks RoCS (endpoint and message shapes from the
vendor's MIT-licensed client, read — not copied) and implements RFC 6455 framing in its own
code. The vendor repo and `websocket-client` are imported at check time only, never vendored.
