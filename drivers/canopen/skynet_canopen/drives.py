"""Simulated CiA 402 drives on a real SocketCAN interface, in a process of their own.

This driver's own suite runs its drives on an in-process loopback bus, which is what lets it run
anywhere — and is also why, until DRY-COVERAGE-PLAN.md F71, it had never met a bus where the other
side answers *later*. This puts the same :class:`SimulatedDrive`s on a kernel CAN interface —
`vcan0`, which an unprivileged user namespace can create (`unshare -rn`) — so a master in another
process, in any language, talks to them through the kernel. It is what `make check-vcan` runs the
bridge against, and what `drivers/canopen-rs` is measured on.

    python3 -m skynet_canopen.drives <interface> <node>... [--fault <node>] [--no-units <node>]
                                     [--lose-tpdo2 <node>]

`--lose-tpdo2` keeps a drive answering SDO and never lets its TPDO2 onto the bus: an axis that
misses every SYNC, which is how a master's degraded read is exercised across processes.

Prints `ready` on stdout once every drive has its socket, then serves until stdin closes — so a
test that spawns it and dies takes the drives with it rather than leaving a bus full of orphans.
"""

from __future__ import annotations

import os
import select
import sys
import threading

from .bus import Bus, SocketCanBus
from .frames import Frame, tpdo2_id
from .slave import SimulatedDrive


class _LosingTpdo2(Bus):
    """A drive's bus with its TPDO2 lost on the way out — `selftest.DroppingEndpoint`, sending side."""

    def __init__(self, inner: SocketCanBus, node: int) -> None:
        self._inner = inner
        self._lost = tpdo2_id(node)

    def send(self, frame: Frame) -> None:
        if frame.arbitration_id != self._lost:
            self._inner.send(frame)

    def recv(self, timeout_s: float = 0.0) -> Frame | None:
        return self._inner.recv(timeout_s)


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    interface = argv[0]
    nodes: list[int] = []
    faulted: set[int] = set()
    unitless: set[int] = set()
    silent: set[int] = set()
    rest = iter(argv[1:])
    for word in rest:
        if word == "--fault":
            faulted.add(int(next(rest)))
        elif word == "--no-units":
            unitless.add(int(next(rest)))
        elif word == "--lose-tpdo2":
            silent.add(int(next(rest)))
        else:
            nodes.append(int(word))

    drives = []
    for node in nodes:
        bus = SocketCanBus(interface)
        drive_bus = _LosingTpdo2(bus, node) if node in silent else bus
        drive = SimulatedDrive(drive_bus, node, report_units=node not in unitless)
        if node in faulted:
            drive.fault()
        drives.append((bus, drive))

    stop = threading.Event()

    def watch_stdin() -> None:
        # EOF on stdin is the parent going away, which is the only shutdown signal this needs.
        sys.stdin.read()
        stop.set()

    threading.Thread(target=watch_stdin, daemon=True).start()
    print("ready", flush=True)
    sockets = {bus._socket.fileno(): drive for bus, drive in drives}
    while not stop.is_set():
        readable, _, _ = select.select(list(sockets), [], [], 0.05)
        for fd in readable:
            sockets[fd].poll()
    os._exit(0)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
