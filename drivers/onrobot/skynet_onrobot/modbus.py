"""The Modbus frames OnRobot's tool bus carries, over TCP (the Compute Box, port 502) or RTU (the tool's own RS-485 behind a USB adapter, 8E1 at 115200 or 1000000 baud). Pure: no I/O, no clock, standard library only.

OnRobot's own client (`libonrobot_tool_api`, BSD-3-Clause, the maker's release 1.2.0) uses libmodbus and three functions: 0x03 read holding registers, 0x06 write single register and 0x10 write multiple registers, and 0x17
write and read multiple registers for its realtime exchange. This driver speaks the first three. The slave address is the mounting-dependent one the maker's `ModbusSlaveId` names: 65 (0x41, the single tool or the
Quick Changer's only), 66 (primary) and 67 (secondary). On TCP the unit identifier carries it.
"""

from __future__ import annotations

import struct

FC_READ_HOLDING = 0x03
FC_WRITE_SINGLE = 0x06
FC_WRITE_MULTIPLE = 0x10

#: The most registers one read or write may carry (the Modbus specification: 125 to read, 123 to write).
MAX_READ_REGISTERS = 125
MAX_WRITE_REGISTERS = 123

MBAP_BYTES = 7                      # transaction id (2), protocol id (2), length (2), unit id (1)
SLAVES = (65, 66, 67)               # ModbusSlaveId::Single, Primary, Secondary (modbus_config.hpp)

EXCEPTIONS = {1: "illegal function", 2: "illegal data address", 3: "illegal data value", 4: "server device failure", 5: "acknowledge", 6: "server device busy", 8: "memory parity error",
              10: "gateway path unavailable", 11: "gateway target device failed to respond"}


class ProtocolError(Exception):
    """A frame that is not one Modbus allows, or an answer that is not the one the request asked for."""


def _slave(slave: int) -> int:
    if isinstance(slave, bool) or not isinstance(slave, int) or not 1 <= slave <= 247:
        raise ProtocolError(f"slave address {slave!r} is not 1 to 247.")
    return slave


def _address(address: int) -> int:
    if isinstance(address, bool) or not isinstance(address, int) or not 0 <= address <= 0xFFFF:
        raise ProtocolError(f"register address {address!r} is not 0 to 65535.")
    return address


def _registers(values: list[int] | tuple[int, ...]) -> bytes:
    for value in values:
        if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value <= 0xFFFF:
            raise ProtocolError(f"{value!r} does not fit a sixteen-bit register (0 to 65535).")
    return struct.pack(f">{len(values)}H", *values)


# ── the PDU: function, address, count or values ─────────────────────────────────────────────

def pdu_read(address: int, count: int) -> bytes:
    _address(address)
    if isinstance(count, bool) or not isinstance(count, int) or not 1 <= count <= MAX_READ_REGISTERS:
        raise ProtocolError(f"a read of {count!r} registers is not 1 to {MAX_READ_REGISTERS}.")
    if address + count > 0x10000:
        raise ProtocolError(f"a read of {count} registers from {address} runs past register 65535.")
    return struct.pack(">BHH", FC_READ_HOLDING, address, count)


def pdu_write_single(address: int, value: int) -> bytes:
    _address(address)
    return struct.pack(">BH", FC_WRITE_SINGLE, address) + _registers([value])


def pdu_write_multiple(address: int, values: list[int] | tuple[int, ...]) -> bytes:
    _address(address)
    if not isinstance(values, (list, tuple)) or not 1 <= len(values) <= MAX_WRITE_REGISTERS:
        raise ProtocolError(f"a write of {len(values) if isinstance(values, (list, tuple)) else values!r} registers is not 1 to {MAX_WRITE_REGISTERS}.")
    if address + len(values) > 0x10000:
        raise ProtocolError(f"a write of {len(values)} registers from {address} runs past register 65535.")
    return struct.pack(">BHHB", FC_WRITE_MULTIPLE, address, len(values), 2 * len(values)) + _registers(values)


