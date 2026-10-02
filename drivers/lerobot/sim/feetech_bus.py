"""A bus of Feetech STS servos behind a pseudo-terminal — the dry substrate for LeRobot's own SO-100/101 (item 12.D, 12.E.5).

`skynet_lerobot` was written against a fake `Robot` (`skynet_lerobot.fake`) and "has been exercised against a faithful
fake and **not** against LeRobot itself". This is the other half: a serial bus a real `lerobot.robots.so_follower.SO101Follower`
can open, with Feetech's own Python SDK (`scservo_sdk`) and LeRobot's own `FeetechMotorsBus` on this side of the pty and an
emulated STS3215 on the other. Nothing of LeRobot or the SDK is patched, mocked or imported by this file: they run as shipped, and
the only thing they are talking to is a file descriptor.

# What is the maker's and what is ours

* **The control table** (addresses, widths, sign-magnitude bits, model numbers, baud codes) is read out of LeRobot's own
  `lerobot/motors/feetech/tables.py` — the file its bus is written from, which in turn cites Feetech's manual — rather than retyped
  here. A register this file uses that the table does not name is an error, not a guess.
* **The error bits** in a status packet are `scservo_sdk`'s own constants (`ERRBIT_*`).
* **The packet format** is Feetech's protocol 0: `FF FF id length instruction params… checksum`, the checksum the bitwise
  complement of the low byte of the sum from `id` to the last parameter. A reply is `FF FF id length error params… checksum`.
* **Ours** is everything a servo *does*: a position loop with a speed and an acceleration limit, torque enable, the EEPROM lock,
  travel limits, the homing offset, sign-magnitude on the registers that carry it. It is written from the manual's description and
  from reading what LeRobot expects, not measured on a servo. **This is a counterpart written here, not independent evidence** (rung 4,
  `docs/robot-support.md` §3): it shows the bridge drives the library as the library is meant to be driven, and that a wrong byte on
  the wire is a failed check; it does not show the arm moves.

# What it can be made to do wrong

`silence(id)`, `drop(n)`, `corrupt(n)`, `fault(id, bits)`, `stall(id)` and `push(id, steps)` exist so a test can ask the questions a
bench asks: a servo that stops answering, a reply with a bad checksum, one that reports an overload, one that does not follow its
goal, one that is moved by hand. Every one is off by default.
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

__all__ = ["FeetechBus", "Servo", "maker_tables"]

HEADER = b"\xff\xff"
BROADCAST = 0xFE
INST_PING, INST_READ, INST_WRITE, INST_REG_WRITE, INST_ACTION = 0x01, 0x02, 0x03, 0x04, 0x05
INST_SYNC_READ, INST_SYNC_WRITE = 0x82, 0x83

#: Feetech STS3215, nominal. Speeds are in steps (1/4096 of a turn) per second.
MAX_SPEED = 3000.0
DEFAULT_ACCELERATION_UNIT = 100.0  # steps/s² per count of the Acceleration register
TICK = 0.005


def maker_tables() -> dict[str, Any]:
    """LeRobot's `tables.py`, loaded by path so that importing it does not import the rest of LeRobot.

    Returns the module's globals. Raises with the places it looked when LeRobot is not installed: this emulator has no table of its own.
    """
    looked = []
    for entry in sys.path:
        candidate = Path(entry or ".") / "lerobot" / "motors" / "feetech" / "tables.py"
        looked.append(str(candidate))
        if candidate.is_file():
            namespace: dict[str, Any] = {"__name__": "lerobot_feetech_tables"}
            exec(compile(candidate.read_text(encoding="utf-8"), str(candidate), "exec"), namespace)  # noqa: S102 — the maker's own constants file
            return namespace
    raise RuntimeError("LeRobot is not installed, so there is no Feetech control table to emulate: looked for " + ", ".join(looked[:4]))


def _error_bits() -> dict[str, int]:
    """`scservo_sdk`'s own names for the bits of a status packet's error byte."""
    import scservo_sdk as scs  # the maker's SDK; imported here so the module loads without it

    return {name[len("ERRBIT_"):].lower(): getattr(scs, name) for name in dir(scs) if name.startswith("ERRBIT_")}


def _sign_magnitude(value: int, bit: int) -> int:
    """Encode a signed value as Feetech's sign-magnitude, `bit` being the sign bit."""
    return ((1 << bit) | -value) if value < 0 else value


def _from_sign_magnitude(raw: int, bit: int) -> int:
    return -(raw & ((1 << bit) - 1)) if raw & (1 << bit) else raw


