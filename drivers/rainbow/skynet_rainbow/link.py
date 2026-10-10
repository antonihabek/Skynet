"""The two connections to a Rainbow RB controller: the command port (5000), which takes one text script a line and says, as it happens, what it made of each, and the data port (5001), which answers the line `reqdata`
with one 580-byte state packet. Standard library only.

**The command port** is not request and reply. Every line the controller parses is acknowledged at once (`The command was executed`) and may then be followed, at any time, by `info[...]`, `warn[...]` and `error[...]`
lines: a motion's start and end (`info[motion_changed][7100]` and `[0]`), a refusal (`warn[code][562]`), a fault (`error[code][N]`). A message whose brackets are not closed goes on to the next line, and the controller's own
syntax error is five of them, so lines are joined until the brackets balance. What is written is never waited for here: the caller counts the acknowledgements.

**The data port** is request and reply, one packet for each `reqdata`. A reply that is late is not the next request's answer, so a request that timed out closes the connection (and whoever drives the link opens it
again), and what is waiting when a request is sent is an answer to an earlier one and is thrown away.
"""

from __future__ import annotations

import select
import socket
import time

from . import frames

#: The longest the lines of one bracketed message are waited for: a message whose brackets never close is something other than the controller's.
PARTIAL_S = 0.5
#: The most lines and bytes one message may have before it is called garbled.
MAX_MESSAGE_LINES = 64
MAX_MESSAGE_BYTES = 8192
#: The most unread bytes the command port may hold without a newline, and the most noise the data port may send before it is called a line that is not the controller's.
MAX_LINE = 8192
MAX_NOISE = 8192


class LinkError(Exception):
    """The line did not carry a command or a packet: nothing opened, a reply that did not come, or a connection that ended."""


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

    def _read(self, wait: float, extra: tuple[int, ...] = ()) -> tuple[bytes, bool]:
        """Whatever has arrived, waiting at most `wait` for the first of it (a wait of nothing or less is a look), and whether one of the `extra` descriptors became readable instead. Empty bytes on a timeout."""
        if self._sock is None:
            return b"", False
        readable, _, _ = select.select([self._sock, *extra], [], [], max(wait, 0.0))
        woke = any(handle in extra for handle in readable)
        if self._sock not in readable:
            return b"", woke
        self._sock.settimeout(0)
        try:
            data = self._sock.recv(65536)
        except BlockingIOError:
            return b"", woke
        if not data:
            self.close()
            raise LinkError(f"{self.target} closed the connection.")
        return data, woke

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


class CommandChannel(_Socket):
    """The text connection: lines out, events in. Nothing here waits for an answer to a line it wrote."""

    def __init__(self, host: str, port: int) -> None:
        super().__init__(host, port)
        self._buffer = b""
        self._partial: list[str] = []
        self._partial_since = 0.0

    def open(self, timeout: float = 1.0) -> None:
        self._reset()
        super().open(timeout)

    def close(self) -> None:
        self._reset()
        super().close()

    def _reset(self) -> None:
        self._buffer = b""
        self._partial = []
        self._partial_since = 0.0

    def send(self, text: str, deadline: float) -> None:
        """One line, within `deadline` (a monotonic time). LinkError for a line that was not taken in time, and for a closed connection: the line is then not on the wire, or not all of it."""
        if not self.is_open:
            raise LinkError(f"{self.target} is not open.")
        if "\n" in text or "\r" in text:
            raise frames.ProtocolError("a script is one line.")
        try:
            self._write(text.encode() + b"\n", deadline)
        except OSError as error:
            self.close()
            raise LinkError(f"{self.target} failed: {error}") from error

    def poll(self, wait: float = 0.0, extra: tuple[int, ...] = ()) -> tuple[list[frames.Event], bool]:
        """The events that have arrived, waiting at most `wait` for the first bytes, and whether an `extra` descriptor woke the wait. LinkError for a connection that ended or sent a line that does not end."""
        if not self.is_open:
            raise LinkError(f"{self.target} is not open.")
        try:
            data, woke = self._read(wait, extra)
        except OSError as error:
            self.close()
            raise LinkError(f"{self.target} failed: {error}") from error
        events: list[frames.Event] = []
        now = time.monotonic()
        if data:
            self._buffer += data
            while b"\n" in self._buffer:
                line, self._buffer = self._buffer.split(b"\n", 1)
                self._take(line.decode("utf-8", "replace").rstrip("\r"), now, events)
            if len(self._buffer) > MAX_LINE:
                self.close()
                raise LinkError(f"{self.target} sent {len(self._buffer)} bytes with no line break: a line that is not the controller's command port.")
        if self._partial and now - self._partial_since > PARTIAL_S:
            events.append(frames.Event("unknown", "", "", "\n".join(self._partial)))
            self._partial = []
        return events, woke

    def _take(self, line: str, now: float, events: list[frames.Event]) -> None:
        if self._partial:
            if line == frames.ACK:
                # An acknowledgement is not part of a message: the message that was being joined never closed.
                events.append(frames.Event("unknown", "", "", "\n".join(self._partial)))
                self._partial = []
            else:
                self._partial.append(line)
                joined = "\n".join(self._partial)
                if frames.bracket_depth(joined) <= 0:
                    events.append(frames.parse_event(joined))
                    self._partial = []
                elif len(self._partial) > MAX_MESSAGE_LINES or len(joined) > MAX_MESSAGE_BYTES:
                    events.append(frames.Event("unknown", "", "", joined[:MAX_MESSAGE_BYTES]))
                    self._partial = []
                return
        if not line.strip():
            return
        if frames.bracket_depth(line) > 0 and line.split("[", 1)[0] in ("info", "warn", "error"):
            self._partial = [line]
            self._partial_since = now
            return
        events.append(frames.parse_event(line))

    def query(self, text: str, accept, budget: float) -> list[frames.Event]:
        """Send `text` and collect events until `accept` (a function of an Event) is true of one, within `budget` seconds: the events up to and including it. LinkError when it does not come. For the open, before the
        reading thread exists; nothing else reads this connection while it runs."""
        deadline = time.monotonic() + budget
        self.send(text, deadline)
        seen: list[frames.Event] = []
        while True:
            now = time.monotonic()
            if now >= deadline:
                raise LinkError(f"{self.target} did not answer {text.split('(')[0]} in {budget * 1000:.0f} ms.")
            events, _ = self.poll(min(deadline - now, 0.05))
            for event in events:
                seen.append(event)
                if accept(event):
                    return seen


