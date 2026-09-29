"""A fixture child that predates `T`, speaking the protocol by hand.

It announces protocol 2 and answers every verb except `T` itself; `T` gets the bare `R EPROTO` an
old binding sends for an unknown verb. The host must read that shape as "this child predates
extension" and report `ENOSYS` — and the stream must stay in sync afterwards.
"""

from __future__ import annotations

import sys


def main() -> int:
    send = lambda line: (sys.stdout.write(line + "\n"), sys.stdout.flush())
    # `H <abi> <protocol> ...`: protocol 2 is what predates `T`; the ABI is the host's own, and this line said
    # `H 1 2` until the ABI went to 2, when the host began refusing it and `make check-extension` failed at
    # `open()` — unseen, because no CI step ran that target (F183, the same defect as the LeRobot bridge's).
    send("H 2 2 guest v2-fixture 0.1.0")
    for raw in sys.stdin:
        line = raw.rstrip("\n")
        parts = line.split()
        verb = parts[0] if parts else ""
        if verb == "X":
            send("R 0")
            return 0
        if verb == "E":
            send("R 0")
        elif verb == "O":
            length = int(parts[1]) if len(parts) > 1 else 0
            if length:
                sys.stdin.read(length)
            sys.stdin.readline()
            send("R 0")
        elif verb == "D":
            body = '{"schema_version":1,"robot_slug":"v2-fixture","dof":0,"joints":[]}'
            send(f"D {len(body)}")
            send(body)
        elif verb == "S":
            send("S 0 0 0 0 0 0")
        elif verb == "C":
            send("R -1")
        elif verb == "F":
            send("F -38 0 0 0 0 0 0")
        elif verb == "Y":
            send("Y")
        else:
            # `T` lands here on a binding from before it: one line in, one bare `R EPROTO` out,
            # nothing left unread. That is why the request is single-line — a payload on a second
            # line would sit in stdin and poison the next exchange.
            send("R -71")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
