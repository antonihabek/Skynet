"""The black box — §13 and §26, Python edition.

Transcribed from ``reference/typescript/blackbox.ts``. Every offset, every width and every schema
sentence is read from that file, never from ``corpus/``.

This is the one module where the artifact under test is **bytes**. §31.2 puts the ring's record
layout and the six MCAP channel schemas in the short list of things that stay single-source across
editions, on the grounds that *a black box sealed by one edition must be readable by every other* —
and a format with one implementation has not been specified, only written. So a third
implementation is worth having even though no Python process will ever record a control cycle: it is
the reading that says whether the other two agree because they are right or because they are the
same code twice.

── 64-bit values arrive as decimal strings ──────────────────────────────────

A wall-clock epoch in nanoseconds is about 1.7e18, past 2**53, so a corpus input carrying one as a
JSON number would be comparing two editions on a value neither of them received intact. The oracle
writes them as strings. Python's integers are arbitrary-precision, which makes this the one trap in
this file the language gets *right* for free and every other edition has to work at — and it is
precisely why the Python edition must still parse them from strings rather than reading a number:
the case file is shared, and an edition that read the field as a number would be measuring a
different value from every other reader of the same file.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``encoded[end] & 0xc0``
    ``end`` may be one past the last byte, where JavaScript reads ``undefined`` and coerces the
    expression to ``NaN & 0xc0``, which is ``0`` — so the truncation loop exits and a string that
    fits is written whole. Python raises on that index. :func:`_text` reads a ``0`` past the end,
    which is the same answer for the same reason rather than by accident.

``value >>> 0``
    Every ``u32`` written here is masked. Python's integers do not wrap, so a CRC or a length that
    JavaScript would coerce silently is masked explicitly — and a negative that JavaScript would
    turn into a large positive must be made to do the same, which ``struct`` will not do for you.

``Math.ceil`` on a division
    ``divCeil`` is integer arithmetic in both editions here, so it is written as ``-(-a // b)``
    rather than as ``math.ceil(a / b)`` — the float division of two large integers is not exact, and
    a record size that came out one byte short would be a ring whose every slot overlapped the next.

``struct.pack('<f', nan)``
    Writes ``0000c07f``, the same canonical quiet NaN as ``DataView.setFloat32``. §24 spells "this
    is not measured" as NaN and the corpus holds that byte pattern, so a transcription that
    substituted a zero would report an unmeasured joint as a joint at rest.
"""

from __future__ import annotations

import hashlib
import struct
from typing import Any, Final

from .canonical import canonical

__all__ = [
    "CYCLE_FIXED",
    "EVENT_CODES",
    "FORMAT_VERSION",
    "RECORD_SCHEMA",
    "GAP_REASONS",
    "GUARDS",
    "KINDS",
    "MAX_STREAMS",
    "MAX_TRIPPED",
    "PREFIX",
    "RESPONSES",
    "SKEW_UNMEASURABLE",
    "crc32",
    "crc32_final",
    "crc32_of",
    "encode_header",
    "encode_record",
    "header_to_json",
    "hex_of",
    "opcodes",
    "record_size",
    "records_to_json",
    "schemas_to_json",
    "seal_to_json",
    "wire",
    "write_mcap",
]

# ── CRC-32 ───────────────────────────────────────────────────────────────────
#
# Written out rather than taken from `zlib`, for `canonical.py`'s reason: a check computed by the
# same library on both sides of a comparison is not a check. `zlib.crc32` would almost certainly
# agree; "almost certainly" is what a corpus exists to replace.


def _crc_table() -> tuple[int, ...]:
    table = []
    for i in range(256):
        crc = i
        for _ in range(8):
            crc = (0xEDB88320 ^ (crc >> 1)) if crc & 1 else (crc >> 1)
        table.append(crc & 0xFFFFFFFF)
    return tuple(table)


_CRC_TABLE: Final = _crc_table()


def crc32(data: bytes, seed: int = 0xFFFFFFFF) -> int:
    """The running CRC-32 state over ``data``, seeded. Not the finished value — see below."""
    state = seed
    for byte in data:
        state = (_CRC_TABLE[(state ^ byte) & 0xFF] ^ (state >> 8)) & 0xFFFFFFFF
    return state


def crc32_final(state: int) -> int:
    """The finished value: the running state, inverted."""
    return (state ^ 0xFFFFFFFF) & 0xFFFFFFFF


def crc32_of(data: bytes) -> int:
    """The CRC-32 of one buffer, start to finish."""
    return crc32_final(crc32(data))


# ── The record ───────────────────────────────────────────────────────────────

PREFIX: Final = 24
CYCLE_FIXED: Final = 176
MAX_TRIPPED: Final = 8
MAX_STREAMS: Final = 6
STREAM_NAME: Final = 16
EVENT_SUBJECT: Final = 64
EVENT_DETAIL: Final = 96
NO_CAPABILITY: Final = 0xFFFF

_FRAME_BODY: Final = 52 + MAX_STREAMS * (STREAM_NAME + 8)
_EVENT_BODY: Final = 4 + EVENT_SUBJECT + EVENT_DETAIL
_GAP_BODY: Final = 24

