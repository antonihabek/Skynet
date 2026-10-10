"""The RHAL driver: an OnRobot RG2, RG6, 2FG7 or 2FG14 gripper over Modbus TCP (the Compute Box) or Modbus RTU (the tool's own RS-485) (DRY-COVERAGE-PLAN.md item 12.E.8).

§4's five mandatory entry points over :mod:`skynet_onrobot.frames`, :mod:`skynet_onrobot.modbus` and :mod:`skynet_onrobot.link`. It links nothing of the maker's: OnRobot publish a closed client library (`libonrobot_tool_api`,
BSD-3-Clause, release 1.2.0) and no register table, so the registers were read off that library by running it (`frames.py`), and `layout_check.py` holds this driver's bytes to it by running it again.

# The body

One prismatic joint, **`jaw`**: the opening between the fingertips in metres. For an RG it is the maker's *task aperture*, the width at the fitted fingertips (fingertip offset included); for a 2FG it is the external
width. The installation fixes the useful range, which the library makes the integrator say for an RG (`RgMotionLimits`) and reads off the device for a 2FG, so `limits` is required for an RG and may only narrow a 2FG's.
It takes a position, one control mode, and its tier ceiling is Guest (§31.3).

# What a command is

A grip: a target width and a force, and for a 2FG a speed. The force is the operator's (`force_n`, required: nothing in a position request says how hard to close) and so is a 2FG's speed (`speed`, percent); an RG has no
speed command. Every command is one write of the control block, validated here as the maker's library validates it before it writes, and rounded as it rounds. A 2FG's supply budget (`supply_power_w`, 14 to 48 W) is
told to the gripper at open and verified, and told again whenever the line has been dead: the maker's firmware gives it back to 14 W at every power cycle, and the force ceiling goes with it.

# The stop

`stop_mode` is required and accepts only `stop`: the maker's own stop (an RG's control word 40, a 2FG's type 3), the only one the library offers. **What it does to a held workpiece is the maker's and was not measured** (its
documentation says to support the workpiece before stopping or releasing a grip). `emergency_stop` latches and returns, and the reading thread writes the stop, because the RHAL's "never blocks" is 50 ms and a line that is
slow or dead would make a stop that wrote its own control block wait for it. It is a **software stop**: the gripper's own safety circuit is a switch, and there is none on this wire.
"""

from __future__ import annotations

import contextlib
import json
import math
import sys
import threading
import time

import skynet_rhal as rhal

from . import frames, link, modbus
from .frames import MODELS

__all__ = ["BridgeRefusal", "OnRobotBridge", "TOKEN"]

TOKEN = "vendor.onrobot.task"
JOINT = "jaw"

_RATE_HZ = 20.0
_MAX_RATE_HZ = 100.0
_TRANSACTION_S = 0.08
_OPEN_TIMEOUT_S = 2.0
_STALE_S = 0.5
#: The most a command may take to be written: the control loop waits for it, and a quarter of a second is the contract's bound (`tools/faultproxy/chaos.py`, F256).
_COMMAND_BUDGET_S = 0.25
_FAULT_THRESHOLD = 3
_INITIAL_READ_ATTEMPTS = 3
#: A finger within this of its target is there. The registers resolve a tenth of a millimetre and nothing says how close a finger rests: this driver's own figure.
_ARRIVAL_TOLERANCE_MM = 1.0
_REOPEN_S = 0.5
_KNOWN_KEYS = {"model", "host", "port", "device", "baud", "slave", "limits", "force_n", "speed", "supply_power_w", "stop_mode", "simulator", "roles", "rate_hz", "timeout_ms", "stale_ms", "open_timeout_ms"}
_MOTIONS = ("status", "move")


class BridgeRefusal(Exception):
    """A reason this body did not come up or a command did not happen, in the terms an integrator acts on."""

    def __init__(self, message: str, code: int = rhal.SKR_ENODEV) -> None:
        super().__init__(message)
        self.code = code


class Unreachable(BridgeRefusal):
    """The gripper did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""

    def __init__(self, message: str) -> None:
        super().__init__(message, rhal.SKR_EIO)


class Reading:
    """One cycle's worth of what the gripper says."""

    __slots__ = ("rg", "fg", "limits")

    def __init__(self, rg: frames.RgState | None, fg: frames.FgState | None, limits: frames.FgLimits | None) -> None:
        self.rg, self.fg, self.limits = rg, fg, limits

    @property
    def width_mm(self) -> float:
        return self.rg.width_mm if self.rg is not None else self.fg.external_width_mm       # type: ignore[union-attr]

    @property
    def busy(self) -> bool:
        return self.rg.busy if self.rg is not None else self.fg.busy                          # type: ignore[union-attr]

    @property
    def grip(self) -> bool:
        return self.rg.grip_detected if self.rg is not None else self.fg.grip_detected        # type: ignore[union-attr]

    @property
    def raw_status(self) -> int:
        return self.rg.raw_status if self.rg is not None else self.fg.raw_status              # type: ignore[union-attr]

    @property
    def fault(self) -> str | None:
        return frames.rg_error_text(self.rg) if self.rg is not None else frames.fg_error_text(self.fg)   # type: ignore[arg-type]

    @property
    def fault_word(self) -> int:
        """The status word's fault bits as the maker lays them out: an RG's safety bits and its three-bit error code (bits 2 to 9), a 2FG's not-calibrated and sensor-error bits."""
        if self.rg is not None:
            return self.rg.raw_status & 0x3FC
        return self.fg.raw_status & (frames.FG_NOT_CALIBRATED | frames.FG_LINEAR_SENSOR_ERROR)   # type: ignore[union-attr]


