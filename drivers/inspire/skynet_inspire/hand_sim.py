"""A simulated Inspire RH56 hand, the dry substrate: a private-protocol slave on a pseudo-terminal, behind the register map the maker's manuals document.

Written here from the manuals the driver was (`frames.py` cites them), and so **it agrees with whoever wrote the driver** (F15). What the maker publishes is a manual, a PC tool and no simulator, so this is the only hand
the driver runs against, and what it adds is the part a driver has to survive: fingers that move at the speed they were told, an object that stops a closing finger at a force threshold, the manual's statuses and
error bits, an over-temperature error that no write clears, a calibration that moves the hand by itself for six seconds, and levers for the line (`drop`, `delay_s`, `garble`, `silent`, `truncate`, `echo_request`, ...).

    hand = SimulatedHand().start()      # hand.path is the device a driver opens

What it does **not** model, because the manuals do not say: what a hand does with a command during its calibration (it ignores them here), what a speed of zero means (the finger does not move), and whether a
finger that has stalled takes a new target (it does not, until the error is cleared).
"""

from __future__ import annotations

import os
import select
import socket
import threading
import time

from . import frames
from .frames import ADDR_ANGLE_ACT, ADDR_ANGLE_SET, ADDR_CLEAR_ERROR, ADDR_CURRENT, ADDR_ERROR, ADDR_FORCE_ACT, ADDR_FORCE_CALIBRATION, ADDR_FORCE_SET, ADDR_SPEED_SET, ADDR_STATUS, ADDR_TEMP

MEMORY = 2048
#: The series manual §2.4.6: "the dexterous hand will start the force sensor calibration process, which will take 6 seconds".
CALIBRATION_S = 6.0
OVER_TEMPERATURE_C = 75


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


