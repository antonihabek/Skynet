"""The software slave stack: an EtherCAT Slave Controller, and a CiA 402 drive behind it.

`DRY-COVERAGE-PLAN.md`'s Phase 2 table names this row's substrate *"software slave stack"*, and this
is it. A slave is two things stacked:

* an **ESC** — a register file at fixed addresses, an AL state machine, sync managers and FMMUs. It
  is silicon on a real slave and arithmetic here, and it is what answers a datagram.
* a **drive** — the CiA 402 profile, reached over CoE (CANopen over EtherCAT) through the ESC's
  mailbox sync managers, and mapped into process data through the others.

## The profile is `skynet_canopen.cia402`, imported rather than rewritten

CoE is CANopen's application layer on EtherCAT's transport. The state machine, the statusword
decoding and the increments-to-radians arithmetic are **the same profile**, so this driver imports
the module `drivers/canopen` already holds rather than writing a second copy.

That is §80's rule applied where it actually bites: two implementations of one state machine
disagree exactly once, after the failure that matters, and a CiA 402 transition table written twice
would be the clearest possible example. What is genuinely different between the two drivers — SDO
framing, PDO mapping, the transport, the addressing — is written here and only here.

## What a slave refuses, and why each refusal is the interesting part

A slave that answered everything would make a master look correct. This one implements the refusals
a real ESC makes, because they are what a master is actually held to:

* a datagram for an **address this slave does not have** is passed on untouched, and the working
  counter does not move. A master that ignored the counter would read zeros as a measurement.
* **Op is refused without process data**: a master that asks for Op before the sync managers are
  configured gets Safe-Op + an AL status code, which is exactly the failure a real bring-up meets.
* a **mailbox read with nothing in it** answers a working counter of zero rather than an empty
  message, because the master is supposed to poll the sync manager's status first.
"""

from __future__ import annotations

import struct

from skynet_canopen import cia402

from . import frames

# ── ESC registers (ETG.1000.4), the handful a master actually touches ────────

REG_TYPE = 0x0000
REG_REVISION = 0x0001
REG_BUILD = 0x0002
REG_FMMU_COUNT = 0x0004
REG_SM_COUNT = 0x0005
REG_STATION_ADDRESS = 0x0010
REG_AL_CONTROL = 0x0120
REG_AL_STATUS = 0x0130
REG_AL_STATUS_CODE = 0x0134
REG_SM_BASE = 0x0800
REG_SM_STRIDE = 8
REG_FMMU_BASE = 0x0600
REG_FMMU_STRIDE = 16

# ── AL states (ETG.1000.6) ───────────────────────────────────────────────────

AL_INIT = 0x01
AL_PREOP = 0x02
AL_BOOT = 0x03
AL_SAFEOP = 0x04
AL_OP = 0x08
#: Set by the slave in AL Status when it refused the state the master asked for. The master reads
#: the code at 0x0134 to find out why, and a master that only read the state would see it settle at
#: the old one with no explanation.
AL_ERROR_BIT = 0x10

AL_NAMES = {AL_INIT: "Init", AL_PREOP: "Pre-Op", AL_BOOT: "Boot", AL_SAFEOP: "Safe-Op", AL_OP: "Op"}

#: AL status codes this stack produces, from ETG.1000.6's table.
ALSTATUS_NO_ERROR = 0x0000
ALSTATUS_INVALID_REQUESTED_STATE = 0x0011
ALSTATUS_INVALID_SM_CFG = 0x001D
ALSTATUS_NO_VALID_INPUTS = 0x001A
ALSTATUS_NO_VALID_OUTPUTS = 0x001B

# ── CoE over the mailbox (ETG.1000.6 §5.6) ───────────────────────────────────

MBX_TYPE_COE = 0x03
COE_SDO_REQUEST = 0x02
COE_SDO_RESPONSE = 0x03

#: Where the fields sit in a CoE SDO message, counted once.
#:
#: Six bytes of mailbox header (length, address, channel+priority, type+counter), two of CoE header
#: (number and service), then the SDO itself: one byte of command specifier, a two-byte index, a
#: one-byte subindex and four bytes of data. Sixteen in total for an expedited transfer.
#:
#: Written down as constants because the first version of this file counted them twice — once in the
#: slave's reader and once in the master's — and the two disagreed by one byte, which presented as
#: *"asked for 0x608F:1 and the reply names 0x0100:0"*. Two records of one fact, disagreeing exactly
#: once (§80).
MBX_HEADER_BYTES = 6
COE_HEADER_BYTES = 2
SDO_SPEC_AT = MBX_HEADER_BYTES + COE_HEADER_BYTES
SDO_INDEX_AT = SDO_SPEC_AT + 1
SDO_SUBINDEX_AT = SDO_SPEC_AT + 3
SDO_DATA_AT = SDO_SPEC_AT + 4
SDO_MESSAGE_BYTES = SDO_DATA_AT + 4

