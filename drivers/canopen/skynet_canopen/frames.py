"""CANopen on the wire: COB-IDs, expedited SDO, and the PDO layouts this driver uses.

Pure. Nothing here opens a socket, reads a clock or holds a drive — every function is bytes in,
bytes out — which is what lets the whole protocol be exercised by :mod:`skynet_canopen.selftest`
with no bus of any kind, and what keeps :mod:`skynet_canopen.slave` and :mod:`skynet_canopen.bridge`
from each growing their own idea of a frame.

**Written out rather than imported.** `python-can` and `canopen` are both excellent and this driver
uses neither, for the reason `CONTRIBUTING.md` gives about the Rust workspace and `drivers/lerobot`
demonstrates on the Python side: a driver author should be able to clone this repository and reach a
passing conformance run with nothing installed. Expedited SDO is eight bytes with four fields in it,
and a dependency for that is a dependency a vendor's build system has to survive for ten years.

# The identifiers

CANopen's default allocation (CiA 301) gives every service a base identifier and adds the node id:

| Service | COB-ID | Direction |
|---|---|---|
| NMT | `0x000` | master → all |
| SYNC | `0x080` | master → all |
| EMCY | `0x080 + node` | drive → master |
| TPDO1 / TPDO2 | `0x180 + node` / `0x280 + node` | drive → master |
| RPDO1 / RPDO2 | `0x200 + node` / `0x300 + node` | master → drive |
| SDO response | `0x580 + node` | drive → master |
| SDO request | `0x600 + node` | master → drive |
| Heartbeat | `0x700 + node` | drive → master |

SYNC and EMCY share `0x080` and do not collide, because node 0 is not a node: EMCY from node 1 is
`0x081`. That is CiA 301's arrangement rather than this file's, and it is the sort of thing worth
stating once where a reader can check it.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

__all__ = [
    "Frame",
    "NMT_ID",
    "SYNC_ID",
    "emcy_id",
    "tpdo1_id",
    "tpdo2_id",
    "rpdo1_id",
    "rpdo2_id",
    "sdo_request_id",
    "sdo_response_id",
    "heartbeat_id",
    "NMT_START",
    "NMT_STOP",
    "NMT_PRE_OPERATIONAL",
    "NMT_RESET_NODE",
    "NMT_RESET_COMMUNICATION",
    "HEARTBEAT_BOOT",
    "HEARTBEAT_STOPPED",
    "HEARTBEAT_OPERATIONAL",
    "HEARTBEAT_PRE_OPERATIONAL",
    "SdoAbort",
    "ABORT_NO_SUCH_OBJECT",
    "ABORT_NO_SUCH_SUBINDEX",
    "ABORT_READ_ONLY",
    "ABORT_LENGTH",
    "sdo_upload_request",
    "sdo_download_request",
    "parse_sdo_request",
    "sdo_upload_response",
    "sdo_download_response",
    "sdo_abort_response",
    "parse_sdo_response",
    "rpdo2",
    "parse_rpdo2",
    "tpdo2",
    "parse_tpdo2",
]

NMT_ID = 0x000
SYNC_ID = 0x080

NMT_START = 0x01
NMT_STOP = 0x02
NMT_PRE_OPERATIONAL = 0x80
NMT_RESET_NODE = 0x81
NMT_RESET_COMMUNICATION = 0x82

HEARTBEAT_BOOT = 0x00
HEARTBEAT_STOPPED = 0x04
HEARTBEAT_OPERATIONAL = 0x05
HEARTBEAT_PRE_OPERATIONAL = 0x7F


def emcy_id(node: int) -> int:
    return 0x080 + node


def tpdo1_id(node: int) -> int:
    return 0x180 + node


def rpdo1_id(node: int) -> int:
    return 0x200 + node


def tpdo2_id(node: int) -> int:
    return 0x280 + node


def rpdo2_id(node: int) -> int:
    return 0x300 + node


def sdo_response_id(node: int) -> int:
    return 0x580 + node


def sdo_request_id(node: int) -> int:
    return 0x600 + node


def heartbeat_id(node: int) -> int:
    return 0x700 + node


@dataclass(frozen=True)
class Frame:
    """One classic CAN frame.

    ``data`` is never padded here. CAN carries a length and a drive that answers a two-byte object
    with two bytes is answering correctly; padding to eight in this layer would make a short reply
    indistinguishable from a long one with zeros on the end, which is exactly the distinction an
    expedited SDO's command byte exists to carry.
    """

    arbitration_id: int
    data: bytes

    def __post_init__(self) -> None:
        if not 0 <= self.arbitration_id <= 0x7FF:
            raise ValueError(f"{self.arbitration_id:#x} is not an 11-bit identifier")
        if len(self.data) > 8:
            raise ValueError("a classic CAN frame carries at most 8 bytes")


# ── Expedited SDO ────────────────────────────────────────────────────────────
#
# Byte 0 is the command specifier, bytes 1-2 the index little-endian, byte 3 the subindex, bytes
# 4-7 the value little-endian. Expedited means the value fits in those four bytes, which every
# object this driver touches does; segmented transfer is for strings and blocks and is deliberately
# not implemented — a driver that needed it would be reading a device name it does not use.

_DOWNLOAD_BY_SIZE = {1: 0x2F, 2: 0x2B, 3: 0x27, 4: 0x23}
_UPLOAD_BY_SIZE = {1: 0x4F, 2: 0x4B, 3: 0x47, 4: 0x43}
_SIZE_BY_UPLOAD = {value: size for size, value in _UPLOAD_BY_SIZE.items()}
_SIZE_BY_DOWNLOAD = {value: size for size, value in _DOWNLOAD_BY_SIZE.items()}

UPLOAD_REQUEST = 0x40
DOWNLOAD_RESPONSE = 0x60
ABORT = 0x80

ABORT_NO_SUCH_OBJECT = 0x06020000
ABORT_NO_SUCH_SUBINDEX = 0x06090011
ABORT_READ_ONLY = 0x06010002
ABORT_LENGTH = 0x06070010


class SdoAbort(Exception):
    """A drive refused an object access, with CiA 301's own code.

    Carried as an exception rather than a return value because every caller in this driver wants the
    same thing: stop, and report which object and which code. A code nobody decodes is a support
    call, so :meth:`explain` turns the four this driver can provoke into sentences.
    """

    def __init__(self, index: int, subindex: int, code: int) -> None:
        super().__init__(f"SDO abort {code:#010x} on {index:#06x}:{subindex:02x}")
        self.index = index
        self.subindex = subindex
        self.code = code

    def explain(self) -> str:
        known = {
            ABORT_NO_SUCH_OBJECT: "the drive has no such object",
            ABORT_NO_SUCH_SUBINDEX: "the object exists and has no such subindex",
            ABORT_READ_ONLY: "the object is read-only",
            ABORT_LENGTH: "the value's length does not match the object's",
        }
        reason = known.get(self.code, "the drive did not say which of CiA 301's reasons applies")
        return (
            f"{self.index:#06x}:{self.subindex:02x} was refused with {self.code:#010x} — {reason}."
        )


def sdo_upload_request(node: int, index: int, subindex: int) -> Frame:
    """Read an object."""
    return Frame(sdo_request_id(node), struct.pack("<BHB4x", UPLOAD_REQUEST, index, subindex))


def sdo_download_request(node: int, index: int, subindex: int, value: int, size: int) -> Frame:
    """Write an object, with ``size`` in bytes and ``value`` two's-complement if negative."""
    if size not in _DOWNLOAD_BY_SIZE:
        raise ValueError(f"expedited SDO carries 1 to 4 bytes, not {size}")
    raw = int(value) & ((1 << (8 * size)) - 1)
    payload = raw.to_bytes(size, "little") + bytes(4 - size)
    return Frame(
        sdo_request_id(node),
        struct.pack("<BHB", _DOWNLOAD_BY_SIZE[size], index, subindex) + payload,
    )


