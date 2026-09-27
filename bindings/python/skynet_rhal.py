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
import json
import math
import os
import sys
import threading
import time
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
    "CMD_PER_JOINT_MODES",
    "JMODE_INHERIT",
    "JMODE_POSITION",
    "JMODE_TORQUE",
    "JMODE_VELOCITY",
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
#:
#: v2 appends ``release_stop`` after ``extension``. Appending is still a break — the vtable is the
#: driver's own static struct, so a v1 table read as v2 would have its first word past the end
#: taken for a function pointer — which is why the version bumped.
ABI_VERSION = 2

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

# ── Per-joint control modes, §86.1 ────────────────────────────────────────────
#
# Deliberately not the ``MODE_*`` values. Zero is ``INHERIT`` because zero is what an unfilled array
# holds, and a producer that allocated ``modes`` and forgot to fill it must read as *the mode the
# command already carries* — which is exactly ABI 1 — rather than as "every joint on position". The
# offset is what makes the mistake that invites loud: a bare ``MODE_TORQUE`` written into this array
# is not a valid entry at all, so a driver refuses it instead of moving a joint in the wrong units.

JMODE_INHERIT = 0x00  #: take ``Command.mode``
JMODE_POSITION = 0x10
JMODE_VELOCITY = 0x11
JMODE_TORQUE = 0x12

#: ``Command.flags``: ``modes`` is present and readable.
CMD_PER_JOINT_MODES = 1 << 0


def jmode_of(mode: int) -> int:
    """The per-joint spelling of a ``MODE_*`` (§86.1)."""
    return 0x10 | mode


def mode_of_jmode(jmode: int) -> int | None:
    """Decode one ``modes`` entry.

    ``None`` for ``JMODE_INHERIT``. Raises for an entry this ABI does not define, because the value
    it is most likely to be is a bare ``MODE_*`` written into the wrong array — the transcription
    error the offset exists to catch, and one a driver reports as ``EINVAL`` rather than guessing.
    """
    if jmode == JMODE_INHERIT:
        return None
    if jmode not in (JMODE_POSITION, JMODE_VELOCITY, JMODE_TORQUE):
        raise ValueError(f"not a per-joint control mode: {jmode:#04x} (§86.1)")
    return jmode & 0x0F


#: §35.1's clock, when this platform has it. Linux and macOS do; Windows does not.
_ACQUISITION_CLOCK = getattr(time, "CLOCK_MONOTONIC_RAW", None)


def acquisition_ns() -> int:
    """Nanoseconds on ``CLOCK_MONOTONIC_RAW`` — the clock every ``stamp_ns`` is on (§24, §35.1).

    Call it at the instant of acquisition, never at return. It exists so that a driver cannot reach
    for ``time.monotonic_ns()``, which is ``CLOCK_MONOTONIC``: NTP slews that clock and not this one,
    and the loop ages every stamp against its own ``CLOCK_MONOTONIC_RAW`` reading, so a stamp on the
    wrong one arrives off by the accumulated correction (``DRY-COVERAGE-PLAN.md`` F65 measured 112 ms
    of it five seconds after boot). A **simulator** stamps its own simulated time instead, which is
    §35.3's rule and not this function's business.

    Off Linux and macOS there is no raw clock and ``time.monotonic_ns()`` is the closest available —
    on Windows it is ``QueryPerformanceCounter``, which is not slewed either.
    """
    if _ACQUISITION_CLOCK is None:
        return time.monotonic_ns()
    return time.clock_gettime_ns(_ACQUISITION_CLOCK)


