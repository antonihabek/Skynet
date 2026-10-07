"""A simulated OnRobot RG2, RG6, 2FG7 or 2FG14, the dry substrate: a Modbus slave (TCP on a port, RTU on a pseudo-terminal) behind the register map `frames.py` documents.

Written here from what the maker's own library (`libonrobot_tool_api`) was seen to do, so **it agrees with whoever wrote the driver** (F15): no OnRobot software or hardware is on the other end. What it adds is the
part a driver has to survive: fingers that cross their range at a speed, an object that stops a closing finger and sets the grip flag, the statuses and errors the registers carry, a supply-power budget that a power
cycle gives back, and levers for the line (`drop`, `delay_s`, `garble`, `silent`, `truncate`, `split_gap_s`, `skip_requests`, `exception_next`, `wrong_unit`, `stale_state_s`).

    gripper = SimulatedGripper("RG2").start()       # gripper.port is the Modbus TCP port, gripper.path the RTU device

What it does **not** model, because nothing here says: the real finger dynamics, the force a closing finger develops (a closing finger stops at an object's width at once), how a gripper answers a command while its
safety input is open (it takes it), or what the RG's undocumented state registers hold (they read zero).
"""

from __future__ import annotations

import os
import select
import socket
import struct
import threading
import time

from . import frames, modbus

MEMORY = 2048
TICK_S = 0.01
#: The simulator's own figures (the maker publishes none this repository could read): how fast a finger crosses its range.
RG_SPEED_MM_S = {"RG2": 100.0, "RG6": 150.0}
FG_FULL_SPEED_MM_S = {"2FG7": 150.0, "2FG14": 200.0}      # at a speed of 100 %; a speed of 1 % is a hundredth of it
#: Its own figures for what a 2FG reports about itself: external and internal width limits (mm) and the power-dependent conventional force ceiling.
FG_LIMITS_MM = {"2FG7": (10.0, 74.0, 0.0, 60.0), "2FG14": (15.0, 150.0, 0.0, 135.0)}
FG_MAX_FORCE_N = {"2FG7": 140, "2FG14": 220}
FG_REALTIME_FORCE_N = {"2FG7": 95, "2FG14": 196}


def _write_fd(fd: int, data: bytes) -> None:
    try:
        os.write(fd, data)
    except OSError:
        pass


def _send_all(connection: socket.socket, data: bytes) -> None:
    try:
        connection.sendall(data)
    except OSError:
        pass


