"""The RHAL driver: a SCHUNK EGU, EGK or EZU gripper, over its module's HTTP interface (DRY-COVERAGE-PLAN.md item 12.E.9).

§4's five mandatory entry points over :mod:`skynet_schunk.frames`. It links nothing of the maker's: SCHUNK publish a client and a simulator (`schunk_mechatronic_gripper`, GPL-3.0), the wire was read out of both, and
`layout_check.py` runs the maker's own client and holds this driver's bytes to what it sends. The substrate the maker ships is its simulator, so what this driver is held to is rung 2 and not a gripper.

# The body

One prismatic joint, **`jaw`**: the module's own position, in metres (the module counts micrometres), from its minimum to its maximum, which **the module reports** (parameters `0x0600` and `0x0608`) and a
configuration may narrow and never widen. The speed ceiling is the operator's (`speed`: nothing in a position command says how fast), and the module's own minimum and maximum bound it. A gripper declares an
end effector of kind `gripper` whose opening is its travel.

# What it refuses to bring up

A module that is not an EtherNet/IP or EtherCAT one (a PROFINET module's frame is not this driver's to read: the maker's client takes the error byte from byte 12 for every fieldbus and its own PROFINET dump has it in
byte 15, and nothing here can say which is right), a module that says it is not ready, and a `speed`, `stop_mode` or `simulator` the configuration does not give. **A module that reports an error opens, with the stop
latched**: the maker's own simulator comes up in a fast stop (error byte 0xD9, status bit 7) the way a real module does until its controller acknowledges it, and an error is cleared by `release_stop`, which is the
maker's acknowledge and is verified to have cleared it. Nothing here acknowledges anything by itself.

# A command

A command is the maker's handshake (`Driver._send_cmd`): clear the control word, read the command-received toggle, write the command, wait for the toggle to flip. It is made under a lock of its own that the stop also
takes, and the stop latch is looked at **immediately before the command is written**, so a command can never be written after a stop. A command whose target is the one already issued is not sent again.

# The stop

`stop_mode` has **no default**, because what stopping a gripper means is a person's decision: `fast` writes the fast stop (control bit 0 low), which puts the module in error 0xD9 until it is acknowledged;
`controlled` writes the stop bit (1), which ends the motion with no error. Neither releases a workpiece. It is a **software stop**: the module's own fast-stop input is a circuit.
"""

from __future__ import annotations

import http.client
import json
import math
import sys
import threading
import time
from urllib.parse import urlencode

import skynet_rhal as rhal

from . import frames
from .frames import Bit, Status

__all__ = ["BridgeRefusal", "SchunkBridge", "TOKEN"]

TOKEN = "vendor.schunk.task"
JOINT = "jaw"

_DEFAULT_PORT = 80
_HTTP_TIMEOUT_S = 0.5
_OPEN_TIMEOUT_S = 3.0
_STALE_S = 0.5
_RATE_HZ = 5.0
#: The maker's client will not poll faster than this ("update_cycle must be at least 0.05 seconds").
_STATE_POLL_S = 0.05
#: How long a command's handshake may take in all: the control loop waits for it, and a quarter of a second is the contract's bound (`tools/faultproxy/chaos.py`). The budget is 0.2 s so that the bound holds with the
#: scheduler's jitter and a request that ends at the deadline (F256).
_COMMAND_BUDGET_S = 0.2
#: A request is not started with less than this left: a socket timeout of nothing is a non-blocking socket, and a request given a few milliseconds is one that fails for being given them.
_MIN_REQUEST_S = 0.01
_TOGGLE_WAIT_S = 1.0
_STOP_ATTEMPTS = 8
_STOP_RETRY_S = 0.1
_KNOWN_KEYS = {"host", "port", "simulator", "stop_mode", "speed", "limits", "roles", "rate_hz", "http_timeout_ms", "stale_ms", "open_timeout_ms"}
_MOTIONS = ("status", "param", "move", "grip", "release")


class BridgeRefusal(Exception):
    """A reason this body did not come up or a command did not happen, in the terms an integrator acts on."""

    def __init__(self, message: str, code: int = rhal.SKR_ENODEV) -> None:
        super().__init__(message)
        self.code = code


