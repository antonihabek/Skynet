"""Simulated moteus controllers: the dry substrate, answering the way the maker's documentation and firmware say.

**What this is, and what it is not.** It is a counterpart written in this repository (rung 4): it parses the frames a master sends, runs each
controller's mode machine and a one-axis plant, and answers queries, in the maker's own layouts. ``layout_check.py`` holds the wire to the maker's Python
client and its enumerations to the firmware's headers, so the *bytes* are not this file's invention. Its behaviour is: what the firmware does on a
watchdog timeout, on a fault, on a stop, on a command from each mode is modelled here from `fw/bldc_servo_control.h` (the mode transitions in
``ISR_MaybeChangeMode``, read for this file) and `docs/protocol/registers.md`, and is not measured on any controller. An agreement between this model and the
driver is a driver agreeing with its author.

The behaviours it models, each with the place it comes from:

* **A reply only when asked.** A frame whose sender byte has its top bit set is answered, with one reply subframe per read subframe; one without it is acted
  on in silence, queries included (`docs/protocol/can.md`, *CAN ID*).
* **The mode register is written first, and not every value may be written.** Fault (1) and the three preparing modes (2, 3, 4) are the controller's own to
  enter; writing one is a write error (`registers.md`, *0x000*).
* **Stopped clears.** A write of 0 is always taken: a fault, a timeout, anything. It is the only way out of either (`bldc_servo_structs.h`, *kPositionTimeout:
  the only way to exit this state is through a stop command*).
* **A fault ignores everything but a stop.** `ISR_MaybeChangeMode`: from *kFault* a command to any other mode returns without effect.
* **From stopped, a command goes through preparing.** The first non-stop command out of *kStopped* starts calibrating (the ADC offsets); the controller
  reports 2, 3 and 4 for a few cycles and then takes the commanded mode. From *position*, *zero velocity*, *brake* and the rest, it switches at once, with no
  interval in which the driver is off — which is what makes a zero-velocity stop releasable and a stopped one not.
* **Position mode refuses to start outside its bounds.** `servopos.position_min` / `position_max`, unless the command says to ignore them: fault 39
  (*outside limit*), the maker's `kStartOutsideLimit`.
* **The watchdog.** A command carries its own timeout (register 0x027; 0.0 means the controller's configured default, NaN none). When it runs out the
  controller enters *timeout* (11) and applies its fallback; only a stop leaves it.
* **The diagnostic channel.** `conf get <name>` over the tunnelled stream (`diagnostic.md`): one line, the value; `ERR …` for a name it does not hold.

Time is cycles, not seconds: one :meth:`SimulatedMoteus.poll` is one control cycle of ``dt`` seconds for every controller on the bus. That is what makes the
watchdog deterministic — a test advances it by calling :meth:`SimulatedController.step` — and it is why nothing here sleeps.
"""

from __future__ import annotations

import math

from . import protocol as p
from .bus import Bus, FdFrame

__all__ = ["SimulatedController", "SimulatedMoteus"]

#: One control cycle, seconds. Only the watchdog and the plant read it.
_DT = 0.002
#: How many cycles the preparing phase (modes 2, 3, 4) takes; three so a read can see each once, and a poll-per-read driver sees the controller arrive.
_PREPARE_CYCLES = 3
_MAX_TORQUE_DEFAULT = 2.0
_KP = 40.0
_KD = 1.0
_INERTIA = 0.01
_DAMPING = 0.02
_MAX_VELOCITY = 4.0

#: The registers a write may name, and the ones a read may (the rest answer an error subframe, as a controller answers a register it does not have).
_WRITABLE = {p.MODE, *range(p.COMMAND_POSITION, p.COMMAND_ACCEL_LIMIT + 1), 0x02B, 0x02C, 0x02D}
_READABLE = {
    p.MODE, p.POSITION_REG, p.VELOCITY_REG, p.TORQUE_REG, p.Q_CURRENT, p.D_CURRENT, p.ABS_POSITION, p.POWER_REG,
    p.MOTOR_TEMPERATURE, p.TRAJECTORY_COMPLETE, p.HOME_STATE, p.VOLTAGE_REG, p.TEMPERATURE_REG, p.FAULT,
    p.MODEL_NUMBER, p.FIRMWARE_VERSION, p.REGISTER_MAP_VERSION,
}
#: The modes a master may write (`registers.md`: *not all values are valid to write*).
_COMMANDABLE = {p.MODE_STOPPED, p.MODE_PWM, p.MODE_VOLTAGE, p.MODE_VOLTAGE_FOC, p.MODE_VOLTAGE_DQ, p.MODE_CURRENT, p.MODE_POSITION, p.MODE_TIMEOUT,
                p.MODE_ZERO_VELOCITY, p.MODE_STAY_WITHIN, p.MODE_MEASURE_INDUCTANCE, p.MODE_BRAKE}