class DataChannel(_Socket):
    """The packet connection: one request, one packet, and nothing older."""

    def __init__(self, host: str, port: int) -> None:
        super().__init__(host, port)
        self._buffer = b""

    def open(self, timeout: float = 1.0) -> None:
        self._buffer = b""
        super().open(timeout)

    def close(self) -> None:
        self._buffer = b""
        super().close()

    def request(self, budget: float, wake: int = -1, on_wake=None) -> bytes:
        """Send `reqdata` and return the 580-byte state packet it is answered with, within `budget` seconds. LinkError for a line that did not carry it (and the line is closed: a reply that comes late is not the next
        one's). While the reply is awaited a readable `wake` descriptor calls `on_wake` and the wait goes on, so a stop that is owed is not made to wait for the reply."""
        if not self.is_open:
            raise LinkError(f"{self.target} is not open.")
        deadline = time.monotonic() + budget
        extra = (wake,) if wake >= 0 else ()
        try:
            self._discard()
            self._write(frames.REQDATA.encode() + b"\n", deadline)
            noise = 0
            while True:
                packet, dropped = self._pop()
                noise += dropped
                if noise > MAX_NOISE:
                    self.close()
                    raise LinkError(f"{self.target} sent {noise} bytes and none of them begins a state packet: a line that is noise, or not the controller's data port.")
                if packet is not None:
                    return packet
                now = time.monotonic()
                if now >= deadline:
                    self.close()
                    raise LinkError(f"{self.target} did not answer {frames.REQDATA} in {budget * 1000:.0f} ms.")
                chunk, woke = self._read(deadline - now, extra)
                if woke and on_wake is not None:
                    on_wake()
                self._buffer += chunk
        except OSError as error:
            self.close()
            raise LinkError(f"{self.target} failed: {error}") from error

    def _pop(self) -> tuple[bytes | None, int]:
        """One whole state packet off the front of the buffer, and how many bytes were dropped to find it: a packet of another type is skipped by its own size, and anything that does not begin `$` is noise."""
        dropped = 0
        while len(self._buffer) >= 4:
            if self._buffer[0] != frames.PACKET_START:
                self._buffer = self._buffer[1:]
                dropped += 1
                continue
            total = (self._buffer[1] | self._buffer[2] << 8) + 4
            if total < 4 or total > 4096:
                self._buffer = self._buffer[1:]
                dropped += 1
                continue
            if len(self._buffer) < total:
                return None, dropped
            packet, self._buffer = self._buffer[:total], self._buffer[total:]
            if packet[3] == frames.PACKET_TYPE_STATE and total == frames.PACKET_BYTES:
                return packet, dropped
        return None, dropped

    def _discard(self) -> None:
        """Whatever is waiting when a request is sent is an answer to an earlier one."""
        self._buffer = b""
        if self._sock is None:
            return
        self._sock.setblocking(False)
        try:
            while True:
                if not self._sock.recv(4096):
                    break
        except (BlockingIOError, OSError):
            pass