def parse_sdo_request(data: bytes) -> tuple[str, int, int, int | None, int]:
    """``(kind, index, subindex, value, size)`` where kind is ``upload`` or ``download``."""
    if len(data) != 8:
        raise ValueError("an SDO frame is eight bytes")
    command, index, subindex = struct.unpack("<BHB", data[:4])
    if command == UPLOAD_REQUEST:
        return ("upload", index, subindex, None, 0)
    if command in _SIZE_BY_DOWNLOAD:
        size = _SIZE_BY_DOWNLOAD[command]
        return ("download", index, subindex, int.from_bytes(data[4 : 4 + size], "little"), size)
    raise ValueError(f"command specifier {command:#04x} is not an expedited SDO request")


def sdo_upload_response(node: int, index: int, subindex: int, value: int, size: int) -> Frame:
    if size not in _UPLOAD_BY_SIZE:
        raise ValueError(f"expedited SDO carries 1 to 4 bytes, not {size}")
    raw = int(value) & ((1 << (8 * size)) - 1)
    payload = raw.to_bytes(size, "little") + bytes(4 - size)
    return Frame(
        sdo_response_id(node),
        struct.pack("<BHB", _UPLOAD_BY_SIZE[size], index, subindex) + payload,
    )


def sdo_download_response(node: int, index: int, subindex: int) -> Frame:
    return Frame(sdo_response_id(node), struct.pack("<BHB4x", DOWNLOAD_RESPONSE, index, subindex))


