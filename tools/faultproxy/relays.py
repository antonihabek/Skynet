"""The transports: TCP, UDP, a serial line, SocketCAN and raw Ethernet, each relayed through an `Engine`, and the kernel's own link emulation (`netem`) for what none of them can stand in front of.

A relay sits between a driver and its substrate. The driver is pointed at the relay instead of the substrate; the relay reads what
each side sends, asks the engine what the link does with it, and delivers the survivors at the times the engine says. A relay
adds nothing of its own: a clean profile is a wire, and what comes out is what went in, in order, at once. The kernel relay is the
exception in how it stands: it is not between anything, and it carries the profile on an interface the driver and the substrate
share.

DRY-COVERAGE-PLAN.md item 12.C.4. Python's standard library and no more, as the drivers' own fakes are: `socket` carries `AF_CAN`
and `AF_PACKET`, `pty` makes a serial line without hardware (the kernel relay runs `tc`, which is iproute2's and not Python's).

## What each transport's faults mean

* **TCP** is a stream. A relay reads whatever the kernel hands it (a *chunk*), so `loss`, `truncate` and `corrupt` are what a
  broken gateway does to a stream and not what a lossy network does to TCP, which retransmits: the driver sees a hole in its
  framing. `delay`, `jitter`, `slow`, `stall`, `blackhole` (a half-open connection: accepted, open, silent), `reset` and `kill`
  (the substrate's process killed and started again) are what a real link and a real peer do.
* **UDP** and **CAN** and **raw Ethernet** are datagrams and frames: every fault applies to one message and is what a real
  network or bus does to it — except `corrupt` on CAN, where a frame with a wrong CRC never reaches a controller, so the fault is
  a gateway's and is named as such.
* **A serial line** is a stream of bytes with no framing and no flow control unless the substrate adds them.
* **The kernel's `netem`** carries packets on one interface, in both directions at once, and a profile is the interface's: see `netem_arguments` for what it cannot do (a scoped fault, `truncate`, `reset`, `kill`)
  and `NetemRelay` for why it exists (DDS and the vendor SDKs on it name their peers in discovery messages, so there is no address to point a relay at).

The relay is one thread per direction to read, and one per direction to deliver. The delivery queue is bounded: when a slow
link (`slow=`) or a stall has filled it the relay stops reading, and that is how the writer finds out — a blocking `send` that
blocks, a non-blocking one that says it would.
"""

from __future__ import annotations

import os
import select
import shutil
import signal
import socket
import struct
import subprocess
import sys
import threading
import time
from typing import Callable

from engine import DOWN, UP, Delivery, Engine, Fault, parse_profile

_CHUNK = 4096


def monotonic() -> float:
    return time.monotonic()


