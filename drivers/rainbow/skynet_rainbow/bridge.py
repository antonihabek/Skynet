"""The RHAL driver: a Rainbow Robotics RB arm (RB3, RB5, RB10, RB16 and the rest of the series) over the control box's command and data ports (DRY-COVERAGE-PLAN.md item 12.E.14).

§4's five mandatory entry points over :mod:`skynet_rainbow.frames` and :mod:`skynet_rainbow.link`. It links nothing of Rainbow's: the script grammar (UI Script manual 6.10, Appendix E of the user manual) and the
580-byte state packet (the `reqdata` Structure document) are the maker's documents, and `layout_check.py` holds this driver's text and its reading of the packet to the maker's own client (`rbpodo`), which it runs.

# The body

Six revolute joints, `j1` to `j6` (base, shoulder, elbow, wrist 1 to 3), in radians (the controller speaks degrees). It takes a position, one control mode, and its tier ceiling is Guest (§31.3). A position command is one
**`move_servo_j`**: the controller's own call for streaming a target, whose `t1` (the time to arrive), `t2` (the time the motion is kept after arrival), `gain` and `alpha` are the operator's (`servo`; the defaults are
those of the maker's own ROS 2 package, `t1` being the control period). The joint ranges are the operator's (`limits`, required): the maker's own URDF gives every joint the same placeholder, plus or minus 3.14, and the
manual's figure of the limits is a picture.

# What it never does on its own

It does not activate the arm (`mc jall init` powers it and turns the servos on), it does not change the controller between its real and its simulation mode (in the simulation "only the internal reference is changed", and
the arm does not move), it does not tell the controller a payload, and it does not acknowledge a fault. Each is a motion (`activate`, `set_mode`, `set_payload`, `clear_faults`) that a person asks for. A command is
refused, naming the state, until the controller's mode is the one the operator said it is (`simulator`), the arm is activated (in the real mode), and nothing is wrong or paused.

# The stop

`task stop` (the UI Script manual spells it `halt` too): "stop any program flow or action/movement". **It does not latch on the controller**: the Virtual Control Box was seen to end a servo motion on it and to start the
next one at the next `move_servo_j`, so the latch is this driver's: `emergency_stop` latches and returns and never touches the line, commands are refused from that instant, and the reading thread writes the stop under the same
lock the commands are written under, so on the wire it follows every command that was in flight and never precedes one. `task pause` is not a stop: it was seen to leave a servo stream running. The maker documents one more way
to stop, `arm_powerdown()` ("be careful with use"), which this driver never sends: whether a stop should drop the brakes is the integrator's risk assessment, and nothing here shows what it does. It is a **software stop**:
the arm's own safety circuit is a hardware chain, and there is none on this wire.
"""

from __future__ import annotations

import contextlib
import json
import math
import os
import re
import sys
import threading
import time
from collections import deque

import skynet_rhal as rhal

from . import frames, link

__all__ = ["BridgeRefusal", "RainbowBridge", "TOKEN"]

TOKEN = "vendor.rainbow.task"
JOINTS = frames.JOINTS

_RATE_HZ = 50.0
#: The control cycle of external script control is 10 ms (the user manual, 9.6), 1.8 ms in the high-speed mode that this driver does not set.
_MAX_RATE_HZ = 100.0
_TRANSACTION_S = 0.1
_OPEN_TIMEOUT_S = 3.0
_STALE_S = 0.5
#: The most a command may take to be written: the control loop waits for it, and a quarter of a second is the contract's bound (`tools/faultproxy/chaos.py`, F256).
_COMMAND_BUDGET_S = 0.25
_FAULT_THRESHOLD = 3
_REOPEN_S = 0.5
#: How long a controller's warning keeps the state degraded: a warning is an event, and the controller says it once.
_WARN_HOLD_S = 2.0
#: How long the oldest command may wait for its acknowledgement before the controller is called stalled. A controller acknowledges a line as it parses it, so this is its backlog, not a deadline for a motion.
_ACK_STALE_S = 1.0
_MAX_PENDING = 4096
#: A joint within this many degrees of its goal is there.
_ARRIVAL_TOLERANCE_DEG = 0.5
_KNOWN_KEYS = {"model", "host", "port", "data_port", "simulator", "limits", "servo", "payload_kg", "payload_com_mm", "roles", "rate_hz", "timeout_ms", "stale_ms", "open_timeout_ms"}
_MOTIONS = ("status", "set_mode", "activate", "resume", "clear_faults", "set_payload", "move")
_MODEL = re.compile(r"RB[0-9]+[A-Za-z0-9_-]*")
_DEG = math.pi / 180.0
#: The fault word, in the bits above any message code of the maker's table (the largest is 605): a message code is its own number, and what the state packet says is a band and the field's own value.
SOS_BAND = 1 << 16
EMS_BAND = 2 << 16
COLLISION_BAND = 3 << 16
JOINT_BAND = 4 << 16
MODE_BAND = 5 << 16
SOS_STATE = 6 << 16


class BridgeRefusal(Exception):
    """A reason this body did not come up or a command did not happen, in the terms an integrator acts on."""

    def __init__(self, message: str, code: int = rhal.SKR_ENODEV) -> None:
        super().__init__(message)
        self.code = code


