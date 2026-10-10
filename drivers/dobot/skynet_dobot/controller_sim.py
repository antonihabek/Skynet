"""A simulated Dobot V4 controller, the dry substrate: the dashboard's text protocol and the 1440-byte real-time feedback on two TCP ports, a six-joint arm that moves at a speed (DRY-COVERAGE-PLAN.md item 12.E.13).

Written here from Dobot's TCP/IP interface document V4.6.6 and from reading what the maker's own Python client (`Dobot-Arm/TCP-IP-Python-V4`) sends, so **it agrees with whoever wrote the driver** (F15): no Dobot
software or hardware is on the other end. The maker's own client runs against it unmodified (`layout_check.py`), which is what makes the bytes independent evidence. What it adds is the part a driver has to survive:
the controller states and what each refuses (the document's section 5), an alarm that holds until it is cleared, a software emergency stop that has to be released and then cleared, a collision, a controller that is
not in TCP mode, and a lever for every way a link can go wrong.

    arm = SimulatedController("CR5").start()        # arm.dashboard_port, arm.feedback_port

What it does **not** model, because nothing here says: the arm's dynamics (a joint here moves toward its target at a fixed speed), its inverse kinematics (a pose target is refused), its coordinate systems, the
queue's blending, the controller's payload and collision detection, what the real controller answers to a command it is in no state to take (the document lists the codes, not which state gets which), or the text
it sends a client that is not in TCP mode (the maker's client matches `Control Mode Is Not Tcp`, which is what is sent here).
"""

from __future__ import annotations

import math
import select
import socket
import threading
import time

from . import frames

TICK_S = 0.004
FEEDBACK_PERIOD_S = 0.008
#: The simulator's own figure (nothing the document says): how fast a joint crosses its range, in degrees a second, at a speed factor and a speed ratio of 100.
MAX_SPEED_DEG_S = 120.0
#: The simulator's own joint ranges (degrees), wide of what a CR5's manual gives; a driver's `limits` are the operator's.
JOINT_LIMITS_DEG = (-360.0, 360.0)
#: How long a controller takes to come up after `PowerOn()`: the document says "about 10 seconds".
POWER_ON_S = 10.0
#: The alarm identifiers this simulator raises, from the maker's own alarm table (`demo/files/alarmController.py`): 132 "The software emergency stop signal is triggered", 17 "Inverse kinematics error with no solution".
ALARM_SOFTWARE_ESTOP = 132
ALARM_NO_SOLUTION = 17


def _write_all(connection: socket.socket, data: bytes) -> None:
    try:
        connection.sendall(data)
    except OSError:
        pass