class Supervised:
    """The substrate's process, for `kill`: killed with SIGKILL, started again after a pause, never restarted by anything else."""

    def __init__(self, argv: list[str], ready: Callable[[], bool] | None = None) -> None:
        self.argv = argv
        self.ready = ready
        self.process: subprocess.Popen | None = None
        self.restarts = 0
        self._lock = threading.Lock()

    def start(self) -> None:
        with self._lock:
            self.process = subprocess.Popen(self.argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if self.ready is not None:
            deadline = monotonic() + 10
            while monotonic() < deadline and not self.ready():
                time.sleep(0.02)

    def kill(self) -> None:
        with self._lock:
            process = self.process
        if process is not None and process.poll() is None:
            process.send_signal(signal.SIGKILL)
            process.wait()

    def stop(self) -> None:
        self.kill()


class _Pump(threading.Thread):
    """Reads one direction off a source and puts what survives into that direction's delivery queue."""

    def __init__(self, name: str, read: Callable[[], object], engine: Engine, direction: str, delivery: Delivery, stop: threading.Event, on_end: Callable[[bool], None] | None = None):
        super().__init__(name=name, daemon=True)
        self.read, self.engine, self.direction, self.delivery, self.stop_event = read, engine, direction, delivery, stop
        #: Called once when the source ends: with False for an orderly end of stream, True for an error.
        self.on_end = on_end

    def run(self) -> None:
        while not self.stop_event.is_set():
            try:
                got = self.read()
            except (OSError, ValueError):
                # A socket closed under the pump has file descriptor -1, and `select` says so with a ValueError: it is the same end as a reset, and it used to kill this thread with a traceback.
                if self.on_end:
                    self.on_end(True)
                return
            if got is None:
                continue
            if got is EOF:
                if self.on_end:
                    self.on_end(False)
                return
            data, meta = got
            for when, out in self.engine.offer(self.direction, data, monotonic()):
                self.delivery.put(when, out, meta)


EOF = object()


class _Deliverer(threading.Thread):
    """Writes what is due, in time order, to the far side."""

    def __init__(self, name: str, delivery: Delivery, write: Callable[[bytes, object], None], stop: threading.Event):
        super().__init__(name=name, daemon=True)
        self.delivery, self.write, self.stop_event = delivery, write, stop

    def run(self) -> None:
        while not self.stop_event.is_set():
            got = self.delivery.get(monotonic)
            if got is None:
                continue
            data, meta = got
            try:
                self.write(data, meta)
            except OSError:
                continue


class Relay:
    """What every transport's relay offers a test or the command line."""

    transport = "?"

    def __init__(self, engine: Engine, supervised: Supervised | None = None) -> None:
        self.engine = engine
        self.supervised = supervised
        self._stop = threading.Event()
        self._threads: list[threading.Thread] = []
        self._deliveries: list[Delivery] = []
        self._watch = threading.Thread(target=self._events, daemon=True)

    def start(self) -> "Relay":
        if self.supervised is not None and self.supervised.process is None:
            self.supervised.start()
        self._open()
        self._watch.start()
        return self

    def ready(self) -> str:
        return f"ready {self.transport}"

    def stop(self) -> None:
        self._stop.set()
        for delivery in self._deliveries:
            delivery.close()
        # The threads first, and only then the descriptors they were reading: a descriptor number closed under a thread that has
        # not noticed is handed to the next relay, and the old thread reads the new one's bytes.
        for thread in [*self._threads, self._watch]:
            if thread.is_alive() and thread is not threading.current_thread():
                thread.join(1.0)
        self._close()
        if self.supervised is not None:
            self.supervised.stop()

    def _new_delivery(self, cap_bytes: int = 1 << 20) -> Delivery:
        delivery = Delivery(cap_bytes)
        self._deliveries.append(delivery)
        return delivery

    def _events(self) -> None:
        while not self._stop.is_set():
            for event, arg in self.engine.due_events(monotonic()):
                if event == "reset":
                    self._reset()
                elif event == "kill" and self.supervised is not None:
                    self._kill(arg)
            time.sleep(0.01)

    def _kill(self, restart_after: float) -> None:
        self._reset()
        self.supervised.kill()

        def again() -> None:
            time.sleep(restart_after)
            if not self._stop.is_set():
                self.supervised.restarts += 1
                self.supervised.start()

        threading.Thread(target=again, daemon=True).start()

    # Overridden by a transport:
    def _open(self) -> None: ...
    def _close(self) -> None: ...
    def _reset(self) -> None: ...


# ── TCP ──────────────────────────────────────────────────────────────────────


class _Connection:
    def __init__(self, client: socket.socket, upstream: socket.socket) -> None:
        self.client, self.upstream = client, upstream

    def reset(self) -> None:
        for sock in (self.client, self.upstream):
            try:
                sock.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
            except OSError:
                pass
            try:
                sock.close()
            except OSError:
                pass


class TcpRelay(Relay):
    transport = "tcp"

    def __init__(self, engine: Engine, listen: tuple[str, int], upstream: tuple[str, int], supervised: Supervised | None = None, buffer: int | None = None, cap_bytes: int = 1 << 20):
        super().__init__(engine, supervised)
        self.listen_address, self.upstream_address = listen, upstream
        #: A small kernel buffer on the relay's own sockets, so that a slow link pushes back on the writer after a few kilobytes
        #: and not after a few megabytes.
        self.buffer = buffer
        self.cap_bytes = cap_bytes
        self.connections: list[_Connection] = []
        self._listener: socket.socket | None = None

    def _open(self) -> None:
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        if self.buffer:
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, self.buffer)
        listener.bind(self.listen_address)
        listener.listen(8)
        self.listen_address = listener.getsockname()[:2]
        self._listener = listener
        thread = threading.Thread(target=self._accept, daemon=True)
        thread.start()
        self._threads.append(thread)

    def ready(self) -> str:
        return f"ready tcp {self.listen_address[0]}:{self.listen_address[1]}"

    def _accept(self) -> None:
        assert self._listener is not None
        while not self._stop.is_set():
            try:
                ready, _, _ = select.select([self._listener], [], [], 0.05)
                if not ready:
                    continue
                client, _ = self._listener.accept()
            except OSError:
                return
            try:
                upstream = socket.create_connection(self.upstream_address, timeout=2)
            except OSError:
                # A substrate that is down refuses the connection, and so does the relay: the client sees it closed.
                client.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
                client.close()
                continue
            upstream.settimeout(None)
            for sock in (client, upstream):
                sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                if self.buffer:
                    sock.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, self.buffer)
                    sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, self.buffer)
            self._serve(client, upstream)

    def _serve(self, client: socket.socket, upstream: socket.socket) -> None:
        connection = _Connection(client, upstream)
        self.connections.append(connection)
        ended = {UP: False, DOWN: False}

        def reader(sock: socket.socket) -> Callable[[], object]:
            def read() -> object:
                ready, _, _ = select.select([sock], [], [], 0.05)
                if not ready:
                    return None
                data = sock.recv(_CHUNK)
                if not data:
                    return EOF
                return data, None

            return read

        def writer(sock: socket.socket) -> Callable[[bytes, object], None]:
            def write(data: bytes, _meta: object) -> None:
                sock.sendall(data)

            return write

        def ending(direction: str, delivery: Delivery, sink: socket.socket) -> Callable[[bool], None]:
            def end(error: bool) -> None:
                # An orderly end is passed on once everything queued before it has gone out, as a FIN follows the data it
                # follows; an error tears the connection down.
                if error:
                    connection.reset()
                    return
                deadline = monotonic() + 30
                while delivery.pending() and monotonic() < deadline and not self._stop.is_set():
                    time.sleep(0.005)
                try:
                    sink.shutdown(socket.SHUT_WR)
                except OSError:
                    pass
                ended[direction] = True
                if all(ended.values()):
                    connection.reset()

            return end

        for direction, source, sink in ((UP, client, upstream), (DOWN, upstream, client)):
            delivery = self._new_delivery(self.cap_bytes)
            for thread in (
                _Pump(f"tcp-{direction}-read", reader(source), self.engine, direction, delivery, self._stop, ending(direction, delivery, sink)),
                _Deliverer(f"tcp-{direction}-write", delivery, writer(sink), self._stop),
            ):
                thread.start()
                self._threads.append(thread)

    def _reset(self) -> None:
        for connection in self.connections:
            connection.reset()
        self.connections.clear()

    def _close(self) -> None:
        if self._listener is not None:
            try:
                self._listener.close()
            except OSError:
                pass
        self._reset()


