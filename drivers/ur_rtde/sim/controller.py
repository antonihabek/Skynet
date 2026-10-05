"""A Universal Robots controller's three sockets, written here, for the link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4).

    PYTHONPATH=bindings/python:drivers/ur_rtde python3 tools/faultproxy/chaos.py drivers/ur_rtde/chaos.py

The bridge's own suite runs against objects that stand in for its RTDE and dashboard sessions, and `make check-ursim-docker` against URSim, UR's own controller; neither puts a *relay* between the bridge and a
controller, and a relay is what the contract needs: something on the wire that delays, stalls, blackholes and resets. This is the smallest controller that serves one over real sockets:

| socket | what it serves |
|---|---|
| dashboard | the line commands the bridge sends (`robotmode`, `safetystatus`, `power on`, `brake release`, `stop`, `play`, `running`) and the banner |
| script | takes the control program and runs it: a program is "running" from the moment one arrives, until a dashboard `stop` |
| RTDE | protocol 2: the version, the two recipes, `start` and `pause`, a data package at the negotiated frequency, and the input registers a client writes |

An arm of six joints moves toward the setpoint the control program would hand `servoj` — the six input float registers, while the gate register says servo and the program is running — at no more than `joint_max_speed`.
RTDE's general-purpose registers belong to one session at a time and are freed a moment after the session's socket goes, as URSim does (`REGISTER_RELEASE_S` in the bridge cites the measurement).

**This controller agrees with whoever wrote it** (F15). It says what the bridge does with a link that is worse than a cable, and nothing about URControl: its frame layout is the bridge's own client's, which was measured against URSim, and its
timing is no controller's. It is the substrate of a contract about a driver's waits, staleness and stop path, and it is not evidence about a UR.
"""

from __future__ import annotations

import socket
import struct
import threading
import time

#: What RTDE calls each variable the bridge reads, and the type it answers for it. A name not here is `NOT_FOUND`, as a controller answers for a variable it does not have.
OUTPUT_TYPES = {
    "actual_q": "VECTOR6D",
    "actual_qd": "VECTOR6D",
    "actual_current": "VECTOR6D",
    "robot_mode": "INT32",
    "safety_status": "INT32",
    "runtime_state": "UINT32",
}
INPUT_TYPES = {**{f"input_double_register_{index}": "DOUBLE" for index in range(24)}, **{f"input_int_register_{index}": "INT32" for index in range(24)}}

_FORMAT = {"DOUBLE": ">d", "INT32": ">i", "UINT32": ">I", "VECTOR6D": ">6d"}
_SAFETY = {1: "NORMAL", 2: "REDUCED", 3: "PROTECTIVE_STOP", 4: "RECOVERY", 5: "SAFEGUARD_STOP", 6: "SYSTEM_EMERGENCY_STOP", 7: "ROBOT_EMERGENCY_STOP", 8: "VIOLATION", 9: "FAULT"}
_MODES = {"POWER_OFF": 3, "IDLE": 5, "RUNNING": 7}

#: RTDE packet types.
_V, _VERSION, _TEXT, _OUTPUTS, _INPUTS, _START, _PAUSE, _DATA = 86, 118, 77, 79, 73, 83, 80, 85


def _packet(kind: int, payload: bytes = b"") -> bytes:
    return struct.pack(">HB", 3 + len(payload), kind) + payload


