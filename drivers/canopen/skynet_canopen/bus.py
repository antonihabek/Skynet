"""Where a CAN frame goes: a real SocketCAN interface, or an in-process bus that needs no kernel.

Two transports behind one method pair, and the second one is why this driver can be exercised at
all on the machines that build it.

`DRY-COVERAGE-PLAN.md`'s Phase 2 table gives this driver its substrate as *"Linux `vcan` + CiA402
slave sim"*, and `vcan` is a kernel module: it needs `modprobe`, which needs a kernel with `CONFIG_CAN`
and a container with the privilege to load it. The machine this driver was written on has neither —
`socket.AF_CAN` exists in CPython and the kernel answers `EAFNOSUPPORT` — and a great many CI
runners are the same. So the frames go through :class:`LoopbackBus` instead, which is the identical
protocol over a Python list.

**What that costs, stated rather than implied.** A loopback bus is not a bus. It has no arbitration,
no bit timing, no error frames, no bus-off, and no bandwidth — so it can show that this driver
encodes CiA 402 correctly and it can show nothing whatever about whether eight axes fit in a
millisecond at 1 Mbit. That second question is the one row in the plan's table under *not
dry-verifiable*, and it stays there.

:class:`SocketCanBus` is the same driver against the real thing, and it is what runs on a machine
with `vcan0` up. It is written and it is **not exercised here** — the module refuses to construct on
a kernel without `AF_CAN` rather than pretending, and the selftest reports that it was skipped by
name instead of passing quietly.
"""

from __future__ import annotations

import socket
import struct
from collections import deque

from .frames import Frame

__all__ = ["Bus", "LoopbackBus", "SocketCanBus", "can_available"]


class Bus:
    """Two methods, and one property. Everything above this layer is written against them."""

    #: Whether a frame that is not here *now* will never arrive.
    #:
    #: True on a loopback bus, where delivery happens inside :meth:`recv` and there is nothing to
    #: wait for; false on a real one, where a drive answers some microseconds later. The bridge uses
    #: it to decide whether an empty read means *not yet* or *never*, which without it is either a
    #: deadline burned on every synchronous test or a real bus given up on after one empty poll.
    synchronous = False

    def send(self, frame: Frame) -> None:
        raise NotImplementedError

    def recv(self, timeout_s: float = 0.0) -> Frame | None:
        """The next frame, or ``None``. Never blocks longer than ``timeout_s``."""
        raise NotImplementedError

    def close(self) -> None:
        pass


class LoopbackBus(Bus):
    """An in-process bus: everything sent is delivered to every *other* attached endpoint.

    Two endpoints share one :class:`LoopbackBus` and each holds its own queue, so a master does not
    receive its own requests and a drive does not receive its own replies. That is not decoration —
    a loopback that echoed to the sender would let the bridge pass its own SDO request back to
    itself as a response, and the shape of the two is close enough that it would sometimes parse.
    """

    def __init__(self) -> None:
        self._queues: list[deque[Frame]] = []
        self._pumps: list[object] = []
        self._pumping = False

    def endpoint(self) -> "LoopbackEndpoint":
        """A handle with its own inbox. Attach one per party on the bus."""
        queue: deque[Frame] = deque()
        self._queues.append(queue)
        return LoopbackEndpoint(self, queue)

    def attach(self, pump) -> None:
        """Register something that consumes its inbox and may answer — a simulated drive's ``poll``.

        Without this the bus would need threads. A real CAN bus is concurrent: the master sends and
        the drive answers while the master is still in its read. Here the *reader* runs the other
        parties once when its own inbox is empty, which produces the same ordering with no
        scheduler, no sleep and no flakiness — and it is why the whole of this driver's suite is
        deterministic.
        """
        self._pumps.append(pump)

    def pump(self) -> None:
        """Let every attached party handle what is waiting for it. Re-entrancy is refused."""
        if self._pumping:
            return
        self._pumping = True
        try:
            for pump in self._pumps:
                pump()
        finally:
            self._pumping = False

    def _publish(self, frame: Frame, sender: deque[Frame]) -> None:
        for queue in self._queues:
            if queue is not sender:
                queue.append(frame)


class LoopbackEndpoint(Bus):
    """One party's view of a :class:`LoopbackBus`."""

    synchronous = True

    def __init__(self, bus: LoopbackBus, inbox: deque[Frame]) -> None:
        self._bus = bus
        self._inbox = inbox

    def send(self, frame: Frame) -> None:
        self._bus._publish(frame, self._inbox)

    def recv(self, timeout_s: float = 0.0) -> Frame | None:
        # `timeout_s` is accepted and ignored: this bus is synchronous, so a frame that is not here
        # now will not arrive by waiting, and sleeping would only make a test slower.
        del timeout_s
        if not self._inbox:
            self._bus.pump()
        return self._inbox.popleft() if self._inbox else None


def can_available() -> bool:
    """Whether this kernel will give out a raw CAN socket at all."""
    if not hasattr(socket, "AF_CAN"):
        return False
    try:
        probe = socket.socket(socket.AF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
    except OSError:
        return False
    probe.close()
    return True


class SocketCanBus(Bus):
    """Linux SocketCAN, on a named interface — `can0`, or `vcan0` for a virtual one.

    The frame layout is `struct can_frame`: a 32-bit identifier, a length, three padding bytes and
    eight data bytes, always sixteen bytes on the wire whatever the length says. Written out for
    `frames.py`'s reason, and it is one `struct` format string.
    """

    #: `<` little-endian, `I` can_id, `B` len, `3x` pad, `8s` data.
    _LAYOUT = "<IB3x8s"

    def __init__(self, interface: str = "vcan0") -> None:
        if not can_available():
            raise OSError(
                "this kernel has no CAN support, so there is no interface to bind to. On Linux "
                "that is `modprobe vcan && ip link add dev vcan0 type vcan && ip link set up "
                "vcan0`; in a container it also needs the privilege to load a module. "
                "`LoopbackBus` is the substitute, and it is a substitute — it proves the protocol "
                "and says nothing about bandwidth."
            )
        self._socket = socket.socket(socket.AF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
        self._socket.bind((interface,))
        self._socket.settimeout(0.0)

    def send(self, frame: Frame) -> None:
        payload = frame.data + bytes(8 - len(frame.data))
        self._socket.send(
            struct.pack(self._LAYOUT, frame.arbitration_id, len(frame.data), payload)
        )

    def recv(self, timeout_s: float = 0.0) -> Frame | None:
        self._socket.settimeout(timeout_s if timeout_s > 0 else 0.0)
        try:
            raw = self._socket.recv(16)
        except (BlockingIOError, TimeoutError):
            return None
        can_id, length, data = struct.unpack(self._LAYOUT, raw)
        # The top bits carry the extended/RTR/error flags; this driver speaks 11-bit standard
        # frames, and a frame with any of them set is somebody else's traffic rather than ours.
        if can_id & 0xE0000000:
            return None
        return Frame(can_id & 0x7FF, data[:length])

    def close(self) -> None:
        self._socket.close()
