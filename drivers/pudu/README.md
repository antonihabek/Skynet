# skynet-pudu — a Pudu service robot over the vendor's OpenAPI cloud

`DRY-COVERAGE-PLAN.md`'s Phase 7 item 4: one service-robot driver on the execute path, for a class
that today has nothing. Pudu, Temi, Keenon and Bear share a body plan from this OS's side — a
wheeled base with no joints the OS can see, moved by task calls — and of the four, Pudu is the one
whose task API is published as machine-readable OpenAPI. So this driver speaks Pudu's cloud
(delivery, cruise and call dispatch; map position and task state reads), and the class gets its
shape from the one vendor that documents it.

```bash
git clone --depth 1 https://github.com/pudu-robotics/skills /tmp/pudu_skills
make check PUDU_SKILL=/tmp/pudu_skills
```

The driver is Python and runs under `drivers/python-host` (the cloud is HTTPS and `urllib` is
stdlib; a C driver would need a TLS library the drivers here do not take on). Python's tier
ceiling is Guest and the service class is Guest ceiling anyway — a policy that dispatches
deliveries over the internet was never going to hold a real-time path.

## The fake is a cloud, and it verifies the signature

`skynet_pudu/fake_cloud.py` serves the seven OpenAPI endpoints this driver speaks on loopback
HTTP, and on every request it recomputes the HMAC-SHA1 over the string-to-sign and compares in
constant time. A driver that signs wrong gets 401 and a `bad_auth` count, not data. It also
shape-checks each task body against the documented fields (a delivery without destinations is
`malformed`, not queued) and counts unknown paths.

The signing argument has an independent half: `cross_check.py` imports the vendor's own
`pudu-request.py` unmodified, stubs its `requests` calls (nothing leaves the machine), and feeds
what the vendor's code signed into this fake's verifier — a GET and a POST, the POST carrying CJK
bytes, plus a tampered negative. The fake accepts the vendor's construction and refuses the
tampering, and the suite accepts the driver against the fake: the driver's signing matches the
vendor's through two independent implementations.

That cross-check caught a real defect while being built: the vendor serialises POST bodies with
non-ASCII unescaped, and the default escaping signs different bytes — a CJK map point would have
401'd in production while passing every test in this tree. The driver matches the vendor now, and
the suite dispatches a CJK delivery to prove it.

## The execute path, under `vendor.pudu.task`

`write_command` refuses everything: there are no joints, and §4's shape is the honest one — the
only route that moves this robot is `extension`, four motions in the pendant's text shape:

```
deliver points=A01,B02     POST delivery_task: one tray, N destinations by map point
cruise line=Loop-A         POST cruise_task: by cruise-line name
call point=A01             POST custom_call: by map point
cancel_call id=task-0003   POST custom_call/cancel: by task id
```

The §84.1 motion list declares the same four with their required parameters, which is what lets a
0-joint body reach a tier at all: conformance asks for "at least one joint or motion", and a
program-level body answers with motions. Unknown tokens answer ENOSYS, malformed input answers
EINVAL with the usage beside it, and a cloud refusal arrives as EIO with the cloud's words.

`read_state` reports zero joints and a base pose from the cloud's map position — x, y, z with a
NaN quaternion, because the cloud publishes no yaw and zeros would be a claim. The fault word
stays 0: task states are free strings and Pudu documents no fault vocabulary, so there is nothing
honest to derive one from.

## Where it stops

| | |
|---|---|
| **Guest tier** | Reached. 63 checks in its own suite, 32 through the real ABI |
| **Stopping a delivery** | Not in the API. No cancel endpoint exists for delivery, transport, errand or cruise tasks — only calls cancel — so `emergency_stop` latches locally, refuses every later motion, and says in words that it cannot recall the cloud's in-flight task. No `safety.estop` capability is declared, because the latch is not a stop |
| **Transport and errand tasks** | Same shape as delivery (`transport_task`, `task_errand`), not wired to motions yet — four verbs cover the class's breadth; the rest is one motion each when somebody asks |
| **Faults** | Undocumented vocabulary; the word stays 0 and transport failures arrive as EIO |
| **`rhal.state.rate`, `rhal.command.latency`** | A bench measurement, as ever — and over the internet, the interesting one |

## What the descriptor declares, and what it will not

Zero joints, `wheeled` base, no control modes, the four §84.1 motions, the `vendor.pudu.task`
extension, no capabilities, no sensors — and **no envelope**. A delivery robot's limits are map
speed and battery, neither of which Pudu publishes per call, and the envelope is not where a guess
goes.

Config: `sn` (required — the cloud addresses robots by serial, and a MAC in this field is refused
the way the vendor's tool refuses it), `app_key`/`app_secret` (required — every call is signed),
and either `cluster` (`cn`/`sea`/`de`/`us`, the vendor's hostnames) or `base_url` (tests point it
at the fake). `open()` probes the cloud once, so wrong credentials fail there rather than on a
later call.

## Licences

This directory is this tree's licence. It speaks Pudu's OpenAPI (functional endpoint shapes, from
the vendor's published skill) and implements the documented signing scheme in its own code — the
skill repo carries no licence file, so nothing is copied from it; `cross_check.py` only imports
the vendor's script at check time.
