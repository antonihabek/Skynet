"""The Franka bridge's subject for `tools/faultproxy/chaos.py`: the FCI's two transports on one loopback, with the kernel's own link emulation on it.

    make -C drivers/franka chaos       # runs this in a network namespace of its own

The FCI is a TCP command port and a UDP state stream that the Control box sends to *a port the driver announces in `Connect`*: the robot sends wherever the driver says, so no relay with an address of its own can stand in
front of it, which is why the plan's item 12.C.4 listed `franka` and `franka-rs` as not bindable. The kernel's link emulation needs no address. The driver and the fake Control box share the loopback of a network
namespace of their own, and every packet either sends, TCP and UDP alike, meets the profile in both directions at once (`tools/faultproxy/relays.py`'s `NetemRelay`; it cannot carry a stall, a truncation, a reset or a
fault in one direction, which `profiles` leaves out). The TCP is the kernel's own, so a lost, duplicated, reordered or corrupt segment is what real TCP makes of it (a retransmission, a late reply, a connection that
takes seconds to find its feet again) and not the gateway's fault a user-space relay shows a stream.

The substrate is `skynet_franka.sim.FakeControlBox`, the one the suite uses, in this process: a TCP command port, a UDP stream at 500 Hz from a thread, and an arm that goes to the position it is commanded in. **The fake
agrees with whoever wrote it** (F15): what this shows is the driver's own waits, staleness and stop path against a link that delays, loses, repeats, reorders, corrupts and blackholes, and nothing about Franka's Control
box. The poses are held for `HOLD_S` and not changed with every call: the fake moves the arm the instant a command arrives, so a target that flipped every few milliseconds would make every reading look old, which no
link has anything to do with.
"""

from __future__ import annotations

import json
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "bindings", "python"))
sys.path.insert(0, HERE)

import skynet_rhal as rhal  # noqa: E402
from skynet_franka.bridge import FrankaBridge  # noqa: E402
from skynet_franka.sim import FakeControlBox  # noqa: E402

#: Two far-apart poses inside every joint's published travel, so the arm is always on its way from one to the other.
POSES = (
    [0.4, -0.5, 0.3, -2.0, 0.2, 1.8, 0.5],
    [-0.4, 0.3, -0.3, -1.4, -0.2, 2.6, -0.5],
)

#: How long one pose is held before the other is asked for.
HOLD_S = 0.6


class Subject:
    name = "franka (the FCI's TCP command port and UDP state stream, the kernel's link emulation on the loopback)"
    flows = [("netem", "lo")]
    #: The profiles a kernel queue can carry (`tools/faultproxy/profiles.json` marks them `netem`).
    profiles = ["delay-50ms", "jitter-30ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct", "corrupt-5pct"]
    #: How fast the arm is on its way between the poses, in radians a second: the fake moves it at once, so this is the speed a reading's lag is divided by to say how old it is, and a deliberately ordinary one.
    speed = 3.0
    #: The longest the driver may go without a new state before a read says so.
    stale_s = 0.1

    def __init__(self) -> None:
        self.fake: FakeControlBox | None = None
        self.bridge = FrankaBridge()
        self._origin = time.monotonic()

    def start(self) -> None:
        self.fake = FakeControlBox(hz=500.0)
        self.port = self.fake.start()
        self.bridge = FrankaBridge()

    def stop(self) -> None:
        if self.fake is not None:
            self.fake.stop()
            self.fake = None

    def config(self, addresses) -> str:
        return json.dumps({"host": "127.0.0.1", "port": self.port, "simulator": True})

    def open(self, config: str) -> int:
        return self.bridge.open(config)

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad), state.seq

    def target(self, index: int):
        # Held for HOLD_S at a time whatever the caller's own cadence is.
        return POSES[int((time.monotonic() - self._origin) / HOLD_S) & 1]

    def write(self, values) -> int:
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(values), mask=[1] * len(values))
        return self.bridge.write_command(command, 0)

    def stop_call(self) -> int:
        return self.bridge.emergency_stop()

    def release(self) -> None:
        self.bridge.release_stop()

    def close(self) -> None:
        self.bridge.close()

    def truth(self):
        return list(self.fake.q)

    def commands_taken(self) -> int:
        """How many commands the fake Control box has taken off the UDP stream."""
        return self.fake.commands_seen


def subject() -> Subject:
    return Subject()
