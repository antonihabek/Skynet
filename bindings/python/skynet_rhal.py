"""The Skynet RHAL, for Python.

§4 promises this: *"Wrappers ship for C++ (bindings/cpp/skynet_rhal.hpp), Rust (the `skynet-rhal`
crate) and Python (ctypes, for simulator and bench drivers where the RT path does not matter). They
are conveniences over these symbols; none of them is a second implementation."*

This module is two things that share one vocabulary:

1. **The driver protocol** — :class:`Driver`, :class:`State`, :class:`Command`. A Python driver
   subclasses :class:`Driver` and is served to the OS by ``drivers/python-host`` (see
   :func:`serve`).
2. **The ctypes view of the C ABI** — :class:`SkrState`, :class:`SkrCommand`, :class:`SkrDriver`.
   For *consuming* a driver: a bench harness that dlopens somebody's ``.so`` and calls it. The field
   offsets are asserted against the same numbers ``crates/skynet-rhal`` and ``drivers/mock`` assert,
   so if the layout ever moves, three suites fail rather than one robot behaving oddly.

Python's tier ceiling is **Guest** (§31.3, §66) and this module never pretends otherwise. The GIL
and the garbage collector cannot hold a real-time path, so a Python driver is an *observer* with a
body attached: it gets the package system, skills, capability grants, fleet reconciliation, the
black box and episode capture, and it does not get an enforcing kernel.

Requires nothing but the standard library. A driver author with numpy, LeRobot or MuJoCo installed
uses them in their own code; this module does not import them.
"""

from __future__ import annotations

import base64
import ctypes
import math
import sys
from dataclasses import dataclass, field
from typing import Protocol, Sequence

__all__ = [
    "ABI_VERSION",
    "SKR_OK",
    "SKR_EPERM",
    "SKR_EIO",
    "SKR_ENODEV",
    "SKR_EINVAL",
    "SKR_ENOSYS",
    "SKR_EPROTO",
    "SKR_ETIMEDOUT",
    "FLAG_ESTOP",
    "FLAG_FAULT",
    "FLAG_DEGRADED",
    "FLAG_BRAKED",
    "HOMING_SHIFT",
    "HOMING_MASK",
    "HOMING_UNREPORTED",
    "HOMING_NOT_REQUIRED",
    "HOMING_PENDING",
    "HOMING_COMPLETE",
    "HOMING_NAMES",
    "MODE_POSITION",
    "MODE_VELOCITY",
    "MODE_TORQUE",
    "MODE_NAMES",
    "State",
    "Command",
    "Frame",
    "Driver",
    "serve",
    "SkrState",
    "SkrCommand",
    "SkrFrame",
    "SkrDriver",
    "assert_abi_layout",
]

#: Bumped only for a breaking change. The OS refuses a driver whose major does not match, rather
#: than attempting a partial load — half a hardware driver is worse than none.
ABI_VERSION = 1

# ── Return codes ──────────────────────────────────────────────────────────────
#
# Negative errno-style, and the distinction between them is recorded in the black box: "the drive is
# faulted" and "the bus timed out" lead to completely different investigations.

SKR_OK = 0
SKR_EPERM = -1  #: drive disabled, or the command is not permitted in this mode
SKR_EIO = -5  #: bus or transport failure
SKR_ENODEV = -19  #: the named joint, sensor or device does not exist
SKR_EINVAL = -22  #: malformed request — a wrong joint_count, an unknown mode
SKR_ENOSYS = -38  #: the driver does not implement this optional entry point
SKR_EPROTO = -71  #: the device answered, and the answer was not valid
SKR_ETIMEDOUT = -110  #: the deadline passed before the operation completed

# ── State flags ───────────────────────────────────────────────────────────────

FLAG_ESTOP = 1 << 0  #: an emergency stop is asserted
FLAG_FAULT = 1 << 1  #: the drive has faulted; commands are being ignored
FLAG_DEGRADED = 1 << 2  #: running, but something is wrong — thermal, a dead encoder
FLAG_BRAKED = 1 << 3  #: holding brakes are engaged

