# `drivers/onrobot` — an OnRobot RG2, RG6, 2FG7 or 2FG14 gripper over Modbus TCP or Modbus RTU

`DRY-COVERAGE-PLAN.md` item 12.E.8. OnRobot's RG2 and RG6 (two fingers on a lever, one actuator) and 2FG7 and 2FG14 (two parallel fingers on a linear axis) are reached two ways: **Modbus TCP** to the Compute Box
(port 502, the tool addressed by a unit id) or **Modbus RTU** on the tool's own RS-485 (8E1, the same unit id). OnRobot publish **no register table** and no simulator. They publish a closed client library,
`libonrobot_tool_api` 1.2.0 (BSD-3-Clause, with its headers and documentation) as two `.deb` packages on GitHub, and this driver links none of it: **the registers were read off that library by running it**, against a
Modbus server that logs every request and answers from a table the experiment sets, one register at a time (`frames.py` says so, `layout_check.py` repeats the experiments). The gripper on the other end of the line is
written in this repository. It is not a gripper, and nothing the maker has written is on the other end of the line, but the maker's own client is **run** against it, so the stage is **beta** at F118's rung 3: an
independent implementation of the client side, not the maker's controller software and not the maker's simulator.

```bash
make selftest          # 623 checks: the register map, the Modbus frames, every refusal, the exchange cycle, the stop, the faults, the motions and the line, against a simulated gripper (TCP, and RTU on a pseudo-terminal; needs Linux)
make check             # and the sim half of SCTS through the real ABI
make fetch-sdk         # OnRobot's own packages from the maker's GitHub release, pinned by SHA-256 (ONROBOT_SDK=where to unpack); never committed
make build-oracle      # sim/oracle.cpp built against them (g++ and libmodbus-dev): the maker's client, as a process
make check-layout      # this driver's registers against the maker's own client, which is run (62 checks; needs ONROBOT_ORACLE)
make chaos             # the link-chaos contract on both links: the Compute Box's TCP (30 checks) and the tool bus's RTU on a serial relay (51 checks)
make faultvocab        # the status bits the maker's library names, injected and held to the class the table gives each (68 checks over 17 codes)
make check-oracle-docker   # all of the maker's-client half in a container that fetches the packages and builds the oracle at ONROBOT_RELEASE (the nightly runs it at 1.2.0 and at `latest`)
```

## What is here

| file | what it is |
|---|---|
| `skynet_onrobot/frames.py` | the registers: addresses, scales, bits, the maker's validation of a grip before it writes, the models' profiles. Pure; every constant read off the library |
| `skynet_onrobot/modbus.py` | the frames: MBAP for TCP, the CRC for RTU, the six function codes the driver uses, the exception codes |
| `skynet_onrobot/link.py` | the line: a TCP socket (the Compute Box) or a serial port (8E1), a transaction with a bound, the stale bytes of an earlier answer skipped |
| `skynet_onrobot/bridge.py` | the driver |
| `skynet_onrobot/gripper_sim.py` | a simulated gripper written here: the dry substrate. A Modbus slave (TCP on a port, RTU on a pseudo-terminal), fingers that cross their range at a speed, an object that stops a closing finger, the statuses and errors the registers carry, a supply budget that a power cycle gives back, and a lever for every way a line can go wrong |
| `skynet_onrobot/__main__.py` | the entry `drivers/python-host` serves, with `SKYNET_ONROBOT_SIM=1` for `skynet driver check` with no gripper |
| `selftest.py` | the driver's own suite |
| `layout_check.py` | this driver's registers against the maker's own client, run through `sim/oracle.cpp` |
| `chaos.py`, `faultvocab.py` | the subjects for `tools/faultproxy/chaos.py` and `tools/fault-vocab/contract.py` |
| `sim/fetch.sh`, `sim/build.sh`, `sim/oracle.cpp` | fetch the maker's packages (sha-pinned at 1.2.0), build the oracle: a process that takes `<model> <port> <call>…` and drives the maker's client against a Modbus server |
| `sim/Dockerfile`, `sim/run-oracle.sh` | the same in a container, for the nightly |

## The body

