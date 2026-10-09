"""The RHAL driver: an Inspire Robots RH56 dexterous hand over its RS-485 private protocol (DRY-COVERAGE-PLAN.md item 12.E.8).

§4's five mandatory entry points over :mod:`skynet_inspire.frames` and :mod:`skynet_inspire.link`. It links nothing of the maker's: Inspire publish a manual and a PC tool, no SDK and no simulator. The wire was read out of
the manuals (`frames.py` cites each constant), and `layout_check.py` holds this driver's bytes to the manuals' own worked frames and to the two independent transcriptions of them on PyPI.

# The body

Six revolute joints, one per actuator, in the order of every register group of the manuals: **`pinky`, `ring`, `middle`, `index`, `thumb_bend`, `thumb_rotation`**. A position is the angle the hand's ANGLE registers
stand for, in radians, between the two ends of the range the model's manual gives (`frames.PROFILES`). The registers are called dimensionless by the maker and the angles are given at their two ends: that the scale between
is linear, and (for the thumb) which end is 1000, are assumptions and README.md lists them. Nothing the hand says identifies its model, so `model` is the operator's claim and has no default; a hand of another build is
`custom` with a profile somebody measured.

# What it does not send

* **-1.** The manuals give -1 in ANGLE_SET as "no response", and give thumb rotation a range of 0 to 1000 with no -1 in the series manual. A joint a command does not apply is simply not written: a write covers a run of
  applied joints and no more.
* **SAVE, RESET_PARA, HAND_ID, REDU_RATIO, the IP registers, the action sequences.** They write the hand's flash or change how it is reached: a person's, at the bench.
* **A stop that the manual does not document.** The manuals document no stop. `stop_mode` has no default and accepts only `hold`: the hand's own ANGLE_ACT, written back into ANGLE_SET, which the manual defines as
  "the finger will move to the preset angle" and so is a target where the finger already is. A speed of zero (which the third-party clients use) is not in either manual and is not offered.

# The one thing it does every cycle

Three reads: ANGLE_ACT, then the three command groups as one span (ANGLE_SET, FORCE_SET, SPEED_SET: the hand's own record of what it was told, which shows a write that was acknowledged and not taken, and a power cycle that
gave the speed and force back to their defaults), then ERROR and STATUS (adjacent, so one read). A hand whose speed or force differ from the operator's is told them again; the maker documents no keep-alive and the driver
sends none.

# The stop

`emergency_stop` latches under the lock `write_command` checks and wakes the reading thread, and returns: it never touches the line (the RHAL's "never blocks" is 50 ms, F298). The reading thread writes the stop before
anything else, between two of its own reads if it has to: the hand's ANGLE_ACT read fresh and written back into ANGLE_SET, under the line's own lock so that it follows any write already in flight. If the line is down it
cannot be written, so the loop writes it as soon as the line is back and the latch holds meanwhile. `release_stop` clears the latch only after an exchange shows the hand's own record of its targets equal to what the stop
wrote. It is a **software stop**: the hand's own emergency stop is a circuit and there is none documented.
"""

from __future__ import annotations

import contextlib
import json
import math
import sys
import threading
import time

import skynet_rhal as rhal

from . import frames, link
from .frames import ADDR_ANGLE_ACT, ADDR_CLEAR_ERROR, ADDR_ERROR, ADDR_FORCE_ACT, ADDR_FORCE_CALIBRATION, ADDR_FORCE_SET, ADDR_SPEED_SET, ADDR_TEMP, ADDR_CURRENT, ADDR_ANGLE_SET, JOINTS, Reading

__all__ = ["BridgeRefusal", "InspireBridge", "TOKEN"]

TOKEN = "vendor.inspire.task"

_RATE_HZ = 20.0
_MAX_RATE_HZ = 50.0
_TRANSACTION_S = 0.08
_OPEN_TIMEOUT_S = 2.0
_STALE_S = 0.5
#: The most a command may take to be written: the control loop waits for it, and a quarter of a second is the contract's bound (`tools/faultproxy/chaos.py`, F256).
_COMMAND_BUDGET_S = 0.25
_FAULT_THRESHOLD = 3
_INITIAL_READ_ATTEMPTS = 3
#: A target the hand's own record does not show within this long is a command it did not take (F175).
_ECHO_GRACE_S = 0.5
#: A finger the status calls "stopped after reaching the target position" is believed to have arrived only if its angle is this close to the target, in registers (5% of the range). A hand whose status lags its target (a firmware cycle between
#: the two) shows the old status beside the new target, and a move that took that for arrival would return while the fingers had not moved. The figure is this driver's own and the manuals give none: a hand that rests further from
#: its target than this is a move that ends in a timeout naming the angles, never in a success the angles do not show.
_ARRIVAL_TOLERANCE = 50
#: Series manual §2.4.6: the calibration "will take 6 seconds". The DFTP manual gives no figure; the margin is the third-party client's own timeout (8 s).
_CALIBRATION_S = 8.0
_BAUDS = (115200, 57600, 19200, 921600)   # series manual §2.4.2 (0 to 2) and DFTP manual §2.6.2 (0 to 3)
_KNOWN_KEYS = {"device", "baud", "host", "port", "hand_id", "model", "profile", "protocol", "simulator", "stop_mode", "speed", "force_g", "limits", "roles", "rate_hz", "timeout_ms", "stale_ms", "open_timeout_ms"}
_PROFILE_KEYS = ("angle_deg", "full_range_s", "force_set_max_g")
_MOTIONS = ("status", "move", "clear_errors", "calibrate_force")
_REOPEN_S = 0.5


