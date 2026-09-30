"""The transports: TCP, UDP, a serial line, SocketCAN and raw Ethernet, each relayed through an `Engine`.

A relay sits between a driver and its substrate. The driver is pointed at the relay instead of the substrate; the relay reads what
each side sends, asks the engine what the link does with it, and delivers the survivors at the times the engine says. A relay
adds nothing of its own: a clean profile is a wire, and what comes out is what went in, in order, at once.

DRY-COVERAGE-PLAN.md item 12.C.4. Python's standard library and no more, as the drivers' own fakes are: `socket` carries `AF_CAN`
and `AF_PACKET`, `pty` makes a serial line without hardware.

## What each transport's faults mean

* **TCP** is a stream. A relay reads whatever the kernel hands it (a *chunk*), so `loss`, `truncate` and `corrupt` are what a
  broken gateway does to a stream and not what a lossy network does to TCP, which retransmits: the driver sees a hole in its
  framing. `delay`, `jitter`, `slow`, `stall`, `blackhole` (a half-open connection: accepted, open, silent), `reset` and `kill`
  (the substrate's process killed and started again) are what a real link and a real peer do.
* **UDP** and **CAN** and **raw Ethernet** are datagrams and frames: every fault applies to one message and is what a real
  network or bus does to it — except `corrupt` on CAN, where a frame with a wrong CRC never reaches a controller, so the fault is
  a gateway's and is named as such.
* **A serial line** is a stream of bytes with no framing and no flow control unless the substrate adds them.

The relay is one thread per direction to read, and one per direction to deliver. The delivery queue is bounded: when a slow
link (`slow=`) or a stall has filled it the relay stops reading, and that is how the writer finds out — a blocking `send` that
blocks, a non-blocking one that says it would.
"""

from __future__ import annotations

import os
import select
import signal
import socket
import struct
import subprocess
import sys
import threading
import time
from typing import Callable

from engine import DOWN, UP, Delivery, Engine

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
            except OSError:
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


def _can_socket(interface: str) -> socket.socket:
    try:
        sock = socket.socket(socket.PF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
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
    through unchanged. A raw CAN socket does not hear its own transmissions, so a relay does not hear itself."""

    transport = "can"

    def __init__(self, engine: Engine, driver_side: str, substrate_side: str):
        super().__init__(engine)
        self.driver_side, self.substrate_side = driver_side, substrate_side
        self._socks: list[socket.socket] = []

    def _open(self) -> None:
        a, b = _can_socket(self.driver_side), _can_socket(self.substrate_side)
        self._socks = [a, b]

        def reader(sock: socket.socket) -> Callable[[], object]:
            def read() -> object:
                ready, _, _ = select.select([sock], [], [], 0.05)
                if not ready:
                    return None
                raw = sock.recv(_CAN_FRAME.size)
                can_id, dlc, data = _CAN_FRAME.unpack(raw[: _CAN_FRAME.size].ljust(_CAN_FRAME.size, b"\0"))
                return data[: min(dlc, 8)], can_id

            return read

        def writer(sock: socket.socket) -> Callable[[bytes, object], None]:
            def write(data: bytes, can_id: object) -> None:
                sock.send(_CAN_FRAME.pack(can_id, len(data), data.ljust(8, b"\0")))  # type: ignore[arg-type]

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
