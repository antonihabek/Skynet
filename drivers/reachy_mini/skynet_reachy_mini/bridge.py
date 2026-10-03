"""The RHAL driver: a Reachy Mini, over its daemon's own REST interface.

§4's five mandatory entry points over :mod:`skynet_reachy_mini.frames`. It links nothing of the maker's: Pollen Robotics publish the daemon (`reachy_mini`, Apache-2.0) and its routes and models were read
out of its own source; `layout_check.py` opens it and requires each to still be there. The daemon is the maker's own software, with the maker's own simulator behind it (`--sim`, MuJoCo), so what this
driver is held to is rung 2 (the maker's simulator) and not a robot.

# What the daemon says, and what only the operator can

The daemon reports whether it is a simulation (`simulation_enabled` in its status, and `mockup_sim_enabled`), its state and the error it holds, and where the head, the body and the antennas are. It says
**nothing about a joint's travel except the ranges the maker's documentation gives** (head pitch and roll ±40°, head yaw ±180°, body yaw ±160°, head and body yaw within 65° of each other), and nothing about the head's
translation, the antennas' range or how fast any of it may move. So:

* `limits`: the head's x, y and z and both antennas' ranges are **required**; the documented four are the defaults and a configuration may narrow them and never widen them;
* `speeds`: every joint's ceiling, **required**: nothing in the daemon says how fast a head may be driven, and a step the ceiling does not allow in one command period is refused, never clipped;
* `stop_mode`: **no default**. What stopping a Reachy Mini means is a person's decision: `hold` stops the stream and holds the head where it is, stiff; `limp` takes the motors' torque away, which on a head that is
  held up by them is a fall. The driver will not choose between them for somebody who cannot be asked;
* `simulator` is the daemon's word, and a configuration can only make it more cautious (`{"simulator": true}`); nothing forces it false.

# What it refuses to bring up

A daemon that is not running or holds an error, motors that are not enabled (`disabled`, or `gravity_compensation`, where somebody may have a hand on the head: this driver changes neither), and a move that
somebody else started (the daemon ignores `set_target` while one runs, so two masters is one that is ignored without a word, and a move it did not start is not one it may stop at open).

# What a command does

`write_command` sends one `POST /api/move/set_target` with the whole body: the head pose, the antennas and the body yaw. A joint the command masks holds the setpoint this driver last issued. The daemon
answers `{"status": "ignored"}` while a move runs, which is a command that did not happen and is told as one. Every setpoint is checked first: finite, inside the joint's range, inside the head-and-body yaw
difference, and no further from the last setpoint than the joint's speed allows in one period.

# The stop

`emergency_stop` latches and returns, and a thread of its own tells the daemon: **cancel every move that is running** (`GET /api/move/running`, `POST /api/move/stop` for each uuid) and then `hold` (a target at the
present pose) or `limp` (`set_mode disabled`). A stop that waited for HTTP would be a stop that waited. It is a **software stop**: the robot's own safety is the motors' firmware and the person at the desk.
"""

from __future__ import annotations

import http.client
import json
import math
import sys
import threading
import time

import skynet_rhal as rhal

from . import frames

__all__ = ["BridgeRefusal", "ReachyMiniBridge"]

_DEFAULT_PORT = 8000
_HTTP_TIMEOUT_S = 0.5
_OPEN_TIMEOUT_S = 3.0
_STALE_S = 0.5
_RATE_HZ = 20.0
#: How often the state is asked for and how often the daemon's own status is: the first is the control loop's, the second is what a person would notice.
_STATE_POLL_S = 0.04
_STATUS_POLL_S = 0.5
_STOP_TIMEOUT_S = 0.25
#: How long a command may wait for the daemon: it is the control loop's, and the loop waits for it (a quarter of a second is the contract's bound), so a command is not given the longer wait a read of the daemon's status has.
_WRITE_TIMEOUT_S = 0.2
#: How many times a stop is told to the daemon, and how long between: eight attempts over about four seconds.
_STOP_ATTEMPTS = 8
_STOP_RETRY_S = 0.1
_KNOWN_KEYS = {"host", "port", "simulator", "stop_mode", "limits", "speeds", "roles", "rate_hz", "http_timeout_ms", "stale_ms", "open_timeout_ms"}
_NAMES = tuple(name for name, _ in frames.JOINTS)


