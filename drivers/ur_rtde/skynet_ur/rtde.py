"""RTDE — Universal Robots' Real-Time Data Exchange, on the wire.

Every byte of this was **measured against URSim**, Universal Robots' own controller software, rather
than transcribed from memory of a document. `DRY-COVERAGE-PLAN.md` F15 is the reason that mattered
enough to arrange: a hand-transcribed protocol that has never been executed is the shape that
session was right to refuse, and the answer is not to be more careful, it is to have something real
to talk to.

# The frame

Three bytes of header, big-endian throughout — which is worth saying because CANopen next door is
little-endian and a driver author reading both in one afternoon will mix them up once.

```
 size : u16   the whole packet, header included
 type : u8    one of the constants below
 payload
```

| Type | Name | Request payload | Reply |
|---|---|---|---|
| 86 `V` | `REQUEST_PROTOCOL_VERSION` | `u16` version | `u8` accepted |
| 118 `v` | `GET_URCONTROL_VERSION` | — | four `u32`: major, minor, bugfix, build |
| 79 `O` | `CONTROL_PACKAGE_SETUP_OUTPUTS` | `f64` frequency, then names | `u8` recipe id, then the types |
| 73 `I` | `CONTROL_PACKAGE_SETUP_INPUTS` | names | `u8` recipe id, then the types |
| 83 `S` | `CONTROL_PACKAGE_START` | — | `u8` accepted |
| 80 `P` | `CONTROL_PACKAGE_PAUSE` | — | `u8` accepted |
| 85 `U` | `DATA_PACKAGE` | `u8` recipe id, then the values | — |
| 77 `M` | `TEXT_MESSAGE` | — | the controller talking |

# Two things the measurement taught that a reading might not have

**`TEXT_MESSAGE` arrives unsolicited, interleaved with replies.** The first thing a fresh URSim says
is *"Safety Setup has not been confirmed yet, therefore no data will be send yet"*, and it says it in
the middle of the handshake. A client that treats the next packet as its reply is then **off by one
for the rest of the session** — it reads the version reply as the outputs reply, the outputs reply as
the inputs reply, and every one of them parses. That is not a hypothetical: it is what the first
probe against URSim did, and the symptom was a recipe id of zero and a types list of spaces.
:meth:`Rtde.transact` therefore reads until the type it asked for, and hands every text message to a
callback rather than dropping it.

**The output stream is a firehose and a client that does not drain it reads the past.** Outputs
arrive at the negotiated frequency — 125 Hz here — whether or not anybody is reading, and a socket
buffer holds seconds of them. The second probe commanded a motion, sampled every fortieth cycle, and
concluded the robot had not moved; the robot had moved, and the packets being read were a second old.
:meth:`Rtde.newest` drains to the last complete packet each call, which is §47.6's latest-wins rule
arriving from a completely different direction: for a control loop, an old sample is not data.
"""

from __future__ import annotations

import socket
import struct
from dataclasses import dataclass

__all__ = [
    "REQUEST_PROTOCOL_VERSION",
    "GET_URCONTROL_VERSION",
    "TEXT_MESSAGE",
    "SETUP_OUTPUTS",
    "SETUP_INPUTS",
    "START",
    "PAUSE",
    "DATA_PACKAGE",
    "RtdeError",
    "Rtde",
    "decode",
    "encode",
    "width_of",
]

REQUEST_PROTOCOL_VERSION = 86
GET_URCONTROL_VERSION = 118
TEXT_MESSAGE = 77
SETUP_OUTPUTS = 79
SETUP_INPUTS = 73
START = 83
PAUSE = 80
DATA_PACKAGE = 85

#: How many bytes each RTDE type occupies, and how to read it. `IN_USE` is what the controller
#: answers for a variable it does not have, and it is a **refusal wearing a type's clothes** — a
#: client that treated it as a width would slide every field after it.
_TYPES: dict[str, tuple[int, str]] = {
    "BOOL": (1, ">?"),
    "UINT8": (1, ">B"),
    "INT32": (4, ">i"),
    "UINT32": (4, ">I"),
    "UINT64": (8, ">Q"),
    "DOUBLE": (8, ">d"),
    "VECTOR3D": (24, ">3d"),
    "VECTOR6D": (48, ">6d"),
    "VECTOR6INT32": (24, ">6i"),
    "VECTOR6UINT32": (24, ">6I"),
}