#: Guard names in evaluation order (§7.1). A guard's wire code is its one-based position here.
GUARDS: Final = (
    "watchdog",
    "envelope",
    "rate_limit",
    "workspace",
    "self_collision",
    "balance",
    "proximity",
    "mode_gate",
)

#: Responses weakest to strongest; the wire code is the one-based position, 0 meaning none.
RESPONSES: Final = ("clamp", "hold", "stop_cat2", "stop_cat1", "stop_cat0")

KINDS: Final = {"cycle": 1, "frame": 2, "event": 3, "gap": 4}

EVENT_CODES: Final = (
    "lease_granted",
    "lease_released",
    "lease_preempted",
    "estop",
    "estop_cleared",
    "package_started",
    "package_stopped",
    "grant_changed",
    "ota",
    "clock_step",
    "recording_started",
    "sealed",
    "config_changed",
    "admission",
    "skill_transition",
)

GAP_REASONS: Final = ("writer_behind", "encoder_behind", "not_recording", "write_failed")


def _div_ceil(value: int, by: int) -> int:
    """Integer division rounding up. See the header: never through a float."""
    return -(-value // by)


def _index_of(items: tuple[str, ...], value: Any) -> int:
    """``Array.prototype.indexOf`` — ``-1`` for anything not in the list."""
    return items.index(value) if isinstance(value, str) and value in items else -1


def record_size(shape: dict[str, int]) -> int:
    cycle = (
        CYCLE_FIXED
        + shape["channels"] * 13
        + _div_ceil(shape["channels"], 8)
        + shape["joints"] * 12
    )
    largest = max(cycle, PREFIX + _FRAME_BODY, PREFIX + _EVENT_BODY, PREFIX + _GAP_BODY)
    return _div_ceil(largest, 8) * 8


def _cycle_offsets(shape: dict[str, int]) -> dict[str, int]:
    desired = CYCLE_FIXED
    command = desired + shape["channels"] * 4
    mask = command + shape["channels"] * 8
    clamped = mask + shape["channels"]
    position = clamped + _div_ceil(shape["channels"], 8)
    velocity = position + shape["joints"] * 4
    effort = velocity + shape["joints"] * 4
    return {
        "desired": desired,
        "command": command,
        "mask": mask,
        "clamped": clamped,
        "position": position,
        "velocity": velocity,
        "effort": effort,
    }


class _Cursor:
    """A little-endian writer over a fixed buffer, so every offset here reads like the Rust."""

    __slots__ = ("bytes",)

    def __init__(self, size: int) -> None:
        self.bytes = bytearray(size)

    def _put(self, at: int, packed: bytes) -> None:
        self.bytes[at : at + len(packed)] = packed

    def u8(self, at: int, value: int) -> None:
        self._put(at, struct.pack("<B", value & 0xFF))

    def u16(self, at: int, value: int) -> None:
        self._put(at, struct.pack("<H", value & 0xFFFF))

    def u32(self, at: int, value: int) -> None:
        self._put(at, struct.pack("<I", value & 0xFFFFFFFF))

    def i32(self, at: int, value: int) -> None:
        self._put(at, struct.pack("<i", _to_i32(value)))

    def u64(self, at: int, value: int) -> None:
        self._put(at, struct.pack("<Q", value & 0xFFFFFFFFFFFFFFFF))

    def f32(self, at: int, value: float) -> None:
        self._put(at, struct.pack("<f", _to_f32(value)))

    def f64(self, at: int, value: float) -> None:
        self._put(at, struct.pack("<d", float(value)))

    def raw(self, at: int, source: bytes) -> None:
        self._put(at, source)

    def text(self, at: int, width: int, value: str) -> None:
        """Write ``value`` into ``width`` bytes, NUL-padded, truncated on a character boundary.

        The boundary matters: a cut inside a multi-byte sequence puts invalid UTF-8 in the ring, and
        every reader then has to decide what to do about it. A continuation byte is ``10xxxxxx``, so
        the cut walks back off any it lands on.
        """
        encoded = value.encode("utf-8")
        end = min(len(encoded), width)
        # One past the end reads as `undefined` in the reference, and `undefined & 0xc0` is 0 — so a
        # string that fits exits this loop at once. A zero here is that same answer.
        while end > 0 and ((encoded[end] if end < len(encoded) else 0) & 0xC0) == 0x80:
            end -= 1
        self.bytes[at : at + width] = bytes(width)
        self.bytes[at : at + end] = encoded[:end]


def _to_i32(value: Any) -> int:
    """``DataView.setInt32`` — the low 32 bits, read as signed."""
    masked = int(value) & 0xFFFFFFFF
    return masked - 0x100000000 if masked >= 0x80000000 else masked


def _to_f32(value: Any) -> float:
    """A value on its way into four bytes. ``None`` is NaN, which is how §24 spells unmeasured."""
    return float("nan") if value is None else float(value)


def _f_list(values: Any) -> list[float]:
    """A JSON array of numbers, with ``null`` read as NaN (§24)."""
    return [_to_f32(value) for value in (values or [])]


def encode_record(
    shape: dict[str, int], sequence: int, mono_ns: int, entry: dict[str, Any]
) -> bytes:
    """Encode one record into a fresh slot of exactly :func:`record_size` bytes."""
    slot = _Cursor(record_size(shape))
    kind = entry.get("kind")
    slot.u8(4, KINDS[kind])
    slot.u8(5, RECORD_SCHEMA)
    slot.u64(8, sequence)
    slot.u64(16, mono_ns)

    if kind == "cycle":
        _encode_cycle(shape, slot, entry)
    elif kind == "frame":
        _encode_frame(slot, entry)
    elif kind == "event":
        _encode_event(slot, entry)
    else:
        slot.u64(PREFIX, int(entry["dropped"]))
        slot.u64(PREFIX + 8, int(entry["first_tick"]))
        slot.u16(PREFIX + 16, _index_of(GAP_REASONS, entry.get("reason")) + 1)

    slot.u32(0, crc32_of(bytes(slot.bytes[4:])))
    return bytes(slot.bytes)


def _encode_cycle(shape: dict[str, int], slot: _Cursor, cycle: dict[str, Any]) -> None:
    def check(field_name: str, found: int, expected: int) -> None:
        if found != expected:
            raise ValueError(
                f"black box: {field_name} holds {found} values, and the ring's shape says {expected}"
            )

    desired = _f_list(cycle.get("desired"))
    command = _f_list(cycle.get("command"))
    mask = list(cycle.get("mask") or [])
    clamped = list(cycle.get("clamped") or [])
    position = _f_list(cycle.get("position"))
    velocity = _f_list(cycle.get("velocity"))
    effort = _f_list(cycle.get("effort"))

    check("desired", len(desired), shape["channels"])
    check("command", len(command), shape["channels"])
    check("mask", len(mask), shape["channels"])
    check("clamped", len(clamped), shape["channels"])
    check("position", len(position), shape["joints"])
    check("velocity", len(velocity), shape["joints"])
    check("effort", len(effort), shape["joints"])

    slot.u64(24, int(cycle["tick"]))
    slot.raw(32, bytes.fromhex(cycle["observation"]))
    slot.u16(64, int(cycle["policy"]))
    slot.u16(66, int(cycle["state_flags"]))
    slot.u32(68, int(cycle["fault_code"]))
    slot.i32(72, int(cycle.get("rhal_result") or 0))
    slot.u32(76, int(cycle["state_seq"]))
    slot.u64(80, int(cycle["state_stamp_ns"]))
    slot.u64(88, int(cycle["command_stamp_ns"]))
    slot.u32(96, int(cycle["command_seq"]))
    slot.u8(100, int(cycle["mode"]))
    slot.u8(101, 1 if cycle.get("advisory") is True else 0)
    response = cycle.get("response")
    slot.u8(102, 0 if response is None else _index_of(RESPONSES, response) + 1)
    tripped = list(cycle.get("tripped") or [])
    slot.u8(103, min(len(tripped), MAX_TRIPPED))
    for index, guard in enumerate(tripped[:MAX_TRIPPED]):
        slot.u8(104 + index, _index_of(GUARDS, guard) + 1)
    slot.u16(112, int(cycle["capability"]))
    slot.f32(116, _to_f32(cycle.get("nearest_person_m")))
    for index, value in enumerate(_f_list(cycle.get("base_pose"))):
        slot.f32(120 + index * 4, value)
    for index, value in enumerate(_f_list(cycle.get("base_twist"))):
        slot.f32(148 + index * 4, value)
    # 172..176 was reserved padding, which is why this landed without moving a byte or bumping the
    # ring's format. The schema byte separates "reserved zero" from "the read succeeded".
    slot.i32(172, int(cycle.get("read_result") or 0))

    at = _cycle_offsets(shape)
    for index, value in enumerate(desired):
        slot.f32(at["desired"] + index * 4, value)
    for index, value in enumerate(command):
        slot.f64(at["command"] + index * 8, value)
    for index, value in enumerate(mask):
        slot.u8(at["mask"] + index, int(value or 0))
    for index, was in enumerate(clamped):
        if was:
            slot.bytes[at["clamped"] + index // 8] |= 1 << (index % 8)
    for index, value in enumerate(position):
        slot.f32(at["position"] + index * 4, value)
    for index, value in enumerate(velocity):
        slot.f32(at["velocity"] + index * 4, value)
    for index, value in enumerate(effort):
        slot.f32(at["effort"] + index * 4, value)


def _encode_frame(slot: _Cursor, frame: dict[str, Any]) -> None:
    slot.raw(PREFIX, bytes.fromhex(frame["observation"]))
    slot.u64(PREFIX + 32, int(frame["target_ns"]))
    skew = frame.get("worst_skew_us")
    slot.u32(PREFIX + 40, SKEW_UNMEASURABLE if skew is None else int(skew))
    streams = list(frame.get("streams") or [])
    written = min(len(streams), MAX_STREAMS)
    slot.u16(PREFIX + 44, written)
    slot.u16(
        PREFIX + 46,
        int(frame["streams_omitted"]) + max(0, len(streams) - MAX_STREAMS),
    )
    slot.u16(PREFIX + 48, int(frame["streams_absent"]))
    # PREFIX + 50 is reserved and left zero, so the stream table starts on a four-byte boundary and
    # a hex dump of a damaged ring stays readable.
    for index, stream in enumerate(streams[:MAX_STREAMS]):
        at = PREFIX + 52 + index * (STREAM_NAME + 8)
        slot.text(at, STREAM_NAME, stream.get("name") or "")
        slot.u32(at + STREAM_NAME, int(stream.get("age_us") or 0))
        slot.u8(at + STREAM_NAME + 4, 1 if stream.get("stale") is True else 0)


def _encode_event(slot: _Cursor, event: dict[str, Any]) -> None:
    slot.u16(PREFIX, _index_of(EVENT_CODES, event.get("code")) + 1)
    slot.text(PREFIX + 4, EVENT_SUBJECT, event.get("subject") or "")
    slot.text(PREFIX + 4 + EVENT_SUBJECT, EVENT_DETAIL, event.get("detail") or "")


# ── The ring header ──────────────────────────────────────────────────────────

MAGIC: Final = b"SKYNETBB"
HEADER_FIXED: Final = 184
HEADER_ALIGN: Final = 4096
ROBOT_ID: Final = 64
TOKEN_NAME: Final = 48
CHANNEL_NAME: Final = 48

#: The ring's on-disk format version.
#:
#: **2** adds ``streams_absent`` to the frame record and gives ``worst_skew_us`` a value meaning
#: *not measurable*, which moves the stream table four bytes. Both exist because the version-1
#: record could not distinguish a frame whose streams were all present from one whose capture paths
#: had died: it recorded the survivors and said nothing about the rest, so a recording of a robot
#: with two dead cameras read as a recording of a robot with one camera.
FORMAT_VERSION: Final = 2

#: The per-record schema version this edition writes.
#:
#: Distinct from :data:`FORMAT_VERSION`, which is the **ring header's** and refuses a whole
#: recording whose layout it does not know. This one is per slot, for the reason the encoder has
#: always given — a ring outlives a daemon upgrade and a reader must be able to tell which layout a
#: given slot is in — and until 2026-09-07 nothing read it, so it said ``1`` while the frame record
#: had already grown a field.
#:
#: **2** puts ``read_state``'s outcome on the cycle record, in four bytes (172..176) that were
#: reserved padding, so nothing moved and ``FORMAT_VERSION`` did not change. What did change is
#: what a zero there means: in a schema-1 record those bytes are padding and the read's outcome is
#: *unrecorded*; in a schema-2 record a zero means the driver answered.
RECORD_SCHEMA: Final = 2

#: ``worst_skew_us`` when the skew is **not measurable**.
#:
#: A frame in which fewer than two streams contributed has no spread — the spread of one instant is
#: undefined, not zero — and an unsigned field has to write something. ``u32::MAX`` is seventy-one
#: minutes of skew, so nothing real can collide with it.
SKEW_UNMEASURABLE: Final = 4_294_967_295

_AT: Final = {
    "MAGIC": 0,
    "VERSION": 8,
    "HEADER_LEN": 12,
    "RECORD_SIZE": 16,
    "CAPACITY": 20,
    "JOINTS": 24,
    "CHANNELS": 26,
    "TIER": 28,
    "ENFORCEMENT": 29,
    "SIMULATOR": 30,
    "EMBODIMENT": 32,
    "OS_IMAGE": 64,
    "ROBOT_ID": 96,
    "DIGEST_CAPACITY": 160,
    "CAPABILITY_CAPACITY": 162,
    "WALL_EPOCH_NS": 164,
    "MONO_NS": 172,
    "CRC": 180,
}


def header_len(geometry: dict[str, Any]) -> int:
    raw = (
        HEADER_FIXED
        + geometry["digests"] * 32
        + geometry["capabilities"] * TOKEN_NAME
        + geometry["shape"]["channels"] * CHANNEL_NAME
    )
    return _div_ceil(raw, HEADER_ALIGN) * HEADER_ALIGN


def encode_header(
    geometry: dict[str, Any],
    meta: dict[str, Any],
    digests: list[bytes] | None = None,
    capabilities: list[str] | None = None,
) -> bytes:
    """The header a ring is created with, including the two tables."""
    digests = digests or []
    capabilities = capabilities or []
    length = header_len(geometry)
    header = _Cursor(length)
    header.raw(_AT["MAGIC"], MAGIC)
    header.u16(_AT["VERSION"], FORMAT_VERSION)
    header.u32(_AT["HEADER_LEN"], length)
    header.u32(_AT["RECORD_SIZE"], record_size(geometry["shape"]))
    header.u32(_AT["CAPACITY"], geometry["capacity"])
    header.u16(_AT["JOINTS"], geometry["shape"]["joints"])
    header.u16(_AT["CHANNELS"], geometry["shape"]["channels"])
    header.u8(_AT["TIER"], int(meta["tier"]))
    header.u8(_AT["ENFORCEMENT"], int(meta["enforcement"]))
    header.u8(_AT["SIMULATOR"], 1 if meta.get("simulator") is True else 0)
    header.raw(_AT["EMBODIMENT"], meta["embodiment"])
    header.raw(_AT["OS_IMAGE"], meta["os_image"])
    header.text(_AT["ROBOT_ID"], ROBOT_ID, meta.get("robot_id") or "")
    header.u16(_AT["DIGEST_CAPACITY"], geometry["digests"])
    header.u16(_AT["CAPABILITY_CAPACITY"], geometry["capabilities"])
    header.u64(_AT["WALL_EPOCH_NS"], int(meta["wall_epoch_ns"]))
    header.u64(_AT["MONO_NS"], int(meta["mono_ns"]))

    digest_base = HEADER_FIXED
    for index, digest in enumerate(digests):
        header.raw(digest_base + index * 32, digest)
    token_base = digest_base + geometry["digests"] * 32
    for index, token in enumerate(capabilities):
        header.text(token_base + index * TOKEN_NAME, TOKEN_NAME, token)
    channel_base = token_base + geometry["capabilities"] * TOKEN_NAME
    for index, name in enumerate((meta.get("channels") or [])[: geometry["shape"]["channels"]]):
        header.text(channel_base + index * CHANNEL_NAME, CHANNEL_NAME, name)

    header.u32(_AT["CRC"], crc32_of(bytes(header.bytes)))
    return bytes(header.bytes)


# ── The channel schemas (§26, §31.2) ─────────────────────────────────────────


def _field(kind: str, description: str) -> dict[str, Any]:
    return {"type": kind, "description": description}


def _maybe(kind: str, description: str) -> dict[str, Any]:
    return {"type": [kind, "null"], "description": description}


def _array_of(items: Any, description: str) -> dict[str, Any]:
    return {"type": "array", "items": items, "description": description}


def _number_array(description: str) -> dict[str, Any]:
    return _array_of({"type": ["number", "null"]}, description)


def _object(
    properties: dict[str, Any], required: list[str], description: str
) -> dict[str, Any]:
    return {
        "type": "object",
        "description": description,
        "properties": properties,
        "required": required,
    }


def _mono() -> dict[str, Any]:
    return _field(
        "integer",
        "CLOCK_MONOTONIC_RAW at the instant this record was produced. The time base every control "
        "decision was made against (§35.1); MCAP's log_time is the wall-clock rendering of it.",
    )


def _channel_values(description: str) -> dict[str, Any]:
    return _array_of(
        _object(
            {
                "channel": _field("string", "The action channel's name."),
                "value": _maybe("number", "The value on that channel."),
                "hold": _field(
                    "boolean",
                    "True where the mask says hold this joint at its current setpoint — the OS "
                    "saying nothing rather than saying zero (§24).",
                ),
            },
            ["channel", "value", "hold"],
            "One channel's value.",
        ),
        description,
    )


def wire() -> list[dict[str, Any]]:
    """The six channels, in id order. Ids are fixed for the life of the format."""
    documents: list[tuple[int, str, str, dict[str, Any]]] = [
        (
            1,
            "/sk/state",
            "sk_state",
            _object(
                {
                    "mono_ns": _mono(),
                    "stamp_ns": _field(
                        "integer",
                        "rhal_state.stamp_ns: CLOCK_MONOTONIC_RAW at acquisition, never at return "
                        "(§24).",
                    ),
                    "seq": _field("integer", "rhal_state.seq."),
                    "flags": _field(
                        "integer",
                        "Bitwise-or of the RHAL flags: estop, fault, degraded, braked.",
                    ),
                    "fault_code": _field("integer", "Vendor-specific, surfaced verbatim (§24)."),
                    "position_rad": _number_array("Measured joint positions."),
                    "velocity_rad_s": _number_array(
                        "Measured velocities; null where the joint does not measure it."
                    ),
                    "effort_nm": _number_array("Measured efforts; null where unmeasured."),
                    "base_pose": _number_array(
                        "x y z qx qy qz qw; all null when the base is not localised."
                    ),
                    "base_twist": _number_array("vx vy vz wx wy wz."),
                },
                [
                    "mono_ns",
                    "stamp_ns",
                    "seq",
                    "flags",
                    "fault_code",
                    "position_rad",
                    "velocity_rad_s",
                    "effort_nm",
                    "base_pose",
                    "base_twist",
                ],
                "One sample of the robot's state, as the RHAL reported it (§24).",
            ),
        ),
        (
            2,
            "/sk/desired",
            "sk_desired",
            _object(
                {
                    "mono_ns": _mono(),
                    "tick": _field("integer", "The control tick."),
                    "stamp_ns": _field(
                        "integer",
                        "When the policy produced this command, not when it was sent.",
                    ),
                    "seq": _field("integer", "rhal_command.seq; must advance (§24)."),
                    "mode": _field("string", "position, velocity, torque, or unknown."),
                    "policy_digest": _field(
                        "string",
                        "Hex SHA-256 of the package version that produced this, or empty when the "
                        "recording's digest table did not hold it.",
                    ),
                    "observation": _field(
                        "string",
                        "Hex SHA-256 of the observation frame the policy was given. A hash, never "
                        "the bytes: an hour of three-camera video is not something the robot can "
                        "keep (§26).",
                    ),
                    "values": _channel_values("What the policy asked for."),
                },
                [
                    "mono_ns",
                    "tick",
                    "stamp_ns",
                    "seq",
                    "mode",
                    "policy_digest",
                    "observation",
                    "values",
                ],
                "What the policy asked for, before the Safety Kernel saw it.",
            ),
        ),
        (
            3,
            "/sk/verdict",
            "sk_verdict",
            _object(
                {
                    "mono_ns": _mono(),
                    "tick": _field("integer", "The control tick."),
                    "response": _maybe(
                        "string",
                        "clamp, hold, stop_cat2, stop_cat1 or stop_cat0 — the strongest response "
                        "any tripped guard demanded. Null when nothing tripped, which is a fact "
                        "rather than an absence and so is present-and-null.",
                    ),
                    "tripped": _array_of(
                        _field(
                            "string",
                            "A guard name, in evaluation order: watchdog, envelope, rate_limit, "
                            "workspace, self_collision, balance, proximity, mode_gate (§7.1).",
                        ),
                        "Guards that tripped.",
                    ),
                    "clamped": _array_of(
                        _object(
                            {
                                "channel": _field("string", "Which channel was reduced."),
                                "from": _maybe("number", "What the policy asked for."),
                                "to": _maybe("number", "What the kernel sent instead."),
                            },
                            ["channel", "from", "to"],
                            "One clamped channel, and by how much.",
                        ),
                        "Channels whose values were reduced.",
                    ),
                    "advisory": _field(
                        "boolean",
                        "True when the tier could only advise: the command was forwarded unchanged "
                        "despite a trip. Always false at host and certified (§42).",
                    ),
                },
                ["mono_ns", "tick", "response", "tripped", "clamped", "advisory"],
                "What the Safety Kernel decided this cycle.",
            ),
        ),
        (
            4,
            "/sk/command",
            "sk_command",
            _object(
                {
                    "mono_ns": _mono(),
                    "tick": _field("integer", "The control tick."),
                    "values": _channel_values("What actually went to write_command."),
                    "rhal_result": _field(
                        "integer",
                        "What write_command returned: 0, or a negative errno. -1 EPERM because the "
                        "drive is faulted is a different fact from -110 ETIMEDOUT (§24).",
                    ),
                },
                ["mono_ns", "tick", "values", "rhal_result"],
                "What was sent to the hardware, and what the hardware said about it.",
            ),
        ),
        (
            5,
            "/sk/frame",
            "sk_frame",
            _object(
                {
                    "mono_ns": _mono(),
                    "observation": _field("string", "Hex SHA-256 of the assembled frame."),
                    "target_ns": _field("integer", "The instant the assembler targeted."),
                    "worst_skew_us": _maybe(
                        "integer",
                        "The worst skew between any stream and that instant. Recorded so that 'the "
                        "policy behaved oddly' can be checked against 'the frame it was given was "
                        "80 ms skewed' months later (§25.1). Null when fewer than two streams "
                        "contributed: the spread of one instant is undefined rather than zero, and "
                        "zero would make a frame with three dead capture paths read as a perfectly "
                        "synchronised one.",
                    ),
                    "streams_omitted": _field(
                        "integer",
                        "Streams the record could not carry, counted rather than dropped. A limit "
                        "of the format.",
                    ),
                    "streams_absent": _field(
                        "integer",
                        "Streams the policy declared that produced no sample at all. A limit of "
                        "the robot, and deliberately not the field above: an investigation reading "
                        "one as the other goes looking for a bigger record instead of a dead "
                        "camera.",
                    ),
                    "streams": _array_of(
                        _object(
                            {
                                "name": _field("string", "The stream's name."),
                                "age_us": _field(
                                    "integer",
                                    "How far back this stream's sample was, relative to the "
                                    "instant the assembler targeted.",
                                ),
                                "stale": _field(
                                    "boolean",
                                    "Whether that exceeded the policy's declared staleness budget, "
                                    "which is a hold rather than a silently older frame (§25.1).",
                                ),
                            },
                            ["name", "age_us", "stale"],
                            "One stream's contribution to the frame.",
                        ),
                        "Per-stream ages, for the streams that had one.",
                    ),
                },
                [
                    "mono_ns",
                    "observation",
                    "target_ns",
                    "worst_skew_us",
                    "streams_omitted",
                    "streams_absent",
                    "streams",
                ],
                "One assembled observation frame, and how well aligned it was.",
            ),
        ),
        (
            6,
            "/sk/event",
            "sk_event",
            _object(
                {
                    "mono_ns": _mono(),
                    "event": _field(
                        "string",
                        "lease_granted, lease_released, lease_preempted, estop, estop_cleared, "
                        "package_started, package_stopped, grant_changed, ota, clock_step, "
                        "recording_started, sealed, config_changed, admission, skill_transition, "
                        "or gap.",
                    ),
                    "subject": _field(
                        "string", "What it happened to: a package id, a capability token, a slot."
                    ),
                    "detail": _field("string", "A sentence about it."),
                    "dropped": _field(
                        "integer",
                        "Gap events only: how many records were not written. A gap annotated as a "
                        "gap is honest; a gap that looks like a quiet period is not (§47.4).",
                    ),
                    "first_tick": _field(
                        "integer", "Gap events only: the first tick that went unrecorded."
                    ),
                    "reason": _field(
                        "string",
                        "Gap events only: writer_behind, encoder_behind, not_recording or "
                        "write_failed.",
                    ),
                },
                ["mono_ns", "event", "subject", "detail"],
                "Something that changed, and every gap in the recording.",
            ),
        ),
    ]
    return [
        {"id": id_, "topic": topic, "name": name, "definition": canonical(document)}
        for id_, topic, name, document in documents
    ]


# ── The MCAP writer ──────────────────────────────────────────────────────────

MCAP_MAGIC: Final = bytes([0x89, 0x4D, 0x43, 0x41, 0x50, 0x30, 0x0D, 0x0A])

OP: Final = {
    "HEADER": 0x01,
    "FOOTER": 0x02,
    "SCHEMA": 0x03,
    "CHANNEL": 0x04,
    "MESSAGE": 0x05,
    "STATISTICS": 0x0B,
    "METADATA": 0x0C,
    "SUMMARY_OFFSET": 0x0E,
    "DATA_END": 0x0F,
}


class _Bytes:
    __slots__ = ("parts",)

    def __init__(self) -> None:
        self.parts = bytearray()

    def push(self, *values: int) -> None:
        self.parts.extend(value & 0xFF for value in values)

    def u16(self, value: int) -> None:
        self.parts.extend(struct.pack("<H", value & 0xFFFF))

    def u32(self, value: int) -> None:
        self.parts.extend(struct.pack("<I", value & 0xFFFFFFFF))

    def u64(self, value: int) -> None:
        self.parts.extend(struct.pack("<Q", value & 0xFFFFFFFFFFFFFFFF))

    def raw(self, source: bytes) -> None:
        self.parts.extend(source)

    def string(self, value: str) -> None:
        encoded = value.encode("utf-8")
        self.u32(len(encoded))
        self.raw(encoded)

    @property
    def bytes(self) -> bytes:
        return bytes(self.parts)

    def __len__(self) -> int:
        return len(self.parts)


def write_mcap(
    library: str,
    schemas: list[dict[str, Any]],
    channels: list[dict[str, Any]],
    metadata: list[dict[str, Any]],
    messages: list[dict[str, Any]],
) -> bytes:
    """Write one MCAP file.

    Magic, header, schemas, channels, metadata, messages, data end, summary, summary offsets,
    footer, magic. Unchunked and uncompressed, exactly as the Rust edition writes it.
    """
    out = _Bytes()

    def record(opcode: int, body: _Bytes) -> None:
        out.push(opcode)
        out.u64(len(body))
        out.raw(body.bytes)

    def schema_body(schema: dict[str, Any]) -> _Bytes:
        body = _Bytes()
        body.u16(schema["id"])
        body.string(schema["name"])
        body.string("jsonschema")
        encoded = schema["definition"].encode("utf-8")
        body.u32(len(encoded))
        body.raw(encoded)
        return body

    def channel_body(channel: dict[str, Any]) -> _Bytes:
        body = _Bytes()
        body.u16(channel["id"])
        body.u16(channel["schema_id"])
        body.string(channel["topic"])
        body.string("json")
        body.u32(0)
        return body

    out.raw(MCAP_MAGIC)
    header = _Bytes()
    header.string("")
    header.string(library)
    record(OP["HEADER"], header)

    for schema in schemas:
        record(OP["SCHEMA"], schema_body(schema))
    for channel in channels:
        record(OP["CHANNEL"], channel_body(channel))

    for entry in metadata:
        table = _Bytes()
        for key, value in entry["pairs"]:
            table.string(key)
            table.string(value)
        body = _Bytes()
        body.string(entry["name"])
        body.u32(len(table))
        body.raw(table.bytes)
        record(OP["METADATA"], body)

    sequences: dict[int, int] = {channel["id"]: 0 for channel in channels}
    counts: dict[int, int] = {channel["id"]: 0 for channel in channels}
    first: int | None = None
    last = 0
    for message in messages:
        channel_id = message["channel_id"]
        log_time = message["log_time"]
        sequence = sequences.get(channel_id, 0) + 1
        sequences[channel_id] = sequence
        counts[channel_id] = counts.get(channel_id, 0) + 1
        first = log_time if first is None or log_time < first else first
        last = log_time if log_time > last else last

        body = _Bytes()
        body.u16(channel_id)
        body.u32(sequence)
        body.u64(log_time)
        body.u64(log_time)
        body.raw(message["payload"].encode("utf-8"))
        record(OP["MESSAGE"], body)

    data_end = _Bytes()
    data_end.u32(crc32_of(out.bytes))
    record(OP["DATA_END"], data_end)

    summary_start = len(out)
    schema_start = len(out)
    for schema in schemas:
        record(OP["SCHEMA"], schema_body(schema))
    schema_len = len(out) - schema_start

    channel_start = len(out)
    for channel in channels:
        record(OP["CHANNEL"], channel_body(channel))
    channel_len = len(out) - channel_start

    statistics_start = len(out)
    statistics = _Bytes()
    statistics.u64(sum(counts.values()))
    statistics.u16(len(schemas))
    statistics.u32(len(channels))
    statistics.u32(0)
    statistics.u32(len(metadata))
    statistics.u32(0)
    statistics.u64(first if first is not None else 0)
    statistics.u64(last)
    per_channel = _Bytes()
    for id_, count in counts.items():
        per_channel.u16(id_)
        per_channel.u64(count)
    statistics.u32(len(per_channel))
    statistics.raw(per_channel.bytes)
    record(OP["STATISTICS"], statistics)
    statistics_len = len(out) - statistics_start

    summary_offset_start = len(out)
    for opcode, start, length in (
        (OP["SCHEMA"], schema_start, schema_len),
        (OP["CHANNEL"], channel_start, channel_len),
        (OP["STATISTICS"], statistics_start, statistics_len),
    ):
        body = _Bytes()
        body.push(opcode)
        body.u64(start)
        body.u64(length)
        record(OP["SUMMARY_OFFSET"], body)

    footer = _Bytes()
    footer.push(OP["FOOTER"])
    footer.u64(20)
    footer.u64(summary_start)
    footer.u64(summary_offset_start)
    summary_section = out.bytes[summary_start:]
    footer.u32(crc32_final(crc32(footer.bytes, crc32(summary_section))))
    out.raw(footer.bytes)

    out.raw(MCAP_MAGIC)
    return out.bytes


def opcodes(data: bytes) -> str:
    """The file's record opcodes, in order, as two-digit hex.

    Pinned beside the digest in the corpus because a digest says *that* two editions disagree and
    this says *where*. It also checks that the framing walks: a file whose record lengths do not
    land exactly on the trailing magic is a file no reader will open.
    """
    end = len(data) - 8
    found: list[str] = []
    at = 8
    while at < end:
        if at + 9 > end:
            raise ValueError(f"mcap: a record header runs past the end of the file at {at}")
        found.append(format(data[at], "02x"))
        at += 9 + struct.unpack_from("<Q", data, at + 1)[0]
    if at != end:
        raise ValueError(f"mcap: the framing ended at {at}, the trailing magic at {end}")
    return ",".join(found)


def hex_of(data: bytes) -> str:
    """Lower-case hex, as the sealed file's metadata spells a digest."""
    return data.hex()


# ── The wire shapes ──────────────────────────────────────────────────────────


def records_to_json(case_input: dict[str, Any]) -> Any:
    """``blackbox/records`` — one encoded record."""
    encoded = encode_record(
        case_input["shape"],
        int(case_input["sequence"]),
        int(case_input["mono_ns"]),
        case_input["entry"],
    )
    return {"size": len(encoded), "hex": hex_of(encoded)}


def header_to_json(case_input: dict[str, Any]) -> Any:
    """``blackbox/header`` — a ring header, with both of its tables."""
    geometry = case_input["geometry"]
    meta = dict(case_input["meta"])
    meta["embodiment"] = bytes.fromhex(meta["embodiment"])
    meta["os_image"] = bytes.fromhex(meta["os_image"])
    encoded = encode_header(
        geometry,
        meta,
        [bytes.fromhex(digest) for digest in case_input.get("digests") or []],
        list(case_input.get("capabilities") or []),
    )
    return {
        "size": len(encoded),
        "record_size": record_size(geometry["shape"]),
        "hex": hex_of(encoded),
    }


def schemas_to_json(case_input: dict[str, Any]) -> Any:
    """``blackbox/schemas`` — one embedded channel schema."""
    topic = case_input["channel"]
    for channel in wire():
        if channel["topic"] == topic:
            return channel
    raise ValueError(f"`{topic}` is not a channel this edition writes")


def seal_to_json(case_input: dict[str, Any]) -> Any:
    """``blackbox/seal`` — a whole sealed file, held by its length, its digest and its framing."""
    channels = wire()
    encoded = write_mcap(
        case_input["library"],
        [
            {"id": channel["id"], "name": channel["name"], "definition": channel["definition"]}
            for channel in channels
        ],
        [
            {"id": channel["id"], "schema_id": channel["id"], "topic": channel["topic"]}
            for channel in channels
        ],
        [case_input["metadata"]],
        [
            {
                "channel_id": message["channel_id"],
                "log_time": int(message["log_time"]),
                "payload": message["payload"],
            }
            for message in case_input["messages"]
        ],
    )
    return {
        "size": len(encoded),
        "sha256": hashlib.sha256(encoded).hexdigest(),
        "opcodes": opcodes(encoded),
    }
