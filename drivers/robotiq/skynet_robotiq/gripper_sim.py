"""A simulated Robotiq 2F gripper, the dry substrate: a Modbus RTU slave on a pseudo-terminal, behind the register map the maker's SDK documents.

Written here from the same headers the driver was (`frames.py` cites them), and so **it agrees with whoever wrote the driver** (F15). What it adds to Robotiq's own `makeFakeGripper()` (which `sim/` runs as the
second substrate: activation instant, fingers wherever they were told, no faults) is the part a driver has to survive: an activation that takes time, a motion at the speed it was given, a workpiece that stops
the fingers, the manual's faults, the one-second no-communication fault, and levers for the line (`drop`, `delay_s`, `garble`, `slave`).

    sim = SimulatedGripper().start()      # sim.path is the device a driver opens
"""

from __future__ import annotations

import os
import select
import socket
import struct
import threading
import time

from . import frames
from .frames import (ACT, ATR, ARD, GTO, STA_COMPLETE, STA_IN_PROGRESS, STA_RESET, OBJ_AT_POSITION, OBJ_CLOSING, OBJ_MOVING, OBJ_OPENING)

REGISTER_COUNT = frames.STATUS_ADDRESS + 8       # the maker's fake: "one contiguous span, address 0 through the end of the status block"
NO_COMMUNICATION_S = 1.0                          # fault_status.hpp: gFLT 0x09, "no communication during at least 1 second"


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
    def __init__(self, profile: str = "2F-85", slave: int = frames.SLAVE_ADDRESS, *, time_scale: float = 1.0, activation_s: float = 0.4, workpiece: int | None = None, activated: bool = False, tcp: bool = False) -> None:
        self.profile = frames.PROFILES[profile]
        self.slave = slave
        self.time_scale = time_scale
        self.activation_s = activation_s
        #: A workpiece between the fingers, as the register the fingers stop at (gPO), or None.
        self.workpiece = workpiece
        self.path = ""
        self._master = -1
        self._slave_fd = -1
        self._lock = threading.RLock()
        self._registers = [0] * REGISTER_COUNT
        self._position = float(self.profile.open_position)
        #: Started activated, as a gripper is that was activated before the host (re)connected: "activation survives com loss" (the maker's `activate`).
        self._action = ACT if activated else 0
        self._request = (self.profile.open_position if activated else 0, 0, 0)            # rPR, rSP, rFR
        self._sta = STA_COMPLETE if activated else STA_RESET
        self._activation_started = 0.0
        self._obj = OBJ_MOVING
        self._fault = 0                       # gFLT
        self._controller_fault = 0            # kFLT
        self._last_request = time.monotonic()
        self._moving_to: float | None = None
        self._stopping = False
        self._thread: threading.Thread | None = None
        self._physics_thread: threading.Thread | None = None
        #: What a test turns on.
        self.drop = False                     # requests are read and not answered: a gripper that has gone
        self.delay_s = 0.0                    # every answer waits
        self.garble = False                   # every answer's CRC is wrong
        self.silent = False                   # the line is dead: nothing is read, nothing answered
        self.split_gap_s = 0.0                # an answer comes in two pieces with this long between them (a slow USB adapter)
        self.exception_code = 0               # every request is answered with this Modbus exception instead
        self.reply_slave: int | None = None   # every answer says it is from this slave
        self.truncate = 0                     # every answer is this many bytes short
        self.port = 0                         # the TCP port, when `tcp` is on: a gateway that forwards the line raw
        self._tcp = tcp
        self._listener: socket.socket | None = None
        self._tcp_thread: threading.Thread | None = None
        self.command_times: list[float] = []
        self.connections = 0                  # TCP connections accepted, so far
        self.refuse = False                   # a gateway that is up and takes no connection: each is closed at once
        self._open_connections: list[socket.socket] = []
        self._attached: list[int] = []
        self._raised_warning = False          # the warning in `_fault` is one this module raised itself (and takes down itself), and not one a test set
        self.skip_requests = 0                # the next this-many requests are read and not answered: a lost frame, and not a gripper that has gone
        self.echo_lag_s = 0.0                 # the status block shows the gripper as it was this long ago: a firmware cycle between a command and its echo
        self.reset_lag_s = 0.0                # after rACT falls, gACT and gSTA go on saying "activated" for this long
        self.blocks_opening: int | None = None   # an obstacle at this register stops the fingers when they open against it (gOBJ: an object detected while opening)
        self._history: list[tuple[float, list[int]]] = []
        self._stale_byte = 0
        self._stale_until = 0.0
        self.deaf = False                     # a command is answered on the wire and not taken: the echoes (gGTO, gPR) never change
        self.requests: list[tuple[int, int, int]] = []        # (function, address, quantity) of each request answered
        self.commands: list[frames.Command] = []              # every command block written, in order
        self.exchanges = 0

    # ── the module ───────────────────────────────────────────────────────────────────────────────────────

    def _status_block(self) -> bytes:
        gsta = (self._sta << 4) & 0x30
        gripper_status = (self._action & ACT) | (self._action & GTO) | gsta | (self._obj << 6)
        if self._stale_until and time.monotonic() < self._stale_until:
            gripper_status = self._stale_byte
        position = int(round(self._position)) & 0xFF
        return bytes([gripper_status, 0, (self._controller_fault << 4 | self._fault) & 0xFF, self._request[0], position, 0x2A])

    def _publish(self) -> None:
        block = frames.registers_from_block(self._status_block())
        self._registers[frames.STATUS_ADDRESS:frames.STATUS_ADDRESS + 3] = block
        now = time.monotonic()
        self._history.append((now, block))
        del self._history[:-400]

    def set_fault(self, gripper: int = 0, controller: int = 0) -> None:
        """Raise a fault the module raises by itself (an over-temperature, an overcurrent): it stays until the rACT falling edge that resets the gripper."""
        with self._lock:
            self._fault, self._controller_fault, self._raised_warning = gripper & 0x0F, controller & 0x0F, False
            if gripper and frames.severity(frames.GRIPPER_FAULTS, gripper) == "major":
                self._moving_to = None
            self._publish()

    def place(self, register: int) -> None:
        with self._lock:
            self._position = float(register)
            self._moving_to = None
            self._publish()

    @property
    def position(self) -> int:
        with self._lock:
            return int(round(self._position))

    @property
    def fault(self) -> int:
        return self._fault

    @property
    def activation_state(self) -> int:
        return self._sta

    @property
    def go_to(self) -> bool:
        return bool(self._action & GTO)

    def _apply(self, block: bytes) -> None:
        """What the module does with a command block (the maker's `RegisterModel::processCommand`, with the time the fake leaves out)."""
        action, _, _, position, speed, force = block
        self.commands.append(frames.Command(action, position, speed, force))
        self.command_times.append(time.monotonic())
        if self.deaf:
            return
        previous = self._action
        previous_position = self._request[0]
        status_before = self._status_block()[0]
        self._action = action
        self._request = (position, speed, force)
        now = time.monotonic()
        if not action & ACT:
            # A falling edge resets the gripper and clears its fault; the fingers stay where they are.
            if self.reset_lag_s and previous & ACT:
                self._stale_byte, self._stale_until = status_before, now + self.reset_lag_s
            self._sta, self._fault, self._controller_fault, self._moving_to = STA_RESET, 0, 0, None
            self._obj = OBJ_MOVING
        elif not previous & ACT:
            self._sta, self._activation_started = STA_IN_PROGRESS, now
            self._fault = 0
        # The two warnings of an action that cannot be taken yet (fault_status.hpp): rGTO without rACT, and rGTO while the activation has not finished. They clear on their own, with the next command that has no such request.
        if action & GTO and not action & ACT:
            self._fault, self._raised_warning = 0x07, True
        elif action & GTO and self._sta != STA_COMPLETE:
            self._fault, self._raised_warning = 0x05, True
        elif self._raised_warning and self._fault in (0x05, 0x07):
            self._fault, self._raised_warning = 0, False
        if self._sta != STA_COMPLETE:
            self._moving_to = None
        elif frames.severity(frames.GRIPPER_FAULTS, self._fault) == "major":
            self._moving_to = None
        elif action & GTO:
            # The command is sent again in every exchange (the maker's SDK sends its whole image each cycle), so the same go-to is not a new motion: only a request that was not there, or a different target, starts one.
            if not previous & GTO or position != previous_position:
                self._moving_to = float(position)
                self._obj = OBJ_MOVING
        else:
            self._moving_to = None
            self._obj = OBJ_MOVING      # the maker's gOBJ 0: "fingers moving, or motion not yet requested"
        if action & ATR and self._sta == STA_COMPLETE:
            self._fault = 0x0B
            self._moving_to = 0.0 if action & ARD else float(self.profile.closed_position)
        self._publish()

    def _physics(self) -> None:
        last = time.monotonic()
        while not self._stopping:
            time.sleep(0.004)
            now = time.monotonic()
            dt, last = now - last, now
            with self._lock:
                if self._sta == STA_IN_PROGRESS:
                    # The calibration sweep the maker's SDK warns about: the fingers close and open again through their full range, and only then does the gripper say it is activated.
                    progress = (now - self._activation_started) / (self.activation_s / self.time_scale)
                    open_, closed = float(self.profile.open_position), float(self.profile.closed_position)
                    if progress >= 1.0:
                        self._sta = STA_COMPLETE
                        self._position = open_
                        if self._action & GTO:
                            self._moving_to = float(self._request[0])
                    else:
                        self._position = open_ + (closed - open_) * (2 * progress if progress < 0.5 else 2 - 2 * progress)
                    self._publish()
                    continue
                if self._stale_until and now >= self._stale_until:
                    self._stale_until = 0.0
                    self._publish()
                if now - self._last_request > NO_COMMUNICATION_S and self._fault == 0 and self._sta != STA_RESET:
                    self._fault = 0x09
                    self._publish()
                target = self._moving_to
                if target is None:
                    continue
                speed_register = self._request[1]
                metres_per_second = self.profile.min_speed + (self.profile.max_speed - self.profile.min_speed) * speed_register / 255.0
                registers_per_metre = self.profile.register_range / self.profile.opening_range
                step = metres_per_second * registers_per_metre * dt * self.time_scale
                # A workpiece between the fingers stops them when they close on it. It does not stop them opening: they open from it.
                if self.workpiece is not None and target > self._position and self._position < self.workpiece <= self._position + step:
                    self._position = float(self.workpiece)
                    self._moving_to = None
                    self._obj = OBJ_CLOSING
                    self._publish()
                    continue
                if self.blocks_opening is not None and target < self._position and self._position - step <= self.blocks_opening < self._position:
                    self._position = float(self.blocks_opening)
                    self._moving_to = None
                    self._obj = OBJ_OPENING
                    self._publish()
                    continue
                gap = target - self._position
                if abs(gap) <= step:
                    self._position = target
                    self._moving_to = None
                    self._obj = OBJ_AT_POSITION
                    if self._action & ATR:
                        self._fault = 0x0F
                else:
                    self._position += step if gap > 0 else -step
                self._publish()

    # ── the line ─────────────────────────────────────────────────────────────────────────────────────────

    def start(self) -> "SimulatedGripper":
        import termios
        self._master, self._slave_fd = os.openpty()
        self.path = os.ttyname(self._slave_fd)
        attributes = termios.tcgetattr(self._slave_fd)
        attributes[0] = attributes[1] = attributes[3] = 0
        attributes[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
        attributes[6][termios.VMIN] = 0
        attributes[6][termios.VTIME] = 0
        termios.tcsetattr(self._slave_fd, termios.TCSANOW, attributes)
        self._publish()
        self._stopping = False
        self._thread = threading.Thread(target=self._serve, daemon=True, name="robotiq-sim-line")
        self._physics_thread = threading.Thread(target=self._physics, daemon=True, name="robotiq-sim-physics")
        self._thread.start()
        self._physics_thread.start()
        if self._tcp:
            self._listener = socket.socket()
            self._listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            self._listener.bind(("127.0.0.1", 0))
            self._listener.listen(4)
            self._listener.settimeout(0.1)
            self.port = self._listener.getsockname()[1]
            self._tcp_thread = threading.Thread(target=self._accept, daemon=True, name="robotiq-sim-tcp")
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

    def __enter__(self) -> "SimulatedGripper":
        return self.start()

    def __exit__(self, *exc: object) -> None:
        self.stop()

    def _serve(self) -> None:
        self._pump(lambda wait: select.select([self._master], [], [], wait)[0], lambda: os.read(self._master, 512), self._write_pty, quiet=0.003, pty=True)

    def _pump(self, ready, read, write, *, quiet: float, pty: bool = False) -> None:
        """The framing of a Modbus RTU slave: what has arrived, once the line has been quiet for a few milliseconds, is one request."""
        pending = b""
        while not self._stopping:
            if not ready(quiet if pending else 0.05):
                if pending:
                    frame, pending = pending, b""
                    self._handle(frame, write)
                continue
            try:
                chunk = read()
            except (OSError, ConnectionError):
                if pty:
                    time.sleep(0.01)
                    continue
                return
            if chunk == b"" and not pty:
                return                                         # the gateway's peer closed
            if self.silent:
                pending = b""
                continue
            pending += chunk

    def _write_pty(self, data: bytes) -> None:
        try:
            os.write(self._master, data)
        except OSError:
            pass

    def attach(self, path: str) -> None:
        """Serve the gripper on a serial device that already exists: the substrate's end of a fault-injecting relay (`tools/faultproxy`), which makes the pair of pseudo-terminals and cannot let the substrate make its own."""
        import termios
        fd = os.open(path, os.O_RDWR | os.O_NOCTTY)
        attributes = termios.tcgetattr(fd)
        attributes[0] = attributes[1] = attributes[3] = 0
        attributes[2] = termios.CS8 | termios.CREAD | termios.CLOCAL
        attributes[6][termios.VMIN] = 0
        attributes[6][termios.VTIME] = 0
        termios.tcsetattr(fd, termios.TCSANOW, attributes)
        self._attached.append(fd)
        threading.Thread(target=self._pump, args=(lambda wait: select.select([fd], [], [], wait)[0], lambda: os.read(fd, 512), lambda data: _write_fd(fd, data)), kwargs={"quiet": 0.003, "pty": True},
                         daemon=True, name="robotiq-sim-attached").start()

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
            if self.refuse:
                connection.close()
                continue
            self._open_connections.append(connection)
            threading.Thread(target=self._serve_socket, args=(connection,), daemon=True, name="robotiq-sim-conn").start()

    def go_down(self) -> None:
        """The gateway stops listening (its address is still the same, and nothing answers at it): a connection is refused, and `come_up` restores it."""
        if self._listener is not None:
            self._listener.close()
            self._listener = None
        self.hang_up()

    def come_up(self) -> None:
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
        self._pump(lambda wait: select.select([connection], [], [], wait)[0], lambda: connection.recv(512), lambda data: _send_all(connection, data), quiet=0.003)
        connection.close()

    def _answer(self, data: bytes, write) -> None:
        if self.delay_s:
            time.sleep(self.delay_s)
        if self.reply_slave is not None:
            data = frames.with_crc(bytes([self.reply_slave]) + data[1:-2])
        if self.garble:
            data = data[:-1] + bytes([data[-1] ^ 0xFF])
        if self.truncate:
            data = data[:max(0, len(data) - self.truncate)]
        if self.split_gap_s and len(data) > 3:
            write(data[:3])
            time.sleep(self.split_gap_s)
            write(data[3:])
        else:
            write(data)

    def _handle(self, frame: bytes, write) -> None:
        if len(frame) < 4 or frames.crc16(frame[:-2]) != struct.unpack("<H", frame[-2:])[0]:
            return                                            # a corrupt request is not answered, as a Modbus RTU slave does not
        slave, function = frame[0], frame[1]
        if slave != self.slave or self.drop:
            return
        if self.skip_requests > 0:
            self.skip_requests -= 1
            return
        with self._lock:
            self._last_request = time.monotonic()
            if self._fault == 0x09:
                self._fault = 0
                self._publish()
            if self.exception_code:
                return self._answer(frames.with_crc(bytes([slave, function | 0x80, self.exception_code])), write)
            try:
                if function == frames.FC_READ_HOLDING and len(frame) == 8:
                    address, quantity = struct.unpack(">HH", frame[2:6])
                    data = self._read(address, quantity)
                    if data is None:
                        return self._answer(frames.with_crc(bytes([slave, function | 0x80, 2])), write)
                    self.requests.append((function, address, quantity))
                    return self._answer(frames.with_crc(bytes([slave, function, len(data)]) + data), write)
                if function == frames.FC_WRITE_MULTIPLE and len(frame) >= 9:
                    address, quantity, count = struct.unpack(">HHB", frame[2:7])
                    values = frame[7:7 + count]
                    if count != 2 * quantity or len(values) != count or address + quantity > REGISTER_COUNT:
                        return self._answer(frames.with_crc(bytes([slave, function | 0x80, 2 if address + quantity > REGISTER_COUNT else 3])), write)
                    self.requests.append((function, address, quantity))
                    self._write(address, quantity, values)
                    return self._answer(frames.with_crc(struct.pack(">BBHH", slave, function, address, quantity)), write)
                if function == frames.FC_READ_WRITE and len(frame) >= 13:
                    read_address, read_quantity, write_address, write_quantity, count = struct.unpack(">HHHHB", frame[2:11])
                    values = frame[11:11 + count]
                    if count != 2 * write_quantity or len(values) != count or read_address + read_quantity > REGISTER_COUNT or write_address + write_quantity > REGISTER_COUNT:
                        return self._answer(frames.with_crc(bytes([slave, function | 0x80, 2])), write)
                    self.requests.append((function, write_address, write_quantity))
                    self.exchanges += 1
                    self._write(write_address, write_quantity, values)
                    data = self._read(read_address, read_quantity) or b""
                    return self._answer(frames.with_crc(bytes([slave, function, len(data)]) + data), write)
                self._answer(frames.with_crc(bytes([slave, function | 0x80, 1])), write)
            except (struct.error, IndexError):
                return

    def _read(self, address: int, quantity: int) -> bytes | None:
        if address + quantity > REGISTER_COUNT or quantity < 1:
            return None
        out = []
        shown = self._registers[frames.STATUS_ADDRESS:frames.STATUS_ADDRESS + 3]
        if self.echo_lag_s and self._history:
            cutoff = time.monotonic() - self.echo_lag_s
            earlier = [block for when, block in self._history if when <= cutoff]
            shown = earlier[-1] if earlier else self._history[0][1]
        for reg in range(address, address + quantity):
            in_command = frames.COMMAND_ADDRESS <= reg < frames.COMMAND_ADDRESS + 8
            in_status = frames.STATUS_ADDRESS <= reg < frames.STATUS_ADDRESS + 3
            out.append(0 if in_command else shown[reg - frames.STATUS_ADDRESS] if in_status else self._registers[reg])        # "command-block reads answer zeros and no error, as a real gripper does (bench-verified)"
        return struct.pack(f">{len(out)}H", *out)

    def _write(self, address: int, quantity: int, values: bytes) -> None:
        for i in range(quantity):
            self._registers[address + i] = struct.unpack(">H", values[2 * i:2 * i + 2])[0]
        if address <= frames.COMMAND_ADDRESS and address + quantity >= frames.COMMAND_ADDRESS + frames.BLOCK_REGISTERS:
            start = frames.COMMAND_ADDRESS
            self._apply(frames.block_from_registers(self._registers[start:start + frames.BLOCK_REGISTERS]))