class SimulatedController:
    """The three sockets and the arm behind them. `start()` binds each on a free loopback port (`ports`), `stop()` closes them all."""

    def __init__(self, host: str = "127.0.0.1", joint_max_speed: float = 1.0, release_delay_s: float = 0.06, mode: str = "RUNNING") -> None:
        self.host = host
        self.joint_max_speed = joint_max_speed
        self.release_delay_s = release_delay_s
        self.axis = 6
        self.position = [0.0, -1.2, 1.2, -1.0, -1.5, 0.0]
        self.velocity = [0.0] * 6
        self.registers = [0.0] * 6
        self.gate = 0
        self.mode = mode
        self.safety = 1
        self.program: str | None = None
        self.running = False
        #: What the control program sees from a client: how many data packages reached the input registers.
        self.inputs_received = 0
        #: Every dashboard command received, in order.
        self.dashboard_log: list[str] = []
        #: A dashboard that takes a command and never answers it: the stop's request waits on a controller that has gone quiet.
        self.dashboard_mute = False
        self.ports: dict[str, int] = {}
        self._lock = threading.Lock()
        self._stopping = threading.Event()
        self._listeners: dict[str, socket.socket] = {}
        self._threads: list[threading.Thread] = []
        self._connections: list[socket.socket] = []
        self._register_owner: object | None = None
        self._register_free_at = 0.0

    # ── lifecycle ────────────────────────────────────────────────────────────

    def start(self) -> "SimulatedController":
        for name, serve in (("dashboard", self._serve_dashboard), ("script", self._serve_script), ("rtde", self._serve_rtde)):
            listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.bind((self.host, 0))
            listener.listen(8)
            listener.settimeout(0.2)
            self._listeners[name] = listener
            self.ports[name] = listener.getsockname()[1]
            self._spawn(self._accept, listener, serve)
        self._spawn(self._physics)
        return self

    def stop(self) -> None:
        self._stopping.set()
        for listener in self._listeners.values():
            try:
                listener.close()
            except OSError:
                pass
        with self._lock:
            connections, self._connections = self._connections, []
        for connection in connections:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            try:
                connection.close()
            except OSError:
                pass
        for thread in self._threads:
            thread.join(timeout=1.0)
        self._threads = []

    def _spawn(self, target, *args) -> None:
        thread = threading.Thread(target=target, args=args, daemon=True)
        thread.start()
        self._threads.append(thread)

    def _accept(self, listener: socket.socket, serve) -> None:
        while not self._stopping.is_set():
            try:
                connection, _ = listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            with self._lock:
                self._connections.append(connection)
            self._spawn(serve, connection)

    # ── the arm ──────────────────────────────────────────────────────────────

    def _physics(self) -> None:
        period = 1.0 / 125.0
        due = time.monotonic()
        while not self._stopping.is_set():
            due += period
            with self._lock:
                moving = self.running and self.gate == 1 and self.mode == "RUNNING" and self.safety == 1
                reach = self.joint_max_speed * period
                for index in range(6):
                    if moving:
                        step = max(-reach, min(reach, self.registers[index] - self.position[index]))
                    else:
                        step = 0.0
                    self.position[index] += step
                    self.velocity[index] = step / period
            pause = due - time.monotonic()
            if pause > 0:
                time.sleep(pause)
            else:
                due = time.monotonic()

    # ── the dashboard ────────────────────────────────────────────────────────

    def _serve_dashboard(self, connection: socket.socket) -> None:
        try:
            connection.sendall(b"Connected: Universal Robots Dashboard Server\n")
            pending = b""
            while not self._stopping.is_set():
                chunk = connection.recv(4096)
                if not chunk:
                    return
                pending += chunk
                while b"\n" in pending:
                    line, pending = pending.split(b"\n", 1)
                    text = line.decode("utf-8", errors="replace").strip()
                    self.dashboard_log.append(text)
                    if self.dashboard_mute:
                        continue
                    connection.sendall(self._dashboard_reply(text).encode() + b"\n")
        except OSError:
            return

    def _dashboard_reply(self, command: str) -> str:
        with self._lock:
            if command == "robotmode":
                return f"Robotmode: {self.mode}"
            if command == "safetystatus":
                return f"Safetystatus: {_SAFETY.get(self.safety, 'FAULT')}"
            if command == "power on":
                if self.mode == "POWER_OFF":
                    self.mode = "IDLE"
                return "Powering on"
            if command == "brake release":
                if self.mode == "IDLE":
                    self.mode = "RUNNING"
                return "Brake releasing"
            if command == "stop":
                self.running = False
                return "Stopped"
            if command == "play":
                if self.program is None:
                    return "Failed to execute: play"
                self.running = True
                return "Starting program"
            if command == "running":
                return f"Program running: {'true' if self.running else 'false'}"
        return f"could not understand: '{command}'"

    # ── the script socket ────────────────────────────────────────────────────

    def _serve_script(self, connection: socket.socket) -> None:
        text = b""
        try:
            while not self._stopping.is_set():
                chunk = connection.recv(4096)
                if not chunk:
                    return
                text += chunk
                with self._lock:
                    self.program = text.decode("utf-8", errors="replace")
                    self.running = True
        except OSError:
            return

    # ── RTDE ─────────────────────────────────────────────────────────────────

    def _serve_rtde(self, connection: socket.socket) -> None:
        session = _Session(connection)
        try:
            buffer = b""
            while not self._stopping.is_set():
                chunk = connection.recv(4096)
                if not chunk:
                    return
                buffer += chunk
                while len(buffer) >= 3:
                    size, kind = struct.unpack(">HB", buffer[:3])
                    if len(buffer) < size:
                        break
                    payload, buffer = buffer[3:size], buffer[size:]
                    self._handle(session, kind, payload)
        except OSError:
            return
        finally:
            session.publishing = False
            self._release(session)

    def _handle(self, session: "_Session", kind: int, payload: bytes) -> None:
        if kind == _V:
            session.send(_packet(_V, b"\x01" if struct.unpack(">H", payload)[0] == 2 else b"\x00"))
        elif kind == _VERSION:
            session.send(_packet(_VERSION, struct.pack(">IIII", 5, 26, 0, 1234)))
        elif kind == _OUTPUTS:
            frequency = struct.unpack(">d", payload[:8])[0]
            names = payload[8:].decode().split(",")
            session.frequency = frequency
            session.output_names = names
            session.output_types = [OUTPUT_TYPES.get(name, "NOT_FOUND") for name in names]
            session.send(_packet(_OUTPUTS, bytes([1]) + ",".join(session.output_types).encode()))
        elif kind == _INPUTS:
            names = payload.decode().split(",")
            with self._lock:
                free = self._register_owner in (None, session) and time.monotonic() >= self._register_free_at
                if free:
                    self._register_owner = session
            types = [INPUT_TYPES.get(name, "NOT_FOUND") if free else "IN_USE" for name in names]
            if free:
                session.input_names = names
                session.input_types = types
            session.send(_packet(_INPUTS, bytes([2]) + ",".join(types).encode()))
        elif kind == _START:
            ready = bool(session.output_names)
            session.send(_packet(_START, b"\x01" if ready else b"\x00"))
            if ready:
                session.publishing = True
                self._spawn(self._publish, session)
        elif kind == _PAUSE:
            session.publishing = False
            session.send(_packet(_PAUSE, b"\x01"))
        elif kind == _DATA and session.input_types and payload and payload[0] == 2:
            self._receive(session, payload[1:])

    def _receive(self, session: "_Session", body: bytes) -> None:
        at = 0
        values = {}
        for name, kind in zip(session.input_names, session.input_types):
            fmt = _FORMAT[kind]
            width = struct.calcsize(fmt)
            if at + width > len(body):
                return
            values[name] = struct.unpack(fmt, body[at : at + width])[0]
            at += width
        with self._lock:
            for index in range(6):
                name = f"input_double_register_{index}"
                if name in values:
                    self.registers[index] = values[name]
            if "input_int_register_0" in values:
                self.gate = values["input_int_register_0"]
            self.inputs_received += 1

    def _release(self, session: "_Session") -> None:
        with self._lock:
            if self._register_owner is session:
                self._register_owner = None
                self._register_free_at = time.monotonic() + self.release_delay_s

    def _publish(self, session: "_Session") -> None:
        period = 1.0 / max(session.frequency, 1.0)
        due = time.monotonic()
        while session.publishing and not self._stopping.is_set():
            due += period
            with self._lock:
                values = {
                    "actual_q": list(self.position),
                    "actual_qd": list(self.velocity),
                    "actual_current": [0.0] * 6,
                    "robot_mode": _MODES.get(self.mode, 3),
                    "safety_status": self.safety,
                    "runtime_state": 2 if self.running else 1,
                }
            body = bytearray([1])
            for name, kind in zip(session.output_names, session.output_types):
                value = values[name]
                body += struct.pack(_FORMAT[kind], *value) if isinstance(value, list) else struct.pack(_FORMAT[kind], value)
            if not session.send(_packet(_DATA, bytes(body))):
                return
            pause = due - time.monotonic()
            if pause > 0:
                time.sleep(pause)
            else:
                due = time.monotonic()


class _Session:
    """One RTDE client's connection and what it has set up."""

    def __init__(self, connection: socket.socket) -> None:
        self.connection = connection
        self.frequency = 125.0
        self.output_names: list[str] = []
        self.output_types: list[str] = []
        self.input_names: list[str] = []
        self.input_types: list[str] = []
        self.publishing = False
        self._write = threading.Lock()

    def send(self, data: bytes) -> bool:
        try:
            with self._write:
                self.connection.sendall(data)
            return True
        except OSError:
            self.publishing = False
            return False