class Servo:
    """One STS3215 on the bus: a register file and a position loop."""

    def __init__(self, ident: int, tables: dict[str, Any], model: str = "sts3215") -> None:
        self.id = ident
        self.table: dict[str, tuple[int, int]] = tables["STS_SMS_SERIES_CONTROL_TABLE"]
        self.signs: dict[str, int] = tables["STS_SMS_SERIES_ENCODINGS_TABLE"]
        self.resolution: int = tables["MODEL_RESOLUTION"][model]
        self.regs = bytearray(256)
        #: The shaft, in raw encoder steps, before the homing offset (a float: the loop is continuous).
        self.actual = float(self.resolution // 2)
        self.velocity = 0.0
        self.error_bits = 0
        self.silent = False
        self.stalled = False
        self._poke_int("Model_Number", tables["MODEL_NUMBER_TABLE"][model])
        self._poke_int("Firmware_Major_Version", 3)
        self._poke_int("Firmware_Minor_Version", 10)
        self._poke_int("ID", ident)
        self._poke_int("Baud_Rate", 0)
        self._poke_int("Return_Delay_Time", 250)  # the factory default LeRobot's `configure_motors` lowers to 0
        self._poke_int("Response_Status_Level", 1)
        self._poke_int("Min_Position_Limit", 0)
        self._poke_int("Max_Position_Limit", self.resolution - 1)
        self._poke_int("Max_Temperature_Limit", 70)
        self._poke_int("Max_Voltage_Limit", 140)
        self._poke_int("Min_Voltage_Limit", 40)
        self._poke_int("Max_Torque_Limit", 1000)
        self._poke_int("Phase", 0x1C)  # bit 4 set, as shipped: `configure_motors` clears it
        self._poke_int("P_Coefficient", 32)
        self._poke_int("D_Coefficient", 32)
        self._poke_int("I_Coefficient", 0)
        self._poke_int("Protection_Current", 500)
        self._poke_int("Overload_Torque", 80)
        self._poke_int("Maximum_Acceleration", 254)
        self._poke_int("Acceleration", 0)
        self._poke_int("Torque_Limit", 1000)
        self._poke_int("Present_Voltage", 120)  # 12.0 V
        self._poke_int("Present_Temperature", 30)
        self._sync_registers()
        self.goal = self.reported  # a servo that has just powered up goes nowhere
        self._poke_int("Goal_Position", int(self.goal))

    # ── registers ──

    def _span(self, name: str) -> tuple[int, int]:
        if name not in self.table:
            raise KeyError(f"{name!r} is not in LeRobot's Feetech control table")
        return self.table[name]

    def _poke_int(self, name: str, value: int) -> None:
        address, size = self._span(name)
        if name in self.signs:
            value = _sign_magnitude(value, self.signs[name])
        self.regs[address:address + size] = int(value).to_bytes(size, "little")

    def peek(self, name: str) -> int:
        """A register by its maker's name, decoded the way LeRobot decodes it."""
        address, size = self._span(name)
        raw = int.from_bytes(self.regs[address:address + size], "little")
        return _from_sign_magnitude(raw, self.signs[name]) if name in self.signs else raw

    # ── the loop ──

    @property
    def reported(self) -> float:
        """`Present_Position = Actual_Position − Homing_Offset` (LeRobot's `_get_half_turn_homings`), wrapped into one turn."""
        return (self.actual - self.peek("Homing_Offset")) % self.resolution

    def _sync_registers(self) -> None:
        self._poke_int("Present_Position", int(round(self.reported)) % self.resolution)
        self._poke_int("Present_Velocity", int(round(self.velocity)))
        self._poke_int("Moving", 1 if self.moving else 0)

    @property
    def moving(self) -> bool:
        return self.peek("Torque_Enable") == 1 and abs(self.velocity) > 1.0

    def step(self, dt: float) -> None:
        if self.peek("Torque_Enable") == 1 and not self.stalled:
            low, high = self.peek("Min_Position_Limit"), self.peek("Max_Position_Limit")
            goal = self.peek("Goal_Position")
            if high > low:
                goal = min(max(goal, low), high)
            target = goal + self.peek("Homing_Offset")
            speed = self.peek("Goal_Velocity")
            limit = abs(speed) if speed else MAX_SPEED
            wanted = max(-limit, min(limit, 20.0 * (target - self.actual)))
            acceleration = (self.peek("Acceleration") or 254) * DEFAULT_ACCELERATION_UNIT
            change = max(-acceleration * dt, min(acceleration * dt, wanted - self.velocity))
            self.velocity += change
            self.actual += self.velocity * dt
        else:
            self.velocity *= 0.0
        self._sync_registers()

    def write(self, address: int, data: bytes) -> None:
        """A WRITE: EEPROM (below 40) is ignored while `Lock` is 1 — acknowledged and dropped, as on the servo."""
        if address + len(data) > len(self.regs):
            return
        lock_address = self.table["Lock"][0]
        for offset, byte in enumerate(data):
            target = address + offset
            if target < self.table["Torque_Enable"][0] and self.regs[lock_address] == 1 and target != lock_address:
                continue
            self.regs[target] = byte
        if address <= self.table["ID"][0] < address + len(data):
            self.id = self.regs[self.table["ID"][0]]


class FeetechBus:
    """N servos on one pty. `start()` returns the path to hand to `FeetechMotorsBus(port=…)`."""

    def __init__(self, ids: tuple[int, ...] = (1, 2, 3, 4, 5, 6)) -> None:
        self._tables = maker_tables()
        self.servos: dict[int, Servo] = {i: Servo(i, self._tables) for i in ids}
        self.errors = _error_bits()
        self.packets: list[tuple[int, int, bytes]] = []  # (id, instruction, params) of every well-formed frame received
        self.checksum_failures = 0
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

    def __enter__(self) -> "FeetechBus":
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
            servo._poke_int("Homing_Offset", homing_offset)
            servo._poke_int("Min_Position_Limit", range_min)
            servo._poke_int("Max_Position_Limit", range_max)
            servo.goal = servo.reported
            servo._poke_int("Goal_Position", int(servo.goal))

    def silence(self, ident: int, silent: bool = True) -> None:
        self.servos[ident].silent = silent

    def drop(self, count: int) -> None:
        """The next `count` replies are never sent."""
        self._drop = count

    def corrupt(self, count: int) -> None:
        """The next `count` replies go out with a wrong checksum."""
        self._corrupt = count

    def fault(self, ident: int, bits: int) -> None:
        """Put `bits` (a mix of `errors[...]`) in the error byte of every reply this servo makes."""
        self.servos[ident].error_bits = bits

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
        """One well-formed instruction packet off the front of `buffer`, or `None` while it is incomplete."""
        while True:
            start = buffer.find(HEADER)
            if start < 0:
                del buffer[:max(0, len(buffer) - 1)]
                return None
            del buffer[:start]
            if len(buffer) < 4:
                return None
            length = buffer[3]
            if length < 2:
                del buffer[:2]
                continue
            if len(buffer) < length + 4:
                return None
            body = bytes(buffer[2:length + 3])  # id, length, instruction, params…
            checksum = buffer[length + 3]
            del buffer[:length + 4]
            if (~sum(body) & 0xFF) != checksum:
                self.checksum_failures += 1
                continue
            return body[0], body[2], body[3:]

    def _handle(self, ident: int, instruction: int, params: bytes) -> None:
        self.packets.append((ident, instruction, params))
        if instruction == INST_SYNC_WRITE:
            address, length = params[0], params[1]
            rest = params[2:]
            for index in range(0, len(rest) - length, length + 1):
                target = rest[index]
                if target in self.servos:
                    self.servos[target].write(address, rest[index + 1:index + 1 + length])
            return
        if instruction == INST_SYNC_READ:
            address, length = params[0], params[1]
            for target in params[2:]:
                self._reply_read(target, address, length)
            return
        targets = list(self.servos) if ident == BROADCAST else [ident]
        for target in targets:
            servo = self.servos.get(target)
            if servo is None:
                continue
            if instruction == INST_PING:
                self._reply(servo, b"")
            elif instruction == INST_READ:
                self._reply_read(target, params[0], params[1])
            elif instruction == INST_WRITE:
                servo.write(params[0], params[1:])
                if ident != BROADCAST and servo.regs[self.servos[target].table["Response_Status_Level"][0]] == 1:
                    self._reply(servo, b"")
            elif ident != BROADCAST:
                self._reply(servo, b"", error=0x40)  # an instruction this emulator does not implement

    def _reply_read(self, ident: int, address: int, length: int) -> None:
        servo = self.servos.get(ident)
        if servo is None:
            return
        if address + length > len(servo.regs):
            self._reply(servo, b"", error=0x40)
            return
        # The position registers are refreshed on the loop's tick; a read is answered from the shaft as it is now, so a write that
        # moves what Present_Position means (the homing offset) or a hand on the shaft shows in the very next read, not one tick later.
        servo._sync_registers()
        self._reply(servo, bytes(servo.regs[address:address + length]))

    def _reply(self, servo: Servo, data: bytes, error: int = 0) -> None:
        if servo.silent:
            return
        if self._drop > 0:
            self._drop -= 1
            return
        body = bytes([servo.id, len(data) + 2, error | servo.error_bits]) + data
        checksum = ~sum(body) & 0xFF
        if self._corrupt > 0:
            self._corrupt -= 1
            checksum ^= 0xFF
        try:
            os.write(self._master, HEADER + body + bytes([checksum]))
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