class Unreachable(BridgeRefusal):
    """The module did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""

    def __init__(self, message: str) -> None:
        super().__init__(message, rhal.SKR_EIO)


class _Module:
    """One HTTP connection to the module, used by one thread at a time, and made again when it fails."""

    def __init__(self, host: str, port: int, timeout: float) -> None:
        self._host, self._port, self._timeout = host, port, timeout
        self._connection: http.client.HTTPConnection | None = None
        self.lock = threading.Lock()
        self.reconnects = 0

    def _request(self, method: str, path: str, body: bytes | None, headers: dict, timeout: float | None) -> bytes:
        for attempt in (0, 1):
            try:
                if self._connection is None:
                    self._connection = http.client.HTTPConnection(self._host, self._port, timeout=timeout or self._timeout)
                self._connection.timeout = timeout or self._timeout
                if self._connection.sock is not None:
                    self._connection.sock.settimeout(timeout or self._timeout)
                self._connection.request(method, path, body=body, headers=headers)
                response = self._connection.getresponse()
                payload = response.read()
            except (OSError, http.client.HTTPException) as error:
                self.close()
                if attempt == 0 and isinstance(error, (http.client.RemoteDisconnected, BrokenPipeError, ConnectionResetError)):
                    self.reconnects += 1
                    continue
                raise Unreachable(f"the gripper's module ({self._host}:{self._port}) did not answer {method} {path}: {error}") from error
            if response.status >= 400:
                raise BridgeRefusal(f"the gripper's module answered {method} {path} with {response.status}: {payload[:200].decode('utf-8', 'replace')}", rhal.SKR_EIO)
            return payload
        raise Unreachable("unreachable")  # pragma: no cover

    def read(self, inst: str, timeout: float | None = None) -> str:
        """An instance's bytes as hex, or a refusal: the maker's `Driver._read_param_now`."""
        payload = self._request("GET", f"{frames.Route.DATA}?{urlencode({'inst': inst, 'count': '1'})}", None, {}, timeout)
        try:
            document = json.loads(payload)
        except json.JSONDecodeError as error:
            raise BridgeRefusal(f"the module's answer for {inst} is not JSON: {error}", rhal.SKR_EIO) from error
        if not isinstance(document, list) or not document or not isinstance(document[0], str):
            raise BridgeRefusal(f"the module has no instance {inst}: it answered {document!r}.", rhal.SKR_EIO)
        return document[0]

    def write(self, inst: str, value_hex: str, timeout: float | None = None) -> None:
        """Write an instance: the maker's `Driver._write_param_now` (a form post of `inst` and the hex value)."""
        body = urlencode({"inst": inst, "value": value_hex}).encode()
        payload = self._request("POST", frames.Route.UPDATE, body, {"Content-Type": "application/x-www-form-urlencoded"}, timeout)
        try:
            document = json.loads(payload)
        except json.JSONDecodeError as error:
            raise BridgeRefusal(f"the module's answer to a write of {inst} is not JSON: {error}", rhal.SKR_EIO) from error
        if not isinstance(document, dict) or document.get("result") != 0:
            raise BridgeRefusal(f"the module refused a write of {inst}: {document!r}.", rhal.SKR_EIO)

    def close(self) -> None:
        if self._connection is not None:
            try:
                self._connection.close()
            except OSError:
                pass
            self._connection = None


