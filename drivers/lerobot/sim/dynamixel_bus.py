"""A bus of Dynamixel X-series servos behind a pseudo-terminal — the dry substrate for LeRobot's own Koch and OpenMANIPULATOR-X (item 12.D, 12.E.5).

The Feetech half (`feetech_bus.py`) put LeRobot's own SO-101 on a wire. This is the other half: a serial bus that `lerobot.robots.koch_follower` and
`lerobot.robots.omx_follower` can open, with ROBOTIS's own Python SDK (`dynamixel_sdk`, protocol 2.0) and LeRobot's own `DynamixelMotorsBus` on this side
of the pty and an emulated XL430 / XL330 on the other. Nothing of LeRobot or the SDK is patched, mocked or imported by this file: they run as shipped, and
the only thing they are talking to is a file descriptor.

# What is the maker's and what is ours

* **The control table** (addresses, widths, model numbers, operating modes, baud codes, resolution) is read out of LeRobot's own
  `lerobot/motors/dynamixel/tables.py`, which cites ROBOTIS's e-manual, rather than retyped here. A register this file uses that the table does not name
  is an error, not a guess.
* **The packet format** is protocol 2.0: `FF FF FD 00 id len_l len_h instruction params… crc_l crc_h`, a CRC-16 (polynomial 0x8005, initial 0, no reflection)
  over everything before it, and byte stuffing (a `FF FF FD` inside the parameters is followed by one more `FD`; the length counts the stuffed bytes). A
  reply is `FF FF FD 00 id len_l len_h 55 error params… crc_l crc_h`. The CRC is checked against the SDK's own table in `test_dynamixel_bus.py`.
* **The error byte** is the e-manual's: bits 0–6 the error number (1 result fail, 2 instruction error, 3 CRC error, 4 data range, 5 data length, 6 data
  limit, 7 access), bit 7 the hardware-alert flag.
* **Ours** is everything a servo *does*: a position loop with a profile velocity, torque enable, the EEPROM lock while torque is on, the homing offset
  (`Present_Position = Actual_Position + Homing_Offset`, the opposite sign of Feetech's), the registers that cannot be written, the shutdown a hardware
  error causes. It is written from the e-manual's description and from reading what LeRobot expects, not measured on a servo. **This is a counterpart
  written here, not independent evidence** (rung 4, `docs/robot-support.md` §3): it shows the bridge drives the library as the library is meant to be
  driven, and that a wrong byte on the wire is a failed check; it does not show the arm moves.

# What it can be made to do wrong

`silence(id)`, `drop(n)`, `corrupt(n)`, `fault(id, bits)`, `stall(id)` and `push(id, steps)` exist so a test can ask the questions a bench asks: a servo that
stops answering, a reply with a bad CRC, one that reports an overload and shuts its torque off, one that does not follow its goal, one that is moved by
hand. Every one is off by default.
"""

from __future__ import annotations

import os
import pty
import select
import sys
import threading
import time
import tty
from pathlib import Path
from typing import Any

__all__ = ["DynamixelBus", "Servo", "maker_tables", "crc16", "stuff", "unstuff"]

HEADER = b"\xff\xff\xfd\x00"
BROADCAST = 0xFE
INST_PING, INST_READ, INST_WRITE, INST_REG_WRITE, INST_ACTION = 0x01, 0x02, 0x03, 0x04, 0x05
INST_FACTORY_RESET, INST_REBOOT, INST_CLEAR = 0x06, 0x08, 0x10
INST_SYNC_READ, INST_SYNC_WRITE, INST_BULK_READ, INST_BULK_WRITE = 0x82, 0x83, 0x92, 0x93
INST_STATUS = 0x55

#: The e-manual's error numbers (bits 0-6 of a status packet's error byte) and its alert flag (bit 7).
ERR_RESULT_FAIL, ERR_INSTRUCTION, ERR_CRC, ERR_DATA_RANGE, ERR_DATA_LENGTH, ERR_DATA_LIMIT, ERR_ACCESS = 1, 2, 3, 4, 5, 6, 7
ALERT = 0x80

