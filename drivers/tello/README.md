# `drivers/tello` — a Ryze (DJI) Tello or Tello EDU over the SDK 2.0 User Guide's UDP text commands

`DRY-COVERAGE-PLAN.md` item 12.E.11. A Tello is a small quadcopter that is, to a program, a UDP socket: **commands on port 8889** as text (`command`, `takeoff`, `forward 100`, `rc 0 0 50 0`), answered with `ok`, `error` or a
value, and **a state string pushed to port 8890** of whoever spoke to it, ten times a second (`pitch:0;roll:0;yaw:0;…;h:80;bat:87;…`). Both are in the maker's SDK 2.0 User Guide. Ryze published a sample client of the guide
(`Tello3.py`) on a download address that no longer answers, and no simulator, so **no software of the maker's can be run**: the independent evidence is **djitellopy**, the client of the same guide that everyone else uses, run
unmodified against a drone written in this repository. It is another person's reading of the same document, which is why the release stage is **alpha** (F118's rung 3 is the best this wire has; the stage table asks for
the maker's software or simulator for beta, and a client of the guide held to bytes is a transcription check, as Inspire's two are).

```bash
make selftest          # 569 checks: the frames, every refusal, how the drone is opened, the state, the motions, the stop, the keepalive and the lines, against a simulated drone on loopback ports (needs Linux)
make check             # and the sim half of SCTS through the real ABI (34 passed, 1 not measured: the descriptor's signature, which a driver cannot make)
make check-layout      # this driver's text and state string against djitellopy, which is run (55 checks; TELLO_PYTHON=a python with requirements-layout.txt installed by hash, --no-deps)
make chaos             # the link-chaos contract: the drone's command and state flows through relays that delay, stall, lose, repeat, reorder, truncate and corrupt them (68 checks)
make check-sdk-docker  # the layout half in a container that installs djitellopy at TELLO_DJITELLOPY (2.5.0 pinned by hash; `latest` in the nightly's second run)
```

## What is here

| file | what it is |
|---|---|
| `skynet_tello/frames.py` | the wire: the guide's commands with their ranges, the replies, and the state string, parsed and encoded. Pure |
| `skynet_tello/link.py` | the two UDP sockets: a datagram from any address but the drone's is dropped and counted |
| `skynet_tello/bridge.py` | the driver |
| `skynet_tello/drone_sim.py` | a simulated Tello written here: the dry substrate. It answers `ok` when a motion is done, streams its state at 10 Hz, drains a battery, lands itself after 15 s without a command, and has levers for a reply that is lost, a state that stalls or is garbage, and an error on the next command |
| `skynet_tello/__main__.py` | the entry `drivers/python-host` serves, with `SKYNET_TELLO_SIM=1` for `skynet driver check` with no drone |
| `selftest.py` | the driver's own suite |
| `layout_check.py` | this driver's text and state string against djitellopy, run, with djitellopy and the driver each moving or reading the same simulated drone |
| `chaos.py` | the subject for `tools/faultproxy/chaos.py` |
| `sim/Dockerfile`, `sim/run-sdk.sh` | install djitellopy (pinned, or the newest) and run the layout half in a container |

## The body

