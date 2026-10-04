"""The line to the gripper: a serial port (the gripper's RS-485 through its USB adapter) or a TCP socket (an RS-485 gateway, or a robot's tool-communication port that forwards the line raw). Stdlib only.

One transaction at a time: write a request, read until the answer is as long as its header says, with the SDK's byte timeout (`src/modbus_client.cpp`: 50 ms, "the FTDI latency timer is 16 ms by default on many platforms")
between bytes and the caller's budget over the whole. A late answer to an earlier request is discarded before the next is written, because on a serial line it would otherwise be read as the answer to that one.
"""

from __future__ import annotations

import os
import select
import socket
import time

from . import frames

BYTE_TIMEOUT_S = 0.05


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
            # Raw 8N1, no flow control, no echo: the gripper's line as the maker's client opens it.
            attributes[0] = 0                                                   # iflag
            attributes[1] = 0                                                   # oflag
            attributes[2] = termios.CS8 | termios.CREAD | termios.CLOCAL       # cflag
            attributes[3] = 0                                                   # lflag
            attributes[4] = attributes[5] = speed
            attributes[6][termios.VMIN] = 0
            attributes[6][termios.VTIME] = 0
            termios.tcsetattr(fd, termios.TCSANOW, attributes)
        except (OSError, ValueError, termios.error):
            os.close(fd)
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

    def transact(self, request: bytes, function: int, slave: int, budget: float) -> bytes:
        """Send `request` and return the data of its answer, within `budget` seconds. Raises LinkError for a line that did not carry it and frames.ProtocolError for an answer that is wrong."""
        if not self.is_open:
            raise LinkError(f"{self.target} is not open.")
        began = time.monotonic()
        deadline = began + budget
        self._discard()
        try:
            self._write(request, deadline)
            answer = b""
            wanted: int | None = None
            while True:
                now = time.monotonic()
                if now >= deadline:
                    raise LinkError(f"{self.target} did not answer in {budget * 1000:.0f} ms" + (f" (it had sent {len(answer)} of {wanted} bytes)" if answer and wanted else ""))
                chunk = self._read(min(deadline - now, BYTE_TIMEOUT_S if answer else deadline - now))
                if not chunk:
                    if answer:
                        raise LinkError(f"{self.target} stopped sending {len(answer)} bytes into an answer of {wanted or 'an unknown number of'} bytes.")
                    continue
                answer += chunk
                if wanted is None:
                    wanted = frames.response_length(answer)
                if wanted is not None and len(answer) >= wanted:
                    break
        except OSError as error:
            # A line that failed (an adapter pulled out, a gateway that went away) is not coming back by waiting: it is closed, so that whoever drives it knows to open it again.
            self.close()
            raise LinkError(f"{self.target} failed: {error}") from error
        except frames.ProtocolError as error:
            raise LinkError(f"{self.target} sent something that is not an answer: {error}") from error
        return frames.parse_response(answer[:wanted], slave, function)