# ── UDP ──────────────────────────────────────────────────────────────────────


class UdpRelay(Relay):
    """A datagram relay. The first datagram from an address opens a flow to the substrate from a socket of its own, and what
    the substrate sends back on it goes to that address. A substrate that sends on its own (a state stream to a port the driver
    named in a message the relay cannot read) needs `downstream`: where such datagrams go."""

    transport = "udp"

    def __init__(self, engine: Engine, listen: tuple[str, int], upstream: tuple[str, int], downstream: tuple[str, int] | None = None, supervised: Supervised | None = None):
        super().__init__(engine, supervised)
        self.listen_address, self.upstream_address, self.downstream = listen, upstream, downstream
        self._socket: socket.socket | None = None
        self._flows: dict[tuple, socket.socket] = {}
        self._up = self._new_delivery()
        self._down = self._new_delivery()

    def _open(self) -> None:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.bind(self.listen_address)
        self.listen_address = sock.getsockname()[:2]
        self._socket = sock

        def read_clients() -> object:
            ready, _, _ = select.select([sock], [], [], 0.05)
            if not ready:
                return None
            data, source = sock.recvfrom(65535)
            if source not in self._flows:
                self._flows[source] = self._open_flow(source)
            return data, source

        def to_upstream(data: bytes, source: object) -> None:
            self._flows[source].sendto(data, self.upstream_address)  # type: ignore[index]

        def to_client(data: bytes, destination: object) -> None:
            sock.sendto(data, destination)  # type: ignore[arg-type]

        self._to_client = to_client
        for thread in (
            _Pump("udp-up-read", read_clients, self.engine, UP, self._up, self._stop),
            _Deliverer("udp-up-write", self._up, to_upstream, self._stop),
            _Deliverer("udp-down-write", self._down, to_client, self._stop),
        ):
            thread.start()
            self._threads.append(thread)

    def _open_flow(self, client: tuple) -> socket.socket:
        flow = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        flow.bind(("127.0.0.1", 0))

        def read() -> object:
            ready, _, _ = select.select([flow], [], [], 0.05)
            if not ready:
                return None
            data, _source = flow.recvfrom(65535)
            return data, (self.downstream or client)

        thread = _Pump("udp-down-read", read, self.engine, DOWN, self._down, self._stop)
        thread.start()
        self._threads.append(thread)
        return flow

    def ready(self) -> str:
        return f"ready udp {self.listen_address[0]}:{self.listen_address[1]}"

    def _close(self) -> None:
        if self._socket is not None:
            self._socket.close()
        for flow in self._flows.values():
            flow.close()


