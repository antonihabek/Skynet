"""The line to the hand: a serial port (the hand's RS-485 through its USB adapter) or a TCP socket (an RS-485 gateway that forwards the line raw). Stdlib only.

One transaction at a time: write a request, read until the answer that the request implies has arrived (a read of n bytes is answered with n + 8, a write with nine), with a byte timeout between bytes and the caller's
budget over the whole. Whatever arrives before the answer's `90 EB` header is skipped: stale input, or the echo of the request that a half-duplex adapter hears (its header is `EB 90`, the other way round). A late
answer to an earlier request is discarded before the next is written, because on a serial line it would otherwise be read as the answer to that one.
"""

from __future__ import annotations

import os
import select
import socket
import time

from . import frames

#: Between bytes of one answer. The maker gives no figure; a USB serial adapter's latency timer is commonly 16 ms, and the same bound Robotiq's own client uses for the same adapters is the nearest measured thing.
BYTE_TIMEOUT_S = 0.05
#: Bytes that are not the start of an answer, read before the line is called noise.
MAX_SKIPPED = 512


class LinkError(Exception):
    """The line did not carry a transaction: nothing opened, nothing answered, or an answer that stopped half way."""


class Link:
    def __init__(self, device: str | None = None, baud: int = 115200, host: str | None = None, port: int | None = None) -> None:
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
            # 8 data bits, 1 stop bit, no parity (series manual §2.2: "the communication parameters of RS485 are 8 data bits, 1 stop bit, and no parity"), raw, no flow control, no echo.
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

    def transact(self, request: bytes, hand: int, command: int, address: int, length: int, budget: float) -> bytes:
        """Send `request` and return the data of its answer (nothing, for a write), within `budget` seconds. Raises LinkError for a line that did not carry it and frames.ProtocolError for an answer that is wrong."""
        if not self.is_open:
            raise LinkError(f"{self.target} is not open.")
        wanted = length + 8 if command == frames.CMD_READ else 9
        deadline = time.monotonic() + budget
        self._discard()
        try:
            self._write(request, deadline)
            buffer = b""
            skipped = 0
            while True:
                now = time.monotonic()
                if now >= deadline:
                    start = frames.find_response(buffer)
                    if start is not None:
                        raise LinkError(f"{self.target} did not finish its answer in {budget * 1000:.0f} ms (it had sent {len(buffer) - start} of {wanted} bytes)")
                    raise LinkError(f"{self.target} did not answer in {budget * 1000:.0f} ms" + (f" (it sent {len(buffer)} bytes that are not an answer)" if buffer else ""))
                start = frames.find_response(buffer)
                chunk = self._read(min(deadline - now, BYTE_TIMEOUT_S if start is not None else deadline - now))
                if not chunk:
                    if start is not None:
                        raise LinkError(f"{self.target} stopped sending {len(buffer) - start} bytes into an answer of {wanted}.")
                    continue
                buffer += chunk
                start = frames.find_response(buffer)
                if start is None:
                    skipped += len(buffer) - 1
                    if skipped > MAX_SKIPPED:
                        raise LinkError(f"{self.target} sent {skipped} bytes and none of them is the start of an answer (90 EB): a line that is noise, or at the wrong baud rate.")
                    # Keep the last byte: it may be the 90 of a header whose EB is still on its way.
                    buffer = buffer[-1:]
                    continue
                if len(buffer) - start >= wanted:
                    frame = buffer[start:start + wanted]
                    break
        except OSError as error:
            # A line that failed (an adapter pulled out, a gateway that went away) is not coming back by waiting: it is closed, so that whoever drives it knows to open it again.
            self.close()
            raise LinkError(f"{self.target} failed: {error}") from error
        return frames.parse_response(frame, hand, command, address, length)
