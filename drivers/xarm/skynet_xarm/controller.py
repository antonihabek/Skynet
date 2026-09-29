"""A simulated xArm controller — the dry substrate this driver is measured against first.

`DRY-COVERAGE-PLAN.md` Phase 10 item 10.5 asks for a driver and then its substrate, and the substrate
that matters is UFACTORY's own controller firmware in its simulator image (`make check-sim-docker`).
This is the one that runs anywhere: the same two kinds of socket, on loopback, with the behaviour
measured on that firmware written into it — and *written here*, which is why it ranks last on the
plan's ladder (F118's rung 4). It is what the driver's own suite runs against, and what the vendor's
own `XArmAPI` is pointed at in `cross_check.py` to show the wire it speaks is one the vendor's client
accepts.

# What it does, and where each behaviour was measured

Every line below was first observed on the 2.4.0 controller in `danielwang123321/uf-ubuntu-docker`
(6 axes, type 6), by speaking the register table directly, and is held there by `make
check-sim-docker` running the same suite against the real thing:

* `SET_MODE` answers with the *not ready* bit set until `SET_STATE 0` follows it, and `GET_STATE`
  then reads 2. Servo mode is mode 1.
* `MOVE_SERVOJ` in servo mode moves each joint toward its target no faster than the joint speed
  ceiling. In any other mode it answers as if it had taken the target and does nothing with it.
* **A target beyond a joint's range is acknowledged.** The reply's state byte is clean, the arm
  starts to move, and a moment later the controller stops with an error and state 4. That is the
  property this driver's own range check exists for.
* `SET_STATE 4` stops the arm without an error: state 4, replies carry the *not ready* bit, and
  `SET_STATE 0` runs it again. With an error latched it takes `CLEAN_ERR` first.
* A register the firmware does not know is not answered at all.
* The rich report says the controller is a simulation (`is_simulation_robot`), the joint ranges it
  holds, and the motors' enable byte — 255 on the simulator, which is why `MOTION_EN` (the
  simulator never replies to it) has to be skipped when the byte already says every axis is on.

# What it is not

A robot, and not UFACTORY's controller either: it has no dynamics, no self-collision check (the real
one raised error 22 for a J3 target beyond its range before any range check I could name), no
temperature, no brake, and it moves in straight lines at the speed ceiling. Nothing it reports is
evidence about a machine, and the descriptor above it declares `simulator: true` because this says so.
"""

from __future__ import annotations

import math
import socket
import struct
import threading
import time
from typing import Callable

from .frames import Fun, State

__all__ = ["SimulatedController", "XARM6_LIMITS"]

#: The xArm 6's joint ranges (type 6, `XCONF.Robot.JOINT_LIMITS` — J1 ±2π, J2 -2.059488..2.094395,
#: J3 -3.92699..0.191986, J4 ±2π, J5 -1.692969..π, J6 ±2π), the seventh entry unused.
XARM6_LIMITS: tuple[tuple[float, float], ...] = (
    (-2 * math.pi, 2 * math.pi),
    (-2.059488, 2.094395),
    (-3.92699, 0.191986),
    (-2 * math.pi, 2 * math.pi),
    (-1.692969, math.pi),
    (-2 * math.pi, 2 * math.pi),
    (0.0, 0.0),
)

#: The error the firmware raised when a servo target was out of range (and that this simulates).
RANGE_ERROR = 22


