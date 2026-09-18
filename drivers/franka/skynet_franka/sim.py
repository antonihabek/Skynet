"""A fake Control box: the FCI's two transports, and the state machine behind them.

What `DRY-COVERAGE-PLAN.md`'s Phase 2 table calls *"protocol fake from public docs"*. There is no
Franka simulator — the FCI talks to a Control box and nothing else — so unlike URSim (a real
controller in a container) or `ros2_control`'s `mock_components` (the framework's own fake), this
one is written here.

That is the arrangement F15 warns about, and the README says exactly how far it reaches. Two things
are done about it:

* **The layout is the vendor's**, not this file's. Every offset both sides use comes out of
  `protocol.py`, which is pinned to `layout.json`, which came from compiling Franka's own headers.
  A wrong offset fails a check rather than agreeing with itself.
* **The refusals are modelled, not skipped.** A fake that answers every command with success is a
  fake that tests the happy path and nothing else, and the happy path is the part a driver gets
  right by accident. This one refuses a wrong protocol version, refuses `Move` while an error is
  latched, refuses a second `Move` while one is running, and stops the stream on `StopMove`.

## The 1 kHz is not simulated, and saying so is the point

A real Control box sends a `RobotState` every millisecond whether or not anybody reads, and a client
that misses ticks is told so in `control_command_success_rate`. This fake sends at a rate its caller
chooses, default 1 kHz, from a Python thread — which is a *nominal* rate and not a real-time one.
What that exercises is the driver's *behaviour* against a stream: latest-wins reads, a message id
that advances, a command that must be answered within a tick. What it cannot exercise is timing, and
timing is the whole of what Franka's `libfranka` claim is about.
"""

from __future__ import annotations

import math
import socket
import struct
import threading
import time

from . import protocol as fci


