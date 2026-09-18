"""The segment: where a frame goes after the master sends it.

EtherCAT is a ring. A frame leaves the master's port, passes through each slave's ESC in turn — each
one reading and writing the datagrams addressed to it *as the bits go past* — and comes back to the
master modified. That is the whole reason the protocol is fast and it is the part a software stack
cannot reproduce: the modification happens in silicon at line rate, and the delay between one slave
and the next is what the distributed clocks measure.

So there are two transports here and the difference between them is stated rather than absorbed,
exactly as `drivers/canopen` states the difference between `LoopbackBus` and `SocketCanBus`:

* [`LoopbackSegment`] hands the frame to each slave in order, in this process. The datagrams, the
  addressing, the working counter and every refusal are the real ones. **The wire is not**: no
  propagation delay, no jitter, no line break, no lost frame, and therefore no distributed clocks
  and no FSoE — which is exactly the *not dry-verifiable* column of this row's entry in
  `DRY-COVERAGE-PLAN.md`'s Phase 2 table, and is the basis of EtherCAT's Certified ceiling.
* [`RawSocketSegment`] puts the same frames on a real NIC through `AF_PACKET`. It needs
  `CAP_NET_RAW` and an interface with something on it, and it **refuses to construct** without one
  rather than pretending — because a segment that reported success with nothing attached would be a
  master measuring its own echo.
"""

from __future__ import annotations

import socket

from . import frames


class SegmentError(Exception):
    """A transport that could not be constructed, named rather than fallen back from."""


class LoopbackSegment:
    """Every slave in this process, in order, with the frame passing through each in turn.

    The ordering is the protocol's and is not an implementation detail: auto-increment addressing
    *is* the position in this list, so a slave inserted in the middle changes what every address
    after it means. A segment that handed the frame to slaves in any other order would make a master
    that got the addressing wrong look correct.
    """

    def __init__(self, slaves) -> None:
        self.slaves = list(slaves)
        #: Every frame that went round, for a suite that wants to assert on the bytes rather than
        #: on their effect.
        self.sent: list[bytes] = []

    def cycle(self, frame: frames.Frame) -> frames.Frame:
        raw = frame.pack()
        self.sent.append(raw)
        travelling = frames.Frame.unpack(raw)
        pass_round(self.slaves, travelling)
        return travelling

    def close(self) -> None:
        pass


class RawSocketSegment:
    """The same frames on a real NIC. Needs `CAP_NET_RAW` and a slave on the other end.

    Written, refusing to construct where there is nothing to construct against, and **not** exercised
    by this driver's suite — there is no EtherCAT hardware here. That is the arrangement
    `drivers/canopen`'s `SocketCanBus` is in, and the reason is the same: the code that speaks to the
    real thing should exist and should say plainly that it has not run, rather than being written on
    the day somebody first has hardware.
    """

    def __init__(self, interface: str, timeout_s: float = 0.05) -> None:
        try:
            self.socket = socket.socket(
                socket.AF_PACKET, socket.SOCK_RAW, socket.htons(frames.ETHERTYPE)
            )
        except (AttributeError, OSError) as error:
            raise SegmentError(
                f"a raw EtherCAT socket on `{interface}` could not be opened: {error}. This needs "
                f"Linux and CAP_NET_RAW. There is no fallback here on purpose: a master that "
                f"silently ran against a loopback segment would report a ring that is not there."
            ) from error
        try:
            self.socket.bind((interface, frames.ETHERTYPE))
            self.socket.settimeout(timeout_s)
        except OSError as error:
            self.socket.close()
            raise SegmentError(f"`{interface}` could not be bound: {error}") from error
        self.interface = interface

    def cycle(self, frame: frames.Frame) -> frames.Frame:
        self.socket.send(frame.pack())
        while True:
            raw = self.socket.recv(1518)
            # A raw socket sees this master's own outgoing frame as well as the ring's answer. The
            # two are told apart by the index the master put in the first datagram — which is the
            # only thing that distinguishes them, because the answer is the same frame modified.
            try:
                answer = frames.Frame.unpack(raw)
            except frames.FrameError:
                continue
            if answer.datagrams and frame.datagrams:
                if answer.datagrams[0].index != frame.datagrams[0].index:
                    continue
                if raw == frame.pack():
                    continue  # this master's own echo, unmodified
            return answer

    def close(self) -> None:
        self.socket.close()


def pass_round(slaves, frame: frames.Frame) -> None:
    """The frame's trip round the ring: through every slave in order, and back to the master.

    One function for every transport that has slaves in this process — the loopback segment above,
    and `wire.py`, which serves the same ring on a real interface to a master that is not this one.
    The segment is also what knows the topology, so each slave is told here whether one follows it;
    DL status reports that, and a master reads the segment's shape out of it.
    """
    for position, slave in enumerate(slaves):
        slave.has_downstream = position + 1 < len(slaves)
        _pass_through(slave, frame)