# ── Homing, bits 4-5 of the same word ─────────────────────────────────────────
#
# A two-bit field rather than two booleans, and zero is `unreported` rather than `pending` because
# those are different sentences to whoever is holding the robot: *pending* is a machine somebody
# has to home, *unreported* is a driver written before the field existed. The OS refuses to command
# on either, and says which. Adding these bits is not an ABI break — the word had twelve bits
# spare, no offset moves, and a driver that never sets them reads as exactly what it is.

HOMING_SHIFT = 4
HOMING_MASK = 3 << HOMING_SHIFT
HOMING_UNREPORTED = 0 << HOMING_SHIFT  #: this driver does not answer the question
HOMING_NOT_REQUIRED = 1 << HOMING_SHIFT  #: absolute encoders; there is nothing to home
HOMING_PENDING = 2 << HOMING_SHIFT  #: joint positions are not yet trustworthy
HOMING_COMPLETE = 3 << HOMING_SHIFT  #: homed, and the positions mean what they say

#: The four spellings, for a driver that wants to log what it just reported.
HOMING_NAMES = {
    HOMING_UNREPORTED: "unreported",
    HOMING_NOT_REQUIRED: "not_required",
    HOMING_PENDING: "pending",
    HOMING_COMPLETE: "complete",
}

# ── Control modes ─────────────────────────────────────────────────────────────

MODE_POSITION = 0  #: values are radians (or metres, for a prismatic joint)
MODE_VELOCITY = 1  #: values are rad/s (or m/s)
MODE_TORQUE = 2  #: values are Nm (or N)

MODE_NAMES = {MODE_POSITION: "position", MODE_VELOCITY: "velocity", MODE_TORQUE: "torque"}


@dataclass
class State:
    """One sample of the robot's state.

    ``velocity_rad_s`` and ``effort_nm`` may hold ``nan`` where the joint does not measure them, and
    that is the honest value rather than a defect — zero is a *measurement*. ``position_rad`` may
    not: every guard that bounds travel reads it.
    """

    #: Monotonic nanoseconds **at acquisition**, never at return. A timestamp applied on return
    #: folds the driver's own latency into the measurement and silently corrupts every downstream
    #: budget (§24).
    stamp_ns: int = 0
    #: Sample sequence number. Must advance; a repeat is how the watchdog guard recognises a stalled
    #: producer, so a driver that never advances it is indistinguishable from a dead bus.
    seq: int = 0
    #: Bitwise-or of the ``FLAG_*`` constants.
    flags: int = 0
    #: Vendor-specific, surfaced verbatim to the black box.
    fault_code: int = 0
    position_rad: list[float] = field(default_factory=list)
    velocity_rad_s: list[float] = field(default_factory=list)
    effort_nm: list[float] = field(default_factory=list)
    #: ``x y z qx qy qz qw``; all ``nan`` when the base is not localised.
    base_pose: list[float] = field(default_factory=lambda: [math.nan] * 7)
    #: ``vx vy vz wx wy wz``.
    base_twist: list[float] = field(default_factory=lambda: [0.0] * 6)

    @property
    def joint_count(self) -> int:
        return len(self.position_rad)


@dataclass
class Command:
    """One command.

    ``mask`` is the field worth reading twice. A zero means **hold this joint**, and it is how the OS
    says nothing rather than saying zero — on a torque-controlled joint that is the difference
    between holding position and going limp. It is also where a ``drop`` binding from the transport
    plan meets the wire: a channel this body has no equivalent for becomes a masked joint rather
    than a fabricated value.
    """

    #: When the command was produced, not when it was sent.
    stamp_ns: int = 0
    #: Must advance. A repeat is treated as a stalled producer.
    seq: int = 0
    #: One of ``MODE_POSITION`` / ``MODE_VELOCITY`` / ``MODE_TORQUE``.
    mode: int = MODE_POSITION
    #: Units follow ``mode``.
    values: list[float] = field(default_factory=list)
    #: 0 = hold, non-zero = apply.
    mask: list[int] = field(default_factory=list)

    @property
    def joint_count(self) -> int:
        return len(self.values)

    def applied(self) -> list[tuple[int, float]]:
        """The ``(index, value)`` pairs this command actually asks for."""
        return [(i, v) for i, (v, m) in enumerate(zip(self.values, self.mask)) if m]


