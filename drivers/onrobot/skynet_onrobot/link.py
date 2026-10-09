"""The line to the gripper: Modbus TCP (the Compute Box) or Modbus RTU (the tool's RS-485 behind a USB adapter, 8E1 at 115200 or 1000000 baud). Standard library only.

One transaction at a time: write a request, read until the answer the request implies has arrived, with a byte timeout between the bytes of one answer and the caller's budget over the whole. On TCP an answer
whose transaction id is not the request's is a late answer to an earlier one and is discarded; on a serial line, where an answer has no id, whatever is waiting is discarded before the request is written, because a late
answer would otherwise be read as the answer to this one.
"""

from __future__ import annotations

import errno
import os
import select
import socket
import time

from . import modbus

#: Between bytes of one RTU answer. The maker's library takes its own response and byte timeouts from the caller's `Timeout` (1000 ms each by default); a USB serial adapter's latency timer is commonly 16 ms.
BYTE_TIMEOUT_S = 0.05
BAUDS = (115200, 1000000)               # modbus_config.hpp: ModbusBaudRate


class LinkError(Exception):
    """The line did not carry a transaction: nothing opened, nothing answered, or an answer that stopped half way."""


class Link:
    def __init__(self, host: str | None = None, port: int = 502, device: str | None = None, baud: int = 115200) -> None:
        self._host, self._port, self._device, self._baud = host, port, device, baud
        self._sock: socket.socket | None = None
        self._fd: int | None = None
        self._transaction = 0
        self._rx = b""

    @property
    def target(self) -> str:
        return self._device if self._device else f"{self._host}:{self._port}"

    @property
    def is_tcp(self) -> bool:
        return self._device is None

    def open(self, timeout: float = 1.0) -> None:
        self.close()
        try:
            if self._device:
                self._open_serial()
            else:
                sock = socket.create_connection((self._host, self._port), timeout=timeout)
                sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                self._sock = sock
        except (OSError, ValueError) as error:
            raise LinkError(f"{self.target} could not be opened: {error}") from error

    def _open_serial(self) -> None:
        import termios
        fd = os.open(self._device, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
        try:
            attributes = termios.tcgetattr(fd)
            speed = getattr(termios, f"B{self._baud}", None)
            if speed is None:
                raise ValueError(f"{self._baud} baud is not a speed the line can be set to")
            # 8 data bits, even parity, one stop bit: the maker's `ModbusRtuParity::Even`, "OnRobot tool buses use 8 data bits, even parity, and one stop bit" (modbus_config.hpp). Raw, no flow control.
            attributes[0] = termios.INPCK
            attributes[1] = 0
            attributes[2] = termios.CS8 | termios.PARENB | termios.CREAD | termios.CLOCAL
            attributes[3] = 0
            attributes[4] = attributes[5] = speed
            attributes[6][termios.VMIN] = 0
            attributes[6][termios.VTIME] = 0
            termios.tcsetattr(fd, termios.TCSANOW, attributes)
        except (OSError, ValueError, termios.error) as error:
            if isinstance(error, termios.error) and error.args[:1] == (errno.EINVAL,) and self._is_pty(fd) and self._pty_took_the_rest(fd, termios):
                # A pseudo-terminal cannot carry parity, and the C library reports a parity request it did not get as EINVAL after the ioctl has applied everything else: the first open of a pty gets away with it and
                # every later one meets it, so a driver restarted on a `socat` gateway's pty (or run twice in a test) could not open its own line. There is no parity bit on a pty to get wrong (F302).
                self._fd = fd
                return
            os.close(fd)
            if isinstance(error, termios.error):
                # `termios.error` is an Exception and not an OSError: let go as it is, it passes `open`'s handler and the driver's own, and the host's `open` raises instead of returning a code (F301).
                raise OSError(*error.args) from error
            raise
        self._fd = fd

    @staticmethod
    def _is_pty(fd: int) -> bool:
        try:
            return os.path.realpath(os.ttyname(fd)).startswith("/dev/pts/")
        except OSError:
            return False

    def _pty_took_the_rest(self, fd: int, termios) -> bool:
        """After a refused parity request: eight data bits and the speed asked for are what the line holds."""
        try:
            attributes = termios.tcgetattr(fd)
        except (OSError, termios.error):
            return False
        return attributes[2] & termios.CSIZE == termios.CS8 and attributes[4] == attributes[5] == getattr(termios, f"B{self._baud}", None)

    def close(self) -> None:
        self._rx = b""
        if self._fd is not None:
            try:
                os.close(self._fd)
            except OSError:
                pass
            self._fd = None
        if self._sock is not None:
            try:
                self._sock.close()
            except OSError:
                pass
            self._sock = None

    @property
    def is_open(self) -> bool:
        return self._fd is not None or self._sock is not None

    def _discard(self) -> None:
        self._rx = b""
        if self._fd is not None:
            import termios
            try:
                termios.tcflush(self._fd, termios.TCIFLUSH)
            except (OSError, termios.error):
                pass

    def _write(self, data: bytes, deadline: float) -> None:
        view = memoryview(data)
        while view:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise LinkError(f"{self.target} did not take the request in time.")
            if self._fd is not None:
                _, writable, _ = select.select([], [self._fd], [], remaining)
                if not writable:
                    continue
                try:
                    sent = os.write(self._fd, view)
                except BlockingIOError:
                    continue
            else:
                assert self._sock is not None
                self._sock.settimeout(remaining)
                try:
                    sent = self._sock.send(view)
                except (socket.timeout, BlockingIOError):
                    continue
            view = view[sent:]

    def _read(self, wait: float) -> bytes:
        """Whatever has arrived, waiting at most `wait` for the first of it. Empty on a timeout."""
        if wait <= 0:
            return b""
        handle = self._fd if self._fd is not None else self._sock
        readable, _, _ = select.select([handle], [], [], wait)
        if not readable:
            return b""
        if self._fd is not None:
            try:
                return os.read(self._fd, 256)
            except BlockingIOError:
                return b""
        assert self._sock is not None
        self._sock.settimeout(0)
        try:
            data = self._sock.recv(256)
        except BlockingIOError:
            return b""
        if not data:
            self.close()
            raise LinkError(f"{self.target} closed the connection.")
        return data

    def _read_exactly(self, count: int, deadline: float, what: str) -> bytes:
        """`count` bytes: first what an earlier read left over, then the line. Bytes beyond them stay for the next frame."""
        started = bool(self._rx)
        while len(self._rx) < count:
            now = time.monotonic()
            if now >= deadline:
                raise LinkError(f"{self.target} did not finish {what} in time (it had sent {len(self._rx)} of {count} bytes).")
            chunk = self._read(min(deadline - now, BYTE_TIMEOUT_S if started else deadline - now))
            if not chunk:
                if started:
                    have = len(self._rx)
                    self._rx = b""
                    raise LinkError(f"{self.target} stopped sending {have} bytes into {what} of {count}.")
                continue
            started = True
            self._rx += chunk
        frame, self._rx = self._rx[:count], self._rx[count:]
        return frame

    def transact(self, slave: int, pdu: bytes, budget: float) -> list[int]:
        """Send `pdu` to `slave` and return what it answers (the registers of a read, nothing for a write) within `budget` seconds. LinkError for a line that did not carry it, modbus.ProtocolError for an answer that is wrong."""
        if not self.is_open:
            raise LinkError(f"{self.target} is not open.")
        deadline = time.monotonic() + budget
        try:
            if self.is_tcp:
                return modbus.parse_pdu(pdu, self._transact_tcp(slave, pdu, deadline))
            return modbus.parse_pdu(pdu, self._transact_rtu(slave, pdu, deadline))
        except OSError as error:
            # A line that failed (an adapter pulled out, a gateway that went away) is not coming back by waiting: it is closed, so that whoever drives it knows to open it again.
            self.close()
            raise LinkError(f"{self.target} failed: {error}") from error

    def _transact_tcp(self, slave: int, pdu: bytes, deadline: float) -> bytes:
        self._transaction = (self._transaction + 1) & 0xFFFF
        mine = self._transaction
        self._write(modbus.tcp_frame(mine, slave, pdu), deadline)
        while True:
            header = self._read_exactly(modbus.MBAP_BYTES, deadline, "an answer's header")
            transaction, length, unit = modbus.tcp_split(header)
            body = self._read_exactly(length, deadline, "an answer")
            if transaction != mine:
                continue                    # a late answer to an earlier request
            if unit != slave:
                raise modbus.ProtocolError(f"the answer is from unit {unit} and the request was for {slave}.")
            return body

    def _transact_rtu(self, slave: int, pdu: bytes, deadline: float) -> bytes:
        self._discard()
        self._write(modbus.rtu_frame(slave, pdu), deadline)
        head = self._read_exactly(2, deadline, "an answer's address and function")
        if head[1] & 0x80:
            total = 5
        else:
            total = modbus.expected_pdu_length(pdu) + 3
        tail = self._read_exactly(total - 2, deadline, "an answer") if total > 2 else b""
        return modbus.rtu_check(head + tail, slave)
