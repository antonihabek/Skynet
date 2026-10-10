"""The RHAL driver: a LEAP Hand (sixteen Dynamixel servos on one bus) over protocol 2.0 (DRY-COVERAGE-PLAN.md item 12.E).

§4's five mandatory entry points over :mod:`skynet_leap.frames` and :mod:`skynet_leap.link`. It links nothing of ROBOTIS's or of the LEAP Hand authors': the protocol and the control table are the e-manual's, the hand's
joint order, ranges and zero are the authors' (`leap_hand_utils.py`), and `layout_check.py` holds this driver's bytes to ROBOTIS's own `dynamixel_sdk`, which it runs (and, where a person points it at a checkout, to the
authors' own client, whose licence is non-commercial).

# The body

Sixteen revolute joints in the authors' order, an index, middle and ring finger and a thumb of four joints each: the side-to-side MCP, the forward MCP, the PIP and the DIP. A position is the joint's angle in the authors'
"sim" convention (0 is the extended, allegro-like zero; closing is positive), in radians; the servo's own angle is that plus 3.14159 (`LEAPsim_to_LEAPhand`). It takes a position, one control mode, and its tier ceiling
is Guest (§31.3). The servos are in current-based position control (operating mode 5): a goal position, capped by a goal current that is the operator's (`current_ma`, required: nothing in a position request says how hard
the fingers may push).

# The stop

`stop_mode` is required and accepts only `hold`: every servo's own present position, read fresh, written back as its goal. Torque stays on. A stop that cut torque would let a hand holding anything drop it, and what a
stop should do to a held object is the integrator's risk assessment. It is a **software stop**: the servos have no stop input. `emergency_stop` latches and returns and never touches the line (the RHAL's "never blocks"
is 50 ms, and a line that is slow or dead would make a stop that wrote its own hold wait for it); the reading thread writes the hold before anything else, and a call that comes while an earlier hold is being written is
owed a hold read after it.

# What it never does on its own

It does not change a servo's EEPROM while the servo holds torque (the e-manual locks it), it does not switch torque on for a servo that was off when it found it except by the `enable_torque` motion (a hand that was
limp is a person's to decide about), and it does not reboot a servo (that drops its torque and its gains) except by `clear_errors` with `confirm=limp`.
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
from .frames import IDS, JOINTS, Reading

__all__ = ["BridgeRefusal", "LeapBridge", "TOKEN"]

TOKEN = "vendor.leap.task"

_RATE_HZ = 50.0
_MAX_RATE_HZ = 100.0
_TRANSACTION_S = 0.05
_OPEN_TIMEOUT_S = 3.0
_STALE_S = 0.5
#: The most a command may take to be written: the control loop waits for it, and a quarter of a second is the contract's bound (`tools/faultproxy/chaos.py`, F256).
_COMMAND_BUDGET_S = 0.25
_FAULT_THRESHOLD = 3
_INITIAL_READ_ATTEMPTS = 3
#: A joint within this many ticks (about 1 degree) of its goal is there. The e-manual gives no figure for how close a servo in current-based position control rests; this is this driver's own.
_ARRIVAL_TOLERANCE = 12
#: How long a goal the servo does not show yet is given before the hand is called not to have taken it.
_ECHO_GRACE_S = 0.5
_REBOOT_S = 0.6
#: Operating mode 5 holds a position of -256 to 256 turns (e-manual); a reading outside that is not a position.
_MULTI_TURN_TICKS = 256 * 4096
_KNOWN_KEYS = {"device", "host", "port", "baud", "variant", "stop_mode", "current_ma", "gains", "side_scale", "simulator", "limits", "roles", "rate_hz", "timeout_ms", "stale_ms", "open_timeout_ms"}
_MOTIONS = ("status", "move", "enable_torque", "clear_errors")
#: The side-to-side MCP joints, which the maker's example drives with three quarters of the gain.
_SIDE_JOINTS = (0, 4, 8)


class BridgeRefusal(Exception):
    """A reason this body did not come up or a command did not happen, in the terms an integrator acts on."""

    def __init__(self, message: str, code: int = rhal.SKR_ENODEV) -> None:
        super().__init__(message)
        self.code = code


class Unreachable(BridgeRefusal):
    """The servos did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""

    def __init__(self, message: str) -> None:
        super().__init__(message, rhal.SKR_EIO)


