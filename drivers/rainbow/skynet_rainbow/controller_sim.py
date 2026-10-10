"""A simulated Rainbow RB controller, the dry substrate: the command port (a script a line, acknowledged and then answered by `info`, `warn` and `error` lines) and the data port (`reqdata` and a 580-byte state packet) on two
loopback ports, and an arm that follows a servo target or a joint move (DRY-COVERAGE-PLAN.md item 12.E.14).

**It agrees with whoever wrote it** (F15): it was written from the maker's documents and from what Rainbow's own Virtual Control Box (software 8.4.4, run under QEMU) was seen to do (`drivers/rainbow/README.md`), and it is
accepted by the maker's own client because it was made to be. What it holds is what the driver does with these lines and packets, and nothing about an arm: it has no brakes, no payload, no safety board and no real mode that
means anything.

What it copies from the Virtual Control Box, each seen on the wire: every line it parses is acknowledged (`The command was executed`) before anything else it says; a motion's start and end are `info[motion_changed][N]`
(N is not 0 while moving); a refused target or parameter is acknowledged and then `warn[code][562]` or `warn[code][568]`; `task stop` and `halt` end a motion (`info[system][halt]`) and **do not latch**, so the next
servo command moves the arm again; `task pause` sets the task state to 2 and does not stop a servo stream; a motion command sent while paused is `warn[code][561]` and a halt; and a line it cannot parse is a syntax
`error[msg]` spread over four lines. What it does not copy: the Virtual Control Box accepts any first word and a gain or an alpha of 0, and this refuses neither, because nothing the driver does depends on it.
"""

from __future__ import annotations

import re
import socket
import threading
import time

from . import frames

#: How fast the simulated arm follows a servo target, in degrees a second: the simulation's own figure (the maker publishes none).
MAX_SPEED_DEG_S = 400.0
_NUMBER = r"\s*([-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?)\s*"
_JNT = r"jnt\[" + ",".join([_NUMBER] * 6) + r"\]"
_SERVO = re.compile(r"^move_servo_j\(\s*" + _JNT + r"\s*," + ",".join([_NUMBER] * 4) + r"\)\s*$")
_MOVE_J = re.compile(r"^move_j\(\s*" + _JNT + r"\s*," + ",".join([_NUMBER] * 2) + r"\)\s*$")
_PAYLOAD = re.compile(r"^set_payload_info\(" + ",".join([_NUMBER] * 4) + r"\)\s*$")
_PRINT = re.compile(r'^print\(\s*(\w+)\s*,\s*"([#\w]+)\[",\s*"\]"\s*\)\s*$')
_SPEED_BAR = re.compile(r"^set_speed_bar\(" + _NUMBER + r"\)\s*$")
VERSION = 26042604


