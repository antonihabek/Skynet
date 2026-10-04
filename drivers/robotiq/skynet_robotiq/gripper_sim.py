"""A simulated Robotiq 2F gripper, the dry substrate: a Modbus RTU slave on a pseudo-terminal, behind the register map the maker's SDK documents.

Written here from the same headers the driver was (`frames.py` cites them), and so **it agrees with whoever wrote the driver** (F15). What it adds to Robotiq's own `makeFakeGripper()` (which `sim/` runs as the
second substrate: activation instant, fingers wherever they were told, no faults) is the part a driver has to survive: an activation that takes time, a motion at the speed it was given, a workpiece that stops
the fingers, the manual's faults, the one-second no-communication fault, and levers for the line (`drop`, `delay_s`, `garble`, `slave`).

    sim = SimulatedGripper().start()      # sim.path is the device a driver opens
"""

from __future__ import annotations

import os
import select
import struct
import threading
import time

from . import frames
from .frames import (ACT, ATR, ARD, GTO, STA_COMPLETE, STA_IN_PROGRESS, STA_RESET, OBJ_AT_POSITION, OBJ_CLOSING, OBJ_MOVING, OBJ_OPENING)

REGISTER_COUNT = frames.STATUS_ADDRESS + 8       # the maker's fake: "one contiguous span, address 0 through the end of the status block"
NO_COMMUNICATION_S = 1.0                          # fault_status.hpp: gFLT 0x09, "no communication during at least 1 second"


