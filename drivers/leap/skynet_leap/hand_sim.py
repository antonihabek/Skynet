"""A simulated LEAP Hand, the dry substrate: sixteen Dynamixel X-series servos on one bus behind a pseudo-terminal (or a TCP port), speaking protocol 2.0 (DRY-COVERAGE-PLAN.md item 12.E).

Written here from the e-manual's description of the XC330-M288-T and XL330-M288-T and from reading what the hand's makers' own client does with them, so **it agrees with whoever wrote the driver** (F15): no ROBOTIS or
LEAP Hand software or hardware is on the other end. ROBOTIS's own `dynamixel_sdk` runs against it unmodified (`layout_check.py`, and so does the LEAP Hand authors' own `dynamixel_client.py` where a person points
`LEAP_API` at a checkout), which is what makes the bytes independent evidence. What it adds is the part a driver has to survive: servos that cross their range at a speed, an object that stops a closing finger and loads its motor, a hardware error that shuts a servo's
torque off, a power cycle that gives back the RAM (torque off, the default gains), and a lever for every way a line can go wrong.

    hand = SimulatedHand("full").start()        # hand.path is the serial device, hand.port a TCP port when tcp=True

What it does **not** model, because nothing here says: the real servo's control loop (a servo here moves toward its goal at a fixed speed), the current a real servo draws (a figure proportional to the error, capped
at the goal current), its temperature, or what the real firmware answers to an instruction it does not know (it answers with an instruction error here).
"""

from __future__ import annotations

import os
import select
import socket
import threading
import time

from . import frames

TICK_S = 0.004
MEMORY = 256
#: The servo's own figures here (nothing the e-manual says is a no-load speed this repository could read): how fast a servo crosses its range, in radians a second.
SPEED_RAD_S = 4.0
#: The e-manual's defaults for the XC330-M288 (current limit 2352 mA, gains P 1100, I 0, D 500, Shutdown 52); the Lite's current limit is the simulator's own.
DEFAULT_CURRENT_LIMIT = {"lite": 1750, "full": 2352}
DEFAULT_GAINS = {"p": 1100, "i": 0, "d": 500}
DEFAULT_SHUTDOWN = 52
START_REGISTER = 2048 + 0                       # a servo at the hand's sim zero (the angle register for 3.14159 rad)

_ERR_RESULT_FAIL, _ERR_INSTRUCTION, _ERR_DATA_RANGE, _ERR_DATA_LENGTH, _ERR_ACCESS = 1, 2, 4, 5, 7
_EEPROM_END = 64                                # addresses below this are EEPROM, locked while torque is on
_READ_ONLY = {0, 1, 2, 3, 4, 5, 6, 70, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 144, 145, 146}


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


class _Servo:
    """One servo's registers and state."""

    def __init__(self, servo_id: int, variant: str, mode: int, position: float, torque: bool) -> None:
        self.id = servo_id
        self.variant = variant
        self.mem = bytearray(MEMORY)
        self.position = float(position)
        self.moving_to: float | None = None
        self.silent = False
        self.blocked = False
        self.stalled = False
        self._eeprom_defaults(mode)
        self.power_on(position, torque)

    def _eeprom_defaults(self, mode: int) -> None:
        number = frames.VARIANTS[self.variant].model_number
        self.mem[0:2] = number.to_bytes(2, "little")
        self.mem[6] = 52                                            # firmware version: the simulator's own
        self.mem[7] = self.id
        self.mem[8] = 6                                             # 4 Mbaud (e-manual's code 6)
        self.mem[9] = 250
        self.mem[11] = mode
        self.mem[38:40] = DEFAULT_CURRENT_LIMIT[self.variant].to_bytes(2, "little")
        self.mem[63] = DEFAULT_SHUTDOWN

    def power_on(self, position: float, torque: bool = False) -> None:
        """What a power-on leaves: the EEPROM as it was, the RAM at its defaults, torque off, and a goal where the servo is."""
        for address in range(64, MEMORY):
            self.mem[address] = 0
        self.mem[68] = 2                                            # status return level: everything is answered
        self.mem[80:82] = DEFAULT_GAINS["d"].to_bytes(2, "little")
        self.mem[82:84] = DEFAULT_GAINS["i"].to_bytes(2, "little")
        self.mem[84:86] = DEFAULT_GAINS["p"].to_bytes(2, "little")
        self.mem[102:104] = self.mem[38:40]
        self.position = float(position)
        self.moving_to = None
        self.blocked = False
        self.mem[116:120] = int(round(position)).to_bytes(4, "little", signed=True)
        self.mem[132:136] = int(round(position)).to_bytes(4, "little", signed=True)
        self.mem[64] = 1 if torque else 0
        self.mem[144:146] = (50).to_bytes(2, "little")             # 5.0 V
        self.mem[146] = 30

    # ── registers ──

    def get(self, address: int, length: int) -> bytes:
        return bytes(self.mem[address:address + length])

    def int16(self, address: int) -> int:
        return int.from_bytes(self.mem[address:address + 2], "little", signed=True)

    def int32(self, address: int) -> int:
        return int.from_bytes(self.mem[address:address + 4], "little", signed=True)

    @property
    def torque(self) -> bool:
        return bool(self.mem[64])

    @property
    def hw_error(self) -> int:
        return self.mem[70]