class SimulatedHand:
    def __init__(self, profile: str = "RH56", hand: int = 1, *, time_scale: float = 1.0, start_angles: tuple[int, ...] | None = None, tcp: bool = False) -> None:
        self.profile = frames.PROFILES[profile]
        self.hand = hand
        self.time_scale = time_scale
        self.path = ""
        self.port = 0
        self._master = -1
        self._slave_fd = -1
        self._lock = threading.RLock()
        self._memory = bytearray(MEMORY)
        self._angle = [float(v) for v in (start_angles or (1000, 1000, 1000, 1000, 1000, 1000))]
        self._moving_to: list[float | None] = [None] * 6
        self._status = [2] * 6
        self._error = [0] * 6
        self._force = [0] * 6
        self._temperature = [31] * 6
        self._calibrating_until = 0.0
        self._calibration_started = 0.0
        self._calibration_from: list[float] = list(self._angle)
        #: An object between each finger and the palm, as the angle register the finger stops at when it closes on it; None for nothing there.
        self.blocks: list[int | None] = [None] * 6
        self._stopping = False
        self._thread: threading.Thread | None = None
        self._physics_thread: threading.Thread | None = None
        self._tcp = tcp
        self._listener: socket.socket | None = None
        self._tcp_thread: threading.Thread | None = None
        self._open_connections: list[socket.socket] = []
        self._attached: list[int] = []
        self._history: list[tuple[float, bytes]] = []
        # The registers a power-on leaves: a target where the fingers are, the maker's default speed and force.
        self._store_shorts(ADDR_ANGLE_SET, [int(v) for v in self._angle])
        self._store_shorts(ADDR_FORCE_SET, [500] * 6)
        self._store_shorts(ADDR_SPEED_SET, [500] * 6)
        # What a test turns on.
        self.drop = False                   # requests are read and not answered: a hand that has gone
        self.delay_s = 0.0                  # every answer waits
        self.garble = False                 # every answer's checksum is wrong
        self.silent = False                 # the line is dead: nothing is read, nothing answered
        self.split_gap_s = 0.0              # an answer comes in two pieces with this long between them (a slow USB adapter)
        self.truncate = 0                   # every answer is this many bytes short
        self.reply_hand: int | None = None  # every answer says it is from this hand
        self.skip_requests = 0              # the next this-many requests are read and not answered: a lost frame
        self.echo_request = False           # the request is heard back before the answer, as a half-duplex adapter does
        self.reject_writes = 0              # the next this-many writes are answered with result 0
        self.deaf = False                   # a write to ANGLE_SET is acknowledged and not taken
        self.echo_lag_s = 0.0               # reads of the feedback registers show the hand as it was this long ago
        self.ack_result = 1                 # the result byte of every write acknowledgement (1 is success)
        self.noise = 0                      # a request is answered with this many bytes of noise that hold no header: a line at the wrong baud rate
        self.split_at = 3                   # where an answer is cut when `split_gap_s` splits it (1 is inside the two-byte header)
        self.trailer = b""                  # bytes sent after every answer that belong to no answer
        self.refuse_connections = False     # a TCP connection is accepted and closed at once: a gateway that is up and will not talk
        self.connections = 0                # TCP connections accepted so far
        self.rest_offset = 0                # a finger that arrives rests this many registers short of its target, and says it arrived
        self.report_angles: list[int] | None = None     # ANGLE_ACT reads as these, whatever the fingers are at
        self.report_forces: list[int] | None = None     # FORCE_ACT reads as these
        self.mute_reads = False             # reads are not answered and writes are: a hand that takes what it is told and says nothing
        self.requests: list[tuple[int, int, int]] = []     # (command, address, length) of each request answered
        self.writes: list[tuple[float, int, bytes]] = []   # (time, address, data) of each write that was acknowledged
        self.target_writes: list[tuple[float, tuple[int, ...]]] = []     # each ANGLE_SET write, as the registers it carried

    # ── the memory ───────────────────────────────────────────────────────────────────────────────────────

    def _store_shorts(self, address: int, values: list[int]) -> None:
        self._memory[address:address + 2 * len(values)] = frames.pack_shorts(values)

    def _load_shorts(self, address: int, count: int, *, signed: bool = True) -> list[int]:
        return frames.shorts(bytes(self._memory[address:address + 2 * count]), signed=signed)

    def _publish(self) -> None:
        angles = [int(round(a)) for a in self._angle]
        if self.report_angles is not None:
            angles = list(self.report_angles)
        self._store_shorts(frames.ADDR_ANGLE_ACT, angles)
        self._store_shorts(frames.ADDR_ANGLE_ACT - 12, [2 * (1000 - a) for a in angles])      # POS_ACT, 0 at the maximum angle (series manual §2.4.14)
        self._store_shorts(ADDR_FORCE_ACT, self.report_forces if self.report_forces is not None else self._force)
        self._store_shorts(ADDR_CURRENT, [self._current(j) for j in range(6)])
        self._memory[ADDR_ERROR:ADDR_ERROR + 6] = bytes(self._error)
        self._memory[ADDR_STATUS:ADDR_STATUS + 6] = bytes(self._status)
        self._memory[ADDR_TEMP:ADDR_TEMP + 6] = bytes(self._temperature)
        self._history.append((time.monotonic(), bytes(self._memory[frames.ADDR_ANGLE_ACT - 12:ADDR_TEMP + 6])))
        del self._history[:-400]

    def _current(self, joint: int) -> int:
        if self._error[joint] & 0b100:
            return 1500
        return 120 + (200 if self._moving_to[joint] is not None else 0) + abs(self._force[joint]) // 10

    @property
    def angles(self) -> list[int]:
        with self._lock:
            return [int(round(a)) for a in self._angle]

    @property
    def targets(self) -> list[int]:
        with self._lock:
            return self._load_shorts(ADDR_ANGLE_SET, 6)

    @property
    def speeds(self) -> list[int]:
        with self._lock:
            return self._load_shorts(ADDR_SPEED_SET, 6)

    @property
    def force_limits(self) -> list[int]:
        with self._lock:
            return self._load_shorts(ADDR_FORCE_SET, 6)

    @property
    def statuses(self) -> list[int]:
        with self._lock:
            return list(self._status)

    @property
    def errors(self) -> list[int]:
        with self._lock:
            return list(self._error)

    @property
    def forces(self) -> list[int]:
        with self._lock:
            return list(self._force)

    @property
    def calibrating(self) -> bool:
        return time.monotonic() < self._calibrating_until

    def place(self, angles: list[int] | tuple[int, ...], *, retarget: bool = True) -> None:
        """Put the fingers somewhere without moving them there. The hand's record of its targets goes there too (a hand placed at an angle is a hand holding it), unless `retarget` is off."""
        with self._lock:
            self._angle = [float(v) for v in angles]
            self._moving_to = [None] * 6
            if retarget:
                self._store_shorts(ADDR_ANGLE_SET, [int(v) for v in angles])
            self._publish()

    def set_registers(self, address: int, values: list[int]) -> None:
        """Write sixteen-bit registers into the hand's memory, as something other than the driver (another master, a power-on) would have."""
        with self._lock:
            self._store_shorts(address, values)
            self._publish()

    def heal(self) -> None:
        """A hand with nothing wrong: no error, every finger at rest, cool, no object, no calibration, and the line as it should be."""
        with self._lock:
            self._error = [0] * 6
            self._status = [2] * 6
            self._force = [0] * 6
            self._temperature = [31] * 6
            self._moving_to = [None] * 6
            self._calibrating_until = 0.0
            self.blocks = [None] * 6
            self.drop = self.silent = self.garble = self.deaf = self.echo_request = False
            self.delay_s = self.split_gap_s = self.echo_lag_s = 0.0
            self.truncate = self.skip_requests = self.reject_writes = self.noise = self.rest_offset = 0
            self.reply_hand = None
            self.ack_result = 1
            self.split_at = 3
            self.trailer = b""
            self.refuse_connections = False
            self.mute_reads = False
            self.report_angles = self.report_forces = None
            self._publish()

    def set_fault(self, joint: int, error: int = 0, status: int | None = None) -> None:
        """Raise an error in one actuator, as a stalled finger or a hot motor does: the finger stops, the error byte holds until CLEAR_ERROR clears what is clearable, and the status says why it stopped."""
        with self._lock:
            self._error[joint] |= error
            if status is None:
                status = 6 if error & 1 else 5 if error & 4 else 7 if error else self._status[joint]
            self._status[joint] = status
            if error & 2:
                self._temperature[joint] = OVER_TEMPERATURE_C
            if error or status in (5, 6, 7):
                self._moving_to[joint] = None
                self._force[joint] = 0
            self._publish()

    def cool(self, joint: int) -> None:
        """The temperature falls, and the over-temperature error clears by itself (series manual §2.4.3)."""
        with self._lock:
            self._temperature[joint] = 31
            self._error[joint] &= ~0b10
            if not self._error[joint] and self._status[joint] in (5, 6, 7):
                self._status[joint] = 2
            self._publish()

    def hold_status(self, joint: int, status: int) -> None:
        """Make a status byte read as `status` without any motion or error: the maker's table has holes (4, and everything above 7) and this is how a test puts a code into one."""
        with self._lock:
            self._status[joint] = status
            self._moving_to[joint] = None
            self._publish()

    # ── what the hand does with a write ──────────────────────────────────────────────────────────────────

    def _apply_write(self, address: int, data: bytes) -> bool:
        """Take a write into the registers, as far as the manual lets it. False is a write the hand refuses."""
        end = address + len(data)
        if address == ADDR_CLEAR_ERROR and len(data) == 1 and data[0] in (0, 1):
            if data[0] == 1:
                for joint in range(6):
                    self._error[joint] &= ~frames.CLEARABLE_ERROR_BITS
                    if not self._error[joint] and self._status[joint] in (5, 6, 7):
                        self._status[joint] = 2
                self._publish()
            return True
        if address == ADDR_FORCE_CALIBRATION and len(data) == 1 and data[0] in (0, 1):
            if data[0] == 1:
                self._calibration_started = time.monotonic()
                self._calibrating_until = self._calibration_started + CALIBRATION_S / self.time_scale
                self._calibration_from = list(self._angle)
                self._moving_to = [None] * 6
            return True
        if ADDR_ANGLE_SET <= address and end <= ADDR_ANGLE_SET + 12 and len(data) % 2 == 0 and (address - ADDR_ANGLE_SET) % 2 == 0:
            values = frames.shorts(data, signed=True)
            if any(not -1 <= v <= 1000 for v in values):
                return False
            first = (address - ADDR_ANGLE_SET) // 2
            self.target_writes.append((time.monotonic(), tuple(values)))
            if self.deaf or self.calibrating:
                return True
            for offset, value in enumerate(values):
                joint = first + offset
                self._store_shorts(ADDR_ANGLE_SET + 2 * joint, [value])
                if value < 0:
                    continue
                if self._error[joint] or self._status[joint] in (5, 6, 7):
                    continue                    # a stalled finger takes no target until it is cleared
                self._start(joint, float(value))
            # The status of a finger that has just been told to move is "moving" in the same instant the target is recorded: a status that lagged the target by the physics tick showed a driver a target it had been given and
            # the stale "stopped" of the move before it, for as long as a loaded machine took to schedule the tick (a check of a move inside the arrival tolerance failed one run in fifteen that way).
            self._publish()
            return True
        if ADDR_FORCE_SET <= address and end <= ADDR_FORCE_SET + 12 and len(data) % 2 == 0 and (address - ADDR_FORCE_SET) % 2 == 0:
            values = frames.shorts(data, signed=True)
            if any(not 0 <= v <= self.profile.force_set_max_g for v in values):
                return False
            self._memory[address:end] = data
            return True
        if ADDR_SPEED_SET <= address and end <= ADDR_SPEED_SET + 12 and len(data) % 2 == 0 and (address - ADDR_SPEED_SET) % 2 == 0:
            values = frames.shorts(data, signed=True)
            if any(not 0 <= v <= 1000 for v in values):
                return False
            self._memory[address:end] = data
            return True
        return False

    def _start(self, joint: int, target: float) -> None:
        angle = self._angle[joint]
        if target == angle:
            self._moving_to[joint] = None
            self._status[joint] = 2
            self._force[joint] = 0
        else:
            self._moving_to[joint] = target
            self._status[joint] = 1 if target < angle else 0

    def _physics(self) -> None:
        last = time.monotonic()
        while not self._stopping:
            time.sleep(0.004)
            now = time.monotonic()
            dt, last = now - last, now
            with self._lock:
                if now < self._calibrating_until:
                    self._calibrate(now)
                    continue
                if self._calibrating_until:
                    self._calibrating_until = 0.0
                    self._angle = [1000.0] * 6
                    self._status = [2 if not (e or s in (5, 6, 7)) else s for e, s in zip(self._error, self._status)]
                    self._publish()
                    continue
                speeds = self._load_shorts(ADDR_SPEED_SET, 6)
                limits = self._load_shorts(ADDR_FORCE_SET, 6)
                for joint in range(6):
                    target = self._moving_to[joint]
                    if target is None:
                        continue
                    step = speeds[joint] / self.profile.full_range_s * dt * self.time_scale
                    block = self.blocks[joint]
                    # An object stops a finger that closes on it, at a force the hand is allowed (FORCE_SET). It does not stop one that opens from it.
                    if block is not None and target < self._angle[joint] and self._angle[joint] - step <= block <= self._angle[joint]:
                        self._angle[joint] = float(block)
                        self._moving_to[joint] = None
                        self._status[joint] = 3
                        self._force[joint] = limits[joint]
                        continue
                    gap = target - self._angle[joint]
                    if abs(gap) <= step:
                        self._angle[joint] = target - (self.rest_offset if gap > 0 else -self.rest_offset) if self.rest_offset else target
                        self._moving_to[joint] = None
                        self._status[joint] = 2
                        self._force[joint] = 0
                    else:
                        self._angle[joint] += step if gap > 0 else -step
                        self._force[joint] = 0
                self._publish()

    def _calibrate(self, now: float) -> None:
        """The force sensor calibration the manual describes (§2.4.6): five fingers open, four fingers bend and open, the thumb bends and extends. The hand moves by itself and nothing may touch it."""
        progress = (now - self._calibration_started) / (CALIBRATION_S / self.time_scale)
        for joint in range(6):
            lo, hi = (0.0, 0.5) if joint < 4 else (0.5, 1.0) if joint == 4 else (2.0, 3.0)
            if not lo <= progress <= hi:
                continue
            phase = (progress - lo) / (hi - lo)
            self._angle[joint] = 1000.0 * abs(1.0 - 2.0 * phase)
            self._status[joint] = 1 if phase < 0.5 else 0
        self._publish()

    # ── the line ─────────────────────────────────────────────────────────────────────────────────────────

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
            self._publish()
        self._stopping = False
        self._thread = threading.Thread(target=self._serve, daemon=True, name="inspire-sim-line")
        self._physics_thread = threading.Thread(target=self._physics, daemon=True, name="inspire-sim-physics")
        self._thread.start()
        self._physics_thread.start()
        if self._tcp:
            self._listener = socket.socket()
            self._listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            self._listener.bind(("127.0.0.1", 0))
            self._listener.listen(4)
            self._listener.settimeout(0.1)
            self.port = self._listener.getsockname()[1]
            self._tcp_thread = threading.Thread(target=self._accept, daemon=True, name="inspire-sim-tcp")
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
        self._pump(lambda wait: select.select([self._master], [], [], wait)[0], lambda: os.read(self._master, 512), self._write_pty, pty=True)

    def _pump(self, ready, read, write, *, pty: bool = False) -> None:
        """The framing of a request: find `EB 90`, read its length byte, and take that many bytes more. A partial frame that goes quiet is dropped, as a hand's receiver drops it."""
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
                start = pending.find(frames.HEADER_REQUEST)
                if start < 0:
                    pending = pending[-1:] if pending[-1:] == frames.HEADER_REQUEST[:1] else b""
                    break
                pending = pending[start:]
                if len(pending) < 4:
                    break
                total = pending[3] + 5
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
        threading.Thread(target=self._pump, args=(lambda wait: select.select([fd], [], [], wait)[0], lambda: os.read(fd, 512), lambda data: _write_fd(fd, data)), kwargs={"pty": True},
                         daemon=True, name="inspire-sim-attached").start()

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
            threading.Thread(target=self._serve_socket, args=(connection,), daemon=True, name="inspire-sim-conn").start()

    def stop_listening(self) -> None:
        """The gateway's port is closed: a driver that tries to reach it is refused, as it is when the gateway is down."""
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
        self._pump(lambda wait: select.select([connection], [], [], wait)[0], lambda: connection.recv(512), lambda data: _send_all(connection, data))
        connection.close()

    def pull(self) -> None:
        """The adapter is pulled out: the pseudo-terminal's far end goes, and the driver's reads and writes on its own end fail with an OS error."""
        if self._master >= 0:
            try:
                os.close(self._master)
            except OSError:
                pass
            self._master = -1

    def _answer(self, request: bytes, data: bytes, write) -> None:
        if self.delay_s:
            time.sleep(self.delay_s)
        if self.noise:
            return write(b"U" * self.noise)
        if self.trailer:
            data = data + self.trailer
        if self.reply_hand is not None:
            body = bytes([self.reply_hand]) + data[3:-1]
            data = frames.HEADER_RESPONSE + body + bytes([frames.checksum(body)])
        if self.garble:
            data = data[:-1] + bytes([data[-1] ^ 0xFF])
        if self.truncate:
            data = data[:max(0, len(data) - self.truncate)]
        if self.echo_request:
            data = request + data
        if self.split_gap_s and len(data) > self.split_at:
            write(data[:self.split_at])
            time.sleep(self.split_gap_s)
            write(data[self.split_at:])
        else:
            write(data)

    def _handle(self, frame: bytes, write) -> None:
        if len(frame) < 9 or frames.checksum(frame[2:-1]) != frame[-1]:
            return                                              # a corrupt request is not answered
        hand, command = frame[2], frame[4]
        if hand != self.hand or self.drop or (self.mute_reads and command == frames.CMD_READ):
            return
        if self.skip_requests > 0:
            self.skip_requests -= 1
            return
        address = frame[5] | frame[6] << 8
        with self._lock:
            if command == frames.CMD_READ and frame[3] == 4:
                length = frame[7]
                if address + length > MEMORY:
                    return
                self.requests.append((command, address, length))
                body = bytes([hand, length + 3, frames.CMD_READ, address & 0xFF, address >> 8]) + self._read(address, length)
                return self._answer(frame, frames.HEADER_RESPONSE + body + bytes([frames.checksum(body)]), write)
            if command == frames.CMD_WRITE and frame[3] >= 4:
                data = frame[7:-1]
                self.requests.append((command, address, len(data)))
                result = 0
                if self.reject_writes > 0:
                    self.reject_writes -= 1
                elif self._apply_write(address, data):
                    result = self.ack_result
                    self.writes.append((time.monotonic(), address, bytes(data)))
                body = bytes([hand, 4, frames.CMD_WRITE, address & 0xFF, address >> 8, result])
                return self._answer(frame, frames.HEADER_RESPONSE + body + bytes([frames.checksum(body)]), write)

    def _read(self, address: int, length: int) -> bytes:
        memory = self._memory
        feedback_start = frames.ADDR_ANGLE_ACT - 12
        if self.echo_lag_s and self._history:
            cutoff = time.monotonic() - self.echo_lag_s
            earlier = [snapshot for when, snapshot in self._history if when <= cutoff]
            snapshot = earlier[-1] if earlier else self._history[0][1]
            memory = bytearray(self._memory)
            memory[feedback_start:feedback_start + len(snapshot)] = snapshot
        return bytes(memory[address:address + length])
