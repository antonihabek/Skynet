"""The RHAL driver: a Dobot V4 arm (CR, CRA, E6 and Nova) over the controller's TCP interface (DRY-COVERAGE-PLAN.md item 12.E.13).

§4's five mandatory entry points over :mod:`skynet_dobot.frames` and :mod:`skynet_dobot.link`. It links nothing of Dobot's: the dashboard's text protocol (port 29999) and the 1440-byte real-time feedback (port 30004)
are in the interface document, and `layout_check.py` holds this driver's text and its reading of the packet to Dobot's own Python client, which it runs.

# The body

Six revolute joints, `j1` to `j6`, in radians (the controller speaks degrees). It takes a position, one control mode, and its tier ceiling is Guest (§31.3). A position command is one **`ServoJ`**: the controller's own
dynamic-follow call for streaming a target, which "is not affected by the global speed factor but by the speed limits", so how fast the arm follows is the controller's limit and the operator's `servo` parameters, and not
something this driver invents. The joint ranges are the operator's (`limits`, required): the interface document gives none, and the hardware manual's figures are not in anything read here.

# What it never does on its own

It does not take the controller's control mode (`RequestControl()` is a person's, because it takes control from whoever has it), it does not power the arm on (about ten seconds, with brakes releasing), it does not enable
it (that releases the brakes, and carries the payload the operator gave), and it does not clear an alarm. Each is a motion (`request_control`, `power_on`, `enable`, `clear_errors`) that a person asks for. A command is
refused, naming the state, until the arm is enabled and idle or running a motion.

# The stop

`stop_mode` is required and is one of two: **`stop`** is the controller's `Stop()`, which halts the queued motion and leaves the arm enabled where it is; **`emergency`** is its software emergency stop,
`EmergencyStop(1)`, after which the arm is disabled and in alarm and "must have the emergency stop released and the alarm cleared before it can be enabled again". Which is the safer for a given arm and tool is the
integrator's risk assessment, so neither is the default. `emergency_stop` latches and returns and never touches the line; the reading thread writes the stop. It is a **software stop**: the arm's own safety circuit is
a hardware chain, and there is none on this wire.
"""

from __future__ import annotations

import contextlib
import json
import math
import os
import select
import sys
import threading
import time

import skynet_rhal as rhal

from . import frames, link

__all__ = ["BridgeRefusal", "DobotBridge", "TOKEN"]

TOKEN = "vendor.dobot.task"
JOINTS = frames.JOINTS

_RATE_HZ = 33.0
_MAX_RATE_HZ = 250.0
_TRANSACTION_S = 0.08
_OPEN_TIMEOUT_S = 3.0
_STALE_S = 0.5
#: The most a command may take to be written: the control loop waits for it, and a quarter of a second is the contract's bound (`tools/faultproxy/chaos.py`, F256).
_COMMAND_BUDGET_S = 0.25
_FAULT_THRESHOLD = 3
_REOPEN_S = 0.5
#: How far feedback and `GetAngle()` may disagree at open before the units of one of them are in doubt (degrees): the arm may be moving between the two reads.
_UNITS_TOLERANCE_DEG = 2.0
#: A joint within this many degrees of its goal is there.
_ARRIVAL_TOLERANCE_DEG = 0.5
#: How often the alarm list is read while the controller is in its alarm state.
_ALARM_POLL_S = 0.5
_KNOWN_KEYS = {"model", "host", "port", "feedback_port", "stop_mode", "simulator", "limits", "payload_kg", "servo", "roles", "rate_hz", "timeout_ms", "stale_ms", "open_timeout_ms"}
_MOTIONS = ("status", "request_control", "power_on", "enable", "disable", "clear_errors", "move")
#: The alarms the maker's own table (`demo/files/alarmController.py`) describes as an emergency stop: 116 "The emergency stop button is captured", 132 "The software emergency stop signal is triggered", 133 "The user
#: emergency stop signal is triggered". Every other alarm in the table is the arm not doing what it is told.
ESTOP_ALARMS = frozenset({116, 132, 133})
#: The fault word of a RobotMode the interface document does not list: this and the mode, above any alarm identifier of the maker's table (the largest is 4193).
UNLISTED_MODE_BASE = 1 << 16
_RUNNING_MODES = (frames.MODE_ENABLE, frames.MODE_RUNNING, frames.MODE_SINGLE_MOVE)
_DEG = math.pi / 180.0


class BridgeRefusal(Exception):
    """A reason this body did not come up or a command did not happen, in the terms an integrator acts on."""

    def __init__(self, message: str, code: int = rhal.SKR_ENODEV) -> None:
        super().__init__(message)
        self.code = code