class BridgeRefusal(Exception):
    """A reason this body did not come up or a command did not happen, in the terms an integrator acts on."""

    def __init__(self, message: str, code: int = rhal.SKR_ENODEV) -> None:
        super().__init__(message)
        self.code = code


class _StopOwed(Exception):
    """A reading cycle was left between two of its reads because a stop has to be written first."""


class Unreachable(BridgeRefusal):
    """The hand did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""

    def __init__(self, message: str) -> None:
        super().__init__(message, rhal.SKR_EIO)


class InspireBridge:
    """A body made of one hand's six actuators."""

    #: A stop written to the host while a motion runs is read at once: `emergency_stop` is safe from another thread (python-host's `stop_during_extension`).
    stop_during_extension = True

    def __init__(self) -> None:
        self._opened = False
        self._estop = False
        self._descriptor: str | None = None
        self._seq = 0
        self._simulated = True
        self._profile: frames.Profile = frames.PROFILES["RH56"]
        self._model = ""
        self._hand = 1
        self._limits: list[tuple[float, float]] = []
        self._speed = 0.0
        self._speed_register = 0
        self._force_g = [0] * 6
        self._roles: dict[str, str | None] = {}
        self._rate_hz = _RATE_HZ
        self._stale_s = _STALE_S
        self._timeout = _TRANSACTION_S
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
        self._image: list[int] = [0] * 6
        self._latest: Reading | None = None
        self._stamp_ns = 0
        self._exchanges = 0
        self._failures = 0
        self._dead: str | None = None
        self._commanded_at = 0.0
        self._params_dirty = True
        self._hold: list[int] | None = None
        self._stop_delivered = False
        #: Counts the calls of `emergency_stop`: a hold that was read before a later call is not that call's hold.
        self._stop_calls = 0
        self._calibrating_until = 0.0
        self._reasserted = 0
        self._needs_seed = False

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
        except (OSError, frames.ProtocolError, link.LinkError) as error:
            self._teardown()
            _say(f"the hand could not be read: {error}")
            return rhal.SKR_EIO
        self._opened = True
        return rhal.SKR_OK

    def _reset_session(self) -> None:
        """A session starts with no stop, no fault remembered, no calibration and no reading: nothing of the last one is the hand's."""
        self._estop = False
        self._descriptor = None
        self._last_refusal = None
        with self._lock:
            self._latest = None
            self._hold = None
            self._stop_delivered = False
            self._calibrating_until = 0.0
            self._params_dirty = True
            self._needs_seed = False
            self._reasserted = 0

    def _open(self, settings: dict) -> None:
        unknown = sorted(set(settings) - _KNOWN_KEYS)
        if unknown:
            raise BridgeRefusal(f"the configuration has {', '.join(map(repr, unknown))}, which this driver does not read. The keys it reads are {', '.join(sorted(_KNOWN_KEYS))}.")
        protocol = settings.get("protocol")
        if protocol != "private":
            raise BridgeRefusal(
                f"`protocol` of {protocol!r} is not `private`. This driver speaks the RS-485 private protocol (`EB 90` frames, series manual §2.2) and nothing else. The manuals describe Modbus RTU and Modbus TCP on the same "
                "registers, and this driver refuses them by name instead of running something else: the manuals say the registers are addressed by byte and give no byte order for the byte registers (ERROR, STATUS, TEMP) when "
                "they are carried in sixteen-bit Modbus registers, so a driver that read the fault bytes that way would be guessing at the one thing it must not guess. Say `\"protocol\": \"private\"`.")
        device, host = settings.get("device"), settings.get("host")
        if (device is None) == (host is None):
            raise BridgeRefusal("the configuration names neither a serial `device` nor a `host` and `port`, or names both. This driver does not scan for a hand, and one that guessed a port would command whichever device "
                                "answered first: give the hand's RS-485 adapter (`device`: /dev/ttyUSB0) or the RS-485 gateway that forwards its line raw (`host` and `port`).")
        baud = settings.get("baud", 115200)
        if device is not None:
            if not isinstance(device, str) or not device:
                raise BridgeRefusal(f"`device` of {device!r} is not a path.")
            if isinstance(baud, bool) or not isinstance(baud, int) or baud not in _BAUDS:
                raise BridgeRefusal(f"`baud` of {baud!r} is not a speed the hand can be set to ({', '.join(map(str, _BAUDS))}): it must match the hand's own setting, 115200 from the factory.")
            self._link = link.Link(device=device, baud=baud)
        else:
            port = settings.get("port")
            if not isinstance(host, str) or not host or isinstance(port, bool) or not isinstance(port, int) or not 0 < port < 65536:
                raise BridgeRefusal(f"`host` of {host!r} and `port` of {port!r} do not name a gateway: a host name and a port from 1 to 65535.")
            self._link = link.Link(host=host, port=port)
        hand = settings.get("hand_id", 1)
        if isinstance(hand, bool) or not isinstance(hand, int) or not 1 <= hand <= 254:
            raise BridgeRefusal(f"`hand_id` of {hand!r} is not 1 to 254 (the hand's factory setting is 1).")
        self._hand = hand
        self._profile = self._load_profile(settings)
        stop = settings.get("stop_mode")
        if stop != "hold":
            raise BridgeRefusal(
                "`stop_mode` is not in the configuration, or is not `hold`. The manuals document no stop for this hand, and what stopping it means is a person's decision. `hold` writes the hand's own measured angles back "
                "as its targets, which leaves every finger where it is and does not release a grasp. A speed of zero, which the third-party clients use, is in neither manual and is not offered.")
        claim = settings.get("simulator")
        if not isinstance(claim, bool):
            raise BridgeRefusal("`simulator` is not in the configuration, or is not true or false. Nothing the hand says tells a real one from a simulation, so the descriptor cannot be left to guess: a bench run "
                                "described as a simulation costs a tier it could have had, and a simulation described as a bench run is a false claim.")
        self._simulated = claim
        speed = settings.get("speed")
        if isinstance(speed, bool) or not isinstance(speed, (int, float)) or not math.isfinite(speed) or not 0.0 < speed <= 1.0:
            raise BridgeRefusal(f"no `speed` in the configuration, or {speed!r} is not a number above 0 and at most 1: the fraction of the hand's top speed (SPEED_SET 1000, which crosses a joint's whole range in "
                                f"{self._profile.full_range_s:g} s with no load). How fast the fingers close is the operator's, nothing in a position request says it, and a speed of zero is not documented.")
        self._speed = float(speed)
        self._speed_register = int(round(self._speed * frames.SPEED_REGISTER_MAX))
        self._force_g = self._load_force(settings.get("force_g"))
        self._limits = self._load_limits(settings.get("limits"))
        roles = settings.get("roles") or {}
        if not isinstance(roles, dict) or set(roles) - set(JOINTS):
            raise BridgeRefusal(f"`roles` is a mapping from a joint's name to its role, and the joints are {', '.join(JOINTS)}.")
        self._roles = {name: roles.get(name) for name in JOINTS}
        self._rate_hz = _bounded(settings, "rate_hz", _RATE_HZ, 1.0, _MAX_RATE_HZ)
        self._timeout = _bounded(settings, "timeout_ms", _TRANSACTION_S * 1000, 10.0, 5000.0) / 1000.0
        self._stale_s = _bounded(settings, "stale_ms", _STALE_S * 1000, 20.0, 30000.0) / 1000.0
        open_s = _bounded(settings, "open_timeout_ms", _OPEN_TIMEOUT_S * 1000, 100.0, 60000.0) / 1000.0

        assert self._link is not None
        self._link.open(timeout=min(open_s, 3.0))
        reading = self._first_reading(open_s)
        with self._lock:
            self._exchanges, self._failures, self._dead = 0, 0, None
            # What the hand is already doing is not disturbed: the image is the hand's own record of its targets, and a joint with no target (-1) is where it is.
            self._image = [t if t >= 0 else a for t, a in zip(reading.target, reading.angle)]
        fault = frames.joint_fault(reading.error, reading.status)
        if fault is not None:
            _say(f"the hand reports {frames.describe_joint_fault(*fault)}: commands are refused until it is cleared (the `clear_errors` motion; an over-temperature error clears itself when the actuator cools).")
        self._assert_parameters()
        # The state a caller first sees is the hand after it was told the operator's speed and force, so it is not degraded for a record that was true a moment ago.
        reading = self._first_reading(open_s)
        with self._lock:
            self._latest, self._stamp_ns = reading, rhal.acquisition_ns()
        self._closing.clear()
        self._kick.clear()
        self._loop = threading.Thread(target=self._exchange_loop, daemon=True, name="inspire-exchange")
        self._loop.start()
        self._descriptor = self._build_descriptor()

    def _load_profile(self, settings: dict) -> frames.Profile:
        model = settings.get("model")
        given = settings.get("profile")
        if model in frames.PROFILES:
            if given is not None:
                raise BridgeRefusal(f"`profile` was given with `model` {model!r}, whose profile is the maker's. Use `model: \"custom\"` for a profile you measured yourself.")
            self._model = model
            return frames.PROFILES[model]
        if model != "custom":
            raise BridgeRefusal(f"`model` of {model!r} is not in the configuration, or is not one of {', '.join(map(repr, [*frames.PROFILES, 'custom']))}. Nothing the hand says identifies its model, and what a register means "
                                "depends on it: the series manual (the DF and BF hands) and the DFTP manual give different ranges for the fingers and the thumb, a different speed and a different force range. Say which this is.")
        if not isinstance(given, dict) or set(given) != set(_PROFILE_KEYS):
            raise BridgeRefusal(f"`model: \"custom\"` needs a `profile` with exactly {', '.join(_PROFILE_KEYS)}: six [register 0, register 1000] angle pairs in degrees, the seconds a joint takes to cross its range at "
                                "speed 1000 with no load, and the largest force register in grams.")
        pairs = given["angle_deg"]
        if not (isinstance(pairs, list) and len(pairs) == 6 and all(isinstance(p, list) and len(p) == 2 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) for v in p) for p in pairs)):
            raise BridgeRefusal("`profile.angle_deg` is six [degrees at register 0, degrees at register 1000] pairs, in the order " + ", ".join(JOINTS) + ".")
        if any(p[0] == p[1] or abs(p[0]) > 360 or abs(p[1]) > 360 for p in pairs):
            raise BridgeRefusal("`profile.angle_deg` has a pair that is not a range of at most 360 degrees either way.")
        seconds, force_max = given["full_range_s"], given["force_set_max_g"]
        if isinstance(seconds, bool) or not isinstance(seconds, (int, float)) or not math.isfinite(seconds) or not 0.05 <= seconds <= 60:
            raise BridgeRefusal(f"`profile.full_range_s` of {seconds!r} is not a number of seconds from 0.05 to 60.")
        if isinstance(force_max, bool) or not isinstance(force_max, int) or not 1 <= force_max <= 32767:
            raise BridgeRefusal(f"`profile.force_set_max_g` of {force_max!r} is not a whole number of grams from 1 to 32767.")
        self._model = "custom"
        return frames.Profile("custom", "the operator's measurement", tuple((float(p[0]), float(p[1])) for p in pairs), float(seconds), force_max)

    def _load_force(self, given: object) -> list[int]:
        top = self._profile.force_set_max_g
        what = f"grams from 0 to {top}: the force the fingertips may close on what they meet with (FORCE_SET). How hard to hold is the operator's, and a held part is told it from the first command"
        values = given if isinstance(given, list) else [given] * 6
        if len(values) != 6 or any(isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) or not 0 <= v <= top or float(v) != int(v) for v in values):
            raise BridgeRefusal(f"no `force_g` in the configuration, or {given!r} is not a whole number of {what}, once for all six joints or as a list of six.")
        return [int(v) for v in values]

    def _load_limits(self, given: object) -> list[tuple[float, float]]:
        full = []
        for joint in range(6):
            low, high = self._profile.angle_deg[joint]
            full.append((math.radians(min(low, high)), math.radians(max(low, high))))
        if given is None:
            return full
        if not isinstance(given, dict) or set(given) - set(JOINTS):
            raise BridgeRefusal(f"`limits` is a mapping from a joint's name to [min, max] in radians, and the joints are {', '.join(JOINTS)}.")
        out = list(full)
        for name, pair in given.items():
            joint = JOINTS.index(name)
            if not (isinstance(pair, (list, tuple)) and len(pair) == 2 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) for v in pair)):
                raise BridgeRefusal(f"`limits.{name}` is [min, max] in radians, as two numbers.")
            lo, hi = float(pair[0]), float(pair[1])
            if not lo < hi:
                raise BridgeRefusal(f"`limits.{name}` of [{lo}, {hi}] is not a range.")
            low, high = full[joint]
            if lo < low - 1e-9 or hi > high + 1e-9:
                raise BridgeRefusal(f"`limits.{name}` of [{lo:.6f}, {hi:.6f}] rad is wider than the joint's range in the profile, [{low:.6f}, {high:.6f}]: a configuration may narrow it and never widen it.")
            out[joint] = (lo, hi)
        return out

    def _first_reading(self, budget: float) -> Reading:
        """Up to three whole readings before the hand is called absent."""
        deadline = time.monotonic() + budget
        last: Exception | None = None
        for _ in range(_INITIAL_READ_ATTEMPTS):
            try:
                return self._read_all(max(0.01, min(self._timeout * 4, deadline - time.monotonic())))
            except (link.LinkError, frames.ProtocolError) as error:
                last = error
                if time.monotonic() >= deadline:
                    break
        assert self._link is not None
        raise Unreachable(f"no hand answered at id {self._hand} on {self._link.target} ({last}). Is it powered, and do the baud rate and the hand id match the hand's (115200 and 1 from the factory)?")

    def close(self) -> int:
        # A close writes nothing: the hand keeps the last target it was given, and what a stop does to a held object is the integrator's to decide (see `stop_mode`), never a close's.
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
            image, commanded_at, calibrating = list(self._image), self._commanded_at, self._calibrating_until
            reasserted_pending = self._params_dirty
        self._seq += 1
        out.seq = self._seq
        out.stamp_ns = stamp or now
        out.flags = rhal.HOMING_NOT_REQUIRED
        out.fault_code = 0
        out.position_rad = [math.nan] * 6
        out.velocity_rad_s = [math.nan] * 6
        out.effort_nm = [math.nan] * 6
        if reading is None:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        out.position_rad = [frames.register_to_angle(a, j, self._profile) for j, a in enumerate(reading.angle)]
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP
        fault = frames.joint_fault(reading.error, reading.status)
        if fault is not None:
            joint, status, error = fault
            out.flags |= rhal.FLAG_FAULT | rhal.FLAG_DEGRADED
            out.fault_code = (joint + 1) << 16 | status << 8 | error
        # F175: a target the hand acknowledged on the wire and then did not take. The hand's own record of its targets is read every cycle; one that is not what was written, a moment after, is a command that did not happen.
        if not self._estop and time.monotonic() - commanded_at > _ECHO_GRACE_S and any(t != i for t, i in zip(reading.target, image) if t >= 0):
            out.flags |= rhal.FLAG_DEGRADED
        if reasserted_pending or tuple(reading.speed) != (self._speed_register,) * 6 or list(reading.force_limit) != self._force_g:
            out.flags |= rhal.FLAG_DEGRADED
        if time.monotonic() < calibrating:
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
        if cmd.joint_count != 6:
            return self._refused("count", f"the command has {cmd.joint_count} joints and this body has six ({', '.join(JOINTS)}): nothing was sent.")
        if cmd.mode != rhal.MODE_POSITION:
            return self._refused("mode", f"the command is in mode {cmd.mode}, and this hand takes positions: a target angle per joint, and there is no velocity or torque route. Send positions.")
        if self._estop:
            return rhal.SKR_EPERM
        applied = [bool(cmd.mask[j]) if cmd.mask else True for j in range(6)]
        if not any(applied):
            return rhal.SKR_OK
        registers: dict[int, int] = {}
        # Every applied joint is checked before any is written: a command refused for one joint is refused whole (F281), and a refused command never changes what the hand is told.
        for joint in range(6):
            if not applied[joint]:
                continue
            value = cmd.values[joint]
            if not math.isfinite(value):
                return self._refused("finite", f"{JOINTS[joint]} was commanded to {value!r}, which is not finite: the command was refused whole and nothing was sent.")
            low, high = self._limits[joint]
            if not low - 1e-9 <= value <= high + 1e-9:
                return self._refused("range", f"{JOINTS[joint]} was commanded to {value:.6f} rad, outside its range [{low:.6f}, {high:.6f}]: the command was refused whole and nothing was sent.")
            register = frames.angle_to_register(min(max(value, low), high), joint, self._profile)
            if register is None:
                return self._refused("register", f"{JOINTS[joint]} at {value:.6f} rad has no register in this profile: nothing was sent.")
            registers[joint] = register
        try:
            self._gate()
            self._send_targets(registers)
        except BridgeRefusal as error:
            return self._refused("command:" + str(error.code) + ":" + error.args[0][:40], str(error), error.code)
        except frames.ProtocolError as error:
            return self._refused("protocol", str(error), rhal.SKR_EIO)
        self._last_refusal = None
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        """Latch, and wake the thread that writes the hold. The call does not touch the line: the RHAL's "never blocks" is 50 ms, and a line that is slow or dead would make a stop that wrote its own hold wait for it (the
        link-chaos contract found 101 ms behind a blackhole, F298). The latch is the stop: commands are refused from this instant, and the hold is on the wire as soon as the line carries it, which `status` reports."""
        with self._lock:
            self._estop = True
            self._stop_calls += 1
            self._stop_delivered = False
            self._hold = None
            self._changed.notify_all()
        self._kick.set()
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """Clear the session latch (§48.2) once the hand has been *seen* to take the stop: an exchange after the call shows its own record of its targets equal to what the stop wrote."""
        if self._link is None or self._loop is None:
            self._estop = False
            return rhal.SKR_OK
        try:
            self._wait_for_reading(lambda r: self._stop_delivered and self._hold is not None and list(r.target) == self._hold, _COMMAND_BUDGET_S * 4 + self._timeout * 6, honour_stop=False)
        except BridgeRefusal as error:
            _say(f"the stop stays latched: no exchange has shown the hand take it ({error}) Fix the line, or close and open the driver, which is a new session.")
            return rhal.SKR_EIO
        with self._lock:
            hold = list(self._hold or self._image)
            self._image = hold
            self._estop = False
            self._hold = None
            self._stop_delivered = False
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

    def _txn_read(self, address: int, length: int, budget: float | None = None) -> bytes:
        link_ = self._link
        if link_ is None:
            raise link.LinkError("the line is not open.")
        with self._link_lock:
            return link_.transact(frames.request_read(self._hand, address, length), self._hand, frames.CMD_READ, address, length, budget or self._timeout)

    def _txn_write(self, address: int, data: bytes, by: float | None = None) -> None:
        link_ = self._link
        if link_ is None:
            raise link.LinkError("the line is not open.")
        with self._line(by):
            link_.transact(frames.request_write(self._hand, address, data), self._hand, frames.CMD_WRITE, address, 0, self._budget(by))

    def _read_all(self, budget: float | None = None, between=None) -> Reading:
        """The cycle's three reads, checked: a register outside what the manual allows is a frame that is wrong, not a hand that is somewhere strange. `between` is called before the second and third, so that a stop
        that is owed goes out ahead of them."""
        angle = tuple(frames.shorts(self._txn_read(ADDR_ANGLE_ACT, frames.BLOCK_SHORTS, budget)))
        if between is not None:
            between()
        block = self._txn_read(frames.COMMAND_BLOCK_ADDRESS, frames.COMMAND_BLOCK_BYTES, budget)
        if between is not None:
            between()
        diagnostics = self._txn_read(ADDR_ERROR, 2 * frames.BLOCK_BYTES, budget)
        target = tuple(frames.shorts(block[0:12], signed=True))
        force_limit = tuple(frames.shorts(block[12:24], signed=True))
        speed = tuple(frames.shorts(block[36:48], signed=True))
        if any(not 0 <= a <= 1000 for a in angle):
            raise frames.ProtocolError(f"the hand reports angles {list(angle)}, and an angle is 0 to 1000 (series manual §2.4.15).")
        if any(not -1 <= t <= 1000 for t in target) or any(not 0 <= s <= 1000 for s in speed) or any(f < 0 for f in force_limit):
            raise frames.ProtocolError(f"the hand's record of its own targets, speeds or force limits is outside what the manual allows (targets {list(target)}, speeds {list(speed)}, force limits {list(force_limit)}).")
        return Reading(angle, target, force_limit, speed, tuple(diagnostics[:6]), tuple(diagnostics[6:]))

    def _assert_parameters(self, by: float | None = None) -> None:
        """Tell the hand the operator's force limits and speeds. Writes no target, so nothing moves. `by`: the time a caller who is owed an answer has."""
        self._txn_write(ADDR_FORCE_SET, frames.pack_shorts(self._force_g), by)
        self._txn_write(ADDR_SPEED_SET, frames.pack_shorts([self._speed_register] * 6), by)
        with self._lock:
            self._params_dirty = False

    def _gate(self) -> None:
        """What a command waits for: a hand that has been read, that is not calibrating, and that holds no fault."""
        with self._lock:
            reading, calibrating = self._latest, self._calibrating_until
        if reading is None:
            raise BridgeRefusal("the hand has not been read yet.", rhal.SKR_ETIMEDOUT)
        if time.monotonic() < calibrating:
            raise BridgeRefusal("the hand is calibrating its force sensors and moves by itself for about six seconds (series manual §2.4.6): a command now would be fought by the calibration. The command was not sent.", rhal.SKR_EPERM)
        fault = frames.joint_fault(reading.error, reading.status)
        if fault is not None:
            raise BridgeRefusal(f"the hand reports {frames.describe_joint_fault(*fault)}, and a hand holding a fault takes nothing: the `clear_errors` motion clears what the maker says is clearable, and an "
                                "over-temperature error clears itself when the actuator cools. The command was not sent.", rhal.SKR_EPERM)

    def _send_targets(self, registers: dict[int, int]) -> None:
        """Write the targets of the joints a command applies, one write per run of neighbours, and only those. Checks the latch again under the line's lock, so that a command that waited for a stop does not follow it."""
        deadline = time.monotonic() + _COMMAND_BUDGET_S
        try:
            if self._params_dirty:
                self._assert_parameters(deadline)
            for first, last in _runs(sorted(registers)):
                if self._estop:
                    raise BridgeRefusal("the stop is latched: nothing is written.", rhal.SKR_EPERM)
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise BridgeRefusal(f"the hand was not told in the {_COMMAND_BUDGET_S:.2f} s a command is given: a slow or dead line, not a refusal by the hand.", rhal.SKR_EIO)
                values = [registers[j] for j in range(first, last + 1)]
                with self._line(deadline):
                    if self._estop:
                        raise BridgeRefusal("the stop was latched while the command was being made.", rhal.SKR_EPERM)
                    assert self._link is not None
                    self._link.transact(frames.request_write_shorts(self._hand, ADDR_ANGLE_SET + 2 * first, values), self._hand, frames.CMD_WRITE, ADDR_ANGLE_SET + 2 * first, 0, self._budget(deadline))
                with self._lock:
                    for joint in range(first, last + 1):
                        self._image[joint] = registers[joint]
                    self._commanded_at = time.monotonic()
        except link.LinkError as error:
            # A command that ran out of time waiting for the line while a stop was being written was refused by the stop, not by a slow line: the reading thread holds the line for the hold's two transactions, and
            # which of the two gets it first is the scheduler's (it showed as one run in six under load). The latch decides, so the answer does not depend on the race.
            if self._estop:
                raise BridgeRefusal("the stop is latched: nothing was written.", rhal.SKR_EPERM) from error
            raise BridgeRefusal(f"the hand did not take the command ({error}): a slow or dead line, not a refusal by the hand. What the hand holds is its own record of its targets, which `status` shows.", rhal.SKR_EIO) from error

    def _deliver_stop(self) -> None:
        """The stop: the hand's own angles, read fresh, written back as its targets. Under the line's lock, so it follows any write already in flight. A call of `emergency_stop` that comes while this is being written is
        owed a hold read after it, so a delivery that began before it does not stand for it: the stop stays owed and the reading thread writes it again."""
        with self._lock:
            calls = self._stop_calls
        with self._link_lock:
            link_ = self._link
            if link_ is None:
                raise link.LinkError("the line is not open.")
            angle = frames.shorts(link_.transact(frames.request_read(self._hand, ADDR_ANGLE_ACT, 12), self._hand, frames.CMD_READ, ADDR_ANGLE_ACT, 12, self._timeout))
            if any(not 0 <= a <= 1000 for a in angle):
                raise frames.ProtocolError(f"the hand reports angles {angle}, and an angle is 0 to 1000.")
            link_.transact(frames.request_write_shorts(self._hand, ADDR_ANGLE_SET, angle), self._hand, frames.CMD_WRITE, ADDR_ANGLE_SET, 0, self._timeout)
        with self._lock:
            if calls != self._stop_calls:
                return
            self._hold = list(angle)
            self._stop_delivered = True
            self._image = list(angle)
            self._changed.notify_all()

    def _exchange_loop(self) -> None:
        """The reading thread: the hand's own state in, once a period, forever; and, when something is owed, the stop or the operator's speed and force out."""
        assert self._link is not None
        link_ = self._link
        period = 1.0 / self._rate_hz
        reopened = 0.0
        open_error: str | None = None
        due = time.monotonic()
        while not self._closing.is_set():
            self._kick.wait(max(0.0, due - time.monotonic()))
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
            except (link.LinkError, frames.ProtocolError, BridgeRefusal) as error:
                reason = str(error)
                if not link_.is_open:
                    if time.monotonic() - reopened >= _REOPEN_S:
                        # A gateway that went away, or an adapter that was pulled out: the line is opened again, at most twice a second, and what it says is what is reported while it stays shut.
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
                        # A line that was dead is not a hand that was left as this driver last asked: it may have been power-cycled, which gives the speed and force back to their defaults.
                        self._params_dirty = True
                        self._needs_seed = True
                    self._changed.notify_all()
            else:
                with self._lock:
                    reseed = self._needs_seed
                    self._latest, self._stamp_ns = reading, rhal.acquisition_ns()
                    self._exchanges += 1
                    self._failures, self._dead = 0, None
                    if reseed:
                        self._needs_seed = False
                        if not self._estop:
                            self._image = [t if t >= 0 else a for t, a in zip(reading.target, reading.angle)]
                    self._changed.notify_all()
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
        """What a good reading shows is owed: a stop that has not been written, or the operator's speed and force where the hand's own record differs."""
        with self._lock:
            owed_stop = self._estop and not self._stop_delivered
            differs = tuple(reading.speed) != (self._speed_register,) * 6 or list(reading.force_limit) != self._force_g
            if differs:
                self._params_dirty = True
            dirty = self._params_dirty
        if owed_stop:
            self._deliver_stop()
        elif dirty:
            self._assert_parameters()
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
        joints = []
        for joint, name in enumerate(JOINTS):
            low, high = self._limits[joint]
            span = math.radians(abs(self._profile.angle_deg[joint][1] - self._profile.angle_deg[joint][0]))
            joints.append({"name": name, "role": self._roles.get(name), "type": "revolute",
                           "limits": {"position_min_rad": low, "position_max_rad": high, "velocity_max_rad_s": self._speed * span / self._profile.full_range_s}, "control_modes": ["position"]})
        effector = {"name": "hand", "role": None, "kind": "dexterous_hand", "dof": 6, "max_grip_force_n": None, "max_opening_m": None}
        descriptor = {
            "schema_version": 1, "robot_slug": "inspire-" + self._model.lower(), "digest": None, "verified": False, "simulator": self._simulated, "base": "fixed", "dof": 6,
            "joints": joints, "end_effectors": [effector], "sensors": [],
            "control": {"modes": ["position"], "rate_hz_max": self._rate_hz, "rate_hz_min": 1.0},
            # Null: the range comes from a manual's two ends and the speed from its no-load figure, and none of it is a risk assessment (§5.2); and no force in newtons, because the hand takes grams of fingertip force
            # and says how a part other than the fingertip changes it (series manual §2.4.9), which this driver cannot see.
            "envelope": None, "capabilities": ["safety.estop"], "motions": [_descriptor(name, self._model) for name in _MOTIONS], "extensions": [TOKEN], "solvers": [],
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
        except frames.ProtocolError as error:
            return rhal.SKR_EINVAL, f"the request does not fit the wire: {error}".encode()
        except link.LinkError as error:
            return rhal.SKR_EIO, f"the hand did not carry the request: {error}".encode()

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
            raise BridgeRefusal("the hand has not been read yet.", rhal.SKR_ETIMEDOUT)
        return reading

    def _motion_status(self, args: dict) -> dict:
        self._no_arguments("status", args)
        reading = self._present()
        with self._lock:
            image, exchanges, dead = list(self._image), self._exchanges, self._dead
            delivered, hold, reasserted, calibrating = self._stop_delivered, self._hold, self._reasserted, self._calibrating_until
        extra: dict[str, object] = {}
        try:
            extra["force_g"] = frames.shorts(self._txn_read(ADDR_FORCE_ACT, 12), signed=True)
            extra["current_ma"] = frames.shorts(self._txn_read(ADDR_CURRENT, 12))
            extra["temperature_c"] = list(self._txn_read(ADDR_TEMP, 6))
        except (link.LinkError, frames.ProtocolError) as error:
            extra["read_error"] = str(error)
        fault = frames.joint_fault(reading.error, reading.status)
        return {"model": self._model, "hand_id": self._hand, "link": self._link.target if self._link else None, "joints": list(JOINTS),
                "angle_register": list(reading.angle), "position_rad": [frames.register_to_angle(a, j, self._profile) for j, a in enumerate(reading.angle)],
                "target_register": list(reading.target), "image": image, "speed_register": list(reading.speed), "force_limit_g": list(reading.force_limit),
                "status": [frames.STATUS_NAMES.get(s, f"code {s}, which the maker does not name") for s in reading.status], "status_code": list(reading.status), "error": [f"0x{e:02X}" for e in reading.error],
                "fault": frames.describe_joint_fault(*fault) if fault else None, "mapping": {"profile": self._profile.name, "source": self._profile.source, "angle_deg_at_register_0_and_1000": [list(p) for p in self._profile.angle_deg],
                                                                                                    "linear": "assumed", "thumb_orientation": "assumed: register 1000 is the larger angle for every joint"},
                "limits_rad": [list(pair) for pair in self._limits], "speed": self._speed, "force_limit_configured_g": list(self._force_g), "exchanges": exchanges, "link_dead": dead,
                "stop_latched": self._estop, "stop_delivered": delivered, "stop_hold": hold, "parameters_reasserted": reasserted, "calibrating": time.monotonic() < calibrating,
                "simulator": self._simulated, **extra}

    def _motion_move(self, args: dict) -> dict:
        timeout = self._number(args, "timeout", minimum=0.5, maximum=120.0)
        targets: dict[int, float] = {}
        for name in list(args):
            if name not in JOINTS:
                raise BridgeRefusal(f"`{name}` is not a joint ({', '.join(JOINTS)}) or `timeout`.", rhal.SKR_EINVAL)
            joint = JOINTS.index(name)
            low, high = self._limits[joint]
            value = self._number(args, name, minimum=low, maximum=high)
            assert value is not None
            targets[joint] = value
        if not targets:
            raise BridgeRefusal(f"`move` needs at least one joint, as `<joint>=<radians>`: {', '.join(JOINTS)}.", rhal.SKR_EINVAL)
        registers: dict[int, int] = {}
        for joint, value in targets.items():
            register = frames.angle_to_register(value, joint, self._profile)
            if register is None:
                raise BridgeRefusal(f"{JOINTS[joint]} at {value:.6f} rad has no register in this profile.", rhal.SKR_EINVAL)
            registers[joint] = register
        before = self._present()
        travel = max(abs(registers[j] - before.angle[j]) for j in registers) / frames.ANGLE_REGISTER_MAX
        wait = timeout if timeout is not None else travel * self._profile.full_range_s / self._speed + 3.0
        self._gate()
        self._send_targets(registers)

        def done(reading: Reading) -> bool:
            if frames.joint_fault(reading.error, reading.status) is not None:
                return True
            return all(reading.target[j] == registers[j] and (reading.status[j] == 3 or (reading.status[j] == 2 and abs(reading.angle[j] - registers[j]) <= _ARRIVAL_TOLERANCE)) for j in registers)

        reading = self._wait_for_reading(done, wait)
        # A cycle is three transactions, the angles first and the statuses last, so a finger that stopped between them is shown at the angle it still had beside the status that says it stopped. The next reading
        # is of a finger that has stopped; if the line cannot give one, the reading that ended the wait is reported as it was.
        try:
            later = self._wait_for_reading(lambda r: True, self._timeout * 6 + 1.0, honour_stop=False)
        except BridgeRefusal:
            later = reading
        if done(later):
            reading = later
        fault = frames.joint_fault(reading.error, reading.status)
        if fault is not None:
            raise BridgeRefusal(f"the hand reports {frames.describe_joint_fault(*fault)} during the move.", rhal.SKR_EIO)
        names = {j: JOINTS[j] for j in registers}
        return {"result": "arrived" if all(reading.status[j] == 2 for j in registers) else "stopped_on_an_object",
                "joints": {names[j]: {"angle_register": reading.angle[j], "position_rad": frames.register_to_angle(reading.angle[j], j, self._profile),
                                      "status": frames.STATUS_NAMES[reading.status[j]]} for j in registers}}

    def _motion_clear_errors(self, args: dict) -> dict:
        self._no_arguments("clear_errors", args)
        before = self._present()
        self._txn_write(ADDR_CLEAR_ERROR, b"\x01")
        try:
            after = self._wait_for_reading(lambda r: True, self._timeout * 6 + 1.0)
        except BridgeRefusal as error:
            raise BridgeRefusal(f"the clear was written and no reading has shown what it did ({error}).", rhal.SKR_EIO) from None
        left = frames.joint_fault(after.error, after.status)
        return {"result": "cleared" if left is None else "not_cleared", "before": [f"0x{e:02X}" for e in before.error],
                "after": [f"0x{e:02X}" for e in after.error], "remaining": frames.describe_joint_fault(*left) if left else None,
                "note": None if left is None else "an over-temperature error is not clearable and clears itself when the actuator cools; a locked rotor, an over-current or an abnormal operation that is still there is back as soon as it is commanded"}

    def _motion_calibrate_force(self, args: dict) -> dict:
        confirm = args.pop("confirm", None)
        self._no_arguments("calibrate_force", args)
        if confirm != "unloaded":
            raise BridgeRefusal("`calibrate_force` moves the hand by itself for about six seconds (series manual §2.4.6: five fingers open, four fingers bent and opened, the thumb bent and extended) and the hand "
                                "must hold nothing and touch nothing while it does. Say `confirm=unloaded` when that is so.", rhal.SKR_EINVAL)
        reading = self._present()
        fault = frames.joint_fault(reading.error, reading.status)
        if fault is not None:
            raise BridgeRefusal(f"the hand reports {frames.describe_joint_fault(*fault)}: it is not calibrated while it holds a fault. The `clear_errors` motion first.", rhal.SKR_EPERM)
        with self._lock:
            self._calibrating_until = time.monotonic() + _CALIBRATION_S
        try:
            self._txn_write(ADDR_FORCE_CALIBRATION, b"\x01")
        except (link.LinkError, frames.ProtocolError):
            with self._lock:
                self._calibrating_until = 0.0
            raise
        started = time.monotonic()
        while time.monotonic() < started + _CALIBRATION_S:
            if self._estop:
                with self._lock:
                    self._calibrating_until = 0.0
                raise BridgeRefusal("the calibration was left: the stop was latched. The hand was told to hold where it was; whether that ends a calibration is not in the manual, so the hand may "
                                    "still be moving by itself.", rhal.SKR_EPERM)
            if self._closing.wait(0.05):
                raise BridgeRefusal("the driver was closed during the calibration.", rhal.SKR_EPERM)
        with self._lock:
            self._calibrating_until = 0.0
        after = self._wait_for_reading(lambda r: True, self._timeout * 6 + 1.0)
        left = frames.joint_fault(after.error, after.status)
        return {"result": "calibrated" if left is None else "ended_with_a_fault", "seconds": round(time.monotonic() - started, 3), "fault": frames.describe_joint_fault(*left) if left else None}


def _descriptor(name: str, model: str) -> dict:
    del model
    if name == "status":
        return {"name": "status", "parameters": []}
    if name == "move":
        return {"name": "move", "parameters": [*({"name": joint, "unit": "rad", "required": False} for joint in JOINTS), {"name": "timeout", "unit": "s", "required": False}]}
    if name == "clear_errors":
        return {"name": "clear_errors", "parameters": []}
    return {"name": "calibrate_force", "parameters": [{"name": "confirm", "required": True}]}


def _runs(joints: list[int]) -> list[tuple[int, int]]:
    """Maximal runs of consecutive joint indices, as (first, last)."""
    out: list[tuple[int, int]] = []
    for joint in joints:
        if out and out[-1][1] == joint - 1:
            out[-1] = (out[-1][0], joint)
        else:
            out.append((joint, joint))
    return out


def _bounded(settings: dict, key: str, default: float, low: float, high: float) -> float:
    value = settings.get(key)
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not low <= value <= high:
        raise BridgeRefusal(f"`{key}` of {value!r} is not a number from {low:g} to {high:g}.")
    return float(value)


def _say(message: str) -> None:
    print(f"skynet-inspire: {message}", file=sys.stderr, flush=True)