@dataclass
class Frame:
    """One sensor frame."""

    stamp_ns: int = 0
    width: int = 0
    height: int = 0
    stride: int = 0
    #: One of the ``SKR_FMT_*`` values; 0 is RGB8.
    format: int = 0
    data: bytes = b""


class Driver(Protocol):
    """What a Python driver implements.

    The three rules from §4 apply, with one of them relaxed by the tier and one made *harder*:

    1. ``read_state`` and ``write_command`` are wait-free and allocate nothing — **relaxed**, because
       a Python driver is Guest tier and is not on the real-time path. It is still worth not doing
       anything silly here: the OS's cycle budget is a real number and this is inside it.
    2. ``emergency_stop`` must work from a signal handler and from a dying process — **harder**, and
       this is why ``drivers/python-host`` runs the driver out of process. Calling into CPython from
       a signal handler is not safe, so the host never does: it writes one line to a pipe, which is,
       and the interpreter picks it up when it next reads. See that driver's README.
    3. ``describe`` is the source of truth for the body — **unchanged**. Build it once at ``open``
       and return the cached string.
    """

    def open(self, config: str) -> int:
        """Open the device. Returns ``SKR_OK`` or a negative code."""
        ...

    def close(self) -> int: ...

    def read_state(self, out: State, deadline_ns: int) -> int:
        """Fill ``out``. Returns ``SKR_OK`` or a negative code."""
        ...

    def write_command(self, cmd: Command, deadline_ns: int) -> int: ...

    def emergency_stop(self) -> int: ...

    def describe(self) -> str:
        """The embodiment descriptor, as JSON."""
        ...

    def read_sensor(self, name: str, out: Frame) -> int:
        """Optional. Return ``SKR_ENOSYS`` when this body exposes no sensors through the RHAL."""
        ...


# ══════════════════════════════════════════════════════════════════════════════
# The wire protocol
# ══════════════════════════════════════════════════════════════════════════════
#
# `drivers/python-host` is a C shared object that speaks this over the interpreter's stdin and
# stdout. It is deliberately not JSON: the host would then need a JSON parser in C, and the only
# structured thing crossing the boundary is the descriptor, which passes through opaquely.
#
# Every line is whitespace-delimited ASCII. Requests from the host:
#
#   O <byte-length>\n<config-json>        open
#   D                                     describe
#   S <deadline_ns>                       read_state
#   C <stamp> <seq> <mode> <n> <v...> <m...>   write_command
#   E                                     emergency_stop
#   F <name>                              read_sensor
#   X                                     close and exit
#
# Replies:
#
#   H <abi> <protocol> <ceiling> <name> <version>   hello, sent once at startup
#   D <byte-length>\n<bytes>              descriptor
#   S <rc> <stamp> <seq> <flags> <fault> <n> <p...> <v...> <e...>
#   F <rc> <stamp> <w> <h> <stride> <format> <bytes>\n<base64>
#   R <rc>                                for O, C, E and anything that only returns a code
#
# A frame is base64 on its own line rather than raw bytes, because the rest of the protocol is text
# and interleaving a binary write with a text one is how a frame ends up inside the line that
# describes it. The 33% it costs is paid on a path that should not be carrying a real camera: a
# 640x480 RGB frame is 900 KB per cycle through a pipe, and the answer for one of those is the
# runtime's own capture path (§47.2).
#
# **Everything the driver wants to say to a human goes to stderr.** stdout is the wire, and a stray
# `print` on it is a corrupted frame rather than a log line — which the host reports as a protocol
# error, correctly and unhelpfully.
#
# A non-finite float is written as `nan`, `inf` or `-inf` and read back the same way, because NaN in
# a velocity means "this joint does not measure it" and is a value the protocol must carry rather
# than round to zero.