class Unreachable(BridgeRefusal):
    """The controller did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""

    def __init__(self, message: str) -> None:
        super().__init__(message, rhal.SKR_EIO)


class DobotBridge:
    """A body made of one Dobot arm's six joints."""

    #: A stop written to the host while a motion runs is read at once: `emergency_stop` is safe from another thread (python-host's `stop_during_extension`).
    stop_during_extension = True

    def __init__(self) -> None:
        self._opened = False
        self._estop = False
        self._descriptor: str | None = None
        self._seq = 0
        self._simulated = True
        self._model = "CR5"
        self._robot_type = 5
        self._limits: list[tuple[float, float]] = [(-math.pi, math.pi)] * 6
        self._payload_kg = 0.0
        self._servo: dict[str, float] = {}
        self._stop_mode = "stop"
        self._roles: dict[str, str | None] = {}
        self._rate_hz = _RATE_HZ
        self._stale_s = _STALE_S
        self._timeout = _TRANSACTION_S
        self._dashboard: link.Dashboard | None = None
        self._feedback: link.FeedbackStream | None = None
        self._loop: threading.Thread | None = None
        self._closing = threading.Event()
        #: Wakes the reading thread: a stop is written by it, at once, and `emergency_stop` never waits on the line. A pipe made when a session opens and closed when it ends, so a driver that is built and never opened (or
        #: opened and closed a thousand times) holds no descriptor it does not use.
        self._wake_r = self._wake_w = -1
        self._wake_lock = threading.Lock()
        self._dashboard_lock = threading.Lock()
        self._lock = threading.Lock()
        self._changed = threading.Condition(self._lock)
        self._last_refusal: str | None = None
        # Everything below is guarded by `_lock`.
        self._latest: frames.Feedback | None = None
        self._stamp_ns = 0
        self._packets = 0
        self._failures = 0
        self._dead: str | None = None
        self._tcp: bool | None = None
        self._goal: list[float] | None = None
        self._commanded_at = 0.0
        self._stop_delivered = False
        self._stop_calls = 0
        self._alarms: list[int] = []
        self._alarms_read_at = 0.0
        self._alarms_for_mode = 0
        self._stop_retry_at = 0.0
        self._dashboard_reopened = 0.0

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
        """A session starts with no stop, no alarm remembered and no reading: nothing of the last one is the arm's."""
        self._estop = False
        self._descriptor = None
        self._last_refusal = None
        with self._lock:
            self._latest = None
            self._goal = None
            self._stop_delivered = False
            self._alarms = []
            self._alarms_read_at = 0.0
            self._alarms_for_mode = 0
            self._tcp = None

    def _open(self, settings: dict) -> None:
        unknown = sorted(set(settings) - _KNOWN_KEYS)
        if unknown:
            raise BridgeRefusal(f"the configuration has {', '.join(map(repr, unknown))}, which this driver does not read. The keys it reads are {', '.join(sorted(_KNOWN_KEYS))}.")
        by_name = {name: number for number, name in frames.ROBOT_TYPES.items()}
        model = settings.get("model")
        if not isinstance(model, str) or model not in by_name:
            raise BridgeRefusal(f"`model` of {model!r} is not in the configuration, or is not one of the interface document's robot types ({', '.join(map(repr, by_name))}). The controller reports its own type, but a joint's range "
                                "and a payload's limit depend on the arm, so the operator says which it is and the open checks the controller agrees.")
        self._model, self._robot_type = model, by_name[model]
        host = settings.get("host")
        port = settings.get("port", frames.DASHBOARD_PORT)
        feedback_port = settings.get("feedback_port", frames.FEEDBACK_PORTS[0])
        for key, value in (("port", port), ("feedback_port", feedback_port)):
            if isinstance(value, bool) or not isinstance(value, int) or not 0 < value < 65536:
                raise BridgeRefusal(f"`{key}` of {value!r} is not a port from 1 to 65535 (the controller's dashboard is {frames.DASHBOARD_PORT} and its feedback {frames.FEEDBACK_PORTS[0]}).")
        if not isinstance(host, str) or not host:
            raise BridgeRefusal(f"`host` of {host!r} is not the controller's address: a host name or an IP address (192.168.5.1 from the factory). This driver does not scan for an arm.")
        stop_mode = settings.get("stop_mode")
        if stop_mode not in ("stop", "emergency"):
            raise BridgeRefusal("`stop_mode` is not in the configuration, or is not `stop` or `emergency`. The controller's `Stop()` halts the queued motion and leaves the arm enabled where it is; its software emergency stop "
                                "disables the arm and puts it in alarm, to be released and cleared before it can be enabled again. Which one a stop should be, with this tool and this payload, is the integrator's: "
                                "say `\"stop_mode\": \"stop\"` or `\"stop_mode\": \"emergency\"`.")
        self._stop_mode = stop_mode
        claim = settings.get("simulator")
        if not isinstance(claim, bool):
            raise BridgeRefusal("`simulator` is not in the configuration, or is not true or false. Nothing the controller says tells a real arm from a simulation, so the descriptor cannot be left to guess.")
        self._simulated = claim
        self._limits = self._read_limits(settings.get("limits"))
        payload = settings.get("payload_kg")
        if isinstance(payload, bool) or not isinstance(payload, (int, float)) or not math.isfinite(payload) or not 0 <= payload <= 100:
            raise BridgeRefusal(f"no `payload_kg` in the configuration, or {payload!r} is not a number from 0 to 100: the load on the flange, which the controller's gravity compensation and collision detection depend on. "
                                "It is told to the arm when it is enabled, and what it weighs is the operator's.")
        self._payload_kg = float(payload)
        self._servo = self._read_servo(settings.get("servo"))
        roles = settings.get("roles") or {}
        if not isinstance(roles, dict) or set(roles) - set(JOINTS):
            raise BridgeRefusal(f"`roles` is a mapping from a joint's name to its role, and the joints are {', '.join(JOINTS)}.")
        self._roles = {name: roles.get(name) for name in JOINTS}
        self._rate_hz = _bounded(settings, "rate_hz", _RATE_HZ, 1.0, _MAX_RATE_HZ)
        self._timeout = _bounded(settings, "timeout_ms", _TRANSACTION_S * 1000, 10.0, 5000.0) / 1000.0
        self._stale_s = _bounded(settings, "stale_ms", _STALE_S * 1000, 20.0, 30000.0) / 1000.0
        open_s = _bounded(settings, "open_timeout_ms", _OPEN_TIMEOUT_S * 1000, 100.0, 60000.0) / 1000.0

        self._dashboard = link.Dashboard(host, port)
        self._feedback = link.FeedbackStream(host, feedback_port)
        self._feedback.open(timeout=min(open_s, 3.0))
        first = self._first_packet(open_s)
        if first.robot_type != self._robot_type:
            raise BridgeRefusal(f"the controller reports robot type {first.robot_type} ({frames.ROBOT_TYPES.get(first.robot_type, 'not in the document')}), and a {self._model} is {self._robot_type}. A joint's range and a payload's "
                                "limit are the arm's, so nothing was commanded: check `model` against what is mounted.")
        self._dashboard.open(timeout=min(open_s, 3.0))
        self._probe_control(first, open_s)
        with self._lock:
            self._latest, self._stamp_ns = first, rhal.acquisition_ns()
            self._packets, self._failures, self._dead = 1, 0, None
        self._closing.clear()
        self._open_wake()
        self._loop = threading.Thread(target=self._exchange_loop, args=(self._wake_r,), daemon=True, name="dobot-exchange")
        self._loop.start()
        self._descriptor = self._build_descriptor()

    def _read_limits(self, given: object) -> list[tuple[float, float]]:
        if given is None:
            raise BridgeRefusal("`limits` is not in the configuration. The controller's interface gives no joint range, and the hardware manual's is not read here: it is the operator's, for this arm and this cell, "
                                "as six [min, max] pairs in radians.")
        if not (isinstance(given, (list, tuple)) and len(given) == 6 and all(isinstance(pair, (list, tuple)) and len(pair) == 2 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) for v in pair) for pair in given)):
            raise BridgeRefusal("`limits` is six [min, max] pairs in radians, one for each of j1 to j6, as numbers.")
        out: list[tuple[float, float]] = []
        for name, (low, high) in zip(JOINTS, given):
            if not low < high:
                raise BridgeRefusal(f"`limits` of {name} is [{low}, {high}], which is not a range.")
            if low < -2 * math.pi - 1e-9 or high > 2 * math.pi + 1e-9:
                raise BridgeRefusal(f"`limits` of {name} is [{low}, {high}] rad, past a turn either way, which no joint of these arms has.")
            out.append((float(low), float(high)))
        return out

    def _read_servo(self, given: object) -> dict[str, float]:
        """`servo`: the optional parameters of ServoJ, each checked against the document's range. Absent ones are not sent, and the controller's own defaults (0.1 s, 50, 500) apply."""
        if given is None:
            return {}
        names = {"t": frames.SERVO_T, "aheadtime": frames.SERVO_AHEAD, "gain": frames.SERVO_GAIN}
        if not isinstance(given, dict) or set(given) - set(names):
            raise BridgeRefusal(f"`servo` is a mapping from {', '.join(names)} to numbers: the optional parameters of ServoJ.")
        out: dict[str, float] = {}
        for name, value in given.items():
            low, high, _ = names[name]
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or not low <= value <= high:
                raise BridgeRefusal(f"`servo` {name} of {value!r} is not a number from {low:g} to {high:g} (the interface document's range).")
            out[name] = float(value)
        return out

    def _first_packet(self, open_s: float) -> frames.Feedback:
        assert self._feedback is not None
        deadline = time.monotonic() + open_s
        last = "nothing arrived"
        while time.monotonic() < deadline:
            try:
                packet = self._feedback.read_packet(min(deadline - time.monotonic(), 0.25))
            except link.LinkError as error:
                last = str(error)
                raise Unreachable(f"the controller's feedback did not come: {error} Is the controller powered and reachable at {self._feedback.target}?") from error
            if packet is None:
                continue
            try:
                return frames.decode_feedback(packet)
            except frames.ProtocolError as error:
                last = str(error)
        raise Unreachable(f"no feedback packet arrived from {self._feedback.target} in {open_s:.1f} s ({last}). Is the controller powered, and is {self._feedback.target} its feedback port?")

    def _probe_control(self, first: frames.Feedback, open_s: float) -> None:
        """Whether the controller takes TCP commands, by `RobotMode()`; and, if it does, whether `GetAngle()` and the feedback agree, which they cannot if one of them is not in the degrees the document uses."""
        assert self._dashboard is not None
        try:
            reply = self._command(frames.robot_mode(), min(open_s, 1.0))
        except link.NotTcp:
            with self._lock:
                self._tcp = False
            return
        except link.LinkError as error:
            raise Unreachable(f"the controller's dashboard did not answer at {self._dashboard.target} ({error}). Is it the controller's address, and is its TCP interface on?") from error
        with self._lock:
            self._tcp = True
        if reply.error != 0:
            raise BridgeRefusal(f"the controller answered RobotMode() with error {reply.error} ({frames.describe_error(reply.error)}).", rhal.SKR_EIO)
        frames.numbers(reply.payload, 1)
        if first.robot_mode in (frames.MODE_ERROR, frames.MODE_POWEROFF):
            return          # GetAngle() is not taken in these states (the document's section 5)
        angle = self._command(frames.get_angle(), min(open_s, 1.0))
        if angle.error != 0:
            return
        degrees = frames.numbers(angle.payload, 6)
        worst = max(abs(a - b) for a, b in zip(degrees, first.q_actual))
        if worst > _UNITS_TOLERANCE_DEG:
            raise BridgeRefusal(f"GetAngle() says {[round(d, 2) for d in degrees]} and the feedback's QActual {[round(q, 2) for q in first.q_actual]}: they differ by {worst:.1f}, and the document gives both in degrees. "
                                "One of them is in another unit, or the arm is moving fast: nothing was commanded.")

    def close(self) -> int:
        # A close writes nothing: the arm keeps its last target and its state, and what stopping it does is the integrator's to decide (see `stop_mode`), never a close's.
        self._teardown()
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        now = rhal.acquisition_ns()
        with self._lock:
            reading, stamp, dead, alarms = self._latest, self._stamp_ns, self._dead, list(self._alarms)
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
        out.position_rad = [q * _DEG for q in reading.q_actual]
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP | rhal.FLAG_DEGRADED
        verdict = _classify(reading, alarms)
        if verdict is not None:
            kind, code = verdict
            out.flags |= (rhal.FLAG_ESTOP if kind == "estop" else rhal.FLAG_FAULT) | rhal.FLAG_DEGRADED
            out.fault_code = code
        elif reading.robot_mode not in _RUNNING_MODES:
            out.flags |= rhal.FLAG_DEGRADED         # an arm that is not enabled takes no command
        age = (rhal.acquisition_ns() - stamp) / 1e9
        if dead or age > self._stale_s:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened or self._dashboard is None:
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
        The latch is the stop: commands are refused from this instant, and the controller's stop is on the wire as soon as the line carries it, which `status` reports."""
        with self._lock:
            self._estop = True
            self._stop_calls += 1
            self._stop_delivered = False
            self._changed.notify_all()
        self._wake()
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """Clear the session latch (§48.2) once the arm has been *seen* to take the stop: a reading after the stop was written shows the arm not moving (`stop`), or in its alarm state (`emergency`, which is then released
        and cleared, and leaves the arm disabled)."""
        if self._dashboard is None or self._loop is None:
            self._estop = False
            return rhal.SKR_OK
        try:
            if self._stop_mode == "emergency":
                self._wait_for_packet(lambda r: self._stop_delivered and r.robot_mode in (frames.MODE_ERROR, frames.MODE_DISABLED, frames.MODE_POWEROFF), _COMMAND_BUDGET_S * 4 + self._timeout * 6, honour_stop=False)
            else:
                self._wait_for_packet(lambda r: self._stop_delivered and r.running_status == 0 and r.robot_mode != frames.MODE_RUNNING and r.robot_mode != frames.MODE_SINGLE_MOVE, _COMMAND_BUDGET_S * 4 + self._timeout * 6, honour_stop=False)
        except BridgeRefusal as error:
            _say(f"the stop stays latched: no reading has shown the arm take it ({error}) Fix the line, or close and open the driver, which is a new session.")
            return rhal.SKR_EIO
        if self._stop_mode == "emergency":
            try:
                self._release_software_stop()
            except (BridgeRefusal, link.LinkError, frames.ProtocolError) as error:
                _say(f"the stop stays latched: the software emergency stop could not be released ({error}).")
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

    def _command(self, text: str, budget: float | None = None, by: float | None = None, *, unless_stopped: bool = False) -> frames.Reply:
        """One command on the dashboard, under its lock. With `by` (a monotonic time) the wait for the lock and the reply both end there. With `unless_stopped` a stop that was latched while the command waited for the
        line is the answer: a command is not written after the stop is latched, so on the wire the stop follows every command that was in flight and never precedes one."""
        dashboard = self._dashboard
        if dashboard is None:
            raise link.LinkError("the dashboard is not open.")
        wanted = budget or self._timeout
        if by is None:
            self._dashboard_lock.acquire()
        elif not self._dashboard_lock.acquire(timeout=max(0.0, by - time.monotonic())):
            raise link.LinkError("the dashboard was in use until the time the caller had been given ran out.")
        try:
            if unless_stopped and self._estop:
                raise link.LinkError("the stop was latched before the command was written.")
            if by is not None:
                remaining = by - time.monotonic()
                if remaining <= 0:
                    raise link.LinkError("the time the caller had been given ran out before the dashboard was free.")
                wanted = min(wanted, remaining)
            return dashboard.command(text, wanted)
        finally:
            self._dashboard_lock.release()

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

    def _gate(self) -> frames.Feedback:
        """What a command waits for: an arm that has been read, that is taking commands from this line, that holds no alarm, and that is enabled."""
        with self._lock:
            reading, tcp, alarms = self._latest, self._tcp, list(self._alarms)
        if reading is None:
            raise BridgeRefusal("the arm has not been read yet.", rhal.SKR_ETIMEDOUT)
        if tcp is False:
            raise BridgeRefusal("the controller's control mode is not TCP, so it takes no command from this line. The `request_control` motion asks for control, which takes it from whoever has it: a person's decision.", rhal.SKR_EPERM)
        verdict = _classify(reading, alarms)
        if verdict is not None:
            kind, code = verdict
            if alarms:
                what = f"alarm {alarms[0]}"
            elif reading.robot_mode not in frames.MODES:
                what = f"mode {reading.robot_mode}, which the interface document does not list"
            elif reading.robot_mode == frames.MODE_COLLISION or reading.collision_state:
                what = frames.MODES[frames.MODE_COLLISION]
            else:
                what = frames.MODES[reading.robot_mode]
            raise BridgeRefusal(f"the arm is in {'emergency stop' if kind == 'estop' else 'an alarm state'} ({what}), and an arm holding one takes nothing: the `clear_errors` motion clears an alarm, and an emergency stop is "
                                "released first. The command was not sent.", rhal.SKR_EPERM)
        if reading.robot_mode not in _RUNNING_MODES:
            raise BridgeRefusal(f"the arm is {frames.MODES.get(reading.robot_mode, 'in mode ' + str(reading.robot_mode))}, and an arm that is not enabled takes no target: "
                                f"{'the `power_on` motion powers it' if reading.robot_mode == frames.MODE_POWEROFF else 'the `enable` motion enables it'}. The command was not sent.", rhal.SKR_EPERM)
        return reading

    def _send_servo(self, targets: list[float | None], reading: frames.Feedback) -> None:
        """One ServoJ of every joint: the ones the command applies at their targets, and the others where this driver last asked them to be (or, at first, where they are)."""
        deadline = time.monotonic() + _COMMAND_BUDGET_S
        with self._lock:
            held = list(self._goal) if self._goal is not None else list(reading.q_actual)
        goal = [held[j] if targets[j] is None else float(targets[j]) for j in range(6)]       # type: ignore[arg-type]
        text = frames.servo_j(goal, **{name: self._servo[name] for name in ("t", "aheadtime", "gain") if name in self._servo})
        try:
            reply = self._command(text, by=deadline, unless_stopped=True)
        except link.NotTcp as error:
            with self._lock:
                self._tcp = False
            raise BridgeRefusal(f"the controller's control mode is not TCP ({error}). The command was not sent.", rhal.SKR_EPERM) from error
        except frames.ProtocolError as error:
            raise BridgeRefusal(f"the controller's reply to ServoJ is not one the interface document allows ({error}).", rhal.SKR_EIO) from error
        except link.LinkError as error:
            # A command that ran out of time while a stop was being written was refused by the stop, not by a slow line: the latch decides, so the answer does not depend on the race.
            if self._estop:
                raise BridgeRefusal("the stop is latched: nothing was written.", rhal.SKR_EPERM) from error
            raise BridgeRefusal(f"the arm did not take the command ({error}): a slow or dead line, not a refusal by the controller.", rhal.SKR_EIO) from error
        if self._estop:
            raise BridgeRefusal("the stop was latched while the command was being made.", rhal.SKR_EPERM)
        if reply.error != 0:
            raise BridgeRefusal(f"the controller answered ServoJ with error {reply.error} ({frames.describe_error(reply.error)}).", _code_of(reply.error))
        with self._lock:
            self._goal = goal
            self._commanded_at = time.monotonic()

    # ── the reading thread ───────────────────────────────────────────────────

    def _exchange_loop(self, wake_r: int) -> None:
        """The reading thread: the arm's own feedback in, and, when something is owed, the stop out. It waits on the feedback socket and on a pipe that a stop writes to, so a stop is on the wire at once."""
        assert self._feedback is not None
        feedback = self._feedback
        reopened = 0.0
        open_error: str | None = None
        silent_since = time.monotonic()
        while not self._closing.is_set():
            handle = feedback._sock          # noqa: SLF001 - the one thing the loop waits on besides its wake-up
            try:
                if handle is None:
                    raise link.LinkError(f"{feedback.target} is not open.")
                select.select([handle, wake_r], [], [], min(1.0 / self._rate_hz, 0.05))
                if self._closing.is_set():
                    break
                self._drain_wake(wake_r)
                self._deliver_if_owed()
                packet = feedback.read_packet(0.0)
                if packet is None:
                    if time.monotonic() - silent_since > max(self._timeout * 3, 0.25):
                        raise link.LinkError(f"{feedback.target} has sent no packet in {time.monotonic() - silent_since:.2f} s.")
                    continue
                reading = frames.decode_feedback(packet)
                self._observe(reading)
            except (link.LinkError, link.NotTcp, frames.ProtocolError, BridgeRefusal, ValueError) as error:
                reason = str(error)
                if not feedback.is_open:
                    if time.monotonic() - reopened >= _REOPEN_S:
                        # A controller that went away: the feedback is opened again, at most twice a second, and what it says is what is reported while it stays shut.
                        reopened = time.monotonic()
                        try:
                            feedback.open(timeout=min(self._timeout, 0.5))
                            open_error = None
                            silent_since = time.monotonic()
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
                silent_since = time.monotonic()
                with self._lock:
                    self._latest, self._stamp_ns = reading, rhal.acquisition_ns()
                    self._packets += 1
                    self._failures, self._dead = 0, None
                    self._changed.notify_all()
            if self._dashboard is not None and not self._dashboard.is_open and not self._closing.is_set():
                self._reopen_dashboard()

    def _reopen_dashboard(self) -> None:
        dashboard = self._dashboard
        if dashboard is None or dashboard.is_open:
            return
        now = time.monotonic()
        if now - self._dashboard_reopened < _REOPEN_S:
            return
        self._dashboard_reopened = now
        with contextlib.suppress(link.LinkError):
            dashboard.open(timeout=min(self._timeout, 0.5))

    def _observe(self, reading: frames.Feedback) -> None:
        """What a good packet shows is owed: the alarm list while the arm is in its alarm state, and a stop that has not been written."""
        if reading.robot_mode == frames.MODE_ERROR or reading.error_status:
            with self._lock:
                stale = time.monotonic() - self._alarms_read_at > _ALARM_POLL_S or self._alarms_for_mode != reading.robot_mode
            if stale and (self._tcp is not False):
                self._read_alarms(reading.robot_mode)
        else:
            with self._lock:
                # An arm that is well holds no alarm, and the next alarm it holds is read at once and not when the last read was half a second ago.
                self._alarms, self._alarms_read_at, self._alarms_for_mode = [], 0.0, 0
        self._deliver_if_owed()

    def _read_alarms(self, mode: int) -> None:
        try:
            reply = self._command(frames.get_error_id())
        except link.NotTcp:
            with self._lock:
                self._tcp = False
            return
        except link.LinkError:
            return
        if reply.error != 0:
            return
        try:
            ids = frames.error_ids(reply.payload)
        except frames.ProtocolError:
            return
        with self._lock:
            self._alarms = ids
            self._alarms_read_at = time.monotonic()
            self._alarms_for_mode = mode

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
        """The controller's stop, written by the reading thread: `Stop()`, or `EmergencyStop(1)` for `emergency`."""
        with self._lock:
            calls = self._stop_calls
        text = frames.emergency_stop(True) if self._stop_mode == "emergency" else frames.stop()
        reply = self._command(text)
        if reply.error != 0:
            raise BridgeRefusal(f"the controller answered {text} with error {reply.error} ({frames.describe_error(reply.error)}).", rhal.SKR_EIO)
        with self._lock:
            if calls != self._stop_calls:
                return
            self._stop_delivered = True
            self._goal = None
            self._changed.notify_all()

    def _release_software_stop(self) -> None:
        """`EmergencyStop(0)` and then `ClearError()`: the arm is left disabled, for a person to enable."""
        for text in (frames.emergency_stop(False), frames.clear_error()):
            reply = self._command(text, self._timeout * 3)
            if reply.error != 0:
                raise BridgeRefusal(f"the controller answered {text} with error {reply.error} ({frames.describe_error(reply.error)}).", rhal.SKR_EIO)

    def _wait_for_packet(self, predicate, timeout: float, *, honour_stop: bool = True) -> frames.Feedback:
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
        for connection in (self._dashboard, self._feedback):
            if connection is not None:
                connection.close()
        self._dashboard = self._feedback = None
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
            "schema_version": 1, "robot_slug": "dobot-" + self._model.lower().replace(" ", "-").replace("(", "").replace(")", ""), "digest": None, "verified": False, "simulator": self._simulated, "base": "fixed", "dof": 6,
            "joints": joints, "end_effectors": [effector], "sensors": [],
            "control": {"modes": ["position"], "rate_hz_max": self._rate_hz, "rate_hz_min": 1.0},
            # Null: the ranges are the operator's, the speed is the controller's own limit and ServoJ's `t`, and none of it is a risk assessment (§5.2).
            "envelope": None, "capabilities": ["safety.estop"], "motions": [_descriptor(name) for name in _MOTIONS], "extensions": [TOKEN], "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))

    # ── the extension: motions ───────────────────────────────────────────────

    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        if token != TOKEN:
            return rhal.SKR_ENOSYS, f"unknown token {token!r}. This driver serves {TOKEN}.".encode()
        if not self._opened or self._dashboard is None:
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
        except link.NotTcp as error:
            return rhal.SKR_EPERM, f"the controller's control mode is not TCP ({error}); `request_control` asks for it.".encode()
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

    def _present(self) -> frames.Feedback:
        with self._lock:
            reading = self._latest
        if reading is None:
            raise BridgeRefusal("the arm has not been read yet.", rhal.SKR_ETIMEDOUT)
        return reading

    def _do(self, text: str) -> frames.Reply:
        """A command a motion makes, whose refusal is the motion's: an ErrorID other than 0 is said in the document's words."""
        reply = self._command(text, _COMMAND_BUDGET_S)
        if reply.error != 0:
            raise BridgeRefusal(f"the controller answered {text.split('(')[0]} with error {reply.error} ({frames.describe_error(reply.error)}).", _code_of(reply.error))
        return reply

    def _motion_status(self, args: dict) -> dict:
        self._no_arguments("status", args)
        reading = self._present()
        with self._lock:
            packets, dead, delivered, goal, alarms, tcp = self._packets, self._dead, self._stop_delivered, self._goal, list(self._alarms), self._tcp
        verdict = _classify(reading, alarms)
        return {"model": self._model, "link": self._dashboard.target if self._dashboard else None, "feedback": self._feedback.target if self._feedback else None, "simulator": self._simulated,
                "robot_mode": reading.robot_mode, "robot_mode_name": frames.MODES.get(reading.robot_mode, "not in the document"), "tcp_control": tcp, "enabled": bool(reading.enable_status),
                "alarm": reading.error_status != 0 or reading.robot_mode == frames.MODE_ERROR, "alarms": alarms, "fault": None if verdict is None else {"class": verdict[0], "code": verdict[1]}, "collision": bool(reading.collision_state),
                "running": reading.running_status != 0, "position_rad": [round(q * _DEG, 6) for q in reading.q_actual], "position_deg": [round(q, 4) for q in reading.q_actual],
                "goal_deg": goal, "temperatures": [round(t, 1) for t in reading.motor_temperatures], "speed_factor": reading.speed_scaling, "load_kg": reading.load_kg, "payload_kg_configured": self._payload_kg,
                "robot_type": reading.robot_type, "packets": packets, "link_dead": dead, "stop_latched": self._estop, "stop_delivered": delivered, "stop_mode": self._stop_mode}

    def _motion_request_control(self, args: dict) -> dict:
        self._no_arguments("request_control", args)
        self._do(frames.request_control())
        with self._lock:
            self._tcp = True
        return {"result": "requested", "note": "the controller now takes TCP commands from this line"}

    def _motion_power_on(self, args: dict) -> dict:
        self._no_arguments("power_on", args)
        self._do(frames.power_on())
        return {"result": "requested", "note": "powering on takes about ten seconds; the arm is enabled by the `enable` motion once the mode is DISABLED"}

    def _motion_enable(self, args: dict) -> dict:
        self._no_arguments("enable", args)
        reading = self._present()
        if reading.robot_mode == frames.MODE_ENABLE or reading.enable_status:
            return {"result": "already_enabled"}
        self._do(frames.enable_robot(self._payload_kg))
        try:
            after = self._wait_for_packet(lambda r: r.robot_mode in _RUNNING_MODES or r.robot_mode == frames.MODE_ERROR, self._timeout * 6 + 2.0)
        except BridgeRefusal as error:
            raise BridgeRefusal(f"EnableRobot was accepted and no reading has shown the arm enabled ({error}).", rhal.SKR_EIO) from None
        if after.robot_mode == frames.MODE_ERROR:
            raise BridgeRefusal("the arm went into its alarm state while it was being enabled: read the alarm with `status`.", rhal.SKR_EIO)
        with self._lock:
            self._goal = None
        return {"result": "enabled", "payload_kg": self._payload_kg}

    def _motion_disable(self, args: dict) -> dict:
        self._no_arguments("disable", args)
        self._do(frames.disable_robot())
        with self._lock:
            self._goal = None
        return {"result": "requested"}

    def _motion_clear_errors(self, args: dict) -> dict:
        self._no_arguments("clear_errors", args)
        self._present()
        self._do(frames.clear_error())

        def clear(packet: frames.Feedback) -> bool:
            return packet.robot_mode not in (frames.MODE_ERROR, frames.MODE_COLLISION) and not packet.error_status and not packet.collision_state

        # The first packet after the reply may have been made before the controller acted on the command, so "the next reading" is not an answer; a reading that shows the alarm gone is, and an alarm whose cause is
        # still there is one that no reading shows gone in the time given.
        try:
            after = self._wait_for_packet(clear, self._timeout * 6 + 1.0)
        except BridgeRefusal as error:
            with self._lock:
                left, dead = self._latest, self._dead
            if left is None or dead is not None:
                raise BridgeRefusal(f"ClearError was accepted and no reading has shown what it did ({error}).", rhal.SKR_EIO) from None
            return {"result": "not_cleared", "robot_mode": left.robot_mode, "note": "the alarm is still there: its cause has to be removed; read it with `status`"}
        return {"result": "cleared", "robot_mode": after.robot_mode, "note": "the arm is disabled after an alarm: the `enable` motion enables it"}

    def _motion_move(self, args: dict) -> dict:
        timeout = self._number(args, "timeout", minimum=0.5, maximum=300.0)
        velocity = self._number(args, "v", minimum=1, maximum=100)
        if velocity is not None and velocity != int(velocity):
            raise BridgeRefusal(f"`v` of {velocity:g} is not a whole percentage from 1 to 100: the controller's MovJ takes a whole number, and a fraction would be cut to one without saying so.", rhal.SKR_EINVAL)
        targets: dict[int, float] = {}
        for name in list(args):
            if name not in JOINTS:
                raise BridgeRefusal(f"`{name}` is not a joint ({', '.join(JOINTS)}), `v` or `timeout`.", rhal.SKR_EINVAL)
            joint = JOINTS.index(name)
            low, high = self._limits[joint]
            value = self._number(args, name, minimum=low, maximum=high)
            assert value is not None
            targets[joint] = value
        if not targets:
            raise BridgeRefusal(f"`move` needs at least one joint, as `<joint>=<radians>`: {', '.join(JOINTS)}.", rhal.SKR_EINVAL)
        reading = self._gate()
        goal = [float(q) for q in reading.q_actual]
        for joint, value in targets.items():
            goal[joint] = value / _DEG
        wait = timeout if timeout is not None else max(abs(goal[j] - reading.q_actual[j]) for j in targets) / 10.0 + 5.0
        reply = self._do(frames.mov_j(goal, v=None if velocity is None else int(velocity)))
        with self._lock:
            self._goal = list(goal)
        started = [False]

        def done(packet: frames.Feedback) -> bool:
            if packet.robot_mode in (frames.MODE_ERROR, frames.MODE_COLLISION):
                return True
            started[0] = started[0] or packet.running_status != 0 or packet.robot_mode in (frames.MODE_RUNNING, frames.MODE_SINGLE_MOVE) or max(abs(packet.q_actual[j] - reading.q_actual[j]) for j in targets) > _ARRIVAL_TOLERANCE_DEG
            if packet.running_status != 0 or packet.robot_mode in (frames.MODE_RUNNING, frames.MODE_SINGLE_MOVE):
                return False
            return started[0] or max(abs(packet.q_actual[j] - goal[j]) for j in targets) <= _ARRIVAL_TOLERANCE_DEG

        try:
            after = self._wait_for_packet(done, wait)
        except BridgeRefusal as error:
            if error.code == rhal.SKR_EIO and "no reading showed" in str(error) and not started[0]:
                raise BridgeRefusal(f"the arm never started moving after MovJ (it was not running, and the joints did not change, in {wait:.1f} s): {error}", rhal.SKR_EIO) from None
            raise
        if after.robot_mode in (frames.MODE_ERROR, frames.MODE_COLLISION):
            raise BridgeRefusal(f"the arm went into {'an alarm' if after.robot_mode == frames.MODE_ERROR else 'a collision'} during the move: read it with `status`.", rhal.SKR_EIO)
        arrived = max(abs(after.q_actual[j] - goal[j]) for j in targets) <= _ARRIVAL_TOLERANCE_DEG
        return {"result": "arrived" if arrived else "stopped_short", "result_id": reply.payload.strip(), "position_deg": [round(q, 4) for q in after.q_actual], "goal_deg": [round(g, 4) for g in goal]}


