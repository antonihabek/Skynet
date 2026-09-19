"""The slave stack on a real interface, for a master this repository did not write.

    python -m skynet_ethercat.wire --interface ecat1 --drives 3

Every frame that arrives on the interface with EtherCAT's EtherType is passed round the same ring
`LoopbackSegment` passes frames round — :func:`segment.pass_round`, one implementation — and sent back
out of the interface it came in on, exactly as the last slave on a line hands a frame back to the
master.

## Why this exists

`drivers/ethercat`'s own suite runs this repository's master against this repository's slaves, and a
master and a slave written by one author agree with each other whether or not either agrees with
ETG.1000. `DRY-COVERAGE-PLAN.md` F48 named the one check a licence does not restrict: SOEM, an
independent master, run as a separate program against these slaves over an interface. SOEM is
GPLv3-or-commercial and this repository is Apache-2.0, so nothing here links it or includes it — it
talks to this process over a veth pair, which is interoperation rather than combination.

`soem.sh` is what drives it. This module is only the far end of the cable.

## What it is not

A software ring on a veth pair has no propagation delay, no line rate and no jitter a real segment
would recognise, and it answers at whatever speed Python answers. So nothing timed here is evidence
of anything — the row's *not dry-verifiable* column is unchanged by it — and the drives it serves
are the same simulated ones `LoopbackSegment` holds. A master cannot tell them from hardware, which
is what makes this a cross-check, and is also why a bridge pointed at this interface would describe
a real body: the descriptor's `simulator` flag comes from the segment's type, and over a NIC the
driver cannot know. The cross-check runs SOEM, not the bridge, for that reason.
"""

from __future__ import annotations

import argparse
import signal
import socket
import sys

from . import frames
from .segment import pass_round
from .slave import EtherCatDrive


class WireServer:
    """A ring of simulated drives answering on one interface."""

    def __init__(self, interface: str, drives: list[EtherCatDrive]) -> None:
        try:
            self._socket = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(frames.ETHERTYPE))
            self._socket.bind((interface, frames.ETHERTYPE))
        except (AttributeError, OSError) as error:
            raise SystemExit(
                f"skynet-ethercat wire: cannot serve on `{interface}`: {error}. This needs Linux and "
                "CAP_NET_RAW over the interface's network namespace — `soem.sh` gets both from "
                "`unshare -rn` without root."
            ) from error
        self.interface = interface
        self.drives = drives
        self.frames = 0
        self.refused = 0

    def serve(self) -> None:
        while True:
            try:
                raw = self._socket.recv(1518)
            except InterruptedError:
                continue
            except OSError:
                return
            try:
                frame = frames.Frame.unpack(raw)
            except frames.FrameError as error:
                # Not answered: a real ring would pass a frame it cannot parse and the master would
                # see it come back unmodified, which is the same thing as silence to a master that
                # counts. Said on stderr, so a malformed frame is a line somebody can read.
                self.refused += 1
                print(f"skynet-ethercat wire: dropped a frame — {error}", file=sys.stderr, flush=True)
                continue
            pass_round(self.drives, frame)
            self._socket.send(frame.pack())
            self.frames += 1

    def close(self) -> None:
        self._socket.close()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m skynet_ethercat.wire", description=__doc__.split("\n")[0])
    parser.add_argument("--interface", required=True, help="the interface the master is on the other end of")
    parser.add_argument("--drives", required=True, type=int, help="how many simulated drives are on the ring")
    # For a master's refusals across a real interface — `drivers/ethercat-rs`'s live test. Each
    # takes a position on the ring, and may be given more than once.
    parser.add_argument("--fault", type=int, action="append", default=[],
                        help="a drive that has latched a fault, by position")
    parser.add_argument("--no-units", type=int, action="append", default=[],
                        help="a drive that will not report its encoder resolution, by position")
    options = parser.parse_args(argv)
    if options.drives <= 0:
        parser.error("--drives is a count of drives on the ring, and a ring of none answers nothing")

    drives = [
        EtherCatDrive(
            position,
            name=f"drive{position}",
            report_units=position not in options.no_units,
        )
        for position in range(options.drives)
    ]
    for position in options.fault:
        drives[position].latch_fault(0x2310)
    server = WireServer(options.interface, drives)

    def stop(signum, _frame) -> None:
        server.close()

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    # The line `soem.sh` waits for. Printed after the socket is bound, so a master started on seeing
    # it is never started against an interface nothing is listening on.
    print(f"serving {options.drives} drive(s) on {options.interface}", flush=True)
    server.serve()
    print(f"served {server.frames} frame(s), dropped {server.refused}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
