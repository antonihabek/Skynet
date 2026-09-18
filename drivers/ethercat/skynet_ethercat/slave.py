"""The software slave stack: an EtherCAT Slave Controller, and a CiA 402 drive behind it.

`DRY-COVERAGE-PLAN.md`'s Phase 2 table names this row's substrate *"software slave stack"*, and this
is it. A slave is two things stacked:

* an **ESC** — a register file at fixed addresses, an SII EEPROM, an AL state machine, sync managers
  over process RAM and FMMUs mapping logical addresses onto physical ones. It is silicon on a real
  slave and arithmetic here, and it is what answers a datagram.
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

## Held to an independent master, not only to this repository's own

The first version of this file was exercised by `master.py` alone, and a master and a slave written
by one author agree with each other whether or not either agrees with ETG.1000. SOEM — a master
nobody here wrote — was run against it over a veth pair (`make check-soem`, `DRY-COVERAGE-PLAN.md`'s
fourteenth session), and everything below marked *ETG.1000* is what that took: the SII, the mailbox
full/empty rules, FMMUs that map by physical address, a broadcast read that ORs, and AL status codes
from the standard's table rather than from memory.

## What a slave refuses, and why each refusal is the interesting part

A slave that answered everything would make a master look correct. This one implements the refusals
a real ESC makes, because they are what a master is actually held to:

* a datagram for an **address this slave does not have** is passed on untouched, and the working
  counter does not move. A master that ignored the counter would read zeros as a measurement.
* a **mailbox read with nothing in it**, and a mailbox write into one that is still full, are denied:
  the working counter does not move for either, which is how a master learns to poll the sync
  manager's status bit first rather than read an empty buffer as a message.
* a **state change ETG.1000.6 does not allow** — Init straight to Op — is refused with 0x0011 and the
  slave stays where it was; the transitions it does allow each need their sync managers configured,
  and say which one was not.
"""

from __future__ import annotations

import struct

from skynet_canopen import cia402

from . import sii

# ── ESC registers (ETG.1000.4), the ones a master actually touches ───────────

REG_TYPE = 0x0000
REG_REVISION = 0x0001
REG_FMMU_COUNT = 0x0004
REG_SM_COUNT = 0x0005
REG_RAM_SIZE = 0x0006
REG_PORT_DESCRIPTOR = 0x0007
REG_FEATURES = 0x0008
REG_STATION_ADDRESS = 0x0010
REG_DL_STATUS = 0x0110
REG_AL_CONTROL = 0x0120
REG_AL_STATUS = 0x0130
REG_AL_STATUS_CODE = 0x0134
REG_PDI_CONTROL = 0x0140
REG_EEPROM_CONTROL = 0x0502
REG_EEPROM_ADDRESS = 0x0504
REG_EEPROM_DATA = 0x0508
REG_SM_BASE = 0x0800
REG_SM_STRIDE = 8
REG_FMMU_BASE = 0x0600
REG_FMMU_STRIDE = 16

#: Process RAM starts here; below it is the register file. Four kilobytes are modelled, which is the
#: smallest ESC's and is what `REG_RAM_SIZE` reports.
PDRAM_START = 0x1000
PDRAM_KILOBYTES = 4

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

#: The transitions ETG.1000.6's state machine allows, from each state. Anything else is 0x0011 and
#: the slave stays where it was — there is no shortcut from Init to Op, whatever is configured.
_TRANSITIONS = {
    AL_INIT: {AL_PREOP},
    AL_PREOP: {AL_INIT, AL_SAFEOP},
    AL_SAFEOP: {AL_INIT, AL_PREOP, AL_OP},
    AL_OP: {AL_INIT, AL_PREOP, AL_SAFEOP},
}

