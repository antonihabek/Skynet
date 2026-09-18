# skynet-ethercat — CiA 402 drives on an EtherCAT segment

`DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this row its substrate as *"SOEM + simulated slave"*,
its dry-verifiable column as *"datagrams, working counter, AL state machine, mailbox CoE"*, and its
**not** dry-verifiable column as *"distributed clocks, jitter on a real segment"*.

```bash
make selftest    # the frames, the bring-up, the profile and every refusal. python3 and nothing else
make check       # the whole path: skynet driver check → python-host → this bridge → three drives
make check-soem SOEM=~/src/SOEM/build   # SOEM, an independent master, against these slaves
sudo make check-nic NIC=eth1    # the same driver on a real NIC, where there is a segment on it
```

`make selftest` is **87 checks, 0 failed**. `make check` reports **26 passed, 0 failed, 1 not
measured, 5 not applicable → guest tier** through the real ABI. `make check-soem` passes against
SOEM v2.0.0: every slave's SII read, its process data sized from the CoE mapping, the ring at Op and
every cycle at the working counter ETG.1000.4 predicts.

## SOEM is not linked, and that is a licence finding rather than a preference

The plan names SOEM as this row's substrate. It cannot be one. **SOEM 2.x is dual-licensed GPLv3 or
commercial**, so a driver in this Apache-2.0 repository cannot link it and still be distributed under
this repository's licence — and no arrangement of the code changes that, because the combination is
what the licence governs. The earlier rows had the opposite answer: `libfranka`'s common headers are
Apache-2.0 and `drivers/franka` compiles them; `unitree_sdk2` is BSD-3-Clause and `drivers/unitree`
links it on both sides of the wire.

So the master here is this repository's own — `frames.py`, `slave.py`, `segment.py`, `master.py` —
written against ETG.1000. SOEM keeps the one role a licence does not restrict: an **independent
master**, run separately against this slave stack over an interface, which is interoperation rather
than combination.

## SOEM against this stack, and what it took

`make check-soem` makes a veth pair inside a network namespace of its own (`unshare -rn` — no root,
no NIC), serves the ring on one end with `python -m skynet_ethercat.wire`, and runs SOEM's own
`slaveinfo -map` and `simple_ng` on the other, unmodified. Then it runs this repository's master over
`RawSocketSegment` on the same wire — the first time that class has run at all.

A master and a slave written by one author agree whether or not either agrees with ETG.1000, and the
first version of this stack had only ever met this repository's master. Pointed at SOEM, it would
not have reached Pre-Op: SOEM reads the SII before it configures anything, and there was no SII.
Building what SOEM needs, and reading SOEM's source to know what that was, found the rest:

| What | The first version | Now |
|---|---|---|
| SII EEPROM | none; the EEPROM registers read zero | ETG.2010 image — identity, mailboxes, general, FMMU, SM, PDO and string categories — through the ESC's EEPROM interface (`sii.py`) |
| PDO mapping objects | none | `0x1C00`, `0x1C12`/`0x1C13`, `0x1600`/`0x1A00`, read-only, derived from the same definition as the SII and the image |
| AL status codes | three on the wrong row of ETG.1000.6's table | the table's rows, pinned by number |
| AL state machine | Init → Op allowed if configured | ETG.1000.6's transitions; the rest refused 0x0011 |
| A mailbox | any write delivered, any read answered | whole-buffer writes and reads, empty reads and full writes denied — and `master.py` wrote 16 bytes of 128, which no real ESC would have taken |
| An FMMU | mapped to the drive's inputs whatever it pointed at | mapped by physical address — SOEM maps one onto every slave's mailbox status |
| `LRW` working counter | +1 for a slave it only wrote | +2, per ETG.1000.4 |
| `BRD` | last slave's value | OR of every slave's |
| DL status | zero — a segment with no links | ports 0 and 1 by position on the line |

Each has a check in `make selftest` that needs no SOEM, and each check was held to a control: the rule
reverted on its own, and the check beside it fails. The cross-check has one too — an SII that reads as
zeros, which is the first version's, fails `check-soem` on eight assertions.

## What the software slave stack actually models

`slave.py` is an ESC, not a mock: a register file, an SII EEPROM behind the EEPROM interface,
auto-increment and station addressing, the AL state machine with its error bit, sync managers over
process RAM in mailbox and buffered mode, FMMUs mapping logical addresses onto physical ones, and a
CoE mailbox carrying expedited SDO. `segment.py` passes a frame down the ring slave by slave and back,
so every datagram is serviced by position exactly as it would be on a cable — and `wire.py` passes
frames from a real interface round the same ring, through the same function.

The one number that makes any of this a measurement is the **working counter**. A slave that
services a datagram increments it; a master that ignores it reads the right number of bytes from a
segment with nothing on it and calls them a position. So the master asserts the counter it expects
on every exchange, and a drive that drops off the ring is a refusal rather than a short read. That
check was held to a control: until `segment.py` stopped copying its slave list, removing a drive
could not fail it, and it did not.

## The profile is imported, not rewritten

CoE is CANopen's application layer over EtherCAT's transport, so the CiA 402 state machine, the
statusword decoding and the increments-to-radians arithmetic come from `drivers/canopen`'s
`skynet_canopen.cia402` rather than being written a second time. Two implementations of one state
machine disagree exactly once: after the failure that matters (§80).

That import carries the refusals with it. A drive that will not report its encoder resolution and
gear ratio is refused rather than assumed (§5.2 — a unit with no provenance is worse than a limit
with none, because it is applied silently to every number the body reports). No joint declares
`effort_max_nm`: `0x6076` is the *motor's* rated torque, a design value and not a joint safety
rating (§63.4). Velocity reads NaN because this PDO map carries no `0x606C`, and a zero there would
read as "this joint is still" (§24).

## What this cannot tell you

* **Distributed clocks.** `0x0910` and the drift-compensation loop are what make an EtherCAT segment
  synchronous, and their whole content is propagation delay measured on copper. A loopback segment
  has no propagation delay to measure, so a DC implementation exercised here would be arithmetic
  over numbers this repository invented. It is not implemented, and it is not stubbed.
* **Jitter, and therefore rate.** The cycle here runs as fast as Python runs. A real segment's cycle
  time is the thing an operator cares about and nothing measured here bears on it.
* **FSoE.** Safety over EtherCAT is a certified protocol with a certified implementation, and a
  dry-verified one would be the exact failure §63.4 is about: a safety claim resting on evidence
  nobody could have collected. `simulator: true` caps this row at guest (§35.3) regardless.

And, smaller, the parts of an ESC this one does not model, named so a green `check-soem` does not
imply them: EEPROM ownership (`0x0500`/`0x0501` — the master always has the EEPROM here), the
*multiple write* half of `ARMW`/`FRMW` (only distributed clocks use them), bit-granular FMMUs, sync
manager watchdogs, and a drive that holds Safe-Op until its first outputs arrive. SOEM exercised none
of them, so its passing says nothing about them either.

**Over a NIC, a master cannot tell these drives from hardware** — that is what makes `check-soem` a
cross-check. It is also why the bridge must not be pointed at `wire.py`: its descriptor's `simulator`
flag comes from the segment's type, and over an interface the driver would describe a real body. The
cross-check runs SOEM and the bare master, not the bridge, for that reason.
