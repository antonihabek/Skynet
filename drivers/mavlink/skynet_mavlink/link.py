"""Where the bytes go: a socket or a serial port, behind one small interface.

A MAVLink vehicle is reached three ways in practice and all three are here, in the stdlib:

| Spec | What it is | Used for |
|---|---|---|
| `tcp:HOST:PORT` | connect to the vehicle | ArduPilot SITL (`5760`), an autopilot behind a bridge |
| `udpin:HOST:PORT` | bind, and answer whoever sends first | a vehicle that pushes to a ground station (`14550`), `mavlink-router` |
| `udpout:HOST:PORT` | send to a vehicle that listens | a companion computer's own endpoint |
| `serial:PATH:BAUD` | a flight controller's USB or telemetry port | a real flight controller (`/dev/ttyACM0`, `921600`) |

A spec with no scheme is refused: `localhost:5760` could be any of them, and a driver that guessed which
would open a different link on a machine that answered differently.

Nothing here parses MAVLink and nothing retries. A link says what happened to bytes: `recv` returns what
arrived (or nothing, after its timeout) and raises `LinkClosed` when the far end went away, and `send`
raises `LinkClosed` when it could not deliver.
"""

from __future__ import annotations

import os
import select
import socket
import termios
import threading

__all__ = ["Link", "LinkClosed", "LinkRefusal", "open_link", "parse_spec"]


class LinkClosed(Exception):
    """The far end went away, or a write could not be delivered."""


class LinkRefusal(Exception):
    """A spec this module will not open, in the words an integrator acts on."""


_SCHEMES = ("tcp", "udpin", "udpout", "serial")


def parse_spec(spec: object) -> tuple[str, str, int | None]:
    """`(scheme, host-or-path, port-or-baud)`, or a refusal that shows the accepted forms."""
    forms = "tcp:127.0.0.1:5760, udpin:0.0.0.0:14550, udpout:192.168.1.10:14550 or serial:/dev/ttyACM0:921600"
    if not isinstance(spec, str) or not spec:
        raise LinkRefusal(f"no `link` in the configuration. Name one of {forms}.")
    scheme, _, rest = spec.partition(":")
    if scheme not in _SCHEMES or not rest:
        raise LinkRefusal(
            f"`link` {spec!r} does not start with one of {', '.join(_SCHEMES)}. Name one of {forms} — a bare "
            "host:port could be any of them, and a driver that guessed would open a different link on a "
            "machine that answered differently."
        )
    where, sep, tail = rest.rpartition(":")
    if not sep or not where:
        raise LinkRefusal(f"`link` {spec!r} has no {'baud rate' if scheme == 'serial' else 'port'}. Name one of {forms}.")
    try:
        number = int(tail)
    except ValueError as error:
        raise LinkRefusal(f"`link` {spec!r}: {tail!r} is not a number. Name one of {forms}.") from error
    if scheme == "serial":
        if not hasattr(termios, f"B{number}"):
            raise LinkRefusal(f"`link` {spec!r}: this platform has no {number} baud setting.")
    elif not 0 < number < 65536:
        raise LinkRefusal(f"`link` {spec!r}: {number} is not a port.")
    return scheme, where, number


class Link:
    """One open link. `recv` and `send` are safe to call from different threads."""

    def __init__(self, scheme: str) -> None:
        self.scheme = scheme
        self._send_lock = threading.Lock()
        self.closed = False

    def recv(self, timeout: float) -> bytes:  # pragma: no cover - interface
        raise NotImplementedError

    def send(self, data: bytes) -> None:  # pragma: no cover - interface
        raise NotImplementedError

    def try_send(self, data: bytes, wait: float) -> bool:
        """Send if the link is free within `wait` seconds; `False` when a write in progress kept it.

        Used by the stop path: it must not queue behind a command in flight, and it must not write into the
        middle of one either — a half-frame followed by a stop is a corrupt frame, and the parser on the far
        end would drop both.
        """
        if not self._send_lock.acquire(timeout=wait):
            return False
        try:
            self._write(data)
        finally:
            self._send_lock.release()
        return True

    def _write(self, data: bytes) -> None:  # pragma: no cover - interface
        raise NotImplementedError

    def close(self) -> None:  # pragma: no cover - interface
        raise NotImplementedError