#: AL status codes this stack produces, from ETG.1000.6's table.
#:
#: Checked against an independent master's copy of that table (SOEM's `ec_ALstatuscode2string`),
#: because the first version of this file had three of them on the wrong row — *"invalid SM
#: configuration"* was 0x001D, which is invalid **output** configuration, and the two *"no valid
#: inputs/outputs"* codes were 0x001A and 0x001B, which are a synchronisation error and a sync manager
#: watchdog. Both ends of this driver import these constants, so the two agreed and nothing noticed:
#: F49's shape, a third time (`DRY-COVERAGE-PLAN.md` F52).
ALSTATUS_NO_ERROR = 0x0000
ALSTATUS_INVALID_STATE_CHANGE = 0x0011
ALSTATUS_UNKNOWN_STATE = 0x0012
ALSTATUS_BOOTSTRAP_NOT_SUPPORTED = 0x0013
ALSTATUS_INVALID_MAILBOX_CONFIG = 0x0016
ALSTATUS_INVALID_OUTPUT_CONFIG = 0x001D
ALSTATUS_INVALID_INPUT_CONFIG = 0x001E

AL_STATUS_NAMES = {
    ALSTATUS_NO_ERROR: "no error",
    ALSTATUS_INVALID_STATE_CHANGE: "invalid requested state change",
    ALSTATUS_UNKNOWN_STATE: "unknown requested state",
    ALSTATUS_BOOTSTRAP_NOT_SUPPORTED: "bootstrap not supported",
    ALSTATUS_INVALID_MAILBOX_CONFIG: "invalid mailbox configuration",
    ALSTATUS_INVALID_OUTPUT_CONFIG: "invalid output configuration",
    ALSTATUS_INVALID_INPUT_CONFIG: "invalid input configuration",
}

# ── The EEPROM interface (ETG.1000.4 registers 0x0500-0x050F) ────────────────

#: Bits 8-10 of EEPROM control are the command; the rest of the word is status.
EEPROM_COMMAND_MASK = 0x0700
EEPROM_COMMAND_READ = 0x0100
#: Status: this ESC reads eight bytes per request, so a master can walk the SII in half the reads.
EEPROM_READS_8_BYTES = 0x0040
#: Status: the last command was not acknowledged. Set by a write, because this SII is read-only.
EEPROM_ERROR_ACKNOWLEDGE = 0x2000

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

#: SDO abort codes this drive answers with (CiA 301).
ABORT_NO_OBJECT = 0x06020000
ABORT_READ_ONLY = 0x06010002
ABORT_BAD_COMMAND = 0x05040001

# ── The process data, once ───────────────────────────────────────────────────
#
# Outputs are RxPDO 0x1600 and inputs TxPDO 0x1A00: controlword and target position out, statusword
# and actual position in — the cyclic-synchronous-position minimum, and the same pair
# `drivers/canopen` maps. Every other record of them is derived from these two definitions: the
# mapping objects a master reads over CoE, the SII's PDO categories, the sync manager lengths, and the
# byte layout the process image is packed with. Four records written by hand would be four chances
# for a master reading one of them to configure a slave that means another.

UINT16 = 0x0006  #: CoE base data types (ETG.1000.6)
INT32 = 0x0004

RXPDO = sii.Pdo(
    index=0x1600,
    sync_manager=2,
    name="Outputs",
    entries=(
        sii.PdoEntry(0x6040, 0, 16, UINT16, "Controlword"),
        sii.PdoEntry(0x607A, 0, 32, INT32, "Target position"),
    ),
)
TXPDO = sii.Pdo(
    index=0x1A00,
    sync_manager=3,
    name="Inputs",
    entries=(
        sii.PdoEntry(0x6041, 0, 16, UINT16, "Statusword"),
        sii.PdoEntry(0x6064, 0, 32, INT32, "Position actual value"),
    ),
)
OUTPUT_BYTES = RXPDO.bits // 8
INPUT_BYTES = TXPDO.bits // 8
_OUTPUT_LAYOUT = "<Hi"
_INPUT_LAYOUT = "<Hi"
if struct.calcsize(_OUTPUT_LAYOUT) != OUTPUT_BYTES or struct.calcsize(_INPUT_LAYOUT) != INPUT_BYTES:
    raise AssertionError("the process image layout disagrees with the PDO definitions above")