def _code_of(error_id: int) -> int:
    """The RHAL code for a controller's ErrorID: a state that refuses commands is EPERM, a parameter that is wrong is EINVAL, and a command that was received and failed is EIO."""
    if error_id in (-2, -3, -4, -5, -7, -8, -9):
        return rhal.SKR_EPERM
    if error_id <= -20000 or error_id == -6:
        return rhal.SKR_EINVAL
    return rhal.SKR_EIO


def _classify(reading: frames.Feedback, alarms: list[int]) -> tuple[str, int] | None:
    """('estop' | 'fault', the code) for an arm that is not taking commands because of an alarm, a collision, an emergency stop or a state the document does not list, and None for one that is.

    The code is the first alarm the controller lists. Where none has been read it is the mode that says why, which cannot be taken for an alarm because the maker's alarm table starts at 16: 9 for an alarm, 11 for a
    collision. A mode the document does not list is a state of the arm nobody here has read about, so it is a fault (an unknown state is not a healthy one), and its code is that mode in the bits above an alarm's
    (`UNLISTED_MODE_BASE | mode`), where it cannot be taken for one either."""
    if reading.robot_mode not in frames.MODES:
        return "fault", UNLISTED_MODE_BASE | reading.robot_mode
    alarm = reading.robot_mode == frames.MODE_ERROR or bool(reading.error_status)
    collision = reading.robot_mode == frames.MODE_COLLISION or bool(reading.collision_state)
    if alarm or collision:
        code = alarms[0] if alarms else (frames.MODE_ERROR if alarm else frames.MODE_COLLISION)
        if any(alarm_id in ESTOP_ALARMS for alarm_id in alarms):
            return "estop", code
        return "fault", code
    return None


def _descriptor(name: str) -> dict:
    if name == "status":
        return {"name": "status", "parameters": []}
    if name == "move":
        return {"name": "move", "parameters": [{"name": joint, "unit": "rad", "required": False} for joint in JOINTS] + [{"name": "v", "unit": "%", "required": False}, {"name": "timeout", "unit": "s", "required": False}]}
    return {"name": name, "parameters": []}


def _bounded(settings: dict, key: str, default: float, low: float, high: float) -> float:
    value = settings.get(key)
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not low <= value <= high:
        raise BridgeRefusal(f"`{key}` of {value!r} is not a number from {low:g} to {high:g}.")
    return float(value)


def _say(message: str) -> None:
    print(f"skynet-dobot: {message}", file=sys.stderr, flush=True)