def expected_pdu_length(request_pdu: bytes) -> int:
    """How long the answer to a request is, in PDU bytes (the exception answer is two and is told apart by its function's high bit)."""
    function = request_pdu[0]
    if function == FC_READ_HOLDING:
        return 2 + 2 * struct.unpack(">H", request_pdu[3:5])[0]
    return 5                                # a write echoes its function, address and count (or value)


def parse_pdu(request_pdu: bytes, answer_pdu: bytes) -> list[int]:
    """The registers a read answers with, or an empty list for a write; ProtocolError for an exception answer or an answer that is not the request's."""
    if not answer_pdu:
        raise ProtocolError("the answer is empty.")
    function = request_pdu[0]
    if answer_pdu[0] == function | 0x80:
        if len(answer_pdu) != 2:
            raise ProtocolError(f"an exception answer is two bytes, not {len(answer_pdu)}.")
        code = answer_pdu[1]
        raise ProtocolError(f"the device answered function 0x{function:02X} with exception {code} ({EXCEPTIONS.get(code, 'unknown')}).")
    if answer_pdu[0] != function:
        raise ProtocolError(f"the answer is for function 0x{answer_pdu[0]:02X} and the request was 0x{function:02X}.")
    if function == FC_READ_HOLDING:
        count = struct.unpack(">H", request_pdu[3:5])[0]
        if len(answer_pdu) != 2 + 2 * count or answer_pdu[1] != 2 * count:
            raise ProtocolError(f"a read of {count} registers is answered with {2 * count} data bytes after a byte count of {2 * count}, not {len(answer_pdu) - 2} bytes with a count of {answer_pdu[1] if len(answer_pdu) > 1 else '-'}.")
        return list(struct.unpack(f">{count}H", answer_pdu[2:]))
    if len(answer_pdu) != 5:
        raise ProtocolError(f"a write is answered with five bytes, not {len(answer_pdu)}.")
    if answer_pdu[1:] != request_pdu[1:5]:
        raise ProtocolError("the answer to a write does not echo its address and value (or count).")
    return []


# ── TCP: the MBAP header ────────────────────────────────────────────────────────────────────

def tcp_frame(transaction: int, slave: int, pdu: bytes) -> bytes:
    _slave(slave)
    if isinstance(transaction, bool) or not isinstance(transaction, int) or not 0 <= transaction <= 0xFFFF:
        raise ProtocolError(f"transaction id {transaction!r} is not 0 to 65535.")
    return struct.pack(">HHHB", transaction, 0, len(pdu) + 1, slave) + pdu


def tcp_split(header: bytes) -> tuple[int, int, int]:
    """(transaction, length of what follows the header, unit) from the seven header bytes."""
    if len(header) != MBAP_BYTES:
        raise ProtocolError(f"an MBAP header is {MBAP_BYTES} bytes, not {len(header)}.")
    transaction, protocol, length, unit = struct.unpack(">HHHB", header)
    if protocol != 0:
        raise ProtocolError(f"the protocol id is {protocol}, and Modbus is 0.")
    if not 2 <= length <= 254:
        raise ProtocolError(f"the header's length of {length} is not a Modbus frame's (2 to 254).")
    return transaction, length - 1, unit


# ── RTU: the slave address and the CRC ──────────────────────────────────────────────────────

def crc16(data: bytes) -> int:
    crc = 0xFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ 0xA001 if crc & 1 else crc >> 1
    return crc


def rtu_frame(slave: int, pdu: bytes) -> bytes:
    body = bytes([_slave(slave)]) + pdu
    return body + struct.pack("<H", crc16(body))


def rtu_check(frame: bytes, slave: int) -> bytes:
    """The PDU of an answer after its slave address and CRC have been checked."""
    if len(frame) < 5:
        raise ProtocolError(f"an RTU answer is at least five bytes, not {len(frame)}.")
    if crc16(frame[:-2]) != struct.unpack("<H", frame[-2:])[0]:
        raise ProtocolError("the answer's CRC is wrong.")
    if frame[0] != slave:
        raise ProtocolError(f"the answer is from slave {frame[0]} and the request was for {slave}.")
    return bytes(frame[1:-2])