class FakeControlBox:
    """One fake robot: a TCP command port and a UDP state stream.

    Started with :meth:`start`, which binds and returns the port actually bound — zero means "let
    the kernel choose", which is what the suite uses so that two tests never collide on 1337.
    """

    def __init__(self, hz: float = 1000.0, joints: int = fci.JOINTS) -> None:
        self.hz = hz
        self.joints = joints
        #: Where the arm is. A real robot's home-ish pose rather than zeros: an all-zero Franka
        #: configuration is a singular one, and a fake whose start pose is singular would make
        #: `kStartAtSingularPoseRejected` unreachable — a refusal that exists precisely there.
        self.q = [0.0, -0.785, 0.0, -2.356, 0.0, 1.571, 0.785][:joints]
        self.dq = [0.0] * joints
        self.tau_J = [0.0] * joints
        self.robot_mode = fci.ROBOT_MODE_IDLE
        self.motion_mode = fci.MOTION_IDLE
        self.controller_mode = fci.CONTROLLER_JOINT_IMPEDANCE
        self.errors = [False] * fci.ERROR_COUNT
        self.message_id = 0
        #: The last command the client sent, for the suite to assert against.
        self.last_command: dict | None = None
        self.commands_seen = 0

        self._tcp: socket.socket | None = None
        self._udp: socket.socket | None = None
        self._client: tuple[str, int] | None = None
        self._conn: socket.socket | None = None
        self._stop = threading.Event()
        self._threads: list[threading.Thread] = []
        self._lock = threading.Lock()
        self.port = 0

    # ── Lifecycle ────────────────────────────────────────────────────────────

    def start(self, host: str = "127.0.0.1", port: int = 0) -> int:
        self._tcp = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._tcp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._tcp.bind((host, port))
        self._tcp.listen(1)
        self._tcp.settimeout(0.2)
        self.port = self._tcp.getsockname()[1]
        self._udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self._udp.bind((host, 0))
        # Non-blocking, and this is not a detail: the tick below drains whatever has arrived and
        # then sends the next state, so a timeout here is a floor under the period. With 50 ms on
        # it a box asked for 1 kHz sent twenty states a second, and the driver's own latest-wins
        # test — which needs a stream faster than its reader — measured the fake instead.
        self._udp.setblocking(False)
        thread = threading.Thread(target=self._serve_commands, name="fci-tcp", daemon=True)
        thread.start()
        self._threads.append(thread)
        return self.port

    def stop(self) -> None:
        self._stop.set()
        for thread in self._threads:
            thread.join(timeout=2.0)
        for sock in (self._conn, self._tcp, self._udp):
            try:
                if sock is not None:
                    sock.close()
            except OSError:
                pass

    # ── Faults, for the suite to inject ──────────────────────────────────────

    def latch_error(self, index: int = 0) -> None:
        """Set one of the 41 error bits, as a reflex or a collision would."""
        with self._lock:
            self.errors[index] = True
            self.robot_mode = fci.ROBOT_MODE_REFLEX
            self.motion_mode = fci.MOTION_IDLE

    def clear_errors(self) -> None:
        with self._lock:
            self.errors = [False] * fci.ERROR_COUNT
            self.robot_mode = fci.ROBOT_MODE_IDLE

    # ── TCP: the command port ────────────────────────────────────────────────

    def _serve_commands(self) -> None:
        while not self._stop.is_set():
            try:
                conn, _ = self._tcp.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            self._conn = conn
            conn.settimeout(0.2)
            buffer = b""
            while not self._stop.is_set():
                try:
                    chunk = conn.recv(4096)
                except socket.timeout:
                    continue
                except OSError:
                    break
                if not chunk:
                    break
                buffer += chunk
                # A stream, not a datagram: a header may arrive split and two commands may arrive
                # in one read. A fake that assumed one recv per command would let a driver that
                # writes two commands back to back look correct here and stall on a real box.
                while len(buffer) >= fci.HEADER_SIZE:
                    header = fci.Header.unpack(buffer)
                    if len(buffer) < header.size:
                        break
                    message, buffer = buffer[: header.size], buffer[header.size :]
                    reply = self._answer(header, message[fci.HEADER_SIZE :])
                    if reply:
                        try:
                            conn.sendall(reply)
                        except OSError:
                            break
            try:
                conn.close()
            except OSError:
                pass
            self._conn = None

    def _answer(self, header: fci.Header, payload: bytes) -> bytes:
        if header.command == fci.COMMAND_CONNECT:
            version, udp_port = fci.CONNECT_REQUEST.unpack(payload[: fci.CONNECT_REQUEST.size])
            if version != fci.VERSION:
                # The vendor's own version gate, and the reason a driver must send `Connect` first:
                # everything after it is fixed-width structs whose meaning is the version's.
                return fci.pack_response(
                    fci.COMMAND_CONNECT,
                    header.command_id,
                    fci.CONNECT_INCOMPATIBLE_LIBRARY_VERSION,
                    struct.pack("<H", fci.VERSION),
                )
            self._client = (self._peer_host(), udp_port)
            thread = threading.Thread(target=self._stream, name="fci-udp", daemon=True)
            thread.start()
            self._threads.append(thread)
            return fci.pack_response(
                fci.COMMAND_CONNECT,
                header.command_id,
                fci.CONNECT_SUCCESS,
                struct.pack("<H", fci.VERSION),
            )

        if header.command == fci.COMMAND_MOVE:
            with self._lock:
                if any(self.errors):
                    return fci.pack_response(
                        fci.COMMAND_MOVE, header.command_id, fci.MOVE_REJECTED_DUE_TO_SAFETY
                    )
                if self.motion_mode != fci.MOTION_IDLE:
                    return fci.pack_response(
                        fci.COMMAND_MOVE,
                        header.command_id,
                        fci.MOVE_COMMAND_NOT_POSSIBLE_REJECTED,
                    )
                controller, motion = struct.unpack_from("<II", payload, 0)
                self.controller_mode = (
                    fci.CONTROLLER_EXTERNAL
                    if controller == fci.MOVE_CONTROLLER_EXTERNAL
                    else fci.CONTROLLER_JOINT_IMPEDANCE
                )
                self.motion_mode = {
                    fci.MOVE_MOTION_JOINT_POSITION: fci.MOTION_JOINT_POSITION,
                    fci.MOVE_MOTION_JOINT_VELOCITY: fci.MOTION_JOINT_VELOCITY,
                    fci.MOVE_MOTION_NONE: fci.MOTION_NONE,
                }.get(motion, fci.MOTION_NONE)
                self.robot_mode = fci.ROBOT_MODE_MOVE
            return fci.pack_response(fci.COMMAND_MOVE, header.command_id, fci.MOVE_STARTED)

        if header.command == fci.COMMAND_STOP_MOVE:
            with self._lock:
                self.motion_mode = fci.MOTION_IDLE
                self.robot_mode = fci.ROBOT_MODE_IDLE
                self.dq = [0.0] * self.joints
            return fci.pack_response(fci.COMMAND_STOP_MOVE, header.command_id, 0)

        if header.command == fci.COMMAND_AUTOMATIC_ERROR_RECOVERY:
            self.clear_errors()
            return fci.pack_response(
                fci.COMMAND_AUTOMATIC_ERROR_RECOVERY, header.command_id, 0
            )

        if header.command in (
            fci.COMMAND_SET_COLLISION_BEHAVIOR,
            fci.COMMAND_SET_JOINT_IMPEDANCE,
            fci.COMMAND_SET_CARTESIAN_IMPEDANCE,
            fci.COMMAND_SET_LOAD,
        ):
            return fci.pack_response(header.command, header.command_id, 0)

        # An unknown command is refused with its own id rather than ignored: a client waiting for a
        # reply that never comes cannot tell a rejected command from a dropped one.
        return fci.pack_response(header.command, header.command_id, 1)

    def _peer_host(self) -> str:
        try:
            return self._conn.getpeername()[0] if self._conn else "127.0.0.1"
        except OSError:
            return "127.0.0.1"

    # ── UDP: the robot at rate ───────────────────────────────────────────────

    def _stream(self) -> None:
        period = 1.0 / self.hz
        next_tick = time.monotonic()
        while not self._stop.is_set() and self._client is not None:
            now = time.monotonic()
            if now < next_tick:
                time.sleep(min(period, next_tick - now))
                continue
            next_tick += period
            self._integrate(period)
            with self._lock:
                message = fci.pack_robot_state(
                    self.message_id,
                    self.q,
                    self.dq,
                    self.tau_J,
                    robot_mode=self.robot_mode,
                    motion_generator_mode=self.motion_mode,
                    controller_mode=self.controller_mode,
                    errors=self.errors,
                )
                self.message_id += 1
            try:
                self._udp.sendto(message, self._client)
            except OSError:
                return
            # Read whatever the client sent since the last tick, and keep the newest. A real box
            # reads one command per tick; draining is what makes a client that is behind visible as
            # a low success rate rather than as a queue that grows.
            while True:
                try:
                    raw, _ = self._udp.recvfrom(4096)
                except BlockingIOError:
                    break
                except OSError:
                    return
                if len(raw) != fci.ROBOT_COMMAND_SIZE:
                    continue
                with self._lock:
                    self.last_command = fci.unpack_robot_command(raw)
                    self.commands_seen += 1
                    self._apply(self.last_command)

    def _apply(self, command: dict) -> None:
        if self.motion_mode == fci.MOTION_JOINT_POSITION:
            self.q = list(command["q_c"][: self.joints])
        elif self.motion_mode == fci.MOTION_JOINT_VELOCITY:
            self.dq = list(command["dq_c"][: self.joints])
        if self.controller_mode == fci.CONTROLLER_EXTERNAL:
            self.tau_J = list(command["tau_J_d"][: self.joints])

    def _integrate(self, dt: float) -> None:
        with self._lock:
            if self.motion_mode == fci.MOTION_JOINT_VELOCITY:
                self.q = [q + v * dt for q, v in zip(self.q, self.dq)]
            # Gravity torque, so that `tau_J` is never a column of zeros a reader could mistake for
            # a measurement nobody took. Not a model of anything — a smooth function of the pose.
            if self.controller_mode != fci.CONTROLLER_EXTERNAL:
                self.tau_J = [round(2.0 * math.sin(q), 6) for q in self.q]