class SchunkBridge:
    """A body made of one gripper's jaw."""

    #: A stop written to the host while a motion runs is read at once: `emergency_stop` is safe from another thread (python-host's `stop_during_extension`).
    stop_during_extension = True

    def __init__(self) -> None:
        self._opened = False
        self._estop = False
        self._descriptor: str | None = None
        self._seq = 0
        self._simulated = True
        self._limits = (0.0, 0.0)
        self._speed = 0.0
        self._roles: dict[str, str | None] = {}
        self._period_s = 1.0 / _RATE_HZ
        self._rate_hz = _RATE_HZ
        self._stale_s = _STALE_S
        self._stop_mode = ""
        self._host = ""
        self._port = _DEFAULT_PORT
        self._timeout = _HTTP_TIMEOUT_S
        self._fieldbus = "EI"
        self._info: dict = {}
        self._module: _Module | None = None
        self._stop_module: _Module | None = None
        self._poller: threading.Thread | None = None
        self._closing = threading.Event()
        self._lock = threading.Lock()
        self._writer = threading.Lock()
        self._latest: frames.Input | None = None
        self._stamp_ns = 0
        self._dead: str | None = None
        self._blip_until = 0.0
        self._target_um: int | None = None
        self._last_refusal: str | None = None
        self._stop_threads: list[threading.Thread] = []

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
        self._estop = False
        self._descriptor = None
        self._last_refusal = None
        try:
            self._open(settings)
        except BridgeRefusal as refusal:
            self._teardown()
            _say(str(refusal))
            return rhal.SKR_EIO if isinstance(refusal, Unreachable) else rhal.SKR_ENODEV
        except (OSError, frames.ProtocolError) as error:
            self._teardown()
            _say(f"the gripper's module could not be read: {error}")
            return rhal.SKR_EIO
        self._opened = True
        return rhal.SKR_OK

    def _open(self, settings: dict) -> None:
        unknown = sorted(set(settings) - _KNOWN_KEYS)
        if unknown:
            raise BridgeRefusal(f"the configuration has {', '.join(map(repr, unknown))}, which this driver does not read. The keys it reads are {', '.join(sorted(_KNOWN_KEYS))}.")
        host = settings.get("host")
        if not isinstance(host, str) or not host:
            raise BridgeRefusal("no `host` in the configuration. This driver does not scan for a gripper, and one that guessed an address would command whichever module answered first.")
        port = settings.get("port", _DEFAULT_PORT)
        if isinstance(port, bool) or not isinstance(port, int) or not 0 < port < 65536:
            raise BridgeRefusal(f"`port` of {port!r} is not a port.")
        self._host, self._port = host, port
        stop = settings.get("stop_mode")
        if stop not in ("fast", "controlled"):
            raise BridgeRefusal(
                "`stop_mode` is not in the configuration, or is neither `fast` nor `controlled`. What stopping a gripper means is a person's decision and this driver will not make it: `fast` writes the module's fast stop, "
                "which leaves it in error 0xD9 until it is acknowledged; `controlled` writes the stop bit, which ends the motion with no error. Neither releases a workpiece.")
        self._stop_mode = stop
        claim = settings.get("simulator")
        if not isinstance(claim, bool):
            raise BridgeRefusal("`simulator` is not in the configuration, or is not true or false. Nothing in the module says whether it is a simulation, so the descriptor cannot be left to guess: a bench run described "
                                "as a simulation costs a tier it could have had, and a simulation described as a bench run is a false claim.")
        self._simulated = claim
        roles = settings.get("roles") or {}
        if not isinstance(roles, dict) or set(roles) - {JOINT}:
            raise BridgeRefusal(f"`roles` is a mapping from a joint's name to its role, and the one joint is {JOINT!r}.")
        self._roles = {JOINT: roles.get(JOINT)}
        self._rate_hz = _bounded(settings, "rate_hz", _RATE_HZ, 0.5, 20.0)
        self._period_s = 1.0 / self._rate_hz
        self._timeout = _bounded(settings, "http_timeout_ms", _HTTP_TIMEOUT_S * 1000, 20.0, 30000.0) / 1000.0
        self._stale_s = _bounded(settings, "stale_ms", _STALE_S * 1000, 20.0, 30000.0) / 1000.0
        open_s = _bounded(settings, "open_timeout_ms", _OPEN_TIMEOUT_S * 1000, 100.0, 60000.0) / 1000.0

        module = _Module(host, port, min(self._timeout, open_s))
        self._module = module
        fieldbus_raw = frames.decode_parameter("0x1130", bytes.fromhex(module.read("0x1130", timeout=open_s)), "EI")
        fieldbus = frames.FIELDBUS.get(fieldbus_raw)
        if fieldbus is None:
            raise BridgeRefusal(f"the module reports fieldbus {fieldbus_raw!r}, which is not one of the maker's (PN, EI, EC, MB).")
        if fieldbus == "MB":
            raise BridgeRefusal("this module is a Modbus RTU one. This driver speaks the web interface of the Ethernet modules (EtherNet/IP and EtherCAT), and has no serial port to open.")
        if fieldbus == "PN":
            raise BridgeRefusal(
                "this is a PROFINET module, which this driver does not drive. The maker's own client reads the error byte at byte 12 of the input frame for every fieldbus, and the maker's own PROFINET dump has the "
                "error code 0xD9 at byte 15: nothing here can say which is right, and a driver that read the wrong byte would report no error from a module that holds one.")
        self._fieldbus = fieldbus
        info: dict = {"fieldbus": fieldbus}
        for param, spec in frames.PARAMETERS.items():
            if param == "0x1130":
                continue
            info[spec.name] = frames.decode_parameter(param, bytes.fromhex(module.read(param, timeout=open_s)), fieldbus)
        module_type = frames.MODULE_TYPES.get(info["module_type"])
        if module_type is None:
            raise BridgeRefusal(f"the module's type is {info['module_type']!r}, which is not one the maker's client knows (EGU, EGK or EZU).")
        info["module"] = module_type
        self._info = info
        low, high = info["min_pos"] / 1000.0, info["max_pos"] / 1000.0
        if not low < high:
            raise BridgeRefusal(f"the module reports a travel of [{low}, {high}] m, which is not a range.")
        self._limits = self._load_limits(settings.get("limits"), low, high)
        self._speed = self._load_speed(settings.get("speed"), info)

        state = frames.parse_input(module.read(frames.Instance.INPUT, timeout=open_s), fieldbus)
        if not state.bit(Status.ERROR) and not state.bit(Status.READY):
            raise BridgeRefusal("the module says it is not ready for operation (status bit 0 is clear) and holds no error. This driver does not bring a module out of that: do it where the gripper is.")
        self._latest, self._stamp_ns = state, rhal.acquisition_ns()
        self._target_um = state.position_um
        if state.bit(Status.ERROR):
            self._estop = True
            what = "its fast stop is active" if state.error == frames.ERROR_FAST_STOP else "it holds an error the maker's client does not name"
            _say(f"the gripper reports error 0x{state.error:02X}: {what}. It is opened with the stop latched, and nothing is commanded until `release_stop` acknowledges it (the maker's own simulator, "
                 "like a real module, comes up this way until its controller does). The acknowledge is verified: if the cause is still there the stop stays latched.")
        self._stop_module = _Module(host, port, 0.25)
        self._closing.clear()
        self._dead = None
        self._poller = threading.Thread(target=self._poll, daemon=True, name="schunk-poll")
        self._poller.start()
        self._descriptor = self._build_descriptor()

    def _load_limits(self, given: object, low: float, high: float) -> tuple[float, float]:
        if given is None:
            return (low, high)
        if not (isinstance(given, (list, tuple)) and len(given) == 2 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) for v in given)):
            raise BridgeRefusal("`limits` is [min, max] in metres, as two numbers.")
        lo, hi = float(given[0]), float(given[1])
        if not lo < hi:
            raise BridgeRefusal(f"`limits` of [{lo}, {hi}] is not a range.")
        if lo < low - 1e-9 or hi > high + 1e-9:
            raise BridgeRefusal(f"`limits` of [{lo:.6f}, {hi:.6f}] m is wider than the travel the module itself reports, [{low:.6f}, {high:.6f}]: a configuration may narrow it and never widen it.")
        return (lo, hi)

    def _load_speed(self, given: object, info: dict) -> float:
        low, high = info["min_vel"] / 1000.0, info["max_vel"] / 1000.0
        if isinstance(given, bool) or not isinstance(given, (int, float)) or not math.isfinite(given) or given <= 0:
            raise BridgeRefusal(f"no `speed` in the configuration, or it is not a number above zero. Nothing in a position command says how fast the jaw goes: give it in m/s, between the module's own "
                                f"minimum and maximum, {low:g} and {high:g}.")
        if not low - 1e-12 <= given <= high + 1e-12:
            raise BridgeRefusal(f"`speed` of {given:g} m/s is outside what the module itself reports it can do, {low:g} to {high:g} m/s.")
        return float(given)

    def close(self) -> int:
        # A close writes nothing. A jaw that is closing on a workpiece keeps closing, and holds it: what a stop does to a held workpiece (with grip force and position maintenance, or without) is in the maker's
        # firmware manual, which is behind a sign-in this repository does not use, so a stop is the operator's to ask for and never a close's.
        self._teardown()
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        now = rhal.acquisition_ns()
        with self._lock:
            state, stamp, dead = self._latest, self._stamp_ns, self._dead
        self._seq += 1
        out.seq = self._seq
        out.stamp_ns = stamp or now
        out.flags = rhal.HOMING_NOT_REQUIRED
        out.fault_code = 0
        out.position_rad = [math.nan]
        out.velocity_rad_s = [math.nan]
        out.effort_nm = [math.nan]
        if state is None:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        out.position_rad = [frames.um_to_m(state.position_um)]
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP
        if state.bit(Status.ERROR):
            out.fault_code = state.error or 1
            if state.error == frames.ERROR_FAST_STOP:
                out.flags |= rhal.FLAG_ESTOP | rhal.FLAG_DEGRADED
            else:
                out.flags |= rhal.FLAG_FAULT | rhal.FLAG_DEGRADED
        elif state.bit(Status.WARNING) or not state.bit(Status.READY):
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
        if not self._opened or self._module is None:
            return rhal.SKR_EPERM
        if cmd.joint_count != 1:
            return self._refused("count", f"the command has {cmd.joint_count} joints and this body has one, {JOINT}: nothing was sent.")
        if cmd.mode != rhal.MODE_POSITION:
            return self._refused("mode", f"the command is in mode {cmd.mode}, and this gripper takes a position: its target is the jaw's, in micrometres, and there is no velocity or torque route. Send positions.")
        if self._estop:
            return rhal.SKR_EPERM
        applied = cmd.mask[0] if cmd.mask else 1
        if not applied:
            return rhal.SKR_OK
        value = cmd.values[0]
        low, high = self._limits
        if not math.isfinite(value):
            return self._refused("finite", f"the jaw was commanded to {value!r}, which is not finite: the command was refused and nothing was sent.")
        if not low - 1e-9 <= value <= high + 1e-9:
            return self._refused("range", f"the jaw was commanded to {value:.6f} m, outside its travel [{low:.6f}, {high:.6f}]: the command was refused whole and nothing was sent, where a module can answer *not feasible* "
                                          "to a target it cannot reach and do nothing.")
        with self._lock:
            state = self._latest
        if state is not None and state.bit(Status.ERROR):
            return self._refused("error", f"the gripper holds error 0x{state.error:02X}, and a module in error takes nothing but an acknowledge: `release_stop` acknowledges it. The command was not sent.", rhal.SKR_EPERM)
        target = frames.m_to_um(min(max(value, low), high))
        if target == self._target_um:
            # The target the module already has: a stream at the control rate that asks for the same place says nothing new, and a module that is on its way must not be told again.
            self._last_refusal = None
            return rhal.SKR_OK
        try:
            self._command({Bit.MOVE_ABSOLUTE: True}, position=target, speed=frames.m_to_um(self._speed), budget=_COMMAND_BUDGET_S)
        except BridgeRefusal as error:
            return self._refused("command:" + str(error.code), str(error), error.code)
        except frames.ProtocolError as error:
            return self._refused("protocol", str(error))
        self._target_um = target
        self._last_refusal = None
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        self._estop = True
        thread = threading.Thread(target=self._stop_quietly, daemon=True, name="schunk-stop")
        self._stop_threads = [t for t in self._stop_threads if t.is_alive()] + [thread]
        thread.start()
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """Clear the session latch by the maker's acknowledge, and say whether the gripper can be commanded again (§48.2): the acknowledge is verified to have cleared the error."""
        for thread in self._stop_threads:
            thread.join(timeout=1.0)
        if self._module is None or self._stop_module is None:
            self._estop = False
            return rhal.SKR_OK
        try:
            self._command({Bit.ACKNOWLEDGE: True}, module=self._stop_module, budget=self._timeout * 4 + 1.0, latched_ok=True)
            state = frames.parse_input(self._stop_module.read(frames.Instance.INPUT, timeout=self._timeout), self._fieldbus)
        except (BridgeRefusal, frames.ProtocolError) as error:
            _say(f"the gripper cannot be commanded again: {error}")
            return rhal.SKR_EIO
        if state.bit(Status.ERROR) or not state.bit(Status.READY):
            _say(f"the gripper still reports error 0x{state.error:02X} after an acknowledge (ready for operation: {bool(state.bit(Status.READY))}): the cause is still there. The stop stays latched; clear it where the gripper is.")
            return rhal.SKR_EIO
        with self._lock:
            self._latest, self._stamp_ns = state, rhal.acquisition_ns()
        self._target_um = state.position_um
        self._estop = False
        self._last_refusal = None
        return rhal.SKR_OK

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── the handshake ────────────────────────────────────────────────────────

    def _command(self, bits: dict[int, bool], *, position: int | None = None, speed: int | None = None, force: int | None = None, module: _Module | None = None,
                 budget: float = _COMMAND_BUDGET_S, latched_ok: bool = False, wait_toggle: bool = True) -> frames.Input:
        """The maker's handshake (`Driver._send_cmd` and `wait_for_status`), under the writer lock, with the stop latch looked at immediately before the command is written.

        Returns the input frame in which the toggle flipped. Raises `BridgeRefusal` with the code the caller should answer.
        """
        module = module or self._module
        assert module is not None
        deadline = time.monotonic() + budget

        def left(doing: str) -> float:
            """What is left of the budget, as a request's timeout. F256: every request used to be given at least 50 ms whatever was left, so a handshake on a slow link ran a request past the budget (the chaos contract
            measured 256 to 329 ms against the control loop's quarter of a second). With the budget spent nothing more is sent: a command that cannot finish in time is EIO now and not a late write."""
            remaining = deadline - time.monotonic()
            if remaining < _MIN_REQUEST_S:
                raise BridgeRefusal(f"the gripper's module did not finish the handshake in the {budget:.2f} s a command is given (it was {doing}): the command was not completed. A slow link, not a refusal by the gripper.", rhal.SKR_EIO)
            return min(self._timeout, remaining)

        with self._writer:
            if self._estop and not latched_ok:
                raise BridgeRefusal("the stop is latched: nothing is written.", rhal.SKR_EPERM)
            with module.lock:
                module.write(frames.Instance.OUTPUT, frames.clear_output().hex().upper(), timeout=left("clearing the control word"))
                before = frames.parse_input(module.read(frames.Instance.INPUT, timeout=left("reading the toggle")), self._fieldbus)
                if self._estop and not latched_ok:
                    raise BridgeRefusal("the stop was latched while the command was being made: it was not written.", rhal.SKR_EPERM)
                frame = frames.command_output(bits, self._fieldbus, position=position, speed=speed, force=force)
                module.write(frames.Instance.OUTPUT, frame.hex().upper(), timeout=left("writing the command"))
                expected = before.toggle ^ 1
                if not wait_toggle:
                    return before
                while True:
                    state = frames.parse_input(module.read(frames.Instance.INPUT, timeout=left("waiting for the toggle")), self._fieldbus)
                    if state.toggle == expected:
                        break
                    if time.monotonic() >= deadline:
                        raise BridgeRefusal(f"the gripper did not take the command: its command-received toggle did not flip in {budget:.2f} s (status bits {frames.bits_set(state.word)}).", rhal.SKR_EIO)
                    if self._estop and not latched_ok:
                        raise BridgeRefusal("the stop was latched while the command was being made.", rhal.SKR_EPERM)
                    time.sleep(0.01)
        if state.bit(Status.NOT_FEASIBLE):
            raise BridgeRefusal("the gripper answered *not feasible* (status bit 3): it will not do what was asked, with the position, speed or force it was given.", rhal.SKR_EINVAL)
        if state.bit(Status.ERROR) and not latched_ok:
            raise BridgeRefusal(f"the gripper reports error 0x{state.error:02X} after the command: it did not take it. `release_stop` acknowledges it.", rhal.SKR_EPERM)
        return state

    def _stop_quietly(self) -> None:
        """The latch is already holding, so this is the module being told. A request that was lost is asked again for a few seconds, and then `read_state` reports what the module says."""
        module = self._stop_module
        if module is None:
            return
        bits = {Bit.FAST_STOP: False} if self._stop_mode == "fast" else {Bit.STOP: True}
        for attempt in range(_STOP_ATTEMPTS):
            try:
                self._command(bits, module=module, budget=1.0, latched_ok=True)
                return
            except (BridgeRefusal, frames.ProtocolError, OSError):
                if self._closing.is_set() and self._stop_module is None:
                    return
                time.sleep(_STOP_RETRY_S)

    # ── plumbing ─────────────────────────────────────────────────────────────

    def _refused(self, cause: str, message: str, code: int = rhal.SKR_EINVAL) -> int:
        """`code` and the reason, said once for a run of the same cause: a stream at the control rate that fails for one reason would otherwise say it every period."""
        if cause != self._last_refusal:
            self._last_refusal = cause
            _say(message)
        return code

    def _poll(self) -> None:
        module = _Module(self._host, self._port, self._timeout)
        seen = 0
        while not self._closing.is_set():
            began = time.monotonic()
            try:
                state = frames.parse_input(module.read(frames.Instance.INPUT), self._fieldbus)
                if module.reconnects != seen:
                    seen = module.reconnects
                    self._blip_until = time.monotonic() + self._stale_s
                with self._lock:
                    self._latest, self._stamp_ns, self._dead = state, rhal.acquisition_ns(), None
            except (BridgeRefusal, frames.ProtocolError) as error:
                with self._lock:
                    self._dead = str(error)
            self._closing.wait(max(0.0, _STATE_POLL_S - (time.monotonic() - began)))
        module.close()

    def _teardown(self) -> None:
        self._closing.set()
        if self._poller is not None:
            self._poller.join(timeout=2.0)
        self._poller = None
        for module in (self._module, self._stop_module):
            if module is not None:
                module.close()
        self._module = self._stop_module = None
        self._latest = None

    def _build_descriptor(self) -> str:
        low, high = self._limits
        info = self._info
        joint = {"name": JOINT, "role": self._roles.get(JOINT), "type": "prismatic",
                 "limits": {"position_min_rad": low, "position_max_rad": high, "velocity_max_rad_s": self._speed}, "control_modes": ["position"]}
        effector = {"name": "gripper", "role": self._roles.get(JOINT), "kind": "gripper", "dof": 1, "max_grip_force_n": None, "max_opening_m": high - low}
        descriptor = {
            "schema_version": 1, "robot_slug": "schunk-" + str(info["module"]).lower().replace("_", "-"), "digest": None, "verified": False, "simulator": self._simulated, "base": "fixed", "dof": 1,
            "joints": [joint], "end_effectors": [effector], "sensors": [],
            "control": {"modes": ["position"], "rate_hz_max": self._rate_hz, "rate_hz_min": 0.5},
            # Null: the module reports its own travel, speed and force limits, and none of them is a risk assessment (§5.2). No force in newtons: the module's is a percentage of a rating this driver cannot see.
            "envelope": None, "capabilities": ["safety.estop"], "motions": [_DESCRIPTORS[name] for name in _MOTIONS], "extensions": [TOKEN], "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))

    # ── the extension: motions ───────────────────────────────────────────────

    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        if token != TOKEN:
            return rhal.SKR_ENOSYS, f"unknown token {token!r}. This driver serves {TOKEN}.".encode()
        if not self._opened or self._module is None:
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
        if self._estop and motion not in ("status", "param"):
            return rhal.SKR_EPERM, b"refused: the stop is latched. Only `status` and `param` are answered until it is released."
        try:
            return rhal.SKR_OK, json.dumps(getattr(self, f"_motion_{motion}")(args), separators=(",", ":")).encode()
        except BridgeRefusal as refusal:
            return refusal.code, str(refusal).encode()
        except frames.ProtocolError as error:
            return rhal.SKR_EINVAL, f"the request does not fit the wire: {error}".encode()

    def _number(self, args: dict, key: str, *, required: bool = False, minimum: float | None = None, maximum: float | None = None) -> float | None:
        if key not in args:
            if required:
                raise BridgeRefusal(f"`{key}` is required.", rhal.SKR_EINVAL)
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

    def _present(self) -> frames.Input:
        with self._lock:
            state = self._latest
        if state is None:
            raise BridgeRefusal("the gripper has not been read yet.", rhal.SKR_ETIMEDOUT)
        return state

    def _wait(self, patterns: dict[str, dict[int, int]], timeout: float) -> tuple[str | None, frames.Input]:
        """The maker's `wait_for_any_status`: the first pattern of status bits the module shows, or `None` on a timeout. A stop latched while it waits ends it."""
        assert self._stop_module is not None
        waiter = _Module(self._host, self._port, self._timeout)
        deadline = time.monotonic() + timeout
        state = self._present()
        try:
            while time.monotonic() < deadline:
                if self._estop:
                    raise BridgeRefusal("the motion ended: the stop was latched.", rhal.SKR_EPERM)
                state = frames.parse_input(waiter.read(frames.Instance.INPUT), self._fieldbus)
                for name, pattern in patterns.items():
                    if all(state.bit(bit) == value for bit, value in pattern.items()):
                        return name, state
                time.sleep(_STATE_POLL_S)
        finally:
            waiter.close()
        return None, state

    def _motion_status(self, args: dict) -> dict:
        self._no_arguments("status", args)
        state = self._present()
        return {"module": self._info["module"], "fieldbus": self._fieldbus, "serial": self._info["serial_no_txt"], "firmware": self._info["sw_version_txt"],
                "position_m": frames.um_to_m(state.position_um), "limits_m": list(self._limits), "speed_m_s": self._speed, "target_m": None if self._target_um is None else frames.um_to_m(self._target_um),
                "status_bits": [Status.NAMES[bit] for bit in frames.bits_set(state.word)], "error": f"0x{state.error:02X}", "warning": f"0x{state.warning:02X}", "additional": f"0x{state.additional:02X}",
                "stop_latched": self._estop, "simulator": self._simulated}

    def _motion_param(self, args: dict) -> dict:
        name = args.pop("name", None)
        self._no_arguments("param", args)
        if name is None:
            raise BridgeRefusal("`name` is required: a parameter such as 0x0600 or min_pos.", rhal.SKR_EINVAL)
        by_name = {spec.name: param for param, spec in frames.PARAMETERS.items()}
        param = by_name.get(name, name)
        if param not in frames.PARAMETERS:
            raise BridgeRefusal(f"{name!r} is not a parameter this driver reads: {', '.join(sorted(by_name))}.", rhal.SKR_EINVAL)
        assert self._stop_module is not None
        reader = _Module(self._host, self._port, self._timeout)
        try:
            raw = bytes.fromhex(reader.read(param))
        finally:
            reader.close()
        spec = frames.PARAMETERS[param]
        return {"param": param, "name": spec.name, "value": frames.decode_parameter(param, raw, self._fieldbus), "unit": spec.unit, "raw": raw.hex().upper()}

    def _motion_move(self, args: dict) -> dict:
        low, high = self._limits
        position = self._number(args, "position", required=True, minimum=low, maximum=high)
        speed = self._number(args, "speed", minimum=self._info["min_vel"] / 1000.0, maximum=self._info["max_vel"] / 1000.0) or self._speed
        self._no_arguments("move", args)
        state = self._present()
        assert position is not None
        self._command({Bit.MOVE_ABSOLUTE: True}, position=frames.m_to_um(position), speed=frames.m_to_um(speed), budget=_TOGGLE_WAIT_S)
        self._target_um = frames.m_to_um(position)
        seconds = abs(position - frames.um_to_m(state.position_um)) / speed + 2.0
        name, state = self._wait({"reached": {Status.SUCCESS: 1, Status.POSITION_REACHED: 1}, "error": {Status.ERROR: 1}}, seconds)
        if name != "reached":
            raise BridgeRefusal(f"the move did not complete: {'the gripper reports error 0x%02X' % state.error if name == 'error' else f'no completion in {seconds:.1f} s'}.", rhal.SKR_EIO)
        return {"position_m": frames.um_to_m(state.position_um)}

    def _motion_grip(self, args: dict) -> dict:
        ceiling = float(self._info["max_grp_force"])
        force = self._number(args, "force", required=True, minimum=0.0, maximum=ceiling)
        low, high = self._limits
        position = self._number(args, "position", minimum=low, maximum=high)
        velocity = self._number(args, "velocity", minimum=0.0, maximum=self._info["max_grp_vel"] / 1000.0)
        outward = args.pop("outward", "0") not in ("0", "false", "no")
        self._no_arguments("grip", args)
        assert force is not None
        if force <= 0:
            raise BridgeRefusal(f"`force` must be above zero and at most {ceiling:g} (a percentage: the module's own rating).", rhal.SKR_EINVAL)
        bits = {Bit.GRIP_AT_POSITION if position is not None else Bit.GRIP_WORKPIECE: True, Bit.GRIP_DIRECTION: outward}
        state = self._present()
        self._command(bits, position=None if position is None else frames.m_to_um(position), speed=None if velocity is None else frames.m_to_um(velocity), force=int(round(force)), budget=_TOGGLE_WAIT_S)
        # F253: the module took a grip, and the jaw goes where the workpiece (or the stop of its travel) is, not where this driver last sent it. Keeping the old target would make `write_command` swallow a stream that
        # asked for that place again as "already issued", with the jaw somewhere else and nothing said.
        self._target_um = None
        travel = abs((position if position is not None else (high if outward else low)) - frames.um_to_m(state.position_um))
        seconds = travel / (velocity or self._info["max_grp_vel"] / 1000.0) + 2.0
        patterns = {"gripped": {Status.SUCCESS: 1, Status.GRIPPED: 1}, "no_workpiece": {Status.SUCCESS: 0, Status.NO_WORKPIECE: 1}, "wrong_workpiece": {Status.SUCCESS: 0, Status.WRONG_WORKPIECE: 1},
                    "workpiece_lost": {Status.SUCCESS: 0, Status.WORKPIECE_LOST: 1}, "error": {Status.ERROR: 1}}
        name, state = self._wait(patterns, seconds)
        if name is None:
            raise BridgeRefusal(f"the grip did not complete in {seconds:.1f} s.", rhal.SKR_EIO)
        if name == "error":
            raise BridgeRefusal(f"the gripper reports error 0x{state.error:02X} during the grip.", rhal.SKR_EIO)
        return {"result": name, "position_m": frames.um_to_m(state.position_um)}

    def _motion_release(self, args: dict) -> dict:
        self._no_arguments("release", args)
        self._command({Bit.RELEASE_WORKPIECE: True}, budget=_TOGGLE_WAIT_S)
        self._target_um = None  # F253, as in `_motion_grip`: from here the jaw is going where the module's release distance takes it, whether or not the motion completes
        seconds = float(self._info["wp_release_delta"]) / max(1.0, float(self._info["max_grp_vel"])) + 2.0
        name, state = self._wait({"released": {Status.SUCCESS: 1, Status.POSITION_REACHED: 1}, "error": {Status.ERROR: 1}}, seconds)
        if name != "released":
            raise BridgeRefusal(f"the release did not complete: {'the gripper reports error 0x%02X' % state.error if name == 'error' else f'no completion in {seconds:.1f} s'}.", rhal.SKR_EIO)
        self._target_um = state.position_um
        return {"position_m": frames.um_to_m(state.position_um)}


#: One entry per motion name (§84.1).
_DESCRIPTORS: dict[str, dict] = {
    "status": {"name": "status", "parameters": []},
    "param": {"name": "param", "parameters": [{"name": "name", "required": True}]},
    "move": {"name": "move", "parameters": [{"name": "position", "unit": "m", "required": True}, {"name": "speed", "unit": "m/s", "required": False}]},
    "grip": {"name": "grip", "parameters": [{"name": "force", "unit": "%", "required": True, "min": 0.0}, {"name": "position", "unit": "m", "required": False},
                                            {"name": "velocity", "unit": "m/s", "required": False}, {"name": "outward", "required": False}]},
    "release": {"name": "release", "parameters": []},
}


def _bounded(settings: dict, key: str, default: float, low: float, high: float) -> float:
    value = settings.get(key)
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not low <= value <= high:
        raise BridgeRefusal(f"`{key}` of {value!r} is not a number from {low:g} to {high:g}.")
    return float(value)


def _say(message: str) -> None:
    print(f"skynet-schunk: {message}", file=sys.stderr, flush=True)