#: Where this drive's SII recommends its sync managers. A master may configure them anywhere in
#: process RAM — `master.py` does — and one that reads the SII puts them here.
SII_SYNC_MANAGERS = (
    sii.SyncManagerEntry(0x1000, 128, 0x26, sii.SM_MAILBOX_OUT),
    sii.SyncManagerEntry(0x1080, 128, 0x22, sii.SM_MAILBOX_IN),
    sii.SyncManagerEntry(0x1100, OUTPUT_BYTES, 0x64, sii.SM_OUTPUTS),
    sii.SyncManagerEntry(0x1180, INPUT_BYTES, 0x20, sii.SM_INPUTS),
)

#: This simulator's identity. Vendor 0 is unassigned by ETG, so no tool that reads it can mistake
#: this drive for somebody's product; the product code is the profile's number.
VENDOR_ID = 0x00000000
PRODUCT_CODE = 0x00000402
REVISION = 0x00000001

#: The name the SII carries, which is what a master prints for this slave.
DEVICE_NAME = "skynet-ethercat CiA 402 (simulated)"
if len(DEVICE_NAME) > 40:
    raise AssertionError("an SII device name longer than 40 characters is truncated by SOEM")

#: The PDI this ESC reports, and the SII says the same (sii.build writes it into word 0).
PDI_SPI = 0x05


class SyncManager:
    """One sync manager: a window into process RAM, with a mode, a direction and a length.

    Two modes, and the difference is the whole of what a master has to get right (ETG.1000.4):

    * **Mailbox** — one buffer, full or empty. A write is accepted only into an empty buffer and
      completes when its *last* byte is written; a read is accepted only from a full one and empties
      it when its last byte is read. A denied access leaves the working counter where it was.
    * **Buffered** — process data, three buffers, never refused. A write is handed over when its
      last byte lands; a read returns the newest complete buffer.
    """

    def __init__(self) -> None:
        self.start = 0
        self.length = 0
        self.control = 0
        self.enabled = False
        #: Mailbox bytes written so far, and whether the buffer holds a complete message.
        self.buffer = bytearray()
        self.full = False

    @property
    def is_mailbox(self) -> bool:
        """Control bits 0-1: `10` is mailbox mode, `00` is buffered."""
        return (self.control & 0x03) == 0x02

    @property
    def writes_to_slave(self) -> bool:
        """Control bits 2-3: `01` means the master writes; `00` means the master reads."""
        return ((self.control >> 2) & 0x03) == 1

    def covers(self, address: int) -> bool:
        return self.enabled and self.start <= address < self.start + self.length

    def reset(self) -> None:
        self.buffer = bytearray(self.length)
        self.full = False