# ── a serial line ────────────────────────────────────────────────────────────


class SerialRelay(Relay):
    """Two pseudo-terminals, one for the driver and one for the substrate, and the relay between them.

    `driver_path` and `substrate_path` are the slave ends: the driver opens the first as its serial port and the substrate the
    second as its own, and what one writes the other reads, through the engine. Both are symlinked where asked, so a config can
    name a fixed path."""

    transport = "serial"

    def __init__(self, engine: Engine, link_driver: str | None = None, link_substrate: str | None = None):
        super().__init__(engine)
        self.link_driver, self.link_substrate = link_driver, link_substrate
        self.driver_path = self.substrate_path = ""
        self._masters: list[int] = []
        self._slaves: list[int] = []
        self._links: list[str] = []

    def _open(self) -> None:
        import pty
        import tty

        ends = []
        for _ in range(2):
            master, slave = pty.openpty()
            tty.setraw(slave)
            ends.append((master, slave, os.ttyname(slave)))
            self._masters.append(master)
            self._slaves.append(slave)
        (self._a, _, self.driver_path), (self._b, _, self.substrate_path) = ends
        for wanted, real in ((self.link_driver, self.driver_path), (self.link_substrate, self.substrate_path)):
            if wanted:
                if os.path.lexists(wanted):
                    os.unlink(wanted)
                os.symlink(real, wanted)
                self._links.append(wanted)

        def reader(master: int) -> Callable[[], object]:
            def read() -> object:
                ready, _, _ = select.select([master], [], [], 0.05)
                if not ready:
                    return None
                try:
                    data = os.read(master, _CHUNK)
                except OSError:
                    return None
                return (data, None) if data else None

            return read

        def writer(master: int) -> Callable[[bytes, object], None]:
            def write(data: bytes, _meta: object) -> None:
                view = memoryview(data)
                while view and not self._stop.is_set():
                    try:
                        view = view[os.write(master, view):]
                    except BlockingIOError:
                        time.sleep(0.001)

            return write

        for direction, source, sink in ((UP, self._a, self._b), (DOWN, self._b, self._a)):
            delivery = self._new_delivery()
            for thread in (
                _Pump(f"serial-{direction}-read", reader(source), self.engine, direction, delivery, self._stop),
                _Deliverer(f"serial-{direction}-write", delivery, writer(sink), self._stop),
            ):
                thread.start()
                self._threads.append(thread)

    def ready(self) -> str:
        return f"ready serial {self.link_driver or self.driver_path} {self.link_substrate or self.substrate_path}"

    def _close(self) -> None:
        for path in self._links:
            try:
                os.unlink(path)
            except OSError:
                pass
        for fd in self._masters + self._slaves:
            try:
                os.close(fd)
            except OSError:
                pass


