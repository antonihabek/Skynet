# `drivers/schunk` — a SCHUNK EGU, EGK or EZU gripper over its module's web interface

`DRY-COVERAGE-PLAN.md` item 12.E.9. SCHUNK's EGU, EGK and EZU electric grippers carry a fieldbus module (an HMS Anybus board) that serves a small **web interface** next to its fieldbus: two routes, `GET /adi/data.json`
and `POST /adi/update.json`, over which a controller reads the gripper's status and writes its commands as two 16-byte frames. SCHUNK publish a client for it and a protocol simulator in one repository
(`SCHUNK-SE-Co-KG/schunk_mechatronic_gripper`, GPL-3.0). This driver speaks that interface. It links nothing of the maker's: the wire was read out of both, `layout_check.py` **runs the maker's client** and
holds this driver's bytes to it, and the live block runs the driver against **the maker's own simulator with the maker's own client reading the same module**: F118's rung 2. It is not a gripper.

```bash
make selftest          # 202 checks: the frames and every refusal, against a simulated module written here
make check             # and the sim half of SCTS through the real ABI (37 passed)
make chaos             # the link-chaos contract: the module's one TCP port through a relay that delays, stalls and blackholes it (30 checks)
make check-layout SCHUNK_PYTHON=/path/to/python SCHUNK_REPO=/path/to/schunk_mechatronic_gripper    # this driver's bytes against the maker's client (21 checks)
make check-sim-docker  # all of it against the maker's own simulator and client, in a container
```

## What is here

