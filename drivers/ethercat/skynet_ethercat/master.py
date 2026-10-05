"""The master: scan the segment, configure it, walk the AL states, and run the cycle.

Everything here is `DRY-COVERAGE-PLAN.md`'s *"CiA 402 state machine, frame encoding"* — the half of
this row that is dry-verifiable — and nothing here is the half that is not. There is no distributed
clock in this file and no FSoE, because both are properties of a ring of silicon and neither can be
had from a segment that is a list.

## Why a master asserts a working counter instead of checking for errors

Nothing in EtherCAT refuses. A datagram addressed to a slave that is absent, mis-addressed or in the
wrong state comes back the right length, full of whatever the master sent, with a working counter one
lower than it should be. So **every exchange here says what the counter must be** and refuses when it
is not — which is the only way to tell a reading from a slave that answered from a reading of the
master's own zeros.

## The bring-up order is the protocol's, not a preference

Init → Pre-Op → Safe-Op → Op, and each step needs the one before it. Station addresses are written
in Init; the mailbox sync managers are configured before Pre-Op, because CoE is what Pre-Op is *for*;
the process-data managers and the FMMUs before Safe-Op. A master that asked for Op straight from Init
is refused with 0x0011 and the slave stays in Init, which is ETG.1000.6's state machine and what
`slave.py` models.

## A mailbox is read and written whole

ETG.1000.4's rule for a mailbox sync manager: a write is accepted when its *last* byte lands, and a
read empties the buffer when its last byte is read. The first version of this master wrote sixteen
bytes into a 128-byte mailbox and read sixteen back — which worked against the first version of
`slave.py`, written by the same hand, and would have been ignored by every real ESC: the request
never complete, the reply never released. SOEM, run against the slave stack in the fourteenth
session, writes and reads the whole buffer and polls the input mailbox's *full* bit in between, and
so does this now.
"""

from __future__ import annotations

import struct

from skynet_canopen import cia402

from . import frames
from .segment import SegmentError
from .slave import (
    AL_ERROR_BIT,
    AL_STATUS_NAMES,
    SDO_DATA_AT,
    SDO_INDEX_AT,
    SDO_MESSAGE_BYTES,
    SDO_SPEC_AT,
    AL_INIT,
    AL_NAMES,
    AL_OP,
    AL_PREOP,
    AL_SAFEOP,
    MBX_TYPE_COE,
    REG_AL_CONTROL,
    REG_AL_STATUS,
    REG_AL_STATUS_CODE,
    REG_FMMU_BASE,
    REG_FMMU_STRIDE,
    REG_SM_BASE,
    REG_SM_STRIDE,
    REG_STATION_ADDRESS,
    REG_TYPE,
)

#: Where this master puts each slave's mailbox and process data in ESC memory. Any addresses would
#: do; these are written down once and used by both halves so the two cannot disagree.
MBX_OUT_START = 0x1000
MBX_IN_START = 0x1400
MBX_LENGTH = 128
PDO_OUT_START = 0x1800
PDO_IN_START = 0x1880

#: Six bytes each way: controlword + target position out, statusword + actual position in. The same
#: pair `drivers/canopen` maps into RPDO2/TPDO2, which is not a coincidence — it is the CiA 402
#: cyclic-synchronous-position minimum, and both transports carry the same profile.
PDO_OUT_BYTES = 6
PDO_IN_BYTES = 6

#: How many times the input mailbox's status is read before a drive that has not answered is given
#: up on. The drive here answers before the next frame; a real one answers within its firmware's
#: cycle, and a bound is what turns a drive that never answers into a refusal rather than a hang.
MAILBOX_POLLS = 100

#: The first station address this master assigns. 0 means "unconfigured" to an ESC, so the first
#: slave has to be 1 — a master that started at zero would configure a slave into invisibility.
FIRST_STATION = 0x1001


class MasterError(Exception):
    """The segment did not answer the way the protocol says it must."""