# ── SocketCAN and raw Ethernet ───────────────────────────────────────────────

_CAN_FRAME = struct.Struct("=IB3x8s")  # struct can_frame: id, dlc, padding, data
_CAN_FD_FRAME = struct.Struct("=IBB2x64s")  # struct canfd_frame: id, len, flags (BRS, ESI), two reserved bytes, data


def _can_socket(interface: str, fd: bool = False) -> socket.socket:
    try:
        sock = socket.socket(socket.PF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
        if fd:
            sock.setsockopt(socket.SOL_CAN_RAW, socket.CAN_RAW_FD_FRAMES, 1)
        sock.bind((interface,))
    except (AttributeError, OSError) as error:
        raise SystemExit(
            f"faultproxy: cannot open SocketCAN on `{interface}`: {error}. This needs Linux, a CAN interface (`ip link add dev "
            f"{interface} type vcan`) and CAP_NET_RAW over its network namespace; `drivers/canopen/vcan.sh` gets all three from "
            "`unshare -rn` without root."
        ) from error
    return sock


class CanRelay(Relay):
    """Two SocketCAN interfaces — the driver's bus and the substrate's — with the engine between them. A frame is its data
    bytes to the engine, so `truncate` shortens the DLC and `corrupt` flips bits in the data, and its identifier is carried
    through unchanged. A raw CAN socket does not hear its own transmissions, so a relay does not hear itself.

    With ``fd`` the sockets carry CAN-FD (up to 64 bytes a frame; the interfaces need an MTU of 72), and a frame's flags — the bit-rate switch and the error-state
    indicator — travel with its identifier, so what is delivered is the frame that was sent. A classic frame read from an FD socket is delivered as a classic one."""

    transport = "can"

    def __init__(self, engine: Engine, driver_side: str, substrate_side: str, fd: bool = False):
        super().__init__(engine)
        self.driver_side, self.substrate_side = driver_side, substrate_side
        self.fd = fd
        self._socks: list[socket.socket] = []

    def _open(self) -> None:
        fd = self.fd
        a, b = _can_socket(self.driver_side, fd), _can_socket(self.substrate_side, fd)
        self._socks = [a, b]

        def reader(sock: socket.socket) -> Callable[[], object]:
            def read() -> object:
                ready, _, _ = select.select([sock], [], [], 0.05)
                if not ready:
                    return None
                raw = sock.recv(_CAN_FD_FRAME.size if fd else _CAN_FRAME.size)
                if fd and len(raw) == _CAN_FD_FRAME.size:
                    can_id, length, flags, data = _CAN_FD_FRAME.unpack(raw)
                    return data[: min(length, 64)], (can_id, flags)
                can_id, dlc, data = _CAN_FRAME.unpack(raw[: _CAN_FRAME.size].ljust(_CAN_FRAME.size, b"\0"))
                return data[: min(dlc, 8)], ((can_id, None) if fd else can_id)

            return read

        def writer(sock: socket.socket) -> Callable[[bytes, object], None]:
            def write(data: bytes, identity: object) -> None:
                if fd:
                    can_id, flags = identity  # type: ignore[misc]
                    if flags is not None:
                        sock.send(_CAN_FD_FRAME.pack(can_id, len(data), flags, data.ljust(64, b"\0")))
                        return
                    sock.send(_CAN_FRAME.pack(can_id, len(data), data.ljust(8, b"\0")))
                    return
                sock.send(_CAN_FRAME.pack(identity, len(data), data.ljust(8, b"\0")))  # type: ignore[arg-type]

            return write

        for direction, source, sink in ((UP, a, b), (DOWN, b, a)):
            delivery = self._new_delivery()
            for thread in (
                _Pump(f"can-{direction}-read", reader(source), self.engine, direction, delivery, self._stop),
                _Deliverer(f"can-{direction}-write", delivery, writer(sink), self._stop),
            ):
                thread.start()
                self._threads.append(thread)

    def ready(self) -> str:
        return f"ready can {self.driver_side} {self.substrate_side}"

    def _close(self) -> None:
        for sock in self._socks:
            sock.close()


class RawRelay(Relay):
    """Two Ethernet interfaces — `ethercat-rs`'s master on one and the ring on the other — relayed frame by frame. A frame is
    the whole Ethernet frame to the engine."""

    transport = "raw"

    def __init__(self, engine: Engine, driver_side: str, substrate_side: str, ethertype: int = 0x88A4):
        super().__init__(engine)
        self.driver_side, self.substrate_side, self.ethertype = driver_side, substrate_side, ethertype
        self._socks: list[socket.socket] = []

    def _open(self) -> None:
        socks = []
        for interface in (self.driver_side, self.substrate_side):
            try:
                sock = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(self.ethertype))
                sock.bind((interface, self.ethertype))
            except (AttributeError, OSError) as error:
                raise SystemExit(
                    f"faultproxy: cannot open a raw socket on `{interface}`: {error}. This needs Linux and CAP_NET_RAW over the "
                    "interface's network namespace (`unshare -rn` without root), and an interface of that name."
                ) from error
            socks.append(sock)
        a, b = self._socks = socks

        def reader(sock: socket.socket) -> Callable[[], object]:
            def read() -> object:
                ready, _, _ = select.select([sock], [], [], 0.05)
                if not ready:
                    return None
                return sock.recv(1518), None

            return read

        def writer(sock: socket.socket) -> Callable[[bytes, object], None]:
            def write(data: bytes, _meta: object) -> None:
                sock.send(data)

            return write

        for direction, source, sink in ((UP, a, b), (DOWN, b, a)):
            delivery = self._new_delivery()
            for thread in (
                _Pump(f"raw-{direction}-read", reader(source), self.engine, direction, delivery, self._stop),
                _Deliverer(f"raw-{direction}-write", delivery, writer(sink), self._stop),
            ):
                thread.start()
                self._threads.append(thread)

    def ready(self) -> str:
        return f"ready raw {self.driver_side} {self.substrate_side}"

    def _close(self) -> None:
        for sock in self._socks:
            sock.close()