#: Hardware_Error_Status bits (X-series e-manual).
HW_INPUT_VOLTAGE, HW_OVERHEATING, HW_MOTOR_ENCODER, HW_ELECTRICAL_SHOCK, HW_OVERLOAD = 0x01, 0x04, 0x08, 0x10, 0x20

TICK = 0.005
#: One unit of Velocity_Limit and Profile_Velocity is 0.229 rev/min (e-manual); 4096 steps a turn.
STEPS_PER_SECOND_PER_UNIT = 0.229 / 60.0 * 4096.0
#: One unit of Profile_Acceleration is 214.577 rev/min².
STEPS_PER_SECOND2_PER_UNIT = 214.577 / 3600.0 * 4096.0


def maker_tables() -> dict[str, Any]:
    """LeRobot's Dynamixel `tables.py`, loaded by path so that importing it does not import the rest of LeRobot.

    Returns the module's globals. Raises with the places it looked when LeRobot is not installed: this emulator has no table of its own.
    """
    looked = []
    for entry in sys.path:
        candidate = Path(entry or ".") / "lerobot" / "motors" / "dynamixel" / "tables.py"
        looked.append(str(candidate))
        if candidate.is_file():
            namespace: dict[str, Any] = {"__name__": "lerobot_dynamixel_tables"}
            exec(compile(candidate.read_text(encoding="utf-8"), str(candidate), "exec"), namespace)  # noqa: S102 — the maker's own constants file
            return namespace
    raise RuntimeError("LeRobot is not installed, so there is no Dynamixel control table to emulate: looked for " + ", ".join(looked[:4]))


def crc16(data: bytes) -> int:
    """Protocol 2.0's CRC: polynomial 0x8005, initial value 0, MSB first, over every byte before the CRC (stuffing included)."""
    crc = 0
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x8005) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def stuff(params: bytes) -> bytes:
    """Byte stuffing: `FF FF FD` inside the parameters would read as a header, so a `FD` is inserted after it."""
    out = bytearray()
    for byte in params:
        out.append(byte)
        if byte == 0xFD and out[-3:-1] == b"\xff\xff":
            out.append(0xFD)
    return bytes(out)


def unstuff(params: bytes) -> bytes:
    out = bytearray()
    index = 0
    while index < len(params):
        out.append(params[index])
        if params[index] == 0xFD and out[-3:-1] == b"\xff\xff" and index + 1 < len(params) and params[index + 1] == 0xFD:
            index += 1
        index += 1
    return bytes(out)


def _twos(value: int, size: int) -> int:
    return value & ((1 << (8 * size)) - 1)


def _signed(raw: int, size: int) -> int:
    return raw - (1 << (8 * size)) if raw & (1 << (8 * size - 1)) else raw


