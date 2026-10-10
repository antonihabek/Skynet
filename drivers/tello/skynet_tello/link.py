"""The two UDP sockets of a Tello: the command socket, which sends text to the drone's port 8889 and is answered from it at the address it was sent from, and the state socket, on which the drone streams its state string to
port 8890 of the host it was last spoken to by. Standard library only.

**A datagram is a whole message or nothing**, so there is no framing here, and the only discipline is the source: a datagram from any address but the drone's is not the drone's and is counted and dropped (a Tello's
network is an open access point), and the length is bounded at 1,024 bytes, which is what the maker's own sample reads (`recvfrom(1024)`).
"""

from __future__ import annotations

import select
import socket

__all__ = ["LinkError", "UdpLink", "MAX_DATAGRAM"]

MAX_DATAGRAM = 1024


class LinkError(Exception):
    """The sockets could not be opened, or a datagram could not be sent."""


class UdpLink:
    """Both sockets, for one drone at `host`."""

    def __init__(self, host: str, port: int, state_port: int, local_port: int = 0) -> None:
        self._host, self._port, self._state_port, self._local_port = host, port, state_port, local_port
        self._command: socket.socket | None = None
        self._state: socket.socket | None = None
        #: Datagrams from an address that is not the drone's, and datagrams longer than the bound, since the sockets were opened.
        self.foreign = 0
        self.oversize = 0

    @property
    def target(self) -> str:
        return f"{self._host}:{self._port}"

    @property
    def is_open(self) -> bool:
        return self._command is not None and self._state is not None

    @property
    def local_port(self) -> int:
        return self._command.getsockname()[1] if self._command is not None else 0

    def open(self) -> None:
        self.close()
        try:
            command = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            command.bind(("", self._local_port))
            command.setblocking(False)
            state = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                state.bind(("", self._state_port))
            except OSError:
                command.close()
                state.close()
                raise
            state.setblocking(False)
        except (OSError, OverflowError) as error:
            raise LinkError(f"the sockets could not be opened (local port {self._local_port}, state port {self._state_port}): {error}") from error
        self._command, self._state = command, state

    def close(self) -> None:
        for sock in (self._command, self._state):
            if sock is not None:
                try:
                    sock.close()
                except OSError:
                    pass
        self._command = self._state = None

    def send(self, text: str) -> None:
        """One datagram to the drone's command port. LinkError for a socket that is closed or that the network will not take it on."""
        if self._command is None:
            raise LinkError(f"{self.target} is not open.")
        try:
            self._command.sendto(text.encode("utf-8"), (self._host, self._port))
        except (OSError, OverflowError) as error:
            raise LinkError(f"{self.target} could not be sent to: {error}") from error

    def poll(self, wait: float, extra: tuple[int, ...] = ()) -> tuple[list[bytes], list[bytes], bool]:
        """The replies and the state strings that have arrived, waiting at most `wait` for the first of anything, and whether an `extra` descriptor woke the wait. A datagram from an address that is not the drone's is dropped."""
        if self._command is None or self._state is None:
            return [], [], False
        try:
            readable, _, _ = select.select([self._command, self._state, *extra], [], [], max(wait, 0.0))
        except (OSError, ValueError):
            return [], [], False
        woke = any(handle in extra for handle in readable)
        return self._drain(self._command, readable), self._drain(self._state, readable), woke

    def _drain(self, sock: socket.socket, readable: list) -> list[bytes]:
        out: list[bytes] = []
        if sock not in readable:
            return out
        while True:
            try:
                data, address = sock.recvfrom(MAX_DATAGRAM + 1)
            except (BlockingIOError, InterruptedError):
                return out
            except OSError:
                return out
            if address[0] != self._host:
                self.foreign += 1
                continue
            if len(data) > MAX_DATAGRAM:
                self.oversize += 1
                continue
            out.append(data)