class SimulatedController:
    def __init__(self, model: str = "RB5-850", *, time_scale: float = 1.0, simulation: bool = True, activated: bool = False, limits_deg: tuple[tuple[float, float], ...] | None = None,
                 activation_s: float = 0.5) -> None:
        self.model = model
        self.time_scale = time_scale
        self.allow_real = True
        self.limits_deg = limits_deg or ((-360.0, 360.0),) * 6
        self.activation_s = activation_s
        self.command_port = 0
        self.data_port = 0
        self._lock = threading.RLock()
        self._servers: list[socket.socket] = []
        self._connections: list[socket.socket] = []
        self._threads: list[threading.Thread] = []
        self._closing = threading.Event()
        # What the arm is doing.
        self.ref = [0.0] * 6
        self._servo_target: list[float] | None = None
        self._servo_until = 0.0
        self._move_goal: list[float] | None = None
        self._move_speed = 0.0
        self._moving = False
        self._last_tick = time.monotonic()
        self.simulation = simulation
        self.init_state = 6 if activated else 0
        self._activating_until = 0.0
        self.task_state = frames.TASK_IDLE
        self.speed_bar = 1.0
        self.payload: tuple[float, ...] | None = None
        # What a test injects.
        self.sos = 0
        self.ems = 0
        self.collision = 0
        self.joint_info = [0b110] * 6
        self.chunk1 = 0
        self.paused_flag = 0
        self.time_raw = 21950
        self.mode_override: int | None = None
        self.data_stalled = False
        self.data_extra_packet = False
        self.data_garbage = False
        self.ack_stalled = False
        self.ack_delay_s = 0.0
        self.close_data_after = 0
        #: A controller that takes a motion command, acknowledges it and does nothing: no motion, no `motion_changed`.
        self.freeze = False
        #: A controller that acknowledges a `task stop` and does not stop: the driver must not call the arm stopped.
        self.ignore_stop = False
        # What the driver did.
        self.commands: list[str] = []
        self.data_requests = 0
        self.servo_commands = 0

    # ── lifecycle ────────────────────────────────────────────────────────────

    def start(self) -> SimulatedController:
        self._closing.clear()
        self._last_tick = time.monotonic()
        for name in ("command", "data"):
            server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            server.bind(("127.0.0.1", 0))
            server.listen(8)
            server.settimeout(0.1)
            self._servers.append(server)
            setattr(self, f"{name}_port", server.getsockname()[1])
            self._spawn(self._accept, server, name)
        self._spawn(self._tick_loop)
        return self

    def stop(self) -> None:
        self._closing.set()
        with self._lock:
            sockets, self._connections = list(self._connections), []
        for sock in sockets + self._servers:
            try:
                sock.close()
            except OSError:
                pass
        self._servers = []
        for thread in self._threads:
            thread.join(timeout=2.0)
        self._threads = []

    def _spawn(self, target, *args) -> None:
        thread = threading.Thread(target=target, args=args, daemon=True, name="rainbow-sim")
        thread.start()
        self._threads.append(thread)

    def _accept(self, server: socket.socket, kind: str) -> None:
        while not self._closing.is_set():
            try:
                connection, _ = server.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            with self._lock:
                self._connections.append(connection)
            self._spawn(self._serve, self._serve_commands if kind == "command" else self._serve_data, connection)

    def _serve(self, handler, connection: socket.socket) -> None:
        """One connection's thread: whatever ends it closes the controller's end, as a controller that hangs up does, and forgets it."""
        try:
            handler(connection)
        finally:
            with self._lock:
                if connection in self._connections:
                    self._connections.remove(connection)
            try:
                connection.close()
            except OSError:
                pass

    def drop_connections(self, kind: str | None = None) -> None:
        """What a switch that restarts does to the controller's clients: every connection (or those of one port) is closed."""
        with self._lock:
            keep, drop = [], []
            for connection in self._connections:
                try:
                    port = connection.getsockname()[1]
                except OSError:
                    continue
                (drop if kind is None or port == (self.command_port if kind == "command" else self.data_port) else keep).append(connection)
            self._connections = keep
        for connection in drop:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            connection.close()

    # ── the command port ─────────────────────────────────────────────────────

    def _serve_commands(self, connection: socket.socket) -> None:
        buffer = b""
        connection.settimeout(0.1)
        while not self._closing.is_set():
            try:
                chunk = connection.recv(4096)
            except socket.timeout:
                continue
            except OSError:
                return
            if not chunk:
                return
            buffer += chunk
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                text = line.decode("utf-8", "replace").strip()
                if not text:
                    continue
                with self._lock:
                    self.commands.append(text)
                replies = self._handle(text)
                if not self.ack_stalled:
                    if self.ack_delay_s:
                        time.sleep(self.ack_delay_s)
                    self._send(connection, [frames.ACK] + replies)
                else:
                    self._send(connection, replies)

    def _send(self, connection: socket.socket, lines: list[str]) -> None:
        if not lines:
            return
        try:
            connection.sendall(("\n".join(lines) + "\n").encode())
        except OSError:
            pass

    def broadcast(self, line: str) -> None:
        """A line the controller says on its own (a motion that ended), to every client of the command port."""
        with self._lock:
            targets = [c for c in self._connections if self._is_command(c)]
        for connection in targets:
            self._send(connection, [line])

    def _is_command(self, connection: socket.socket) -> bool:
        try:
            return connection.getsockname()[1] == self.command_port
        except OSError:
            return False

    def _handle(self, text: str) -> list[str]:
        """What the controller says to a script, after its acknowledgement. Called under no lock: it takes the state's own."""
        with self._lock:
            if (match := _SERVO.match(text)):
                return self._servo(*(float(g) for g in match.groups()))
            if (match := _MOVE_J.match(text)):
                return self._move(*(float(g) for g in match.groups()))
            if text in ("task stop", "halt"):
                return [] if self.ignore_stop else self._halt()
            if text == "task pause":
                self.task_state = frames.TASK_PAUSED
                return ["info[system][pause]"]
            if text in ("task resume_a", "task resume_b"):
                self.task_state = frames.TASK_IDLE
                self.paused_flag = 0
                if text == "task resume_b":
                    self.collision = 0
                return []
            if text == "mc jall init":
                self._activating_until = time.monotonic() + self.activation_s / max(self.time_scale, 1e-9)
                self.init_state = 1
                return []
            if text.startswith("pgmode "):
                word = text.split(None, 1)[1].strip()
                if word == "simulation":
                    self.simulation = True
                    return []
                if word == "real" and self.allow_real:
                    self.simulation = False
                    return []
                if word == "real":
                    return ["warn[code][589]"]
                return self._syntax(text)
            if text == "get_control_box_info()":
                return [f"info[controlbox][{VERSION},0]"]
            if (match := _PRINT.match(text)):
                return [f"info[print_var][{match.group(2)}[{self._variable(match.group(1)):.3f}]]"]
            if (match := _SPEED_BAR.match(text)):
                self.speed_bar = float(match.group(1))
                return []
            if (match := _PAYLOAD.match(text)):
                self.payload = tuple(float(g) for g in match.groups())
                return []
            if text.startswith("set_payload_info("):
                return ["error[code][48]"]
            return self._syntax(text)

    @staticmethod
    def _syntax(text: str) -> list[str]:
        return [f"error[msg][Parsing Err[V:{VERSION}]", f"0: syntax error at ' ( ' in this line:", text, "]"]

    def _variable(self, name: str) -> float:
        table = {"SD_INIT_STATE_INFO": float(self.init_state), "SD_PG_MODE": 1.0 if self.simulation else 0.0, "SD_ROBOT_STATE": 3.0 if self._moving else 1.0, "SD_DEFAULT_SPEED": self.speed_bar}
        for index in range(6):
            table[f"SD_J{index}_ANG"] = self.ref[index]
            table[f"SD_J{index}_REF"] = self.ref[index]
        return table.get(name, 0.0)

    def _servo(self, *values: float) -> list[str]:
        angles, (t1, t2, gain, alpha) = list(values[:6]), values[6:]
        self.servo_commands += 1
        if t1 < frames.MIN_T1 or not frames.T2_RANGE[0] < t2 < frames.T2_RANGE[1]:
            return ["warn[code][568]"]
        for angle, (low, high) in zip(angles, self.limits_deg):
            if not low <= angle <= high:
                return ["warn[code][562]"]
        if self.freeze or not self._can_move():
            return []
        started = not self._moving
        self._servo_target = angles
        self._servo_until = time.monotonic() + t1 + t2
        self._move_goal = None
        self._moving = True
        return ["info[motion_changed][7100]"] if started else []

    def _move(self, *values: float) -> list[str]:
        angles, (speed, accel) = list(values[:6]), values[6:]
        if self.task_state == frames.TASK_PAUSED:
            self.task_state = frames.TASK_IDLE
            self._cancel()
            return ["warn[code][561]", "info[system][halt]", "info[motion_changed][0]"]
        for angle, (low, high) in zip(angles, self.limits_deg):
            if not low <= angle <= high:
                return ["warn[code][562]"]
        if self.freeze or not self._can_move():
            return []
        self._move_goal = angles
        self._move_speed = max(speed * self.speed_bar, 1e-6)
        self._servo_target = None
        self._moving = True
        return ["info[motion_changed][200]"]

    def _can_move(self) -> bool:
        """A real controller that was never activated has no arm to move; the simulation mode needs no activation (the Virtual Control Box moves with `init_state_info` at 0)."""
        return self.simulation or self.init_state == frames.ACTIVATED

    def _halt(self) -> list[str]:
        was = self._moving
        self._cancel()
        lines = ["info[system][halt]"]
        if was:
            lines.append("info[motion_changed][0]")
        if self.task_state == frames.TASK_PAUSED:
            self.task_state = frames.TASK_IDLE
        return lines

    def _cancel(self) -> None:
        self._servo_target = None
        self._move_goal = None
        self._moving = False

    # ── the arm ──────────────────────────────────────────────────────────────

    def _tick_loop(self) -> None:
        while not self._closing.wait(0.004):
            lines = self.step()
            for line in lines:
                self.broadcast(line)

    def step(self) -> list[str]:
        """Advance the arm to now. Returns the lines the controller says on its own (the end of a motion)."""
        out: list[str] = []
        with self._lock:
            now = time.monotonic()
            dt = (now - self._last_tick) * self.time_scale
            self._last_tick = now
            if self.init_state in range(1, 6) and now >= self._activating_until:
                self.init_state = 6
                out.append("info[mc][done]")
            if not self._moving:
                return out
            goal, speed = (self._move_goal, self._move_speed) if self._move_goal is not None else (self._servo_target, MAX_SPEED_DEG_S)
            if goal is None:
                self._moving = False
                return out
            budget = speed * dt
            arrived = True
            for index in range(6):
                delta = goal[index] - self.ref[index]
                step = max(-budget, min(budget, delta))
                self.ref[index] += step
                if abs(goal[index] - self.ref[index]) > 1e-9:
                    arrived = False
            if arrived and (self._move_goal is not None or now >= self._servo_until):
                self._cancel()
                out.append("info[motion_changed][0]")
        return out

    @property
    def joints_deg(self) -> list[float]:
        with self._lock:
            return list(self.ref)

    @property
    def robot_state(self) -> int:
        with self._lock:
            return 3 if self._moving else 1

    # ── the data port ────────────────────────────────────────────────────────

    def packet(self) -> bytes:
        with self._lock:
            mode = self.mode_override if self.mode_override is not None else (frames.MODE_SIMULATION if self.simulation else frames.MODE_REAL)
            chunk1 = self.chunk1 | ((1 << 6) if (not self.simulation and self.init_state == frames.ACTIVATED) else 0)
            return frames.encode_state(ref=tuple(self.ref), task_state=self.task_state, robot_state=3 if self._moving else 1, speed_bar=self.speed_bar, chunk1=chunk1, joint_info=tuple(self.joint_info), mode=mode,
                                       init_state=self.init_state, collision=self.collision, sos=self.sos, ems=self.ems, paused_flag=self.paused_flag, time_raw=self.time_raw)

    def _serve_data(self, connection: socket.socket) -> None:
        buffer = b""
        connection.settimeout(0.1)
        served = 0
        while not self._closing.is_set():
            try:
                chunk = connection.recv(4096)
            except socket.timeout:
                continue
            except OSError:
                return
            if not chunk:
                return
            buffer += chunk
            while b"\n" in buffer:
                line, buffer = buffer.split(b"\n", 1)
                if line.strip() != b"reqdata":
                    continue
                with self._lock:
                    self.data_requests += 1
                if self.data_stalled:
                    continue
                payload = b""
                if self.data_garbage:
                    payload += b"\x00\x01garbage"
                if self.data_extra_packet:
                    other = bytearray(frames.encode_state())
                    other[3] = 9
                    payload += bytes(other)
                try:
                    connection.sendall(payload + self.packet())
                except OSError:
                    return
                served += 1
                if self.close_data_after and served >= self.close_data_after:
                    connection.close()
                    return

    # ── what a test injects ──────────────────────────────────────────────────

    def raise_error(self, code: int) -> None:
        self.broadcast(f"error[code][{code}]")

    def raise_warn(self, code: int) -> None:
        self.broadcast(f"warn[code][{code}]")

    def say(self, line: str) -> None:
        self.broadcast(line)

    def seen(self, prefix: str) -> list[str]:
        with self._lock:
            return [c for c in self.commands if c.startswith(prefix)]

    def set_mode_state(self, *, simulation: bool | None = None, activated: bool | None = None) -> None:
        with self._lock:
            if simulation is not None:
                self.simulation = simulation
            if activated is not None:
                self.init_state = frames.ACTIVATED if activated else 0