An aerial body with **no joints**. It moves by motions under the token `vendor.tello.task` (an `extension` call, `<motion> k=v`), in metres, degrees and percent: `takeoff`, `land`, `hover` (the guide's `stop`), `move`
(`forward=1.0`, from 0.2 to 5), `rotate` (`cw=90` or `ccw=90`), `speed` (`v=0.5`, from 0.1 to 1), `drive` (the four sticks, `rc`, in percent) and `status`. The pose is the height only: the guide gives the attitude
angles and the speeds no sign or unit, and the Tello has no horizontal position, so the rest of the pose is NaN. A joint command is refused (`EPERM`). The tier ceiling is Guest (§31.3).

```json
{"host": "192.168.10.1", "stop_mode": "hover", "simulator": false, "keepalive_s": 5, "low_battery_percent": 10}
```

* **`host`** — the drone's IPv4 address (192.168.10.1 from the factory, in its own access point). No host names and no scan: a datagram is believed by the address it came from.
* **`stop_mode`** and **`simulator`** — **required**. Nothing a Tello says tells it from an emulator of one, so the operator says which, and the descriptor carries the claim. `stop_mode` is `hover` (the guide's `stop`),
  `land`, or `emergency` (which cuts the motors and drops it).
* `port`, `state_port`, `local_port`, `keepalive_s` (0 turns it off), `reply_timeout_ms`, `takeoff_timeout_ms`, `stale_ms`, `open_timeout_ms`, `low_battery_percent`. A key the driver does not read is refused, naming the keys it does.

## What it never does on its own

**A motion is never retried.** A lost `ok` means the drone may have flown the motion, so it is `EIO`, saying so; only the handshake and reads are retried. **A request that is not a whole number of centimetres is refused
and never rounded.** It does not take off, land or turn on anything without being asked, and a close writes nothing: the drone keeps doing what it was told, and what stopping it does is the integrator's to decide. A
drone nobody speaks to lands itself after 15 s, so while a session is open a keepalive reads the battery every `keepalive_s` to feed that watchdog.

## The stop

`emergency_stop` **latches and returns**: it never touches the socket (the RHAL says it never blocks). A thread sends the operator's `stop_mode` three times, and the stop is counted *delivered* when a state string after
it shows the drone hovering (or on the ground), not when the drone says `ok`. Motions are refused from the instant of the latch, whatever the link was doing. `release_stop` clears the latch only once the stop has been
*seen* delivered; behind a dead link it does not, and the driver stays latched until it is closed and opened, which is a new session (F76). It is a **software stop**: nothing here is the drone's own safety.

## What is measured, and how far

| rung | what | result |
|---|---|---|
| 4 | `selftest.py`: the driver against `DroneSimulator`, written here, on loopback ports | 569 checks |
| 4 | `skynet driver check` through python-host against it | 34 passed, 0 failed, 1 not measured (the descriptor's signature, which a driver cannot make), 6 not applicable; the evidence supports guest |
| 4 | the link-chaos contract (`make chaos`): delay, jitter, a stall, a blackhole, loss, bursts of loss, duplicates, reordering, truncation and corruption on both flows | 68 checks |
| independent | `layout_check.py`: djitellopy 2.5.0, run, against the same simulated drone while this driver reads and writes it | 55 checks |
| 4 | defect seeding: `tools/seed-defects/seeds/tello.json`, 575 seeded defects | **no result yet.** The campaign was killed at 251 seeds when the machine was powered off under it: 238 noticed, 13 survived, and all 13 are labelled controls (equivalent mutants, each with its reason). One seed labelled a control, `tello-bridge-teardown-wake`, was noticed (by a two-second wait on a thread, under the load of the run); if it is noticed again alone it is mislabelled. A rate needs the whole run: `node tools/seed-defects/seed-defects.mjs --driver tello --jobs 14 --report` (about an hour and a quarter on 16 cores) |

## Findings

* **F324 — djitellopy sends a control command again when its reply is lost.** `layout_check.py` drops the `ok` of `forward 100` and the simulated drone flew it twice. This driver never retries a motion. Pinned as a difference on purpose.
* **F325 — djitellopy takes any reply with `ok` inside it for success.** `okay` and `error ok` are successes to it; this driver reads the reply as a whole. Pinned.
* **F327 — a send the network refuses leaves the link marked dead until something is sent that it carries.** Found writing the check for the dead-link message in the sixty-first session's survivors. A motion is refused while the link is dead, with the cause, and the datagrams that clear it are the keepalive's and the stop's: with `keepalive_s: 0` one refused send leaves the driver refusing motions until a stop is called and released (the stop goes out, a send the link carries, and the release waits to see it delivered) or the driver is opened again. Left as it is, and pinned: with the keepalive on, which is the default, it recovers within `keepalive_s`, and the drone lands itself after 15 seconds without a word in any case.
* djitellopy sends a distance, an angle or a speed outside the guide's range and lets the drone refuse it; this driver refuses all four before the wire (pinned). Its `retry_count` is a count of attempts, not retries.

## What this does not show

No Tello was run, and the simulated drone is accepted by djitellopy because it was made to be (F15). The assumptions the simulator makes that the guide does not state are in `drone_sim.py`'s header: a drone not sent
`command` says nothing, an interrupted motion is not answered, `rc` and `emergency` get no reply, a motion sent while another runs is an `error`, and the default speed is 10 cm/s. The guide has **no table of error codes**,
so there is no fault-vocabulary contract (`drivers/fault-vocabulary.json` says why). The link-chaos run shows the driver's own waits, staleness and stop path against a worse link, and nothing about a real Tello's radio.