class OnRobotBridge:
    """A body made of one gripper's jaw."""

    #: A stop written to the host while a motion runs is read at once: `emergency_stop` is safe from another thread (python-host's `stop_during_extension`).
    stop_during_extension = True

    def __init__(self) -> None:
        self._opened = False
        self._estop = False
        self._descriptor: str | None = None
        self._seq = 0
        self._simulated = True
        self._model: frames.Model = MODELS["RG2"]
        self._slave = 65
        self._limits_mm: tuple[float, float] = (0.0, 0.0)
        self._force_n = 0.0
        self._speed_percent = 0.0
        self._supply_w = 0
        self._roles: dict[str, str | None] = {}
        self._rate_hz = _RATE_HZ
        self._stale_s = _STALE_S
        self._timeout = _TRANSACTION_S
        self._firmware = ""
        self._link: link.Link | None = None
        self._loop: threading.Thread | None = None
        self._closing = threading.Event()
        #: Wakes the reading thread: a stop is written by it, at once, and `emergency_stop` never waits on the line.
        self._kick = threading.Event()
        self._link_lock = threading.Lock()
        self._lock = threading.Lock()
        self._changed = threading.Condition(self._lock)
        self._last_refusal: str | None = None
        # Everything below is guarded by `_lock`.
        self._latest: Reading | None = None
        self._stamp_ns = 0
        self._exchanges = 0
        self._failures = 0
        self._dead: str | None = None
        self._target_mm: float | None = None
        self._commanded_at = 0.0
        self._power_dirty = False
        self._stop_delivered = False
        self._reasserted = 0

    # ── §4's entry points ────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError as error:
            _say(f"the configuration is not JSON: {error}")
            return rhal.SKR_EINVAL
        if not isinstance(settings, dict):
            _say("the configuration is not a JSON object.")
            return rhal.SKR_EINVAL
        self._teardown()
        self._reset_session()
        try:
            self._open(settings)
        except BridgeRefusal as refusal:
            self._teardown()
            _say(str(refusal))
            return rhal.SKR_EIO if isinstance(refusal, Unreachable) else rhal.SKR_ENODEV
        except (OSError, frames.ProtocolError, modbus.ProtocolError, link.LinkError) as error:
            self._teardown()
            _say(f"the gripper could not be read: {error}")
            return rhal.SKR_EIO
        self._opened = True
        return rhal.SKR_OK

    def _reset_session(self) -> None:
        """A session starts with no stop, no fault remembered and no reading: nothing of the last one is the gripper's."""
        self._estop = False
        self._descriptor = None
        self._last_refusal = None
        with self._lock:
            self._latest = None
            self._target_mm = None
            self._stop_delivered = False
            self._power_dirty = False
            self._reasserted = 0

    def _open(self, settings: dict) -> None:
        unknown = sorted(set(settings) - _KNOWN_KEYS)
        if unknown:
            raise BridgeRefusal(f"the configuration has {', '.join(map(repr, unknown))}, which this driver does not read. The keys it reads are {', '.join(sorted(_KNOWN_KEYS))}.")
        model = settings.get("model")
        if model not in MODELS:
            if model in ("3FG15", "3FG25"):
                raise BridgeRefusal(f"`model` of {model!r} is not driven: the maker offers its three-finger API for evaluation only, and a diameter-and-angle gripper is not a parallel jaw this body describes.")
            raise BridgeRefusal(f"`model` of {model!r} is not in the configuration, or is not one of {', '.join(map(repr, MODELS))}. The gripper's product code identifies it, but what a register means depends on the model, so the "
                                "operator says which it is and the open checks the gripper agrees.")
        self._model = MODELS[model]
        fg = self._model.family == "fg"
        host, device = settings.get("host"), settings.get("device")
        if (host is None) == (device is None):
            raise BridgeRefusal("the configuration names neither a Compute Box (`host`, and `port`) nor a serial `device`, or names both. This driver does not scan for a gripper.")
        slave = settings.get("slave", 65)
        if isinstance(slave, bool) or not isinstance(slave, int) or slave not in modbus.SLAVES:
            raise BridgeRefusal(f"`slave` of {slave!r} is not one of {', '.join(map(str, modbus.SLAVES))}: the single tool, the primary and the secondary of a dual Quick Changer.")
        self._slave = slave
        if host is not None:
            port = settings.get("port", 502)
            if not isinstance(host, str) or not host or isinstance(port, bool) or not isinstance(port, int) or not 0 < port < 65536:
                raise BridgeRefusal(f"`host` of {host!r} and `port` of {port!r} do not name a Compute Box: a host name and a port from 1 to 65535 (502 from the factory).")
            self._link = link.Link(host=host, port=port)
        else:
            baud = settings.get("baud", 115200)
            if not isinstance(device, str) or not device:
                raise BridgeRefusal(f"`device` of {device!r} is not a path.")
            if isinstance(baud, bool) or not isinstance(baud, int) or baud not in link.BAUDS:
                raise BridgeRefusal(f"`baud` of {baud!r} is not one of {', '.join(map(str, link.BAUDS))}: the speeds the tool bus runs at (8 data bits, even parity, one stop bit).")
            self._link = link.Link(device=device, baud=baud)
        if settings.get("stop_mode") != "stop":
            raise BridgeRefusal("`stop_mode` is not in the configuration, or is not `stop`. The maker's library offers one stop, and its documentation says to support the workpiece before stopping or releasing a grip: "
                                "what the stop does to a held part is the integrator's to decide. Say `\"stop_mode\": \"stop\"` to take the maker's.")
        claim = settings.get("simulator")
        if not isinstance(claim, bool):
            raise BridgeRefusal("`simulator` is not in the configuration, or is not true or false. Nothing the gripper says tells a real one from a simulation, so the descriptor cannot be left to guess.")
        self._simulated = claim
        force = settings.get("force_n")
        top = self._model.max_force_n if self._model.max_force_n is not None else 220.0
        if isinstance(force, bool) or not isinstance(force, (int, float)) or not math.isfinite(force) or not 0 < force <= top:
            raise BridgeRefusal(f"no `force_n` in the configuration, or {force!r} is not a number above 0 and at most {top:g}: the closing force the fingers are told to use. How hard to hold is the operator's, and nothing in a position request says it.")
        if fg and 0 < force < (self._model.min_force_n or 0.0):
            raise BridgeRefusal(f"`force_n` of {force!r} is below the {self._model.name}'s conventional minimum of {self._model.min_force_n:g} N.")
        self._force_n = float(force)
        speed, power = settings.get("speed"), settings.get("supply_power_w")
        if fg:
            if isinstance(speed, bool) or not isinstance(speed, (int, float)) or not math.isfinite(speed) or not 1 <= speed <= 100:
                raise BridgeRefusal(f"no `speed` in the configuration, or {speed!r} is not a percentage from 1 to 100: the maker's native 2FG speed, which is not a linear scale in mm/s. How fast the jaw closes is the operator's.")
            if isinstance(power, bool) or not isinstance(power, int) or not frames.FG_POWER_MIN_W <= power <= frames.FG_POWER_MAX_W:
                raise BridgeRefusal(f"no `supply_power_w` in the configuration, or {power!r} is not a whole number of watts from {frames.FG_POWER_MIN_W} to {frames.FG_POWER_MAX_W}: the 2FG's supply budget, which sets its force "
                                    "ceiling and which the firmware gives back to 14 W at every power cycle.")
            self._speed_percent, self._supply_w = float(speed), power
        else:
            if speed is not None or power is not None:
                raise BridgeRefusal(f"`speed` and `supply_power_w` are a 2FG's: an {self._model.name} has no speed command and no supply budget, and a key that would be ignored is refused.")
            self._speed_percent, self._supply_w = 0.0, 0
        roles = settings.get("roles") or {}
        if not isinstance(roles, dict) or set(roles) - {JOINT}:
            raise BridgeRefusal(f"`roles` is a mapping from a joint's name to its role, and the one joint is {JOINT}.")
        self._roles = {JOINT: roles.get(JOINT)}
        self._rate_hz = _bounded(settings, "rate_hz", _RATE_HZ, 1.0, _MAX_RATE_HZ)
        self._timeout = _bounded(settings, "timeout_ms", _TRANSACTION_S * 1000, 10.0, 5000.0) / 1000.0
        self._stale_s = _bounded(settings, "stale_ms", _STALE_S * 1000, 20.0, 30000.0) / 1000.0
        open_s = _bounded(settings, "open_timeout_ms", _OPEN_TIMEOUT_S * 1000, 100.0, 60000.0) / 1000.0

        assert self._link is not None
        self._link.open(timeout=min(open_s, 3.0))
        self._identify(open_s)
        limits = self._device_limits()
        self._limits_mm = self._load_limits(settings.get("limits"), limits)
        if fg:
            self._assert_power()
        reading = self._first_reading(open_s)
        fault = reading.fault
        if fault is not None:
            _say(f"the gripper reports {fault}: commands are refused until it is cleared.")
        with self._lock:
            self._latest, self._stamp_ns = reading, rhal.acquisition_ns()
            self._exchanges, self._failures, self._dead = 0, 0, None
        self._closing.clear()
        self._kick.clear()
        self._loop = threading.Thread(target=self._exchange_loop, daemon=True, name="onrobot-exchange")
        self._loop.start()
        self._descriptor = self._build_descriptor()

    def _identify(self, open_s: float) -> None:
        """The product code at 1536 must be the model's, and the firmware is read once for the record. A gripper that answers as another model is refused by name: its registers mean something else."""
        count = frames.FG_IDENTITY_COUNT if self._model.family == "fg" else 1
        deadline = time.monotonic() + open_s
        last: Exception | None = None
        registers: list[int] = []
        for _ in range(_INITIAL_READ_ATTEMPTS):
            try:
                registers = self._txn_read(frames.PRODUCT_CODE_ADDRESS, count, min(self._timeout * 4, max(0.01, deadline - time.monotonic())))
                break
            except (link.LinkError, modbus.ProtocolError) as error:
                last = error
                if time.monotonic() >= deadline:
                    break
        else:
            registers = []
        if not registers:
            assert self._link is not None
            raise Unreachable(f"nothing answered at slave {self._slave} on {self._link.target} ({last}). Is the Compute Box or the tool powered, and does the slave address match the mounting (65 for a single tool)?")
        if registers[0] != self._model.product_code:
            raise BridgeRefusal(f"the device at slave {self._slave} reports product code {registers[0]}, and an {self._model.name} is {self._model.product_code}. Its registers mean something else, so nothing was commanded: "
                                "check `model` and `slave` against what is mounted.")
        self._firmware = frames.firmware_text(self._txn_read(frames.FIRMWARE_ADDRESS, 2))

    def _device_limits(self) -> frames.FgLimits | None:
        if self._model.family != "fg":
            return None
        return frames.decode_fg_limits(self._txn_read(frames.FG_LIMITS_ADDRESS, 4), self._txn_read(frames.FG_POWER_ADDRESS, frames.FG_POWER_COUNT))

    def _load_limits(self, given: object, device: frames.FgLimits | None) -> tuple[float, float]:
        """The jaw's range in millimetres. An RG's is the installation's and must be given, within the model's absolute width; a 2FG's is the device's own, and a configuration may narrow it and never widen it."""
        if device is not None:
            low, high = device.min_external_mm, device.max_external_mm
            if not 0 <= low < high:
                raise BridgeRefusal(f"the device reports external limits [{low}, {high}] mm, which are not a range: nothing can be commanded against them.")
        else:
            low, high = 0.0, float(self._model.max_width_mm or 0.0)
            if given is None:
                raise BridgeRefusal(f"`limits` is not in the configuration. The maker's library makes the integrator declare the working aperture for the fitted fingertips of an {self._model.name} (0 to {high:g} mm at most), "
                                    "as [min, max] in metres: a gripper told to close on nothing at the end of its range is the integrator's to bound.")
        if given is None:
            return (low, high)
        if not (isinstance(given, (list, tuple)) and len(given) == 2 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) for v in given)):
            raise BridgeRefusal("`limits` is [min, max] in metres, as two numbers.")
        lo, hi = float(given[0]) * 1000.0, float(given[1]) * 1000.0
        if not lo < hi:
            raise BridgeRefusal(f"`limits` of [{given[0]}, {given[1]}] is not a range.")
        if lo < low - 1e-6 or hi > high + 1e-6:
            raise BridgeRefusal(f"`limits` of [{lo:.1f}, {hi:.1f}] mm is wider than the jaw's range, [{low:.1f}, {high:.1f}] mm: a configuration may narrow it and never widen it.")
        return (lo, hi)

    def _first_reading(self, budget: float) -> Reading:
        """Up to three whole readings before the gripper is called absent."""
        deadline = time.monotonic() + budget
        last: Exception | None = None
        for _ in range(_INITIAL_READ_ATTEMPTS):
            try:
                return self._read_all(max(0.01, min(self._timeout * 4, deadline - time.monotonic())))
            except (link.LinkError, modbus.ProtocolError, frames.ProtocolError) as error:
                last = error
                if time.monotonic() >= deadline:
                    break
        assert self._link is not None
        raise Unreachable(f"no gripper answered with its state at slave {self._slave} on {self._link.target} ({last}).")

    def close(self) -> int:
        # A close writes nothing: the gripper keeps the last target it was given, and what stopping it does to a held part is the integrator's to decide (see `stop_mode`), never a close's.
        self._teardown()
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        now = rhal.acquisition_ns()
        with self._lock:
            reading, stamp, dead = self._latest, self._stamp_ns, self._dead
            power_dirty = self._power_dirty
        self._seq += 1
        out.seq = self._seq
        out.stamp_ns = stamp or now
        out.flags = rhal.HOMING_NOT_REQUIRED
        out.fault_code = 0
        out.position_rad = [math.nan]
        out.velocity_rad_s = [math.nan]
        out.effort_nm = [math.nan]
        if reading is None:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        out.position_rad = [reading.width_mm / 1000.0]
        out.velocity_rad_s = [reading.rg.task_velocity_mm_s / 1000.0 if reading.rg is not None else math.nan]
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP
        if reading.fault is not None:
            out.flags |= rhal.FLAG_FAULT | rhal.FLAG_DEGRADED
            out.fault_code = reading.fault_word
        if reading.fg is not None and reading.fg.not_calibrated:
            out.flags = (out.flags & ~rhal.HOMING_MASK) | rhal.HOMING_PENDING
        if power_dirty or (reading.limits is not None and reading.limits.supply_power_w != self._supply_w):
            out.flags |= rhal.FLAG_DEGRADED
        age = (rhal.acquisition_ns() - stamp) / 1e9
        if dead or age > self._stale_s:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened or self._link is None:
            return rhal.SKR_EPERM
        if cmd.joint_count != 1:
            return self._refused("count", f"the command has {cmd.joint_count} joints and this body has one ({JOINT}): nothing was sent.")
        if cmd.mode != rhal.MODE_POSITION:
            return self._refused("mode", f"the command is in mode {cmd.mode}, and this gripper takes a position: a target width, and there is no velocity or force-as-torque route. Send a position.")
        if self._estop:
            return rhal.SKR_EPERM
        if cmd.mask and not cmd.mask[0]:
            return rhal.SKR_OK
        value = cmd.values[0]
        if not math.isfinite(value):
            return self._refused("finite", f"{JOINT} was commanded to {value!r}, which is not finite: nothing was sent.")
        low, high = self._limits_mm
        width_mm = value * 1000.0
        if not low - 1e-6 <= width_mm <= high + 1e-6:
            return self._refused("range", f"{JOINT} was commanded to {width_mm:.2f} mm, outside its range [{low:.2f}, {high:.2f}] mm: nothing was sent.")
        width_mm = min(max(width_mm, low), high)
        try:
            self._gate()
            self._send_grip(width_mm)
        except BridgeRefusal as error:
            return self._refused("command:" + str(error.code) + ":" + error.args[0][:40], str(error), error.code)
        except (frames.ProtocolError, modbus.ProtocolError) as error:
            return self._refused("protocol", str(error), rhal.SKR_EINVAL if isinstance(error, frames.ProtocolError) else rhal.SKR_EIO)
        self._last_refusal = None
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        """Latch, and wake the thread that writes the stop. The call does not touch the line: the RHAL's "never blocks" is 50 ms, and a line that is slow or dead would make a stop that wrote its own control block wait
        for it. The latch is the stop: commands are refused from this instant, and the maker's stop is on the wire as soon as the line carries it, which `status` reports."""
        with self._lock:
            self._estop = True
            self._stop_delivered = False
            self._changed.notify_all()
        self._kick.set()
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """Clear the session latch (§48.2) once the gripper has been *seen* to take the stop: a reading after the stop was written shows the jaw not moving."""
        if self._link is None or self._loop is None:
            self._estop = False
            return rhal.SKR_OK
        try:
            self._wait_for_reading(lambda r: self._stop_delivered and not r.busy, _COMMAND_BUDGET_S * 4 + self._timeout * 6, honour_stop=False)
        except BridgeRefusal as error:
            _say(f"the stop stays latched: no reading has shown the gripper take it ({error}) Fix the line, or close and open the driver, which is a new session.")
            return rhal.SKR_EIO
        with self._lock:
            self._estop = False
            self._stop_delivered = False
            self._target_mm = None
        self._last_refusal = None
        return rhal.SKR_OK

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── the line ─────────────────────────────────────────────────────────────

    @contextlib.contextmanager
    def _line(self, by: float | None = None):
        """The line's lock. With `by` (a monotonic time) the wait for it ends there: a caller that is owed an answer in a fixed time is not owed it later because the reading thread was in a transaction."""
        if by is None:
            self._link_lock.acquire()
        elif not self._link_lock.acquire(timeout=max(0.0, by - time.monotonic())):
            raise link.LinkError("the line was in use until the time the caller had been given ran out.")
        try:
            yield
        finally:
            self._link_lock.release()

    def _budget(self, by: float | None, budget: float | None = None) -> float:
        """A transaction's time: the line's own bound, cut to what is left of `by`."""
        wanted = budget or self._timeout
        if by is None:
            return wanted
        remaining = by - time.monotonic()
        if remaining <= 0:
            raise link.LinkError("the time the caller had been given ran out before the line was free.")
        return min(wanted, remaining)

    def _txn_read(self, address: int, count: int, budget: float | None = None) -> list[int]:
        link_ = self._link
        if link_ is None:
            raise link.LinkError("the line is not open.")
        with self._link_lock:
            return link_.transact(self._slave, modbus.pdu_read(address, count), budget or self._timeout)

    def _txn_write(self, address: int, values: list[int], by: float | None = None) -> None:
        link_ = self._link
        if link_ is None:
            raise link.LinkError("the line is not open.")
        pdu = modbus.pdu_write_single(address, values[0]) if len(values) == 1 else modbus.pdu_write_multiple(address, values)
        with self._line(by):
            link_.transact(self._slave, pdu, self._budget(by))

    def _read_all(self, budget: float | None = None, between=None) -> Reading:
        """The cycle's reads, checked. `between` is called before the second, so that a stop that is owed goes out ahead of it."""
        if self._model.family == "rg":
            return Reading(frames.decode_rg_state(self._txn_read(frames.RG_STATUS_ADDRESS, frames.RG_STATUS_COUNT, budget)), None, None)
        state = frames.decode_fg_state(self._txn_read(frames.FG_STATUS_ADDRESS, frames.FG_STATUS_COUNT, budget))
        if between is not None:
            between()
        limits = frames.decode_fg_limits(self._txn_read(frames.FG_LIMITS_ADDRESS, 4, budget), self._txn_read(frames.FG_POWER_ADDRESS, frames.FG_POWER_COUNT, budget))
        return Reading(None, state, limits)

    def _assert_power(self, by: float | None = None) -> None:
        """Tell a 2FG the operator's supply budget and read it back: the maker's library refuses a gripper that does not keep it. The force ceiling moves with it."""
        self._txn_write(frames.FG_POWER_ADDRESS, [self._supply_w], by)
        power = self._txn_read(frames.FG_POWER_ADDRESS, frames.FG_POWER_COUNT, self._budget(by))
        if power[0] != self._supply_w:
            raise BridgeRefusal(f"the gripper did not keep the supply budget of {self._supply_w} W it was told (it reports {power[0]} W): its force ceiling is not the one the operator chose.", rhal.SKR_EIO)
        with self._lock:
            self._power_dirty = False

    def _gate(self) -> None:
        """What a command waits for: a gripper that has been read and that holds no fault."""
        with self._lock:
            reading = self._latest
        if reading is None:
            raise BridgeRefusal("the gripper has not been read yet.", rhal.SKR_ETIMEDOUT)
        fault = reading.fault
        if fault is not None:
            raise BridgeRefusal(f"the gripper reports {fault}, and a gripper holding a fault takes nothing: whatever the cause is has to be dealt with at the gripper, since the maker's library has no call that clears it. "
                                "The command was not sent.", rhal.SKR_EPERM)

    def _send_grip(self, width_mm: float) -> None:
        """One write of the control block, validated as the maker's library validates it, under the line's lock, with the latch checked again inside it so that a command that waited for a stop does not follow it."""
        deadline = time.monotonic() + _COMMAND_BUDGET_S
        try:
            with self._lock:
                reading = self._latest
                power_dirty = self._power_dirty
            if self._model.family == "fg":
                assert reading is not None and reading.limits is not None
                if power_dirty or reading.limits.supply_power_w != self._supply_w:
                    self._assert_power(deadline)
                    limits = frames.decode_fg_limits(self._txn_read(frames.FG_LIMITS_ADDRESS, 4, self._budget(deadline)), self._txn_read(frames.FG_POWER_ADDRESS, frames.FG_POWER_COUNT, self._budget(deadline)))
                else:
                    limits = reading.limits
                block = frames.encode_fg_grip(width_mm, self._force_n, self._speed_percent, limits, self._model)
            else:
                block = frames.encode_rg_move(width_mm, self._force_n, self._limits_mm[0], self._limits_mm[1], self._model)
            address = frames.FG_CONTROL_ADDRESS if self._model.family == "fg" else frames.RG_CONTROL_ADDRESS
            with self._line(deadline):
                if self._estop:
                    raise BridgeRefusal("the stop was latched while the command was being made.", rhal.SKR_EPERM)
                assert self._link is not None
                self._link.transact(self._slave, modbus.pdu_write_multiple(address, block), self._budget(deadline))
            with self._lock:
                self._target_mm = width_mm
                self._commanded_at = time.monotonic()
        except link.LinkError as error:
            # A command that ran out of time waiting for the line while a stop was being written was refused by the stop, not by a slow line: the reading thread holds the line for the stop's write, and which of
            # the two gets it first is the scheduler's. The latch decides, so the answer does not depend on the race.
            if self._estop:
                raise BridgeRefusal("the stop is latched: nothing was written.", rhal.SKR_EPERM) from error
            raise BridgeRefusal(f"the gripper did not take the command ({error}): a slow or dead line, not a refusal by the gripper.", rhal.SKR_EIO) from error

    def _deliver_stop(self) -> None:
        """The maker's stop, written by the reading thread under the line's lock, so it follows any write already in flight."""
        block = frames.fg_stop_block() if self._model.family == "fg" else frames.rg_stop_block()
        address = frames.FG_CONTROL_ADDRESS if self._model.family == "fg" else frames.RG_CONTROL_ADDRESS
        with self._link_lock:
            link_ = self._link
            if link_ is None:
                raise link.LinkError("the line is not open.")
            link_.transact(self._slave, modbus.pdu_write_multiple(address, block), self._timeout)
        with self._lock:
            self._stop_delivered = True
            self._target_mm = None
            self._changed.notify_all()

    def _exchange_loop(self) -> None:
        """The reading thread: the gripper's own state in, once a period, forever; and, when something is owed, the stop or the supply budget out."""
        assert self._link is not None
        link_ = self._link
        period = 1.0 / self._rate_hz
        reopened = 0.0
        open_error: str | None = None
        due = time.monotonic()
        while not self._closing.is_set():
            woken = self._kick.wait(max(0.0, due - time.monotonic()))
            if self._closing.is_set():
                break
            self._kick.clear()
            try:
                self._deliver_if_owed()
                reading = self._read_all(between=self._yield_to_stop)
                self._maintain(reading)
            except _StopOwed:
                due = time.monotonic()
                continue
            except (link.LinkError, modbus.ProtocolError, frames.ProtocolError, BridgeRefusal) as error:
                reason = str(error)
                if not link_.is_open:
                    if time.monotonic() - reopened >= _REOPEN_S:
                        # A gateway that went away: the line is opened again, at most twice a second, and what it says is what is reported while it stays shut.
                        reopened = time.monotonic()
                        try:
                            link_.open(timeout=min(self._timeout, 0.5))
                            open_error = None
                        except link.LinkError as again:
                            open_error = str(again)
                    reason = open_error or reason
                with self._lock:
                    self._failures += 1
                    if self._failures >= _FAULT_THRESHOLD:
                        self._dead = reason
                        # A line that was dead is not a gripper that was left as this driver last asked: it may have been power-cycled, which gives a 2FG its 14 W back. An RG has no budget to give back.
                        self._power_dirty = self._model.family == "fg"
                    self._changed.notify_all()
            else:
                with self._lock:
                    self._latest, self._stamp_ns = reading, rhal.acquisition_ns()
                    self._exchanges += 1
                    self._failures, self._dead = 0, None
                    self._changed.notify_all()
            if woken and time.monotonic() < due:
                continue                    # a stop woke this cycle ahead of its time: it does not take the period's slot, or at 1 Hz the next reading after a stop would be two seconds away
            due = max(due + period, time.monotonic())

    def _stop_owed(self) -> bool:
        with self._lock:
            return self._estop and not self._stop_delivered

    def _deliver_if_owed(self) -> None:
        if self._stop_owed():
            self._deliver_stop()

    def _yield_to_stop(self) -> None:
        if self._stop_owed():
            raise _StopOwed

    def _maintain(self, reading: Reading) -> None:
        """What a good reading shows is owed: a stop that has not been written, or a 2FG's supply budget that the gripper no longer has."""
        if self._model.family != "fg":
            return
        with self._lock:
            owed_stop = self._estop and not self._stop_delivered
            differs = reading.limits is not None and reading.limits.supply_power_w != self._supply_w
            if differs:
                self._power_dirty = True
            dirty = self._power_dirty
        if owed_stop:
            self._deliver_stop()
        elif dirty:
            self._assert_power()
            with self._lock:
                self._reasserted += 1

    def _wait_for_reading(self, predicate, timeout: float, *, honour_stop: bool = True) -> Reading:
        """The first reading after now that `predicate` holds for."""
        deadline = time.monotonic() + timeout
        with self._changed:
            seen = self._exchanges
            while True:
                if self._closing.is_set():
                    raise BridgeRefusal("the driver was closed.", rhal.SKR_EPERM)
                if honour_stop and self._estop:
                    raise BridgeRefusal("the motion ended: the stop was latched.", rhal.SKR_EPERM)
                if self._exchanges > seen and self._latest is not None and predicate(self._latest):
                    return self._latest
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise BridgeRefusal(f"no reading showed what was waited for in {timeout:.1f} s ({self._dead or 'the line is answering'}).", rhal.SKR_EIO)
                self._changed.wait(min(remaining, 0.05))

    # ── plumbing ─────────────────────────────────────────────────────────────

    def _refused(self, cause: str, message: str, code: int = rhal.SKR_EINVAL) -> int:
        """`code` and the reason, said once for a run of the same cause: a stream at the control rate that fails for one reason would otherwise say it every period."""
        if cause != self._last_refusal:
            self._last_refusal = cause
            _say(message)
        return code

    def _teardown(self) -> None:
        self._closing.set()
        self._kick.set()
        with self._changed:
            self._changed.notify_all()
        if self._loop is not None:
            self._loop.join(timeout=2.0)
        self._loop = None
        if self._link is not None:
            self._link.close()
        self._link = None
        with self._lock:
            self._latest = None

    def _build_descriptor(self) -> str:
        low, high = self._limits_mm
        joint = {"name": JOINT, "role": self._roles.get(JOINT), "type": "prismatic",
                 "limits": {"position_min_rad": low / 1000.0, "position_max_rad": high / 1000.0}, "control_modes": ["position"]}
        effector = {"name": "gripper", "role": self._roles.get(JOINT), "kind": "gripper", "dof": 1, "max_grip_force_n": self._model.max_force_n, "max_opening_m": (high - low) / 1000.0}
        descriptor = {
            "schema_version": 1, "robot_slug": "onrobot-" + self._model.name.lower(), "digest": None, "verified": False, "simulator": self._simulated, "base": "fixed", "dof": 1,
            "joints": [joint], "end_effectors": [effector], "sensors": [],
            "control": {"modes": ["position"], "rate_hz_max": self._rate_hz, "rate_hz_min": 1.0},
            # Null: the range comes from the installation (an RG) or the device (a 2FG), the speed from nothing the maker publishes (an RG has no speed command, a 2FG's is a native percentage), and none of it is a risk assessment (§5.2).
            "envelope": None, "capabilities": ["safety.estop"], "motions": [_descriptor(name) for name in _MOTIONS], "extensions": [TOKEN], "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))

    # ── the extension: motions ───────────────────────────────────────────────

    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        if token != TOKEN:
            return rhal.SKR_ENOSYS, f"unknown token {token!r}. This driver serves {TOKEN}.".encode()
        if not self._opened or self._link is None:
            return rhal.SKR_EPERM, b"the driver is not open."
        try:
            words = data.decode("utf-8").split()
        except UnicodeDecodeError:
            return rhal.SKR_EINVAL, b"the request is text: `<motion> k=v k=v` in UTF-8."
        if not words:
            return rhal.SKR_EINVAL, f"empty request. Motions: {', '.join(_MOTIONS)}.".encode()
        motion, args = words[0], {}
        for word in words[1:]:
            key, sep, value = word.partition("=")
            if not sep or not key:
                return rhal.SKR_EINVAL, f"malformed argument {word!r}: wanted k=v.".encode()
            args[key] = value
        if motion not in _MOTIONS:
            return rhal.SKR_EINVAL, f"unknown motion {motion!r}. Motions: {', '.join(_MOTIONS)}.".encode()
        if self._estop and motion != "status":
            return rhal.SKR_EPERM, b"refused: the stop is latched. Only `status` is answered until it is released."
        try:
            return rhal.SKR_OK, json.dumps(getattr(self, f"_motion_{motion}")(args), separators=(",", ":")).encode()
        except BridgeRefusal as refusal:
            return refusal.code, str(refusal).encode()
        except (frames.ProtocolError, modbus.ProtocolError) as error:
            return rhal.SKR_EINVAL, f"the request does not fit the wire: {error}".encode()
        except link.LinkError as error:
            return rhal.SKR_EIO, f"the gripper did not carry the request: {error}".encode()

    def _number(self, args: dict, key: str, *, minimum: float | None = None, maximum: float | None = None) -> float | None:
        if key not in args:
            return None
        try:
            value = float(args.pop(key))
        except ValueError:
            raise BridgeRefusal(f"`{key}` is not a number.", rhal.SKR_EINVAL) from None
        if not math.isfinite(value) or (minimum is not None and value < minimum) or (maximum is not None and value > maximum):
            raise BridgeRefusal(f"`{key}` of {value:g} is outside {minimum if minimum is not None else '-inf'} to {maximum if maximum is not None else 'inf'}.", rhal.SKR_EINVAL)
        return value

    def _no_arguments(self, motion: str, args: dict) -> None:
        if args:
            raise BridgeRefusal(f"`{motion}` does not take {', '.join(sorted(args))}.", rhal.SKR_EINVAL)

    def _present(self) -> Reading:
        with self._lock:
            reading = self._latest
        if reading is None:
            raise BridgeRefusal("the gripper has not been read yet.", rhal.SKR_ETIMEDOUT)
        return reading

    def _motion_status(self, args: dict) -> dict:
        self._no_arguments("status", args)
        reading = self._present()
        with self._lock:
            exchanges, dead, target = self._exchanges, self._dead, self._target_mm
            delivered, reasserted = self._stop_delivered, self._reasserted
        out: dict[str, object] = {
            "model": self._model.name, "slave": self._slave, "link": self._link.target if self._link else None, "firmware": self._firmware, "width_mm": reading.width_mm, "position_m": reading.width_mm / 1000.0,
            "busy": reading.busy, "grip_detected": reading.grip, "raw_status": reading.raw_status, "fault": reading.fault, "target_mm": target, "limits_mm": list(self._limits_mm),
            "force_n_configured": self._force_n, "exchanges": exchanges, "link_dead": dead, "stop_latched": self._estop, "stop_delivered": delivered, "simulator": self._simulated}
        if reading.rg is not None:
            rg = reading.rg
            out.update({"actual_width_mm": rg.actual_width_mm, "fingertip_offset_mm": rg.fingertip_offset_mm, "error_code": rg.error_code, "temperature_c": rg.temperature_c, "motor_current_a": rg.motor_current_a,
                        "motor_voltage_v": rg.motor_voltage_v, "task_velocity_mm_s": rg.task_velocity_mm_s,
                        "note": "an RG reports no force: what the library calls command force is the command's own echo, and this driver reports none"})
        else:
            fg, limits = reading.fg, reading.limits
            assert fg is not None and limits is not None
            out.update({"internal_width_mm": fg.internal_width_mm, "force_n_signed": fg.force_n, "not_calibrated": fg.not_calibrated, "linear_sensor_error": fg.linear_sensor_error,
                        "speed_percent_configured": self._speed_percent, "supply_power_w": limits.supply_power_w, "supply_power_w_configured": self._supply_w, "supply_power_reasserted": reasserted,
                        "maximum_force_n": limits.max_force_n, "device_limits_mm": [limits.min_external_mm, limits.max_external_mm]})
        return out

    def _motion_move(self, args: dict) -> dict:
        timeout = self._number(args, "timeout", minimum=0.5, maximum=120.0)
        width = self._number(args, "width", minimum=self._limits_mm[0] / 1000.0, maximum=self._limits_mm[1] / 1000.0)
        self._no_arguments("move", args)
        if width is None:
            raise BridgeRefusal("`move` needs `width=<metres>`.", rhal.SKR_EINVAL)
        target_mm = width * 1000.0
        before = self._present()
        wait = timeout if timeout is not None else abs(target_mm - before.width_mm) / 20.0 + 5.0
        self._gate()
        self._send_grip(target_mm)
        started = [False]

        def done(reading: Reading) -> bool:
            if reading.fault is not None:
                return True
            # A jaw has started when a reading shows it busy or somewhere other than where it was: a hand-over that happens between two readings (a fast gripper, a slow period) shows the second and never the first.
            started[0] = started[0] or reading.busy or abs(reading.width_mm - before.width_mm) > _ARRIVAL_TOLERANCE_MM
            if reading.busy:
                return False
            arrived = abs(reading.width_mm - target_mm) <= _ARRIVAL_TOLERANCE_MM
            return arrived or (started[0] and reading.grip)

        try:
            reading = self._wait_for_reading(done, wait)
        except BridgeRefusal as error:
            if not started[0] and error.code == rhal.SKR_EIO and "no reading showed" in str(error):
                raise BridgeRefusal(f"the gripper never started moving after the command (its busy flag did not rise, and the jaw did not move, in {wait:.1f} s): {error}", rhal.SKR_EIO) from None
            raise
        fault = reading.fault
        if fault is not None:
            raise BridgeRefusal(f"the gripper reports {fault} during the move.", rhal.SKR_EIO)
        arrived = abs(reading.width_mm - target_mm) <= _ARRIVAL_TOLERANCE_MM
        return {"result": "arrived" if arrived else "stopped_on_an_object", "width_mm": reading.width_mm, "position_m": reading.width_mm / 1000.0, "grip_detected": reading.grip}


class _StopOwed(Exception):
    """A reading cycle was left between two of its reads because a stop has to be written first."""


def _descriptor(name: str) -> dict:
    if name == "status":
        return {"name": "status", "parameters": []}
    return {"name": "move", "parameters": [{"name": "width", "unit": "m", "required": True}, {"name": "timeout", "unit": "s", "required": False}]}


def _bounded(settings: dict, key: str, default: float, low: float, high: float) -> float:
    value = settings.get(key)
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not low <= value <= high:
        raise BridgeRefusal(f"`{key}` of {value!r} is not a number from {low:g} to {high:g}.")
    return float(value)


def _say(message: str) -> None:
    print(f"skynet-onrobot: {message}", file=sys.stderr, flush=True)
