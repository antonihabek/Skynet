"""Simulated ODrive axes on a kernel CAN bus, in a process of their own.

    python -m skynet_odrive.serve <interface> <node>... [--error <node> <code>]

`selftest.py --vcan`'s way in: one socket per axis (two axes sharing one would eat each
other's requests), "ready" on stdout once every axis is bound, and serve until stdin closes.
The `--error` pair stages a latched axis error before the first frame — the body the bridge
refuses to come up over.
"""

from __future__ import annotations

import select
import sys
import time

from skynet_canopen.bus import SocketCanBus

from .axis import SimulatedAxis


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print("usage: python -m skynet_odrive.serve <interface> <node>... [--error <node> <code>]",
              file=sys.stderr)
        return 2
    interface = argv[0]
    nodes: list[int] = []
    errors: dict[int, int] = {}
    rest = argv[1:]
    while rest:
        if rest[0] == "--error":
            if len(rest) < 3:
                print("serve: --error needs a node and a code", file=sys.stderr)
                return 2
            errors[int(rest[1])] = int(rest[2], 0)
            rest = rest[3:]
        else:
            nodes.append(int(rest[0]))
            rest = rest[1:]
    if not nodes:
        print("serve: no nodes to serve", file=sys.stderr)
        return 2
    axes = []
    for node in nodes:
        axis = SimulatedAxis(SocketCanBus(interface), node)
        if node in errors:
            axis.axis_error = errors[node]
        axes.append(axis)
    print("ready", flush=True)
    stdin = sys.stdin
    while True:
        # stdin readable means the parent closed it (EOF reads empty) — the only exit.
        ready, _, _ = select.select([stdin], [], [], 0)
        if ready and stdin.read(1) == "":
            return 0
        for axis in axes:
            axis.poll()
        time.sleep(0.01)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