class SimulatedController:
    """One controller: its registers, its mode machine and a one-axis plant."""

    def __init__(self, ident: int, *, position_min: float = math.nan, position_max: float = math.nan, home_state: int = 2,
                 default_timeout_s: float = 0.1) -> None:
        self.ident = ident
        # configuration the diagnostic channel reads (`conf get`), the controller's own: revolutions of the output shaft
        self.config: dict[str, float] = {
            "servopos.position_min": position_min,
            "servopos.position_max": position_max,
            "servo.default_timeout_s": default_timeout_s,
            "servo.max_velocity": _MAX_VELOCITY,
            "id.id": float(ident),
        }
        # status
        self.mode = p.MODE_STOPPED
        self.fault = 0
        self.position = 0.0
        self.velocity = 0.0
        self.torque = 0.0
        self.home_state = home_state
        self.voltage = 24.0
        self.temperature = 30.0
        #: A function limiting the output (`fw/error.h`: 96 to 105), shown in the fault register while a valid mode runs; a test sets it.
        self.limiting = 0
        # the last command
        self.cmd_position = math.nan
        self.cmd_velocity = 0.0
        self.cmd_torque = 0.0
        self.kp_scale = 1.0
        self.kd_scale = 1.0
        self.max_torque = math.nan
        self.timeout_s = 0.0  # as written: 0.0 means the configured default
        self._control_position = 0.0
        self._timeout_left = math.inf
        self._prepare_left = 0
        self._pending_mode = p.MODE_STOPPED
        self.ignore_bounds = False
        #: Every mode written to this controller, in order, so a test can say what the driver did and not only where the shaft ended.
        self.modes_written: list[int] = []
        #: Every frame addressed to it as raw bytes (after the identifier), for the same reason.
        self.received: list[bytes] = []
        #: Registers a test wants a controller to refuse a *write* of (answered with an error subframe, as a firmware that lacks it would), and to refuse a read of.
        self.refuse: set[int] = set()
        self.refuse_read: set[int] = set()
        #: Replies a test wants lost: the frame is acted on and nothing is said, whatever the sender asked.
        self.mute = False
        #: A controller whose diagnostic channel does not answer (firmware without it): stream subframes are ignored, and a poll gets nothing back.
        self.refuse_diagnostic = False
        #: The most bytes of the diagnostic channel one poll hands back, whatever the client allows: a real controller's answer arrives in pieces when the line is long.
        self.diag_chunk = 255
        #: Configuration names the diagnostic channel answers `ERR` to although the controller has them: a firmware that lacks one of the two bounds' names.
        self.unanswered: set[str] = set()
        # the diagnostic channel
        self._diag_in = b""
        self._diag_out = b""

    # ── the mode machine ────────────────────────────────────────────────────────────────────────────────────────────────

    def _outside_limits(self) -> bool:
        lo, hi = self.config["servopos.position_min"], self.config["servopos.position_max"]
        return (not math.isnan(lo) and self.position < lo) or (not math.isnan(hi) and self.position > hi)

    def _enter(self, requested: int) -> None:
        """`ISR_MaybeChangeMode`, for the modes this model has."""
        if requested == p.MODE_STOPPED:
            self.mode = p.MODE_STOPPED
            self.fault = 0
            self._prepare_left = 0
            self.velocity = 0.0
            self.torque = 0.0
            return
        if requested == self.mode:
            return  # `ISR_DoControl` only changes mode when the command's differs from the status: a command in the mode it is in updates its fields and nothing else
        if self.mode == p.MODE_FAULT or self.mode == p.MODE_TIMEOUT:
            return  # a fault ignores a command; a timeout is left only through a stop
        if self.mode == p.MODE_STOPPED:
            self.mode = p.MODE_ENABLING
            self._prepare_left = _PREPARE_CYCLES
            self._pending_mode = requested
            return
        if self.mode in p.PREPARING_MODES:
            self._pending_mode = requested
            return
        self._take(requested)

    def _take(self, requested: int) -> None:
        if requested == p.MODE_POSITION and not self.ignore_bounds and self._outside_limits():
            self.mode, self.fault = p.MODE_FAULT, 39
            return
        was = self.mode
        self.mode = requested
        if requested == p.MODE_POSITION and was != p.MODE_POSITION:
            self._control_position = self.position

    def inject_fault(self, code: int) -> None:
        """The controller faults, as the firmware does on its own account: torque off, mode 1, the code in the fault register."""
        self.mode, self.fault = p.MODE_FAULT, code
        self.velocity = 0.0
        self.torque = 0.0

    # ── one control cycle ───────────────────────────────────────────────────────────────────────────────────────────────

    def step(self, cycles: int = 1) -> None:
        for _ in range(cycles):
            self._step_once()

    def _step_once(self) -> None:
        if self.mode in p.PREPARING_MODES:
            self._prepare_left -= 1
            self.mode = {3: p.MODE_ENABLING, 2: p.MODE_CALIBRATING, 1: p.MODE_CALIBRATION_COMPLETE}.get(self._prepare_left, self.mode)
            if self._prepare_left <= 0:
                self.mode = p.MODE_STOPPED
                self._take(self._pending_mode)
            return
        if self.mode == p.MODE_POSITION:
            self._timeout_left -= _DT
            if self._timeout_left <= 0.0:
                self.mode = p.MODE_TIMEOUT
            else:
                self._plant(self._position_torque())
                return
        if self.mode in (p.MODE_TIMEOUT, p.MODE_ZERO_VELOCITY):
            # The fallback: a derivative-only controller, which brings velocity to zero and holds nothing against a load.
            self._plant(max(-_MAX_TORQUE_DEFAULT, min(_MAX_TORQUE_DEFAULT, -_KD * self.velocity)))
            return
        if self.mode == p.MODE_BRAKE:
            self._plant(-_KD * self.velocity * 0.5)
            return
        # stopped, fault or anything this model does not run: the shaft coasts
        self.torque = 0.0
        self.velocity *= 1.0 - _DAMPING
        self.position += self.velocity * _DT

    def _position_torque(self) -> float:
        # position NaN means "use the current position" (registers.md, 0x020), taken once when the mode was entered; a velocity advances it
        if not math.isnan(self.cmd_position):
            self._control_position = self.cmd_position
        self._control_position += (0.0 if math.isnan(self.cmd_velocity) else self.cmd_velocity) * _DT
        lo, hi = self.config["servopos.position_min"], self.config["servopos.position_max"]
        if not self.ignore_bounds:
            if not math.isnan(lo):
                self._control_position = max(self._control_position, lo)
            if not math.isnan(hi):
                self._control_position = min(self._control_position, hi)
        ceiling = _MAX_TORQUE_DEFAULT if math.isnan(self.max_torque) else self.max_torque
        torque = _KP * self.kp_scale * (self._control_position - self.position) - _KD * self.kd_scale * self.velocity + self.cmd_torque
        return max(-ceiling, min(ceiling, torque))

    def _plant(self, torque: float) -> None:
        self.torque = torque
        self.velocity += (torque - _DAMPING * self.velocity) / _INERTIA * _DT
        self.velocity = max(-_MAX_VELOCITY, min(_MAX_VELOCITY, self.velocity))
        self.position += self.velocity * _DT

    # ── a frame ─────────────────────────────────────────────────────────────────────────────────────────────────────────

    def handle(self, data: bytes, reply_wanted: bool, source: int) -> FdFrame | None:
        """Act on one frame's payload; the reply (or None) is what goes back on the wire."""
        self.received.append(bytes(data))
        try:
            subframes = p.parse_frame(data)
        except ValueError:
            return None  # a controller ignores what it cannot parse
        reply = b""
        # reads are answered after the writes of the same frame have been applied (the maker's own client writes a command and asks for the state it made)
        writes = [s for s in subframes if isinstance(s, p.Register) and s.kind == "write"]
        reads = [s for s in subframes if isinstance(s, p.Register) and s.kind == "read"]
        streams = [s for s in subframes if isinstance(s, p.StreamData)]
        errors = b""
        mode_write: int | None = None
        # A command is one frame that opens with the mode (`registers.md`, *0x000*), and a field it does not write is the firmware's default, not what the last
        # command said: a fresh `CommandData` each time. Command registers in a frame with no mode write are not a command and are dropped.
        has_mode = any(item.register == p.MODE and item.register not in self.refuse for item in writes)
        if has_mode:
            self._reset_command()
        for item in writes:
            if item.register in self.refuse or item.register not in _WRITABLE:
                errors += p.error_subframe("write", item.register, 1)
                continue
            if item.register != p.MODE and not has_mode:
                continue
            value = p.decode(item.value, item.resolution, _scales_of(item.register))  # type: ignore[arg-type]
            if item.register == p.MODE:
                mode = int(item.value)  # type: ignore[arg-type]
                if mode not in _COMMANDABLE:
                    errors += p.error_subframe("write", item.register, 2)
                    continue
                mode_write = mode
                self.modes_written.append(mode)
            elif item.register == p.COMMAND_POSITION:
                self.cmd_position = value
            elif item.register == p.COMMAND_VELOCITY:
                self.cmd_velocity = value
            elif item.register == p.COMMAND_FEEDFORWARD_TORQUE:
                self.cmd_torque = 0.0 if math.isnan(value) else value
            elif item.register == p.COMMAND_KP_SCALE:
                self.kp_scale = 1.0 if math.isnan(value) else value
            elif item.register == p.COMMAND_KD_SCALE:
                self.kd_scale = 1.0 if math.isnan(value) else value
            elif item.register == p.COMMAND_MAX_TORQUE:
                self.max_torque = value
            elif item.register == p.COMMAND_WATCHDOG:
                self.timeout_s = value
            elif item.register == 0x02D:
                self.ignore_bounds = bool(value)
        if mode_write is not None:
            self._arm_watchdog()
            self._enter(mode_write)
        if self.refuse_diagnostic:
            streams = []
        for item in streams:
            if item.kind == "client" and item.channel == 1:
                self._diag_in += item.data
                self._diag_run()
        want_poll = [s for s in streams if s.kind == "poll"]
        # replies
        values: dict[int, tuple[int, float | int]] = {}
        read_errors = b""
        for item in reads:
            if item.register in self.refuse_read or item.register not in _READABLE:
                read_errors += p.error_subframe("read", item.register, 1)
                continue
            values[item.register] = (item.resolution, self._register(item.register, item.resolution))
        run: list[tuple[int, int, float | int]] = []
        for item in reads:
            if item.register in values:
                run.append((item.register, item.resolution, values[item.register][1]))
        i = 0
        while i < len(run):
            start, resolution, first = run[i]
            group = [first]
            j = i + 1
            while j < len(run) and run[j][1] == resolution and run[j][0] == start + len(group):
                group.append(run[j][2])
                j += 1
            reply += p.reply_registers(start, resolution, group)
            i = j
        reply += read_errors + errors
        for item in want_poll:
            longest = min(item.data[0] if item.data else 0, self.diag_chunk)
            chunk, self._diag_out = self._diag_out[:longest], self._diag_out[longest:]
            reply += p.stream_server_data(item.channel, chunk)
        if not reply_wanted or self.mute or not reply:
            return None
        ident = (self.ident << 8) | (source & 0x7F)
        return FdFrame(ident, p.pad(reply), extended=ident > 0x7FF)

    def _reset_command(self) -> None:
        """The defaults of a field a command does not write (`registers.md`: position, velocity and feedforward 0.0, the scales 1.0, the ceiling and the stop
        position NaN, the watchdog 0.0 meaning the configured default)."""
        self.cmd_position = 0.0
        self.cmd_velocity = 0.0
        self.cmd_torque = 0.0
        self.kp_scale = 1.0
        self.kd_scale = 1.0
        self.max_torque = math.nan
        self.timeout_s = 0.0
        self.ignore_bounds = False

    def _arm_watchdog(self) -> None:
        # 0.0 means the controller's configured default; NaN means none; anything else is the command's own (`registers.md`, 0x027)
        timeout = self.timeout_s
        if timeout == 0.0:
            timeout = self.config["servo.default_timeout_s"]
        self._timeout_left = math.inf if math.isnan(timeout) else timeout

    def _register(self, register: int, resolution: int) -> float | int:
        if register == p.MODE:
            return self.mode
        if register == p.POSITION_REG:
            return p.encode(self.position, resolution, p.POSITION)
        if register == p.VELOCITY_REG:
            return p.encode(self.velocity, resolution, p.VELOCITY)
        if register == p.TORQUE_REG:
            return p.encode(self.torque, resolution, p.TORQUE)
        if register == p.HOME_STATE:
            return self.home_state
        if register == p.VOLTAGE_REG:
            return p.encode(self.voltage, resolution, p.VOLTAGE)
        if register == p.TEMPERATURE_REG:
            return p.encode(self.temperature, resolution, p.TEMPERATURE)
        if register == p.FAULT:
            if self.mode == p.MODE_FAULT:
                return self.fault
            return self.limiting if self.mode == p.MODE_POSITION else 0
        if register == p.MODEL_NUMBER:
            return 0x0000_0001
        if register == p.FIRMWARE_VERSION:
            return 0x0001_0801
        if register == p.REGISTER_MAP_VERSION:
            return 1
        return 0 if resolution != p.F32 else 0.0

    # ── the diagnostic channel ──────────────────────────────────────────────────────────────────────────────────────────

    def _diag_run(self) -> None:
        while b"\n" in self._diag_in:
            line, self._diag_in = self._diag_in.split(b"\n", 1)
            words = line.decode("latin1").strip().split()
            if len(words) == 3 and words[:2] == ["conf", "get"]:
                name = words[2]
                if name in self.config and name not in self.unanswered:
                    value = self.config[name]
                    text = "nan" if math.isnan(value) else repr(float(value))
                    self._diag_out += f"{text}\r\n".encode("latin1")
                else:
                    self._diag_out += b"ERR error reading\r\n"
            else:
                self._diag_out += b"ERR unknown command\r\n"