class EtherCatMaster:
    """One segment, brought up and then cycled."""

    def __init__(self, segment) -> None:
        self.segment = segment
        self.index = 0
        self.slave_count = 0
        #: Logical address of each slave's output and input window, assigned at configuration.
        self.output_at: list[int] = []
        self.input_at: list[int] = []
        self.logical_length = 0

    # ── one exchange ─────────────────────────────────────────────────────────

    def _next_index(self) -> int:
        self.index = (self.index + 1) & 0xFF
        return self.index

    def exchange(self, datagram: frames.Datagram, expect: int) -> frames.Datagram:
        """Send one datagram and refuse unless the working counter is exactly `expect`."""
        try:
            answer = self.segment.cycle(frames.Frame([datagram]))
        except (SegmentError, OSError) as error:
            # A raw socket's receive timing out is `TimeoutError`, an OSError and not a MasterError, so it used to escape every entry point that cycled the ring: the link-chaos contract (12.C.4) saw
            # `read_state` and `emergency_stop` raise it out of the driver on a stall, a blackhole and every lossy profile.
            raise MasterError(
                f"no answer came back from the segment ({error}). Nothing is known of what became of the frame: the ring may be unplugged, powered down or stalled, and the next cycle's working counter is what says so."
            ) from error
        if not answer.datagrams:
            raise MasterError("the segment returned a frame with no datagram in it.")
        out = answer.datagrams[0]
        if out.working_counter != expect:
            raise MasterError(
                f"{frames.COMMAND_NAMES.get(datagram.command, datagram.command)} came back with a "
                f"working counter of {out.working_counter} and {expect} was required. Nothing in "
                f"EtherCAT refuses: a datagram no slave serviced returns the right length full of "
                f"what was sent, and this number is the only thing that says so."
            )
        return out

    # ── bring-up ─────────────────────────────────────────────────────────────

    def scan(self) -> int:
        """How many slaves are on the segment, counted by a broadcast read's working counter."""
        answer = self.segment.cycle(
            frames.Frame([frames.Datagram(frames.BRD, self._next_index(), (0, REG_TYPE), b"\x00")])
        )
        self.slave_count = answer.datagrams[0].working_counter
        return self.slave_count

    def configure(self, expected: int) -> None:
        """Station addresses, sync managers, FMMUs — everything Init is for.

        `expected` is the number of drives the caller knows about. A segment with a different number
        is refused rather than configured: auto-increment addressing is positional, so a master that
        found four slaves where it expected three has just written three station addresses onto
        three of somebody's four machines.
        """
        found = self.scan()
        if found != expected:
            raise MasterError(
                f"the segment has {found} slave(s) and this configuration describes {expected}. "
                f"Auto-increment addressing is positional, so configuring anyway would write each "
                f"station address onto whichever machine happens to be at that position."
            )

        # Init first, on everything, so nothing is in a state that would refuse configuration.
        self.exchange(
            frames.Datagram(frames.BWR, self._next_index(), (0, REG_AL_CONTROL),
                            struct.pack("<H", AL_INIT)),
            expected,
        )

        for position in range(expected):
            station = FIRST_STATION + position
            auto = (0x10000 - position) & 0xFFFF if position else 0
            self.exchange(
                frames.Datagram(frames.APWR, self._next_index(), (auto, REG_STATION_ADDRESS),
                                struct.pack("<H", station)),
                1,
            )

        for position in range(expected):
            station = FIRST_STATION + position
            # SM0/SM1: the CoE mailbox, out and in. Configured before Pre-Op because Pre-Op is the
            # state in which a mailbox is what a master has.
            self._write_sync_manager(station, 0, MBX_OUT_START, MBX_LENGTH, control=0x26)
            self._write_sync_manager(station, 1, MBX_IN_START, MBX_LENGTH, control=0x22)
            # SM2/SM3: process data, out and in.
            self._write_sync_manager(station, 2, PDO_OUT_START, PDO_OUT_BYTES, control=0x64)
            self._write_sync_manager(station, 3, PDO_IN_START, PDO_IN_BYTES, control=0x20)

        # Logical addresses: one flat space, each slave's outputs then inputs, in order. The map is
        # kept here because the master is the only party that knows the whole of it.
        self.output_at = []
        self.input_at = []
        cursor = 0
        for position in range(expected):
            self.output_at.append(cursor)
            cursor += PDO_OUT_BYTES
        for position in range(expected):
            self.input_at.append(cursor)
            cursor += PDO_IN_BYTES
        self.logical_length = cursor

        for position in range(expected):
            station = FIRST_STATION + position
            self._write_fmmu(station, 0, self.output_at[position], PDO_OUT_BYTES, PDO_OUT_START, 2)
            self._write_fmmu(station, 1, self.input_at[position], PDO_IN_BYTES, PDO_IN_START, 1)

    def _write_sync_manager(
        self, station: int, index: int, start: int, length: int, control: int
    ) -> None:
        body = struct.pack("<HHBBBB", start, length, control, 0, 0x01, 0)
        self.exchange(
            frames.Datagram(
                frames.FPWR, self._next_index(), (station, REG_SM_BASE + REG_SM_STRIDE * index), body
            ),
            1,
        )

    def _write_fmmu(
        self, station: int, index: int, logical: int, length: int, physical: int, kind: int
    ) -> None:
        # The ESC's FMMU entry is sixteen bytes and every one of them is in ETG.1000.4's table:
        # logical start (u32), length (u16), logical start and end bit, physical start (u16),
        # physical start bit, type, activate, and three reserved. Written out in one `pack` so the
        # layout is in one place — the first version of this had `type` and `activate` one byte
        # early, which configures an FMMU that maps nothing and presents as a working counter of
        # zero on every cycle with no slave complaining about anything.
        body = struct.pack(
            "<IHBBHBBB3x",
            logical,
            length,
            0,  # logical start bit
            7,  # logical end bit
            physical,
            0,  # physical start bit
            kind,  # 1 = inputs (slave → master), 2 = outputs (master → slave)
            0x01,  # activate
        )
        self.exchange(
            frames.Datagram(
                frames.FPWR,
                self._next_index(),
                (station, REG_FMMU_BASE + REG_FMMU_STRIDE * index),
                body,
            ),
            1,
        )

    def request_state(self, state: int, expected: int) -> None:
        """Ask every slave for a state, then read back what each one actually holds.

        The read-back is not belt and braces. `AL Control` is a request and `AL Status` is the
        answer, and a slave that refuses sets the error bit and a code rather than failing the
        write — so a master that only wrote would believe every transition it asked for.
        """
        self.exchange(
            frames.Datagram(
                frames.BWR, self._next_index(), (0, REG_AL_CONTROL), struct.pack("<H", state)
            ),
            expected,
        )
        for position in range(expected):
            station = FIRST_STATION + position
            answer = self.exchange(
                frames.Datagram(frames.FPRD, self._next_index(), (station, REG_AL_STATUS), b"\x00\x00"),
                1,
            )
            held = answer.data[0]
            if held & AL_ERROR_BIT:
                code = self.exchange(
                    frames.Datagram(
                        frames.FPRD, self._next_index(), (station, REG_AL_STATUS_CODE), b"\x00\x00"
                    ),
                    1,
                )
                number = struct.unpack("<H", code.data)[0]
                raise MasterError(
                    f"slave {position} refused {AL_NAMES.get(state, state)} and is in "
                    f"{AL_NAMES.get(held & 0x0F, held & 0x0F)}, AL status code 0x{number:04X} "
                    f"({AL_STATUS_NAMES.get(number, 'a code ETG.1000.6 names and this master does not')})."
                )
            if (held & 0x0F) != state:
                raise MasterError(
                    f"slave {position} was asked for {AL_NAMES.get(state, state)} and reports "
                    f"{AL_NAMES.get(held & 0x0F, held & 0x0F)}."
                )

    def bring_up(self, expected: int) -> None:
        """Init → Pre-Op → configure → Safe-Op → Op, in the order the protocol requires."""
        self.configure(expected)
        self.request_state(AL_PREOP, expected)
        self.request_state(AL_SAFEOP, expected)
        self.request_state(AL_OP, expected)

    # ── CoE ──────────────────────────────────────────────────────────────────

    def sdo_upload(self, position: int, index: int, subindex: int, size: int, signed: bool) -> int:
        """Read one object over the mailbox, expedited."""
        request = _coe_request(0x40, index, subindex, b"\x00\x00\x00\x00")
        answer = self._mailbox_exchange(position, request)
        return _coe_value(answer, index, subindex, size, signed)

    def sdo_download(self, position: int, index: int, subindex: int, value: int, size: int) -> None:
        spec = 0x23 | ((4 - size) << 2)
        payload = int(value & 0xFFFFFFFF).to_bytes(4, "little")
        answer = self._mailbox_exchange(position, _coe_request(spec, index, subindex, payload))
        _coe_check_download(answer, index, subindex)

    def _mailbox_exchange(self, position: int, request: bytes) -> bytes:
        """A request into the output mailbox, and the drive's answer out of the input one.

        Three steps, each asserting its working counter: the whole output buffer written, the input
        mailbox's status polled until its *full* bit is set, and the whole input buffer read. A master
        that read before the bit was set would be denied — the counter would not move — and one that
        read part of the buffer would leave it full, so the drive's next answer would have nowhere to
        go and the one after would be read as it.
        """
        station = FIRST_STATION + position
        self.exchange(
            frames.Datagram(
                frames.FPWR,
                self._next_index(),
                (station, MBX_OUT_START),
                request.ljust(MBX_LENGTH, b"\x00"),
            ),
            1,
        )
        status_register = REG_SM_BASE + REG_SM_STRIDE * 1 + 5
        for _ in range(MAILBOX_POLLS):
            status = self.exchange(
                frames.Datagram(frames.FPRD, self._next_index(), (station, status_register), b"\x00"),
                1,
            )
            if status.data[0] & 0x08:
                break
        else:
            raise MasterError(
                f"slave {position} did not answer its mailbox within {MAILBOX_POLLS} polls of the "
                f"input mailbox's status. Reading the buffer anyway would be denied, and a master "
                f"that treated the denial as an empty answer would report a value nobody sent."
            )
        answer = self.exchange(
            frames.Datagram(
                frames.FPRD, self._next_index(), (station, MBX_IN_START), b"\x00" * MBX_LENGTH
            ),
            1,
        )
        return answer.data

    # ── the cycle ────────────────────────────────────────────────────────────

    def send_cycle(self, outputs: bytes) -> None:
        """The `LRW` of `cycle`, written and not waited for: a stop is this, so that it returns with the frame on the wire whatever the ring is doing. What became of it is for the next `cycle` to say, with the working
        counter, and the controlwords in `outputs` are the ones every later cycle carries until a command replaces them."""
        image = bytearray(self.logical_length)
        image[: len(outputs)] = outputs
        datagram = frames.Datagram(frames.LRW, self._next_index(), (0, 0), bytes(image))
        try:
            self.segment.send(frames.Frame([datagram]))
        except (SegmentError, OSError) as error:
            raise MasterError(f"the frame could not be written to the segment ({error}).") from error

    def cycle(self, outputs: bytes) -> bytes:
        """One `LRW`: every slave's outputs written and every slave's inputs read, in one pass.

        This is what EtherCAT is *for*, and the working counter it must come back with is three per
        slave — one datagram serviced on both halves by each of them. A master that expected one
        per slave would read a healthy segment as a failing one, and a master that expected nothing
        would read a dead one as healthy.
        """
        image = bytearray(self.logical_length)
        image[: len(outputs)] = outputs
        answer = self.exchange(
            frames.Datagram(frames.LRW, self._next_index(), (0, 0), bytes(image)),
            3 * self.slave_count,
        )
        return answer.data