class Servo:
    """One X-series servo on the bus: a register file and a position loop."""

    #: Operating modes (the e-manual's numbers; LeRobot's `OperatingMode`).
    CURRENT, VELOCITY, POSITION, EXTENDED_POSITION, CURRENT_POSITION, PWM = 0, 1, 3, 4, 5, 16

    def __init__(self, ident: int, tables: dict[str, Any], model: str) -> None:
        self.id = ident
        self.model = model
        self.table: dict[str, tuple[int, int]] = tables["MODEL_CONTROL_TABLE"][model]
        self.signed: set[str] = set(tables["MODEL_ENCODING_TABLE"][model])
        self.resolution: int = tables["MODEL_RESOLUTION"][model]
        self.modes: list[int] = tables["MODEL_OPERATING_MODES"][model]
        self.regs = bytearray(256)
        #: The shaft, in raw encoder steps, before the homing offset (a float: the loop is continuous, and in extended mode it does not wrap).
        self.actual = float(self.resolution // 2)
        self.velocity = 0.0
        self.silent = False
        self.stalled = False
        self.stuck_torque = False  # a servo that does not let go: a write that would turn its torque off is acknowledged and not made
        self.pending: tuple[int, bytes] | None = None  # REG_WRITE, until an ACTION
        self.error_status = 0  # Hardware_Error_Status
        self._poke("Model_Number", tables["MODEL_NUMBER_TABLE"][model])
        self._poke("Firmware_Version", 52)
        self._poke("ID", ident)
        self._poke("Baud_Rate", 3)  # 1 Mbps, which is LeRobot's default
        self._poke("Return_Delay_Time", 250)  # the factory default `configure_motors` lowers to 0
        self._poke("Operating_Mode", self.POSITION)
        self._poke("Protocol_Type", 2)
        self._poke("Temperature_Limit", 80)
        self._poke("Max_Voltage_Limit", 160)
        self._poke("Min_Voltage_Limit", 95)
        self._poke("Velocity_Limit", 265)
        self._poke("Max_Position_Limit", self.resolution - 1)
        self._poke("Min_Position_Limit", 0)
        self._poke("Shutdown", 0x34)  # overheating, electrical shock, overload: the factory default
        self._poke("Status_Return_Level", 2)
        self._poke("Position_P_Gain", 800)
        self._poke("Position_D_Gain", 0)
        self._poke("Position_I_Gain", 0)
        self._poke("Present_Input_Voltage", 120)  # 12.0 V, in 0.1 V
        self._poke("Present_Temperature", 30)
        self._sync_registers()
        self.goal = self.reported
        self._poke("Goal_Position", int(self.goal))

    # ── registers ──

    def _span(self, name: str) -> tuple[int, int]:
        if name not in self.table:
            raise KeyError(f"{name!r} is not in LeRobot's Dynamixel control table")
        return self.table[name]

    def _poke(self, name: str, value: int) -> None:
        address, size = self._span(name)
        self.regs[address:address + size] = _twos(int(value), size).to_bytes(size, "little")

    def peek(self, name: str) -> int:
        """A register by its maker's name, decoded the way LeRobot decodes it (two's complement where its encoding table says so)."""
        address, size = self._span(name)
        raw = int.from_bytes(self.regs[address:address + size], "little")
        return _signed(raw, size) if name in self.signed else raw

    # ── the loop ──

    @property
    def reported(self) -> float:
        """`Present_Position = Actual_Position + Homing_Offset` (LeRobot's `_get_half_turn_homings`). One turn wide in position mode, unbounded in extended."""
        value = self.actual + self.peek("Homing_Offset")
        return value % self.resolution if self.peek("Operating_Mode") == self.POSITION else value

    @property
    def moving(self) -> bool:
        return self.peek("Torque_Enable") == 1 and abs(self.velocity) > 1.0

    def _sync_registers(self) -> None:
        self._poke("Present_Position", int(round(self.reported)))
        self._poke("Present_Velocity", int(round(self.velocity / STEPS_PER_SECOND_PER_UNIT)))
        self._poke("Moving", 1 if self.moving else 0)
        self._poke("Hardware_Error_Status", self.error_status)
        self._poke("Realtime_Tick", int(time.monotonic() * 1000) % 32768)

    def raise_hardware_error(self, bits: int) -> None:
        """A hardware error: the status's alert flag, and the torque off if the Shutdown register names one of these bits (the factory default does)."""
        self.error_status |= bits
        if bits & self.peek("Shutdown"):
            self._poke("Torque_Enable", 0)
            self.velocity = 0.0

    def step(self, dt: float) -> None:
        mode = self.peek("Operating_Mode")
        if self.peek("Torque_Enable") == 1 and not self.stalled and mode in (self.POSITION, self.EXTENDED_POSITION, self.CURRENT_POSITION):
            goal = self.peek("Goal_Position")
            if mode == self.POSITION:
                low, high = self.peek("Min_Position_Limit"), self.peek("Max_Position_Limit")
                goal = min(max(goal, low), high)
            target = goal - self.peek("Homing_Offset")
            if mode == self.POSITION:
                target = target % self.resolution + (self.actual // self.resolution) * self.resolution
            profile = self.peek("Profile_Velocity")
            limit = (profile or self.peek("Velocity_Limit")) * STEPS_PER_SECOND_PER_UNIT
            wanted = max(-limit, min(limit, 20.0 * (target - self.actual)))
            ramp = self.peek("Profile_Acceleration") * STEPS_PER_SECOND2_PER_UNIT or 1e6
            change = max(-ramp * dt, min(ramp * dt, wanted - self.velocity))
            self.velocity += change
            self.actual += self.velocity * dt
        elif self.peek("Torque_Enable") == 1 and not self.stalled and mode == self.VELOCITY:
            wanted = self.peek("Goal_Velocity") * STEPS_PER_SECOND_PER_UNIT
            self.velocity = wanted
            self.actual += self.velocity * dt
        else:
            self.velocity = 0.0
        self._sync_registers()

    def write(self, address: int, data: bytes) -> int:
        """A WRITE. Returns the error number (0 for none), writing nothing when there is one — as the servo does.

        The e-manual's rules that LeRobot's own flow relies on: the EEPROM area (below `Torque_Enable`) is read-only while torque is on (an access error),
        the registers from `Realtime_Tick` up and the model number, model information and firmware version are read-only, and a value outside a
        register's range (an operating mode the model does not have, a torque enable other than 0 and 1) is a data-range error.
        """
        if address < 0 or address + len(data) > 148:
            return ERR_DATA_RANGE
        torque = self.regs[self.table["Torque_Enable"][0]]
        if address < self.table["Torque_Enable"][0] and torque == 1:
            return ERR_ACCESS
        if address < self.table["ID"][0] or address >= self.table["Realtime_Tick"][0]:
            return ERR_ACCESS
        staged = bytearray(self.regs)
        staged[address:address + len(data)] = data
        mode_at = self.table["Operating_Mode"][0]
        if address <= mode_at < address + len(data) and staged[mode_at] not in self.modes:
            return ERR_DATA_RANGE
        torque_at = self.table["Torque_Enable"][0]
        if address <= torque_at < address + len(data) and staged[torque_at] > 1:
            return ERR_DATA_RANGE
        if self.stuck_torque and address <= torque_at < address + len(data) and staged[torque_at] == 0:
            data = data[:torque_at - address] + bytes([self.regs[torque_at]]) + data[torque_at - address + 1:]
        self.regs[address:address + len(data)] = data
        if address <= self.table["ID"][0] < address + len(data):
            self.id = self.regs[self.table["ID"][0]]
        return 0

    def reboot(self) -> None:
        """RAM returns to its power-up values; the EEPROM stays. The torque is off and the hardware error is cleared."""
        self.error_status = 0
        self.velocity = 0.0
        for name in ("Torque_Enable", "LED", "Registered_Instruction", "Goal_PWM", "Goal_Current", "Goal_Velocity", "Profile_Acceleration", "Profile_Velocity"):
            self._poke(name, 0)
        self._sync_registers()
        self.goal = self.reported
        self._poke("Goal_Position", int(self.goal))


class DynamixelBus:
    """N servos on one pty. `start()` returns the path to hand to `DynamixelMotorsBus(port=…)`."""

    def __init__(self, models: dict[int, str] | None = None) -> None:
        self._tables = maker_tables()
        models = models or {1: "xl430-w250", 2: "xl430-w250", 3: "xl330-m288", 4: "xl330-m288", 5: "xl330-m288", 6: "xl330-m288"}  # Koch's six
        self.servos: dict[int, Servo] = {ident: Servo(ident, self._tables, model) for ident, model in models.items()}
        self.packets: list[tuple[int, int, bytes]] = []  # (id, instruction, params) of every well-formed frame received
        self.crc_failures = 0
        self.bytes_stuffed = 0  # replies whose parameters needed a stuffed byte
        self._drop = 0
        self._corrupt = 0
        self._master = -1
        self._slave = -1
        self.path = ""
        self._quit = threading.Event()
        self._threads: list[threading.Thread] = []
        self._lock = threading.RLock()

    # ── lifecycle ──

    def start(self) -> str:
        master, slave = pty.openpty()
        tty.setraw(master)
        tty.setraw(slave)
        self._master, self._slave = master, slave
        self.path = os.ttyname(slave)
        for target in (self._serve, self._physics):
            thread = threading.Thread(target=target, daemon=True)
            thread.start()
            self._threads.append(thread)
        return self.path

    def stop(self) -> None:
        self._quit.set()
        for thread in self._threads:
            thread.join(timeout=1.0)
        for fd in (self._master, self._slave):
            if fd >= 0:
                try:
                    os.close(fd)
                except OSError:
                    pass

    def __enter__(self) -> "DynamixelBus":
        self.start()
        return self

    def __exit__(self, *_: object) -> None:
        self.stop()

    # ── what a test may do to it ──

    def servo(self, ident: int) -> Servo:
        return self.servos[ident]

    def calibrate(self, ident: int, *, homing_offset: int, range_min: int, range_max: int) -> None:
        """Leave a servo as a calibrated arm's would be: offset and travel limits in EEPROM."""
        servo = self.servos[ident]
        with self._lock:
            servo._poke("Homing_Offset", homing_offset)
            servo._poke("Min_Position_Limit", range_min)
            servo._poke("Max_Position_Limit", range_max)
            servo._sync_registers()
            servo.goal = servo.reported
            servo._poke("Goal_Position", int(servo.goal))

    def silence(self, ident: int, silent: bool = True) -> None:
        self.servos[ident].silent = silent

    def drop(self, count: int) -> None:
        """The next `count` replies are never sent."""
        self._drop = count

    def corrupt(self, count: int) -> None:
        """The next `count` replies go out with a wrong CRC."""
        self._corrupt = count

    def fault(self, ident: int, bits: int) -> None:
        """A hardware error (a mix of `HW_*`): the alert flag on every reply, and the torque off if the Shutdown register names the bit."""
        with self._lock:
            self.servos[ident].raise_hardware_error(bits)

    def stall(self, ident: int, stalled: bool = True) -> None:
        """The servo stops following its goal — a blocked joint. It still answers."""
        self.servos[ident].stalled = stalled

    def push(self, ident: int, steps: float) -> None:
        """Move the shaft by hand."""
        with self._lock:
            self.servos[ident].actual += steps

    # ── the wire ──

    def _serve(self) -> None:
        buffer = bytearray()
        while not self._quit.is_set():
            ready, _, _ = select.select([self._master], [], [], TICK)
            if not ready:
                continue
            try:
                chunk = os.read(self._master, 4096)
            except OSError:
                return
            if not chunk:
                continue
            buffer += chunk
            while True:
                frame = self._take(buffer)
                if frame is None:
                    break
                with self._lock:
                    self._handle(*frame)

    def _take(self, buffer: bytearray) -> tuple[int, int, bytes] | None:
        """One well-formed instruction packet off the front of `buffer`, or `None` while it is incomplete. Returns `(id, instruction, params)`, unstuffed."""
        while True:
            start = buffer.find(HEADER)
            if start < 0:
                del buffer[:max(0, len(buffer) - 3)]
                return None
            del buffer[:start]
            if len(buffer) < 7:
                return None
            length = buffer[5] | (buffer[6] << 8)
            if length < 3:
                del buffer[:4]
                continue
            if len(buffer) < 7 + length:
                return None
            body = bytes(buffer[:7 + length - 2])  # header, id, length, instruction and parameters: everything the CRC covers
            crc = buffer[7 + length - 2] | (buffer[7 + length - 1] << 8)
            del buffer[:7 + length]
            if crc16(body) != crc:
                self.crc_failures += 1
                continue
            return body[4], body[7], unstuff(body[8:])

    def _handle(self, ident: int, instruction: int, params: bytes) -> None:
        self.packets.append((ident, instruction, params))
        if instruction == INST_SYNC_WRITE:
            if len(params) < 4:
                return
            address, length = params[0] | (params[1] << 8), params[2] | (params[3] << 8)
            rest = params[4:]
            for index in range(0, len(rest) - length, length + 1):
                target = rest[index]
                if target in self.servos:
                    self.servos[target].write(address, rest[index + 1:index + 1 + length])
            return
        if instruction == INST_SYNC_READ:
            if len(params) < 4:
                return
            address, length = params[0] | (params[1] << 8), params[2] | (params[3] << 8)
            for target in params[4:]:
                self._reply_read(target, address, length)
            return
        targets = list(self.servos) if ident == BROADCAST else [ident]
        for target in targets:
            servo = self.servos.get(target)
            if servo is None:
                continue
            level = servo.regs[servo.table["Status_Return_Level"][0]]
            if instruction == INST_PING:
                self._reply(servo, bytes([servo.regs[0], servo.regs[1], servo.regs[servo.table["Firmware_Version"][0]]]))
            elif instruction == INST_READ:
                if len(params) == 4 and level >= 1:
                    self._reply_read(target, params[0] | (params[1] << 8), params[2] | (params[3] << 8))
            elif instruction == INST_WRITE:
                error = servo.write(params[0] | (params[1] << 8), params[2:]) if len(params) >= 2 else ERR_DATA_LENGTH
                if ident != BROADCAST and level >= 2:
                    self._reply(servo, b"", error=error)
            elif instruction == INST_REG_WRITE:
                servo.pending = (params[0] | (params[1] << 8), params[2:])
                if ident != BROADCAST and level >= 2:
                    self._reply(servo, b"")
            elif instruction == INST_ACTION:
                if servo.pending is not None:
                    servo.write(*servo.pending)
                    servo.pending = None
                if ident != BROADCAST and level >= 2:
                    self._reply(servo, b"")
            elif instruction == INST_REBOOT:
                servo.reboot()
                if ident != BROADCAST and level >= 2:
                    self._reply(servo, b"")
            elif ident != BROADCAST:
                self._reply(servo, b"", error=ERR_INSTRUCTION)  # an instruction this emulator does not implement

    def _reply_read(self, ident: int, address: int, length: int) -> None:
        servo = self.servos.get(ident)
        if servo is None:
            return
        if address + length > len(servo.regs) or length == 0:
            self._reply(servo, b"", error=ERR_DATA_RANGE)
            return
        # The position registers are refreshed on the loop's tick; a read is answered from the shaft as it is now, so a write that moves what
        # Present_Position means (the homing offset) or a hand on the shaft shows in the very next read, not one tick later.
        servo._sync_registers()
        self._reply(servo, bytes(servo.regs[address:address + length]))

    def _reply(self, servo: Servo, data: bytes, error: int = 0) -> None:
        if servo.silent:
            return
        if self._drop > 0:
            self._drop -= 1
            return
        stuffed = stuff(data)
        if stuffed != data:
            self.bytes_stuffed += 1
        alert = ALERT if servo.error_status else 0
        body = HEADER + bytes([servo.id]) + (len(stuffed) + 4).to_bytes(2, "little") + bytes([INST_STATUS, error | alert]) + stuffed
        crc = crc16(body)
        if self._corrupt > 0:
            self._corrupt -= 1
            crc ^= 0xFFFF
        try:
            os.write(self._master, body + crc.to_bytes(2, "little"))
        except OSError:
            pass

    def _physics(self) -> None:
        last = time.monotonic()
        while not self._quit.is_set():
            time.sleep(TICK)
            now = time.monotonic()
            dt, last = now - last, now
            with self._lock:
                for servo in self.servos.values():
                    servo.step(dt)