One prismatic joint, **`jaw`**: the opening between the fingertips in metres. For an RG it is the maker's *task aperture*, the width at the fitted fingertips (fingertip offset included); for a 2FG it is the external width.
It takes a position, one control mode, and its tier ceiling is Guest (§31.3).

```json
{"model": "RG2", "host": "192.168.1.1", "port": 502, "slave": 65, "limits": [0.0, 0.11], "force_n": 20, "stop_mode": "stop", "simulator": false, "roles": {"jaw": "right_hand.finger.flex#0"}, "rate_hz": 20}
{"model": "2FG7", "device": "/dev/ttyUSB0", "baud": 115200, "slave": 65, "force_n": 30, "speed": 80, "supply_power_w": 48, "stop_mode": "stop", "simulator": false}
```

* **`model`** — `RG2`, `RG6`, `2FG7` or `2FG14`, **required**. The product code at register 1536 identifies the gripper, but what a register *means* depends on the model, so the operator says which it is and the open
  checks the gripper agrees; a different product code is refused by name and nothing is commanded. The 3FG15 and 3FG25 are refused by name too: the maker offers the three-finger API for evaluation only.
* **`host` and `port`** (a Compute Box, 502) or **`device` and `baud`** (the tool's RS-485, 8E1, 115200 from the factory) — exactly one of the two. No default and no scan.
* **`slave`** — 65 (a single tool, from the factory), 66 or 67 (the two tools of a dual Quick Changer).
* **`limits`** — `[min, max]` in metres. **Required for an RG** (the maker's library makes the integrator declare the working aperture for the fitted fingertips), and for a 2FG may only narrow what the device reports.
* **`force_n`** — **required**: nothing in a position request says how hard to close. **`speed`** (1 to 100, the 2FG's native percentage) and **`supply_power_w`** (14 to 48) are a 2FG's, **required** for one, and refused
  for an RG, which has neither a speed command nor a budget.
* **`stop_mode`** (only `stop`) and **`simulator`** — **required**. `rate_hz`, `timeout_ms`, `stale_ms`, `open_timeout_ms`, `roles`. A key the driver does not read is refused, naming the keys it does.

## The stop

`stop_mode` accepts only `stop`: the maker's own stop (an RG's control word 40, a 2FG's type 3), the only one the library offers. **What it does to a held workpiece is the maker's and was not measured** (its documentation
says to support the workpiece before stopping or releasing a grip). It is a **software stop**: the gripper's safety circuit is a switch the integrator wires, and there is none on this wire.

`emergency_stop` **latches and returns**. It never touches the line, because the RHAL says it never blocks (50 ms) and a line that is slow or dead would make a stop that wrote its own control block wait for it
(Inspire's driver was found to do that, F298). The reading thread writes the stop before anything else, between two of its own reads if it has to. Commands are refused from the instant of the latch, **whatever the
line was doing**: a command that ran out of time waiting for the line while the stop was being written is `EPERM`, the stop's, and not `EIO`, a dead line's. `release_stop` clears the latch only after a reading shows
the jaw not moving after the stop was written.

## One cycle

An RG: one read of the 20-register state block at 258. A 2FG: the state block at 256, the device's own limits at 259 and the supply budget at 1028. A 2FG's supply budget is told to the gripper at open and read back, and
**told again whenever the line has been dead**, because the maker's firmware gives it back to 14 W at every power cycle and the force ceiling goes with it. `write_command` validates the grip as the maker's library does
before it writes (the same accept, the same refuse, the same rounding), makes one write of the control block, and is bounded by a quarter of a second *including the wait for the line*. A close writes nothing: the
gripper keeps its last target.

## Faults

The maker's library names the bits and publishes **no table of what any value means** (`fault-vocabulary.json`, `onrobot`): an RG's five safety bits (switch 1 and 2 pushed and triggered, the safety DC error), its
three-bit error code, and a 2FG's not-calibrated and linear-sensor-error bits. **Fail closed**: any of them is `FAULT` with `DEGRADED`, commands are refused, and the maker's library has no call that clears one (a person's
at the gripper). They are `fault` and not `estop`: an RG's switches are inputs the integrator wires, and the library does not say which one means stop. A 2FG that is not calibrated also reports homing pending, and the
driver does not calibrate it.

Motions (`extension`, token `vendor.onrobot.task`): `status` and `move width=<m> [timeout]`.

## What is measured, and how far

| rung | what | result |
|---|---|---|
| 4 | `selftest.py`: the driver against `SimulatedGripper`, written here, over TCP and over RTU on a pseudo-terminal | 623 checks |
| 4 | `skynet driver check` through python-host against it | see `docs/status.md` |
| 4 | the link-chaos contract (`make chaos`): delay, jitter, a slow link, a stall, a blackhole, and on the serial relay bursts of loss, truncated and corrupt frames | 30 checks (TCP), 51 (RTU) |
| 4 | the fault-vocabulary contract (`make faultvocab`): the bits and codes the library names, and codes outside them | 68 checks, 17 codes injected |
| 4 | defect seeding: `tools/seed-defects/seeds/onrobot.json`, 701 seeded defects: **677 of 678 noticed** (18 equivalent mutants left out, each with its reason; five controls that survive as they must). The one survivor, `onrobot-txn-write-line` (a register write made without the line's lock), is a race that the suite met in some runs and not in others; `selftest.py` now holds it without waiting for the race (the lock is asked for at the moment the line is used, on a bridge with no reading thread), and the campaign has not been run again since that check was written | `docs/dry-test-matrix.md` |
| 3 | `layout_check.py`: the maker's own client, run, against the simulated gripper while this driver reads and writes the same one | 62 checks |

What `layout_check.py` compares, with `libonrobot_tool_api` 1.2.0 on one side and `frames.py` on the other: every register range this driver reads is inside one the maker's client reads, and every write is one it makes;
random state blocks (and the signed and unsigned edges of every register) decoded by both, field by field; random valid and invalid grips against the maker's `RgGripper::move` and `TwoFingerGripper::grip`, the
same accept or refuse and the same registers; the stop; the supply budget; and the two together, the maker's client moving the simulated gripper while this driver reads where it went, and this driver moving it while the
maker's client reads where it went.

## What running it found

* **F300 — the maker's client writes a force of zero for any force under 0.05 N** (an RG told to close "with no force" is sent a register of zero), and this driver refuses it
  (`force_n` is above zero). Pinned in `layout_check.py` as the one difference there is.
* **F301 — `termios.error` is not an `OSError`.** A serial port the kernel will not configure raised out of `open` as an exception instead of returning a code, in this driver and in Inspire's and Robotiq's (the same
  `except` in three links). All three now raise `OSError` and the host's `open` returns `EIO`; each selftest has the check, and ran red without the fix.
* **F302 — a pseudo-terminal opened a second time refuses 8E1** (glibc's `tcsetattr` returns `EINVAL` on a re-opened pty when the parity it was asked for cannot be kept). The first open works and every later one met
  it, so a driver restarted on a `socat` gateway's pty (or run twice in a test) could not open its own line. The open accepts what the pty holds when it is a pty, and nothing else.
* **The stop and a line timeout were told apart by a race** (the same defect as Inspire's): a command that ran out of time waiting for the line while the stop was being written was `EIO` or `EPERM` by whichever thread got
  the lock first. The latch decides now; both drivers have a deterministic check.

## What it cannot show, and the assumptions it makes

* **A gripper.** The finger dynamics, the force a closing finger develops (a closing finger stops at an object's width at once), what a real gripper does with a command while its safety input is open (the simulation
  takes it), and what an RG's undocumented state registers hold (they read zero) are not reached. The simulated gripper is accepted by the library's identity checks because it was made to be (F15).
* **Assumption: a reading within 1 mm of the target is there.** The registers resolve a tenth of a millimetre and nothing says how close a finger rests.
* **Assumption: the map holds for firmware other than the one the library was built against.** The release is pinned at 1.2.0 and the nightly re-runs the layout check against `latest`; a firmware that moved a register
  is a gripper this driver would read wrongly, with the identity check the only guard.
* **The 3FG15 and 3FG25** are refused by name. **The gripper's own safety circuit** is not read as a stop (see `fault-vocabulary.json`).
* **Timing.** The 80 ms transaction and the 20 Hz period are this driver's own; the maker gives neither.