def _scales_of(register: int) -> tuple[float, float, float]:
    if register in (p.COMMAND_POSITION, p.COMMAND_STOP_POSITION):
        return p.POSITION
    if register in (p.COMMAND_VELOCITY, p.COMMAND_VELOCITY_LIMIT):
        return p.VELOCITY
    if register == p.COMMAND_ACCEL_LIMIT:
        return p.ACCEL
    if register in (p.COMMAND_FEEDFORWARD_TORQUE, p.COMMAND_MAX_TORQUE):
        return p.TORQUE
    if register in (p.COMMAND_KP_SCALE, p.COMMAND_KD_SCALE, 0x02B):
        return p.PWM
    if register == p.COMMAND_WATCHDOG:
        return p.TIME
    return (1.0, 1.0, 1.0)


class SimulatedMoteus:
    """Several controllers on one bus endpoint. ``poll()`` is the whole life of all of them: the frames waiting are handled, every controller takes one cycle,
    and the replies go out."""

    def __init__(self, bus: Bus, ids: list[int] | None = None, **controller_args) -> None:
        self.bus = bus
        self.controllers: dict[int, SimulatedController] = {i: SimulatedController(i, **controller_args) for i in (ids or [1])}
        self.powered = True
        #: Frames whose destination no controller here has: counted, because on a real bus they are somebody else's traffic.
        self.foreign = 0
        self.cycles = 0

    def poll(self) -> None:
        if not self.powered:
            while self.bus.recv() is not None:
                pass
            return
        replies: list[FdFrame] = []
        while True:
            frame = self.bus.recv()
            if frame is None:
                break
            source, destination, wanted = p.split_id(frame.arbitration_id) if frame.arbitration_id <= 0xFFFF else (0, 0, False)
            controller = self.controllers.get(destination)
            if controller is None:
                self.foreign += 1
                continue
            reply = controller.handle(bytes(frame.data), wanted, source)
            if reply is not None:
                replies.append(reply)
        for controller in self.controllers.values():
            controller.step()
        self.cycles += 1
        for reply in replies:
            self.bus.send(reply)
