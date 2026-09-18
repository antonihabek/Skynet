"""EtherCAT on the wire: one Ethernet frame, one EtherCAT header, one or more datagrams.

The encoding is small and exact, which is why `DRY-COVERAGE-PLAN.md`'s Phase 2 table calls *"frame
encoding"* dry-verifiable for this row. What is **not** in this file, and cannot be, is everything
that makes EtherCAT what it is: a frame that passes through each slave in turn, being modified as it
goes, at a propagation delay the distributed clocks measure. That is a property of a ring of silicon,
and this module encodes and decodes the bytes that travel round it.

## The working counter is the whole protocol in one field

Every datagram ends with a 16-bit working counter, and a master sends it as zero. Each slave that
*successfully serviced* the datagram increments it — by 1 for a read or a write, by 3 for a
read-write it did both halves of — and the frame comes back with the sum. So a master does not ask
"did that work"; it asserts what the total should be and compares.

That is why a slave that is absent, mis-addressed or in the wrong state produces a **low** working
counter rather than an error: nothing refuses, the byte count is right, and the number at the end is
one less than it should be. A master that ignored it would read a datagram full of zeros as a
measurement.

## Addressing: three ways to name a slave, and they are not interchangeable

* **Auto-increment** (`APRD`/`APWR`) — the address is a negative position that each slave increments
  as the frame passes. Slave 0 sees 0, slave 1 sees 1, and a slave acts on the datagram only when it
  sees **zero**. It is how a master talks to a ring it has not configured yet, and it is positional:
  unplug one slave and every address after it means a different machine.
* **Configured station address** (`FPRD`/`FPWR`) — the 16-bit address the master wrote during
  configuration. Stable, and the only kind safe to use after start-up.
* **Logical** (`LRD`/`LWR`/`LRW`) — an address in a flat 4 GB space that each slave's FMMU maps into
  its own memory. This is what makes one datagram carry every drive's process data in one pass.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, field

#: EtherCAT's own EtherType. A frame with any other is not this protocol and is not decoded here.
ETHERTYPE = 0x88A4

#: EtherCAT frame type 1: datagrams. Type 4 is a mailbox over UDP, which this driver does not speak.
FRAME_TYPE_DATAGRAMS = 1

# ── Datagram commands ────────────────────────────────────────────────────────

NOP = 0x00
APRD = 0x01  #: auto-increment read
APWR = 0x02  #: auto-increment write
APRW = 0x03  #: auto-increment read-write
FPRD = 0x04  #: configured-address read
FPWR = 0x05  #: configured-address write
FPRW = 0x06  #: configured-address read-write
BRD = 0x07  #: broadcast read
BWR = 0x08  #: broadcast write
BRW = 0x09  #: broadcast read-write
LRD = 0x0A  #: logical read
LWR = 0x0B  #: logical write
LRW = 0x0C  #: logical read-write
ARMW = 0x0D  #: auto-increment read, multiple write
FRMW = 0x0E  #: configured read, multiple write

COMMAND_NAMES = {
    NOP: "NOP",
    APRD: "APRD",
    APWR: "APWR",
    APRW: "APRW",
    FPRD: "FPRD",
    FPWR: "FPWR",
    FPRW: "FPRW",
    BRD: "BRD",
    BWR: "BWR",
    BRW: "BRW",
    LRD: "LRD",
    LWR: "LWR",
    LRW: "LRW",
    ARMW: "ARMW",
    FRMW: "FRMW",
}

#: Which commands read, write, or both. The working counter's increment depends on it: 1 for a
#: read or a write a slave serviced, 3 for a read-write it serviced both halves of.
READ_COMMANDS = {APRD, FPRD, BRD, LRD, ARMW, FRMW}
WRITE_COMMANDS = {APWR, FPWR, BWR, LWR}
READ_WRITE_COMMANDS = {APRW, FPRW, BRW, LRW}

#: An Ethernet frame's payload floor. A frame shorter than this is padded on the wire, and a decoder
#: that trusted the Ethernet length rather than EtherCAT's own would read the padding as datagrams.
MIN_ETHERNET_PAYLOAD = 46

#: The largest datagram payload EtherCAT's 11-bit length field can carry.
MAX_DATAGRAM_DATA = 1486


class FrameError(Exception):
    """Bytes that are not an EtherCAT frame, or are one that could not have been sent."""


@dataclass
class Datagram:
    """One EtherCAT datagram: a command, an address, some data and a working counter."""

    command: int
    index: int
    #: For auto-increment: (position, register). For configured: (station, register). For logical:
    #: the 32-bit logical address, which this pair carries as (low16, high16).
    address: tuple[int, int]
    data: bytes
    #: Set on every datagram but the last in a frame. Decoded rather than assumed, because a frame
    #: whose last datagram says "more follows" is a truncation somebody has to notice.
    more: bool = False
    irq: int = 0
    working_counter: int = 0

    @property
    def logical_address(self) -> int:
        """The two address halves as one 32-bit logical address."""
        return (self.address[1] << 16) | self.address[0]

    def pack(self) -> bytes:
        if len(self.data) > MAX_DATAGRAM_DATA:
            raise FrameError(
                f"a datagram carries at most {MAX_DATAGRAM_DATA} bytes and this one has "
                f"{len(self.data)}; the length field is eleven bits."
            )
        length = len(self.data) & 0x7FF
        if self.more:
            length |= 1 << 15
        return (
            struct.pack(
                "<BBHHH",
                self.command,
                self.index,
                self.address[0],
                self.address[1],
                length,
            )
            + struct.pack("<H", self.irq)
            + self.data
            + struct.pack("<H", self.working_counter)
        )

    @staticmethod
    def unpack(raw: bytes, offset: int) -> tuple["Datagram", int]:
        """One datagram from `raw` at `offset`, and where the next one starts."""
        if len(raw) < offset + 10:
            raise FrameError(
                f"a datagram header is ten bytes and only {len(raw) - offset} remain."
            )
        command, index, address_low, address_high, length = struct.unpack_from("<BBHHH", raw, offset)
        irq = struct.unpack_from("<H", raw, offset + 8)[0]
        data_length = length & 0x7FF
        more = bool(length & (1 << 15))
        start = offset + 10
        end = start + data_length
        if len(raw) < end + 2:
            raise FrameError(
                f"a datagram declares {data_length} bytes of data and a working counter, and only "
                f"{len(raw) - start} remain. A frame that claims more than it carries is a "
                f"truncation, not a short read: this is one Ethernet frame."
            )
        working_counter = struct.unpack_from("<H", raw, end)[0]
        return (
            Datagram(
                command=command,
                index=index,
                address=(address_low, address_high),
                data=raw[start:end],
                more=more,
                irq=irq,
                working_counter=working_counter,
            ),
            end + 2,
        )


@dataclass
class Frame:
    """One EtherCAT frame: an Ethernet header, the EtherCAT header, and its datagrams."""

    datagrams: list[Datagram] = field(default_factory=list)
    destination: bytes = b"\xff\xff\xff\xff\xff\xff"
    source: bytes = b"\x02\x00\x00\x00\x00\x01"

    def pack(self) -> bytes:
        if not self.datagrams:
            raise FrameError("an EtherCAT frame with no datagram in it carries nothing.")
        body = b""
        for index, datagram in enumerate(self.datagrams):
            # `more` is set here rather than by the caller, because it is a property of the
            # datagram's *position* and a caller that set it by hand would eventually set it on the
            # last one — which reads to a slave as a frame that continues past its own end.
            datagram.more = index + 1 < len(self.datagrams)
            body += datagram.pack()
        if len(body) > 0x7FF:
            raise FrameError(
                f"the EtherCAT header's length field is eleven bits and these datagrams are "
                f"{len(body)} bytes."
            )
        header = struct.pack("<H", (len(body) & 0x7FF) | (FRAME_TYPE_DATAGRAMS << 12))
        payload = header + body
        if len(payload) < MIN_ETHERNET_PAYLOAD:
            payload += b"\x00" * (MIN_ETHERNET_PAYLOAD - len(payload))
        return self.destination + self.source + struct.pack(">H", ETHERTYPE) + payload

    @staticmethod
    def unpack(raw: bytes) -> "Frame":
        if len(raw) < 14 + 2:
            raise FrameError(f"an Ethernet frame is at least sixteen bytes here and this is {len(raw)}.")
        ethertype = struct.unpack_from(">H", raw, 12)[0]
        if ethertype != ETHERTYPE:
            raise FrameError(
                f"EtherType 0x{ethertype:04X} is not EtherCAT's 0x{ETHERTYPE:04X}. A master that "
                f"decoded this would be reading somebody else's traffic as process data."
            )
        header = struct.unpack_from("<H", raw, 14)[0]
        length = header & 0x7FF
        frame_type = (header >> 12) & 0xF
        if frame_type != FRAME_TYPE_DATAGRAMS:
            raise FrameError(
                f"EtherCAT frame type {frame_type} is not 1 (datagrams); this driver speaks no "
                f"other."
            )
        # EtherCAT's own length, not the Ethernet frame's: a short frame is padded to 46 bytes on
        # the wire and a decoder that trusted the outer length would read the padding as datagrams.
        end = 16 + length
        if len(raw) < end:
            raise FrameError(
                f"the EtherCAT header declares {length} bytes of datagrams and only "
                f"{len(raw) - 16} follow."
            )
        datagrams = []
        offset = 16
        while offset < end:
            datagram, offset = Datagram.unpack(raw, offset)
            datagrams.append(datagram)
            if not datagram.more:
                break
        return Frame(
            datagrams=datagrams,
            destination=raw[0:6],
            source=raw[6:12],
        )
