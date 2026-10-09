"""The line to the hand's servos: a serial port (a USB adapter on the servos' TTL or RS-485 bus) or a TCP socket (a gateway that forwards the line raw). Stdlib only.

One transaction at a time: write a request, then read the status packets it implies (one per servo for a read or a ping, none for a write or a synchronised write) until they have all arrived, with a byte timeout
between the bytes of a packet and the caller's budget over the whole. Whatever arrives before a packet's `FF FF FD 00` header is skipped (stale input, or the echo a half-duplex adapter hears of the request). Input
that arrived before the request is discarded first, because on a serial line a late answer to an earlier request would otherwise be read as this one's.
"""

from __future__ import annotations

import os
import select
import socket
import time

from . import frames

#: Between bytes of one status packet. ROBOTIS gives none; a USB serial adapter's latency timer is commonly 16 ms (the maker's readme asks for 1 ms), and this is this driver's own figure.
BYTE_TIMEOUT_S = 0.05
#: Bytes that are not the start of a packet, read before the line is called noise.
MAX_SKIPPED = 2048
#: Baud rates the line can be set to (the servos' own codes are 9600, 57600, 115200, 1M, 2M, 3M and 4M; the maker runs the hand at 4M).
BAUDS = (9600, 57600, 115200, 1000000, 2000000, 3000000, 4000000)


class LinkError(Exception):
    """The line did not carry a transaction: nothing opened, a servo that did not answer, or a packet that stopped half way."""


class Link:
    def __init__(self, device: str | None = None, baud: int = 4000000, host: str | None = None, port: int | None = None) -> None:
        self._device, self._baud, self._host, self._port = device, baud, host, port
        self._fd: int | None = None
        self._sock: socket.socket | None = None

    @property
    def target(self) -> str:
        return self._device if self._device else f"{self._host}:{self._port}"

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
            # 8 data bits, 1 stop bit, no parity (e-manual, protocol 2.0), raw, no flow control, no echo.
            attributes[0] = 0
            attributes[1] = 0
            attributes[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
            attributes[3] = 0
            attributes[4] = attributes[5] = speed
            attributes[6][termios.VMIN] = 0
            attributes[6][termios.VTIME] = 0
            termios.tcsetattr(fd, termios.TCSANOW, attributes)
        except (OSError, ValueError, termios.error) as error:
            os.close(fd)
            if isinstance(error, termios.error):
                # `termios.error` is an Exception and not an OSError: let go as it is, it passes `open`'s handler and the driver's own, and the host's `open` raises instead of returning a code (F301).
                raise OSError(*error.args) from error
            raise
        self._fd = fd

    def close(self) -> None:
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
        if self._fd is not None:
            import termios
            try:
                termios.tcflush(self._fd, termios.TCIFLUSH)
            except (OSError, termios.error):
                pass
        elif self._sock is not None:
            self._sock.setblocking(False)
            try:
                while self._sock.recv(4096):
                    pass
            except (BlockingIOError, OSError):
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
                return os.read(self._fd, 1024)
            except BlockingIOError:
                return b""
        assert self._sock is not None
        self._sock.settimeout(0)
        try:
            data = self._sock.recv(1024)
        except BlockingIOError:
            return b""
        if not data:
            self.close()
            raise LinkError(f"{self.target} closed the connection.")
        return data

    def transact(self, request: bytes, ids: tuple[int, ...] | list[int], budget: float) -> dict[int, frames.StatusPacket]:
        """Send `request` and return the status packets of `ids` (none, for a write that is not answered), within `budget` seconds. Raises LinkError for a line that did not carry it, or a servo that did not
        answer, naming which; frames.ProtocolError for an answer that is wrong (a CRC, a servo that was not asked, a servo that answered twice)."""
        if not self.is_open:
            raise LinkError(f"{self.target} is not open.")
        deadline = time.monotonic() + budget
        self._discard()
        got: dict[int, frames.StatusPacket] = {}
        wanted = list(ids)
        try:
            self._write(request, deadline)
            buffer = b""
            skipped = 0
            while len(got) < len(wanted):
                now = time.monotonic()
                if now >= deadline:
                    break
                started = frames.find_status(buffer) is not None
                chunk = self._read(min(deadline - now, BYTE_TIMEOUT_S if started else deadline - now))
                if not chunk:
                    if started:
                        missing = [servo for servo in wanted if servo not in got]
                        buffer = b""
                        raise LinkError(f"{self.target} stopped sending in the middle of a packet; servos {missing} are missing.")
                    continue
                buffer += chunk
                while True:
                    start = frames.find_status(buffer)
                    if start is None:
                        skipped += max(0, len(buffer) - 3)
                        if skipped > MAX_SKIPPED:
                            raise LinkError(f"{self.target} sent {skipped} bytes and none of them is the start of a packet (FF FF FD 00): a line that is noise, or at the wrong baud rate.")
                        buffer = buffer[-3:]          # the first bytes of a header whose end is still on its way
                        break
                    skipped += start
                    buffer = buffer[start:]
                    parsed = frames.parse_status(buffer)
                    if parsed is None:
                        break
                    status, end = parsed
                    buffer = buffer[end:]
                    if status.servo not in wanted:
                        raise frames.ProtocolError(f"servo {status.servo} answered, and the request asked {wanted}.")
                    if status.servo in got:
                        raise frames.ProtocolError(f"servo {status.servo} answered twice.")
                    got[status.servo] = status
        except OSError as error:
            # A line that failed (an adapter pulled out, a gateway that went away) is not coming back by waiting: it is closed, so that whoever drives it knows to open it again.
            self.close()
            raise LinkError(f"{self.target} failed: {error}") from error
        if len(got) < len(wanted):
            missing = [servo for servo in wanted if servo not in got]
            raise LinkError(f"{self.target}: servos {missing} did not answer in {budget * 1000:.0f} ms ({len(got)} of {len(wanted)} did).")
        return got
