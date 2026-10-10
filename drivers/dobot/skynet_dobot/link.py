"""The two connections to a Dobot V4 controller: the dashboard on 29999, which takes one text command at a time and answers each with one reply, and the feedback on 30004, which sends a 1440-byte packet every 8 ms.
Standard library only.

**The dashboard** is request and reply, and a reply is matched to its request by what it says it heard: the controller echoes the command (`ErrorID,{value},Name(params);`), so a reply that arrives late, after the
caller gave up on its command, is a reply to something else and is thrown away. A command that timed out closes the connection all the same, because the next one's reply could otherwise be the stale one's twin
(`RobotMode()` twice has the same echo twice), and whoever drives the link opens it again.

**The feedback** is a stream of fixed-size packets with nothing between them but their own length and self-test field. The newest complete packet is what a caller wants; older ones that piled up while it was
not looking are dropped, and a stream that has lost its place finds it again at the next 1440 bytes that begin with their own length and carry the self-test value at byte 48.
"""

from __future__ import annotations

import select
import socket
import struct
import time

from . import frames

#: Between bytes of one feedback packet, once its first byte has arrived. The controller sends 1440 bytes every 8 ms; this driver's own figure.
BYTE_TIMEOUT_S = 0.05
#: Bytes of a dashboard reply or of a feedback stream that are not what they should be, read before the line is called noise.
MAX_NOISE = 8192
#: The longest reply read before it is called a line that is not the controller's.
MAX_REPLY = 4096


class LinkError(Exception):
    """The line did not carry a command or a packet: nothing opened, a reply that did not come, or a packet that stopped half way."""


class NotTcp(LinkError):
    """The controller answered that its control mode is not TCP: it hears the line and takes no command from it until a person asks for control."""


def _normal(text: str) -> str:
    return "".join(text.split()).lower()


class _Socket:
    def __init__(self, host: str, port: int) -> None:
        self._host, self._port = host, port
        self._sock: socket.socket | None = None

    @property
    def target(self) -> str:
        return f"{self._host}:{self._port}"

    @property
    def is_open(self) -> bool:
        return self._sock is not None

    def open(self, timeout: float = 1.0) -> None:
        self.close()
        try:
            sock = socket.create_connection((self._host, self._port), timeout=timeout)
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        except (OSError, ValueError) as error:
            raise LinkError(f"{self.target} could not be opened: {error}") from error
        self._sock = sock

    def close(self) -> None:
        sock, self._sock = self._sock, None
        if sock is not None:
            try:
                sock.close()
            except OSError:
                pass

    def _read(self, wait: float) -> bytes:
        """Whatever has arrived, waiting at most `wait` for the first of it (a wait of nothing or less is a look, not a wait). Empty on a timeout."""
        if self._sock is None:
            return b""
        readable, _, _ = select.select([self._sock], [], [], max(wait, 0.0))
        if not readable:
            return b""
        self._sock.settimeout(0)
        try:
            data = self._sock.recv(65536)
        except BlockingIOError:
            return b""
        if not data:
            self.close()
            raise LinkError(f"{self.target} closed the connection.")
        return data

    def _write(self, data: bytes, deadline: float) -> None:
        assert self._sock is not None
        view = memoryview(data)
        while view:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise LinkError(f"{self.target} did not take the request in time.")
            self._sock.settimeout(remaining)
            try:
                sent = self._sock.send(view)
            except (socket.timeout, BlockingIOError):
                continue
            view = view[sent:]


