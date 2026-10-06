"""Simulated moteus controllers on a kernel CAN-FD bus, in a process of their own.

    python -m skynet_moteus.serve <interface> [--ids 1 2 ...] [--limits <min> <max>] [--no-limits] [--fault <id> <code>] [--mode <id> <mode>]
                                   [--home <id> <state>] [--no-diagnostic]

`selftest.py --vcan`'s way in: one socket for the controllers, "ready" on stdout once it is bound, and serve until stdin closes. `--fault` and `--mode` stage a
controller in a fault or another master's mode before the first frame — the axis the bridge refuses to come up over. `--no-diagnostic` is a controller whose
diagnostic channel does not answer, which is what an operator's `limits` are for.
"""

from __future__ import annotations

import math
import select
import sys
import time

from . import protocol as p
from .bus import SocketCanFdBus
from .sim import SimulatedMoteus

_USAGE = ("usage: python -m skynet_moteus.serve <interface> [--ids 1 2 ...] [--limits <min> <max>] [--no-limits] [--fault <id> <code>] "
          "[--mode <id> <mode>] [--home <id> <state>] [--no-diagnostic]")


def main(argv: list[str]) -> int:
    if not argv:
        print(_USAGE, file=sys.stderr)
        return 2
    interface = argv[0]
    ids: list[int] = [1]
    limits: tuple[float, float] = (-0.25, 0.25)
    faults: dict[int, int] = {}
    modes: dict[int, int] = {}
    homes: dict[int, int] = {}
    diagnostic = True
    rest = argv[1:]
    while rest:
        if rest[0] == "--ids":
            ids = []
            rest = rest[1:]
            while rest and not rest[0].startswith("--"):
                ids.append(int(rest[0]))
                rest = rest[1:]
        elif rest[0] == "--limits" and len(rest) >= 3:
            limits = (float(rest[1]), float(rest[2]))
            rest = rest[3:]
        elif rest[0] == "--no-limits":
            limits = (math.nan, math.nan)
            rest = rest[1:]
        elif rest[0] == "--fault" and len(rest) >= 3:
            faults[int(rest[1])] = int(rest[2])
            rest = rest[3:]
        elif rest[0] == "--mode" and len(rest) >= 3:
            modes[int(rest[1])] = int(rest[2])
            rest = rest[3:]
        elif rest[0] == "--home" and len(rest) >= 3:
            homes[int(rest[1])] = int(rest[2])
            rest = rest[3:]
        elif rest[0] == "--no-diagnostic":
            diagnostic = False
            rest = rest[1:]
        else:
            print(f"serve: cannot read {' '.join(rest)!r}\n{_USAGE}", file=sys.stderr)
            return 2
    sim = SimulatedMoteus(SocketCanFdBus(interface), ids, position_min=limits[0], position_max=limits[1])
    for ident, code in faults.items():
        sim.controllers[ident].inject_fault(code)
    for ident, mode in modes.items():
        sim.controllers[ident].mode = mode
    for ident, state in homes.items():
        sim.controllers[ident].home_state = state
    if not diagnostic:
        for controller in sim.controllers.values():
            controller.refuse_diagnostic = True
    print("ready", flush=True)
    stdin = sys.stdin
    while True:
        # stdin readable means the parent closed it (EOF reads empty) — the only exit.
        ready, _, _ = select.select([stdin], [], [], 0)
        if ready and stdin.read(1) == "":
            return 0
        sim.poll()
        time.sleep(0.002)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