_PROTOCOL_VERSION = 1


def _fmt(value: float) -> str:
    """Render a float so it round-trips, including the non-finite ones."""
    if value != value:
        return "nan"
    if value == math.inf:
        return "inf"
    if value == -math.inf:
        return "-inf"
    return repr(float(value))


def serve(driver: Driver, ceiling: str = "guest", name: str = "python-driver",
          version: str = "0.1.0", stdin=None, stdout=None) -> int:
    """Run ``driver`` as a sidecar, speaking the host protocol on stdin and stdout.

    ``ceiling`` is the highest tier this driver may ever be awarded and is reported at startup, not
    chosen at run time — §31.3's third rule: *an edition may not raise its own ceiling*. A Python
    driver's is ``guest`` and there is no argument that changes that; the parameter exists so a
    simulator driver can declare ``guest`` with the simulated flag rather than so anything can
    declare ``host``.

    Returns the process exit code.
    """
    if ceiling != "guest":
        # Not a policy check dressed up as an assertion — a Python process genuinely cannot make the
        # promise the higher tiers encode, and a driver claiming otherwise would be making a safety
        # claim nobody sanctioned.
        raise ValueError(
            f"ceiling {ceiling!r} is not available to a Python driver. The GIL and the garbage "
            "collector cannot hold a real-time path (§31.3), so the ceiling is 'guest' and it is a "
            "property of the language rather than of this driver."
        )

    reader = stdin if stdin is not None else sys.stdin
    writer = stdout if stdout is not None else sys.stdout

    def send(line: str) -> None:
        writer.write(line + "\n")
        writer.flush()

    send(f"H {ABI_VERSION} {_PROTOCOL_VERSION} {ceiling} {name} {version}")

    state = State()
    while True:
        line = reader.readline()
        if not line:
            # The host went away. Stopping is the only safe reading of that.
            try:
                driver.emergency_stop()
            finally:
                driver.close()
            return 0

        parts = line.split()
        if not parts:
            continue
        verb = parts[0]

        try:
            if verb == "X":
                rc = driver.close()
                send(f"R {rc}")
                return 0

            if verb == "E":
                send(f"R {driver.emergency_stop()}")
                continue

            if verb == "O":
                # The RHAL's `open(config)`. The configuration is length-prefixed rather than
                # whitespace-delimited because it is JSON and JSON contains spaces.
                length = int(parts[1]) if len(parts) > 1 else 0
                config = _read_exactly(reader, length)
                send(f"R {driver.open(config)}")
                continue

            if verb == "D":
                body = driver.describe().encode("utf-8")
                send(f"D {len(body)}")
                writer.write(body.decode("utf-8"))
                writer.write("\n")
                writer.flush()
                continue

            if verb == "S":
                deadline = int(parts[1]) if len(parts) > 1 else 0
                rc = driver.read_state(state, deadline)
                if rc != SKR_OK:
                    send(f"S {rc} 0 0 0 0 0")
                    continue
                n = state.joint_count
                fields = [
                    "S",
                    str(SKR_OK),
                    str(state.stamp_ns),
                    str(state.seq),
                    str(state.flags),
                    str(state.fault_code),
                    str(n),
                ]
                fields += [_fmt(v) for v in state.position_rad[:n]]
                fields += [_fmt(v) for v in _padded(state.velocity_rad_s, n)]
                fields += [_fmt(v) for v in _padded(state.effort_nm, n)]
                send(" ".join(fields))
                continue

            if verb == "F":
                name = parts[1] if len(parts) > 1 else ""
                frame = Frame()
                reader_fn = getattr(driver, "read_sensor", None)
                rc = reader_fn(name, frame) if reader_fn else SKR_ENOSYS
                if rc != SKR_OK:
                    # No payload line on a failed read. An empty line here would sit in the stream
                    # with nothing to consume it and put every later exchange one frame out —
                    # which presents as a protocol error on an unrelated call, later.
                    send(f"F {rc} 0 0 0 0 0 0")
                    continue
                payload = base64.b64encode(frame.data).decode("ascii")
                send(
                    f"F {SKR_OK} {frame.stamp_ns} {frame.width} {frame.height} "
                    f"{frame.stride} {frame.format} {len(frame.data)}"
                )
                send(payload)
                continue

            if verb == "C":
                stamp, seq, mode, n = (int(parts[1]), int(parts[2]), int(parts[3]), int(parts[4]))
                numbers = parts[5:]
                if len(numbers) < 2 * n:
                    send(f"R {SKR_EPROTO}")
                    continue
                cmd = Command(
                    stamp_ns=stamp,
                    seq=seq,
                    mode=mode,
                    values=[float(v) for v in numbers[:n]],
                    mask=[int(float(m)) for m in numbers[n : 2 * n]],
                )
                send(f"R {driver.write_command(cmd, 0)}")
                continue

            send(f"R {SKR_EPROTO}")
        except Exception as error:  # noqa: BLE001 — a driver bug must not take the robot with it
            # An exception crossing this boundary would leave the host waiting on a line that never
            # comes, which reads to the OS as a hung bus. Reporting SKR_EIO and stopping is the
            # truthful answer: the driver failed, and the body should not keep moving on the
            # strength of the last command it accepted.
            print(f"skynet-rhal: driver raised {error!r}", file=sys.stderr, flush=True)
            try:
                driver.emergency_stop()
            except Exception:  # noqa: BLE001
                pass
            send(f"R {SKR_EIO}")


