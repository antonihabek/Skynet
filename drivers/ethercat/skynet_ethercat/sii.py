"""The Slave Information Interface: the EEPROM image an ESC hands a master before anything else.

A real EtherCAT slave carries a small EEPROM, and the first thing any master does after counting the
segment is read it: who made the slave, which product it is, where its mailbox lives, which mailbox
protocols it speaks, and — in categories after the fixed header — its name, its sync managers, its
FMMUs and its process data. ETG.2010 fixes the layout, and this module writes it.

The first version of this stack had no SII at all, and nothing noticed, because this repository's own
master never asks for one: it writes its sync managers from constants it already knows. An independent
master does not. SOEM's `ecx_config_init` reads the vendor, product, revision and serial words, both
mailbox windows and the mailbox protocols before it will configure a slave, and a slave that answered
those reads with zeros would be configured as a device with no mailbox and no process data — which is
not a refusal anywhere, and is what `DRY-COVERAGE-PLAN.md`'s fourteenth session found by running one.

Nothing here is a manufacturer's. The identity words are this simulator's own, and the vendor ID is
zero, which ETG does not assign to anybody: a simulated drive that answered with a real vendor's ID
would be a device claiming to be something it is not, to every tool that reads it.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

#: ETG.2010's category types, the ones this image carries.
CATEGORY_STRINGS = 10
CATEGORY_GENERAL = 30
CATEGORY_FMMU = 40
CATEGORY_SYNC_MANAGER = 41
CATEGORY_TXPDO = 50
CATEGORY_RXPDO = 51
CATEGORY_END = 0xFFFF

#: Where the categories start, in words: after the 64-word fixed header.
CATEGORIES_AT = 0x0040

#: Word addresses in the fixed header (ETG.2010 table 2).
WORD_PDI_CONTROL = 0x0000
WORD_CHECKSUM = 0x0007
WORD_VENDOR_ID = 0x0008
WORD_PRODUCT_CODE = 0x000A
WORD_REVISION = 0x000C
WORD_SERIAL = 0x000E
WORD_RX_MAILBOX = 0x0018
WORD_TX_MAILBOX = 0x001A
WORD_MAILBOX_PROTOCOL = 0x001C
WORD_SIZE = 0x003E
WORD_VERSION = 0x003F

#: The mailbox protocol word's CoE bit. The only protocol this drive speaks.
MAILBOX_COE = 0x0004

#: The General category's CoE details byte: bit 0 is *SDO supported*. Bit 1 (SDO information) and
#: bit 5 (complete access) are not set, because this drive answers neither — and a master that read
#: them set would ask for an object dictionary listing or a whole-object transfer and be refused.
COE_DETAILS_SDO = 0x01

#: FMMU usage codes (ETG.2010 category 40).
FMMU_OUTPUTS = 0x01
FMMU_INPUTS = 0x02
FMMU_MAILBOX_STATE = 0x03

#: Sync manager types (ETG.2010 category 41).
SM_MAILBOX_OUT = 0x01
SM_MAILBOX_IN = 0x02
SM_OUTPUTS = 0x03
SM_INPUTS = 0x04


@dataclass(frozen=True)
class SyncManagerEntry:
    start: int
    length: int
    control: int
    kind: int


@dataclass(frozen=True)
class PdoEntry:
    index: int
    subindex: int
    bits: int
    data_type: int
    name: str


@dataclass(frozen=True)
class Pdo:
    index: int
    sync_manager: int
    name: str
    entries: tuple[PdoEntry, ...]

    @property
    def bits(self) -> int:
        return sum(entry.bits for entry in self.entries)


def checksum(header: bytes) -> int:
    """ETG.2010's header checksum: CRC-8, polynomial x^8+x^2+x+1, initial value 0xFF, over words 0-6.

    An ESC checks it when it loads the header at power-up and refuses the PDI configuration when it
    is wrong, so an image with a bad one is a slave that never comes up — which is why it is computed
    here rather than left zero.
    """
    crc = 0xFF
    for byte in header[:14]:
        crc ^= byte
        for _ in range(8):
            crc = ((crc << 1) ^ 0x07) & 0xFF if crc & 0x80 else (crc << 1) & 0xFF
    return crc


def build(
    *,
    vendor_id: int,
    product_code: int,
    revision: int,
    serial: int,
    name: str,
    group: str,
    sync_managers: tuple[SyncManagerEntry, ...],
    fmmus: tuple[int, ...],
    txpdo: Pdo,
    rxpdo: Pdo,
) -> bytes:
    """The whole image, as bytes. Word `n` is at byte `2n`, little-endian."""
    strings = [name, group]

    def string_index(text: str) -> int:
        if text not in strings:
            strings.append(text)
        return strings.index(text) + 1  # ETG.2010 string indices start at 1; 0 means "none"

    for pdo in (txpdo, rxpdo):
        string_index(pdo.name)
        for entry in pdo.entries:
            string_index(entry.name)

    mailbox_out = next(sm for sm in sync_managers if sm.kind == SM_MAILBOX_OUT)
    mailbox_in = next(sm for sm in sync_managers if sm.kind == SM_MAILBOX_IN)

    header = bytearray(CATEGORIES_AT * 2)
    # PDI control: an SPI slave interface, which is what a drive's microcontroller hangs off. The
    # value is this simulator's choice of device, and a master only reports it.
    struct.pack_into("<H", header, WORD_PDI_CONTROL * 2, 0x0005)
    struct.pack_into("<I", header, WORD_VENDOR_ID * 2, vendor_id)
    struct.pack_into("<I", header, WORD_PRODUCT_CODE * 2, product_code)
    struct.pack_into("<I", header, WORD_REVISION * 2, revision)
    struct.pack_into("<I", header, WORD_SERIAL * 2, serial)
    struct.pack_into("<HH", header, WORD_RX_MAILBOX * 2, mailbox_out.start, mailbox_out.length)
    struct.pack_into("<HH", header, WORD_TX_MAILBOX * 2, mailbox_in.start, mailbox_in.length)
    struct.pack_into("<H", header, WORD_MAILBOX_PROTOCOL * 2, MAILBOX_COE)
    struct.pack_into("<H", header, WORD_SIZE * 2, 0x0001)  # 2 kbit, the smallest a slave ships with
    struct.pack_into("<H", header, WORD_VERSION * 2, 0x0001)
    struct.pack_into("<H", header, WORD_CHECKSUM * 2, checksum(bytes(header)))

    body = bytearray()

    general = bytearray(32)
    general[0] = string_index(group)  # group
    general[3] = string_index(name)  # device name
    general[5] = COE_DETAILS_SDO
    # Byte 11 is the flags. Bit 1 would say "use LRD/LWR, not LRW"; it is clear, because this ESC
    # services a read-write datagram on both halves.
    body += _category(CATEGORY_GENERAL, bytes(general))

    body += _category(CATEGORY_FMMU, bytes(fmmus))

    managers = b"".join(
        struct.pack("<HHBBBB", sm.start, sm.length, sm.control, 0x00, 0x01, sm.kind)
        for sm in sync_managers
    )
    body += _category(CATEGORY_SYNC_MANAGER, managers)

    for category, pdo in ((CATEGORY_TXPDO, txpdo), (CATEGORY_RXPDO, rxpdo)):
        raw = struct.pack(
            "<HBBBBH", pdo.index, len(pdo.entries), pdo.sync_manager, 0, string_index(pdo.name), 0
        )
        for entry in pdo.entries:
            raw += struct.pack(
                "<HBBBBH",
                entry.index,
                entry.subindex,
                string_index(entry.name),
                entry.data_type,
                entry.bits,
                0,
            )
        body += _category(category, raw)

    # Placed first and built last: every other category refers to a string by its index, so the
    # table is only complete once they have all been written.
    encoded = bytearray([len(strings)])
    for text in strings:
        raw = text.encode("ascii")
        if len(raw) > 255:
            raise ValueError(f"an SII string is at most 255 bytes and {text!r} is {len(raw)}")
        encoded += bytes([len(raw)]) + raw
    image = bytes(header) + _category(CATEGORY_STRINGS, bytes(encoded)) + bytes(body)
    return image + struct.pack("<H", CATEGORY_END)


def _category(kind: int, data: bytes) -> bytes:
    """One category: its type, its length in words, and its data padded to a whole word."""
    if len(data) % 2:
        data += b"\x00"
    return struct.pack("<HH", kind, len(data) // 2) + data


def categories(image: bytes) -> dict[int, bytes]:
    """Walk an image's categories the way a master does, for a suite that wants to read one back."""
    found: dict[int, bytes] = {}
    at = CATEGORIES_AT * 2
    while at + 4 <= len(image):
        kind, words = struct.unpack_from("<HH", image, at)
        if kind == CATEGORY_END:
            break
        found[kind] = image[at + 4 : at + 4 + words * 2]
        at += 4 + words * 2
    return found
