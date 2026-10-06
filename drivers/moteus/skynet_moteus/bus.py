"""Where a CAN-FD frame goes: a kernel SocketCAN interface, or the CANopen driver's in-process bus carrying `FdFrame`s.

The bus underneath is this repository's own (`skynet_canopen.bus`): its `Bus` contract and its `LoopbackBus` are written against *a frame*, not against
eight bytes — the eight-byte rule lives in `skynet_canopen.frames.Frame`'s constructor and nowhere in the bus — so the same loopback carries the 64-byte,
29-bit-identifier frames a moteus speaks, and `models.json` stages the CANopen package beside this one for that reason. What it did not have is a socket
that speaks CAN-FD, which is `SocketCanFdBus`.

A moteus needs both halves of "FD": the **data** (a command and its query are more than eight bytes: the maker's example is sixteen) and the
**identifier** (a frame asking for a reply is `0x8001`, which no 11-bit identifier holds). `docs/protocol/can.md`: *the CAN frame must be an extended one
if the ID would be greater than 0x7fff*; this driver sends an extended frame for any identifier an 11-bit one cannot carry.
"""

from __future__ import annotations

import socket
import struct
from dataclasses import dataclass

from skynet_canopen.bus import Bus, LoopbackBus, LoopbackEndpoint, can_available

__all__ = ["Bus", "FdFrame", "LoopbackBus", "LoopbackEndpoint", "SocketCanFdBus", "can_fd_available"]

_EFF = 0x80000000
_RTR = 0x40000000
_ERR = 0x20000000
#: `struct canfd_frame`: identifier, length, flags (BRS|ESI), two reserved bytes, 64 bytes of data.
_FD = "<IBB2x64s"
#: `struct can_frame`, which a socket that has asked for FD frames still returns for a classic one.
_CLASSIC = "<IB3x8s"
_BRS = 0x01


@dataclass(frozen=True)
class FdFrame:
    """One CAN-FD frame: an identifier (29 bits when ``extended``) and up to 64 bytes. ``data`` is exactly the bytes sent: padding is `protocol.pad`'s."""

    arbitration_id: int
    data: bytes
    extended: bool = False

    def __post_init__(self) -> None:
        limit = 0x1FFFFFFF if self.extended else 0x7FF
        if not 0 <= self.arbitration_id <= limit:
            raise ValueError(f"{self.arbitration_id:#x} is not a {'29' if self.extended else '11'}-bit identifier")
        if len(self.data) > 64:
            raise ValueError("a CAN-FD frame carries at most 64 bytes")


def can_fd_available() -> bool:
    """Whether this kernel gives out a raw CAN socket that will carry FD frames. (An interface that is not FD-capable — a `vcan` left at its classic MTU
    of 16 — refuses the first FD frame at `send`, loudly.)"""
    return can_available() and hasattr(socket, "CAN_RAW_FD_FRAMES")


class SocketCanFdBus(Bus):
    """Linux SocketCAN, FD frames on, on a named interface. A virtual one needs `ip link set vcan0 mtu 72` for an FD frame to fit."""

    def __init__(self, interface: str = "vcan0") -> None:
        if not can_fd_available():
            raise OSError(
                "this kernel has no CAN-FD support, so there is no interface to bind to. On Linux that is `modprobe vcan && ip link add dev vcan0 "
                "type vcan && ip link set vcan0 mtu 72 && ip link set up vcan0` (the MTU is what lets a 64-byte frame through); in a container it also "
                "needs the privilege to load a module. `LoopbackBus` is the substitute, and it is a substitute."
            )
        self._socket = socket.socket(socket.AF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
        self._socket.setsockopt(socket.SOL_CAN_RAW, socket.CAN_RAW_FD_FRAMES, 1)
        self._socket.bind((interface,))
        self._socket.settimeout(0.0)

    def send(self, frame: FdFrame) -> None:
        identifier = frame.arbitration_id | (_EFF if frame.extended else 0)
        payload = frame.data.ljust(64, b"\x00")
        self._socket.send(struct.pack(_FD, identifier, len(frame.data), _BRS, payload))

    def recv(self, timeout_s: float = 0.0) -> FdFrame | None:
        self._socket.settimeout(timeout_s if timeout_s > 0 else 0.0)
        try:
            raw = self._socket.recv(72)
        except (BlockingIOError, TimeoutError):
            return None
        if len(raw) == 72:
            identifier, length, _, data = struct.unpack(_FD, raw)
        elif len(raw) == 16:
            identifier, length, data = struct.unpack(_CLASSIC, raw)
        else:
            return None
        if identifier & (_RTR | _ERR):
            return None  # a remote request or an error frame is somebody's bus state, not a controller's reply
        extended = bool(identifier & _EFF)
        return FdFrame(identifier & (0x1FFFFFFF if extended else 0x7FF), bytes(data[:length]), extended)

    def close(self) -> None:
        self._socket.close()