def _coe_request(spec: int, index: int, subindex: int, payload: bytes) -> bytes:
    body = struct.pack("<H", 0x02 << 12) + struct.pack("<BHB", spec, index, subindex) + payload
    return struct.pack("<HHBB", len(body), 0, 0, MBX_TYPE_COE) + body


def _coe_value(data: bytes, index: int, subindex: int, size: int, signed: bool) -> int:
    if len(data) < SDO_MESSAGE_BYTES:
        raise MasterError(
            f"an SDO reply is {SDO_MESSAGE_BYTES} bytes and {len(data)} came back."
        )
    if (data[5] & 0x0F) != MBX_TYPE_COE:
        raise MasterError(
            "the mailbox reply is not a CoE message. All-zero bytes here are a mailbox nobody "
            "filled — the reply that never came — and reading them as a value would be a "
            "measurement nobody took."
        )
    spec = data[SDO_SPEC_AT]
    reply_index, reply_sub = struct.unpack_from("<HB", data, SDO_INDEX_AT)
    if spec == 0x80:
        code = struct.unpack_from("<I", data, SDO_DATA_AT)[0]
        raise MasterError(
            f"the drive aborted the read of 0x{index:04X}:{subindex} with code 0x{code:08X}. "
            f"An abort is an answer: reading a zero instead would be a measurement nobody took."
        )
    if (reply_index, reply_sub) != (index, subindex):
        raise MasterError(
            f"asked for 0x{index:04X}:{subindex} and the reply names 0x{reply_index:04X}:"
            f"{reply_sub}. A mailbox holds one message and this is somebody else's."
        )
    raw = data[SDO_DATA_AT : SDO_DATA_AT + size]
    return int.from_bytes(raw, "little", signed=signed)


def _coe_check_download(data: bytes, index: int, subindex: int) -> None:
    if len(data) < SDO_MESSAGE_BYTES:
        raise MasterError(
            f"an SDO reply is {SDO_MESSAGE_BYTES} bytes and {len(data)} came back."
        )
    spec = data[SDO_SPEC_AT]
    reply_index, reply_sub = struct.unpack_from("<HB", data, SDO_INDEX_AT)
    if spec == 0x80:
        code = struct.unpack_from("<I", data, SDO_DATA_AT)[0]
        raise MasterError(
            f"the drive aborted the write of 0x{index:04X}:{subindex} with code 0x{code:08X}."
        )
    if spec != 0x60:
        raise MasterError(
            f"the drive answered a download of 0x{index:04X}:{subindex} with 0x{spec:02X} rather "
            f"than an acknowledgement."
        )
    if (reply_index, reply_sub) != (index, subindex):
        raise MasterError(
            f"wrote 0x{index:04X}:{subindex} and the acknowledgement names 0x{reply_index:04X}:"
            f"{reply_sub}."
        )