def _pass_through(slave, frame: frames.Frame) -> None:
    """One slave's turn with the frame: read, write, and move the working counter.

    This is the ESC's job and it is arithmetic over addresses. Every branch below is a refusal as
    much as an action: a datagram this slave is not addressed by passes through **untouched**, which
    is what makes a low working counter the master's evidence that something is not there. What an
    addressed datagram *reaches* is the slave's own business — `read_physical` and `write_physical`
    are its memory map, and they may deny an access (an empty mailbox, a full one) exactly as an ESC
    does, in which case the counter does not move either.
    """
    for datagram in frame.datagrams:
        command = datagram.command
        if command == frames.NOP:
            continue

        if command in (frames.APRD, frames.APWR, frames.APRW, frames.ARMW):
            position, register = datagram.address
            # Auto-increment: each slave increments the position as the frame goes past, and acts
            # only when it sees zero. Incrementing *after* the test is the whole mechanism.
            addressed = position == 0
            # The 16-bit address is signed on the wire in the sense that it wraps; a master starts
            # at 0 for the first slave and 0x10000 - n for the nth.
            datagram.address = ((position + 1) & 0xFFFF, register)
            if not addressed:
                continue
            _service(slave, datagram, register, command)
            continue

        if command in (frames.FPRD, frames.FPWR, frames.FPRW, frames.FRMW):
            station, register = datagram.address
            if station != slave.station_address or slave.station_address == 0:
                continue
            _service(slave, datagram, register, command)
            continue

        if command in (frames.BRD, frames.BWR, frames.BRW):
            _, register = datagram.address
            _service(slave, datagram, register, command, broadcast=True)
            continue

        if command in (frames.LRD, frames.LWR, frames.LRW):
            _service_logical(slave, datagram, command)
            continue


def _service(
    slave, datagram: frames.Datagram, address: int, command: int, *, broadcast: bool = False
) -> None:
    """A physical access. Read +1, write +1, and a read-write +1 for the read and +2 for the write.

    A broadcast read ORs what this slave holds into what the datagram already carries (ETG.1000.4),
    rather than replacing it — which is what lets a master read every slave's AL status in one
    datagram and learn whether they all agree. The first version replaced it, so a broadcast read
    reported the last slave's state as everybody's.
    """
    length = len(datagram.data)
    if command in frames.READ_COMMANDS:
        read = slave.read_physical(address, length)
        if read is not None:
            datagram.data = _or(datagram.data, read) if broadcast else read
            datagram.working_counter += 1
    elif command in frames.WRITE_COMMANDS:
        if slave.write_physical(address, datagram.data):
            datagram.working_counter += 1
    elif command in frames.READ_WRITE_COMMANDS:
        written = datagram.data
        read = slave.read_physical(address, length)
        wrote = slave.write_physical(address, written)
        # Three for both halves, not two: a master that expected two would see every healthy
        # read-write as a partial failure.
        if read is not None:
            datagram.data = _or(datagram.data, read) if broadcast else read
            datagram.working_counter += 1
        if wrote:
            datagram.working_counter += 2


def _or(carried: bytes, held: bytes) -> bytes:
    return bytes(a | b for a, b in zip(carried, held))


def _service_logical(slave, datagram: frames.Datagram, command: int) -> None:
    """A logical datagram, mapped through this slave's FMMUs onto its **physical** memory.

    One `LRW` carries every drive's process data in one pass, and each slave picks out the windows
    its FMMUs map. Each window is a physical access at the address the FMMU names — a process-data
    sync manager, or any register at all — so an FMMU that points at the input mailbox's status byte
    reads that byte.

    The first version ignored the physical address and handed a read FMMU the drive's inputs
    whatever it pointed at. This repository's master never noticed, because it only maps process
    data. SOEM maps a third FMMU onto every slave's mailbox status register, so it would have read
    the statusword's low byte as that register — and bit 3 of it, *mailbox full*, is CiA 402's fault
    bit. A faulted drive would have read to SOEM as a drive with a message waiting, on every cycle.

    Per slave and per datagram: +1 if any window was read, +2 if any was written — so a slave with
    outputs and inputs adds 3 to an `LRW`, and one with only inputs adds 1 (ETG.1000.4). The first
    version added 1 for a write-only slave, which a master counting ETG's way reads as a missing one.
    """
    base = datagram.logical_address
    payload = bytearray(datagram.data)
    read_any = False
    wrote_any = False

    for fmmu in slave.fmmus:
        if not fmmu.enabled or fmmu.length == 0:
            continue
        start = fmmu.logical_start
        end = start + fmmu.length
        low = max(start, base)
        high = min(end, base + len(payload))
        if low >= high:
            continue
        physical = fmmu.physical_start + (low - start)
        window = slice(low - base, high - base)

        if fmmu.type == fmmu.READ and command in (frames.LRD, frames.LRW):
            read = slave.read_physical(physical, high - low)
            if read is not None:
                payload[window] = read
                read_any = True
        elif fmmu.type == fmmu.WRITE and command in (frames.LWR, frames.LRW):
            if slave.write_physical(physical, bytes(payload[window])):
                wrote_any = True

    datagram.data = bytes(payload)
    if command == frames.LRW:
        datagram.working_counter += (1 if read_any else 0) + (2 if wrote_any else 0)
    elif read_any or wrote_any:
        datagram.working_counter += 1