class Dashboard(_Socket):
    """The text connection: one command in flight, matched to its reply by the command it echoes."""

    def command(self, text: str, budget: float) -> frames.Reply:
        """Send `text` and return the controller's reply to it, within `budget` seconds. LinkError for a line that did not carry it (and the line is closed), NotTcp for a controller that is not taking commands,
        frames.ProtocolError for a reply that is not one the interface document allows."""
        if not self.is_open:
            raise LinkError(f"{self.target} is not open.")
        deadline = time.monotonic() + budget
        try:
            self._discard()
            self._write(text.encode() + b"\n", deadline)
            wanted = _normal(text)
            buffer = b""
            noise = 0
            while True:
                now = time.monotonic()
                if now >= deadline:
                    self.close()
                    raise LinkError(f"{self.target} did not answer {text.split('(')[0]} in {budget * 1000:.0f} ms.")
                chunk = self._read(deadline - now)
                if not chunk:
                    continue
                buffer += chunk
                while True:
                    line, found = self._take(buffer)
                    if not found:
                        if len(buffer) > MAX_REPLY:
                            self.close()
                            raise LinkError(f"{self.target} sent {len(buffer)} bytes that are not a reply.")
                        break
                    buffer = buffer[len(line):]
                    reply_text = line.decode("utf-8", "replace").strip()
                    if not reply_text:
                        continue
                    if frames.NOT_TCP.lower() in reply_text.lower():
                        raise NotTcp(f"{self.target} says its control mode is not TCP: {reply_text[:60]!r}.")
                    reply = frames.parse_reply(reply_text)
                    if reply.error == -10000 or _normal(reply.echo) == wanted:
                        return reply
                    noise += len(line)
                    if noise > MAX_NOISE:
                        self.close()
                        raise LinkError(f"{self.target} sent replies to other commands and none to {text.split('(')[0]}.")
        except OSError as error:
            self.close()
            raise LinkError(f"{self.target} failed: {error}") from error

    @staticmethod
    def _take(buffer: bytes) -> tuple[bytes, bool]:
        """The first complete line of `buffer`: up to a newline, or up to a semicolon that closes a reply."""
        newline = buffer.find(b"\n")
        semicolon = buffer.find(b";")
        ends = [at for at in (newline, semicolon) if at >= 0]
        if not ends:
            return b"", False
        at = min(ends)
        return buffer[:at + 1], True

    def _discard(self) -> None:
        """Whatever is waiting when a command is sent is an answer to an earlier one."""
        if self._sock is None:
            return
        self._sock.setblocking(False)
        try:
            while True:
                if not self._sock.recv(4096):
                    break
        except (BlockingIOError, OSError):
            pass


class FeedbackStream(_Socket):
    """The packet connection: the newest complete packet, and nothing older."""

    def __init__(self, host: str, port: int) -> None:
        super().__init__(host, port)
        self._buffer = b""
        self._partial_since: float | None = None

    def open(self, timeout: float = 1.0) -> None:
        self._buffer = b""
        self._partial_since = None
        super().open(timeout)

    def close(self) -> None:
        self._buffer = b""
        self._partial_since = None
        super().close()

    def read_packet(self, wait: float) -> bytes | None:
        """The newest complete packet that has arrived, waiting at most `wait` for one (a wait of nothing is a look); None when there is none. LinkError for a connection that ended, a packet that stopped half way
        for longer than the byte timeout, or a stream of noise."""
        if not self.is_open:
            raise LinkError(f"{self.target} is not open.")
        deadline = time.monotonic() + wait
        skipped = 0
        newest: bytes | None = None
        looked = False
        try:
            while True:
                packet, dropped = self._pop()
                skipped += dropped
                if skipped > MAX_NOISE:
                    self.close()
                    raise LinkError(f"{self.target} sent {skipped} bytes and none of them begins a feedback packet: a line that is noise, or not the controller's feedback port.")
                if packet is not None:
                    newest = packet
                    self._partial_since = None
                    continue
                if newest is not None:
                    return newest
                now = time.monotonic()
                if self._buffer and self._partial_since is None:
                    self._partial_since = now
                if self._buffer and now - (self._partial_since or now) > BYTE_TIMEOUT_S:
                    have = len(self._buffer)
                    self._buffer = b""
                    self._partial_since = None
                    raise LinkError(f"{self.target} stopped sending {have} bytes into a packet of {frames.FEEDBACK_BYTES}.")
                if looked and now >= deadline:
                    return None
                limit = deadline - now
                if self._buffer:
                    limit = min(limit, BYTE_TIMEOUT_S - (now - (self._partial_since or now)))
                chunk = self._read(limit)
                looked = True
                if chunk:
                    self._buffer += chunk
                    if self._partial_since is None:
                        self._partial_since = time.monotonic()
        except OSError as error:
            self.close()
            raise LinkError(f"{self.target} failed: {error}") from error

    def _pop(self) -> tuple[bytes | None, int]:
        """One whole packet off the front of the buffer, and how many bytes were dropped to find it."""
        dropped = 0
        while len(self._buffer) >= frames.FEEDBACK_BYTES:
            size = struct.unpack_from("<H", self._buffer, 0)[0]
            magic = struct.unpack_from("<Q", self._buffer, 48)[0]
            if size == frames.FEEDBACK_BYTES and magic == frames.FEEDBACK_MAGIC:
                packet, self._buffer = self._buffer[:frames.FEEDBACK_BYTES], self._buffer[frames.FEEDBACK_BYTES:]
                return packet, dropped
            self._buffer = self._buffer[1:]
            dropped += 1
        return None, dropped