class BridgeRefusal(Exception):
    """A reason this body did not come up, in the terms an integrator acts on."""


class Unreachable(BridgeRefusal):
    """The daemon did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""


class _Daemon:
    """One HTTP connection to the daemon, used by one thread at a time, and made again when it fails."""

    def __init__(self, host: str, port: int, timeout: float) -> None:
        self._host, self._port, self._timeout = host, port, timeout
        self._connection: http.client.HTTPConnection | None = None
        self.lock = threading.Lock()
        #: How many times a connection the daemon had dropped was made again: a kept-alive connection that was reset, a daemon that restarted, a switch that cut it.
        self.reconnects = 0

    def call(self, method: str, path: str, body: object = None, timeout: float | None = None) -> object:
        data = None if body is None else json.dumps(body).encode()
        headers = {"Content-Type": "application/json"} if data is not None else {}
        for attempt in (0, 1):
            try:
                if self._connection is None:
                    self._connection = http.client.HTTPConnection(self._host, self._port, timeout=timeout or self._timeout)
                self._connection.timeout = timeout or self._timeout
                if self._connection.sock is not None:
                    self._connection.sock.settimeout(timeout or self._timeout)
                self._connection.request(method, path, body=data, headers=headers)
                response = self._connection.getresponse()
                payload = response.read()
            except (OSError, http.client.HTTPException) as error:
                self.close()
                # A kept-alive connection the daemon closed fails on first use; one new connection is the retry, and a second failure is the daemon's.
                if attempt == 0 and isinstance(error, (http.client.RemoteDisconnected, BrokenPipeError, ConnectionResetError)):
                    self.reconnects += 1
                    continue
                raise Unreachable(f"the daemon ({self._host}:{self._port}) did not answer {method} {path}: {error}") from error
            if response.status >= 400:
                raise BridgeRefusal(f"the daemon answered {method} {path} with {response.status}: {payload[:200].decode('utf-8', 'replace')}")
            try:
                return json.loads(payload) if payload else None
            except json.JSONDecodeError as error:
                raise BridgeRefusal(f"the daemon's answer to {method} {path} is not JSON: {error}") from error
        raise Unreachable("unreachable")  # pragma: no cover

    def close(self) -> None:
        if self._connection is not None:
            try:
                self._connection.close()
            except OSError:
                pass
            self._connection = None


class ReachyMiniBridge:
    """A body made of a Reachy Mini's nine joints."""

    def __init__(self) -> None:
        self._opened = False
        self._estop = False
        self._descriptor: str | None = None
        self._seq = 0
        self._simulated = True
        self._limits: dict[str, tuple[float, float]] = {}
        self._speeds: dict[str, float] = {}
        self._roles: dict[str, str | None] = {}
        self._period_s = 1.0 / _RATE_HZ
        self._rate_hz = _RATE_HZ
        self._stale_s = _STALE_S
        self._stop_mode = "hold"
        self._host = ""
        self._port = _DEFAULT_PORT
        self._timeout = _HTTP_TIMEOUT_S
        self._daemon: _Daemon | None = None
        self._stop_daemon: _Daemon | None = None
        self._poller: threading.Thread | None = None
        self._closing = threading.Event()
        self._lock = threading.Lock()
        self._latest: frames.State | None = None
        self._stamp_ns = 0
        self._dead: str | None = None
        self._status: dict = {}
        self._held: list[float] = []
        self._noted: set[str] = set()
        self._stop_threads: list[threading.Thread] = []
        self._blip_until = 0.0

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
        # A body, and the stop latched on it, belong to one session (DRY-COVERAGE-PLAN.md F73, F76).
        self._teardown()
        self._estop = False
        self._descriptor = None
        try:
            self._open(settings)
        except Unreachable as refusal:
            self._teardown()
            _say(str(refusal))
            return rhal.SKR_EIO
        except BridgeRefusal as refusal:
            self._teardown()
            _say(str(refusal))
            return rhal.SKR_ENODEV
        except OSError as error:
            self._teardown()
            _say(f"the daemon could not be reached: {error}")
            return rhal.SKR_EIO
        self._opened = True
        return rhal.SKR_OK

    def _open(self, settings: dict) -> None:
        unknown = sorted(set(settings) - _KNOWN_KEYS)
        if unknown:
            raise BridgeRefusal(f"the configuration has {', '.join(map(repr, unknown))}, which this driver does not read. The keys it reads are {', '.join(sorted(_KNOWN_KEYS))}.")
        host = settings.get("host")
        if not isinstance(host, str) or not host:
            raise BridgeRefusal("no `host` in the configuration. This driver does not scan for a daemon, and one that guessed an address would command whichever robot answered first.")
        port = settings.get("port", _DEFAULT_PORT)
        if isinstance(port, bool) or not isinstance(port, int) or not 0 < port < 65536:
            raise BridgeRefusal(f"`port` of {port!r} is not a port.")
        self._host, self._port = host, port
        self._stop_mode = self._read_stop_mode(settings.get("stop_mode"))
        self._load_limits(settings.get("limits"))
        self._load_speeds(settings.get("speeds"))
        roles = settings.get("roles") or {}
        if not isinstance(roles, dict) or set(roles) - set(_NAMES):
            raise BridgeRefusal(f"`roles` is a mapping from a joint's name to its role, and the joints are {', '.join(_NAMES)}.")
        self._roles = {name: roles.get(name) for name in _NAMES}
        self._rate_hz = _bounded(settings, "rate_hz", _RATE_HZ, 0.5, 100.0)
        self._period_s = 1.0 / self._rate_hz
        self._timeout = _bounded(settings, "http_timeout_ms", _HTTP_TIMEOUT_S * 1000, 20.0, 30000.0) / 1000.0
        self._stale_s = _bounded(settings, "stale_ms", _STALE_S * 1000, 20.0, 30000.0) / 1000.0
        open_s = _bounded(settings, "open_timeout_ms", _OPEN_TIMEOUT_S * 1000, 100.0, 60000.0) / 1000.0
        claim = settings.get("simulator")
        if claim is not None and not isinstance(claim, bool):
            raise BridgeRefusal(f"`simulator` of {claim!r} is not true or false.")

        self._daemon = _Daemon(host, port, min(self._timeout, open_s))
        status = self._daemon.call("GET", frames.Route.DAEMON_STATUS, timeout=open_s)
        if not isinstance(status, dict):
            raise BridgeRefusal("the daemon's status is not an object: this is not a Reachy Mini daemon, or one this driver does not know.")
        self._check_status(status)
        simulated = status.get("simulation_enabled") is True or status.get("mockup_sim_enabled") is True
        self._simulated = bool(claim) or simulated
        if status.get("simulation_enabled") is None and claim is None:
            _say("the daemon does not say whether it is a simulation (`simulation_enabled` is null), and the configuration does not either, so the descriptor says hardware.")
        running = self._daemon.call("GET", frames.Route.RUNNING, timeout=open_s)
        if running:
            raise BridgeRefusal(
                f"the daemon is running {len(running) if isinstance(running, list) else 'a'} move(s) this session did not start. It ignores `set_target` while one runs, so two masters is one that is ignored without a word: "
                "let it finish, or stop it, deliberately.")
        state = frames.parse_state(self._daemon.call("GET", frames.Route.STATE_FULL, timeout=open_s))
        self._check_mode(state.control_mode)
        self._latest, self._stamp_ns = state, rhal.acquisition_ns()
        self._status = status
        self._held = list(state.positions)
        self._stop_daemon = _Daemon(host, port, _STOP_TIMEOUT_S)
        self._closing.clear()
        self._dead = None
        self._poller = threading.Thread(target=self._poll, daemon=True, name="reachy-poll")
        self._poller.start()
        self._descriptor = self._build_descriptor()

    def _read_stop_mode(self, value: object) -> str:
        if value not in ("hold", "limp"):
            raise BridgeRefusal(
                "`stop_mode` is not in the configuration, or is neither `hold` nor `limp`. What stopping a Reachy Mini means is a person's decision and this driver will not make it: `hold` stops the stream and "
                "holds the head where it is, stiff; `limp` takes the motors' torque away, which on a head the motors hold up is a fall.")
        return str(value)

    def _load_limits(self, given: object) -> None:
        if given is None:
            given = {}
        if not isinstance(given, dict) or set(given) - set(_NAMES):
            raise BridgeRefusal(f"`limits` is a mapping from a joint's name to [min, max], and the joints are {', '.join(_NAMES)}.")
        limits: dict[str, tuple[float, float]] = {}
        for name in _NAMES:
            documented = frames.DOCUMENTED_LIMITS.get(name)
            entry = given.get(name)
            if entry is None:
                if documented is None:
                    raise BridgeRefusal(
                        f"no range for {name!r} in `limits`. The maker's documentation gives none for the head's translation or the antennas, and a driver that made one up would bound the robot with a number nobody measured (§5.2).")
                limits[name] = documented
                continue
            if not (isinstance(entry, (list, tuple)) and len(entry) == 2 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) for v in entry)):
                raise BridgeRefusal(f"the range of {name!r} is not [min, max] as two numbers.")
            low, high = float(entry[0]), float(entry[1])
            if not low < high:
                raise BridgeRefusal(f"the range of {name!r}, [{low}, {high}], is not a range.")
            if documented is not None and (low < documented[0] - 1e-9 or high > documented[1] + 1e-9):
                raise BridgeRefusal(f"the range of {name!r}, [{low:.4f}, {high:.4f}] rad, is wider than the maker's documented [{documented[0]:.4f}, {documented[1]:.4f}]: a configuration may narrow a documented range and never widen it.")
            limits[name] = (low, high)
        self._limits = limits

    def _load_speeds(self, given: object) -> None:
        if not isinstance(given, dict) or not given:
            raise BridgeRefusal("no `speeds` in the configuration. Nothing in the daemon says how fast a head may be driven: give each joint's ceiling in m/s or rad/s, as a mapping from its name.")
        if set(given) - set(_NAMES):
            raise BridgeRefusal(f"`speeds` names {sorted(set(given) - set(_NAMES))}, which are not joints. The joints are {', '.join(_NAMES)}.")
        speeds: dict[str, float] = {}
        for name in _NAMES:
            value = given.get(name)
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or value <= 0:
                raise BridgeRefusal(f"the speed ceiling of {name!r} is missing or is not a number above zero.")
            speeds[name] = float(value)
        self._speeds = speeds

    def _check_status(self, status: dict) -> None:
        state = status.get("state")
        if state != frames.DaemonState.RUNNING:
            raise BridgeRefusal(f"the daemon is {state!r}, not running. This driver does not start or restart it.")
        error = status.get("error") or (status.get("backend_status") or {}).get("error")
        if error:
            raise BridgeRefusal(f"the daemon holds an error: {error}. This body does not come up over one: read why, and clear it at the daemon deliberately.")

    def _check_mode(self, mode: str | None) -> None:
        if mode is not None and mode != frames.MotorMode.ENABLED:
            raise BridgeRefusal(
                f"the motors are in {mode!r} mode. This driver does not enable them: `disabled` is a head with no torque on it, and `gravity_compensation` is one somebody may have a hand on. "
                "Set them to `enabled` where the robot is, deliberately.")

    def close(self) -> int:
        if self._opened and not self._estop:
            try:
                # A session ends with the robot holding where it is, not still going to the last target it was given. Never `limp`, whatever the stop mode is: letting go of a head is the stop's to do, and not a close's.
                self._tell_stop(limp=False, cancel_moves=False)
            except (BridgeRefusal, OSError):
                pass  # closing a body whose daemon has gone away is still closing it
        self._teardown()
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        with self._lock:
            state, stamp, dead, status = self._latest, self._stamp_ns, self._dead, dict(self._status)
        self._seq += 1
        out.seq = self._seq
        out.flags = rhal.HOMING_NOT_REQUIRED
        out.fault_code = 0
        if state is None:
            return rhal.SKR_ETIMEDOUT
        out.stamp_ns = stamp
        out.position_rad = list(state.positions)
        # NaN, not zero, for what the daemon does not measure (§24): it reports present positions only.
        out.velocity_rad_s = [math.nan] * len(_NAMES)
        out.effort_nm = [math.nan] * len(_NAMES)
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP
        error = status.get("error") or (status.get("backend_status") or {}).get("error")
        if error or status.get("state") == frames.DaemonState.ERROR:
            # FAULT for the cause, and DEGRADED because it is the bit §27's `rhal` condition reads. The daemon's error is free text with no code, so the fault word says there is one.
            out.flags |= rhal.FLAG_FAULT | rhal.FLAG_DEGRADED
            out.fault_code = 1
        elif status.get("state") not in (None, frames.DaemonState.RUNNING) or state.control_mode not in (None, frames.MotorMode.ENABLED):
            out.flags |= rhal.FLAG_DEGRADED
        age = (rhal.acquisition_ns() - stamp) / 1e9
        if time.monotonic() < self._blip_until:
            out.flags |= rhal.FLAG_DEGRADED
        if dead or age > self._stale_s:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened or self._daemon is None:
            return rhal.SKR_EPERM
        if cmd.joint_count != len(_NAMES):
            return rhal.SKR_EINVAL
        if cmd.mode != rhal.MODE_POSITION:
            # The daemon takes a target. Velocity and torque are not routes of its REST interface, and shipping them as positions is how a torque command becomes a position one.
            return rhal.SKR_EINVAL
        if self._estop:
            return rhal.SKR_EPERM
        with self._lock:
            state, status = self._latest, dict(self._status)
        if state is not None and state.control_mode not in (None, frames.MotorMode.ENABLED):
            return rhal.SKR_EPERM
        if status.get("state") not in (None, frames.DaemonState.RUNNING) or status.get("error") or (status.get("backend_status") or {}).get("error"):
            return rhal.SKR_EPERM

        # Every setpoint is checked before any request goes out, and nothing is stored until all of them pass (F73's shape).
        targets = list(self._held)
        for index, name in enumerate(_NAMES):
            applied = cmd.mask[index] if index < len(cmd.mask) else 1
            if not applied:
                continue
            value = cmd.values[index]
            low, high = self._limits[name]
            if not math.isfinite(value) or not low <= value <= high:
                return rhal.SKR_EINVAL
            if abs(value - self._held[index]) > self._speeds[name] * self._period_s + 1e-12:
                return rhal.SKR_EINVAL
            targets[index] = value
        if abs(targets[_NAMES.index("head_yaw")] - targets[_NAMES.index("body_yaw")]) > frames.YAW_DELTA_MAX_RAD + 1e-9:
            return rhal.SKR_EINVAL
        try:
            with self._daemon.lock:
                reply = self._daemon.call("POST", frames.Route.SET_TARGET, frames.target_body(targets), timeout=min(self._timeout, _WRITE_TIMEOUT_S))
        except frames.ProtocolError as error:
            _say(str(error))
            return rhal.SKR_EINVAL
        except Unreachable as error:
            _say(str(error))
            return rhal.SKR_EIO
        except BridgeRefusal as error:
            _say(str(error))
            return rhal.SKR_EIO
        if not isinstance(reply, dict) or reply.get("status") != "ok":
            if isinstance(reply, dict) and reply.get("status") == "ignored":
                _say(f"the daemon ignored the target ({reply.get('reason')}): a move that this session did not start is running, and `set_target` is ignored while it does.")
                return rhal.SKR_EPERM
            _say(f"the daemon did not take the target: {reply!r}")
            return rhal.SKR_EIO
        self._held = targets
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        self._estop = True
        thread = threading.Thread(target=self._stop_quietly, daemon=True, name="reachy-stop")
        self._stop_threads = [t for t in self._stop_threads if t.is_alive()] + [thread]
        thread.start()
        return rhal.SKR_OK

    def _stop_quietly(self) -> None:
        # The latch is already holding, so this is the daemon being told. A daemon that is slow or has dropped a request is asked again for a few seconds, and then left to `read_state` to report on:
        # a head that goes on to its last target because the one request that said otherwise was lost is not a stop.
        for attempt in range(_STOP_ATTEMPTS):
            try:
                self._tell_stop(limp=self._stop_mode == "limp", cancel_moves=True)
                return
            except (BridgeRefusal, OSError):
                if self._closing.is_set() and self._stop_daemon is None:
                    return
                time.sleep(_STOP_RETRY_S)

    def _tell_stop(self, limp: bool, cancel_moves: bool) -> None:
        """Cancel every move that is running, then hold at the present pose or let go, on a connection of its own."""
        daemon = self._stop_daemon
        if daemon is None:
            return
        with daemon.lock:
            try:
                if cancel_moves:
                    running = daemon.call("GET", frames.Route.RUNNING, timeout=_STOP_TIMEOUT_S)
                    for move in running if isinstance(running, list) else []:
                        if isinstance(move, dict) and "uuid" in move:
                            daemon.call("POST", frames.Route.STOP_MOVE, {"uuid": move["uuid"]}, timeout=_STOP_TIMEOUT_S)
            finally:
                if limp:
                    daemon.call("POST", frames.Route.SET_MODE.format(mode=frames.MotorMode.DISABLED), timeout=_STOP_TIMEOUT_S)
                else:
                    with self._lock:
                        present = self._latest
                    if present is not None:
                        daemon.call("POST", frames.Route.SET_TARGET, frames.target_body(list(present.positions)), timeout=_STOP_TIMEOUT_S)

    def release_stop(self) -> int:
        """Clear the session latch and say whether the robot can be commanded again (§48.2)."""
        for thread in self._stop_threads:
            thread.join(timeout=1.0)
        self._estop = False
        if self._daemon is None or self._stop_daemon is None:
            return rhal.SKR_OK
        try:
            with self._stop_daemon.lock:
                if self._stop_mode == "limp":
                    self._stop_daemon.call("POST", frames.Route.SET_MODE.format(mode=frames.MotorMode.ENABLED), timeout=self._timeout)
                state = frames.parse_state(self._stop_daemon.call("GET", frames.Route.STATE_FULL, timeout=self._timeout))
                status = self._stop_daemon.call("GET", frames.Route.DAEMON_STATUS, timeout=self._timeout)
            if isinstance(status, dict):
                self._check_status(status)
            self._check_mode(state.control_mode)
        except (BridgeRefusal, frames.ProtocolError) as error:
            self._estop = True
            _say(f"the robot cannot be commanded again: {error}")
            return rhal.SKR_EIO
        self._held = list(state.positions)
        return rhal.SKR_OK

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── plumbing ─────────────────────────────────────────────────────────────

    def _poll(self) -> None:
        daemon = _Daemon(self._host, self._port, self._timeout)
        last_status = 0.0
        seen = 0
        while not self._closing.is_set():
            began = time.monotonic()
            try:
                state = frames.parse_state(daemon.call("GET", frames.Route.STATE_FULL))
                if daemon.reconnects != seen:
                    # A connection the daemon dropped is said, as DEGRADED for the staleness bound, though the next request went through: a peer that crashed and came back is a link that was down, and
                    # what it came back as (a restarted daemon, motors in another mode) is read from the state that follows.
                    seen = daemon.reconnects
                    self._blip_until = time.monotonic() + self._stale_s
                with self._lock:
                    self._latest, self._stamp_ns, self._dead = state, rhal.acquisition_ns(), None
                if began - last_status >= _STATUS_POLL_S:
                    last_status = began
                    status = daemon.call("GET", frames.Route.DAEMON_STATUS)
                    if isinstance(status, dict):
                        with self._lock:
                            self._status = status
            except (BridgeRefusal, frames.ProtocolError) as error:
                with self._lock:
                    self._dead = str(error)
            self._closing.wait(max(0.0, _STATE_POLL_S - (time.monotonic() - began)))
        daemon.close()

    def _teardown(self) -> None:
        self._closing.set()
        if self._poller is not None:
            self._poller.join(timeout=2.0)
        self._poller = None
        for daemon in (self._daemon, self._stop_daemon):
            if daemon is not None:
                daemon.close()
        self._daemon = self._stop_daemon = None

    def _build_descriptor(self) -> str:
        joints = []
        for name, kind in frames.JOINTS:
            low, high = self._limits[name]
            joints.append({
                "name": name, "role": self._roles.get(name), "type": kind,
                "limits": {"position_min_rad": low, "position_max_rad": high, "velocity_max_rad_s": self._speeds[name]},
                "control_modes": ["position"],
            })
        descriptor = {
            "schema_version": 1, "robot_slug": "reachy-mini", "digest": None, "verified": False, "simulator": self._simulated, "base": "fixed", "dof": len(joints), "joints": joints,
            "end_effectors": [], "sensors": [], "control": {"modes": ["position"], "rate_hz_max": self._rate_hz, "rate_hz_min": 0.5},
            # Null, not an empty object: nobody has declared a safety envelope for this body, and a joint range from a configuration is not a risk assessment. No torque rating: the daemon publishes none.
            "envelope": None, "capabilities": ["safety.estop"], "extensions": [], "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))


def _bounded(settings: dict, key: str, default: float, low: float, high: float) -> float:
    value = settings.get(key)
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not low <= value <= high:
        raise BridgeRefusal(f"`{key}` of {value!r} is not a number from {low:g} to {high:g}.")
    return float(value)


def _say(message: str) -> None:
    print(f"skynet-reachy-mini: {message}", file=sys.stderr, flush=True)