def _read_exactly(reader, length: int) -> str:
    """Read exactly ``length`` characters, then the newline that terminates the block.

    A short read is a real possibility on a pipe and a silently truncated configuration is a driver
    opened against half a JSON object, so this loops rather than trusting one call.
    """
    chunks: list[str] = []
    remaining = length
    while remaining > 0:
        piece = reader.read(remaining)
        if not piece:
            break
        chunks.append(piece)
        remaining -= len(piece)
    reader.readline()  # the newline after the block
    return "".join(chunks)


def _padded(values: Sequence[float], n: int) -> list[float]:
    """``n`` values, padding with NaN — an unmeasured channel is NaN, never zero."""
    out = list(values[:n])
    out.extend([math.nan] * (n - len(out)))
    return out


# ══════════════════════════════════════════════════════════════════════════════
# The ctypes view of the C ABI, for code that *consumes* a driver
# ══════════════════════════════════════════════════════════════════════════════


class SkrState(ctypes.Structure):
    """Mirrors ``skr_state``."""

    _fields_ = [
        ("stamp_ns", ctypes.c_uint64),
        ("seq", ctypes.c_uint32),
        ("joint_count", ctypes.c_uint16),
        ("flags", ctypes.c_uint16),
        ("position_rad", ctypes.POINTER(ctypes.c_float)),
        ("velocity_rad_s", ctypes.POINTER(ctypes.c_float)),
        ("effort_nm", ctypes.POINTER(ctypes.c_float)),
        ("base_pose", ctypes.c_float * 7),
        ("base_twist", ctypes.c_float * 6),
        ("fault_code", ctypes.c_uint32),
    ]


