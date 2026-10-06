"""A simulated Piper on a kernel CAN bus, in a process of its own.

    python -m skynet_piper.serve <interface> [--no-gripper] [--status <arm status>] [--error <word>] [--driver <joint> <flags>]

`selftest.py --vcan`'s way in: one socket for the arm, "ready" on stdout once it is bound, and serve until stdin closes.
`--status`, `--error` and `--driver` stage a standing fault before the first frame — the arm the bridge refuses to come up
over.
"""

from __future__ import annotations

import select
import sys
import time

from skynet_canopen.bus import SocketCanBus

from .arm import SimulatedPiper


def main(argv: list[str]) -> int:
    if not argv:
        print("usage: python -m skynet_piper.serve <interface> [--no-gripper] [--status <n>] [--error <word>] [--driver <joint> <flags>]",
              file=sys.stderr)
        return 2
    interface = argv[0]
    gripper = True
    status = 0
    error = 0
    drivers: dict[int, int] = {}
    rest = argv[1:]
    while rest:
        if rest[0] == "--no-gripper":
            gripper = False
            rest = rest[1:]
        elif rest[0] == "--status" and len(rest) >= 2:
            status = int(rest[1], 0)
            rest = rest[2:]
        elif rest[0] == "--error" and len(rest) >= 2:
            error = int(rest[1], 0)
            rest = rest[2:]
        elif rest[0] == "--driver" and len(rest) >= 3:
            drivers[int(rest[1])] = int(rest[2], 0)
            rest = rest[3:]
        else:
            print(f"serve: cannot read {' '.join(rest)!r}", file=sys.stderr)
            return 2
    arm = SimulatedPiper(SocketCanBus(interface), gripper=gripper)
    arm.arm_status = status
    arm.err_code = error
    for joint, flags in drivers.items():
        arm.driver_flags[joint - 1] = flags
    print("ready", flush=True)
    stdin = sys.stdin
    while True:
        # stdin readable means the parent closed it (EOF reads empty) — the only exit.
        ready, _, _ = select.select([stdin], [], [], 0)
        if ready and stdin.read(1) == "":
            return 0
        arm.poll()
        time.sleep(0.005)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