#: Expedited SDO, the only kind this driver speaks. A segmented transfer carries a string or a long
#: array, and every object this driver reads fits in four bytes — so a segmented reply is refused by
#: name rather than read for its first segment.
SDO_UPLOAD_REQUEST = 0x40
SDO_DOWNLOAD_RESPONSE = 0x60
SDO_ABORT = 0x80


class SyncManager:
    """One sync manager: a window into the slave's memory with a direction and a length."""

    def __init__(self) -> None:
        self.start = 0
        self.length = 0
        self.control = 0
        self.enabled = False
        #: Mailbox sync managers carry one message at a time and say whether one is waiting.
        self.mailbox: bytes = b""

    @property
    def writes_to_slave(self) -> bool:
        """Direction bits 2..3 of the control byte: 0 = master reads, 1 = master writes."""
        return ((self.control >> 2) & 0x03) == 1


class Fmmu:
    """One FMMU entry: a logical address range mapped onto physical memory."""

    def __init__(self) -> None:
        self.logical_start = 0
        self.length = 0
        self.physical_start = 0
        #: Type 1 = read (inputs), 2 = write (outputs).
        self.type = 0
        self.enabled = False


class EtherCatDrive:
    """One CiA 402 servo drive behind an ESC, at one position on the segment."""

    def __init__(
        self,
        position: int,
        *,
        name: str = "drive",
        role: str | None = None,
        increments: int = 1 << 16,
        revolutions: int = 1,
        gear_motor: int = 50,
        gear_shaft: int = 1,
        travel: tuple[int, int] = (-(1 << 22), 1 << 22),
        report_units: bool = True,
    ) -> None:
        self._report_units = report_units
        self.position = position
        self.name = name
        self.role = role
        self.station_address = 0
        self.al_state = AL_INIT
        self.al_status_code = ALSTATUS_NO_ERROR
        self.al_error = False
        self.sync_managers = [SyncManager() for _ in range(4)]
        self.fmmus = [Fmmu() for _ in range(3)]

        #: The drive itself. The object *definitions* and the state machine are
        #: `skynet_canopen.cia402`'s — one profile, one implementation — and the storage is this
        #: slave's, exactly as it is the CANopen drive's.
        self.state = cia402.State.SWITCH_ON_DISABLED
        self.previous_controlword = 0
        self.position_increments = 0
        self.target_increments = 0
        #: Cyclic synchronous position. Not a profile mode, for `cia402.Mode`'s own reason: §7's
        #: loop produces a setpoint every cycle and a profile mode would have the drive interpolate
        #: between them — two interpolators, one of which has never heard of the Safety Kernel.
        self.mode = cia402.Mode.CSP
        #: Set by a test to latch a fault, exactly as a real drive latches one.
        self.fault_code = 0
        self._objects: dict[tuple[int, int], tuple[int, int, bool]] = {}
        self._install_objects(increments, revolutions, gear_motor, gear_shaft, travel)

        #: The process-data image, as the master's FMMUs will read and write it. Outputs are
        #: controlword + target position; inputs are statusword + actual position.
        self.outputs = bytearray(6)
        self.inputs = bytearray(6)

    # ── The object dictionary ────────────────────────────────────────────────
    #
    # The same shape `drivers/canopen`'s simulated drive uses, and for the same reason: the objects
    # are `cia402.OD`'s definitions and the *values* are this drive's storage. A second definition
    # of `0x608F` would be a second account of what an encoder resolution is.

    def _install_objects(
        self,
        increments: int,
        revolutions: int,
        gear_motor: int,
        gear_shaft: int,
        travel: tuple[int, int],
    ) -> None:
        def put(obj, value: int) -> None:
            self._objects[(obj.index, obj.subindex)] = (value, obj.size, obj.signed)

        put(cia402.OD.POSITION_LIMIT_MIN, travel[0])
        put(cia402.OD.POSITION_LIMIT_MAX, travel[1])
        put(cia402.OD.MAX_MOTOR_SPEED, 3000)
        put(cia402.OD.MODES_OF_OPERATION, self.mode.value)
        if self._report_units:
            put(cia402.OD.ENCODER_INCREMENTS, increments)
            put(cia402.OD.ENCODER_REVOLUTIONS, revolutions)
            put(cia402.OD.GEAR_MOTOR_REVOLUTIONS, gear_motor)
            put(cia402.OD.GEAR_SHAFT_REVOLUTIONS, gear_shaft)

    def _entry(self, index: int, subindex: int) -> tuple[int, int, bool] | None:
        """Storage, or one of the objects whose value is state rather than storage."""
        if (index, subindex) == (0x6041, 0):
            statusword = cia402.statusword_for(self.state) | (0x08 if self.fault_code else 0)
            return (statusword, 2, False)
        if (index, subindex) == (0x6040, 0):
            return (self.previous_controlword, 2, False)
        if (index, subindex) == (0x6064, 0):
            return (self.position_increments, 4, True)
        if (index, subindex) == (0x607A, 0):
            return (self.target_increments, 4, True)
        if (index, subindex) == (0x6061, 0):
            return (self.mode.value, 1, True)
        if (index, subindex) == (0x603F, 0):
            return (self.fault_code, 2, False)
        return self._objects.get((index, subindex))

    # ── The register file ────────────────────────────────────────────────────

    def read_register(self, register: int, length: int) -> bytes:
        out = bytearray(length)
        for offset in range(length):
            address = register + offset
            out[offset] = self._register_byte(address)
        return bytes(out)

    def _register_byte(self, address: int) -> int:
        if address == REG_TYPE:
            return 0x04  # an ESC with DPRAM, which is what a servo drive has
        if address == REG_REVISION:
            return 0x01
        if address == REG_FMMU_COUNT:
            return len(self.fmmus)
        if address == REG_SM_COUNT:
            return len(self.sync_managers)
        if address == REG_STATION_ADDRESS:
            return self.station_address & 0xFF
        if address == REG_STATION_ADDRESS + 1:
            return (self.station_address >> 8) & 0xFF
        if address == REG_AL_STATUS:
            return (self.al_state | (AL_ERROR_BIT if self.al_error else 0)) & 0xFF
        if address == REG_AL_STATUS + 1:
            return 0
        if address == REG_AL_STATUS_CODE:
            return self.al_status_code & 0xFF
        if address == REG_AL_STATUS_CODE + 1:
            return (self.al_status_code >> 8) & 0xFF
        if REG_SM_BASE <= address < REG_SM_BASE + REG_SM_STRIDE * len(self.sync_managers):
            index = (address - REG_SM_BASE) // REG_SM_STRIDE
            field = (address - REG_SM_BASE) % REG_SM_STRIDE
            manager = self.sync_managers[index]
            if field == 0:
                return manager.start & 0xFF
            if field == 1:
                return (manager.start >> 8) & 0xFF
            if field == 2:
                return manager.length & 0xFF
            if field == 3:
                return (manager.length >> 8) & 0xFF
            if field == 4:
                return manager.control & 0xFF
            if field == 5:
                # The status byte. Bit 3 is "a mailbox message is waiting", which is what a master
                # polls before reading one — and reading a mailbox that has nothing in it is the
                # refusal this stack models rather than answering an empty message.
                return 0x08 if manager.mailbox else 0x00
            if field == 6:
                return 0x01 if manager.enabled else 0x00
            return 0
        return 0

    def write_register(self, register: int, data: bytes) -> None:
        for offset, byte in enumerate(data):
            self._write_register_byte(register + offset, byte)
        # AL Control is two bytes and the state machine runs once both have landed.
        if register <= REG_AL_CONTROL < register + len(data):
            self._request_state(data[REG_AL_CONTROL - register] & 0x0F)

    def _write_register_byte(self, address: int, byte: int) -> None:
        if address == REG_STATION_ADDRESS:
            self.station_address = (self.station_address & 0xFF00) | byte
        elif address == REG_STATION_ADDRESS + 1:
            self.station_address = (self.station_address & 0x00FF) | (byte << 8)
        elif REG_SM_BASE <= address < REG_SM_BASE + REG_SM_STRIDE * len(self.sync_managers):
            index = (address - REG_SM_BASE) // REG_SM_STRIDE
            field = (address - REG_SM_BASE) % REG_SM_STRIDE
            manager = self.sync_managers[index]
            if field == 0:
                manager.start = (manager.start & 0xFF00) | byte
            elif field == 1:
                manager.start = (manager.start & 0x00FF) | (byte << 8)
            elif field == 2:
                manager.length = (manager.length & 0xFF00) | byte
            elif field == 3:
                manager.length = (manager.length & 0x00FF) | (byte << 8)
            elif field == 4:
                manager.control = byte
            elif field == 6:
                manager.enabled = bool(byte & 0x01)
        elif REG_FMMU_BASE <= address < REG_FMMU_BASE + REG_FMMU_STRIDE * len(self.fmmus):
            index = (address - REG_FMMU_BASE) // REG_FMMU_STRIDE
            field = (address - REG_FMMU_BASE) % REG_FMMU_STRIDE
            fmmu = self.fmmus[index]
            if field < 4:
                shift = field * 8
                fmmu.logical_start = (fmmu.logical_start & ~(0xFF << shift)) | (byte << shift)
            elif field == 4:
                fmmu.length = (fmmu.length & 0xFF00) | byte
            elif field == 5:
                fmmu.length = (fmmu.length & 0x00FF) | (byte << 8)
            elif field == 8:
                fmmu.physical_start = (fmmu.physical_start & 0xFF00) | byte
            elif field == 9:
                fmmu.physical_start = (fmmu.physical_start & 0x00FF) | (byte << 8)
            elif field == 11:
                # Byte 10 is the physical start bit and byte 11 is the type; byte 12 activates.
                # These three are one apart and the two ends of this driver each counted them
                # separately at first, which is §80's shape again: an FMMU configured one byte
                # early maps nothing, nothing refuses, and the cycle's working counter is zero.
                fmmu.type = byte
            elif field == 12:
                fmmu.enabled = bool(byte & 0x01)

    def _request_state(self, wanted: int) -> None:
        """The AL state machine: what a slave does when a master writes AL Control.

        Every refusal here is one a real bring-up meets, and each leaves the slave in the state it
        could hold rather than the one it was asked for — with a code at 0x0134 saying why. A master
        that read only the state would see it settle somewhere unexpected with no explanation.
        """
        if wanted == self.al_state:
            self.al_error = False
            self.al_status_code = ALSTATUS_NO_ERROR
            return
        if wanted not in (AL_INIT, AL_PREOP, AL_SAFEOP, AL_OP):
            self.al_error = True
            self.al_status_code = ALSTATUS_INVALID_REQUESTED_STATE
            return
        if wanted in (AL_SAFEOP, AL_OP):
            # Safe-Op needs the mailbox *and* the process-data sync managers; Op needs outputs too.
            if not all(manager.enabled for manager in self.sync_managers[:3]):
                self.al_error = True
                self.al_status_code = ALSTATUS_INVALID_SM_CFG
                self.al_state = min(self.al_state, AL_PREOP)
                return
        if wanted == AL_OP:
            if not self.sync_managers[3].enabled:
                self.al_error = True
                self.al_status_code = ALSTATUS_NO_VALID_OUTPUTS
                self.al_state = AL_SAFEOP
                return
        if wanted == AL_INIT:
            # Init clears the configuration, which is what makes it the state a master goes back to
            # rather than a reset. It does **not** clear a latched fault: F26's rule, from
            # `drivers/canopen` — the erased thing would be evidence about why a robot stopped.
            for manager in self.sync_managers:
                manager.enabled = False
                manager.mailbox = b""
            for fmmu in self.fmmus:
                fmmu.enabled = False
        self.al_state = wanted
        self.al_error = False
        self.al_status_code = ALSTATUS_NO_ERROR

    # ── The mailbox: CoE SDO ─────────────────────────────────────────────────

    def write_mailbox(self, data: bytes) -> None:
        """A master's SDO request. The answer is left in sync manager 1 for it to read."""
        if len(data) < SDO_MESSAGE_BYTES:
            return
        mbx_type = data[5] & 0x0F
        if mbx_type != MBX_TYPE_COE:
            return
        command = data[SDO_SPEC_AT]
        index, subindex = struct.unpack_from("<HB", data, SDO_INDEX_AT)

        if (command & 0xF0) == SDO_UPLOAD_REQUEST:
            entry = self._entry(index, subindex)
            if entry is None:
                # 0x06020000 — the object does not exist. Named rather than answered with zeros,
                # because a master reading a zero for an encoder resolution would divide by it.
                self.sync_managers[1].mailbox = self._abort(index, subindex, 0x06020000)
                return
            value, size, signed = entry
            raw_value = int(value) & ((1 << (size * 8)) - 1) if signed else int(value)
            self.sync_managers[1].mailbox = self._upload_response(index, subindex, raw_value, size)
            return

        if (command & 0xE0) == 0x20:  # expedited download
            size = 4 - ((command >> 2) & 0x03)
            value = int.from_bytes(data[SDO_DATA_AT : SDO_DATA_AT + size], "little", signed=False)
            if self._entry(index, subindex) is None:
                self.sync_managers[1].mailbox = self._abort(index, subindex, 0x06020000)
                return
            self._write_object(index, subindex, value)
            self.sync_managers[1].mailbox = self._download_response(index, subindex)
            return

        self.sync_managers[1].mailbox = self._abort(index, subindex, 0x05040001)

    def read_mailbox(self) -> bytes:
        message = self.sync_managers[1].mailbox
        self.sync_managers[1].mailbox = b""
        return message

    def _header(self, length: int) -> bytes:
        return struct.pack("<HHBB", length, 0, 0, MBX_TYPE_COE)

    def _upload_response(self, index: int, subindex: int, value: int, size: int) -> bytes:
        body = struct.pack("<H", COE_SDO_RESPONSE << 12 | 0)
        spec = 0x43 | ((4 - size) << 2)
        body += struct.pack("<BHB", spec, index, subindex)
        body += int(value).to_bytes(4, "little", signed=False)
        return self._header(len(body)) + body

    def _download_response(self, index: int, subindex: int) -> bytes:
        body = struct.pack("<H", COE_SDO_RESPONSE << 12 | 0)
        body += struct.pack("<BHB", SDO_DOWNLOAD_RESPONSE, index, subindex)
        body += b"\x00\x00\x00\x00"
        return self._header(len(body)) + body

    def _abort(self, index: int, subindex: int, code: int) -> bytes:
        body = struct.pack("<H", COE_SDO_RESPONSE << 12 | 0)
        body += struct.pack("<BHB", SDO_ABORT, index, subindex)
        body += struct.pack("<I", code)
        return self._header(len(body)) + body

    def _write_object(self, index: int, subindex: int, value: int) -> None:
        if index == 0x6040:
            self._apply_controlword(value)
            return
        if index == 0x607A:
            self.target_increments = struct.unpack("<i", struct.pack("<I", value))[0]
            return
        if index == 0x6060:
            self.mode = cia402.Mode(value) if value in [m.value for m in cia402.Mode] else self.mode
            return
        held = self._objects.get((index, subindex))
        if held is not None:
            self._objects[(index, subindex)] = (value, held[1], held[2])

    def _apply_controlword(self, controlword: int) -> None:
        command = cia402.command_of(controlword, self.previous_controlword)
        self.previous_controlword = controlword
        if command is None:
            return
        if self.fault_code and command is not cia402.Command.FAULT_RESET:
            # A latched fault is not commanded through. `drivers/canopen`'s F26 is the reason this
            # is here rather than a reset at bring-up: the erased thing is why a robot stopped.
            return
        if command is cia402.Command.FAULT_RESET:
            self.fault_code = 0
        self.state = cia402.next_state(self.state, command)

    # ── Process data ─────────────────────────────────────────────────────────

    def refresh_inputs(self) -> None:
        statusword = cia402.statusword_for(self.state) | (0x08 if self.fault_code else 0)
        struct.pack_into("<Hi", self.inputs, 0, statusword, self.position_increments)

    def apply_outputs(self) -> None:
        controlword, target = struct.unpack_from("<Hi", self.outputs, 0)
        self._apply_controlword(controlword)
        self.target_increments = target
        if self.state is cia402.State.OPERATION_ENABLED and not self.fault_code:
            # One step towards the setpoint per cycle. Not a model of a servo — a body that moves
            # when commanded and stops when not, so a master's setpoint is visibly what moved it.
            delta = self.target_increments - self.position_increments
            step = max(-20000, min(20000, delta))
            self.position_increments += step

    def latch_fault(self, code: int = 0x2310) -> None:
        self.fault_code = code
        self.state = cia402.State.FAULT