class RtdeError(Exception):
    """A refusal from the controller, or a frame that is not one."""


class RegistersInUse(RtdeError):
    """The controller answered `IN_USE` for a variable in a recipe.

    Its own class because it is the one refusal that can clear by itself: RTDE's general-purpose
    registers belong to one session at a time, and a controller frees a closed session's a moment
    after its socket goes. `bridge.py`'s bring-up waits for that and for nothing else.
    """


def width_of(type_name: str) -> int:
    """Bytes one value of this type occupies."""
    if type_name not in _TYPES:
        raise RtdeError(
            f"`{type_name}` is not an RTDE type. `IN_USE` here means the controller does not have "
            "the variable that was asked for — check the name against this controller's version, "
            "because the set grew between PolyScope releases."
        )
    return _TYPES[type_name][0]


def decode(types: list[str], payload: bytes) -> list:
    """One value per type, in order."""
    expected = sum(width_of(name) for name in types)
    if len(payload) != expected:
        raise RtdeError(
            f"a data package carried {len(payload)} bytes and this recipe is {expected}. A recipe "
            "and a package that disagree is a client reading one variable's bytes as another's."
        )
    out = []
    at = 0
    for name in types:
        size, fmt = _TYPES[name]
        values = struct.unpack(fmt, payload[at : at + size])
        out.append(values[0] if len(values) == 1 else list(values))
        at += size
    return out


def encode(types: list[str], values: list) -> bytes:
    """The inverse. Used for the input recipe a control script reads."""
    if len(types) != len(values):
        raise RtdeError(f"{len(types)} type(s) and {len(values)} value(s)")
    out = bytearray()
    for name, value in zip(types, values):
        width_of(name)  # refuses an unknown type by name before anything is packed
        _size, fmt = _TYPES[name]
        if isinstance(value, (list, tuple)):
            out += struct.pack(fmt, *value)
        else:
            out += struct.pack(fmt, value)
    return bytes(out)


@dataclass
class Recipe:
    """A named set of variables the controller has agreed to exchange."""

    identifier: int
    names: list[str]
    types: list[str]