def sdo_abort_response(node: int, index: int, subindex: int, code: int) -> Frame:
    return Frame(
        sdo_response_id(node), struct.pack("<BHBI", ABORT, index, subindex, code)
    )


def parse_sdo_response(data: bytes, index: int, subindex: int) -> int | None:
    """The value a drive returned, ``None`` for a download acknowledgement.

    Raises :class:`SdoAbort` on an abort, and ``ValueError`` when the reply names a *different*
    object from the one asked about. That last check is not pedantry: an SDO client that accepted
    any reply on the response identifier would, on a bus where a previous transfer timed out, read
    the stale answer to the previous question as the answer to this one — and on this driver that
    number becomes an encoder resolution.
    """
    if len(data) != 8:
        raise ValueError("an SDO frame is eight bytes")
    command, got_index, got_sub = struct.unpack("<BHB", data[:4])
    if command == ABORT:
        raise SdoAbort(got_index, got_sub, struct.unpack("<I", data[4:])[0])
    if (got_index, got_sub) != (index, subindex):
        raise ValueError(
            f"asked about {index:#06x}:{subindex:02x} and the drive answered about "
            f"{got_index:#06x}:{got_sub:02x}"
        )
    if command == DOWNLOAD_RESPONSE:
        return None
    if command in _SIZE_BY_UPLOAD:
        size = _SIZE_BY_UPLOAD[command]
        return int.from_bytes(data[4 : 4 + size], "little")
    raise ValueError(f"command specifier {command:#04x} is not an expedited SDO response")


# ── PDO ──────────────────────────────────────────────────────────────────────
#
# The layouts below are the **default mapping** CiA 402 devices ship with, and this driver asserts
# them rather than configuring them:
#
#   RPDO2 (0x300 + node)   controlword u16, target i32
#   TPDO2 (0x280 + node)   statusword u16, position i32
#
# A real integration writes 0x1600/0x1A00 over SDO to map whichever objects it wants, and a drive
# whose mapping differs from the above will read a target as something else entirely. So the bridge
# **verifies the mapping by SDO before it commands anything**, and refuses a drive whose mapping it
# did not recognise — see `bridge.py`. Assuming the default is the industry's most common source of
# a joint that moves to a number nobody sent.


def rpdo2(node: int, controlword: int, target: int) -> Frame:
    return Frame(rpdo2_id(node), struct.pack("<Hi", controlword & 0xFFFF, int(target)))


def parse_rpdo2(data: bytes) -> tuple[int, int]:
    if len(data) != 6:
        raise ValueError("RPDO2 under the default mapping is six bytes")
    controlword, target = struct.unpack("<Hi", data)
    return controlword, target


def tpdo2(node: int, statusword: int, position: int) -> Frame:
    return Frame(tpdo2_id(node), struct.pack("<Hi", statusword & 0xFFFF, int(position)))


def parse_tpdo2(data: bytes) -> tuple[int, int]:
    if len(data) != 6:
        raise ValueError("TPDO2 under the default mapping is six bytes")
    statusword, position = struct.unpack("<Hi", data)
    return statusword, position