| file | what it is |
|---|---|
| `skynet_schunk/frames.py` | the wire: the two routes, the two instances, the bits of both double words, the module and fieldbus enumerations, the parameters, the encoding of a command |
| `skynet_schunk/bridge.py` | the driver |
| `skynet_schunk/gripper_sim.py` | a simulated module written here: the dry substrate, with a jaw that moves in a thread of its own at the speed it is told |
| `skynet_schunk/__main__.py` | the entry `drivers/python-host` serves, with `SKYNET_SCHUNK_SIM=1` for `skynet driver check` with no gripper and no container |
| `selftest.py` | the driver's own suite |
| `layout_check.py` | this driver's bytes against the maker's client, which is run |
| `live.py`, `sim/` | the live block, the substrate image (the maker's repository at a pinned commit, its two packages and `pytest`), and `sim/dummy_recorder.py`, the maker's own `main()` with one line that writes down each frame its simulator is handed |
| `chaos.py` | the subject for `tools/faultproxy/chaos.py` |

## The body

One prismatic joint, **`jaw`**, in metres (the module counts micrometres), from the module's own minimum position to its maximum, which **the module reports** (parameters `0x0600` and `0x0608`) and a configuration may
narrow and never widen. It declares one control mode, **position**, a gripper end effector whose opening is the travel, and a base that is `fixed`. It is a Python driver (`drivers/python-host` runs it) and its tier
ceiling is Guest (§31.3).

```json
{"host": "192.168.1.20", "port": 80, "stop_mode": "controlled", "speed": 0.02, "simulator": false,
 "limits": [0.01, 0.1], "roles": {"jaw": "right_hand.gripper.grip"}}
```

* **`stop_mode`** — **no default**. `fast` writes the module's fast stop (control bit 0 low), which puts it in error `0xD9` until it is acknowledged; `controlled` writes the stop bit, which ends the motion with no error.
  Neither releases a workpiece. What stopping a gripper means is a person's decision.
* **`speed`** — **required**, in m/s, between the module's own minimum and maximum: nothing in a position command says how fast the jaw goes.
* **`simulator`** — **required**: nothing in the module says whether it is a simulation, and a bench run described as one costs a tier it could have had.
* `limits` (narrower than the module's), `roles`, `rate_hz`, `http_timeout_ms`, `stale_ms`, `open_timeout_ms`. A key the driver does not read is refused, naming the keys it does.

## What it refuses to bring up

A **PROFINET** module (the maker's client reads the error byte at byte 12 of the input frame for every fieldbus, and the maker's own PROFINET dump has the code at byte 15: nothing here can say which is right, F249), a
**Modbus RTU** module (this is the web interface and there is no serial port to open), a module that says it is **not ready** and holds no error, and a configuration that does not give the three required keys.

**A module that reports an error opens with the stop latched.** The maker's simulator comes up in a fast stop (error `0xD9`, status bit 7) the way a real module does until its controller acknowledges it; `release_stop`
is the maker's acknowledge and is **verified to have cleared the error**. Nothing here acknowledges anything by itself.

## A command, and the stop

**A command is the maker's handshake** (`Driver._send_cmd`): clear the control word, read the command-received toggle, write the command, wait for the toggle to flip, under a lock the stop also takes, and with the
stop latch looked at **immediately before the command frame is written**, so a command is never written after a stop. The whole handshake is given **0.2 s** (the control loop waits for it; the contract's bound is a
quarter of a second, F256), each request is given what is left of that and nothing more, and a handshake that cannot finish in time is `EIO` and not a late write. A target the module was last given is not sent again,
except after a grip or a release, which take the jaw somewhere the driver did not choose (F253). A refusal is whole, names its cause and says so once for a run of the same cause.

**The stop** latches and returns in well under a millisecond; a thread of its own tells the module, asked again on a lost request. It is a **software stop**: the module's own fast-stop input is a circuit. A close
writes nothing — what a stop does to a held workpiece, with grip force and position maintenance or without, is in the maker's firmware manual, which is behind a sign-in this repository does not use.

Motions (`extension`, token `vendor.schunk.task`): `status`, `param name=…`, `move position=… [speed=…]`, `grip force=… [position=…] [velocity=…] [outward=1]`, `release`.

## What is measured, and how far

| rung | what | result |
|---|---|---|
| 4 | `selftest.py`: the driver against `SimulatedGripper`, a module written here | 202 checks |
| 4 | `skynet driver check` through python-host against it | 37 passed |
| 4 | defect seeding: `tools/seed-defects/seeds/schunk.json`, 130 seeded defects and two controls: **109 noticed on the first run, and a check written for each of the 21 that were not** (each shown red against its seed); the re-run had not finished | `docs/dry-test-matrix.md` |
| 4 | the link-chaos contract (`make chaos`): the module's port through delay, jitter, a slow link, a stall and a blackhole | 30 checks |
| 2 | `layout_check.py`: this driver's bytes against the maker's client, which is run | 21 checks |
| 2 | `live.py` against the maker's simulator, with the maker's client reading the same module and a record of every frame the maker's server was handed | **83 of 83**, and the sim half of SCTS through the real ABI **37 passed, 0 failed** (`make check-sim-docker`) |

The substrate is **rung 2**: the maker's own simulator and the maker's own client are on the other end. The module's firmware, the motor and the jaw are not.

## What running it found

* **F249 — the maker's client and the maker's own PROFINET dump disagree on where the error byte is.** The client reads byte 12 for every fieldbus; the simulator's PROFINET dump has `0xD9` in byte 15. Run through the
  maker's own getters that frame reads error `0x0`. The driver refuses a PROFINET module, and says why.
* **F250 — the maker's client imports `pytest` at import time and does not declare it** (`utility.py`, for a `skip_without_gripper` marker), so `import schunk_gripper_library.driver` fails on a clean install.
* **F251 — the maker's simulator comes up in a fast stop, as a real module does** (status bits 7 and 31, error `0xD9`, and every command, the maker's client's included, fails until an acknowledge). An EGK 25 dump
  comes up with `0xEF`, a code the maker's client does not name.
* **F252 — two facts about the maker's simulator.** A grip with nothing between the jaws closes to the minimum position and reports *gripped*. It reads the parameters `0x0600` and `0x0608`, which the client decodes as
  floats in millimetres, as integers after reversing the bytes, so an outward grip ends at 123,012 µm where the module says its travel is 120 mm.
* **F253 — a grip or a release left the "already issued" target in place (the driver's).** A stream that went on asking for the old place was swallowed with the jaw elsewhere and nothing said. Found by reading the
  first probe against the maker's simulator (`target_m` 0.05 with the jaw at 0.0), shown failing in the selftest first.
* **F254 — the maker's simulator clears *ready for operation* on every write, the cleared frame included.** `process_control_bits` begins by clearing every valid status bit but 5 and 7 and bit 0 is one of them; the
  maker's client never reads the bit again after an acknowledge. Two consequences, both named in `live.py`: the driver reports the module degraded (not faulted) after its first command, and **a new session on the same
  simulator is refused** ("not ready, and holds no error: do it where the gripper is") until the operator acknowledges it, which the block does with the maker's own client.
* **F255 — the maker's simulator neither stops a motion nor replaces one.** Its `move` loop never looks at a stop, so a fast stop sets error `0xD9` and the jaw goes on to its target; and two moves in flight write the
  position register from two threads, so the one that finishes last wins, which can be the *older* command (0.11 m, then 0.05 m 0.3 s later, ends at 0.11 m). A stop is therefore held to the frame the module was
  handed and the error it reports, never to the jaw halting.
* **F256 — a command could take 256 to 336 ms against the control loop's quarter of a second (the driver's).** The handshake gave every request at least 50 ms whatever was left. Found by the link-chaos contract
  (5 of 30 red, the slowest write 256 to 329 ms on every degraded link) and by a selftest of its own (336 ms); the budget is 0.2 s and a request is given what is left of it.
* **The seeding run found twenty-one behaviours with no check, and a fault in the campaign tool.** A speed of zero, a module in error with no error byte, the latch looked at before anything else, a 503 taken
  as an answer, a write the module refused taken as written, the descriptor's `verified`, `control_modes` and `safety.estop`, a move past the travel, a move with no speed, a latch carried into a new session, the
  target forgotten after a stop and after a release that did not finish: each passed the whole suite, and each has a check now (the simulator gained `error_on_command`, `refuse_writes`, `garble_reads` and `jammed`
  to reach them). Two older checks were vacuous (one passed on the previous sample, one accepted either of two refusals). And 24 seeds had been counted as killed because the release binary the last step runs
  had been deleted: an exit of 126 or 127 is now `invalid`.

## What it cannot show

* **A gripper.** The jaw's dynamics, the grip force in newtons (the module takes a percentage of a rating this driver cannot see), a real workpiece, the motor's heat and the module's firmware are not reached. The
  maker's simulator moves a position register on a timer and is minimal on purpose.
* **What a stop does to a held workpiece.** The manual is behind a sign-in.
* **A fault vocabulary.** The maker's repositories carry no table of error codes, so the fault word carries the module's own byte and `0xD9` is the only code with a name. `drivers/fault-vocabulary.json` has nothing to bind.
* **An EZU, an EtherCAT module, or any size but the two the simulator offers.** The driver reads the module type and the travel from the module; the others are claimed from the maker's client.
* **Timing.** One web request per control period over a LAN is a path for a Guest-tier driver at a few hertz (a web interface is how a controller configures a module, not how it servo-controls a jaw).