# ── the kernel's own link emulation ──────────────────────────────────────────


class NetemUnsupported(ValueError):
    """A profile asks the kernel's `netem` for something it cannot do. The message says what, and what to use instead."""


def _milliseconds(seconds: float) -> str:
    return f"{seconds * 1000:g}ms"


def _percent(probability: float) -> str:
    return f"{probability * 100:g}%"


#: A queue deep enough that the faults, and not the queue's own tail drop, are what a driver meets: `netem`'s default is a thousand packets.
_NETEM_LIMIT = "limit 10000"


def netem_arguments(faults: list[Fault]) -> str:
    """The arguments `tc qdisc replace dev DEV root netem` is given for a profile's steady faults, or `NetemUnsupported` naming the fault that cannot be carried.

    What the kernel does is not what the user-space engine does, and the differences are named here and not hidden:

    * **One queue, both directions.** An interface's qdisc sees every packet it transmits and cannot tell a driver's from a substrate's, so a fault scoped to `up:` or `down:` is refused.
    * `delay` and `jitter` are `netem`'s one delay and its jitter. `reorder=P%:gap` is `delay gap reorder (100-P)%`: `netem` sends a share of the packets at once and delays the rest, so a share of 1-P
      are overtaken, which is what the engine's reorder is; it cannot be combined with a `delay` of its own.
    * `loss` and `burst` are `loss` and `loss gemodel` (the Gilbert-Elliott model, a bad state that drops everything); not both.
    * `duplicate=P%` is `duplicate P%`, whose copy is sent at once: a gap is refused. `corrupt=P%` flips one bit of a packet: more bits, or bytes within a span, are refused.
    * `slow=R` is `rate` in bits a second. `truncate` has no equivalent and is refused.
    * `blackhole` is a window in time, which the relay follows by its own clock (`NetemRelay`) and is a loss of every packet for its length. `stall` is refused: it holds packets and lets them go at once, and a
      kernel queue cannot do that, so a stall turned into a blackhole would be a different test under the same name (the stale burst after the window is what it is for).
    * `reset` and `kill` are a TCP connection's and a process's, and are refused.
    """
    delay = jitter = 0.0
    loss = burst = reorder = duplicate = corrupt = slow = None
    for fault in faults:
        if fault.scope != "both":
            raise NetemUnsupported(f"`{fault.scope}:{fault.kind}` is scoped to one direction, and an interface's `netem` queue carries both: give it unscoped, or use a transport relay")
        kind, args = fault.kind, fault.args
        if kind == "delay":
            delay += args[0]
        elif kind == "jitter":
            jitter += args[0]
        elif kind == "loss":
            loss = args[0]
        elif kind == "burst":
            burst = args
        elif kind == "duplicate":
            if args[1]:
                raise NetemUnsupported(f"`duplicate` with a gap of {_milliseconds(args[1])}: `netem` sends the copy at once")
            duplicate = args[0]
        elif kind == "reorder":
            reorder = args
        elif kind == "corrupt":
            probability, bits, span = args
            if bits != 1 or span is not None:
                where = f" within bytes {span[0]} to {span[1]}" if span else ""
                raise NetemUnsupported(f"`corrupt` of {bits} bit{'' if bits == 1 else 's'}{where}: `netem` flips one bit of a packet, anywhere in it")
            corrupt = probability
        elif kind == "slow":
            slow = args[0]
        elif kind == "blackhole":
            continue
        elif kind == "stall":
            raise NetemUnsupported("`stall` holds packets and lets them go at once, which a kernel queue cannot do: `blackhole` is the window that loses them")
        elif kind == "truncate":
            raise NetemUnsupported("`truncate` has no `netem` equivalent: a packet is delivered whole or not at all")
        else:
            raise NetemUnsupported(f"`{kind}` is a connection's or a process's fault, and `netem` carries packets: use a transport relay")
    if loss is not None and burst is not None:
        raise NetemUnsupported("`loss` and `burst` together: `netem` has one loss model at a time")
    parts = [_NETEM_LIMIT]
    if reorder is not None:
        probability, extra = reorder
        if delay:
            raise NetemUnsupported("`reorder` with a `delay` of its own: `netem` reorders by delaying a share of the packets by its one delay value")
        delay = extra if extra is not None else 0.02
    if delay or jitter:
        parts.append(f"delay {_milliseconds(delay)} {_milliseconds(jitter)}" if jitter else f"delay {_milliseconds(delay)}")
    if reorder is not None:
        parts.append(f"reorder {_percent(1.0 - reorder[0])}")
    if loss is not None:
        parts.append(f"loss {_percent(loss)}")
    if burst is not None:
        parts.append(f"loss gemodel {_percent(burst[0])} {_percent(burst[1])}")
    if duplicate is not None:
        parts.append(f"duplicate {_percent(duplicate)}")
    if corrupt is not None:
        parts.append(f"corrupt {_percent(corrupt)}")
    if slow is not None:
        parts.append(f"rate {int(slow * 8)}bit")
    return " ".join(parts)