class Rtde:
    """One RTDE session."""

    def __init__(self, host: str, port: int = 30004, timeout_s: float = 5.0) -> None:
        self._socket = socket.create_connection((host, port), timeout=timeout_s)
        self._socket.settimeout(timeout_s)
        self._buffer = b""
        self.output: Recipe | None = None
        self.input: Recipe | None = None
        #: Everything the controller said, in order. Kept rather than printed: a text message during
        #: a handshake is the reason a later step fails, and it is usually read afterwards.
        self.messages: list[str] = []

    # ── framing ──────────────────────────────────────────────────────────────

    def _send(self, packet_type: int, payload: bytes = b"") -> None:
        self._socket.sendall(struct.pack(">HB", 3 + len(payload), packet_type) + payload)

    def _packet(self) -> tuple[int, bytes]:
        """The next whole packet. Nothing is consumed from the buffer until all of it is there.

        That is the property the non-blocking drain in :meth:`newest` depends on: a read that runs
        out of data mid-packet raises out of here with the partial packet still buffered, and the
        next drain completes it. The version before consumed the header first, so a packet that
        straddled the moment the socket ran dry lost its header, and the stream was read from the
        wrong offset from then on — a robot that looked frozen (DRY-COVERAGE-PLAN.md F75).
        """
        while True:
            if len(self._buffer) >= 3:
                size, packet_type = struct.unpack(">HB", self._buffer[:3])
                if size < 3:
                    raise RtdeError(f"a packet claiming {size} bytes cannot hold its own header")
                if len(self._buffer) >= size:
                    body = self._buffer[3:size]
                    self._buffer = self._buffer[size:]
                    return packet_type, body
            chunk = self._socket.recv(4096)
            if not chunk:
                raise RtdeError("the controller closed the connection")
            self._buffer += chunk

    def _note(self, payload: bytes) -> None:
        # `<level><message><source>`, with lengths in front of each on protocol 2. The message is
        # the useful part and the rest is not worth a parser that would break on a version change.
        text = payload.decode("utf-8", errors="replace")
        self.messages.append(text.strip("\x00").strip())

    def transact(self, packet_type: int, payload: bytes = b"") -> bytes:
        """Send, then read until the reply of that type. Text messages are collected, not dropped."""
        self._send(packet_type, payload)
        for _ in range(64):
            got, body = self._packet()
            if got == TEXT_MESSAGE:
                self._note(body)
                continue
            if got != packet_type:
                raise RtdeError(
                    f"asked for {packet_type} and the controller answered {got}. This client reads "
                    "past text messages on purpose; anything else out of order is a session two "
                    "clients are sharing."
                )
            return body
        raise RtdeError("the controller sent nothing but text messages")

    # ── the session ──────────────────────────────────────────────────────────

    def negotiate(self, version: int = 2) -> None:
        accepted = self.transact(REQUEST_PROTOCOL_VERSION, struct.pack(">H", version))
        if accepted != b"\x01":
            raise RtdeError(
                f"this controller refused RTDE protocol version {version}. Version 1 has no "
                "frequency field on the output setup and this client does not speak it."
            )

    def controller_version(self) -> tuple[int, int, int, int]:
        body = self.transact(GET_URCONTROL_VERSION)
        if len(body) != 16:
            raise RtdeError(f"a version reply is four u32 and this one is {len(body)} bytes")
        return struct.unpack(">IIII", body)

    def setup_outputs(self, names: list[str], frequency_hz: float) -> Recipe:
        body = self.transact(
            SETUP_OUTPUTS, struct.pack(">d", frequency_hz) + ",".join(names).encode()
        )
        types = body[1:].decode().split(",")
        self._refuse_unavailable(names, types, "output")
        self.output = Recipe(body[0], names, types)
        return self.output

    def setup_inputs(self, names: list[str]) -> Recipe:
        body = self.transact(SETUP_INPUTS, ",".join(names).encode())
        types = body[1:].decode().split(",")
        self._refuse_unavailable(names, types, "input")
        self.input = Recipe(body[0], names, types)
        return self.input

    @staticmethod
    def _refuse_unavailable(names: list[str], types: list[str], which: str) -> None:
        missing = [name for name, kind in zip(names, types) if kind == "IN_USE"]
        if missing:
            raise RegistersInUse(
                f"the controller answered `IN_USE` for {which} variable(s) "
                f"{', '.join(missing)}, which means one of two things and the first is far more "
                "common: **another RTDE client already holds them** — the general-purpose "
                "registers are allocated to one session at a time, and a driver that was not "
                "closed cleanly keeps them for a few seconds after its socket goes — or this "
                "controller does not have the variable at all, which is a name from a newer "
                "PolyScope. Either way the recipe is refused here rather than used, because RTDE "
                "answers `IN_USE` *in place of a type* and a client that did not look would decode "
                "every field after it at the wrong offset."
            )
        if len(types) != len(names):
            raise RtdeError(
                f"{len(names)} {which} name(s) were asked for and {len(types)} type(s) came back"
            )

    def start(self) -> None:
        if self.transact(START) != b"\x01":
            raise RtdeError(
                "the controller refused to start the data exchange. A recipe that was never set "
                "up, or another client already holding the session, are the two causes."
            )

    def pause(self) -> None:
        self.transact(PAUSE)

    def write(self, values: list) -> None:
        """One input data package. The control script reads these as registers."""
        if self.input is None:
            raise RtdeError("no input recipe has been set up")
        self._send(
            DATA_PACKAGE, bytes([self.input.identifier]) + encode(self.input.types, values)
        )

    def newest(self) -> dict | None:
        """The most recent complete output package, discarding everything behind it.

        **Draining is the point.** Outputs arrive at the negotiated frequency whether or not
        anybody reads them, so a control loop that took the next packet each cycle would fall
        further behind every cycle and act on a sample that gets older without bound. This is
        §47.6's latest-wins rule arriving from another direction: for a loop, an old sample is not
        data — and a driver that returned one would report a robot that is not where it says.
        """
        if self.output is None:
            raise RtdeError("no output recipe has been set up")
        latest: dict | None = None
        self._socket.settimeout(0.0)
        try:
            while True:
                try:
                    packet_type, body = self._packet()
                except (BlockingIOError, TimeoutError):
                    break
                except OSError:
                    break
                if packet_type == TEXT_MESSAGE:
                    self._note(body)
                    continue
                if packet_type != DATA_PACKAGE or body[0] != self.output.identifier:
                    continue
                latest = dict(zip(self.output.names, decode(self.output.types, body[1:])))
        finally:
            self._socket.settimeout(5.0)
        return latest

    def close(self) -> None:
        try:
            self._socket.close()
        except OSError:
            pass