@dataclass
class State:
    """One sample of the robot's state.

    ``velocity_rad_s`` and ``effort_nm`` may hold ``nan`` where the joint does not measure them, and
    that is the honest value rather than a defect — zero is a *measurement*. ``position_rad`` may
    not: every guard that bounds travel reads it.
    """

    #: ``CLOCK_MONOTONIC_RAW`` nanoseconds **at acquisition** — :func:`acquisition_ns` — never at
    #: return. A timestamp applied on return folds the driver's own latency into the measurement and
    #: silently corrupts every downstream budget (§24).
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

    ``modes`` is §86.1's, and it is the mask's missing counterpart: the mask has always been per
    joint and the mode was not. Empty means every joint takes ``mode``, which is every command
    written before §86.
    """

    #: When the command was produced, not when it was sent.
    stamp_ns: int = 0
    #: Must advance. A repeat is treated as a stalled producer.
    seq: int = 0
    #: One of ``MODE_POSITION`` / ``MODE_VELOCITY`` / ``MODE_TORQUE`` — the mode of every joint
    #: that inherits.
    mode: int = MODE_POSITION
    #: Units follow this joint's mode.
    values: list[float] = field(default_factory=list)
    #: 0 = hold, non-zero = apply.
    mask: list[int] = field(default_factory=list)
    #: ``JMODE_*`` per joint (§86.1), or empty for a body-wide command.
    modes: list[int] = field(default_factory=list)

    @property
    def joint_count(self) -> int:
        return len(self.values)

    @property
    def flags(self) -> int:
        """``CMD_*``: the per-joint flag is set exactly when ``modes`` is non-empty."""
        return CMD_PER_JOINT_MODES if self.modes else 0

    def mode_of(self, joint: int) -> int:
        """The mode joint ``joint`` is commanded in, inheritance resolved (§86.1)."""
        if not self.modes:
            return self.mode
        resolved = mode_of_jmode(self.modes[joint])
        return self.mode if resolved is None else resolved

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

    def release_stop(self) -> int:
        """Clear the session latch ``emergency_stop`` set, for §48.2's power-on self-test.

        Optional since v2, on ``extension``'s precedent: a driver without this method answers
        ``SKR_ENOSYS`` to the ``R`` verb, and the host — which already cleared its own latch —
        reports that honestly. Clear the latch; resume nothing. Idempotent.
        """
        ...

    def describe(self) -> str:
        """The embodiment descriptor, as JSON."""
        ...

    def read_sensor(self, name: str, out: Frame) -> int:
        """Optional. Return ``SKR_ENOSYS`` when this body exposes no sensors through the RHAL."""
        ...

    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        """Optional. The vendor escape hatch (§4): ``token`` names the capability, ``data`` is the
        request, and the answer is ``(rc, reply)``. A driver without this method answers
        ``SKR_ENOSYS`` to every ``T``, which is what the OS reports as the capability being
        absent."""
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
#   R                                     release_stop
#   F <name>                              read_sensor
#   T <token> <base64>                    extension under a vendor token
#   X                                     close and exit
#
# Replies:
#
#   H <abi> <protocol> <ceiling> <name> <version>   hello, sent once at startup
#   D <byte-length>\n<bytes>              descriptor
#   S <rc> <stamp> <seq> <flags> <fault> <n> <p...> <v...> <e...>
#   F <rc> <stamp> <w> <h> <stride> <format> <bytes>\n<base64>
#   R <rc>                                for O, C, E and anything that only returns a code
#   R <rc> <b64len>\n<base64>             the T reply: the code and the extension's answer
#
# `T` needs no protocol bump, and the request is one line on purpose. A child from before it
# answers an unknown verb with a bare `R EPROTO` — one line, no payload — and the host reads that
# shape as "this child predates extension" and reports `ENOSYS`, which is true. Had the request
# carried its payload on a second line, that old child would leave the line unread and poison every
# later exchange; as it is, the old child consumes the whole request as one unknown verb and the
# stream stays in sync. A child that implements `T` answers the two-line form, and the host tells
# them apart by the field count: it never reads a second line it was not told about.
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

_PROTOCOL_VERSION = 2
"""The host protocol this module speaks.