class Fmmu:
    """One FMMU entry: a logical address range mapped onto physical memory."""

    READ = 1  #: the master reads through it: inputs, or a status register
    WRITE = 2  #: the master writes through it: outputs

    def __init__(self) -> None:
        self.logical_start = 0
        self.length = 0
        self.physical_start = 0
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
        #: Whether a slave follows this one on the segment. The segment says, because only it knows;
        #: it decides what DL status reports for port 1, which is how a master learns the topology.
        self.has_downstream = False
        self._ram = bytearray(PDRAM_KILOBYTES * 1024)
        #: Replies the drive has written and the input mailbox has not yet taken — because it is
        #: still full of the last one, which a master has not read to its end.
        self._outbox: list[bytes] = []

        self.sii = sii.build(
            vendor_id=VENDOR_ID,
            product_code=PRODUCT_CODE,
            revision=REVISION,
            serial=position + 1,
            # Forty characters at most: SOEM keeps `EC_MAXNAME` of them, and the first run cut the
            # closing parenthesis off "(simulated)" — the one word in the name that must survive.
            name=DEVICE_NAME,
            group="Drives",
            sync_managers=SII_SYNC_MANAGERS,
            fmmus=(sii.FMMU_OUTPUTS, sii.FMMU_INPUTS, sii.FMMU_MAILBOX_STATE),
            txpdo=TXPDO,
            rxpdo=RXPDO,
        )
        self._eeprom_status = EEPROM_READS_8_BYTES
        self._eeprom_command_word = 0
        self._eeprom_address = 0
        self._eeprom_data = bytes(8)

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
        self._read_only: set[tuple[int, int]] = set()
        self._install_objects(increments, revolutions, gear_motor, gear_shaft, travel)

        #: The process-data image, as the output and input sync managers carry it.
        self.outputs = bytearray(OUTPUT_BYTES)
        self.inputs = bytearray(INPUT_BYTES)

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

        # The PDO mapping, as a master reads it over CoE (CiA 301 and ETG.1000.6): how many sync
        # managers and what each one is (0x1C00), which PDO each process-data manager carries
        # (0x1C12, 0x1C13), and what each PDO holds (0x1600, 0x1A00). Read-only: this drive's image
        # is fixed, and a mapping object that accepted a write would describe a layout the drive
        # does not use.
        def fixed(index: int, subindex: int, value: int, size: int) -> None:
            self._objects[(index, subindex)] = (value, size, False)
            self._read_only.add((index, subindex))

        fixed(0x1C00, 0, len(SII_SYNC_MANAGERS), 1)
        for number, manager in enumerate(SII_SYNC_MANAGERS, start=1):
            fixed(0x1C00, number, manager.kind, 1)
        for assign, pdo in ((0x1C12, RXPDO), (0x1C13, TXPDO)):
            fixed(assign, 0, 1, 1)
            fixed(assign, 1, pdo.index, 2)
            fixed(pdo.index, 0, len(pdo.entries), 1)
            for number, entry in enumerate(pdo.entries, start=1):
                fixed(pdo.index, number, entry.index << 16 | entry.subindex << 8 | entry.bits, 4)

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

    # ── Physical memory: what a datagram addressed to this slave reaches ─────
    #
    # One entry point for every kind of datagram — auto-increment, configured, broadcast and, through
    # an FMMU, logical — because an ESC has one memory map and the datagram's addressing mode only
    # decides whether this slave is the one being asked. `None` from a read and `False` from a write
    # mean *denied*: the datagram passes on untouched and the working counter does not move.

    def read_physical(self, address: int, length: int) -> bytes | None:
        if address < PDRAM_START:
            return self.read_register(address, length)
        manager = self._sync_manager_at(address)
        if manager is None:
            offset = address - PDRAM_START
            return bytes(self._ram[offset : offset + length]).ljust(length, b"\x00")
        offset = address - manager.start
        last = address + length >= manager.start + manager.length

        if manager.is_mailbox:
            if manager.writes_to_slave or not manager.full:
                return None
            out = bytes(manager.buffer[offset : offset + length]).ljust(length, b"\x00")
            if last:
                manager.reset()
                self._deliver()
            return out

        if manager.writes_to_slave:
            return None
        if offset == 0:
            self.refresh_inputs()
        return bytes(self.inputs[offset : offset + length]).ljust(length, b"\x00")

    def write_physical(self, address: int, data: bytes) -> bool:
        if address < PDRAM_START:
            self.write_register(address, data)
            return True
        manager = self._sync_manager_at(address)
        if manager is None:
            offset = address - PDRAM_START
            self._ram[offset : offset + len(data)] = data[: max(0, len(self._ram) - offset)]
            return True
        offset = address - manager.start
        last = address + len(data) >= manager.start + manager.length
        if not manager.writes_to_slave:
            return False

        if manager.is_mailbox:
            if manager.full:
                return False
            manager.buffer[offset : offset + len(data)] = data[: manager.length - offset]
            if last:
                message = bytes(manager.buffer)
                # The drive takes the message the moment it is complete, so the buffer is empty
                # again before the next frame — a real ESC's firmware does the same within a cycle.
                manager.reset()
                self.write_mailbox(message)
            return True

        self.outputs[offset : offset + len(data)] = data[: OUTPUT_BYTES - offset]
        if last:
            self.apply_outputs()
        return True

    def _sync_manager_at(self, address: int) -> SyncManager | None:
        for manager in self.sync_managers:
            if manager.covers(address):
                return manager
        return None

    # ── The register file ────────────────────────────────────────────────────

    def read_register(self, register: int, length: int) -> bytes:
        return bytes(self._register_byte(register + offset) for offset in range(length))

    def _register_byte(self, address: int) -> int:
        if address == REG_TYPE:
            return 0x04
        if address == REG_REVISION:
            return 0x01
        if address == REG_FMMU_COUNT:
            return len(self.fmmus)
        if address == REG_SM_COUNT:
            return len(self.sync_managers)
        if address == REG_RAM_SIZE:
            return PDRAM_KILOBYTES
        if address == REG_PORT_DESCRIPTOR:
            # Two bits a port: ports 0 and 1 are MII (`11`), ports 2 and 3 are not implemented.
            return 0x0F
        if address == REG_FEATURES:
            # Bit 0: FMMUs are byte-oriented. Bit 2 — distributed clocks — is clear, and that is the
            # one this row cannot honestly set: DC is propagation delay measured on copper, and a
            # software segment has none to measure (see README.md).
            return 0x01
        if address in (REG_STATION_ADDRESS, REG_STATION_ADDRESS + 1):
            return (self.station_address >> (8 * (address - REG_STATION_ADDRESS))) & 0xFF
        if address in (REG_DL_STATUS, REG_DL_STATUS + 1):
            return (self._dl_status() >> (8 * (address - REG_DL_STATUS))) & 0xFF
        if address == REG_AL_STATUS:
            return (self.al_state | (AL_ERROR_BIT if self.al_error else 0)) & 0xFF
        if address in (REG_AL_STATUS_CODE, REG_AL_STATUS_CODE + 1):
            return (self.al_status_code >> (8 * (address - REG_AL_STATUS_CODE))) & 0xFF
        if address == REG_PDI_CONTROL:
            return PDI_SPI
        if address in (REG_EEPROM_CONTROL, REG_EEPROM_CONTROL + 1):
            return (self._eeprom_status >> (8 * (address - REG_EEPROM_CONTROL))) & 0xFF
        if REG_EEPROM_ADDRESS <= address < REG_EEPROM_ADDRESS + 4:
            return (self._eeprom_address >> (8 * (address - REG_EEPROM_ADDRESS))) & 0xFF
        if REG_EEPROM_DATA <= address < REG_EEPROM_DATA + 8:
            return self._eeprom_data[address - REG_EEPROM_DATA]
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
                # The status byte. Bit 3 is "mailbox full", which is what a master polls before
                # reading a mailbox — reading an empty one is denied rather than answered.
                return 0x08 if manager.is_mailbox and manager.full else 0x00
            if field == 6:
                return 0x01 if manager.enabled else 0x00
            return 0
        return 0

    def _dl_status(self) -> int:
        """DL status (0x0110): which ports have a link, and on which the loop is open.

        Port 0 faces the master and is always open with communication established. Port 1 is open
        when a slave follows this one and closed when this is the end of the line; ports 2 and 3 do
        not exist here and read as closed. A master walks these bits to learn the topology, and
        every slave reporting zero — the first version of this file — reads as a segment with no
        links at all.
        """
        low = 0x01 | 0x10  # PDI operational; link on port 0
        high = 0x02  # port 0: loop open, communication established
        if self.has_downstream:
            low |= 0x20  # link on port 1
            high |= 0x08  # port 1: loop open, communication established
        else:
            high |= 0x04  # port 1: loop closed
        high |= 0x10 | 0x40  # ports 2 and 3: loop closed
        return low | (high << 8)

    def write_register(self, register: int, data: bytes) -> None:
        for offset, byte in enumerate(data):
            self._write_register_byte(register + offset, byte)
        # AL Control and EEPROM control act once the whole datagram has landed, because a master
        # writes the command and its operands in one datagram and the command reads the operands.
        if register <= REG_AL_CONTROL < register + len(data):
            self._request_state(data[REG_AL_CONTROL - register] & 0x0F)
        if register <= REG_EEPROM_CONTROL + 1 < register + len(data):
            self._eeprom_command()

    def _write_register_byte(self, address: int, byte: int) -> None:
        if address == REG_STATION_ADDRESS:
            self.station_address = (self.station_address & 0xFF00) | byte
        elif address == REG_STATION_ADDRESS + 1:
            self.station_address = (self.station_address & 0x00FF) | (byte << 8)
        elif address in (REG_EEPROM_CONTROL, REG_EEPROM_CONTROL + 1):
            shift = 8 * (address - REG_EEPROM_CONTROL)
            self._eeprom_command_word = (self._eeprom_command_word & ~(0xFF << shift)) | (byte << shift)
        elif REG_EEPROM_ADDRESS <= address < REG_EEPROM_ADDRESS + 4:
            shift = 8 * (address - REG_EEPROM_ADDRESS)
            self._eeprom_address = (self._eeprom_address & ~(0xFF << shift)) | (byte << shift)
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
                # Activation empties the buffer, so a manager reconfigured mid-run does not hand
                # anybody half of a message addressed to its previous window.
                manager.enabled = bool(byte & 0x01)
                manager.reset()
                if manager.enabled:
                    self._deliver()
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

    def _eeprom_command(self) -> None:
        """Run the command a master just wrote to EEPROM control, against the SII image.

        Instantaneous — busy is never set — and read-only: a write is not acknowledged, because a
        simulated drive whose identity could be rewritten over the wire would be a different drive
        after every run of an EEPROM tool.
        """
        command = self._eeprom_command_word & EEPROM_COMMAND_MASK
        if command == 0:
            # NOP, which is how a master clears the error bits before its next command.
            self._eeprom_status = EEPROM_READS_8_BYTES
            return
        if command == EEPROM_COMMAND_READ:
            at = 2 * (self._eeprom_address & 0xFFFF)
            chunk = self.sii[at : at + 8]
            # An EEPROM read past its end returns erased cells, 0xFF — not zeros, which a master
            # walking the categories would read as one more category of type 0.
            self._eeprom_data = chunk + b"\xff" * (8 - len(chunk))
            self._eeprom_status = EEPROM_READS_8_BYTES
            return
        self._eeprom_status = EEPROM_READS_8_BYTES | EEPROM_ERROR_ACKNOWLEDGE

    # ── The AL state machine (ETG.1000.6) ────────────────────────────────────

    def _request_state(self, wanted: int) -> None:
        """What a slave does when a master writes AL Control.

        Every refusal here is one a real bring-up meets, and each leaves the slave in the state it
        held — with the error bit set and a code at 0x0134 saying why. A master that read only the
        state would see it stay put with no explanation.
        """
        if wanted == self.al_state:
            self.al_error = False
            self.al_status_code = ALSTATUS_NO_ERROR
            return
        if wanted == AL_BOOT:
            self._refuse(ALSTATUS_BOOTSTRAP_NOT_SUPPORTED)
            return
        if wanted not in _TRANSITIONS:
            self._refuse(ALSTATUS_UNKNOWN_STATE)
            return
        if wanted not in _TRANSITIONS[self.al_state]:
            self._refuse(ALSTATUS_INVALID_STATE_CHANGE)
            return

        if wanted == AL_PREOP and self.al_state == AL_INIT:
            # Pre-Op is the state in which a master has a mailbox, so both mailbox managers must be
            # configured — one the master writes, one it reads — before it may be entered.
            if not (self._configured(mailbox=True, writes=True) and self._configured(mailbox=True, writes=False)):
                self._refuse(ALSTATUS_INVALID_MAILBOX_CONFIG)
                return
        if wanted == AL_SAFEOP and self.al_state == AL_PREOP:
            # Safe-Op is the state in which inputs are live, so both process-data managers must be
            # configured, each exactly the size of the PDO it carries. A master that sized one from
            # a different mapping would exchange the wrong number of bytes on every cycle.
            if self._configured(mailbox=False, writes=True) != OUTPUT_BYTES:
                self._refuse(ALSTATUS_INVALID_OUTPUT_CONFIG)
                return
            if self._configured(mailbox=False, writes=False) != INPUT_BYTES:
                self._refuse(ALSTATUS_INVALID_INPUT_CONFIG)
                return

        if wanted == AL_INIT:
            # Init clears the configuration, which is what makes it the state a master goes back to
            # rather than a reset. It does **not** clear a latched fault: F26's rule, from
            # `drivers/canopen` — the erased thing would be evidence about why a robot stopped.
            for manager in self.sync_managers:
                manager.enabled = False
                manager.reset()
            for fmmu in self.fmmus:
                fmmu.enabled = False
            self._outbox.clear()
        self.al_state = wanted
        self.al_error = False
        self.al_status_code = ALSTATUS_NO_ERROR

    def _refuse(self, code: int) -> None:
        self.al_error = True
        self.al_status_code = code

    def _configured(self, *, mailbox: bool, writes: bool) -> int:
        """The length of the enabled manager of this mode and direction, or 0 when there is none."""
        for manager in self.sync_managers:
            if (
                manager.enabled
                and manager.length
                and manager.is_mailbox == mailbox
                and manager.writes_to_slave == writes
            ):
                return manager.length
        return 0

    # ── The mailbox: CoE SDO ─────────────────────────────────────────────────

    def write_mailbox(self, data: bytes) -> None:
        """A complete message from the master. The answer is posted for it to read."""
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
                # Named rather than answered with zeros, because a master reading a zero for an
                # encoder resolution would divide by it.
                self._post(self._abort(index, subindex, ABORT_NO_OBJECT))
                return
            value, size, signed = entry
            raw_value = int(value) & ((1 << (size * 8)) - 1) if signed else int(value)
            self._post(self._upload_response(index, subindex, raw_value, size))
            return

        if (command & 0xE0) == 0x20:  # expedited download
            size = 4 - ((command >> 2) & 0x03)
            value = int.from_bytes(data[SDO_DATA_AT : SDO_DATA_AT + size], "little", signed=False)
            if self._entry(index, subindex) is None:
                self._post(self._abort(index, subindex, ABORT_NO_OBJECT))
                return
            if (index, subindex) in self._read_only:
                self._post(self._abort(index, subindex, ABORT_READ_ONLY))
                return
            self._write_object(index, subindex, value)
            self._post(self._download_response(index, subindex))
            return

        self._post(self._abort(index, subindex, ABORT_BAD_COMMAND))

    def _post(self, reply: bytes) -> None:
        self._outbox.append(reply)
        self._deliver()

    def _deliver(self) -> None:
        """Move the oldest waiting reply into the input mailbox, if it is empty.

        A reply is never written over one the master has not read — that would lose an answer and
        hand the master the next one as though it were this one.
        """
        if not self._outbox:
            return
        for manager in self.sync_managers:
            if manager.enabled and manager.is_mailbox and not manager.writes_to_slave:
                if manager.full:
                    return
                reply = self._outbox.pop(0)
                manager.buffer = bytearray(reply[: manager.length].ljust(manager.length, b"\x00"))
                manager.full = True
                return

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
        struct.pack_into(_INPUT_LAYOUT, self.inputs, 0, statusword, self.position_increments)

    def apply_outputs(self) -> None:
        # Outputs are acted on in Op and nowhere else. Safe-Op is named for exactly this: inputs are
        # live and the drive holds its outputs in a safe state — so a master that exchanges process
        # data before Op, as SOEM does to make its first outputs valid, moves nothing.
        if self.al_state != AL_OP:
            return
        controlword, target = struct.unpack_from(_OUTPUT_LAYOUT, self.outputs, 0)
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