class LeapBridge:
    """A body made of a LEAP Hand's sixteen joints."""

    #: A stop written to the host while a motion runs is read at once: `emergency_stop` is safe from another thread (python-host's `stop_during_extension`).
    stop_during_extension = True

    def __init__(self) -> None:
        self._opened = False
        self._estop = False
        self._descriptor: str | None = None
        self._seq = 0
        self._simulated = True
        self._variant: frames.Variant = frames.VARIANTS["full"]
        self._current_ma = 0
        self._gains = (0, 0, 0)
        self._side_scale = 1.0
        self._limits: list[tuple[float, float]] = [(a, b) for a, b in zip(frames.LIMITS_MIN, frames.LIMITS_MAX)]
        self._roles: dict[str, str | None] = {}
        self._rate_hz = _RATE_HZ
        self._stale_s = _STALE_S
        self._timeout = _TRANSACTION_S
        self._firmware: list[int] = []
        self._current_limit: list[int] = []
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
        #: The goal registers this driver last asked for, per joint.
        self._image: list[int] = [0] * 16
        self._commanded_at = 0.0
        self._params_dirty = True
        self._hold: list[int] | None = None
        self._stop_delivered = False
        #: Counts the calls of `emergency_stop`: a hold that was read before a later call is not that call's hold.
        self._stop_calls = 0
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
        """A session starts with no stop, no fault remembered and no reading: nothing of the last one is the hand's."""
        self._estop = False
        self._descriptor = None
        self._last_refusal = None
        with self._lock:
            self._latest = None
            self._hold = None
            self._stop_delivered = False
            self._params_dirty = True
            self._needs_seed = False
            self._reasserted = 0

    def _open(self, settings: dict) -> None:
        unknown = sorted(set(settings) - _KNOWN_KEYS)
        if unknown:
            raise BridgeRefusal(f"the configuration has {', '.join(map(repr, unknown))}, which this driver does not read. The keys it reads are {', '.join(sorted(_KNOWN_KEYS))}.")
        variant = settings.get("variant")
        if variant not in frames.VARIANTS:
            raise BridgeRefusal(f"`variant` of {variant!r} is not in the configuration, or is not one of {', '.join(map(repr, frames.VARIANTS))}. The Lite has XL330-M288-T servos and the Full XC330-M288-T, "
                                "nothing the bus says tells a hand's builder which it is, and the maker suggests a different current limit for each, so the operator says which it is and the open checks the servos agree.")
        self._variant = frames.VARIANTS[variant]
        host, device = settings.get("host"), settings.get("device")
        if (host is None) == (device is None):
            raise BridgeRefusal("the configuration names neither a serial `device` nor a gateway (`host`, and `port`), or names both. This driver does not scan for a hand.")
        baud = settings.get("baud", 4000000)
        if host is not None:
            port = settings.get("port")
            if not isinstance(host, str) or not host or isinstance(port, bool) or not isinstance(port, int) or not 0 < port < 65536:
                raise BridgeRefusal(f"`host` of {host!r} and `port` of {port!r} do not name a gateway: a host name and a port from 1 to 65535.")
            self._link = link.Link(host=host, port=port)
        else:
            if not isinstance(device, str) or not device:
                raise BridgeRefusal(f"`device` of {device!r} is not a path.")
            if isinstance(baud, bool) or not isinstance(baud, int) or baud not in link.BAUDS:
                raise BridgeRefusal(f"`baud` of {baud!r} is not one of {', '.join(map(str, link.BAUDS))}: the speeds the servos' Baud Rate register can be set to (the maker runs the hand at 4000000).")
            self._link = link.Link(device=device, baud=baud)
        if settings.get("stop_mode") != "hold":
            raise BridgeRefusal("`stop_mode` is not in the configuration, or is not `hold`. A hand has no stop input, and a stop that cut the servos' torque would let go of whatever the fingers hold: "
                                "every servo's own position is written back as its goal, torque on. Say `\"stop_mode\": \"hold\"` to take it.")
        claim = settings.get("simulator")
        if not isinstance(claim, bool):
            raise BridgeRefusal("`simulator` is not in the configuration, or is not true or false. Nothing the servos say tells a real hand from a simulation, so the descriptor cannot be left to guess.")
        self._simulated = claim
        current = settings.get("current_ma")
        if isinstance(current, bool) or not isinstance(current, int) or not 1 <= current <= self._variant.max_current_ma:
            raise BridgeRefusal(f"no `current_ma` in the configuration, or {current!r} is not a whole number of milliamps from 1 to {self._variant.max_current_ma}: the goal current that caps how hard every servo pushes toward "
                                f"its position. How hard the fingers may press is the operator's; the maker's readme suggests about 300 for the Lite and up to 550 for the Full, and {self._variant.max_current_ma} is the most this driver takes "
                                f"for a {self._variant.name} hand.")
        self._current_ma = current
        gains = settings.get("gains")
        if (not isinstance(gains, dict) or set(gains) != {"kp", "ki", "kd"} or not all(isinstance(v, int) and not isinstance(v, bool) and 0 <= v <= 16383 for v in gains.values())):
            raise BridgeRefusal(f"`gains` is not in the configuration, or is not {{\"kp\": …, \"ki\": …, \"kd\": …}} with whole numbers from 0 to 16383 (got {gains!r}): the position loop's P, I and D, which decide whether the hand is "
                                "too weak or jitters, and which the maker's example sets to 600, 0 and 200.")
        side = settings.get("side_scale", 1.0)
        if isinstance(side, bool) or not isinstance(side, (int, float)) or not 0 < side <= 1:
            raise BridgeRefusal(f"`side_scale` of {side!r} is not a number above 0 and at most 1: the fraction of the P and D gains the three side-to-side MCP joints take (the maker's example uses 0.75).")
        self._gains = (gains["kp"], gains["ki"], gains["kd"])
        self._side_scale = float(side)
        self._limits = self._narrowed(settings.get("limits"))
        roles = settings.get("roles") or {}
        if not isinstance(roles, dict) or set(roles) - set(JOINTS):
            raise BridgeRefusal(f"`roles` is a mapping from a joint's name to its role, and the joints are {', '.join(JOINTS)}.")
        self._roles = {name: roles.get(name) for name in JOINTS}
        self._rate_hz = _bounded(settings, "rate_hz", _RATE_HZ, 1.0, _MAX_RATE_HZ)
        self._timeout = _bounded(settings, "timeout_ms", _TRANSACTION_S * 1000, 5.0, 5000.0) / 1000.0
        self._stale_s = _bounded(settings, "stale_ms", _STALE_S * 1000, 20.0, 30000.0) / 1000.0
        open_s = _bounded(settings, "open_timeout_ms", _OPEN_TIMEOUT_S * 1000, 100.0, 60000.0) / 1000.0

        assert self._link is not None
        self._link.open(timeout=min(open_s, 3.0))
        self._identify(open_s)
        self._configure(open_s)
        reading = self._first_reading(open_s)
        with self._lock:
            self._latest, self._stamp_ns = reading, rhal.acquisition_ns()
            self._exchanges, self._failures, self._dead = 0, 0, None
            self._image = list(reading.goal)
            self._params_dirty = False
        self._closing.clear()
        self._kick.clear()
        self._loop = threading.Thread(target=self._exchange_loop, daemon=True, name="leap-exchange")
        self._loop.start()
        self._descriptor = self._build_descriptor()

    def _narrowed(self, given: object) -> list[tuple[float, float]]:
        limits = [(a, b) for a, b in zip(frames.LIMITS_MIN, frames.LIMITS_MAX)]
        if given is None:
            return limits
        if not isinstance(given, dict) or set(given) - set(JOINTS):
            raise BridgeRefusal(f"`limits` is a mapping from a joint's name to [min, max] in radians, and the joints are {', '.join(JOINTS)}.")
        for name, pair in given.items():
            joint = JOINTS.index(name)
            if not (isinstance(pair, (list, tuple)) and len(pair) == 2 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) for v in pair)):
                raise BridgeRefusal(f"`limits` of {name} is [min, max] in radians, as two numbers.")
            low, high = float(pair[0]), float(pair[1])
            if not low < high:
                raise BridgeRefusal(f"`limits` of {name} is [{low}, {high}], which is not a range.")
            full_low, full_high = limits[joint]
            if low < full_low - 1e-9 or high > full_high + 1e-9:
                raise BridgeRefusal(f"`limits` of {name} is [{low}, {high}], wider than the hand's [{full_low}, {full_high}] rad (leap_hand_utils.LEAPsim_limits): a configuration may narrow a joint's range and never widen it.")
            limits[joint] = (low, high)
        return limits

    # ── bringing the hand up ─────────────────────────────────────────────────

    def _txn(self, request: bytes, ids: tuple[int, ...], budget: float | None = None) -> dict[int, frames.StatusPacket]:
        link_ = self._link
        if link_ is None:
            raise link.LinkError("the line is not open.")
        with self._link_lock:
            return link_.transact(request, ids, budget or self._timeout)

    def _sync_read(self, address: int, length: int, budget: float | None = None) -> dict[int, bytes]:
        answers = self._txn(frames.sync_read(address, length, IDS), IDS, budget)
        data: dict[int, bytes] = {}
        for servo in IDS:
            status = answers[servo]
            if status.error_number:
                raise frames.ProtocolError(f"servo {servo} answered a read of {length} bytes at {address} with {frames.ERROR_NUMBERS.get(status.error_number, status.error_number)}.")
            if len(status.params) != length:
                raise frames.ProtocolError(f"servo {servo} answered a read of {length} bytes at {address} with {len(status.params)}.")
            data[servo] = status.params
        return data

    def _sync_write(self, address: int, length: int, values: dict[int, bytes], by: float | None = None) -> None:
        request = frames.sync_write(address, length, values)
        link_ = self._link
        if link_ is None:
            raise link.LinkError("the line is not open.")
        with self._line(by):
            link_.transact(request, (), self._budget(by))

    def _identify(self, open_s: float) -> None:
        """Every servo must answer, and be the model the operator said; the EEPROM area (identity, operating mode, current limit, shutdown) is read once for the record."""
        deadline = time.monotonic() + open_s
        last: Exception | None = None
        eeprom: dict[int, bytes] = {}
        for _ in range(_INITIAL_READ_ATTEMPTS):
            try:
                eeprom = self._sync_read(0, 64, min(self._timeout * 6, max(0.01, deadline - time.monotonic())))
                break
            except (link.LinkError, frames.ProtocolError) as error:
                last = error
                if time.monotonic() >= deadline:
                    break
        if not eeprom:
            assert self._link is not None
            raise Unreachable(f"the servos did not answer on {self._link.target} ({last}). Is the hand powered, the adapter's baud rate the servos' (4000000 from the maker), and the USB latency timer set to 1 ms?")
        wrong = [(servo, frames.u16(eeprom[servo])) for servo in IDS if frames.u16(eeprom[servo]) != self._variant.model_number]
        if wrong:
            raise BridgeRefusal(f"servos {[servo for servo, _ in wrong]} report model numbers {[number for _, number in wrong]}, and a {self._variant.name} is {self._variant.model_number}. A register can mean something else on another servo, so nothing "
                                "was commanded: check `variant` against the hand.")
        self._firmware = [eeprom[servo][6] for servo in IDS]
        self._current_limit = [frames.u16(eeprom[servo], frames.ADDR_CURRENT_LIMIT) for servo in IDS]
        over = [servo for servo in IDS if self._current_ma > self._current_limit[servo]]
        if over:
            raise BridgeRefusal(f"`current_ma` of {self._current_ma} is above the Current Limit register of servos {over} ({[self._current_limit[s] for s in over]}), which the servo would refuse as a goal current.")
        self._eeprom = eeprom

    def _configure(self, open_s: float) -> None:
        """What the maker's example does at start, in an order that cannot move the hand: refuse a hand with a hardware error or in another mode while it holds torque, put a limp hand in current-based position control, tell
        every servo the operator's gains and goal current, and for a servo that was limp set its goal where it is before its torque goes on."""
        state = self._read_all(min(self._timeout * 6, open_s))
        broken = [(servo, state.hw_error[servo]) for servo in IDS if frames.joint_fault(state.hw_error[servo]) is not None]
        if broken:
            servo, bits = broken[0]
            raise BridgeRefusal(f"servo {servo} ({JOINTS[servo]}) reports {frames.describe_hw_error(bits)} (Hardware_Error_Status {bits:#04x}), and {len(broken)} servo(s) do: clear it with the `clear_errors` motion once the hand is open, "
                                "or power-cycle the hand. Nothing was commanded.", rhal.SKR_EPERM)
        modes = {servo: self._eeprom[servo][frames.ADDR_OPERATING_MODE] for servo in IDS}
        holding_wrong = [servo for servo in IDS if state.torque[servo] and modes[servo] != frames.OPERATING_MODE_CURRENT_POSITION]
        if holding_wrong:
            raise BridgeRefusal(f"servos {holding_wrong} hold torque in operating mode {[modes[s] for s in holding_wrong]}, and this driver commands current-based position control (mode 5). The EEPROM is locked while torque is on, "
                                "so it cannot be changed here; release them (torque off) and open again.", rhal.SKR_EPERM)
        limp = [servo for servo in IDS if not state.torque[servo]]
        wrong_mode = [servo for servo in limp if modes[servo] != frames.OPERATING_MODE_CURRENT_POSITION]
        if wrong_mode:
            self._sync_write(frames.ADDR_OPERATING_MODE, 1, {servo: bytes([frames.OPERATING_MODE_CURRENT_POSITION]) for servo in wrong_mode})
        self._assert_parameters()
        if limp:
            self._sync_write(frames.ADDR_GOAL_POSITION, 4, {servo: frames.pack_u32(state.position[servo]) for servo in limp})
            self._sync_write(frames.ADDR_TORQUE_ENABLE, 1, {servo: b"\x01" for servo in limp})

    def _expected_gains(self, servo: int) -> tuple[int, int, int]:
        """(P, I, D) for a joint: the operator's, and a fraction of P and D on the three side-to-side MCP joints."""
        kp, ki, kd = self._gains
        scale = self._side_scale if servo in _SIDE_JOINTS else 1.0
        return int(round(kp * scale)), ki, int(round(kd * scale))

    def _assert_parameters(self, by: float | None = None) -> None:
        """Tell every servo the operator's gains and goal current. Writes no goal, so nothing moves. `by`: the time a caller who is owed an answer has."""
        p = {servo: frames.pack_u16(self._expected_gains(servo)[0]) for servo in IDS}
        i = {servo: frames.pack_u16(self._expected_gains(servo)[1]) for servo in IDS}
        d = {servo: frames.pack_u16(self._expected_gains(servo)[2]) for servo in IDS}
        self._sync_write(frames.ADDR_POSITION_D, 2, d, by)
        self._sync_write(frames.ADDR_POSITION_I, 2, i, by)
        self._sync_write(frames.ADDR_POSITION_P, 2, p, by)
        self._sync_write(frames.ADDR_GOAL_CURRENT, 2, {servo: self._current_ma.to_bytes(2, "little", signed=True) for servo in IDS}, by)
        with self._lock:
            self._params_dirty = False

    def _first_reading(self, budget: float) -> Reading:
        """Up to three whole readings before the hand is called absent."""
        deadline = time.monotonic() + budget
        last: Exception | None = None
        for _ in range(_INITIAL_READ_ATTEMPTS):
            try:
                reading = self._read_all(max(0.01, min(self._timeout * 6, deadline - time.monotonic())))
            except (link.LinkError, frames.ProtocolError) as error:
                last = error
                if time.monotonic() >= deadline:
                    break
                continue
            if self._differs(reading):
                last = frames.ProtocolError("the servos do not show the gains and goal current that were written")
                time.sleep(min(self._timeout, 0.05))
                continue
            return reading
        assert self._link is not None
        raise Unreachable(f"no complete reading of the servos on {self._link.target} ({last}).")

    def close(self) -> int:
        # A close writes nothing: the servos keep the last goal they were given and their torque, and what stopping the hand does to a held object is the integrator's to decide (see `stop_mode`), never a close's.
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
            image, commanded_at = list(self._image), self._commanded_at
            dirty = self._params_dirty
        self._seq += 1
        out.seq = self._seq
        out.stamp_ns = stamp or now
        out.flags = rhal.HOMING_NOT_REQUIRED
        out.fault_code = 0
        out.position_rad = [math.nan] * 16
        out.velocity_rad_s = [math.nan] * 16
        out.effort_nm = [math.nan] * 16
        if reading is None:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        out.position_rad = [frames.register_to_position(p) for p in reading.position]
        out.velocity_rad_s = [v * frames.VELOCITY_RAD_S_PER_UNIT for v in reading.velocity]
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP
        for servo in IDS:
            bits = reading.hw_error[servo]
            if frames.joint_fault(bits) is not None:
                out.flags |= rhal.FLAG_FAULT | rhal.FLAG_DEGRADED
                out.fault_code = (servo + 1) << 16 | bits
                break
        # F175: a goal the servos acknowledged on the wire and then did not take. Their own record of their goals is read every cycle; one that is not what was written, a moment after, is a command that did not happen.
        if not self._estop and time.monotonic() - commanded_at > _ECHO_GRACE_S and any(goal != asked for goal, asked in zip(reading.goal, image)):
            out.flags |= rhal.FLAG_DEGRADED
        if any(not t for t in reading.torque) and out.fault_code == 0:
            out.flags |= rhal.FLAG_DEGRADED          # a limp servo takes no goal
        if dirty or self._differs(reading):
            out.flags |= rhal.FLAG_DEGRADED
        age = (rhal.acquisition_ns() - stamp) / 1e9
        if dead or age > self._stale_s:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def _differs(self, reading: Reading) -> bool:
        """The gains or the goal current a servo shows are not the operator's."""
        for servo in IDS:
            p, i, d = self._expected_gains(servo)
            if (reading.gain_p[servo], reading.gain_i[servo], reading.gain_d[servo]) != (p, i, d) or reading.goal_current[servo] != self._current_ma:
                return True
        return False

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened or self._link is None:
            return rhal.SKR_EPERM
        if cmd.joint_count != 16:
            return self._refused("count", f"the command has {cmd.joint_count} joints and this body has sixteen ({', '.join(JOINTS)}): nothing was sent.")
        if cmd.mode != rhal.MODE_POSITION:
            return self._refused("mode", f"the command is in mode {cmd.mode}, and this hand takes positions: a goal angle per joint, and there is no velocity or torque route. Send positions.")
        if self._estop:
            return rhal.SKR_EPERM
        applied = [bool(cmd.mask[j]) if cmd.mask else True for j in range(16)]
        if not any(applied):
            return rhal.SKR_OK
        registers: dict[int, int] = {}
        # Every applied joint is checked before any is written: a command refused for one joint is refused whole (F281), and a refused command never changes what the hand is told.
        for joint in range(16):
            if not applied[joint]:
                continue
            value = cmd.values[joint]
            if not math.isfinite(value):
                return self._refused("finite", f"{JOINTS[joint]} was commanded to {value!r}, which is not finite: the command was refused whole and nothing was sent.")
            low, high = self._limits[joint]
            if not low - 1e-9 <= value <= high + 1e-9:
                return self._refused("range", f"{JOINTS[joint]} was commanded to {value:.6f} rad, outside its range [{low:.6f}, {high:.6f}]: the command was refused whole and nothing was sent.")
            registers[joint] = frames.position_to_register(min(max(value, low), high))
        try:
            self._gate(list(registers))
            self._send_goals(registers)
        except BridgeRefusal as error:
            return self._refused("command:" + str(error.code) + ":" + error.args[0][:40], str(error), error.code)
        except frames.ProtocolError as error:
            return self._refused("protocol", str(error), rhal.SKR_EIO)
        self._last_refusal = None
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        """Latch, and wake the thread that writes the hold. The call does not touch the line: the RHAL's "never blocks" is 50 ms, and a line that is slow or dead would make a stop that wrote its own hold wait for it
        (the link-chaos contract found 101 ms behind a blackhole in Inspire's driver, F298). The latch is the stop: commands are refused from this instant, and the hold is on the wire as soon as the line carries it,
        which `status` reports."""
        with self._lock:
            self._estop = True
            self._stop_calls += 1
            self._stop_delivered = False
            self._hold = None
            self._changed.notify_all()
        self._kick.set()
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """Clear the session latch (§48.2) once the hand has been *seen* to take the stop: an exchange after the call shows the servos' own record of their goals equal to what the stop wrote."""
        if self._link is None or self._loop is None:
            self._estop = False
            return rhal.SKR_OK
        try:
            self._wait_for_reading(lambda r: self._stop_delivered and self._hold is not None and list(r.goal) == self._hold, _COMMAND_BUDGET_S * 4 + self._timeout * 6, honour_stop=False)
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

    def _read_all(self, budget: float | None = None, between=None) -> Reading:
        """The cycle's three synchronised reads, checked. `between` is called before the second and third, so that a stop that is owed goes out ahead of them."""
        state = self._sync_read(*frames.SPAN_STATE, budget)
        if between is not None:
            between()
        gains = self._sync_read(*frames.SPAN_GAINS, budget)
        if between is not None:
            between()
        motion = self._sync_read(*frames.SPAN_MOTION, budget)
        goal_current_at = frames.ADDR_GOAL_CURRENT - frames.ADDR_POSITION_D
        reading = Reading(
            torque=tuple(state[s][0] for s in IDS),
            hw_error=tuple(state[s][frames.ADDR_HARDWARE_ERROR - frames.ADDR_TORQUE_ENABLE] for s in IDS),
            gain_d=tuple(frames.u16(gains[s], 0) for s in IDS),
            gain_i=tuple(frames.u16(gains[s], 2) for s in IDS),
            gain_p=tuple(frames.u16(gains[s], 4) for s in IDS),
            goal_current=tuple(frames.i16(gains[s], goal_current_at) for s in IDS),
            goal=tuple(frames.i32(motion[s], 0) for s in IDS),
            moving=tuple(motion[s][frames.ADDR_MOVING - frames.ADDR_GOAL_POSITION] for s in IDS),
            current=tuple(frames.i16(motion[s], frames.ADDR_PRESENT_CURRENT - frames.ADDR_GOAL_POSITION) for s in IDS),
            velocity=tuple(frames.i32(motion[s], frames.ADDR_PRESENT_VELOCITY - frames.ADDR_GOAL_POSITION) for s in IDS),
            position=tuple(frames.i32(motion[s], frames.ADDR_PRESENT_POSITION - frames.ADDR_GOAL_POSITION) for s in IDS),
        )
        out = [(s, v) for s in IDS for v in (reading.position[s], reading.goal[s]) if abs(v) > _MULTI_TURN_TICKS]
        if out:
            raise frames.ProtocolError(f"servo {out[0][0]} reports a position or goal of {out[0][1]}, outside the 256 turns either way that the multi-turn range allows (e-manual): a frame that is wrong, not a hand somewhere strange.")
        return reading

    def _gate(self, joints: list[int]) -> None:
        """What a command waits for: a hand that has been read, that holds no fault, and whose servos for these joints hold torque."""
        with self._lock:
            reading = self._latest
        if reading is None:
            raise BridgeRefusal("the hand has not been read yet.", rhal.SKR_ETIMEDOUT)
        for servo in IDS:
            bits = reading.hw_error[servo]
            if frames.joint_fault(bits) is not None:
                raise BridgeRefusal(f"servo {servo} ({JOINTS[servo]}) reports {frames.describe_hw_error(bits)} (Hardware_Error_Status {bits:#04x}), and a hand holding a fault takes nothing: the `clear_errors` motion reboots the servos that "
                                    "hold one, and drops their torque. The command was not sent.", rhal.SKR_EPERM)
        limp = [JOINTS[j] for j in joints if not reading.torque[j]]
        if limp:
            raise BridgeRefusal(f"the servos of {', '.join(limp)} have torque off, and a limp servo takes a goal and does not move to it (a power cycle, or someone switched it off). The `enable_torque` motion holds the hand where it "
                                "is and switches torque on. The command was not sent.", rhal.SKR_EPERM)

    def _send_goals(self, registers: dict[int, int]) -> None:
        """One synchronised write of the goals a command applies, and only those, under the line's lock, with the latch checked again inside it so that a command that waited for a stop does not follow it."""
        deadline = time.monotonic() + _COMMAND_BUDGET_S
        try:
            with self._lock:
                dirty = self._params_dirty
            if dirty:
                self._assert_parameters(deadline)
            request = frames.sync_write(frames.ADDR_GOAL_POSITION, 4, {servo: frames.pack_u32(register) for servo, register in registers.items()})
            with self._line(deadline):
                if self._estop:
                    raise BridgeRefusal("the stop was latched while the command was being made.", rhal.SKR_EPERM)
                assert self._link is not None
                self._link.transact(request, (), self._budget(deadline))
            with self._lock:
                for servo, register in registers.items():
                    self._image[servo] = register
                self._commanded_at = time.monotonic()
        except link.LinkError as error:
            # A command that ran out of time waiting for the line while a stop was being written was refused by the stop, not by a slow line: the reading thread holds the line for the hold's two transactions, and which of
            # the two gets it first is the scheduler's. The latch decides, so the answer does not depend on the race.
            if self._estop:
                raise BridgeRefusal("the stop is latched: nothing was written.", rhal.SKR_EPERM) from error
            raise BridgeRefusal(f"the hand did not take the command ({error}): a slow or dead line, not a refusal by the servos. What they hold is their own record of their goals, which `status` shows.", rhal.SKR_EIO) from error

    def _deliver_stop(self) -> None:
        """The stop: every servo's own position, read fresh, written back as its goal. Under the line's lock, so it follows any write already in flight. A call of `emergency_stop` that comes while this is being written
        is owed a hold read after it, so a delivery that began before it does not stand for it: the stop stays owed and the reading thread writes it again."""
        with self._lock:
            calls = self._stop_calls
        with self._link_lock:
            link_ = self._link
            if link_ is None:
                raise link.LinkError("the line is not open.")
            answers = link_.transact(frames.sync_read(frames.ADDR_PRESENT_POSITION, 4, IDS), IDS, self._timeout)
            hold = [frames.i32(answers[servo].params) for servo in IDS]
            if any(abs(position) > _MULTI_TURN_TICKS for position in hold):
                raise frames.ProtocolError(f"the servos report positions {hold}, outside the 256 turns either way that the multi-turn range allows (e-manual): a hold is not written from positions that are not positions.")
            link_.transact(frames.sync_write(frames.ADDR_GOAL_POSITION, 4, {servo: frames.pack_u32(hold[servo]) for servo in IDS}), (), self._timeout)
        with self._lock:
            if calls != self._stop_calls:
                return
            self._hold = list(hold)
            self._stop_delivered = True
            self._image = list(hold)
            self._changed.notify_all()

    def _exchange_loop(self) -> None:
        """The reading thread: the hand's own state in, once a period, forever; and, when something is owed, the stop or the operator's gains and goal current out."""
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
            except (link.LinkError, frames.ProtocolError, BridgeRefusal) as error:
                reason = str(error)
                if not link_.is_open:
                    if time.monotonic() - reopened >= 0.5:
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
                        # A line that was dead is not a hand that was left as this driver last asked: it may have been power-cycled, which gives the gains and the goal current back to their defaults.
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
                        # The image of the goals is where the hand says its goals are after a line that was dead: what this driver last asked is not necessarily what a power-cycled hand holds.
                        if not self._estop:
                            self._image = list(reading.goal)
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
        """What a good reading shows is owed: a stop that has not been written, or the operator's gains and goal current where the servos show others."""
        with self._lock:
            owed_stop = self._estop and not self._stop_delivered
            if self._differs(reading):
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
            joints.append({"name": name, "role": self._roles.get(name), "type": "revolute", "limits": {"position_min_rad": low, "position_max_rad": high}, "control_modes": ["position"]})
        effector = {"name": "hand", "role": None, "kind": "dexterous_hand", "dof": 16, "max_grip_force_n": None, "max_opening_m": None}
        descriptor = {
            "schema_version": 1, "robot_slug": "leap-hand-" + self._variant.name, "digest": None, "verified": False, "simulator": self._simulated, "base": "fixed", "dof": 16,
            "joints": joints, "end_effectors": [effector], "sensors": [],
            "control": {"modes": ["position"], "rate_hz_max": self._rate_hz, "rate_hz_min": 1.0},
            # Null: the ranges are the authors' and the speed is whatever the servos do under a current cap, none of it a risk assessment (§5.2); and no force in newtons, because the hand is commanded by a current.
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
            exchanges, dead, delivered, hold, reasserted, image = self._exchanges, self._dead, self._stop_delivered, self._hold, self._reasserted, list(self._image)
        out: dict[str, object] = {
            "variant": self._variant.name, "servo": self._variant.model_name, "link": self._link.target if self._link else None, "firmware": list(self._firmware), "simulator": self._simulated,
            "position_rad": [round(frames.register_to_position(p), 6) for p in reading.position], "position": list(reading.position), "goal": list(reading.goal), "image": image,
            "torque": list(reading.torque), "hardware_error": [f"{b:#04x}" for b in reading.hw_error], "moving": list(reading.moving), "current_ma": [c * frames.CURRENT_MA_PER_UNIT for c in reading.current],
            "goal_current_ma": list(reading.goal_current), "gains_p": list(reading.gain_p), "gains_i": list(reading.gain_i), "gains_d": list(reading.gain_d), "current_ma_configured": self._current_ma,
            "gains_configured": {"kp": self._gains[0], "ki": self._gains[1], "kd": self._gains[2], "side_scale": self._side_scale}, "parameters_reasserted": reasserted, "exchanges": exchanges, "link_dead": dead,
            "stop_latched": self._estop, "stop_delivered": delivered, "stop_hold": hold}
        try:
            extra = self._sync_read(frames.ADDR_PRESENT_VOLTAGE, 3)
            out["voltage_v"] = [round(frames.u16(extra[s]) * 0.1, 1) for s in IDS]
            out["temperature_c"] = [extra[s][2] for s in IDS]
        except (link.LinkError, frames.ProtocolError) as error:
            out["read_error"] = str(error)
        return out

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
        registers = {joint: frames.position_to_register(value) for joint, value in targets.items()}
        before = self._present()
        travel = max(abs(registers[j] - before.position[j]) for j in registers) * frames.RAD_PER_TICK
        wait = timeout if timeout is not None else travel / 2.0 + 3.0
        self._gate(list(registers))
        self._send_goals(registers)
        started = {j: False for j in registers}

        def done(reading: Reading) -> bool:
            if any(frames.joint_fault(reading.hw_error[j]) is not None for j in IDS):
                return True
            ended = True
            for j in registers:
                started[j] = started[j] or bool(reading.moving[j]) or abs(reading.position[j] - before.position[j]) > _ARRIVAL_TOLERANCE
                if reading.goal[j] != registers[j] or reading.moving[j]:
                    ended = False
                elif abs(reading.position[j] - registers[j]) > _ARRIVAL_TOLERANCE and not started[j]:
                    ended = False                       # not there, not moving, and never seen to move: a goal that has not been acted on yet
            return ended

        try:
            reading = self._wait_for_reading(done, wait)
        except BridgeRefusal as error:
            if error.code == rhal.SKR_EIO and "no reading showed" in str(error) and not all(started.values()):
                idle = [JOINTS[j] for j, s in started.items() if not s]
                raise BridgeRefusal(f"{', '.join(idle)} never started moving after the goal (the servos did not show it moving, and the position did not change, in {wait:.1f} s): {error}", rhal.SKR_EIO) from None
            raise
        for servo in IDS:
            if frames.joint_fault(reading.hw_error[servo]) is not None:
                raise BridgeRefusal(f"servo {servo} ({JOINTS[servo]}) reports {frames.describe_hw_error(reading.hw_error[servo])} during the move.", rhal.SKR_EIO)
        names = {j: JOINTS[j] for j in registers}
        arrived = all(abs(reading.position[j] - registers[j]) <= _ARRIVAL_TOLERANCE for j in registers)
        return {"result": "arrived" if arrived else "stopped_on_an_object",
                "joints": {names[j]: {"position": reading.position[j], "position_rad": round(frames.register_to_position(reading.position[j]), 6), "goal": registers[j], "current_ma": reading.current[j] * frames.CURRENT_MA_PER_UNIT}
                           for j in registers}}

    def _motion_enable_torque(self, args: dict) -> dict:
        self._no_arguments("enable_torque", args)
        reading = self._present()
        for servo in IDS:
            if frames.joint_fault(reading.hw_error[servo]) is not None:
                raise BridgeRefusal(f"servo {servo} ({JOINTS[servo]}) reports {frames.describe_hw_error(reading.hw_error[servo])}: torque is not switched on over a hardware error. `clear_errors` first.", rhal.SKR_EPERM)
        limp = [servo for servo in IDS if not reading.torque[servo]]
        if not limp:
            return {"result": "already_on", "enabled": []}
        # Where each limp servo is now is its goal, written before its torque goes on, so that switching it on moves nothing.
        deadline = time.monotonic() + _COMMAND_BUDGET_S * 4
        self._assert_parameters(deadline)
        self._sync_write(frames.ADDR_GOAL_POSITION, 4, {servo: frames.pack_u32(reading.position[servo]) for servo in limp}, deadline)
        self._sync_write(frames.ADDR_TORQUE_ENABLE, 1, {servo: b"\x01" for servo in limp}, deadline)
        try:
            after = self._wait_for_reading(lambda r: all(r.torque[s] for s in limp), self._timeout * 6 + 1.0)
        except BridgeRefusal as error:
            raise BridgeRefusal(f"torque was written and no reading has shown it on ({error}).", rhal.SKR_EIO) from None
        with self._lock:
            for servo in limp:
                self._image[servo] = after.goal[servo]
        return {"result": "enabled", "enabled": [JOINTS[s] for s in limp]}

    def _motion_clear_errors(self, args: dict) -> dict:
        confirm = args.pop("confirm", None)
        self._no_arguments("clear_errors", args)
        if confirm != "limp":
            raise BridgeRefusal("`clear_errors` reboots every servo that holds a hardware error, and a rebooted servo comes up with its torque off and its gains at their defaults: its finger goes limp. "
                                "Say `confirm=limp` when that is acceptable.", rhal.SKR_EINVAL)
        reading = self._present()
        faulted = [servo for servo in IDS if frames.joint_fault(reading.hw_error[servo]) is not None]
        if not faulted:
            return {"result": "nothing_to_clear", "rebooted": []}
        for servo in faulted:
            self._txn(frames.reboot(servo), (servo,))
        time.sleep(_REBOOT_S)
        with self._lock:
            self._params_dirty = True
        try:
            self._assert_parameters()
            after = self._wait_for_reading(lambda r: True, self._timeout * 6 + 1.0)
        except BridgeRefusal as error:
            raise BridgeRefusal(f"the servos were rebooted and no reading has shown what it did ({error}).", rhal.SKR_EIO) from None
        left = [servo for servo in IDS if frames.joint_fault(after.hw_error[servo]) is not None]
        return {"result": "cleared" if not left else "not_cleared", "rebooted": [JOINTS[s] for s in faulted], "remaining": [f"{JOINTS[s]}: {frames.describe_hw_error(after.hw_error[s])}" for s in left],
                "note": "the rebooted servos have torque off; the `enable_torque` motion holds them where they are and switches it on"}


class _StopOwed(Exception):
    """A reading cycle was left between two of its reads because a stop has to be written first."""


def _descriptor(name: str) -> dict:
    if name == "status":
        return {"name": "status", "parameters": []}
    if name == "move":
        return {"name": "move", "parameters": [{"name": joint, "unit": "rad", "required": False} for joint in JOINTS] + [{"name": "timeout", "unit": "s", "required": False}]}
    if name == "enable_torque":
        return {"name": "enable_torque", "parameters": []}
    return {"name": "clear_errors", "parameters": [{"name": "confirm", "required": True}]}


def _bounded(settings: dict, key: str, default: float, low: float, high: float) -> float:
    value = settings.get(key)
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not low <= value <= high:
        raise BridgeRefusal(f"`{key}` of {value!r} is not a number from {low:g} to {high:g}.")
    return float(value)


def _say(message: str) -> None:
    print(f"skynet-leap: {message}", file=sys.stderr, flush=True)