class SimulatedGripper:
    def __init__(self, profile: str = "2F-85", slave: int = frames.SLAVE_ADDRESS, *, time_scale: float = 1.0, activation_s: float = 0.4, workpiece: int | None = None) -> None:
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
        self._action = 0
        self._request = (0, 0, 0)            # rPR, rSP, rFR
        self._sta = STA_RESET
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
        self.requests: list[tuple[int, int, int]] = []        # (function, address, quantity) of each request answered
        self.commands: list[frames.Command] = []              # every command block written, in order
        self.exchanges = 0

    # ── the module ───────────────────────────────────────────────────────────────────────────────────────

    def _status_block(self) -> bytes:
        gsta = (self._sta << 4) & 0x30
        gripper_status = (self._action & ACT) | (self._action & GTO) | gsta | (self._obj << 6)
        position = int(round(self._position)) & 0xFF
        return bytes([gripper_status, 0, (self._controller_fault << 4 | self._fault) & 0xFF, self._request[0] if self._action & GTO else self._request[0], position, 0x2A])

    def _publish(self) -> None:
        block = frames.registers_from_block(self._status_block())
        self._registers[frames.STATUS_ADDRESS:frames.STATUS_ADDRESS + 3] = block

    def set_fault(self, gripper: int = 0, controller: int = 0) -> None:
        """Raise a fault the module raises by itself (an over-temperature, an overcurrent): it stays until the rACT falling edge that resets the gripper."""
        with self._lock:
            self._fault, self._controller_fault = gripper & 0x0F, controller & 0x0F
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
        previous = self._action
        self._action = action
        self._request = (position, speed, force)
        self.commands.append(frames.Command(action, position, speed, force))
        now = time.monotonic()
        if not action & ACT:
            # A falling edge resets the gripper and clears its fault; the fingers stay where they are.
            self._sta, self._fault, self._controller_fault, self._moving_to = STA_RESET, 0, 0, None
            self._obj = OBJ_MOVING
        elif not previous & ACT:
            self._sta, self._activation_started = STA_IN_PROGRESS, now
            self._fault = 0
        if self._sta != STA_COMPLETE:
            self._moving_to = None
        elif frames.severity(frames.GRIPPER_FAULTS, self._fault) == "major":
            self._moving_to = None
        elif action & GTO:
            self._moving_to = float(position)
            self._obj = OBJ_MOVING
        else:
            self._moving_to = None
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
                if self._sta == STA_IN_PROGRESS and now - self._activation_started >= self.activation_s / self.time_scale:
                    self._sta = STA_COMPLETE
                    self._position = float(self.profile.open_position)
                    if self._action & GTO:
                        self._moving_to = float(self._request[0])
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
                if self.workpiece is not None and ((target > self._position >= 0 and self._position + step >= self.workpiece and self.workpiece >= self._position) or (target < self._position and self._position - step <= self.workpiece <= self._position)):
                    closing = target > self._position
                    self._position = float(self.workpiece)
                    self._moving_to = None
                    self._obj = OBJ_CLOSING if closing else OBJ_OPENING
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
        return self

    def stop(self) -> None:
        self._stopping = True
        for thread in (self._thread, self._physics_thread):
            if thread is not None:
                thread.join(timeout=2.0)
        for fd in (self._master, self._slave_fd):
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
        pending = b""
        while not self._stopping:
            readable, _, _ = select.select([self._master], [], [], 0.05 if not pending else 0.003)
            if not readable:
                if pending:
                    frame, pending = pending, b""
                    self._handle(frame)
                continue
            try:
                chunk = os.read(self._master, 512)
            except OSError:
                time.sleep(0.01)
                continue
            if self.silent:
                pending = b""
                continue
            pending += chunk

    def _answer(self, data: bytes) -> None:
        if self.delay_s:
            time.sleep(self.delay_s)
        if self.garble:
            data = data[:-1] + bytes([data[-1] ^ 0xFF])
        try:
            os.write(self._master, data)
        except OSError:
            pass

    def _handle(self, frame: bytes) -> None:
        if len(frame) < 4 or frames.crc16(frame[:-2]) != struct.unpack("<H", frame[-2:])[0]:
            return                                            # a corrupt request is not answered, as a Modbus RTU slave does not
        slave, function = frame[0], frame[1]
        if slave != self.slave or self.drop:
            return
        with self._lock:
            self._last_request = time.monotonic()
            if self._fault == 0x09:
                self._fault = 0
                self._publish()
            try:
                if function == frames.FC_READ_HOLDING and len(frame) == 8:
                    address, quantity = struct.unpack(">HH", frame[2:6])
                    data = self._read(address, quantity)
                    if data is None:
                        return self._answer(frames.with_crc(bytes([slave, function | 0x80, 2])))
                    self.requests.append((function, address, quantity))
                    return self._answer(frames.with_crc(bytes([slave, function, len(data)]) + data))
                if function == frames.FC_WRITE_MULTIPLE and len(frame) >= 9:
                    address, quantity, count = struct.unpack(">HHB", frame[2:7])
                    values = frame[7:7 + count]
                    if count != 2 * quantity or len(values) != count or address + quantity > REGISTER_COUNT:
                        return self._answer(frames.with_crc(bytes([slave, function | 0x80, 2 if address + quantity > REGISTER_COUNT else 3])))
                    self.requests.append((function, address, quantity))
                    self._write(address, quantity, values)
                    return self._answer(frames.with_crc(struct.pack(">BBHH", slave, function, address, quantity)))
                if function == frames.FC_READ_WRITE and len(frame) >= 13:
                    read_address, read_quantity, write_address, write_quantity, count = struct.unpack(">HHHHB", frame[2:11])
                    values = frame[11:11 + count]
                    if count != 2 * write_quantity or len(values) != count or read_address + read_quantity > REGISTER_COUNT or write_address + write_quantity > REGISTER_COUNT:
                        return self._answer(frames.with_crc(bytes([slave, function | 0x80, 2])))
                    self.requests.append((function, write_address, write_quantity))
                    self.exchanges += 1
                    self._write(write_address, write_quantity, values)
                    data = self._read(read_address, read_quantity) or b""
                    return self._answer(frames.with_crc(bytes([slave, function, len(data)]) + data))
                self._answer(frames.with_crc(bytes([slave, function | 0x80, 1])))
            except (struct.error, IndexError):
                return

    def _read(self, address: int, quantity: int) -> bytes | None:
        if address + quantity > REGISTER_COUNT or quantity < 1:
            return None
        out = []
        for reg in range(address, address + quantity):
            in_command = frames.COMMAND_ADDRESS <= reg < frames.COMMAND_ADDRESS + 8
            out.append(0 if in_command else self._registers[reg])        # "command-block reads answer zeros and no error, as a real gripper does (bench-verified)"
        return struct.pack(f">{len(out)}H", *out)

    def _write(self, address: int, quantity: int, values: bytes) -> None:
        for i in range(quantity):
            self._registers[address + i] = struct.unpack(">H", values[2 * i:2 * i + 2])[0]
        if address <= frames.COMMAND_ADDRESS and address + quantity >= frames.COMMAND_ADDRESS + frames.BLOCK_REGISTERS:
            start = frames.COMMAND_ADDRESS
            self._apply(frames.block_from_registers(self._registers[start:start + frames.BLOCK_REGISTERS]))