def private_namespace() -> bool:
    """Whether changing an interface's queue here changes nothing of the machine's own: this process is root in a user namespace of its own (`unshare -rn`, whose `uid_map` is not the identity over every id),
    or a container or VM has said its network is its own (SKYNET_FAULTPROXY_NETEM_OK=1). A kernel relay on a host's own `lo` would put the profile on every program the machine runs."""
    if os.environ.get("SKYNET_FAULTPROXY_NETEM_OK") == "1":
        return True
    try:
        with open("/proc/self/uid_map", encoding="ascii") as handle:
            return handle.read().split()[:3] != ["0", "0", "4294967295"]
    except OSError:
        return False


class NetemRelay(Relay):
    """The kernel's link emulation (`tc qdisc ... netem`) on one interface, for a substrate the relays above cannot stand in front of: DDS names its peers in discovery messages, sends to multicast groups and to ports
    nobody configured, and ROS 2 and the vendors' SDKs build on it. Every packet the interface carries meets the profile, in both directions at once, and nothing about the driver or the substrate is told where to connect.

    The substrate and the driver must share the interface (`lo` in a network namespace of their own, which `tools/seed-defects/in-netns.sh` makes) and nothing else may use it: the profile is the interface's, so a
    tap that the contract reads the arm's position through has to be a pipe and not a socket. `netem_arguments` says what the kernel can and cannot be asked for. The window in time (`blackhole`) is followed here
    by the profile's `@` clock, which the runner starts (`Engine.start`), and is a loss of every packet for its length."""

    transport = "netem"

    def __init__(self, engine: Engine, device: str = "lo"):
        super().__init__(engine)
        self.device = device
        #: What stopped the relay from following the profile (a `tc` that failed), raised again by `refresh` so a run that depends on the profile cannot go on with a clean link and call it a fault.
        self.failure: BaseException | None = None
        self._applied: str | None = None
        self._spec: str | None = None
        self._steady = _NETEM_LIMIT
        self._windows: list[tuple[float, float]] = []
        self._lock = threading.Lock()

    @staticmethod
    def accepts(spec: str) -> None:
        """Raises `ProfileError` or `NetemUnsupported` if the kernel cannot be asked for this profile, and says nothing otherwise. Asked before a profile is given to the engine, so that a refused one changes nothing."""
        netem_arguments(parse_profile(spec))

    def _tc(self, *arguments: str, check: bool = True) -> None:
        try:
            result = subprocess.run(["tc", *arguments], capture_output=True, text=True)
        except OSError as error:
            raise SystemExit(f"faultproxy: cannot run `tc`: {error}. The kernel relay needs iproute2 (`tc`) and a kernel with `sch_netem`.") from error
        if check and result.returncode != 0:
            raise SystemExit(
                f"faultproxy: `tc {' '.join(arguments)}` failed: {result.stderr.strip() or result.stdout.strip()}. This needs Linux with `sch_netem` and CAP_NET_ADMIN over "
                f"`{self.device}`'s network namespace (`unshare -rn` without root, which `tools/seed-defects/in-netns.sh` does)."
            )

    def _set(self, arguments: str) -> None:
        if arguments != self._applied:
            self._tc("qdisc", "replace", "dev", self.device, "root", "netem", *arguments.split())
            self._applied = arguments

    def _open(self) -> None:
        # The refusal first: whether the machine's own network is at risk matters more than whether the tool is installed.
        if not private_namespace():
            raise SystemExit(
                f"faultproxy: refusing to change `{self.device}`'s queue: this process is not in a user namespace of its own, so `{self.device}` may be the machine's. Run it under "
                "`sh tools/seed-defects/in-netns.sh` (an unprivileged network namespace), or set SKYNET_FAULTPROXY_NETEM_OK=1 inside a container or VM whose network is its own."
            )
        if shutil.which("tc") is None:
            raise SystemExit("faultproxy: `tc` is not installed. The kernel relay needs iproute2 and a kernel with `sch_netem`.")
        self._follow()

    def ready(self) -> str:
        return f"ready netem {self.device}"

    def _close(self) -> None:
        self._tc("qdisc", "del", "dev", self.device, "root", check=False)

    def _follow(self) -> None:
        """Put on the interface what the profile asks for at this instant: its steady faults, or nothing at all inside one of its windows."""
        with self._lock:
            spec, faults, started = self.engine.current()
            if spec != self._spec:
                self._steady = netem_arguments(faults)
                self._windows = [(fault.args[1], fault.args[0]) for fault in faults if fault.kind == "blackhole"]
                self._spec = spec
            dark = False
            if started is not None:
                elapsed = monotonic() - started
                dark = any(start <= elapsed < start + length for start, length in self._windows)
            self._set(f"{_NETEM_LIMIT} loss 100%" if dark else self._steady)

    def refresh(self) -> None:
        """Have the interface carry the engine's profile now, and not within the next poll. A runner that has just changed the profile calls it before it relies on the link being as the profile says."""
        if self.failure is not None:
            raise self.failure
        self._follow()

    def _events(self) -> None:
        while not self._stop.is_set():
            try:
                self._follow()
            except (ValueError, SystemExit) as error:
                # Not raised in a thread nobody joins, where it would end the thread and leave the last queue in place: the link would look as the profile says until the next one and be taken for it.
                self.failure = error
                return
            time.sleep(0.01)