class SimulatedController:
    def __init__(self, model: str = "CR5", *, time_scale: float = 1.0, start_deg: tuple[float, ...] = (0.0,) * 6, powered: bool = True, enabled: bool = False, tcp_mode: bool = True) -> None:
        by_name = {name: number for number, name in frames.ROBOT_TYPES.items()}
        if model not in by_name:
            raise ValueError(f"{model!r} is not a Dobot model in the interface document's table.")
        self.model = model
        self.robot_type = by_name[model]
        self.time_scale = time_scale
        self.dashboard_port = 0
        self.feedback_port = 0
        self._lock = threading.RLock()
        self._started = time.monotonic()
        # the arm
        self._q = [float(v) for v in start_deg]
        self._qd = [0.0] * 6
        self._target: list[float] | None = None            # a ServoJ target
        self._target_t = 0.1
        self._target_set = 0.0
        self._queue: list[tuple[list[float], float, int]] = []     # (target, degrees a second, result id)
        self._moving_id = 0
        self._result_id = 0
        self._done_id = 0
        self._speed_factor = 100
        self._paused = False
        # the controller
        self._powered = powered
        self._booting_until = 0.0
        self._enabled = enabled and powered
        self._tcp = tcp_mode
        self._alarms: list[int] = []
        self._persistent: set[int] = set()
        self._estop = False
        self._collision = False
        self._load = 0.0
        # What a test turns on.
        self.drop = False                   # dashboard commands are read and not answered: a controller that has gone quiet
        self.delay_s = 0.0                  # every dashboard reply waits
        self.garble = False                 # every dashboard reply is damaged (its closing semicolon is gone)
        self.error_next: list[int] = []     # the next replies are these ErrorIDs, whatever the command was
        self.silent_feedback = False        # nothing is sent on the feedback port
        self.feedback_gap_s = 0.0           # an extra wait before every feedback packet
        self.feedback_split = 0             # a packet is sent in two pieces, cut after this many bytes
        self.feedback_truncate = 0          # every packet is this many bytes short
        self.feedback_garble = False        # every packet's self-test field is wrong
        self.frozen = False                 # the feedback shows the arm where it was when this was set
        self.feedback_q_scale = 1.0         # the feedback's joint angles are the degrees times this: a controller whose feedback is in another unit than GetAngle() answers in
        self.refuse_connections = False
        self.force_mode: int | None = None  # the controller reports this RobotMode whatever its state is: a value the simulator would not otherwise reach (brake open, drag, a value the document does not list)
        self.requests: list[str] = []       # every dashboard line received, as received
        self.servo_calls: list[tuple[float, list[float], float, float, float]] = []     # (time, degrees, t, aheadtime, gain)
        self._frozen_q: list[float] | None = None
        self._stopping = False
        self._listeners: list[socket.socket | None] = [None, None]
        self._open: list[socket.socket] = []
        self._threads: list[threading.Thread] = []
        self._feedback_clients: list[socket.socket] = []

    # ── what a test looks at and does to the arm ─────────────────────────────────────────────────

    @property
    def joints_deg(self) -> list[float]:
        with self._lock:
            return list(self._q)

    @property
    def mode(self) -> int:
        with self._lock:
            return self._mode()

    @property
    def alarms(self) -> list[int]:
        with self._lock:
            return list(self._alarms)

    @property
    def enabled(self) -> bool:
        with self._lock:
            return self._enabled

    @property
    def tcp_mode(self) -> bool:
        with self._lock:
            return self._tcp

    @property
    def estop_pressed(self) -> bool:
        with self._lock:
            return self._estop

    @property
    def speed_factor(self) -> int:
        with self._lock:
            return self._speed_factor

    @property
    def moving(self) -> bool:
        with self._lock:
            return self._is_moving()

    @property
    def load_kg(self) -> float:
        with self._lock:
            return self._load

    def _halt(self) -> None:
        """Whatever the arm was moving to is dropped: the target, the queue and the move that was under way."""
        self._target = None
        self._queue.clear()
        self._moving_id = 0

    def place(self, joints_deg: tuple[float, ...] | list[float]) -> None:
        """The arm is here and not moving: an arm as a person left it."""
        with self._lock:
            self._q = [float(v) for v in joints_deg]
            self._qd = [0.0] * 6
            self._halt()

    def set_mode_state(self, *, powered: bool | None = None, enabled: bool | None = None, tcp: bool | None = None) -> None:
        with self._lock:
            if powered is not None:
                self._powered = powered
                if not powered:
                    self._enabled = False
            if enabled is not None:
                self._enabled = enabled and self._powered
            if tcp is not None:
                self._tcp = tcp

    def raise_alarm(self, alarm_id: int, persistent: bool = False) -> None:
        """An alarm: the controller is in its alarm state, it disables the arm, and it stays so until the alarm is cleared (a persistent one comes back)."""
        with self._lock:
            if alarm_id not in self._alarms:
                self._alarms.append(alarm_id)
            if persistent:
                self._persistent.add(alarm_id)
            self._enabled = False
            self._halt()

    def collide(self) -> None:
        with self._lock:
            self._collision = True
            self._halt()

    def freeze(self, on: bool = True) -> None:
        with self._lock:
            self.frozen = on
            self._frozen_q = list(self._q) if on else None

    # ── lifecycle ────────────────────────────────────────────────────────────────────────────────

    def start(self) -> "SimulatedController":
        self._stopping = False
        for index in range(2):
            listener = self._listen(0)
            self._listeners[index] = listener
        self.dashboard_port = self._listeners[0].getsockname()[1]      # type: ignore[union-attr]
        self.feedback_port = self._listeners[1].getsockname()[1]       # type: ignore[union-attr]
        for name, target in (("dashboard", self._accept_dashboard), ("feedback", self._accept_feedback), ("physics", self._physics)):
            thread = threading.Thread(target=target, daemon=True, name=f"dobot-sim-{name}")
            thread.start()
            self._threads.append(thread)
        return self

    @staticmethod
    def _listen(port: int) -> socket.socket:
        listener = socket.socket()
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        deadline = time.monotonic() + 2.0
        while True:
            try:
                listener.bind(("127.0.0.1", port))
                break
            except OSError:
                # The port of a listener that was just shut is held until the thread accepting on it has let go.
                if not port or time.monotonic() >= deadline:
                    raise
                time.sleep(0.02)
        listener.listen(8)
        listener.settimeout(0.1)
        return listener

    @staticmethod
    def _shut(listener: socket.socket) -> None:
        """Close a listening socket now: a thread blocked in `accept` on it would otherwise keep it listening until its timeout."""
        try:
            listener.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        try:
            listener.close()
        except OSError:
            pass

    def stop(self) -> None:
        self._stopping = True
        for listener in self._listeners:
            if listener is not None:
                self._shut(listener)
        self._listeners = [None, None]
        self.hang_up()
        for thread in self._threads:
            thread.join(timeout=1.0)
        self._threads = []

    def stop_listening(self) -> None:
        """Both ports are closed: a driver that tries to reach the controller is refused, as it is when the controller is off."""
        for index in range(2):
            listener, self._listeners[index] = self._listeners[index], None
            if listener is not None:
                self._shut(listener)

    def resume_listening(self) -> None:
        for index, port in enumerate((self.dashboard_port, self.feedback_port)):
            if self._listeners[index] is None:
                self._listeners[index] = self._listen(port)

    def hang_up(self) -> None:
        """The controller drops every connection it has (it restarted, or its link did): the next one is accepted as usual."""
        with self._lock:
            connections, self._open = self._open, []
            self._feedback_clients = []
        for connection in connections:
            try:
                connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            try:
                connection.close()
            except OSError:
                pass

    def heal(self) -> None:
        with self._lock:
            self.drop = self.garble = self.silent_feedback = self.feedback_garble = self.refuse_connections = False
            self.delay_s = self.feedback_gap_s = 0.0
            self.feedback_split = self.feedback_truncate = 0
            self.error_next = []
            self.force_mode = None
            self.frozen = False
            self._frozen_q = None

    # ── state ────────────────────────────────────────────────────────────────────────────────────

    def _is_moving(self) -> bool:
        return bool(self._queue) or self._moving_id != 0 or (self._target is not None and any(abs(self._target[i] - self._q[i]) > 1e-3 for i in range(6))) or any(abs(v) > 1e-3 for v in self._qd)

    def _mode(self) -> int:
        if self.force_mode is not None:
            return self.force_mode
        if self._alarms or self._estop:
            return frames.MODE_ERROR
        if self._collision:
            return frames.MODE_COLLISION
        if not self._powered:
            return frames.MODE_POWEROFF
        if time.monotonic() < self._booting_until:
            return frames.MODE_INIT
        if not self._enabled:
            return frames.MODE_DISABLED
        if self._paused:
            return frames.MODE_PAUSE
        if self._is_moving():
            return frames.MODE_RUNNING
        return frames.MODE_ENABLE

    def _packet(self) -> bytes:
        with self._lock:
            q = tuple(v * self.feedback_q_scale for v in (self._frozen_q if self._frozen_q is not None else self._q))
            mode = self._mode()
            forced = self.force_mode is not None
            enabled = mode in (frames.MODE_ENABLE, frames.MODE_RUNNING, frames.MODE_SINGLE_MOVE) if forced else self._enabled
            target = tuple(v * self.feedback_q_scale for v in (self._target if self._target is not None else self._q))
            return frames.encode_feedback(
                robot_mode=mode, enable_status=1 if enabled else 0, brake_status=0x3F if enabled else 0, error_status=1 if (self._alarms or self._estop or (forced and mode == frames.MODE_ERROR)) else 0,
                running_status=1 if (mode in (frames.MODE_RUNNING, frames.MODE_SINGLE_MOVE) if forced else self._is_moving()) else 0, collision_state=1 if (self._collision or (forced and mode == frames.MODE_COLLISION)) else 0, robot_type=self.robot_type, safety_state=0xFF & ~(1 if self._estop else 0), speed_scaling=float(self._speed_factor),
                q_actual=q, q_target=target, qd_actual=tuple(self._qd), timestamp_ms=int(time.time() * 1000), run_time_ms=int((time.monotonic() - self._started) * 1000), current_command_id=self._done_id, load_kg=self._load)

    # ── the physics ──────────────────────────────────────────────────────────────────────────────

    def _physics(self) -> None:
        last = time.monotonic()
        while not self._stopping:
            time.sleep(TICK_S)
            now = time.monotonic()
            dt = (now - last) * self.time_scale
            last = now
            with self._lock:
                self._step(dt, now)

    def _step(self, dt: float, now: float) -> None:
        if not self._enabled or self._paused or self._alarms or self._estop or self._collision:
            self._qd = [0.0] * 6
            return
        goal: list[float] | None = None
        speed = MAX_SPEED_DEG_S
        if self._queue:
            goal, speed, self._moving_id = self._queue[0][0], self._queue[0][1], self._queue[0][2]
        elif self._target is not None:
            goal = self._target
            remaining = max(self._target_t - (now - self._target_set) * self.time_scale, dt, 1e-3)
            far = max(abs(goal[i] - self._q[i]) for i in range(6))
            speed = min(MAX_SPEED_DEG_S, far / remaining)
        if goal is None:
            self._qd = [0.0] * 6
            return
        distance = max(abs(goal[i] - self._q[i]) for i in range(6))
        if distance < 1e-6:
            if self._queue:
                self._done_id = self._queue.pop(0)[2]
                self._moving_id = 0
            self._qd = [0.0] * 6
            return
        step = min(distance, max(speed, 1e-3) * dt)
        ratio = step / distance
        previous = list(self._q)
        for i in range(6):
            self._q[i] += (goal[i] - self._q[i]) * ratio
        self._qd = [(self._q[i] - previous[i]) / max(dt, 1e-9) for i in range(6)]

    # ── the dashboard ────────────────────────────────────────────────────────────────────────────

    def _accept_dashboard(self) -> None:
        while not self._stopping:
            listener = self._listeners[0]
            if listener is None:
                time.sleep(0.05)
                continue
            try:
                connection, _ = listener.accept()
            except (socket.timeout, OSError):
                continue
            if self.refuse_connections:
                connection.close()
                continue
            with self._lock:
                self._open.append(connection)
            threading.Thread(target=self._serve_dashboard, args=(connection,), daemon=True, name="dobot-sim-dashboard-client").start()

    def _forget(self, connection: socket.socket) -> None:
        """A client that has gone: its end of the connection is closed here too, and no longer kept."""
        with self._lock:
            if connection in self._open:
                self._open.remove(connection)
            if connection in self._feedback_clients:
                self._feedback_clients.remove(connection)
        try:
            connection.close()
        except OSError:
            pass

    def _serve_dashboard(self, connection: socket.socket) -> None:
        try:
            self._dashboard_session(connection)
        finally:
            self._forget(connection)

    def _dashboard_session(self, connection: socket.socket) -> None:
        connection.setblocking(True)
        pending = b""
        while not self._stopping:
            try:
                ready = select.select([connection], [], [], 0.05)[0]
            except (OSError, ValueError):
                return
            if not ready:
                continue
            try:
                data = connection.recv(4096)
            except OSError:
                return
            if not data:
                return
            pending += data
            while b"\n" in pending:
                line, pending = pending.split(b"\n", 1)
                text = line.decode("utf-8", "replace").strip()
                if not text:
                    continue
                reply = self._command(text)
                if reply is None:
                    continue
                if self.delay_s:
                    time.sleep(self.delay_s)
                _write_all(connection, reply.encode() + b"\n")

    def _command(self, line: str) -> str | None:
        with self._lock:
            self.requests.append(line)
            if self.drop:
                return None
            if self.error_next:
                code = self.error_next.pop(0)
                return self._finish(f"{code},{{}},{line};")
            reply = self._execute(line)
        return reply

    def _finish(self, reply: str) -> str:
        return reply[:-1] if self.garble and reply.endswith(";") else reply

    def _execute(self, line: str) -> str:
        open_paren = line.find("(")
        if open_paren < 1 or not line.endswith(")"):
            return self._finish(f"-10000,{{}},{line};")
        name = line[:open_paren].strip().lower()
        text = line[open_paren + 1:-1]
        args = _split(text)
        if name == "requestcontrol":
            return self._finish(self._request_control(line))
        if not self._tcp:
            return frames.NOT_TCP
        handler = getattr(self, f"_cmd_{name}", None)
        if handler is None:
            return self._finish(f"-10000,{{}},{line};")
        mode = self._mode()
        if mode == frames.MODE_ERROR and name not in ("clearerror", "geterrorid", "emergencystop", "stop", "robotmode"):
            return self._finish(f"{-3 if self._estop else -2},{{}},{line};")
        if mode == frames.MODE_POWEROFF and name not in ("clearerror", "geterrorid", "emergencystop", "stop", "robotmode", "poweron"):
            return self._finish(f"-4,{{}},{line};")
        code, payload = handler(args)
        return self._finish(f"{code},{{{payload}}},{line};")

    def _request_control(self, line: str) -> str:
        mode = self._mode()
        if self._tcp:
            return f"0,{{}},{line};"
        if mode in (frames.MODE_POWEROFF, frames.MODE_DISABLED, frames.MODE_ERROR) and not self._enabled:
            self._tcp = True
            return f"0,{{}},{line};"
        return f"-1,{{}},{line};"

    # each handler returns (ErrorID, the text between the braces)

    def _cmd_poweron(self, args: list[str]) -> tuple[int, str]:
        if args:
            return -20000, ""
        if not self._powered:
            self._powered = True
            self._booting_until = time.monotonic() + POWER_ON_S / max(self.time_scale, 1e-9)
        return 0, ""

    def _cmd_enablerobot(self, args: list[str]) -> tuple[int, str]:
        if len(args) not in (0, 1, 4, 5):
            return -20000, ""
        values: list[float] = []
        for position, arg in enumerate(args, 1):
            try:
                values.append(float(arg))
            except ValueError:
                return -50000 - position, ""
        if values and not 0.0 <= values[0] <= 100.0:
            return -60001, ""
        if time.monotonic() < self._booting_until:
            return -1, ""
        self._enabled = True
        if values:
            self._load = values[0]
        return 0, ""

    def _cmd_disablerobot(self, args: list[str]) -> tuple[int, str]:
        if args:
            return -20000, ""
        self._enabled = False
        self._halt()
        return 0, ""

    def _cmd_clearerror(self, args: list[str]) -> tuple[int, str]:
        if args:
            return -20000, ""
        if self._estop:
            return -1, ""
        self._alarms = [alarm for alarm in self._alarms if alarm in self._persistent]
        self._collision = False
        return 0, ""

    def _cmd_stop(self, args: list[str]) -> tuple[int, str]:
        if args:
            return -20000, ""
        self._halt()
        self._paused = False
        return 0, ""

    def _cmd_pause(self, args: list[str]) -> tuple[int, str]:
        self._paused = True
        return 0, ""

    def _cmd_continue(self, args: list[str]) -> tuple[int, str]:
        self._paused = False
        return 0, ""

    def _cmd_emergencystop(self, args: list[str]) -> tuple[int, str]:
        if len(args) != 1:
            return -20000, ""
        try:
            engage = int(args[0])
        except ValueError:
            return -30001, ""
        if engage not in (0, 1):
            return -40001, ""
        if engage:
            self._estop = True
            self._enabled = False
            self._halt()
            if ALARM_SOFTWARE_ESTOP not in self._alarms:
                self._alarms.append(ALARM_SOFTWARE_ESTOP)
        else:
            self._estop = False
        return 0, ""

    def _cmd_robotmode(self, args: list[str]) -> tuple[int, str]:
        if args:
            return -20000, ""
        return 0, str(self._mode())

    def _cmd_getangle(self, args: list[str]) -> tuple[int, str]:
        if args:
            return -20000, ""
        return 0, ",".join(f"{v:.6f}" for v in self._q)

    def _cmd_geterrorid(self, args: list[str]) -> tuple[int, str]:
        if args:
            return -20000, ""
        return 0, "[" + ",".join(str(alarm) for alarm in self._alarms) + "]"

    def _cmd_speedfactor(self, args: list[str]) -> tuple[int, str]:
        if len(args) != 1:
            return -20000, ""
        try:
            value = int(args[0])
        except ValueError:
            return -30001, ""
        if not 1 <= value <= 100:
            return -40001, ""
        self._speed_factor = value
        return 0, ""

    def _gate_motion(self) -> int | None:
        mode = self._mode()
        if mode == frames.MODE_COLLISION:
            return -1
        if not self._enabled:
            return -1
        return None

    def _cmd_movj(self, args: list[str]) -> tuple[int, str]:
        if not args:
            return -20000, ""
        point = args[0].replace(" ", "")
        if not point.lower().startswith("joint={") or not point.endswith("}"):
            return (-1, "") if point.lower().startswith("pose=") else (-30001, "")
        try:
            joints = [float(v) for v in point[7:-1].split(",")]
        except ValueError:
            return -30001, ""
        if len(joints) != 6:
            return -30001, ""
        if any(not JOINT_LIMITS_DEG[0] <= v <= JOINT_LIMITS_DEG[1] for v in joints):
            return -40001, ""
        a_value = v_value = 100
        for arg in args[1:]:
            key, _, value = arg.replace(" ", "").partition("=")
            if key not in ("a", "v", "cp", "user", "tool"):
                return -50001, ""
            try:
                number = int(value)
            except ValueError:
                return -50001, ""
            if key in ("a", "v") and not 1 <= number <= 100:
                return -60001, ""
            if key == "a":
                a_value = number
            if key == "v":
                v_value = number
        refused = self._gate_motion()
        if refused is not None:
            return refused, ""
        self._result_id += 1
        self._target = None
        self._queue.append((joints, MAX_SPEED_DEG_S * v_value / 100.0 * self._speed_factor / 100.0, self._result_id))
        return 0, str(self._result_id)

    def _cmd_servoj(self, args: list[str]) -> tuple[int, str]:
        if len(args) < 6 or len(args) > 9:
            return -20000, ""
        joints: list[float] = []
        for position, arg in enumerate(args[:6], 1):
            try:
                joints.append(float(arg))
            except ValueError:
                return -30000 - position, ""
        for position, value in enumerate(joints, 1):
            if not math.isfinite(value) or not JOINT_LIMITS_DEG[0] <= value <= JOINT_LIMITS_DEG[1]:
                return -40000 - position, ""
        options = {"t": frames.SERVO_T, "aheadtime": frames.SERVO_AHEAD, "gain": frames.SERVO_GAIN}
        given = {name: default for name, (_, _, default) in options.items()}
        for arg in args[6:]:
            key, sep, value = arg.replace(" ", "").partition("=")
            if not sep or key not in options:
                return -50001, ""
            try:
                number = float(value)
            except ValueError:
                return -50001, ""
            low, high, _ = options[key]
            if not low <= number <= high:
                return -60001, ""
            given[key] = number
        refused = self._gate_motion()
        if refused is not None:
            return refused, ""
        self._result_id += 1
        self._queue.clear()
        self._moving_id = 0
        self._target = joints
        self._target_t = given["t"]
        self._target_set = time.monotonic()
        self._done_id = self._result_id
        self.servo_calls.append((time.monotonic(), joints, given["t"], given["aheadtime"], given["gain"]))
        return 0, str(self._result_id)

    # ── the feedback ─────────────────────────────────────────────────────────────────────────────

    def _accept_feedback(self) -> None:
        while not self._stopping:
            listener = self._listeners[1]
            if listener is None:
                time.sleep(0.05)
                continue
            try:
                connection, _ = listener.accept()
            except (socket.timeout, OSError):
                continue
            if self.refuse_connections:
                connection.close()
                continue
            with self._lock:
                self._open.append(connection)
                self._feedback_clients.append(connection)
            threading.Thread(target=self._serve_feedback, args=(connection,), daemon=True, name="dobot-sim-feedback-client").start()

    def _serve_feedback(self, connection: socket.socket) -> None:
        try:
            self._feedback_session(connection)
        finally:
            self._forget(connection)

    def _feedback_session(self, connection: socket.socket) -> None:
        connection.setblocking(True)
        while not self._stopping:
            time.sleep(FEEDBACK_PERIOD_S)
            with self._lock:
                if connection not in self._feedback_clients:
                    return
                silent, gap, split, cut, garble = self.silent_feedback, self.feedback_gap_s, self.feedback_split, self.feedback_truncate, self.feedback_garble
            if silent:
                continue
            if gap:
                time.sleep(gap)
            packet = self._packet()
            if garble:
                packet = packet[:48] + b"\x00" * 8 + packet[56:]
            if cut:
                packet = packet[:max(0, len(packet) - cut)]
            try:
                if split and len(packet) > split:
                    connection.sendall(packet[:split])
                    time.sleep(0.01)
                    connection.sendall(packet[split:])
                else:
                    connection.sendall(packet)
            except OSError:
                return

    def attach(self, connection: socket.socket) -> None:
        """Serve the dashboard on a connection that already exists (the far end of a relay)."""
        with self._lock:
            self._open.append(connection)
        threading.Thread(target=self._serve_dashboard, args=(connection,), daemon=True, name="dobot-sim-attached").start()


def _split(text: str) -> list[str]:
    """A command's parameters: split at the commas that are not inside braces."""
    parts: list[str] = []
    depth, current = 0, ""
    for char in text:
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
        if char == "," and depth == 0:
            parts.append(current.strip())
            current = ""
        else:
            current += char
    if current.strip() or parts:
        parts.append(current.strip())
    return parts
