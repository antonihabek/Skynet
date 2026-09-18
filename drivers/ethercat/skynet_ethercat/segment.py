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
import struct

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
        for slave in self.slaves:
            _pass_through(slave, travelling)
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


def _pass_through(slave, frame: frames.Frame) -> None:
    """One slave's turn with the frame: read, write, and move the working counter.

    This is the ESC's job and it is arithmetic over addresses. Every branch below is a refusal as
    much as an action: a datagram this slave is not addressed by passes through **untouched**, which
    is what makes a low working counter the master's evidence that something is not there.
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
            _service(slave, datagram, register, command)
            continue

        if command in (frames.LRD, frames.LWR, frames.LRW):
            _service_logical(slave, datagram, command)
            continue


def _service(slave, datagram: frames.Datagram, register: int, command: int) -> None:
    length = len(datagram.data)
    if command in frames.READ_COMMANDS:
        datagram.data = _read(slave, register, length)
        datagram.working_counter += 1
    elif command in frames.WRITE_COMMANDS:
        _write(slave, register, datagram.data)
        datagram.working_counter += 1
    elif command in frames.READ_WRITE_COMMANDS:
        written = datagram.data
        datagram.data = _read(slave, register, length)
        _write(slave, register, written)
        # Three, not two. A read-write a slave serviced both halves of increments the counter by
        # three, and a master that expected two would see every cycle as a partial failure.
        datagram.working_counter += 3


def _read(slave, register: int, length: int) -> bytes:
    """A register read, or a mailbox read when the address is inside the reading mailbox.

    The two are the same datagram at the ESC's level and different things above it, which is why
    the address decides rather than a flag on the command: an ESC's memory map *is* where the
    mailbox lives, and a master addresses it the way it addresses any other register.
    """
    manager = slave.sync_managers[1]
    if manager.enabled and manager.start <= register < manager.start + max(manager.length, 1):
        message = slave.read_mailbox()
        if not message:
            # Nothing waiting. A real ESC answers the memory as it stands, which is zeros, and the
            # master is meant to have polled the sync manager's status bit first — so this returns
            # zeros rather than an empty message, and the master's own decoder refuses them.
            return bytes(length)
        out = bytearray(length)
        out[: min(length, len(message))] = message[:length]
        return bytes(out)
    return slave.read_register(register, length)


def _write(slave, register: int, data: bytes) -> None:
    """A register write, or a mailbox write when the address is inside a mailbox sync manager."""
    for index in (0, 1):
        manager = slave.sync_managers[index]
        if manager.enabled and manager.start <= register < manager.start + max(manager.length, 1):
            if index == 0:
                slave.write_mailbox(data)
            return
    slave.write_register(register, data)


def _service_logical(slave, datagram: frames.Datagram, command: int) -> None:
    """A logical datagram, mapped through this slave's FMMUs.

    One `LRW` carries every drive's process data in one pass, and each slave picks out the window
    its FMMU maps. A master that got a logical address wrong reads another drive's statusword as its
    own — which is the defect this addressing exists to make possible and the working counter exists
    to catch.
    """
    base = datagram.logical_address
    payload = bytearray(datagram.data)
    serviced_read = False
    serviced_write = False

    for fmmu in slave.fmmus:
        if not fmmu.enabled or fmmu.length == 0:
            continue
        start = fmmu.logical_start
        end = start + fmmu.length
        if end <= base or start >= base + len(payload):
            continue
        window_start = max(start, base) - base
        window_end = min(end, base + len(payload)) - base
        span = window_end - window_start

        if fmmu.type == 1 and command in (frames.LRD, frames.LRW):
            slave.refresh_inputs()
            payload[window_start:window_end] = slave.inputs[:span]
            serviced_read = True
        elif fmmu.type == 2 and command in (frames.LWR, frames.LRW):
            slave.outputs[:span] = payload[window_start:window_end]
            slave.apply_outputs()
            serviced_write = True

    datagram.data = bytes(payload)
    if command == frames.LRW and serviced_read and serviced_write:
        datagram.working_counter += 3
    elif serviced_read or serviced_write:
        datagram.working_counter += 1