class SkrCommand(ctypes.Structure):
    """Mirrors ``skr_command``."""

    _fields_ = [
        ("stamp_ns", ctypes.c_uint64),
        ("seq", ctypes.c_uint32),
        ("mode", ctypes.c_uint8),
        ("_pad", ctypes.c_uint8 * 3),
        ("joint_count", ctypes.c_uint16),
        ("values", ctypes.POINTER(ctypes.c_float)),
        ("mask", ctypes.POINTER(ctypes.c_uint8)),
    ]


class SkrFrame(ctypes.Structure):
    """Mirrors ``skr_frame``."""

    _fields_ = [
        ("stamp_ns", ctypes.c_uint64),
        ("width", ctypes.c_uint32),
        ("height", ctypes.c_uint32),
        ("stride", ctypes.c_uint32),
        ("format", ctypes.c_uint32),
        ("data", ctypes.c_void_p),
        ("size", ctypes.c_size_t),
    ]


class SkrDriver(ctypes.Structure):
    """Mirrors ``skr_driver`` — the vtable a shared object exports."""

    _fields_ = [
        ("abi_version", ctypes.c_uint32),
        ("driver_name", ctypes.c_char_p),
        ("driver_version", ctypes.c_char_p),
        ("open", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_char_p, ctypes.c_void_p)),
        ("close", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p)),
        ("read_state", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p,
                                        ctypes.POINTER(SkrState), ctypes.c_uint64)),
        ("write_command", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p,
                                           ctypes.POINTER(SkrCommand), ctypes.c_uint64)),
        ("emergency_stop", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p)),
        ("describe", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p,
                                      ctypes.c_char_p, ctypes.c_size_t)),
        ("read_sensor", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p,
                                         ctypes.c_char_p, ctypes.POINTER(SkrFrame))),
        ("extension", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_char_p,
                                       ctypes.c_void_p, ctypes.c_size_t,
                                       ctypes.c_void_p, ctypes.c_size_t)),
    ]


#: The offsets ``crates/skynet-rhal`` asserts from Rust and ``drivers/mock`` asserts from C.
#:
#: §31.2 keeps the C ABI single-source "because two definitions of a struct layout is a segfault",
#: and the way to keep that true rather than aspirational is to assert it from every side. If the
#: layout moves, three suites fail rather than one robot behaving oddly.
_EXPECTED_OFFSETS = {
    SkrState: [
        ("stamp_ns", 0), ("seq", 8), ("joint_count", 12), ("flags", 14),
        ("position_rad", 16), ("velocity_rad_s", 24), ("effort_nm", 32),
        ("base_pose", 40), ("base_twist", 68), ("fault_code", 92),
    ],
    SkrCommand: [
        ("stamp_ns", 0), ("seq", 8), ("mode", 12), ("_pad", 13),
        ("joint_count", 16), ("values", 24), ("mask", 32),
    ],
    SkrFrame: [
        ("stamp_ns", 0), ("width", 8), ("height", 12), ("stride", 16),
        ("format", 20), ("data", 24), ("size", 32),
    ],
}


def assert_abi_layout() -> None:
    """Raise unless this ctypes view matches the C header's layout on this platform.

    Called by the test suite and cheap enough to call at import time in a bench harness. The
    assertion is only meaningful on a 64-bit target, which every platform the OS runs on is; on
    anything else it reports that rather than checking numbers that do not apply.
    """
    if ctypes.sizeof(ctypes.c_void_p) != 8:
        raise AssertionError(
            "the asserted offsets are for a 64-bit target, and this interpreter is "
            f"{8 * ctypes.sizeof(ctypes.c_void_p)}-bit. The C header is portable; these numbers "
            "are not, and pretending otherwise would be worse than declining to check."
        )
    for structure, offsets in _EXPECTED_OFFSETS.items():
        for name, expected in offsets:
            actual = getattr(structure, name).offset
            if actual != expected:
                raise AssertionError(
                    f"{structure.__name__}.{name} is at offset {actual}, and the C header, the Rust "
                    f"crate and the mock driver all say {expected}. Two definitions of a struct "
                    "layout is a segfault (§31.2)."
                )
