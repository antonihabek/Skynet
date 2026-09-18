# skynet-ethercat — CiA 402 drives on an EtherCAT segment

`DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this row its substrate as *"SOEM + simulated slave"*,
its dry-verifiable column as *"datagrams, working counter, AL state machine, mailbox CoE"*, and its
**not** dry-verifiable column as *"distributed clocks, jitter on a real segment"*.

```bash
make selftest    # the frames, the bring-up, the profile and every refusal. python3 and nothing else
make check       # the whole path: skynet driver check → python-host → this bridge → three drives
sudo make check-nic NIC=eth1    # the same driver on a real NIC, where there is a segment on it
```

`make selftest` is **57 checks, 0 failed**. `make check` reports **26 passed, 0 failed, 1 not
measured, 5 not applicable → guest tier** through the real ABI.

## SOEM is not linked, and that is a licence finding rather than a preference

The plan names SOEM as this row's substrate. It cannot be one. **SOEM 2.x is dual-licensed GPLv3 or
commercial**, so a driver in this Apache-2.0 repository cannot link it and still be distributed under
this repository's licence — and no arrangement of the code changes that, because the combination is
what the licence governs. The earlier rows had the opposite answer: `libfranka`'s common headers are
Apache-2.0 and `drivers/franka` compiles them; `unitree_sdk2` is BSD-3-Clause and `drivers/unitree`
links it on both sides of the wire.

So the master here is this repository's own — `frames.py`, `slave.py`, `segment.py`, `master.py` —
written against ETG.1000. SOEM keeps the one role a licence does not restrict: an **independent
master**, run separately against this slave stack over a TAP device or a real NIC, which is
interoperation rather than combination. That cross-check is *available and was not run here*: it
needs `CAP_NET_ADMIN` to create the device and root to open a raw socket on it, and this machine
grants neither. It is the sharpest test this row could have and it is honest to say it is owed.

## What the software slave stack actually models

`slave.py` is an ESC, not a mock: a register file, auto-increment and station addressing, the AL
state machine with its error bit, SyncManagers, FMMUs mapping logical addresses onto physical ones,
and a CoE mailbox carrying expedited SDO. `segment.py` passes a frame down the ring slave by slave
and back, so every datagram is serviced by position exactly as it would be on a cable.

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