class Unreachable(BridgeRefusal):
    """The controller did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""

    def __init__(self, message: str) -> None:
        super().__init__(message, rhal.SKR_EIO)


class RainbowBridge:
    """A body made of one Rainbow arm's six joints."""

    #: A stop written to the host while a motion runs is read at once: `emergency_stop` is safe from another thread (python-host's `stop_during_extension`).
    stop_during_extension = True

    def __init__(self) -> None:
        self._opened = False
        self._estop = False
        self._descriptor: str | None = None
        self._seq = 0
        self._simulated = True
        self._model = "RB5-850"
        self._limits: list[tuple[float, float]] = [(-math.pi, math.pi)] * 6
        self._payload: tuple[float, tuple[float, float, float]] | None = None
        self._servo: dict[str, float] = {}
        self._roles: dict[str, str | None] = {}
        self._rate_hz = _RATE_HZ
        self._stale_s = _STALE_S
        self._timeout = _TRANSACTION_S
        self._command: link.CommandChannel | None = None
        self._data: link.DataChannel | None = None
        self._loop: threading.Thread | None = None
        self._closing = threading.Event()
        #: Wakes the reading thread: a stop is written by it, at once, and `emergency_stop` never waits on the line. A pipe made when a session opens and closed when it ends.
        self._wake_r = self._wake_w = -1
        self._wake_lock = threading.Lock()
        self._command_lock = threading.Lock()
        self._lock = threading.Lock()
        self._changed = threading.Condition(self._lock)
        self._last_refusal: str | None = None
        # Everything below is guarded by `_lock`.
        self._latest: frames.State | None = None
        self._stamp_ns = 0
        self._packets = 0
        self._failures = 0
        self._dead: str | None = None
        self._goal: list[float] | None = None
        self._stop_delivered = False
        self._stop_calls = 0
        self._stop_retry_at = 0.0
        self._pending: deque[float] = deque()
        self._acks = 0
        self._errors: list[int] = []
        self._warns: list[tuple[int, float, int]] = []
        self._warn_seq = 0
        self._warn_taken = 0
        self._messages: deque[str] = deque(maxlen=16)
        self._unknown = 0
        self._rejected = 0
        self._system_version: int | None = None
        self._halts = 0
        self._command_reopened = 0.0
        self._data_reopened = 0.0

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
            _say(f"the arm could not be read: {error}")
            return rhal.SKR_EIO
        self._opened = True
        return rhal.SKR_OK

    def _reset_session(self) -> None:
        """A session starts with no stop, no fault remembered and no reading: nothing of the last one is the arm's."""
        self._estop = False
        self._descriptor = None
        self._last_refusal = None
        with self._lock:
            self._latest = None
            self._goal = None
            self._stop_delivered = False
            self._errors = []
            self._warns = []
            self._warn_seq = self._warn_taken = 0
            self._pending = deque()
            self._acks = 0
            self._messages.clear()
            self._unknown = self._rejected = self._halts = 0
            self._system_version = None

    def _open(self, settings: dict) -> None:
        unknown = sorted(set(settings) - _KNOWN_KEYS)
        if unknown:
            raise BridgeRefusal(f"the configuration has {', '.join(map(repr, unknown))}, which this driver does not read. The keys it reads are {', '.join(sorted(_KNOWN_KEYS))}.")
        model = settings.get("model")
        if not isinstance(model, str) or not _MODEL.fullmatch(model):
            raise BridgeRefusal(f"`model` of {model!r} is not in the configuration, or is not a Rainbow model name (RB5-850, RB10-1300E, RB3-730ES-U, ...). The controller does not say which arm it is on, so the operator does: "
                                "a joint's range and a payload's limit are the arm's.")
        self._model = model
        host = settings.get("host")
        port = settings.get("port", frames.COMMAND_PORT)
        data_port = settings.get("data_port", frames.DATA_PORT)
        for key, value in (("port", port), ("data_port", data_port)):
            if isinstance(value, bool) or not isinstance(value, int) or not 0 < value < 65536:
                raise BridgeRefusal(f"`{key}` of {value!r} is not a port from 1 to 65535 (the control box's command port is {frames.COMMAND_PORT} and its data port {frames.DATA_PORT}).")
        if not isinstance(host, str) or not host:
            raise BridgeRefusal(f"`host` of {host!r} is not the control box's address: a host name or an IP address (10.0.2.7 from the factory). This driver does not scan for an arm.")
        claim = settings.get("simulator")
        if not isinstance(claim, bool):
            raise BridgeRefusal("`simulator` is not in the configuration, or is not true or false. Whether the controller moves an arm or only its own reference is its mode, and the open checks the claim against it; "
                                "nothing else tells the descriptor which it is.")
        self._simulated = claim
        self._limits = self._read_limits(settings.get("limits"))
        self._payload = self._read_payload(settings.get("payload_kg"), settings.get("payload_com_mm"))
        self._rate_hz = _bounded(settings, "rate_hz", _RATE_HZ, 1.0, _MAX_RATE_HZ)
        self._servo = self._read_servo(settings.get("servo"))
        roles = settings.get("roles") or {}
        if not isinstance(roles, dict) or set(roles) - set(JOINTS):
            raise BridgeRefusal(f"`roles` is a mapping from a joint's name to its role, and the joints are {', '.join(JOINTS)}.")
        self._roles = {name: roles.get(name) for name in JOINTS}
        self._timeout = _bounded(settings, "timeout_ms", _TRANSACTION_S * 1000, 10.0, 5000.0) / 1000.0
        self._stale_s = _bounded(settings, "stale_ms", _STALE_S * 1000, 20.0, 30000.0) / 1000.0
        open_s = _bounded(settings, "open_timeout_ms", _OPEN_TIMEOUT_S * 1000, 100.0, 60000.0) / 1000.0

        self._data = link.DataChannel(host, data_port)
        self._command = link.CommandChannel(host, port)
        try:
            self._data.open(timeout=min(open_s, 3.0))
        except link.LinkError as error:
            raise Unreachable(f"the control box's data port could not be opened ({error}). Is the control box powered, is its address the one set on the pendant, and is {self._data.target} its data port?") from error
        first = self._first_packet(open_s)
        wanted = frames.MODE_SIMULATION if self._simulated else frames.MODE_REAL
        if first.mode != wanted:
            raise BridgeRefusal(f"the controller is in {_mode_name(first.mode)} mode and `simulator` is {str(self._simulated).lower()}: "
                                f"{'a simulated body is on a controller that would move an arm' if self._simulated else 'a real body is on a controller that moves nothing'}. Nothing was commanded: "
                                "say which it is, or change the controller's mode on the pendant (the `set_mode` motion does it once the driver is open).")
        try:
            self._command.open(timeout=min(open_s, 3.0))
        except link.LinkError as error:
            raise Unreachable(f"the control box's command port could not be opened ({error}). Is {self._command.target} its command port?") from error
        self._identify(open_s)
        with self._lock:
            self._latest, self._stamp_ns = first, rhal.acquisition_ns()
            self._packets, self._failures, self._dead = 1, 0, None
        self._closing.clear()
        self._open_wake()
        self._loop = threading.Thread(target=self._exchange_loop, args=(self._wake_r,), daemon=True, name="rainbow-exchange")
        self._loop.start()
        self._descriptor = self._build_descriptor()

    def _read_limits(self, given: object) -> list[tuple[float, float]]:
        if given is None:
            raise BridgeRefusal("`limits` is not in the configuration. The maker's own URDF gives every joint the same placeholder range and the manual's is a picture, so it is the operator's, for this arm and this cell, "
                                "as six [min, max] pairs in radians.")
        if not (isinstance(given, (list, tuple)) and len(given) == 6 and all(isinstance(pair, (list, tuple)) and len(pair) == 2 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) for v in pair) for pair in given)):
            raise BridgeRefusal("`limits` is six [min, max] pairs in radians, one for each of j1 to j6, as numbers.")
        out: list[tuple[float, float]] = []
        for name, (low, high) in zip(JOINTS, given):
            if not low < high:
                raise BridgeRefusal(f"`limits` of {name} is [{low}, {high}], which is not a range.")
            if low < -2 * math.pi - 1e-9 or high > 2 * math.pi + 1e-9:
                raise BridgeRefusal(f"`limits` of {name} is [{low}, {high}] rad, past a turn either way: the manual gives a joint angle from -360 to 360 degrees.")
            out.append((float(low), float(high)))
        return out

    @staticmethod
    def _read_payload(kg: object, com: object) -> tuple[float, tuple[float, float, float]] | None:
        if kg is None:
            if com is not None:
                raise BridgeRefusal("`payload_com_mm` is given without `payload_kg`.")
            return None
        if isinstance(kg, bool) or not isinstance(kg, (int, float)) or not math.isfinite(kg) or not 0 <= kg <= 100:
            raise BridgeRefusal(f"`payload_kg` of {kg!r} is not a number from 0 to 100: the load on the flange, which the controller's gravity compensation and collision detection depend on.")
        centre = (0.0, 0.0, 0.0) if com is None else com
        if not (isinstance(centre, (list, tuple)) and len(centre) == 3 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) and abs(v) <= 2000 for v in centre)):
            raise BridgeRefusal("`payload_com_mm` is the payload's centre of mass as three distances in millimetres, in the maker's default tool frame, each within 2000.")
        return float(kg), (float(centre[0]), float(centre[1]), float(centre[2]))

    def _read_servo(self, given: object) -> dict[str, float]:
        """`servo`: the four parameters of `move_servo_j`, each checked against the manual's range. What is absent is the maker's own ROS 2 package's: `t2` 0.03 s, `gain` 0.5, `alpha` 0.5, and `t1` the control period.
        A `t2` that is not longer than the period would let the arm come to rest between two commands, so the default is twice the period where that is more than 0.03 s (and below the manual's 0.2 s)."""
        period = 1.0 / self._rate_hz
        out = {"t1": max(period, frames.MIN_T1), "t2": min(max(0.03, 2 * period), 0.19), "gain": 0.5, "alpha": 0.5}
        if given is not None:
            self._merge_servo(out, given)
        if out["t2"] <= period:
            raise BridgeRefusal(f"`servo` t2 of {out['t2']} s is not longer than the control period of {period:.4f} s: the arm would come to rest between two commands.")
        return out

    @staticmethod
    def _merge_servo(out: dict[str, float], given: object) -> None:
        if not isinstance(given, dict) or set(given) - set(out):
            raise BridgeRefusal(f"`servo` is a mapping from {', '.join(out)} to numbers: the parameters of `move_servo_j`.")
        for name, value in given.items():
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
                raise BridgeRefusal(f"`servo` {name} of {value!r} is not a number.")
            out[name] = float(value)
        try:
            frames.servo_j([0.0] * 6, **out)
        except frames.ProtocolError as error:
            raise BridgeRefusal(f"`servo` is outside the manual's range: {error}") from None

    def _first_packet(self, open_s: float) -> frames.State:
        assert self._data is not None
        deadline = time.monotonic() + open_s
        last = "nothing arrived"
        while time.monotonic() < deadline:
            try:
                packet = self._data.request(min(deadline - time.monotonic(), 0.5))
            except link.LinkError as error:
                last = str(error)
                if not self._data.is_open:
                    try:
                        self._data.open(timeout=min(max(deadline - time.monotonic(), 0.05), 0.5))
                    except link.LinkError as again:
                        last = str(again)
                        time.sleep(0.05)
                continue
            try:
                return frames.decode_state(packet)
            except frames.ProtocolError as error:
                last = str(error)
        raise Unreachable(f"no state packet arrived from {self._data.target} in {open_s:.1f} s ({last}). Is the control box powered, is {self._data.target} its data port, and is its address set on the pendant?")

    def _identify(self, open_s: float) -> None:
        """`get_control_box_info()`, which the controller answers `info[controlbox][version,type]`: whether the command port is a Rainbow control box's, and its software version."""
        assert self._command is not None
        try:
            events = self._command.query(frames.get_control_box_info(), lambda e: e.kind == "info" and e.category == "controlbox", min(open_s, 1.0))
        except link.LinkError as error:
            raise Unreachable(f"the control box's command port did not answer at {self._command.target} ({error}). Is it the control box's address, and is port {self._command.target.rsplit(':', 1)[1]} its command port?") from error
        info = events[-1].message.split(",")
        try:
            version = int(info[0])
        except ValueError:
            raise BridgeRefusal(f"the control box answered get_control_box_info() with {events[-1].message!r}, which is not its software version and a type.", rhal.SKR_EIO) from None
        with self._lock:
            self._system_version = version

    def close(self) -> int:
        # A close writes nothing: the arm keeps its last target and its state, and what stopping it does is the integrator's to decide, never a close's.
        self._teardown()
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        now = rhal.acquisition_ns()
        with self._lock:
            reading, stamp, dead, errors, warns = self._latest, self._stamp_ns, self._dead, list(self._errors), list(self._warns)
            lag = self._ack_lag()
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
        out.position_rad = [angle * _DEG for angle in self._position(reading)]
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP | rhal.FLAG_DEGRADED
        verdict = _classify(reading, errors)
        recent = [code for _, at, code in warns if time.monotonic() - at <= _WARN_HOLD_S]
        if verdict is not None:
            kind, code = verdict
            out.flags |= (rhal.FLAG_ESTOP if kind == "estop" else rhal.FLAG_FAULT) | rhal.FLAG_DEGRADED
            out.fault_code = code
        elif self._not_commandable(reading) is not None:
            out.flags |= rhal.FLAG_DEGRADED         # an arm that is paused, not activated, or in the wrong mode takes no command
        elif recent:
            out.flags |= rhal.FLAG_DEGRADED
            out.fault_code = recent[-1]
        age = (rhal.acquisition_ns() - stamp) / 1e9
        if dead or age > self._stale_s or lag > _ACK_STALE_S:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened or self._command is None:
            return rhal.SKR_EPERM
        if cmd.joint_count != 6:
            return self._refused("count", f"the command has {cmd.joint_count} joints and this body has six ({', '.join(JOINTS)}): nothing was sent.")
        if cmd.mode != rhal.MODE_POSITION:
            return self._refused("mode", f"the command is in mode {cmd.mode}, and this arm takes positions: a target angle per joint, and there is no velocity or torque route. Send positions.")
        if self._estop:
            return rhal.SKR_EPERM
        applied = [bool(cmd.mask[j]) if cmd.mask else True for j in range(6)]
        if not any(applied):
            return rhal.SKR_OK
        targets: list[float | None] = [None] * 6
        # Every applied joint is checked before any is sent: a command refused for one joint is refused whole (F281).
        for joint in range(6):
            if not applied[joint]:
                continue
            value = cmd.values[joint]
            if not math.isfinite(value):
                return self._refused("finite", f"{JOINTS[joint]} was commanded to {value!r}, which is not finite: the command was refused whole and nothing was sent.")
            low, high = self._limits[joint]
            if not low - 1e-9 <= value <= high + 1e-9:
                return self._refused("range", f"{JOINTS[joint]} was commanded to {value:.6f} rad, outside its range [{low:.6f}, {high:.6f}]: the command was refused whole and nothing was sent.")
            targets[joint] = min(max(value, low), high) / _DEG
        try:
            reading = self._gate()
            self._send_servo(targets, reading)
        except BridgeRefusal as error:
            return self._refused("command:" + str(error.code) + ":" + error.args[0][:40], str(error), error.code)
        except frames.ProtocolError as error:
            return self._refused("protocol", str(error), rhal.SKR_EINVAL)
        self._last_refusal = None
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        """Latch, and wake the thread that writes the stop. The call does not touch the line: the RHAL's "never blocks" is 50 ms, and a line that is slow or dead would make a stop that wrote its own command wait for it.
        The latch is the stop: commands are refused from this instant, and the controller's `task stop` is on the wire as soon as the line carries it, which `status` reports."""
        with self._lock:
            self._estop = True
            self._stop_calls += 1
            self._stop_delivered = False
            self._changed.notify_all()
        self._wake()
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """Clear the session latch (§48.2) once the arm has been *seen* to take the stop: a reading after `task stop` was written that shows no motion command executing. The controller does not latch the stop, and
        nothing here un-latches anything on it."""
        if self._command is None or self._loop is None:
            self._estop = False
            return rhal.SKR_OK
        try:
            self._wait_for_packet(lambda r: self._stop_delivered and r.robot_state == frames.ROBOT_IDLE, _COMMAND_BUDGET_S * 4 + self._timeout * 6, honour_stop=False)
        except BridgeRefusal as error:
            _say(f"the stop stays latched: no reading has shown the arm take it ({error}) Fix the line, or close and open the driver, which is a new session.")
            return rhal.SKR_EIO
        with self._lock:
            self._estop = False
            self._stop_delivered = False
            self._goal = None
        self._last_refusal = None
        return rhal.SKR_OK

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── the line ─────────────────────────────────────────────────────────────

    def _position(self, reading: frames.State) -> tuple[float, ...]:
        """The encoders' angles, or, when the controller is in its simulation mode, the reference it is following: the maker's client documents `jnt_ang` as not changing there, and in that mode the reference is the arm."""
        return reading.ref if reading.mode == frames.MODE_SIMULATION else reading.ang

    def _ack_lag(self) -> float:
        """How long the oldest command has waited for its acknowledgement (under `_lock`)."""
        return time.monotonic() - self._pending[0] if self._pending else 0.0

    def _write(self, text: str, budget: float | None = None, by: float | None = None, *, unless_stopped: bool = False) -> None:
        """One line to the command port, under its lock: waiting for the lock and the write both end at `by` (a monotonic time), or `budget` after now. With `unless_stopped` a stop latched while the command waited for the
        line is the answer: a command is not written after the stop is latched, so on the wire the stop follows every command that was in flight and never precedes one."""
        command = self._command
        if command is None:
            raise link.LinkError("the command port is not open.")
        deadline = by if by is not None else time.monotonic() + (budget or _COMMAND_BUDGET_S)
        if not self._command_lock.acquire(timeout=max(0.0, deadline - time.monotonic())):
            raise link.LinkError("the command port was in use until the time the caller had been given ran out.")
        try:
            if unless_stopped and self._estop:
                raise link.LinkError("the stop was latched before the command was written.")
            if time.monotonic() >= deadline:
                raise link.LinkError("the time the caller had been given ran out before the command port was free.")
            command.send(text, deadline)
            with self._lock:
                self._pending.append(time.monotonic())
                if len(self._pending) > _MAX_PENDING:
                    self._pending.popleft()
        finally:
            self._command_lock.release()

    def _open_wake(self) -> None:
        self._close_wake()
        read_end, write_end = os.pipe()
        os.set_blocking(read_end, False)
        os.set_blocking(write_end, False)
        with self._wake_lock:
            self._wake_r, self._wake_w = read_end, write_end

    def _close_wake(self) -> None:
        with self._wake_lock:
            ends, (self._wake_r, self._wake_w) = (self._wake_r, self._wake_w), (-1, -1)
            for end in ends:
                if end >= 0:
                    with contextlib.suppress(OSError):
                        os.close(end)

    def _wake(self) -> None:
        # Under the lock that closes the pipe: a descriptor that was closed and handed to something else must not be written to.
        with self._wake_lock:
            if self._wake_w >= 0:
                with contextlib.suppress(OSError):
                    os.write(self._wake_w, b"x")

    @staticmethod
    def _drain_wake(read_end: int) -> None:
        with contextlib.suppress(OSError):
            while os.read(read_end, 64):
                pass

    # ── the command ──────────────────────────────────────────────────────────

    def _not_commandable(self, reading: frames.State) -> str | None:
        """Why an arm that holds no fault still takes no command, or None."""
        wanted = frames.MODE_SIMULATION if self._simulated else frames.MODE_REAL
        if reading.mode != wanted:
            return (f"the controller is in {_mode_name(reading.mode)} mode and `simulator` is {str(self._simulated).lower()}: the mode was changed under this session, and "
                    f"{'a controller that moves an arm is not a simulation' if self._simulated else 'a controller in its simulation mode moves nothing'}. The `set_mode` motion puts it back")
        if reading.task_state == frames.TASK_PAUSED or reading.paused_flag:
            return "the controller is paused (a `task pause`, an alarm, or a collision): the `resume` motion resumes it"
        if reading.mode == frames.MODE_REAL and reading.init_state != frames.ACTIVATED:
            return f"the arm is not activated (activation stage {reading.init_state} of {frames.ACTIVATED}): the `activate` motion activates it"
        return None

    def _gate(self) -> frames.State:
        """What a command waits for: an arm that has been read, whose controller is in the mode the operator said, that is activated (in the real mode), that holds no fault and is not paused, and that has acknowledged what it was sent."""
        with self._lock:
            reading, errors, lag, dead = self._latest, list(self._errors), self._ack_lag(), self._dead
        if reading is None:
            raise BridgeRefusal("the arm has not been read yet.", rhal.SKR_ETIMEDOUT)
        if dead:
            raise BridgeRefusal(f"the controller is not being read ({dead}). The command was not sent.", rhal.SKR_EIO)
        if lag > _ACK_STALE_S:
            raise BridgeRefusal(f"the controller has not acknowledged a command for {lag:.1f} s: it is hung or the line is. The command was not sent.", rhal.SKR_EIO)
        verdict = _classify(reading, errors)
        if verdict is not None:
            kind, code = verdict
            raise BridgeRefusal(f"the arm is in {'an emergency stop' if kind == 'estop' else 'a fault'} ({_say_code(code)}), and an arm holding one takes nothing: the `clear_faults` motion acknowledges what the controller "
                                "reported once it holds none, and a stop is released at the controller. The command was not sent.", rhal.SKR_EPERM)
        why = self._not_commandable(reading)
        if why is not None:
            raise BridgeRefusal(f"{why}. The command was not sent.", rhal.SKR_EPERM)
        return reading

    def _send_servo(self, targets: list[float | None], reading: frames.State) -> None:
        """One `move_servo_j` of every joint: the ones the command applies at their targets, and the others where this driver last asked them to be (or, at first, where they are)."""
        deadline = time.monotonic() + _COMMAND_BUDGET_S
        with self._lock:
            held = list(self._goal) if self._goal is not None else list(self._position(reading))
            refused = self._take_warnings()
        goal = [held[j] if targets[j] is None else float(targets[j]) for j in range(6)]       # type: ignore[arg-type]
        if refused:
            raise BridgeRefusal(f"the controller refused the last command: {refused}. This command was not sent.", rhal.SKR_EINVAL)
        text = frames.servo_j(goal, **self._servo)
        try:
            self._write(text, by=deadline, unless_stopped=True)
        except link.LinkError as error:
            # A command that ran out of time while a stop was being written was refused by the stop, not by a slow line: the latch decides, so the answer does not depend on the race.
            if self._estop:
                raise BridgeRefusal("the stop is latched: nothing was written.", rhal.SKR_EPERM) from error
            raise BridgeRefusal(f"the arm did not take the command ({error}): a slow or dead line, not a refusal by the controller.", rhal.SKR_EIO) from error
        if self._estop:
            raise BridgeRefusal("the stop was latched while the command was being made.", rhal.SKR_EPERM)
        with self._lock:
            self._goal = goal

    def _take_warnings(self) -> str:
        """The controller's warnings since the last command, in its own words, once (under `_lock`): the answer to a command is said by the next one, because the controller says it after it has acknowledged."""
        fresh = [w for w in self._warns if w[0] > self._warn_taken]
        self._warn_taken = self._warn_seq
        if not fresh:
            return ""
        return ", ".join(f"code {code} ({frames.describe_code(code)})" for _, _, code in fresh[-3:])

    # ── the reading thread ───────────────────────────────────────────────────

    def _exchange_loop(self, wake_r: int) -> None:
        """The reading thread: the arm's state in (a `reqdata` each period), the controller's lines in, and, when something is owed, the stop out. It waits on the data port and on the command port and on a pipe that a stop
        writes to, so a stop is on the wire at once."""
        data, command = self._data, self._command
        assert data is not None and command is not None
        period = 1.0 / self._rate_hz
        next_at = time.monotonic()
        open_error: str | None = None
        while not self._closing.is_set():
            try:
                self._pump(command, 0.0, wake_r)
                if not data.is_open:
                    raise link.LinkError(f"{data.target} is not open.")
                packet = data.request(self._timeout, wake_r, on_wake=lambda: self._on_wake(wake_r))
                reading = frames.decode_state(packet)
                self._observe(reading)
            except (link.LinkError, frames.ProtocolError, BridgeRefusal, ValueError) as error:
                reason = str(error)
                if not data.is_open and not self._closing.is_set():
                    if time.monotonic() - self._data_reopened >= _REOPEN_S:
                        # A control box that went away: the data port is opened again, at most twice a second, and what it says is what is reported while it stays shut.
                        self._data_reopened = time.monotonic()
                        try:
                            data.open(timeout=min(self._timeout, 0.5))
                            open_error = None
                        except link.LinkError as again:
                            open_error = str(again)
                    reason = open_error or reason
                with self._lock:
                    self._failures += 1
                    if self._failures >= _FAULT_THRESHOLD:
                        self._dead = reason
                    self._changed.notify_all()
                time.sleep(0.005)
            else:
                with self._lock:
                    self._latest, self._stamp_ns = reading, rhal.acquisition_ns()
                    self._packets += 1
                    self._failures, self._dead = 0, None
                    self._changed.notify_all()
            if not command.is_open and not self._closing.is_set():
                self._reopen_command()
            next_at += period
            now = time.monotonic()
            if next_at < now:
                next_at = now
            while not self._closing.is_set():
                remaining = next_at - time.monotonic()
                if remaining <= 0:
                    break
                try:
                    self._pump(command, min(remaining, 0.05), wake_r)
                except link.LinkError:
                    break

    def _on_wake(self, wake_r: int) -> None:
        self._drain_wake(wake_r)
        try:
            self._deliver_if_owed()
        except (link.LinkError, frames.ProtocolError, BridgeRefusal):
            pass

    def _pump(self, command: link.CommandChannel, wait: float, wake_r: int) -> None:
        """The controller's lines, as they come, for at most `wait`: and a woken wait is a stop owed."""
        if not command.is_open:
            if wait > 0:
                time.sleep(min(wait, 0.01))
            return
        try:
            events, woke = command.poll(wait, (wake_r,))
        except link.LinkError:
            return          # the connection is closed; it is opened again at most twice a second, and a write to it says why it failed
        if woke:
            self._on_wake(wake_r)
        if events:
            self._take_events(events)

    def _take_events(self, events: list[frames.Event]) -> None:
        now = time.monotonic()
        with self._lock:
            for event in events:
                if event.kind in ("ack", "rejected"):
                    if self._pending:
                        self._pending.popleft()
                    self._acks += 1
                    if event.kind == "rejected":
                        self._rejected += 1
                elif event.kind == "warn" and event.code is not None:
                    self._warn_seq += 1
                    self._warns.append((self._warn_seq, now, event.code))
                    del self._warns[:-32]
                elif event.kind == "error" and event.code is not None:
                    if event.code not in self._errors:
                        self._errors.append(event.code)
                        del self._errors[:-64]
                elif event.kind == "error":
                    self._messages.append(f"error[{event.category}]: {event.message.strip()[:200]}")
                elif event.kind == "info" and event.category == "system" and event.message == "halt":
                    self._halts += 1
                elif event.kind == "unknown":
                    self._unknown += 1
            self._changed.notify_all()

    def _reopen_command(self) -> None:
        command = self._command
        if command is None or command.is_open:
            return
        now = time.monotonic()
        if now - self._command_reopened < _REOPEN_S:
            return
        self._command_reopened = now
        with contextlib.suppress(link.LinkError):
            command.open(timeout=min(self._timeout, 0.5))
            with self._lock:
                # What was waiting for an acknowledgement on the old connection will never get one.
                self._pending = deque()

    def _observe(self, reading: frames.State) -> None:
        """What a good packet shows is owed: a stop that has not been written."""
        del reading
        self._deliver_if_owed()

    def _stop_owed(self) -> bool:
        with self._lock:
            return self._estop and not self._stop_delivered

    def _deliver_if_owed(self) -> None:
        if self._stop_owed() and time.monotonic() >= self._stop_retry_at:
            try:
                self._deliver_stop()
            except (link.LinkError, frames.ProtocolError, BridgeRefusal):
                self._stop_retry_at = time.monotonic() + 0.05          # a stop that could not be written is tried again, but not at the packet rate
                raise

    def _deliver_stop(self) -> None:
        """The controller's stop, written by the reading thread: `task stop`."""
        with self._lock:
            calls = self._stop_calls
        self._write(frames.task_stop(), _COMMAND_BUDGET_S)
        with self._lock:
            if calls != self._stop_calls:
                return
            self._stop_delivered = True
            self._goal = None
            self._changed.notify_all()

    def _wait_for_packet(self, predicate, timeout: float, *, honour_stop: bool = True) -> frames.State:
        """The first packet after now that `predicate` holds for."""
        deadline = time.monotonic() + timeout
        with self._changed:
            seen = self._packets
            while True:
                if self._closing.is_set():
                    raise BridgeRefusal("the driver was closed.", rhal.SKR_EPERM)
                if honour_stop and self._estop:
                    raise BridgeRefusal("the motion ended: the stop was latched.", rhal.SKR_EPERM)
                if self._packets > seen and self._latest is not None and predicate(self._latest):
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
        self._wake()
        with self._changed:
            self._changed.notify_all()
        loop, self._loop = self._loop, None
        ended = True
        if loop is not None:
            loop.join(timeout=2.0)
            ended = not loop.is_alive()
        for connection in (self._command, self._data):
            if connection is not None:
                connection.close()
        self._command = self._data = None
        with self._lock:
            self._latest = None
        if ended:
            self._close_wake()          # a thread that did not end keeps its end of the pipe: it is let go with the process, not closed under it

    def _build_descriptor(self) -> str:
        joints = []
        for joint, name in enumerate(JOINTS):
            low, high = self._limits[joint]
            joints.append({"name": name, "role": self._roles.get(name), "type": "revolute", "limits": {"position_min_rad": low, "position_max_rad": high}, "control_modes": ["position"]})
        effector = {"name": "flange", "role": None, "kind": "flange", "dof": 0, "max_grip_force_n": None, "max_opening_m": None}
        descriptor = {
            "schema_version": 1, "robot_slug": "rainbow-" + self._model.lower().replace("_", "-"), "digest": None, "verified": False, "simulator": self._simulated, "base": "fixed", "dof": 6,
            "joints": joints, "end_effectors": [effector], "sensors": [],
            "control": {"modes": ["position"], "rate_hz_max": self._rate_hz, "rate_hz_min": 1.0},
            # Null: the ranges are the operator's, the speed is the controller's own limit and the servo parameters, and none of it is a risk assessment (§5.2).
            "envelope": None, "capabilities": ["safety.estop"], "motions": [_descriptor(name) for name in _MOTIONS], "extensions": [TOKEN], "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))

    # ── the extension: motions ───────────────────────────────────────────────

    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        if token != TOKEN:
            return rhal.SKR_ENOSYS, f"unknown token {token!r}. This driver serves {TOKEN}.".encode()
        if not self._opened or self._command is None:
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
            return rhal.SKR_EIO, f"the arm did not carry the request: {error}".encode()

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

    def _present(self) -> frames.State:
        with self._lock:
            reading = self._latest
        if reading is None:
            raise BridgeRefusal("the arm has not been read yet.", rhal.SKR_ETIMEDOUT)
        return reading

    def _do(self, text: str) -> None:
        """A line a motion writes. What the controller says of it is read from the state and the events, not waited for."""
        self._write(text, _COMMAND_BUDGET_S)

    def _motion_status(self, args: dict) -> dict:
        self._no_arguments("status", args)
        reading = self._present()
        with self._lock:
            packets, dead, delivered, goal, errors, warns, version = self._packets, self._dead, self._stop_delivered, self._goal, list(self._errors), list(self._warns), self._system_version
            pending, acks, messages, unknown, rejected, halts = len(self._pending), self._acks, list(self._messages), self._unknown, self._rejected, self._halts
        verdict = _classify(reading, errors)
        why = self._not_commandable(reading)
        return {"model": self._model, "link": self._command.target if self._command else None, "data": self._data.target if self._data else None, "simulator": self._simulated, "software_version": version,
                "mode": reading.mode, "mode_name": _mode_name(reading.mode), "init_state": reading.init_state, "activated": reading.init_state == frames.ACTIVATED, "power_applied": reading.power_applied,
                "task_state": reading.task_state, "paused": reading.task_state == frames.TASK_PAUSED or bool(reading.paused_flag), "moving": reading.robot_state != frames.ROBOT_IDLE, "robot_state": reading.robot_state,
                "fault": None if verdict is None else {"class": verdict[0], "code": verdict[1], "text": _say_code(verdict[1])}, "errors": [{"code": c, "text": frames.describe_code(c)} for c in errors],
                "warnings": [{"code": c, "text": frames.describe_code(c), "age_s": round(time.monotonic() - at, 2)} for _, at, c in warns[-5:]], "messages": messages, "not_commandable": why,
                "sos": reading.sos, "ems": reading.ems, "collision": reading.collision, "joint_info": list(reading.joint_info), "sos_state": reading.sos_state, "estop_bits": list(reading.estop_bits),
                "speed_bar": round(reading.speed_bar, 4), "freedrive": reading.freedrive, "position_rad": [round(a * _DEG, 6) for a in self._position(reading)], "position_deg": [round(a, 4) for a in self._position(reading)],
                "reference_deg": [round(a, 4) for a in reading.ref], "encoder_deg": [round(a, 4) for a in reading.ang], "goal_deg": goal, "temperatures": [round(t, 1) for t in reading.temperature],
                "payload_configured": None if self._payload is None else {"kg": self._payload[0], "com_mm": list(self._payload[1])}, "packets": packets, "link_dead": dead, "acknowledged": acks, "unacknowledged": pending,
                "refused_lines": rejected, "unknown_lines": unknown, "halts_seen": halts, "stop_latched": self._estop, "stop_delivered": delivered}

    def _motion_set_mode(self, args: dict) -> dict:
        word = args.pop("mode", None)
        self._no_arguments("set_mode", args)
        if word not in ("real", "simulation"):
            raise BridgeRefusal("`set_mode` takes `mode=real` or `mode=simulation`.", rhal.SKR_EINVAL)
        if (word == "simulation") != self._simulated:
            raise BridgeRefusal(f"this session says `simulator: {str(self._simulated).lower()}`, so the controller is to be in its {'simulation' if self._simulated else 'real'} mode and not its {word} one: "
                                "open the driver again with the claim that is true. In the real mode the arm physically moves on a command, and that is the integrator's to say, not a motion's to change.", rhal.SKR_EPERM)
        reading = self._present()
        if reading.robot_state != frames.ROBOT_IDLE:
            raise BridgeRefusal("the controller is executing a motion command, and the mode is changed only when it is idle.", rhal.SKR_EPERM)
        self._do(frames.pgmode(word == "real"))
        wanted = frames.MODE_REAL if word == "real" else frames.MODE_SIMULATION
        try:
            self._wait_for_packet(lambda r: r.mode == wanted, self._timeout * 6 + 1.0)
        except BridgeRefusal as error:
            raise BridgeRefusal(f"`pgmode {word}` was written and no reading has shown the controller in that mode ({error}). The Virtual Control Box has no real mode.", rhal.SKR_EIO) from None
        return {"result": "set", "mode": word}

    def _motion_activate(self, args: dict) -> dict:
        timeout = self._number(args, "timeout", minimum=1.0, maximum=600.0)
        self._no_arguments("activate", args)
        reading = self._present()
        if self._simulated:
            return {"result": "not_needed", "note": "the controller is in its simulation mode, which takes motion commands with no activation: only the internal reference moves"}
        if reading.init_state == frames.ACTIVATED:
            return {"result": "already_activated"}
        self._do(frames.activate())
        try:
            after = self._wait_for_packet(lambda r: r.init_state == frames.ACTIVATED or r.sos != 0, timeout if timeout is not None else 60.0)
        except BridgeRefusal as error:
            raise BridgeRefusal(f"`mc jall init` was written and no reading has shown the activation done ({error}).", rhal.SKR_EIO) from None
        if after.init_state != frames.ACTIVATED:
            raise BridgeRefusal(f"the arm reported {_sos_text(after.sos)} during its activation: read it with `status`.", rhal.SKR_EIO)
        told = None
        if self._payload is not None:
            self._do(frames.set_payload_info(*self._payload))
            told = {"kg": self._payload[0], "com_mm": list(self._payload[1])}
        return {"result": "activated", "payload_told": told}

    def _motion_resume(self, args: dict) -> dict:
        which = args.pop("after", "pause")
        self._no_arguments("resume", args)
        if which not in ("pause", "collision"):
            raise BridgeRefusal("`resume` takes `after=pause` (a pause, an alarm or a debug stop: `task resume_a`) or `after=collision` (external collision detection: `task resume_b`).", rhal.SKR_EINVAL)
        reading = self._present()
        if reading.task_state != frames.TASK_PAUSED and not reading.paused_flag:
            return {"result": "not_paused"}
        self._do(frames.task_resume(which == "collision"))
        try:
            after = self._wait_for_packet(lambda r: r.task_state != frames.TASK_PAUSED and not r.paused_flag, self._timeout * 6 + 1.0)
        except BridgeRefusal as error:
            raise BridgeRefusal(f"`task resume` was written and no reading has shown the controller running again ({error}).", rhal.SKR_EIO) from None
        return {"result": "resumed", "task_state": after.task_state}

    def _motion_clear_faults(self, args: dict) -> dict:
        self._no_arguments("clear_faults", args)
        reading = self._present()
        causes = _causes(reading)
        if causes:
            raise BridgeRefusal(f"the controller still reports {'; '.join(causes)}: what was reported is not acknowledged while its cause is there. Remove it at the controller.", rhal.SKR_EPERM)
        with self._lock:
            cleared, self._errors = list(self._errors), []
            self._warns, self._warn_taken = [], self._warn_seq
            self._messages.clear()
        return {"result": "cleared" if cleared else "nothing_to_clear", "codes": cleared, "note": "an acknowledgement in this driver: the controller reports a fault once and has no list to clear"}

    def _motion_set_payload(self, args: dict) -> dict:
        kg = self._number(args, "kg", minimum=0.0, maximum=100.0)
        com = tuple(self._number(args, axis, minimum=-2000.0, maximum=2000.0) or 0.0 for axis in ("x", "y", "z"))
        self._no_arguments("set_payload", args)
        if kg is None:
            raise BridgeRefusal("`set_payload` needs `kg=`, and takes `x=`, `y=` and `z=` for the centre of mass in millimetres (0 where absent).", rhal.SKR_EINVAL)
        self._gate()
        self._do(frames.set_payload_info(kg, com))
        with self._lock:
            self._payload = (kg, (com[0], com[1], com[2]))
        return {"result": "told", "kg": kg, "com_mm": list(com), "note": "the controller returns to the Setup page's payload when its program ends, and says nothing back"}

    def _motion_move(self, args: dict) -> dict:
        timeout = self._number(args, "timeout", minimum=0.5, maximum=300.0)
        speed = self._number(args, "speed", minimum=0.1, maximum=360.0)
        accel = self._number(args, "accel", minimum=0.1, maximum=3600.0)
        targets: dict[int, float] = {}
        for name in list(args):
            if name not in JOINTS:
                raise BridgeRefusal(f"`{name}` is not a joint ({', '.join(JOINTS)}), `speed`, `accel` or `timeout`.", rhal.SKR_EINVAL)
            joint = JOINTS.index(name)
            low, high = self._limits[joint]
            value = self._number(args, name, minimum=low, maximum=high)
            assert value is not None
            targets[joint] = value
        if not targets:
            raise BridgeRefusal(f"`move` needs at least one joint, as `<joint>=<radians>`: {', '.join(JOINTS)}.", rhal.SKR_EINVAL)
        reading = self._gate()
        goal = [float(a) for a in self._position(reading)]
        for joint, value in targets.items():
            goal[joint] = value / _DEG
        speed = 20.0 if speed is None else speed
        accel = 40.0 if accel is None else accel
        wait = timeout if timeout is not None else max(abs(goal[j] - self._position(reading)[j]) for j in targets) / max(speed, 1e-9) + 5.0
        self._do(frames.move_j(goal, speed, accel))
        with self._lock:
            self._goal = list(goal)
        started = [False]

        def done(packet: frames.State) -> bool:
            if _classify(packet, []) is not None:
                return True
            position = self._position(packet)
            started[0] = started[0] or packet.robot_state != frames.ROBOT_IDLE or max(abs(position[j] - self._position(reading)[j]) for j in targets) > _ARRIVAL_TOLERANCE_DEG
            if packet.robot_state != frames.ROBOT_IDLE:
                return False
            return started[0] or max(abs(position[j] - goal[j]) for j in targets) <= _ARRIVAL_TOLERANCE_DEG

        try:
            after = self._wait_for_packet(done, wait)
        except BridgeRefusal as error:
            if error.code == rhal.SKR_EIO and "no reading showed" in str(error) and not started[0]:
                raise BridgeRefusal(f"the arm never started moving after move_j (no motion command executing, and the joints did not change, in {wait:.1f} s): {error}", rhal.SKR_EIO) from None
            raise
        if _classify(after, self._errors_copy()) is not None:
            raise BridgeRefusal("the arm reported a fault during the move: read it with `status`.", rhal.SKR_EIO)
        position = self._position(after)
        arrived = max(abs(position[j] - goal[j]) for j in targets) <= _ARRIVAL_TOLERANCE_DEG
        return {"result": "arrived" if arrived else "stopped_short", "position_deg": [round(a, 4) for a in position], "goal_deg": [round(g, 4) for g in goal]}

    def _errors_copy(self) -> list[int]:
        with self._lock:
            return list(self._errors)


def _sos_text(sos: int) -> str:
    return f"device error {sos} ({frames.SOS_FLAGS.get(sos, 'not in the document')})" if sos else "no error"


def _mode_name(mode: int) -> str:
    return {frames.MODE_REAL: "real", frames.MODE_SIMULATION: "simulation"}.get(mode, f"mode {mode}, which the document does not list")


def _say_code(code: int) -> str:
    """A fault word in the terms it was made of."""
    band, value = code >> 16, code & 0xFFFF
    if band == 0:
        return f"message code {code}: {frames.describe_code(code)}"
    if band == 1:
        return f"device error {value}: {frames.SOS_FLAGS.get(value, 'not in the document')}"
    if band == 2:
        return f"software emergency stop {value}: {frames.EMS_FLAGS.get(value, 'not in the document')}"
    if band == 3:
        return "an external collision was detected"
    if band == 4:
        return f"joint {(value >> 8) + 1}: {frames.JOINT_ERROR_BITS.get(value & 0xFF, 'a bit the document does not list')}"
    if band == 5:
        return f"mode {value}, which the document does not list"
    return "the controller's SOS state"


def _causes(reading: frames.State) -> list[str]:
    """What the state packet itself says is wrong, in words: empty for an arm that holds no fault."""
    causes: list[str] = []
    if reading.sos:
        causes.append(f"a device error ({frames.SOS_FLAGS.get(reading.sos, 'not in the document')})")
    if reading.ems:
        causes.append(f"a software emergency stop ({frames.EMS_FLAGS.get(reading.ems, 'not in the document')})")
    if reading.collision:
        causes.append("an external collision")
    if reading.sos_state:
        causes.append("the SOS state")
    for joint, word in enumerate(reading.joint_info):
        for bit, name in frames.JOINT_ERROR_BITS.items():
            if word >> bit & 1:
                causes.append(f"{JOINTS[joint]}: {name}")
    if reading.mode not in (frames.MODE_REAL, frames.MODE_SIMULATION):
        causes.append(f"a mode the document does not list ({reading.mode})")
    return causes


def _classify(reading: frames.State, errors: list[int]) -> tuple[str, int] | None:
    """('estop' | 'fault', the fault word) for an arm that is not taking commands because of something it reported, and None for one that is.

    The word is the code of the controller's first `error[code][N]` since the last acknowledgement (an emergency stop first: a safety switch's), where there is one, and otherwise the cause the state packet gives, in a band above
    every code of the maker's table: a device error, a software emergency stop, a collision, a joint's error bit, a mode the document does not list, the SOS state. An unknown state is not a healthy one, so a mode that is
    neither real nor simulation is a fault."""
    stops = [code for code in errors if code in frames.ESTOP_CODES]
    if stops:
        return "estop", stops[0]
    if errors:
        return "fault", errors[0]
    if reading.sos:
        return "fault", SOS_BAND | reading.sos
    if reading.ems:
        return "fault", EMS_BAND | reading.ems
    if reading.collision:
        return "fault", COLLISION_BAND | reading.collision
    for joint, word in enumerate(reading.joint_info):
        for bit in frames.JOINT_ERROR_BITS:
            if word >> bit & 1:
                return "fault", JOINT_BAND | joint << 8 | bit
    if reading.mode not in (frames.MODE_REAL, frames.MODE_SIMULATION):
        return "fault", MODE_BAND | reading.mode
    if reading.sos_state:
        return "fault", SOS_STATE
    return None


def _descriptor(name: str) -> dict:
    if name in ("status", "clear_faults", "activate"):
        return {"name": name, "parameters": [{"name": "timeout", "unit": "s", "required": False}] if name == "activate" else []}
    if name == "set_mode":
        return {"name": name, "parameters": [{"name": "mode", "unit": "real|simulation", "required": True}]}
    if name == "resume":
        return {"name": name, "parameters": [{"name": "after", "unit": "pause|collision", "required": False}]}
    if name == "set_payload":
        return {"name": name, "parameters": [{"name": "kg", "unit": "kg", "required": True}] + [{"name": axis, "unit": "mm", "required": False} for axis in ("x", "y", "z")]}
    return {"name": name, "parameters": [{"name": joint, "unit": "rad", "required": False} for joint in JOINTS] + [{"name": "speed", "unit": "deg/s", "required": False}, {"name": "accel", "unit": "deg/s^2", "required": False},
                                                                                                            {"name": "timeout", "unit": "s", "required": False}]}


def _bounded(settings: dict, key: str, default: float, low: float, high: float) -> float:
    value = settings.get(key)
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not low <= value <= high:
        raise BridgeRefusal(f"`{key}` of {value!r} is not a number from {low:g} to {high:g}.")
    return float(value)


def _say(message: str) -> None:
    print(f"skynet-rainbow: {message}", file=sys.stderr, flush=True)