class SimulatedController:
    """One controller: a control socket, a fast report socket and a rich one, and an arm behind them."""

    def __init__(
        self,
        *,
        axis: int = 6,
        robot_type: int = 6,
        version: str = "v2.4.0",
        simulation: bool = True,
        motors_enabled: bool = True,
        limits: tuple[tuple[float, float], ...] = XARM6_LIMITS,
        joint_max_speed: float = math.pi,
        fast_hz: int = 100,
        rich_hz: int = 10,
        error_code: int = 0,
        state_after_boot: int = State.READY,
        fault_delay_s: float = 0.12,
    ) -> None:
        self.axis = axis
        self.robot_type = robot_type
        self.version = version
        self.simulation = simulation
        self.limits = limits
        self.joint_max_speed = joint_max_speed
        self._fast_period = 1.0 / fast_hz
        self._rich_every = max(1, fast_hz // rich_hz)
        self._fault_delay = fault_delay_s
        self._lock = threading.RLock()
        self.mode = 0
        self.state = state_after_boot
        self._running = True  # whether SET_STATE 0 has been received since the last stop or mode change
        self.error_code = error_code
        self.warn_code = 0
        self.motors_enabled = ((1 << axis) - 1) if motors_enabled else 0
        self.position = [0.0] * 7
        self.target = [0.0] * 7
        self.command_number = 0
        self._fault_countdown: float | None = None
        #: What was asked of this controller, for the tests that want to say what it did *not* hear.
        self.log: list[tuple[int, bytes]] = []
        self.servoj_count = 0
        self.stop_count = 0
        #: Register numbers the controller will not answer, on top of the ones it does not know.
        self.mute: set[int] = set()
        self.report_sockets: list[socket.socket] = []
        self._rich_sockets: list[socket.socket] = []
        self._listeners: list[socket.socket] = []
        self._threads: list[threading.Thread] = []
        self._closing = threading.Event()
        self.ports: dict[str, int] = {}
        #: Frames withheld from the fast report — a controller whose stream has gone quiet.
        self.fast_paused = threading.Event()
        self._handlers: dict[int, Callable[[bytes], tuple[bytes, bool] | None]] = {
            Fun.GET_VERSION: self._get_version,
            Fun.GET_STATE: self._get_state,
            Fun.GET_ERROR: self._get_error,
            Fun.CLEAN_ERR: self._clean_err,
            Fun.CLEAN_WAR: self._clean_war,
            Fun.SET_MODE: self._set_mode,
            Fun.SET_STATE: self._set_state,
            Fun.MOTION_EN: self._motion_en,
            Fun.MOVE_SERVOJ: self._servoj,
        }

    # ── lifecycle ────────────────────────────────────────────────────────────

    def start(self) -> "SimulatedController":
        for name, serve in (
            ("control", self._serve_control),
            ("fast", self._serve_fast),
            ("rich", self._serve_rich),
        ):
            listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.bind(("127.0.0.1", 0))
            listener.listen(8)
            self._listeners.append(listener)
            self.ports[name] = listener.getsockname()[1]
            thread = threading.Thread(target=serve, args=(listener,), daemon=True, name=f"xarm-{name}")
            thread.start()
            self._threads.append(thread)
        ticker = threading.Thread(target=self._tick_loop, daemon=True, name="xarm-ticker")
        ticker.start()
        self._threads.append(ticker)
        return self

    def stop(self) -> None:
        self._closing.set()
        for listener in self._listeners:
            try:
                listener.close()
            except OSError:
                pass
        with self._lock:
            for sock in self.report_sockets + self._rich_sockets:
                try:
                    sock.close()
                except OSError:
                    pass

    def __enter__(self) -> "SimulatedController":
        return self.start()

    def __exit__(self, *exc) -> None:
        self.stop()

    # ── the control port ─────────────────────────────────────────────────────

    def _serve_control(self, listener: socket.socket) -> None:
        while not self._closing.is_set():
            try:
                conn, _ = listener.accept()
            except OSError:
                return
            threading.Thread(target=self._control_connection, args=(conn,), daemon=True).start()

    def _control_connection(self, conn: socket.socket) -> None:
        conn.settimeout(0.5)
        try:
            while not self._closing.is_set():
                header = self._read_exact(conn, 6)
                if header is None:
                    return
                transaction, protocol, length = struct.unpack(">HHH", header)
                body = self._read_exact(conn, length)
                if body is None or not body:
                    return
                function, data = body[0], body[1:]
                with self._lock:
                    self.log.append((function, bytes(data)))
                handler = self._handlers.get(function)
                if handler is None or function in self.mute:
                    continue  # the firmware does not answer what it does not know
                with self._lock:
                    answered = handler(bytes(data))
                    state_byte = self._state_byte()
                if answered is None:
                    continue
                payload, _ = answered
                reply = struct.pack(">HHHBB", transaction, protocol, len(payload) + 2, function, state_byte)
                conn.sendall(reply + payload)
        except OSError:
            return
        finally:
            try:
                conn.close()
            except OSError:
                pass

    def _read_exact(self, conn: socket.socket, count: int) -> bytes | None:
        buffer = b""
        while len(buffer) < count:
            if self._closing.is_set():
                return None
            try:
                chunk = conn.recv(count - len(buffer))
            except socket.timeout:
                continue
            if not chunk:
                return None
            buffer += chunk
        return buffer

    def _state_byte(self) -> int:
        byte = 0
        if self.state == State.STOPPED or not self._running:
            byte |= 0x10
        if self.error_code:
            byte |= 0x40
        if self.warn_code:
            byte |= 0x20
        return byte

    # registers ---------------------------------------------------------------

    def _get_version(self, _: bytes):
        text = f"{self.axis},{self.robot_type}, ,XX0000,{self.version}".encode()
        return text.ljust(40, b"\0"), True

    def _get_state(self, _: bytes):
        return bytes([self.state]), True

    def _get_error(self, _: bytes):
        return bytes([self.error_code, self.warn_code]), True

    def _clean_err(self, _: bytes):
        self.error_code = 0
        return b"", True

    def _clean_war(self, _: bytes):
        self.warn_code = 0
        return b"", True

    def _set_mode(self, data: bytes):
        if data:
            self.mode = data[0]
        self._running = False  # the reply says *not ready* until SET_STATE 0 follows
        return b"", True

    def _set_state(self, data: bytes):
        value = data[0] if data else 0
        if value == State.STOP:
            self.stop_count += 1
            self.state = State.STOPPED
            self._running = False
            self.target = list(self.position)
            self._fault_countdown = None
        elif value == State.RUN:
            if self.error_code:
                self.state = State.STOPPED
            else:
                self.state = State.READY
                self._running = True
        return b"", True

    def _motion_en(self, data: bytes):
        if len(data) >= 2:
            axis, on = data[0], data[1]
            bits = ((1 << self.axis) - 1) if axis == 8 else (1 << (axis - 1))
            self.motors_enabled = (self.motors_enabled | bits) if on else (self.motors_enabled & ~bits)
        return b"", True

    def _servoj(self, data: bytes):
        self.servoj_count += 1
        self.command_number = (self.command_number + 1) & 0xFFFF
        if len(data) < 28 or self.state == State.STOPPED or not self._running or self.mode != 1:
            return b"", True  # acknowledged, and nothing is done with it: as the firmware does
        angles = struct.unpack("<7f", data[:28])
        self.target = list(angles)
        out_of_range = any(
            not lo <= angles[i] <= hi for i, (lo, hi) in enumerate(self.limits[: self.axis])
        )
        if out_of_range and self._fault_countdown is None:
            self._fault_countdown = self._fault_delay
        return b"", True

    # ── the report ports ─────────────────────────────────────────────────────

    def _serve_fast(self, listener: socket.socket) -> None:
        self._serve_reports(listener, self.report_sockets)

    def _serve_rich(self, listener: socket.socket) -> None:
        self._serve_reports(listener, self._rich_sockets)

    def _serve_reports(self, listener: socket.socket, into: list[socket.socket]) -> None:
        while not self._closing.is_set():
            try:
                conn, _ = listener.accept()
            except OSError:
                return
            with self._lock:
                into.append(conn)

    def _tick_loop(self) -> None:
        ticks = 0
        next_at = time.monotonic()
        while not self._closing.is_set():
            next_at += self._fast_period
            with self._lock:
                self._step(self._fast_period)
                fast = None if self.fast_paused.is_set() else self._fast_frame()
                rich = self._rich_frame() if ticks % self._rich_every == 0 else None
                fast_sockets = list(self.report_sockets)
                rich_sockets = list(self._rich_sockets)
            for sockets, frame, sink in (
                (fast_sockets, fast, self.report_sockets),
                (rich_sockets, rich, self._rich_sockets),
            ):
                if frame is None:
                    continue
                for conn in sockets:
                    try:
                        conn.sendall(frame)
                    except OSError:
                        with self._lock:
                            if conn in sink:
                                sink.remove(conn)
            ticks += 1
            delay = next_at - time.monotonic()
            if delay > 0:
                time.sleep(delay)
            else:
                next_at = time.monotonic()

    def _step(self, dt: float) -> None:
        if self.state == State.STOPPED or self.mode != 1 or not self._running:
            return
        moving = False
        for index in range(self.axis):
            gap = self.target[index] - self.position[index]
            if abs(gap) > 1e-9:
                step = min(abs(gap), self.joint_max_speed * dt)
                self.position[index] += math.copysign(step, gap)
                moving = moving or abs(gap) > step
        if self._fault_countdown is not None:
            self._fault_countdown -= dt
            if self._fault_countdown <= 0:
                self._fault_countdown = None
                self.error_code = RANGE_ERROR
                self.state = State.STOPPED
                self._running = False
                self.target = list(self.position)
                return
        self.state = State.MOVING if moving else State.READY

    def _head(self, size: int) -> bytearray:
        frame = bytearray(size)
        struct.pack_into(">I", frame, 0, size)
        frame[4] = (self.mode << 4) | (self.state & 0x0F)
        struct.pack_into(">H", frame, 5, self.command_number)
        struct.pack_into("<7f", frame, 7, *self.position)
        return frame

    def _fast_frame(self) -> bytes:
        return bytes(self._head(135))

    def _rich_frame(self) -> bytes:
        frame = self._head(574)
        frame[87] = 0xFF  # brakes, as the simulator reports
        frame[88] = self.motors_enabled & 0xFF
        frame[89] = self.error_code
        frame[90] = self.warn_code
        frame[145] = self.robot_type
        frame[146] = self.axis
        version = self.version.encode()[:29]
        frame[151 : 151 + len(version)] = version
        struct.pack_into("<f", frame, 217, self.joint_max_speed)
        frame[314] = 1 if self.simulation else 0
        limits = [value for pair in self.limits for value in pair]
        struct.pack_into("<14f", frame, 516, *limits)
        return bytes(frame)

    # ── things the tests do to the arm ───────────────────────────────────────

    def latch_error(self, code: int) -> None:
        """A fault the controller raises on its own — an encoder, a limit, a person at the teach pendant."""
        with self._lock:
            self.error_code = code
            self.state = State.STOPPED
            self._running = False
            self.target = list(self.position)