class SimulatedGripper:
    def __init__(self, model: str = "RG2", *, slave: int = 65, time_scale: float = 1.0, start_width_mm: float | None = None, firmware: tuple[int, int, int] = (1, 1, 33)) -> None:
        self.model = frames.MODELS[model]
        self.slave = slave
        self.time_scale = time_scale
        self.path = ""
        self.port = 0
        self._master = -1
        self._slave_fd = -1
        self._lock = threading.RLock()
        self._memory = [0] * MEMORY
        self._fg = self.model.family == "fg"
        if self._fg:
            self._min_mm, self._max_mm = FG_LIMITS_MM[model][0], FG_LIMITS_MM[model][1]
        else:
            self._min_mm, self._max_mm = 0.0, float(self.model.max_width_mm or 0.0)
        self._width = float(start_width_mm if start_width_mm is not None else self._max_mm)
        self._target = self._width
        self._speed_mm_s = 0.0
        self._force_limit_n = 0.0
        self._moving = False
        self._grip = False
        self._error_code = 0
        self._flags = 0                         # extra status bits a test sets (the safety bits of an RG, not-calibrated and the sensor error of a 2FG)
        self._temperature = 31.0
        self._supply_w = frames.FG_POWER_MIN_W
        self._last_tick = time.monotonic()
        #: An object between the fingers, as the width a closing finger stops at; None for nothing there.
        self.object_mm: float | None = None
        self.firmware = firmware
        self._stopping = False
        self._physics_thread: threading.Thread | None = None
        self._thread: threading.Thread | None = None
        self._listener: socket.socket | None = None
        self._tcp_thread: threading.Thread | None = None
        self._open_connections: list[socket.socket] = []
        self._attached: list[int] = []
        self._history: list[tuple[float, list[int]]] = []
        # What a test turns on.
        self.drop = False                       # requests are read and not answered: a gripper that has gone
        self.delay_s = 0.0                      # every answer waits
        self.garble = False                     # every answer's CRC (RTU) or function (TCP) is wrong
        self.silent = False                     # the line is dead: nothing is read, nothing answered
        self.split_gap_s = 0.0                  # an answer comes in two pieces with this long between them
        self.truncate = 0                       # every answer is this many bytes short
        self.skip_requests = 0                  # the next this-many requests are read and not answered: a lost frame
        self.exception_next = 0                 # the next this-many requests are answered with a Modbus exception
        self.exception_code = 2
        self.wrong_unit: int | None = None      # every answer says it is from this unit
        self.late_answer_s = 0.0                # TCP: the answer to a request comes after this long, and a stale one is sent first
        self.stale_state_s = 0.0                # reads of the state block show the gripper as it was this long ago
        self.frozen = False                     # the state block is not republished from the physics: what a test put into it stays
        self.refuse_connections = False         # a TCP connection is accepted and closed at once: a Compute Box that is up and will not talk
        self.connections = 0                    # TCP connections accepted so far
        self.mute_reads = False                 # reads are not answered and writes are: a gripper that takes what it is told and says nothing
        self.deaf = False                       # a write to the control block is acknowledged and not taken
        self.forget_power = False               # 2FG: a write to the supply-power register is acknowledged and not kept
        self.requests: list[tuple[int, int, int]] = []         # (function, address, count) of each request answered
        self.writes: list[tuple[float, int, tuple[int, ...]]] = []    # (time, address, values) of each write that was acknowledged
        self.control_writes: list[tuple[float, tuple[int, ...]]] = []
        self._reset_registers()

    # ── the registers ────────────────────────────────────────────────────────────

    def _reset_registers(self) -> None:
        self._memory[frames.PRODUCT_CODE_ADDRESS] = self.model.product_code
        self._memory[frames.FIRMWARE_ADDRESS] = self.firmware[0] << 8 | self.firmware[1]
        self._memory[frames.FIRMWARE_ADDRESS + 1] = self.firmware[2]
        if self._fg:
            limits = FG_LIMITS_MM[self.model.name]
            for i, mm in enumerate(limits):
                self._memory[frames.FG_LIMITS_ADDRESS + i] = int(round(mm * 10))
            self._memory[frames.FG_POWER_ADDRESS] = self._supply_w
            self._memory[frames.FG_POWER_ADDRESS + 1] = self._force_ceiling()
            self._memory[frames.FG_POWER_ADDRESS + 3] = FG_REALTIME_FORCE_N[self.model.name]
        else:
            self._memory[frames.RG_CONFIG_ADDRESS] = 400           # a long-term power limit of 40 W (the simulator's own figure)
        self._publish()

    def _force_ceiling(self) -> int:
        """The conventional force ceiling a supply budget allows: the simulator's own proportion (14 W is a tenth of the 140 N, 48 W the whole), because the real mapping is not in anything read here."""
        top = FG_MAX_FORCE_N[self.model.name]
        return int(round(top * (self._supply_w - 4) / 44)) if self._supply_w < frames.FG_POWER_MAX_W else top

    def _status_word(self) -> int:
        if self._fg:
            word = self._flags & (frames.FG_NOT_CALIBRATED | frames.FG_LINEAR_SENSOR_ERROR)
            return word | (frames.FG_BUSY if self._moving else 0) | (frames.FG_GRIP if self._grip else 0)
        word = self._flags & (frames.RG_S1_PUSHED | frames.RG_S1_TRIGGERED | frames.RG_S2_PUSHED | frames.RG_S2_TRIGGERED | frames.RG_DC_ERROR)
        return word | (frames.RG_BUSY if self._moving else 0) | (frames.RG_GRIP if self._grip else 0) | (self._error_code & frames.RG_ERROR_MASK) << frames.RG_ERROR_SHIFT

    def _publish(self) -> None:
        if self.frozen:
            return
        width = self._width
        if self._fg:
            base = frames.FG_STATUS_ADDRESS
            block = [0] * frames.FG_STATUS_COUNT
            block[0] = self._status_word()
            block[1] = int(round(width * 10)) & 0xFFFF
            block[2] = int(round((self._max_mm - width) * 10)) & 0xFFFF
            # 259..262 are the device's own limits, inside the state block the maker's client reads whole.
            for i, mm in enumerate(FG_LIMITS_MM[self.model.name]):
                block[3 + i] = int(round(mm * 10))
            block[7] = (-int(round(self._force_limit_n)) if self._grip else 0) & 0xFFFF
            block[9] = int(round((width + 3.0) * 100)) & 0xFFFF
            block[10] = int(round(width * 100)) & 0xFFFF
        else:
            base = frames.RG_STATUS_ADDRESS
            block = [0] * frames.RG_STATUS_COUNT
            block[0] = 0                                           # the fingertip offset: this simulator has no fingertips fitted
            block[1] = 24000                                       # 24 V
            block[2] = (600 if self._moving else 80)
            block[5] = int(round(width * 10)) & 0xFFFF
            block[6] = int(round(width * 10)) & 0xFFFF
            block[7] = int(round((width - self._max_mm) * 6)) & 0xFFFF
            block[9] = int(round(width * 10)) & 0xFFFF
            block[10] = self._status_word()
            block[15] = int(round(self._temperature * 100))
            block[17] = int(round(width * 10)) & 0xFFFF
            block[18] = 0
            block[19] = int(round((self._velocity() * 10))) & 0xFFFF
        for i, value in enumerate(block):
            self._memory[base + i] = value
        self._history.append((time.monotonic(), list(self._memory[base:base + len(block)])))
        del self._history[:-400]

    def _velocity(self) -> float:
        if not self._moving:
            return 0.0
        return self._speed_mm_s * (1.0 if self._target > self._width else -1.0)

    # ── what a test looks at ─────────────────────────────────────────────────────

    @property
    def width_mm(self) -> float:
        with self._lock:
            return self._width

    @property
    def target_mm(self) -> float:
        with self._lock:
            return self._target

    @property
    def moving(self) -> bool:
        with self._lock:
            return self._moving

    @property
    def gripping(self) -> bool:
        with self._lock:
            return self._grip

    @property
    def force_limit_n(self) -> float:
        with self._lock:
            return self._force_limit_n

    @property
    def supply_power_w(self) -> int:
        with self._lock:
            return self._supply_w

    def place(self, width_mm: float) -> None:
        """The fingers are at this width and not moving: a gripper as a person left it."""
        with self._lock:
            self._width = self._target = float(width_mm)
            self._moving = False
            self._grip = False
            self._publish()

    def put_object(self, width_mm: float | None) -> None:
        with self._lock:
            self.object_mm = width_mm

    def set_error(self, code: int = 0, flags: int = 0) -> None:
        """An RG's error code (0 to 7) and the status bits a test sets: the safety bits of an RG, not-calibrated and the sensor error of a 2FG."""
        with self._lock:
            self._error_code = code
            self._flags = flags
            self._publish()

    def heal(self) -> None:
        self.set_error(0, 0)
        with self._lock:
            self.drop = self.silent = self.garble = self.deaf = self.frozen = self.mute_reads = self.refuse_connections = False
            self.delay_s = self.split_gap_s = self.stale_state_s = self.late_answer_s = 0.0
            self.truncate = self.skip_requests = self.exception_next = 0
            self.wrong_unit = None

    def power_cycle(self) -> None:
        """The gripper loses its power and comes back: a 2FG's supply budget returns to 14 W, the fingers stay where they are."""
        with self._lock:
            self._supply_w = frames.FG_POWER_MIN_W
            self._moving = False
            self._target = self._width
            self._reset_registers()

    def set_registers(self, address: int, values: list[int]) -> None:
        with self._lock:
            for i, value in enumerate(values):
                self._memory[address + i] = value & 0xFFFF

    # ── the control block ────────────────────────────────────────────────────────

    def _apply_write(self, address: int, values: list[int]) -> bool:
        """True if the write is one the gripper takes. A write outside the control block (and the supply-power register of a 2FG) is an illegal address."""
        end = address + len(values)
        if self._fg:
            if address == frames.FG_POWER_ADDRESS and len(values) == 1:
                value = values[0]
                if not frames.FG_POWER_MIN_W <= value <= frames.FG_POWER_MAX_W:
                    return False
                if not self.forget_power:
                    self._supply_w = value
                    self._memory[frames.FG_POWER_ADDRESS] = value
                    self._memory[frames.FG_POWER_ADDRESS + 1] = self._force_ceiling()
                return True
            if address == frames.FG_CONTROL_ADDRESS and len(values) == 4:
                return self._control_fg(values)
            return False
        if address == frames.RG_CONTROL_ADDRESS and len(values) == 3:
            return self._control_rg(values)
        return False if end > 0 else False

    def _control_rg(self, values: list[int]) -> bool:
        force, width, control = values
        if self.deaf:
            return True
        if control == frames.RG_CONTROL_STOP:
            self._stop()
            return True
        if control == frames.RG_CONTROL_GRIP:
            self._start(width / 10.0, RG_SPEED_MM_S[self.model.name], force / 10.0)
            return True
        return False

    def _control_fg(self, values: list[int]) -> bool:
        width, force, speed, kind = values
        if self.deaf:
            return True
        if kind == frames.FG_TYPE_STOP:
            self._stop()
            return True
        if kind in (frames.FG_TYPE_EXTERNAL, frames.FG_TYPE_INTERNAL):
            if not 1 <= speed <= 100:
                return False
            self._start(width / 10.0, FG_FULL_SPEED_MM_S[self.model.name] * speed / 100.0, float(force or self.model.min_force_n or 0.0))
            return True
        return False

    def _start(self, target: float, speed: float, force: float) -> None:
        self._target = min(max(target, self._min_mm), self._max_mm)
        self._speed_mm_s = speed
        self._force_limit_n = force
        self._moving = abs(self._target - self._width) > 0.05
        if self._target > self._width:
            self._grip = False
        self._last_tick = time.monotonic()
        self.control_writes.append((time.monotonic(), tuple(self._memory[:4 if self._fg else 3])))
        self._publish()

    def _stop(self) -> None:
        self._moving = False
        self._target = self._width
        self._publish()

    # ── the physics ──────────────────────────────────────────────────────────────

    def _physics(self) -> None:
        while not self._stopping:
            time.sleep(TICK_S)
            now = time.monotonic()
            with self._lock:
                dt = (now - self._last_tick) * self.time_scale
                self._last_tick = now
                if self._moving:
                    step = self._speed_mm_s * dt
                    toward = self._target - self._width
                    closing = toward < 0
                    new = self._target if abs(toward) <= step else self._width + (step if toward > 0 else -step)
                    if closing and self.object_mm is not None and self._width >= self.object_mm and new <= self.object_mm:
                        new = self.object_mm
                        self._moving = False
                        self._grip = True
                    elif new == self._target:
                        self._moving = False
                    self._width = new
                self._publish()

    # ── the line ─────────────────────────────────────────────────────────────────

    def start(self) -> "SimulatedGripper":
        import pty
        self._stopping = False
        self._master, self._slave_fd = pty.openpty()
        import termios
        attributes = termios.tcgetattr(self._slave_fd)
        attributes[0] = attributes[1] = attributes[3] = 0
        attributes[2] = termios.CS8 | termios.PARENB | termios.CREAD | termios.CLOCAL
        attributes[6][termios.VMIN] = 0
        attributes[6][termios.VTIME] = 0
        termios.tcsetattr(self._slave_fd, termios.TCSANOW, attributes)
        self.path = os.ttyname(self._slave_fd)
        self._listener = socket.socket()
        self._listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._listener.bind(("127.0.0.1", 0))
        self._listener.listen(4)
        self._listener.settimeout(0.1)
        self.port = self._listener.getsockname()[1]
        self._thread = threading.Thread(target=self._serve, daemon=True, name="onrobot-sim")
        self._thread.start()
        self._physics_thread = threading.Thread(target=self._physics, daemon=True, name="onrobot-sim-physics")
        self._physics_thread.start()
        self._tcp_thread = threading.Thread(target=self._accept, daemon=True, name="onrobot-sim-accept")
        self._tcp_thread.start()
        return self

    def stop(self) -> None:
        self._stopping = True
        for thread in (self._thread, self._physics_thread, self._tcp_thread):
            if thread is not None:
                thread.join(timeout=1.0)
        self.hang_up()
        if self._listener is not None:
            try:
                self._listener.close()
            except OSError:
                pass
            self._listener = None
        for fd in (*self._attached, self._master, self._slave_fd):
            if fd >= 0:
                try:
                    os.close(fd)
                except OSError:
                    pass
        self._attached = []
        self._master = self._slave_fd = -1

    def __enter__(self) -> "SimulatedGripper":
        return self.start()

    def __exit__(self, *exc: object) -> None:
        self.stop()

    def attach(self, path: str) -> None:
        """Serve the gripper's RTU on a serial device that already exists: the substrate's end of a fault-injecting relay."""
        import termios
        fd = os.open(path, os.O_RDWR | os.O_NOCTTY)
        attributes = termios.tcgetattr(fd)
        attributes[0] = attributes[1] = attributes[3] = 0
        attributes[2] = termios.CS8 | termios.PARENB | termios.CREAD | termios.CLOCAL
        attributes[6][termios.VMIN] = 0
        attributes[6][termios.VTIME] = 0
        termios.tcsetattr(fd, termios.TCSANOW, attributes)
        self._attached.append(fd)
        threading.Thread(target=self._pump_rtu, args=(lambda wait: select.select([fd], [], [], wait)[0], lambda: os.read(fd, 512), lambda data: _write_fd(fd, data)), kwargs={"pty": True},
                         daemon=True, name="onrobot-sim-attached").start()

    def _serve(self) -> None:
        self._pump_rtu(lambda wait: select.select([self._master], [], [], wait)[0], lambda: os.read(self._master, 512), lambda data: _write_fd(self._master, data), pty=True)

    def _accept(self) -> None:
        while not self._stopping:
            listener = self._listener
            if listener is None:
                time.sleep(0.02)
                continue
            try:
                connection, _ = listener.accept()
            except (socket.timeout, OSError):
                continue
            self.connections += 1
            if self.refuse_connections:
                connection.close()
                continue
            self._open_connections.append(connection)
            threading.Thread(target=self._serve_socket, args=(connection,), daemon=True, name="onrobot-sim-conn").start()

    def stop_listening(self) -> None:
        """The Compute Box's port is closed: a driver that tries to reach it is refused, as it is when the box is down."""
        listener, self._listener = self._listener, None
        if listener is not None:
            listener.close()

    def resume_listening(self) -> None:
        listener = socket.socket()
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", self.port))
        listener.listen(4)
        listener.settimeout(0.1)
        self._listener = listener

    def pull(self) -> None:
        """The adapter is pulled out: the pseudo-terminal's far end goes, and the driver's reads and writes on its own end fail with an OS error."""
        if self._master >= 0:
            try:
                os.close(self._master)
            except OSError:
                pass
            self._master = -1

    def hang_up(self) -> None:
        """The Compute Box drops every connection it has (it restarted, or its link did): the next one is accepted as usual."""
        for connection in self._open_connections:
            try:
                connection.shutdown(socket.SHUT_RDWR)
                connection.close()
            except OSError:
                pass
        self._open_connections = []

    def _serve_socket(self, connection: socket.socket) -> None:
        connection.setblocking(True)
        pending = b""
        while not self._stopping:
            if not select.select([connection], [], [], 0.05)[0]:
                continue
            try:
                chunk = connection.recv(512)
            except OSError:
                break
            if not chunk:
                break
            if self.silent:
                pending = b""
                continue
            pending += chunk
            while len(pending) >= modbus.MBAP_BYTES:
                length = struct.unpack(">H", pending[4:6])[0]
                total = 6 + length
                if len(pending) < total:
                    break
                frame, pending = pending[:total], pending[total:]
                self._handle_tcp(frame, lambda data: _send_all(connection, data))
        try:
            connection.close()
        except OSError:
            pass

    def _pump_rtu(self, ready, read, write, *, pty: bool = False) -> None:
        """RTU framing by length: a request's function says how long it is. A partial frame that goes quiet is dropped, as a slave's receiver drops it."""
        pending = b""
        quiet_since = time.monotonic()
        while not self._stopping:
            try:
                waiting = ready(0.05)
            except (OSError, ValueError):
                time.sleep(0.01)                # the far end is gone (`pull`)
                continue
            if not waiting:
                if pending and time.monotonic() - quiet_since > 0.05:
                    pending = b""
                continue
            try:
                chunk = read()
            except (OSError, ConnectionError):
                time.sleep(0.01)
                continue
            if self.silent:
                pending = b""
                continue
            quiet_since = time.monotonic()
            pending += chunk
            while len(pending) >= 2:
                total = self._rtu_length(pending)
                if total is None:
                    pending = pending[1:]
                    continue
                if len(pending) < total:
                    break
                frame, pending = pending[:total], pending[total:]
                self._handle_rtu(frame, write)

    @staticmethod
    def _rtu_length(buffer: bytes) -> int | None:
        function = buffer[1]
        if function in (modbus.FC_READ_HOLDING, modbus.FC_WRITE_SINGLE):
            return 8
        if function == modbus.FC_WRITE_MULTIPLE:
            return None if len(buffer) < 7 else 9 + buffer[6]
        return None

    # ── answering ────────────────────────────────────────────────────────────────

    def _handle_tcp(self, frame: bytes, write) -> None:
        transaction, protocol, _, unit = struct.unpack(">HHHB", frame[:7])
        pdu = frame[7:]
        if protocol != 0 or unit != self.slave or self.drop or (self.mute_reads and pdu[:1] == bytes([modbus.FC_READ_HOLDING])):
            return
        if self.skip_requests > 0:
            self.skip_requests -= 1
            return
        answer = self._execute(pdu)
        if answer is None:
            return
        if self.garble:
            answer = bytes([answer[0] ^ 0x40]) + answer[1:]
        unit_out = self.wrong_unit if self.wrong_unit is not None else unit
        data = struct.pack(">HHHB", transaction, 0, len(answer) + 1, unit_out) + answer
        self._deliver(data, write, tcp=True, transaction=transaction, unit=unit_out)

    def _handle_rtu(self, frame: bytes, write) -> None:
        if modbus.crc16(frame[:-2]) != struct.unpack("<H", frame[-2:])[0]:
            return                                              # a corrupt request is not answered
        if frame[0] != self.slave or self.drop or (self.mute_reads and frame[1:2] == bytes([modbus.FC_READ_HOLDING])):
            return
        if self.skip_requests > 0:
            self.skip_requests -= 1
            return
        answer = self._execute(frame[1:-2])
        if answer is None:
            return
        slave_out = self.wrong_unit if self.wrong_unit is not None else self.slave
        body = bytes([slave_out]) + answer
        data = body + struct.pack("<H", modbus.crc16(body) ^ (0xFFFF if self.garble else 0))
        self._deliver(data, write, tcp=False)

    def _deliver(self, data: bytes, write, *, tcp: bool, transaction: int = 0, unit: int = 0) -> None:
        if self.delay_s:
            time.sleep(self.delay_s)
        if self.truncate:
            data = data[:max(0, len(data) - self.truncate)]
        if tcp and self.late_answer_s:
            # A stale answer first (an earlier transaction's), then the real one after the delay.
            stale = struct.pack(">HHHB", (transaction - 1) & 0xFFFF, 0, 3, unit) + bytes([modbus.FC_READ_HOLDING | 0x80, 4])
            write(stale)
            time.sleep(self.late_answer_s)
        if self.split_gap_s and len(data) > 3:
            write(data[:3])
            time.sleep(self.split_gap_s)
            write(data[3:])
        else:
            write(data)

    def _execute(self, pdu: bytes) -> bytes | None:
        """The answer PDU to a request PDU, or None for a request that is not answered."""
        if not pdu:
            return None
        function = pdu[0]
        with self._lock:
            if self.exception_next > 0:
                self.exception_next -= 1
                return bytes([function | 0x80, self.exception_code])
            if function == modbus.FC_READ_HOLDING and len(pdu) == 5:
                address, count = struct.unpack(">HH", pdu[1:5])
                if not 1 <= count <= modbus.MAX_READ_REGISTERS or address + count > MEMORY:
                    return bytes([function | 0x80, 2])
                self.requests.append((function, address, count))
                return bytes([function, 2 * count]) + struct.pack(f">{count}H", *self._read(address, count))
            if function == modbus.FC_WRITE_SINGLE and len(pdu) == 5:
                address, value = struct.unpack(">HH", pdu[1:5])
                self.requests.append((function, address, 1))
                if not self._apply_write(address, [value]):
                    return bytes([function | 0x80, 2])
                self.writes.append((time.monotonic(), address, (value,)))
                return pdu
            if function == modbus.FC_WRITE_MULTIPLE and len(pdu) >= 6:
                address, count, byte_count = struct.unpack(">HHB", pdu[1:6])
                if byte_count != 2 * count or len(pdu) != 6 + byte_count:
                    return bytes([function | 0x80, 3])
                values = list(struct.unpack(f">{count}H", pdu[6:]))
                self.requests.append((function, address, count))
                if not self._apply_write(address, values):
                    return bytes([function | 0x80, 2])
                if not (self.deaf and address == 0):
                    for i, value in enumerate(values):
                        self._memory[address + i] = value
                self.writes.append((time.monotonic(), address, tuple(values)))
                return pdu[:5]
            return bytes([function | 0x80, 1])

    def _read(self, address: int, count: int) -> list[int]:
        memory = self._memory
        if self.stale_state_s and self._history:
            base = frames.FG_STATUS_ADDRESS if self._fg else frames.RG_STATUS_ADDRESS
            cutoff = time.monotonic() - self.stale_state_s
            earlier = [snapshot for when, snapshot in self._history if when <= cutoff]
            snapshot = earlier[-1] if earlier else self._history[0][1]
            memory = list(self._memory)
            memory[base:base + len(snapshot)] = snapshot
        return memory[address:address + count]
