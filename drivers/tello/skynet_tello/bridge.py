"""The RHAL driver: a Ryze (DJI) Tello over the SDK 2.0's text commands (DRY-COVERAGE-PLAN.md item 12.E.11).

§4's five mandatory entry points over :mod:`skynet_tello.frames` and :mod:`skynet_tello.link`. It links nothing of Ryze's: the text grammar and the state string are the SDK 2.0 User Guide's, and `layout_check.py` holds this
driver's commands and its reading of the state string to djitellopy (MIT), a client of the same guide that is run.

# The body

A drone: no joints (a command in joint space is refused, naming the route that moves it), a height and nothing else it can say of where it is. The Tello streams `h`, "the height in cm", and attitude angles whose sign
conventions the guide does not give, and it has no horizontal position at all, so the base pose is the height beneath an unknown position and an unknown orientation (NaN where it is not known) and `status` reports the raw
fields. It moves by **motions** (`extension`, token `vendor.tello.task`): `takeoff`, `land`, `hover`, `move`, `rotate`, `speed` and `drive`, the sticks (`rc`).

# What it never does on its own

It does not take off, land or move. It enters SDK mode when it opens (`command`, which the guide says must be first and which moves nothing) and keeps the drone's 15-second watchdog fed with a read command while a session
is open (`keepalive_s`; 0 turns that off, and the drone then lands itself 15 s after the last thing any client said). A command is never retried after its reply is lost: a motion that was executed and whose `ok` was lost is a
motion that would be flown twice. Reads and the SDK-mode handshake, which are idempotent, are.

# The stop

`stop_mode` is required and is one of three, because which is the safer is the integrator's: **`hover`** is the guide's `stop` ("Hovers in the air. Note: works at any time."), **`land`** is `land`, and **`emergency`** is `emergency`
("Stop motors immediately": the drone falls). `emergency_stop` latches and returns and never touches the socket; commands are refused from that instant, and the reading thread sends the stop three times (a datagram can be lost) and
waits to see it taken: an `ok` after it was sent, or the state string showing a hover (no speed) for `hover` and the ground for `land`. `emergency` is not answered by the drone, so it is delivered when it has been sent three times.
It is a software stop over a Wi-Fi link: there is no safety circuit on this wire.
"""

from __future__ import annotations

import contextlib
import ipaddress
import json
import math
import os
import sys
import threading
import time

import skynet_rhal as rhal

from . import frames, link

__all__ = ["BridgeRefusal", "TelloBridge", "TOKEN"]

TOKEN = "vendor.tello.task"
_MOTIONS = ("status", "takeoff", "land", "hover", "move", "rotate", "speed", "drive")
_KNOWN_KEYS = {"host", "port", "state_port", "local_port", "simulator", "stop_mode", "keepalive_s", "reply_timeout_ms", "takeoff_timeout_ms", "stale_ms", "open_timeout_ms", "low_battery_percent"}
_STOP_MODES = ("hover", "land", "emergency")
_REPLY_TIMEOUT_S = 7.0
_TAKEOFF_TIMEOUT_S = 20.0
_OPEN_TIMEOUT_S = 3.0
_STALE_S = 1.0
_KEEPALIVE_S = 5.0
#: The Tello "may not respond to commands that follow each other too closely": the maker's sample client waits this long between two.
_MIN_GAP_S = 0.1
#: A command that is sent three times in all (the handshake and the reads): a datagram can be lost.
_ATTEMPTS = 3
_STOP_REPEATS = 3
_STOP_GAP_S = 0.03
_STOP_RETRY_S = 0.5
_LOW_BATTERY = 10
#: Slowest speed the guide allows, the pace a wait is sized for when the drone's speed has not been read.
_SLOWEST_CM_S = 10
#: A rotation's pace assumed for sizing a wait, in degrees a second: the guide gives none, so this is generous.
_SLOW_ROTATION_DEG_S = 30.0
_LANDED_CM = 5


class BridgeRefusal(Exception):
    """A reason this body did not come up or a command did not happen, in the terms an integrator acts on."""

    def __init__(self, message: str, code: int = rhal.SKR_ENODEV) -> None:
        super().__init__(message)
        self.code = code