class SimulatedHand:
    def __init__(self, variant: str = "full", *, time_scale: float = 1.0, start_registers: tuple[int, ...] | None = None, mode: int = 5, torque: bool = False, tcp: bool = False) -> None:
        self.variant = variant
        self.time_scale = time_scale
        self.path = ""
        self.port = 0
        self._master = -1
        self._slave_fd = -1
        self._lock = threading.RLock()
        start = start_registers or (START_REGISTER,) * 16
        self._servos = {i: _Servo(i, variant, mode, start[i], torque) for i in frames.IDS}
        #: An object in front of each finger, as the position register the finger stops at when it closes on it; None for nothing there.
        self.blocks: list[int | None] = [None] * 16
        self._stopping = False
        self._thread: threading.Thread | None = None
        self._physics_thread: threading.Thread | None = None
        self._tcp = tcp
        self._listener: socket.socket | None = None
        self._tcp_thread: threading.Thread | None = None
        self._open_connections: list[socket.socket] = []
        self._attached: list[int] = []
        self._history: list[tuple[float, dict[int, tuple[bytes, bytes]]]] = []
        # What a test turns on.
        self.drop = False                   # requests are read and not answered: a bus that has gone
        self.delay_s = 0.0                  # every answer waits
        self.garble = False                 # every answer's CRC is wrong
        self.silent = False                 # the line is dead: nothing is read, nothing answered
        self.split_gap_s = 0.0              # an answer comes in two pieces with this long between them (a slow USB adapter)
        self.split_at = 3                   # where an answer is cut when `split_gap_s` splits it
        self.truncate = 0                   # every answer is this many bytes short
        self.noise = 0                      # a request is answered with this many bytes of noise that hold no header: a line at the wrong baud rate
        self.trailer = b""                  # bytes sent after every answer that belong to no answer
        self.skip_requests = 0              # the next this-many requests are read and not answered: a lost frame
        self.echo_request = False           # the request is heard back before the answer, as a half-duplex adapter does
        self.reject_writes = 0              # the next this-many writes are answered with a result-fail error
        self.deaf = False                   # a write to Goal_Position is acknowledged and not taken
        self.mute_reads = False             # reads are not answered and writes are: servos that take what they are told and say nothing
        self.echo_lag_s = 0.0               # reads of the feedback registers show the hand as it was this long ago
        self.refuse_connections = False     # a TCP connection is accepted and closed at once: a gateway that is up and will not talk
        self.connections = 0                # TCP connections accepted so far
        self.report_positions: list[int] | None = None      # Present_Position reads as these, whatever the servos are at
        self.rest_offset = 0                # a servo that arrives rests this many ticks short of its goal, and says it arrived
        self.persistent: dict[int, int] = {}                # hardware errors whose cause is still there: a reboot brings them back
        self.requests: list[tuple[int, int, int, tuple[int, ...]]] = []     # (instruction, address, length, ids) of each request answered
        self.writes: list[tuple[float, int, int, bytes]] = []              # (time, id, address, data) of each write that was applied
        self.goal_writes: list[tuple[float, dict[int, int]]] = []          # each Goal_Position write, as {id: register} (a signed value)

    # ── what a test looks at and does to the hand ────────────────────────────────

    @property
    def positions(self) -> list[int]:
        with self._lock:
            return [int(round(self._servos[i].position)) for i in frames.IDS]

    @property
    def goals(self) -> list[int]:
        with self._lock:
            return [self._servos[i].int32(116) for i in frames.IDS]

    @property
    def torque(self) -> list[int]:
        with self._lock:
            return [self._servos[i].mem[64] for i in frames.IDS]

    @property
    def hw_errors(self) -> list[int]:
        with self._lock:
            return [self._servos[i].mem[70] for i in frames.IDS]

    @property
    def gains(self) -> list[tuple[int, int, int]]:
        """(D, I, P) of each servo."""
        with self._lock:
            return [(int.from_bytes(self._servos[i].mem[80:82], "little"), int.from_bytes(self._servos[i].mem[82:84], "little"), int.from_bytes(self._servos[i].mem[84:86], "little")) for i in frames.IDS]

    @property
    def goal_currents(self) -> list[int]:
        with self._lock:
            return [self._servos[i].int16(102) for i in frames.IDS]

    @property
    def operating_modes(self) -> list[int]:
        with self._lock:
            return [self._servos[i].mem[11] for i in frames.IDS]

    def servo(self, servo_id: int) -> _Servo:
        return self._servos[servo_id]

    def place(self, registers: tuple[int, ...] | list[int], *, retarget: bool = True) -> None:
        """Put the servos somewhere without moving them there. Their goals go there too (a servo placed at a position is a servo holding it), unless `retarget` is False."""
        with self._lock:
            for i, register in zip(frames.IDS, registers):
                servo = self._servos[i]
                servo.position = float(register)
                servo.moving_to = None
                servo.blocked = False
                if retarget:
                    servo.mem[116:120] = int(register).to_bytes(4, "little", signed=True)
            self._snapshot()

    def set_register(self, servo_id: int, address: int, data: bytes) -> None:
        """Write registers as something other than the driver (another master, a power-on) would have."""
        with self._lock:
            self._servos[servo_id].mem[address:address + len(data)] = data

    def fault(self, servo_id: int, bits: int) -> None:
        """A hardware error: the bits are set in Hardware_Error_Status, and, for those in the Shutdown register, the servo's torque goes off and it stops where it is."""
        with self._lock:
            servo = self._servos[servo_id]
            servo.mem[70] |= bits
            if bits & servo.mem[63]:
                servo.mem[64] = 0
                servo.moving_to = None

    def clear_faults(self) -> None:
        with self._lock:
            self.persistent.clear()
            for servo in self._servos.values():
                servo.mem[70] = 0

    def persist(self, servo_id: int, bits: int) -> None:
        """A hardware error whose cause is still there after a reboot (a servo that is still too hot)."""
        with self._lock:
            self.persistent[servo_id] = bits

    def power_cycle(self, servo_id: int | None = None) -> None:
        """The hand (or one servo) loses its power and comes back: EEPROM kept, RAM at its defaults, torque off, a goal where the servo is; the fingers stay where they were."""
        with self._lock:
            for i, servo in self._servos.items():
                if servo_id is None or i == servo_id:
                    servo.power_on(servo.position)
            self._snapshot()

    def silence(self, servo_id: int, silent: bool = True) -> None:
        """One servo stops answering: a cable that came out, a servo that has lost its power."""
        with self._lock:
            self._servos[servo_id].silent = silent

    def push(self, servo_id: int, ticks: float) -> None:
        """A finger is moved by hand."""
        with self._lock:
            self._servos[servo_id].position += ticks
            self._snapshot()

    def heal(self) -> None:
        """Every lever back to its default."""
        with self._lock:
            self.drop = self.garble = self.silent = self.deaf = self.mute_reads = self.echo_request = False
            self.delay_s = self.split_gap_s = self.echo_lag_s = 0.0
            self.truncate = self.noise = self.skip_requests = self.reject_writes = 0
            self.trailer = b""
            self.split_at = 3
            self.report_positions = None
            self.rest_offset = 0
            for servo in self._servos.values():
                servo.silent = False

    # ── the line ─────────────────────────────────────────────────────────────────

    def start(self) -> "SimulatedHand":
        import termios
        self._master, self._slave_fd = os.openpty()
        self.path = os.ttyname(self._slave_fd)
        attributes = termios.tcgetattr(self._slave_fd)
        attributes[0] = attributes[1] = attributes[3] = 0
        attributes[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
        attributes[6][termios.VMIN] = 0
        attributes[6][termios.VTIME] = 0
        termios.tcsetattr(self._slave_fd, termios.TCSANOW, attributes)
        with self._lock:
            self._snapshot()
        self._stopping = False
        self._thread = threading.Thread(target=self._serve, daemon=True, name="leap-sim-line")
        self._physics_thread = threading.Thread(target=self._physics, daemon=True, name="leap-sim-physics")
        self._thread.start()
        self._physics_thread.start()
        if self._tcp:
            self._listener = socket.socket()
            self._listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            self._listener.bind(("127.0.0.1", 0))
            self._listener.listen(4)
            self._listener.settimeout(0.1)
            self.port = self._listener.getsockname()[1]
            self._tcp_thread = threading.Thread(target=self._accept, daemon=True, name="leap-sim-tcp")
            self._tcp_thread.start()
        return self

    def stop(self) -> None:
        self._stopping = True
        for thread in (self._thread, self._physics_thread, self._tcp_thread):
            if thread is not None:
                thread.join(timeout=2.0)
        if self._listener is not None:
            self._listener.close()
            self._listener = None
        for fd in (self._master, self._slave_fd, *self._attached):
            if fd >= 0:
                try:
                    os.close(fd)
                except OSError:
                    pass
        self._master = self._slave_fd = -1

    def __enter__(self) -> "SimulatedHand":
        return self.start()

    def __exit__(self, *exc: object) -> None:
        self.stop()

    def _serve(self) -> None:
        self._pump(lambda wait: select.select([self._master], [], [], wait)[0], lambda: os.read(self._master, 1024), self._write_pty, pty=True)

    def _pump(self, ready, read, write, *, pty: bool = False) -> None:
        """The framing of a request: find `FF FF FD 00`, read the length, and take that many bytes more. A partial packet that goes quiet is dropped, as a servo's receiver drops it."""
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
                if pty:
                    time.sleep(0.01)
                    continue
                return
            if chunk == b"" and not pty:
                return
            if self.silent:
                pending = b""
                continue
            quiet_since = time.monotonic()
            pending += chunk
            while True:
                start = pending.find(frames.HEADER)
                if start < 0:
                    pending = pending[-3:]
                    break
                pending = pending[start:]
                if len(pending) < 7:
                    break
                total = 7 + (pending[5] | pending[6] << 8)
                if len(pending) < total:
                    break
                frame, pending = pending[:total], pending[total:]
                self._handle(frame, write)

    def _write_pty(self, data: bytes) -> None:
        try:
            os.write(self._master, data)
        except OSError:
            pass

    def attach(self, path: str) -> None:
        """Serve the hand on a serial device that already exists: the substrate's end of a fault-injecting relay (`tools/faultproxy`), which makes the pair of pseudo-terminals and cannot let the substrate make its own."""
        import termios
        fd = os.open(path, os.O_RDWR | os.O_NOCTTY)
        attributes = termios.tcgetattr(fd)
        attributes[0] = attributes[1] = attributes[3] = 0
        attributes[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
        attributes[6][termios.VMIN] = 0
        attributes[6][termios.VTIME] = 0
        termios.tcsetattr(fd, termios.TCSANOW, attributes)
        self._attached.append(fd)
        threading.Thread(target=self._pump, args=(lambda wait: select.select([fd], [], [], wait)[0], lambda: os.read(fd, 1024), lambda data: _write_fd(fd, data)), kwargs={"pty": True},
                         daemon=True, name="leap-sim-attached").start()

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
            threading.Thread(target=self._serve_socket, args=(connection,), daemon=True, name="leap-sim-conn").start()

    def stop_listening(self) -> None:
        """The gateway's port is closed: a driver that tries to reach it is refused, as it is when the gateway is down."""
        listener, self._listener = self._listener, None
        if listener is not None:
            listener.close()
            time.sleep(0.15)                    # the accepting thread is inside a 100 ms accept that would take one more connection

    def resume_listening(self) -> None:
        listener = socket.socket()
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", self.port))
        listener.listen(4)
        listener.settimeout(0.1)
        self._listener = listener

    def hang_up(self) -> None:
        """The gateway drops every connection it has (it restarted, or its link did): the next one is accepted as usual."""
        for connection in self._open_connections:
            try:
                connection.shutdown(socket.SHUT_RDWR)
                connection.close()
            except OSError:
                pass
        self._open_connections = []

    def _serve_socket(self, connection: socket.socket) -> None:
        connection.setblocking(True)
        self._pump(lambda wait: select.select([connection], [], [], wait)[0], lambda: connection.recv(1024), lambda data: _send_all(connection, data))
        connection.close()

    def pull(self) -> None:
        """The adapter is pulled out: the pseudo-terminal's far end goes, and the driver's reads and writes on its own end fail with an OS error."""
        if self._master >= 0:
            try:
                os.close(self._master)
            except OSError:
                pass
            self._master = -1

    # ── what a servo does with a packet ──────────────────────────────────────────

    def _snapshot(self) -> None:
        """The feedback registers as they are now, kept for `echo_lag_s`."""
        self._history.append((time.monotonic(), {i: (bytes(s.mem[122:136]), bytes(s.mem[144:147])) for i, s in self._servos.items()}))
        del self._history[:-400]

    def _status(self, servo: _Servo, data: bytes = b"", error: int = 0) -> bytes:
        if servo.mem[70]:
            error |= frames.ALERT
        return frames.packet(servo.id, frames.INST_STATUS, bytes([error]) + data)

    def _emit(self, request: bytes, packets: list[bytes], write) -> None:
        data = b"".join(packets)
        if not data:
            return
        if self.delay_s:
            time.sleep(self.delay_s)
        if self.noise:
            return write(b"U" * self.noise)
        if self.garble:
            data = data[:-1] + bytes([data[-1] ^ 0xFF])
        if self.truncate:
            data = data[:max(0, len(data) - self.truncate)]
        if self.trailer:
            data = data + self.trailer
        if self.echo_request:
            data = request + data
        if self.split_gap_s and len(data) > self.split_at:
            write(data[:self.split_at])
            time.sleep(self.split_gap_s)
            write(data[self.split_at:])
        else:
            write(data)

    def _handle(self, frame: bytes, write) -> None:
        if frames.crc16(frame[:-2]) != int.from_bytes(frame[-2:], "little"):
            return                                              # a request with a wrong CRC is not answered
        servo_id, instruction = frame[4], frame[7]
        params = frames.unstuff(frame[8:-2])
        if self.drop:
            return
        if self.skip_requests > 0:
            self.skip_requests -= 1
            return
        with self._lock:
            if instruction == frames.INST_SYNC_READ and len(params) >= 4:
                address, length = int.from_bytes(params[0:2], "little"), int.from_bytes(params[2:4], "little")
                ids = tuple(params[4:])
                self.requests.append((instruction, address, length, ids))
                if self.mute_reads:
                    return
                return self._emit(frame, [self._read_packet(i, address, length) for i in ids if i in self._servos and not self._servos[i].silent], write)
            if instruction == frames.INST_FAST_SYNC_READ and len(params) >= 4:
                address, length = int.from_bytes(params[0:2], "little"), int.from_bytes(params[2:4], "little")
                ids = tuple(params[4:])
                self.requests.append((instruction, address, length, ids))
                if self.mute_reads:
                    return
                present = [i for i in ids if i in self._servos and not self._servos[i].silent]
                return self._emit(frame, [self._fast_packet(present, address, length)] if present else [], write)
            if instruction == frames.INST_SYNC_WRITE and len(params) >= 4:
                address, length = int.from_bytes(params[0:2], "little"), int.from_bytes(params[2:4], "little")
                body = params[4:]
                ids = []
                goals: dict[int, int] = {}
                for at in range(0, len(body) - length, length + 1):
                    target = body[at]
                    ids.append(target)
                    if target in self._servos and not self._servos[target].silent:
                        if address == 116 and length == 4:
                            goals[target] = int.from_bytes(body[at + 1:at + 5], "little", signed=True)
                        self._apply_write(self._servos[target], address, body[at + 1:at + 1 + length])
                self.requests.append((instruction, address, length, tuple(ids)))
                if goals:
                    self.goal_writes.append((time.monotonic(), goals))
                return
            if servo_id not in self._servos or servo_id == frames.BROADCAST:
                return
            servo = self._servos[servo_id]
            if servo.silent:
                return
            if instruction == frames.INST_PING:
                self.requests.append((instruction, 0, 0, (servo_id,)))
                return self._emit(frame, [self._status(servo, servo.get(0, 2) + servo.get(6, 1))], write)
            if instruction == frames.INST_READ and len(params) == 4:
                address, length = int.from_bytes(params[0:2], "little"), int.from_bytes(params[2:4], "little")
                self.requests.append((instruction, address, length, (servo_id,)))
                if self.mute_reads:
                    return
                return self._emit(frame, [self._read_packet(servo_id, address, length)], write)
            if instruction == frames.INST_WRITE and len(params) >= 3:
                address, data = int.from_bytes(params[0:2], "little"), params[2:]
                self.requests.append((instruction, address, len(data), (servo_id,)))
                if address == 116 and len(data) == 4:
                    self.goal_writes.append((time.monotonic(), {servo_id: int.from_bytes(data, "little", signed=True)}))
                error = _ERR_RESULT_FAIL if self.reject_writes > 0 else self._apply_write(servo, address, data)
                if self.reject_writes > 0:
                    self.reject_writes -= 1
                return self._emit(frame, [self._status(servo, b"", error)], write)
            if instruction == frames.INST_REBOOT:
                self.requests.append((instruction, 0, 0, (servo_id,)))
                reply = self._status(servo)
                servo.power_on(servo.position)
                servo.mem[70] = self.persistent.get(servo.id, 0)
                return self._emit(frame, [reply], write)
            return self._emit(frame, [self._status(servo, b"", _ERR_INSTRUCTION)], write)

    def _read_packet(self, servo_id: int, address: int, length: int) -> bytes:
        servo = self._servos[servo_id]
        if address + length > MEMORY or length <= 0:
            return self._status(servo, b"", _ERR_DATA_RANGE)
        memory = servo.mem
        if self.echo_lag_s and self._history:
            cutoff = time.monotonic() - self.echo_lag_s
            earlier = [snapshot for when, snapshot in self._history if when <= cutoff]
            lagged = earlier[-1] if earlier else self._history[0][1]
            memory = bytearray(servo.mem)
            memory[122:136] = lagged[servo_id][0]
            memory[144:147] = lagged[servo_id][1]
        if self.report_positions is not None:
            memory = bytearray(memory)
            memory[132:136] = int(self.report_positions[servo_id]).to_bytes(4, "little", signed=True)
        return self._status(servo, bytes(memory[address:address + length]))

    def _fast_packet(self, ids: list[int], address: int, length: int) -> bytes:
        """ROBOTIS's Fast Sync Read answer: one packet, addressed (as the SDK expects) to the broadcast id, whose parameters are every servo's `error, id, data` followed by a CRC-16 of everything before it (the SDK checks the last one over the whole
        packet; the e-manual describes the others the same way)."""
        blocks = []
        for i in ids:
            servo = self._servos[i]
            status = self._read_packet(i, address, length)
            error = status[8]
            data = status[9:-2] if error & 0x7F == 0 else b"\x00" * length
            blocks.append((i, bytes([error, i]) + data))
        total = sum(len(block) + 2 for _, block in blocks) + 1
        head = frames.HEADER + bytes([frames.BROADCAST, total & 0xFF, total >> 8, frames.INST_STATUS])
        body = head
        for _, block in blocks:
            body += block
            body += frames.crc16(body).to_bytes(2, "little")
        return body

    def _apply_write(self, servo: _Servo, address: int, data: bytes) -> int:
        """Take a write into the registers, as far as the e-manual lets it. The status packet's error number: 0 for taken."""
        end = address + len(data)
        if end > MEMORY or not data:
            return _ERR_DATA_RANGE
        if any(a in _READ_ONLY for a in range(address, end)):
            return _ERR_ACCESS
        if address < _EEPROM_END and servo.torque:
            return _ERR_ACCESS                                  # the EEPROM area is locked while torque is on
        if address == 11 and data[0] not in (0, 1, 3, 4, 5, 16):
            return _ERR_DATA_RANGE
        if address == 64:
            if data[0] not in (0, 1):
                return _ERR_DATA_RANGE
            if data[0] == 1 and servo.mem[70] & servo.mem[63]:
                return _ERR_RESULT_FAIL                         # torque cannot be switched on over an error the shutdown setting acts on
        if address in (80, 82, 84) and len(data) == 2 and int.from_bytes(data, "little") > 16383:
            return _ERR_DATA_RANGE
        if address == 102 and len(data) == 2:
            limit = int.from_bytes(servo.mem[38:40], "little")
            if abs(int.from_bytes(data, "little", signed=True)) > limit:
                return _ERR_DATA_RANGE
        if address == 116 and len(data) == 4 and self.deaf:
            return 0                                            # acknowledged and not taken
        servo.mem[address:end] = data
        self.writes.append((time.monotonic(), servo.id, address, bytes(data)))
        if address == 116 and len(data) == 4:
            if servo.torque and servo.mem[11] in (3, 5):
                servo.moving_to = float(int.from_bytes(data, "little", signed=True))
        if address == 64:
            if data[0] == 1:
                servo.moving_to = float(servo.int32(116)) if servo.int32(116) != int(round(servo.position)) else None
            else:
                servo.moving_to = None
        return 0

    # ── the servos ───────────────────────────────────────────────────────────────

    def _physics(self) -> None:
        last = time.monotonic()
        while not self._stopping:
            time.sleep(TICK_S)
            now = time.monotonic()
            dt, last = now - last, now
            with self._lock:
                step = SPEED_RAD_S / (2 * 3.141592653589793 / 4096) * dt * self.time_scale
                for i, servo in self._servos.items():
                    self._advance(i, servo, step)
                self._snapshot()

    def _advance(self, i: int, servo: _Servo, step: float) -> None:
        goal = servo.moving_to
        moving = 0
        current = 0
        if goal is not None and servo.torque and not servo.stalled:
            block = self.blocks[i]
            gap = goal - servo.position
            limit = abs(servo.int16(102))
            if block is not None and goal > servo.position and servo.position + min(step, abs(gap)) >= block:
                servo.position = float(block)
                servo.blocked = True
                current = limit                                 # an object holds the finger: the motor is at its current limit
                moving = 0
            else:
                servo.blocked = False
                if abs(gap) <= step:
                    servo.position = goal - (self.rest_offset if gap > 0 else -self.rest_offset) if self.rest_offset else goal
                    servo.moving_to = None
                else:
                    servo.position += step if gap > 0 else -step
                    moving = 1
                    current = min(limit, int(abs(gap) * 0.5) + 20) * (1 if gap > 0 else -1)
        servo.mem[122] = moving
        servo.mem[123] = 0
        servo.mem[126:128] = int(current).to_bytes(2, "little", signed=True)
        velocity = (int(step / TICK_S / self.time_scale * (2 * 3.141592653589793 / 4096) / (0.229 * 2 * 3.141592653589793 / 60)) if moving else 0)
        servo.mem[128:132] = int(velocity).to_bytes(4, "little", signed=True)
        servo.mem[132:136] = int(round(servo.position)).to_bytes(4, "little", signed=True)