class _Socket(Link):
    def __init__(self, scheme: str, sock: socket.socket, peer: tuple | None) -> None:
        super().__init__(scheme)
        self._sock = sock
        self._peer = peer  # UDP: where to answer. `None` until the first datagram of a `udpin` link.

    def recv(self, timeout: float) -> bytes:
        if self.closed:
            raise LinkClosed("the link is closed")
        self._sock.settimeout(timeout)
        try:
            if self.scheme == "tcp":
                data = self._sock.recv(65536)
                if not data:
                    raise LinkClosed("the vehicle closed the connection")
                return data
            data, peer = self._sock.recvfrom(65536)
            if self.scheme == "udpin" and self._peer is None:
                self._peer = peer
            return data
        except socket.timeout:
            return b""
        except OSError as error:
            if self.closed:
                raise LinkClosed("the link is closed") from error
            raise LinkClosed(f"the socket failed: {error}") from error

    def send(self, data: bytes) -> None:
        with self._send_lock:
            self._write(data)

    def _write(self, data: bytes) -> None:
        if self.closed:
            raise LinkClosed("the link is closed")
        try:
            if self.scheme == "tcp":
                self._sock.settimeout(1.0)
                self._sock.sendall(data)
            elif self._peer is None:
                # A `udpin` link answers whoever spoke first, and nobody has: there is no one to send to.
                raise LinkClosed("nothing has been heard on this udpin link yet, so there is no one to send to")
            else:
                self._sock.sendto(data, self._peer)
        except OSError as error:
            raise LinkClosed(f"the socket failed: {error}") from error

    def close(self) -> None:
        self.closed = True
        try:
            self._sock.close()
        except OSError:
            pass


class _Serial(Link):
    def __init__(self, fd: int, path: str) -> None:
        super().__init__("serial")
        self._fd = fd
        self.path = path

    def recv(self, timeout: float) -> bytes:
        if self.closed:
            raise LinkClosed("the link is closed")
        try:
            ready, _, _ = select.select([self._fd], [], [], timeout)
            if not ready:
                return b""
            data = os.read(self._fd, 65536)
        except OSError as error:
            raise LinkClosed(f"the serial port failed: {error}") from error
        if not data:
            raise LinkClosed("the serial port reports end of file (a device that was unplugged)")
        return data

    def send(self, data: bytes) -> None:
        with self._send_lock:
            self._write(data)

    def _write(self, data: bytes) -> None:
        if self.closed:
            raise LinkClosed("the link is closed")
        view = memoryview(data)
        try:
            while view:
                _, writable, _ = select.select([], [self._fd], [], 1.0)
                if not writable:
                    raise LinkClosed("the serial port did not accept a write for a second")
                view = view[os.write(self._fd, view) :]
        except OSError as error:
            raise LinkClosed(f"the serial port failed: {error}") from error

    def close(self) -> None:
        self.closed = True
        try:
            os.close(self._fd)
        except OSError:
            pass


def open_link(spec: object, *, connect_timeout: float = 3.0) -> Link:
    """Open `spec`, or raise `LinkRefusal` (the spec is wrong) or `LinkClosed` (it is right and nothing answered)."""
    scheme, where, number = parse_spec(spec)
    assert number is not None
    if scheme == "tcp":
        try:
            sock = socket.create_connection((where, number), timeout=connect_timeout)
        except OSError as error:
            raise LinkClosed(
                f"{where}:{number} did not accept a connection: {error}. A vehicle that is off, one on another "
                "address and one whose port is not open look the same from here."
            ) from error
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        return _Socket("tcp", sock, None)
    if scheme == "udpin":
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            sock.bind((where, number))
        except OSError as error:
            sock.close()
            raise LinkClosed(f"could not listen on {where}:{number}: {error}") from error
        return _Socket("udpin", sock, None)
    if scheme == "udpout":
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.bind(("", 0))
        return _Socket("udpout", sock, (where, number))
    try:
        fd = os.open(where, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
    except OSError as error:
        raise LinkClosed(f"could not open {where}: {error}") from error
    try:
        attrs = termios.tcgetattr(fd)
        speed = getattr(termios, f"B{number}")
        attrs[0] = 0  # no input processing: MAVLink is binary
        attrs[1] = 0
        attrs[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
        attrs[3] = 0
        attrs[4] = attrs[5] = speed
        attrs[6][termios.VMIN] = 0
        attrs[6][termios.VTIME] = 0
        termios.tcsetattr(fd, termios.TCSANOW, attrs)
        termios.tcflush(fd, termios.TCIOFLUSH)
    except (termios.error, OSError) as error:
        os.close(fd)
        raise LinkClosed(f"{where} is not a serial port this platform can configure: {error}") from error
    return _Serial(fd, where)