class Unreachable(BridgeRefusal):
    """The drone did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""

    def __init__(self, message: str) -> None:
        super().__init__(message, rhal.SKR_EIO)


class _Pending:
    __slots__ = ("text", "sent", "reply")

    def __init__(self, text: str) -> None:
        self.text = text
        self.sent = time.monotonic()
        self.reply: frames.Reply | None = None


class TelloBridge:
    """A body made of one Tello."""

    #: A stop written to the host while a motion runs is read at once: `emergency_stop` is safe from another thread (python-host's `stop_during_extension`).
    stop_during_extension = True

    def __init__(self) -> None:
        self._opened = False
        self._estop = False
        self._descriptor: str | None = None
        self._seq = 0
        self._simulated = True
        self._stop_mode = "hover"
        self._keepalive_s = _KEEPALIVE_S
        self._reply_timeout = _REPLY_TIMEOUT_S
        self._takeoff_timeout = _TAKEOFF_TIMEOUT_S
        self._stale_s = _STALE_S
        self._low_battery = _LOW_BATTERY
        self._link: link.UdpLink | None = None
        self._thread: threading.Thread | None = None
        self._keeper: threading.Thread | None = None
        self._closing = threading.Event()
        self._wake_r = self._wake_w = -1
        self._wake_lock = threading.Lock()
        self._command_lock = threading.Lock()
        self._lock = threading.Lock()
        self._changed = threading.Condition(self._lock)
        # Everything below is guarded by `_lock`.
        self._latest: frames.State | None = None
        self._stamp_ns = 0
        self._states = 0
        self._bad_states = 0
        self._dead: str | None = None
        self._pending: _Pending | None = None
        self._stray = 0
        self._last_reply = 0.0
        self._last_sent = 0.0
        self._stop_calls = 0
        self._stop_delivered = False
        self._stop_sent = 0
        self._stop_last_sent = 0.0
        self._stop_last_sent_ns = 0
        self._stop_acked = False
        self._speed_cm_s: int | None = None
        self._sdk: str | None = None
        self._serial: str | None = None
        self._errors: list[str] = []

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
            return refusal.code if isinstance(refusal, Unreachable) else rhal.SKR_ENODEV
        except (OSError, frames.ProtocolError, link.LinkError) as error:
            self._teardown()
            _say(f"the drone could not be reached: {error}")
            return rhal.SKR_EIO
        self._opened = True
        return rhal.SKR_OK

    def _reset_session(self) -> None:
        """A session starts with no stop, no fault remembered and no reading: nothing of the last one is the drone's."""
        self._estop = False
        self._descriptor = None
        with self._lock:
            self._latest = None
            self._states = self._bad_states = 0
            self._dead = None
            self._pending = None
            self._stray = 0
            self._stop_delivered = False
            self._stop_sent = 0
            self._stop_acked = False
            self._speed_cm_s = None
            self._sdk = self._serial = None
            self._errors = []

    def _open(self, settings: dict) -> None:
        unknown = sorted(set(settings) - _KNOWN_KEYS)
        if unknown:
            raise BridgeRefusal(f"the configuration has {', '.join(map(repr, unknown))}, which this driver does not read. The keys it reads are {', '.join(sorted(_KNOWN_KEYS))}.")
        host = settings.get("host")
        try:
            if not isinstance(host, str):
                raise ValueError
            ipaddress.IPv4Address(host)
        except ValueError:
            raise BridgeRefusal(f"`host` of {host!r} is not the drone's address: an IPv4 address ({frames.FACTORY_HOST} from the factory, in the Tello's own access point). A host name is not accepted: a datagram is believed by the address it came from, "
                                "and this driver does not scan for a drone.") from None
        ports = {key: settings.get(key, default) for key, default in (("port", frames.COMMAND_PORT), ("state_port", frames.STATE_PORT), ("local_port", 0))}
        for key, value in ports.items():
            if isinstance(value, bool) or not isinstance(value, int) or not (0 if key == "local_port" else 1) <= value < 65536:
                raise BridgeRefusal(f"`{key}` of {value!r} is not a port ({'0, for any, or ' if key == 'local_port' else ''}1 to 65535; the drone's command port is {frames.COMMAND_PORT} and it streams its state to {frames.STATE_PORT}).")
        claim = settings.get("simulator")
        if not isinstance(claim, bool):
            raise BridgeRefusal("`simulator` is not in the configuration, or is not true or false. Nothing a Tello says tells a drone from an emulator of one, so the operator says which it is, and the descriptor carries the claim.")
        self._simulated = claim
        mode = settings.get("stop_mode")
        if mode not in _STOP_MODES:
            raise BridgeRefusal(f"`stop_mode` of {mode!r} is not in the configuration, or is not one of {', '.join(_STOP_MODES)}. Which is safer is the integrator's risk assessment for this drone and this room: `hover` is the guide's `stop`, `land` lands "
                                "it, and `emergency` cuts the motors, which drops it.")
        self._stop_mode = mode
        self._keepalive_s = _bounded(settings, "keepalive_s", _KEEPALIVE_S, 0.0, 10.0, zero_ok=True)
        self._reply_timeout = _bounded(settings, "reply_timeout_ms", _REPLY_TIMEOUT_S * 1000, 100.0, 60000.0) / 1000.0
        self._takeoff_timeout = _bounded(settings, "takeoff_timeout_ms", _TAKEOFF_TIMEOUT_S * 1000, 1000.0, 120000.0) / 1000.0
        self._stale_s = _bounded(settings, "stale_ms", _STALE_S * 1000, 100.0, 30000.0) / 1000.0
        self._low_battery = int(_bounded(settings, "low_battery_percent", _LOW_BATTERY, 1.0, 50.0))
        open_s = _bounded(settings, "open_timeout_ms", _OPEN_TIMEOUT_S * 1000, 100.0, 60000.0) / 1000.0

        self._link = link.UdpLink(host, ports["port"], ports["state_port"], ports["local_port"])
        try:
            self._link.open()
        except link.LinkError as error:
            raise Unreachable(str(error)) from error
        self._closing.clear()
        self._open_wake()
        self._thread = threading.Thread(target=self._exchange_loop, args=(self._wake_r,), daemon=True, name="tello-exchange")
        self._thread.start()
        self._keeper = threading.Thread(target=self._keepalive_loop, daemon=True, name="tello-keepalive")
        self._keeper.start()
        self._handshake(open_s)
        self._descriptor = self._build_descriptor()

    def _handshake(self, open_s: float) -> None:
        """`command`, which the guide says is sent before any other, then what the drone says of itself, then the first state string."""
        deadline = time.monotonic() + open_s
        try:
            self._ask(frames.command(), min(self._reply_timeout, max(0.2, open_s / _ATTEMPTS)), attempts=_ATTEMPTS, strict=True)
        except BridgeRefusal as error:
            raise Unreachable(f"the drone did not take `command` ({error}). Is it on, is this host on its access point, and is {self._link.target if self._link else 'its address'} its command port?") from error
        for key in ("sdk", "serial", "speed"):
            try:
                reply = self._ask(frames.read(key), self._reply_timeout, attempts=_ATTEMPTS, strict=True)
            except BridgeRefusal:
                continue
            with self._lock:
                if key == "sdk":
                    self._sdk = reply.text
                elif key == "serial":
                    self._serial = reply.text
                elif reply.text.strip().isdigit():
                    self._speed_cm_s = int(reply.text.strip())
        with self._changed:
            while self._latest is None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise Unreachable(f"no state string arrived on port {self._state_port()} in {open_s:.1f} s after `command`. The drone streams its state to the host that spoke to it, on port {frames.STATE_PORT}: "
                                      "is that port open on this host, and is the `state_port` the one the drone sends to?")
                self._changed.wait(min(remaining, 0.05))

    def _state_port(self) -> int:
        return self._link._state_port if self._link is not None else 0       # noqa: SLF001

    def close(self) -> int:
        # A close writes nothing: the drone keeps doing what it was told, and what stopping it does is the integrator's to decide, never a close's. A drone nobody speaks to lands itself after 15 s.
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
        self._seq += 1
        out.seq = self._seq
        out.stamp_ns = stamp or now
        out.flags = rhal.HOMING_NOT_REQUIRED
        out.fault_code = 0
        out.position_rad = []
        out.velocity_rad_s = []
        out.effort_nm = []
        out.base_pose = [math.nan] * 7
        out.base_twist = [math.nan] * 6
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP | rhal.FLAG_DEGRADED
        if reading is None:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        out.base_pose[2] = reading.h / 100.0
        if reading.bat <= self._low_battery:
            out.flags |= rhal.FLAG_DEGRADED
        age = (now - stamp) / 1e9
        if dead or age > self._stale_s:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        # The shape before the permission, the way `drivers/mavlink` checks it.
        if cmd.joint_count != 0:
            return rhal.SKR_EINVAL
        if cmd.mode not in (rhal.MODE_POSITION, rhal.MODE_VELOCITY, rhal.MODE_TORQUE):
            return rhal.SKR_EINVAL
        _say(f"joint commands are refused: this body has no joints, and the routes that move the drone are `extension` under {TOKEN} (`drive` is the sticks).")
        return rhal.SKR_EPERM

    def emergency_stop(self) -> int:
        """Latch, and wake the thread that sends the stop. The call does not touch the socket: the RHAL's "never blocks" is 50 ms. The latch is the stop: motions are refused from this instant, and the guide's `stop`, `land` or
        `emergency` (the operator's `stop_mode`) is sent as soon as the thread runs, three times, which `status` reports."""
        with self._lock:
            self._estop = True
            self._stop_calls += 1
            self._stop_delivered = False
            self._stop_acked = False
            self._stop_sent = 0
            self._changed.notify_all()
        self._wake()
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """Clear the session latch (§48.2) once the stop has been *seen* delivered. A drone that was told to land is on the ground and one that was told to cut its motors is not flying; neither is told to fly again."""
        if self._link is None or self._thread is None:
            self._estop = False
            return rhal.SKR_OK
        try:
            self._wait(lambda: self._stop_delivered, _STOP_RETRY_S * 8 + self._reply_timeout, honour_stop=False)
        except BridgeRefusal as error:
            _say(f"the stop stays latched: it has not been seen delivered ({error}) Fix the link, or close and open the driver, which is a new session.")
            return rhal.SKR_EIO
        with self._lock:
            self._estop = False
            self._stop_delivered = False
        return rhal.SKR_OK

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── the line ─────────────────────────────────────────────────────────────

    def _send(self, text: str) -> None:
        link_ = self._link
        if link_ is None:
            raise link.LinkError("the sockets are not open.")
        try:
            link_.send(text)
        except link.LinkError as error:
            with self._lock:
                self._dead = str(error)
            raise
        with self._lock:
            self._last_sent = time.monotonic()
            self._dead = None

    def _ask(self, text: str, timeout: float, *, attempts: int = 1, strict: bool = False, unless_stopped: bool = True) -> frames.Reply:
        """Send one command and wait for the drone's answer, one at a time (the drone takes one command at a time), at least `_MIN_GAP_S` after the last answer. `attempts` above one is for commands that are idempotent: a lost datagram is a
        command sent again, and a motion is never one of those. With `strict` an `error` reply is a refusal (BridgeRefusal) and not a reply. The wait ends at once when a stop is latched."""
        if not self._command_lock.acquire(timeout=timeout):
            raise BridgeRefusal(f"the drone was still answering an earlier command after {timeout:.1f} s: `{text}` was not sent.", rhal.SKR_EIO)
        try:
            for attempt in range(attempts):
                gap = _MIN_GAP_S - (time.monotonic() - self._last_reply)
                if gap > 0:
                    time.sleep(gap)
                with self._lock:
                    if unless_stopped and self._estop:
                        raise BridgeRefusal("the stop is latched: nothing was sent.", rhal.SKR_EPERM)
                    pending = _Pending(text)
                    self._pending = pending
                try:
                    self._send(text)
                except link.LinkError as error:
                    with self._lock:
                        self._pending = None
                    raise BridgeRefusal(f"`{text}` could not be sent: {error}", rhal.SKR_EIO) from error
                end = time.monotonic() + timeout
                with self._changed:
                    while pending.reply is None:
                        if self._closing.is_set():
                            self._pending = None
                            raise BridgeRefusal("the driver was closed.", rhal.SKR_EPERM)
                        if unless_stopped and self._estop:
                            self._pending = None
                            raise BridgeRefusal("the motion ended: the stop was latched.", rhal.SKR_EPERM)
                        remaining = end - time.monotonic()
                        if remaining <= 0:
                            break
                        self._changed.wait(min(remaining, 0.05))
                    self._pending = None
                    reply = pending.reply
                if reply is None:
                    if attempt + 1 < attempts:
                        continue
                    raise BridgeRefusal(f"the drone did not answer `{text}` in {timeout:.1f} s (it may be doing it still: `hover` stops it).", rhal.SKR_EIO)
                if strict and reply.kind == "error":
                    raise BridgeRefusal(f"the drone answered `{text}` with {reply.text!r}.", rhal.SKR_EIO)
                return reply
            raise AssertionError("unreachable")
        finally:
            self._command_lock.release()

    # ── the reading thread ───────────────────────────────────────────────────

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
        with self._wake_lock:
            if self._wake_w >= 0:
                with contextlib.suppress(OSError):
                    os.write(self._wake_w, b"x")

    @staticmethod
    def _drain_wake(read_end: int) -> None:
        with contextlib.suppress(OSError):
            while os.read(read_end, 64):
                pass

    def _exchange_loop(self, wake_r: int) -> None:
        """The reading thread: the replies and the state strings in, and the stop out. It waits on both sockets and on a pipe that a stop writes to."""
        link_ = self._link
        assert link_ is not None
        while not self._closing.is_set():
            replies, states, woke = link_.poll(0.05, (wake_r,))
            if woke:
                self._drain_wake(wake_r)
            for data in states:
                self._take_state(data)
            for data in replies:
                self._take_reply(frames.parse_reply(data))
            try:
                self._deliver_if_owed()
            except (link.LinkError, BridgeRefusal):
                pass
            with self._lock:
                self._changed.notify_all()

    def _take_state(self, data: bytes) -> None:
        try:
            state = frames.parse_state(data)
        except frames.ProtocolError:
            with self._lock:
                self._bad_states += 1
            return
        with self._lock:
            self._latest, self._stamp_ns = state, rhal.acquisition_ns()
            self._states += 1
            self._changed.notify_all()

    def _take_reply(self, reply: frames.Reply) -> None:
        with self._lock:
            self._last_reply = time.monotonic()
            if self._estop and not self._stop_delivered and reply.kind == "ok" and self._stop_sent and self._stop_mode != "emergency":
                self._stop_acked = True
            pending = self._pending
            if pending is not None and pending.reply is None and not self._estop:
                pending.reply = reply
            else:
                self._stray += 1
            self._changed.notify_all()

    def _stop_text(self) -> str:
        return {"hover": frames.stop, "land": frames.land, "emergency": frames.emergency}[self._stop_mode]()

    def _deliver_if_owed(self) -> None:
        """The stop, sent three times a short while apart and then again every half second until it is seen taken."""
        with self._lock:
            if not self._estop or self._stop_delivered:
                return
            calls, sent, last, last_ns = self._stop_calls, self._stop_sent, self._stop_last_sent, self._stop_last_sent_ns
            latest, stamp, mode = self._latest, self._stamp_ns, self._stop_mode
        now = time.monotonic()
        if sent >= _STOP_REPEATS and self._stop_taken(mode, latest, stamp > last_ns, now - last):
            with self._lock:
                if calls == self._stop_calls:
                    self._stop_delivered = True
                    self._changed.notify_all()
            return
        if sent and now - last < (_STOP_GAP_S if sent < _STOP_REPEATS else _STOP_RETRY_S):
            return
        try:
            self._send(self._stop_text())
        finally:
            with self._lock:
                if calls == self._stop_calls:
                    self._stop_sent += 1
                    self._stop_last_sent = time.monotonic()
                    self._stop_last_sent_ns = rhal.acquisition_ns()

    @staticmethod
    def _stop_taken(mode: str, latest: frames.State | None, fresh: bool, since_last: float) -> bool:
        """Whether the stop has been seen taken. `emergency` is not answered and shows nothing the guide names, so it is taken when it has been sent; the others by a state string that came after the stop was sent and shows what it does: no
        speed for `hover`, the ground for `land`. An `ok` is not enough (a drone that says it hovers and goes on flying has said it), and it is not needed (a reply can be lost)."""
        if mode == "emergency":
            return since_last >= _STOP_GAP_S
        if latest is None or not fresh:
            return False
        if mode == "hover":
            return latest.vgx == 0 and latest.vgy == 0 and latest.vgz == 0
        return latest.h <= _LANDED_CM

    def _keepalive_loop(self) -> None:
        """A read command when nothing has been sent for `keepalive_s`: "If there is no command for 15 seconds, the Tello will land automatically." Not while a command is in flight, and not while a stop is latched. A thread of its own,
        because the wait for an answer is the reading thread's to end."""
        while not self._closing.wait(0.1):
            if self._keepalive_s <= 0 or self._estop:
                continue
            if time.monotonic() - max(self._last_sent, self._last_reply) < self._keepalive_s:
                continue
            if self._command_lock.locked():
                continue
            try:
                self._ask(frames.read("battery"), self._reply_timeout)
            except BridgeRefusal:
                pass

    def _wait(self, predicate, timeout: float, *, honour_stop: bool = True) -> None:
        deadline = time.monotonic() + timeout
        with self._changed:
            while True:
                if self._closing.is_set():
                    raise BridgeRefusal("the driver was closed.", rhal.SKR_EPERM)
                if honour_stop and self._estop:
                    raise BridgeRefusal("the motion ended: the stop was latched.", rhal.SKR_EPERM)
                if predicate():
                    return
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise BridgeRefusal(f"nothing showed what was waited for in {timeout:.1f} s ({self._dead or 'the link is answering'}).", rhal.SKR_EIO)
                self._changed.wait(min(remaining, 0.05))

    def _teardown(self) -> None:
        self._closing.set()
        self._wake()
        with self._changed:
            self._changed.notify_all()
        thread, self._thread = self._thread, None
        keeper, self._keeper = self._keeper, None
        ended = True
        for each in (thread, keeper):
            if each is not None and each is not threading.current_thread():
                each.join(timeout=2.0)
                ended = ended and not each.is_alive()
        if self._link is not None:
            self._link.close()
        self._link = None
        with self._lock:
            self._latest = None
        if ended:
            self._close_wake()

    def _build_descriptor(self) -> str:
        descriptor = {
            "schema_version": 1, "robot_slug": "ryze-tello", "digest": None, "verified": False, "simulator": self._simulated, "base": "aerial", "dof": 0, "joints": [], "end_effectors": [], "sensors": [],
            "control": {"modes": [], "rate_hz_max": 20, "rate_hz_min": 1, "command_latency_ms": None},
            # Null: the speed limits are the drone's own and the room is the operator's, and none of it is a risk assessment anybody here made (§5.2).
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
            return rhal.SKR_EIO, f"the drone did not carry the request: {error}".encode()

    @staticmethod
    def _number(args: dict, key: str, *, minimum: float | None = None, maximum: float | None = None) -> float | None:
        if key not in args:
            return None
        try:
            value = float(args.pop(key))
        except ValueError:
            raise BridgeRefusal(f"`{key}` is not a number.", rhal.SKR_EINVAL) from None
        if not math.isfinite(value) or (minimum is not None and value < minimum) or (maximum is not None and value > maximum):
            raise BridgeRefusal(f"`{key}` of {value:g} is outside {minimum if minimum is not None else '-inf'} to {maximum if maximum is not None else 'inf'}.", rhal.SKR_EINVAL)
        return value

    @staticmethod
    def _no_arguments(motion: str, args: dict) -> None:
        if args:
            raise BridgeRefusal(f"`{motion}` does not take {', '.join(sorted(args))}.", rhal.SKR_EINVAL)

    @staticmethod
    def _whole_units(value: float, per_unit: float, what: str) -> int:
        """`value` (metres, or degrees) as the whole number of the wire's unit (centimetres: 100 a metre; degrees: 1). A request that is not a whole number of them is refused and not rounded."""
        scaled = value * per_unit
        if abs(scaled - round(scaled)) > 1e-6:
            raise BridgeRefusal(f"{what} of {value:g} is not a whole number of {'centimetres' if per_unit == 100 else 'degrees'}: the Tello takes whole numbers, and a request is not rounded.", rhal.SKR_EINVAL)
        return int(round(scaled))

    def _present(self) -> frames.State:
        with self._lock:
            reading, stamp, dead = self._latest, self._stamp_ns, self._dead
        if reading is None:
            raise BridgeRefusal("the drone has not been read yet.", rhal.SKR_ETIMEDOUT)
        if dead or (rhal.acquisition_ns() - stamp) / 1e9 > self._stale_s:
            raise BridgeRefusal(f"the drone's state is stale ({dead or 'no state string in ' + format(self._stale_s, 'g') + ' s'}): nothing was sent.", rhal.SKR_EIO)
        return reading

    def _ready(self) -> frames.State:
        reading = self._present()
        if reading.bat <= self._low_battery:
            raise BridgeRefusal(f"the battery is at {reading.bat}% (the operator's floor is {self._low_battery}%): nothing was sent.", rhal.SKR_EPERM)
        return reading

    def _control(self, text: str, timeout: float) -> frames.Reply:
        """A control command: one attempt, and an `error` is a refusal that says what the drone said."""
        reply = self._ask(text, timeout)
        if reply.kind == "error":
            raise BridgeRefusal(f"the drone answered `{text}` with {reply.text!r}.", rhal.SKR_EPERM)
        if reply.kind != "ok":
            raise BridgeRefusal(f"the drone answered `{text}` with {reply.text!r}, which is neither `ok` nor an error.", rhal.SKR_EIO)
        return reply

    def _motion_status(self, args: dict) -> dict:
        self._no_arguments("status", args)
        reading = self._present_or_none()
        with self._lock:
            states, bad, dead, stray, sdk, serial, speed = self._states, self._bad_states, self._dead, self._stray, self._sdk, self._serial, self._speed_cm_s
            sent, acked, delivered = self._stop_sent, self._stop_acked, self._stop_delivered
            foreign, oversize = (self._link.foreign, self._link.oversize) if self._link else (0, 0)
        out: dict = {"host": self._link.target if self._link else None, "simulator": self._simulated, "sdk": sdk, "serial": serial, "speed_cm_s": speed, "stop_mode": self._stop_mode, "stop_latched": self._estop,
                     "stop_sent": sent, "stop_acked": acked, "stop_delivered": delivered, "states": states, "bad_states": bad, "stray_replies": stray, "foreign_datagrams": foreign, "oversize_datagrams": oversize, "link_dead": dead,
                     "keepalive_s": self._keepalive_s, "low_battery_percent": self._low_battery}
        if reading is not None:
            out.update({"height_cm": reading.h, "tof_cm": reading.tof, "baro_cm": reading.baro, "battery_percent": reading.bat, "pitch_deg": reading.pitch, "roll_deg": reading.roll, "yaw_deg": reading.yaw,
                        "speed_xyz": [reading.vgx, reading.vgy, reading.vgz], "accel_xyz": [reading.agx, reading.agy, reading.agz], "temp_low_c": reading.templ, "temp_high_c": reading.temph, "motor_time_s": reading.time,
                        "mission_pad": None if reading.pad is None else list(reading.pad), "other_fields": dict(reading.extra)})
        return out

    def _present_or_none(self) -> frames.State | None:
        with self._lock:
            return self._latest

    def _motion_takeoff(self, args: dict) -> dict:
        self._no_arguments("takeoff", args)
        self._ready()
        self._control(frames.takeoff(), self._takeoff_timeout)
        reading = self._present_or_none()
        return {"result": "airborne", "height_cm": None if reading is None else reading.h}

    def _motion_land(self, args: dict) -> dict:
        self._no_arguments("land", args)
        self._present()
        self._control(frames.land(), self._takeoff_timeout)
        return {"result": "landed"}

    def _motion_hover(self, args: dict) -> dict:
        self._no_arguments("hover", args)
        self._present()
        self._control(frames.stop(), self._reply_timeout)
        return {"result": "hovering"}

    def _motion_move(self, args: dict) -> dict:
        directions = [key for key in args if key in frames.DIRECTIONS]
        if len(directions) != 1:
            raise BridgeRefusal(f"`move` takes exactly one of {', '.join(frames.DIRECTIONS)} as `<direction>=<metres>` (from 0.2 to 5), and {'none was given' if not directions else 'several were given'}.", rhal.SKR_EINVAL)
        direction = directions[0]
        metres = self._number(args, direction)
        timeout = self._number(args, "timeout", minimum=1.0, maximum=300.0)
        self._no_arguments("move", args)
        assert metres is not None
        cm = self._whole_units(metres, 100, "a distance")
        text = frames.move(direction, cm)
        self._ready()
        with self._lock:
            pace = self._speed_cm_s or _SLOWEST_CM_S
        self._control(text, timeout if timeout is not None else cm / max(pace, 1) + 5.0)
        return {"result": "arrived", "direction": direction, "cm": cm}

    def _motion_rotate(self, args: dict) -> dict:
        senses = [key for key in args if key in ("cw", "ccw")]
        if len(senses) != 1:
            raise BridgeRefusal("`rotate` takes exactly one of `cw=<degrees>` and `ccw=<degrees>` (from 1 to 360).", rhal.SKR_EINVAL)
        sense = senses[0]
        degrees = self._number(args, sense)
        timeout = self._number(args, "timeout", minimum=1.0, maximum=300.0)
        self._no_arguments("rotate", args)
        assert degrees is not None
        whole = self._whole_units(degrees, 1, "an angle")
        text = frames.rotate(sense == "cw", whole)
        self._ready()
        self._control(text, timeout if timeout is not None else whole / _SLOW_ROTATION_DEG_S + 5.0)
        return {"result": "turned", "sense": sense, "degrees": whole}

    def _motion_speed(self, args: dict) -> dict:
        mps = self._number(args, "v")
        self._no_arguments("speed", args)
        if mps is None:
            raise BridgeRefusal("`speed` needs `v=<metres per second>` (from 0.1 to 1).", rhal.SKR_EINVAL)
        cm_s = self._whole_units(mps, 100, "a speed")
        text = frames.speed(cm_s)
        self._present()
        self._control(text, self._reply_timeout)
        with self._lock:
            self._speed_cm_s = cm_s
        return {"result": "set", "cm_s": cm_s}

    def _motion_drive(self, args: dict) -> dict:
        values = [self._number(args, key, minimum=-100.0, maximum=100.0) for key in ("left", "forward", "up", "yaw")]
        self._no_arguments("drive", args)
        wire = []
        for key, value in zip(("left", "forward", "up", "yaw"), values):
            if value is None:
                wire.append(0)
                continue
            if abs(value - round(value)) > 1e-6:
                raise BridgeRefusal(f"`{key}` of {value:g} is not a whole percent: the Tello takes whole numbers, and a request is not rounded.", rhal.SKR_EINVAL)
            wire.append(int(round(value)))
        text = frames.rc(*wire)
        self._present()
        with self._lock:
            if self._estop:
                raise BridgeRefusal("the stop is latched: nothing was sent.", rhal.SKR_EPERM)
        self._send(text)
        return {"result": "sent", "rc": wire, "note": "the Tello does not answer rc: nothing here shows it was taken"}


def _descriptor(name: str) -> dict:
    if name in ("status", "takeoff", "land", "hover"):
        return {"name": name, "parameters": []}
    if name == "move":
        return {"name": name, "parameters": [{"name": direction, "unit": "m", "required": False, "min": 0.2, "max": 5.0} for direction in frames.DIRECTIONS] + [{"name": "timeout", "unit": "s", "required": False}]}
    if name == "rotate":
        return {"name": name, "parameters": [{"name": "cw", "unit": "deg", "required": False, "min": 1, "max": 360}, {"name": "ccw", "unit": "deg", "required": False, "min": 1, "max": 360}, {"name": "timeout", "unit": "s", "required": False}]}
    if name == "speed":
        return {"name": name, "parameters": [{"name": "v", "unit": "m/s", "required": True, "min": 0.1, "max": 1.0}]}
    return {"name": name, "parameters": [{"name": key, "unit": "%", "required": False, "min": -100, "max": 100} for key in ("left", "forward", "up", "yaw")]}


def _bounded(settings: dict, key: str, default: float, low: float, high: float, *, zero_ok: bool = False) -> float:
    value = settings.get(key)
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not ((zero_ok and value == 0) or low <= value <= high):
        raise BridgeRefusal(f"`{key}` of {value!r} is not a number from {low:g} to {high:g}{' (or 0)' if zero_ok else ''}.")
    return float(value)


def _say(message: str) -> None:
    print(f"skynet-tello: {message}", file=sys.stderr, flush=True)