2 adds `Y` and §89.1's sensor channel. It is a hard bump and `drivers/python-host/python_host.c`
says why: a protocol-1 child has no thread on the second pipe, so a host that asked it for a
concurrent read would wait for a reply nobody will write — a hang, in the process holding the robot,
rather than a refusal at load.
"""

#: Where the sensor channel arrives. POSIX descriptors are fixed by the host; on Windows the two
#: inheritable handles are named in the environment, because there is no equivalent of a descriptor
#: number to agree on.
_SENSOR_IN_FD = 3
_SENSOR_OUT_FD = 4
_SENSOR_IN_VAR = "SKYNET_PYTHON_HOST_SENSOR_IN"
_SENSOR_OUT_VAR = "SKYNET_PYTHON_HOST_SENSOR_OUT"


def _concurrent_sensors(driver: Driver) -> list[str]:
    """The sensors this driver's own descriptor declares ``concurrent_read`` on (§89.1).

    Read out of ``describe()`` rather than asked for separately, because the descriptor is the
    source of truth for the body (§24's third rule) and a driver that answered one thing here and
    another there would have two accounts of one promise.

    Exactly ``true`` declares it — §5's ``detects_people`` rule, which §89.1 names — so a string
    ``"true"``, a ``1`` or a missing field all read as undeclared. A descriptor that does not parse
    declares nothing: the host is about to fail the load on it anyway, and guessing here would put a
    sensor on a thread on the strength of a document nobody could read.
    """
    try:
        document = json.loads(driver.describe())
        sensors = document.get("sensors") or []
    except Exception:  # noqa: BLE001 — see above: an unreadable descriptor declares nothing
        return []
    names = []
    for sensor in sensors:
        if not isinstance(sensor, dict):
            continue
        if sensor.get("concurrent_read") is not True:
            continue
        name = sensor.get("name")
        # A name with whitespace in it cannot cross the host's line protocol, and the host refuses
        # one it cannot route. Leaving it out here would be this side quietly disagreeing about
        # which sensors are declared, so it is left in and the host does the refusing.
        if isinstance(name, str) and name:
            names.append(name)
    return names


def _sensor_channel():
    """The host's second pipe, as ``(reader, writer)``, or ``None`` when there is not one.

    ``None`` is not an error: a driver run outside ``drivers/python-host`` — a test harness, or
    ``python -m`` by hand — has no second pipe and no need of one, and a driver that declares
    nothing never asks for it either.
    """
    try:
        if os.name == "nt":
            import msvcrt

            in_handle = os.environ.get(_SENSOR_IN_VAR)
            out_handle = os.environ.get(_SENSOR_OUT_VAR)
            if not in_handle or not out_handle:
                return None
            in_fd = msvcrt.open_osfhandle(int(in_handle), os.O_RDONLY)
            out_fd = msvcrt.open_osfhandle(int(out_handle), 0)
        else:
            in_fd, out_fd = _SENSOR_IN_FD, _SENSOR_OUT_FD
            # `fstat` rather than a try around the first read: an unopened descriptor here would
            # otherwise surface as a frame that never arrives.
            os.fstat(in_fd)
            os.fstat(out_fd)
        return (
            os.fdopen(in_fd, "r", buffering=1),
            os.fdopen(out_fd, "w", buffering=1),
        )
    except OSError:
        return None


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
    sensor_thread: threading.Thread | None = None
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

            if verb == "R":
                # §48.2's release. A driver without the method predates v2 and answers ENOSYS,
                # on `extension`'s precedent — and the host, which already cleared its own
                # latch, reports that honestly rather than claiming the child released.
                release = getattr(driver, "release_stop", None)
                if release is None:
                    send(f"R {SKR_ENOSYS}")
                else:
                    send(f"R {release()}")
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
                _serve_frame(driver, parts[1] if len(parts) > 1 else "", send)
                continue

            if verb == "T":
                _serve_extension(driver, parts, send)
                continue

            if verb == "Y":
                # §89.1, asked once after `open()`. The names, and then a thread to serve them —
                # started here rather than at import, because a driver that declares nothing must
                # get exactly the arrangement it had before protocol 2, down to the thread count.
                declared = _concurrent_sensors(driver)
                send("Y " + " ".join(declared) if declared else "Y")
                if declared and sensor_thread is None:
                    channel = _sensor_channel()
                    if channel is not None:
                        sensor_thread = threading.Thread(
                            target=_serve_sensor_channel,
                            args=(driver, channel, set(declared)),
                            name="skynet-rhal sensors",
                            daemon=True,
                        )
                        sensor_thread.start()
                continue

            if verb == "C":
                stamp, seq, mode, n = (int(parts[1]), int(parts[2]), int(parts[3]), int(parts[4]))
                numbers = parts[5:]
                if len(numbers) < 2 * n:
                    send(f"R {SKR_EPROTO}")
                    continue
                # §86.1's per-joint modes ride as trailing numbers when the
                # producer sent them — values[n] mask[n] modes[n] — and are
                # absent otherwise, which is every command written before §86.
                # Anything between is a framing error, not a short array.
                modes: list[int] = []
                if len(numbers) > 2 * n:
                    if len(numbers) != 3 * n:
                        send(f"R {SKR_EPROTO}")
                        continue
                    try:
                        modes = [int(m) for m in numbers[2 * n : 3 * n]]
                    except ValueError:
                        send(f"R {SKR_EPROTO}")
                        continue
                cmd = Command(
                    stamp_ns=stamp,
                    seq=seq,
                    mode=mode,
                    values=[float(v) for v in numbers[:n]],
                    mask=[int(float(m)) for m in numbers[n : 2 * n]],
                    modes=modes,
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


def _serve_frame(driver: Driver, name: str, send) -> None:
    """One ``F`` exchange: read the named sensor and write the reply and its payload.

    One implementation for both channels. Two would be two accounts of a wire format — and the one
    on the second channel would be the one nobody reads, because it only runs for a driver that has
    declared something.
    """
    frame = Frame()
    reader_fn = getattr(driver, "read_sensor", None)
    rc = reader_fn(name, frame) if reader_fn else SKR_ENOSYS
    if rc != SKR_OK:
        # No payload line on a failed read. An empty line here would sit in the stream with nothing
        # to consume it and put every later exchange one frame out — which presents as a protocol
        # error on an unrelated call, later.
        send(f"F {rc} 0 0 0 0 0 0")
        return
    payload = base64.b64encode(frame.data).decode("ascii")
    send(
        f"F {SKR_OK} {frame.stamp_ns} {frame.width} {frame.height} "
        f"{frame.stride} {frame.format} {len(frame.data)}"
    )
    send(payload)


#: The largest base64 payload a `T` exchange carries, either way. The host reads lines into a
#: 64 KiB buffer, so this is the line discipline both sides honour: anything larger is refused
#: before it is framed, because a half-read line desynchronises the stream and presents as a
#: protocol error on an unrelated call, later.
_T_B64_MAX = 65535


def _serve_extension(driver: Driver, parts: list[str], send) -> None:
    """One ``T`` exchange: the vendor escape hatch under ``token``, bytes in and bytes out.

    Framing errors inside this function answer in-frame (`R <rc> 0` plus an empty line) rather
    than raising to the serve loop, whose generic handler sends a bare `R` — one line where the
    host was promised two, which is exactly the desynchronisation the drain exists to mop up
    everywhere else.
    """
    try:
        token = parts[1] if len(parts) > 1 else ""
        payload = parts[2] if len(parts) > 2 else ""
        if not token or len(payload) > _T_B64_MAX:
            raise ValueError("malformed T request line")
        try:
            data = base64.b64decode(payload.encode("ascii")) if payload else b""
        except Exception as error:
            raise ValueError(f"T payload is not base64: {error}") from error
    except Exception as error:  # noqa: BLE001 — in-frame, as above
        print(f"skynet-rhal: malformed T exchange: {error}", file=sys.stderr, flush=True)
        send(f"R {SKR_EPROTO} 0")
        send("")
        return
    extend = getattr(driver, "extension", None)
    if extend is None:
        send(f"R {SKR_ENOSYS} 0")
        send("")
        return
    try:
        rc, reply = extend(token, data)
    except Exception as error:  # noqa: BLE001 — the loop's convention, in-frame
        # A driver bug must not take the robot with it: stop first, then report EIO — the same
        # order as the serve loop's generic handler, with the T reply shape instead of its bare R.
        print(f"skynet-rhal: driver raised {error!r}", file=sys.stderr, flush=True)
        try:
            driver.emergency_stop()
        except Exception:  # noqa: BLE001
            pass
        send(f"R {SKR_EIO} 0")
        send("")
        return
    if not isinstance(reply, (bytes, bytearray)):
        print("skynet-rhal: extension() must answer (rc, bytes)", file=sys.stderr, flush=True)
        send(f"R {SKR_EPROTO} 0")
        send("")
        return
    encoded = base64.b64encode(bytes(reply)).decode("ascii")
    if len(encoded) > _T_B64_MAX:
        send(f"R {SKR_EINVAL} 0")
        send("")
        return
    send(f"R {int(rc)} {len(encoded)}")
    send(encoded)


def _serve_sensor_channel(driver: Driver, channel, declared: set[str]) -> None:
    """§89.1's second channel: ``F`` for a declared sensor, and nothing else, for ever.

    What makes this safe is the driver's own declaration and nothing this module can check. The
    promise §89.1 defines is the driver's — ``read_sensor`` for *this* sensor may run while
    ``read_state`` and ``write_command`` do, without making them wait — and the two things the
    runtime promises back are kept here: ``read_sensor`` is never called concurrently with itself,
    because this is the only thread that calls it for a declared sensor and it serves one request at
    a time; and nothing else is called from this thread.

    An undeclared name is refused rather than served. The host never sends one, so a request for one
    means the two sides disagree about the declaration — and serving it would be this thread making
    a promise the driver did not.

    A driver exception is reported as a failed read and the channel stays open. The main loop's
    handler stops the robot on one, and it must stay the only place that does: a camera that raised
    is not a reason to stop a body, and the two paths reaching for the stop at once is exactly the
    concurrency this design is trying not to have.
    """
    reader, writer = channel

    def send(line: str) -> None:
        writer.write(line + "\n")
        writer.flush()

    while True:
        try:
            line = reader.readline()
        except (OSError, ValueError):
            return
        if not line:
            # The host closed it: `close()` and `emergency_stop()` belong to the main loop, which
            # sees the same end-of-file on its own channel.
            return
        parts = line.split()
        if not parts or parts[0] != "F":
            send(f"F {SKR_EPROTO} 0 0 0 0 0 0")
            continue
        name = parts[1] if len(parts) > 1 else ""
        if name not in declared:
            send(f"F {SKR_EPERM} 0 0 0 0 0 0")
            continue
        try:
            _serve_frame(driver, name, send)
        except Exception as error:  # noqa: BLE001 — a driver bug must not take the robot with it
            print(f"skynet-rhal: driver raised {error!r} on the sensor channel",
                  file=sys.stderr, flush=True)
            try:
                send(f"F {SKR_EIO} 0 0 0 0 0 0")
            except (OSError, ValueError):
                return


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
        # §86.1. Was `_pad[0]`, and zero in every ABI-1 command — §27's homing precedent one field
        # along: no offset moves and the ABI version does not bump.
        ("flags", ctypes.c_uint8),
        ("_pad", ctypes.c_uint8 * 2),
        ("joint_count", ctypes.c_uint16),
        ("values", ctypes.POINTER(ctypes.c_float)),
        ("mask", ctypes.POINTER(ctypes.c_uint8)),
        # [joint_count] of SKR_JMODE_*. Read only when `flags` carries SKR_CMD_PER_JOINT_MODES.
        ("modes", ctypes.POINTER(ctypes.c_uint8)),
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
        ("release_stop", ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p)),
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
        ("stamp_ns", 0), ("seq", 8), ("mode", 12), ("flags", 13), ("_pad", 14),
        ("joint_count", 16), ("values", 24), ("mask", 32), ("modes", 40),
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
