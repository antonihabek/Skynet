"""A ROS 2 mobile base, over its velocity-command and odometry topics, through `python-host`.

`DRY-COVERAGE-PLAN.md` item 12.E.3: one driver for the class of bases whose ROS 2 interface is the one every wheeled-base stack has settled on, a velocity command in
(`geometry_msgs/Twist`, or `TwistStamped` since Jazzy's `diff_drive_controller`) and `nav_msgs/Odometry` out — Clearpath's Husky and Jackal, TurtleBot 3 and 4, iRobot's Create 3, AgileX,
Hello Robot's Stretch base, PAL's TIAGo base, and anything built on `ros2_control`'s `diff_drive_controller` or `mecanum_drive_controller`. It measures none of them: what it was held against
is `ros2_control`'s own controller on mock hardware (`live.py`), and the rest is the claim that they speak the same two topics.

Like `drivers/pudu` and `drivers/mavlink` the body has no joints the OS can see, so `write_command` refuses and the base is moved by named motions under `vendor.ros2.base` (§84.1):

    status      what the driver sees of the graph: the topics, the message type, who is subscribed, the odometry's age and frames
    velocity    vx, vy, wz for `duration` seconds, streamed; the base is told to stop at the end and the motion returns when it has
    hold        publish zero velocity until the base reads as at rest

Three things about ROS 2 bases shape every line of it, and each is the driver's to be right about because nothing in the graph will say so:

    * **A twist is not a speed, it is a lease.** `diff_drive_controller` zeroes its target when the newest command is older than `cmd_vel_timeout` (0.5 s by default); other bases hold the last command
      until they get another. A velocity is therefore a stream at `command_period_s` for the whole `duration` (so the base never times out on a driver that is alive), and it always ends with a zero
      command, so a base that holds its last command does not keep it. A driver that dies mid-stream leaves the first kind stopped within its timeout and the second kind moving: which kind the
      base is is the operator's to know, and `status` says the driver cannot tell.
    * **A command of the wrong type reaches nobody and fails nothing.** DDS matches by topic and type. Jazzy's controller takes `TwistStamped`, Humble's takes `Twist`, and a publisher of the other
      kind is simply not connected. The driver reads the type off the subscribers the graph reports on the command topic (or is told it, and refuses when the two disagree), creates its
      publisher for that type, and refuses to open until the publisher is connected to someone.
    * **Silence is not stillness.** A base whose odometry has stopped arriving is not a base at rest. The state read says so (`ETIMEDOUT`, DEGRADED) once the newest sample is older than
      `stale_after_s`, a stream in flight is ended and the base told to stop the first period it is true, and "the base is at rest" is only ever claimed from an odometry sample.

The emergency stop publishes a zero twist at once and keeps publishing it every period until the latch is released, because the base may be one that holds its last command, and a stop
that was said once and lost on the link is not a stop. It cannot brake: the wheels follow the controller's own deceleration, and the state says so honestly, as FAULT, if the odometry still
shows motion `stop_settle_s` after a stop was asked for. ROS 2 defines no fault vocabulary for a base, so the fault word stays 0.

Python's tier ceiling is Guest (§31.3); a base reached over DDS from a Python process was never going to be a real-time path.
"""

from __future__ import annotations

import json
import math
import re
import sys
import threading
import time

import skynet_rhal as rhal

from .transport import COMMAND_TYPES, TWIST, TWIST_STAMPED, Odometry, Transport, TransportError

TOKEN = "vendor.ros2.base"

MOTIONS = ("status", "velocity", "hold")

#: The base kinds a twist-commanded body can honestly be, from §97's closed set. The operator says which; the topics do not.
BASE_KINDS = ("wheeled", "tracked")

_MIN_DURATION_S = 0.1
_MAX_DURATION_S = 5.0
_POLL_S = 0.05

_SHORT_TYPES = {"Twist": TWIST, "TwistStamped": TWIST_STAMPED}


class Refusal(Exception):
    """A configuration or request the driver will not act on, with the words it answers in."""

    def __init__(self, message: str, code: int = rhal.SKR_EINVAL) -> None:
        super().__init__(message)
        self.message = message
        self.code = code


def _say(text: str) -> None:
    print(f"skynet-ros2-base: {text}", file=sys.stderr, flush=True)


def _number(value, name: str, *, positive: bool = False):
    """A finite float from a configuration value, or a refusal naming the field."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise Refusal(f"`{name}` must be a number (it is {value!r}).")
    number = float(value)
    if not math.isfinite(number) or (positive and number <= 0.0):
        raise Refusal(f"`{name}` must be a finite number{' above zero' if positive else ''} (it is {value!r}).")
    return number


class Settings:
    """What an operator says about their base, parsed once at open. Anything the topics cannot tell the driver has no default."""

    def __init__(self, config_json: str) -> None:
        try:
            config = json.loads(config_json or "{}")
        except json.JSONDecodeError as error:
            raise Refusal(f"the configuration is not JSON ({error}).") from None
        if not isinstance(config, dict):
            raise Refusal("the configuration must be a JSON object.")
        known = {
            "command_topic", "odometry_topic", "message_type", "max_linear", "max_angular", "holonomic", "base", "simulator", "domain_id", "frame_id",
            "command_period_s", "stale_after_s", "open_wait_s", "stop_settle_s", "rest_tolerance",
        }
        unknown = sorted(set(config) - known)
        if unknown:
            raise Refusal(f"unknown configuration key(s) {unknown}. This driver reads: {', '.join(sorted(known))}.")
        self.command_topic = self._topic(config, "command_topic", "/cmd_vel")
        self.odometry_topic = self._topic(config, "odometry_topic", "/odom")
        for key in ("max_linear", "max_angular"):
            if key not in config:
                raise Refusal(
                    f"no `{key}` in the configuration. How fast this base may be driven is the operator's decision, and the driver has no default for it; "
                    "name a speed the base is safe at in its surroundings (metres per second for max_linear, radians per second for max_angular)."
                )
        self.max_linear = _number(config["max_linear"], "max_linear", positive=True)
        self.max_angular = _number(config["max_angular"], "max_angular", positive=True)
        message = config.get("message_type", "auto")
        if not isinstance(message, str) or (message != "auto" and message not in _SHORT_TYPES):
            raise Refusal(f"`message_type` must be \"auto\", \"Twist\" or \"TwistStamped\" (it is {message!r}).")
        self.message_type = "auto" if message == "auto" else _SHORT_TYPES[message]
        holonomic = config.get("holonomic", False)
        if not isinstance(holonomic, bool):
            raise Refusal(f"`holonomic` must be true or false (it is {holonomic!r}).")
        self.holonomic = holonomic
        base = config.get("base", "wheeled")
        if base not in BASE_KINDS:
            raise Refusal(f"`base` must be one of {', '.join(BASE_KINDS)} (it is {base!r}).")
        self.base = base
        simulator = config.get("simulator", True)
        if not isinstance(simulator, bool):
            raise Refusal(f"`simulator` must be true or false (it is {simulator!r}).")
        self.simulator = simulator
        domain = config.get("domain_id")
        if domain is not None and (isinstance(domain, bool) or not isinstance(domain, int) or not 0 <= domain <= 232):
            raise Refusal(f"`domain_id` must be a whole number from 0 to 232 (it is {domain!r}).")
        self.domain_id = domain
        frame = config.get("frame_id", "base_link")
        if not isinstance(frame, str) or not frame:
            raise Refusal(f"`frame_id` must be text (it is {frame!r}).")
        self.frame_id = frame
        self.command_period_s = _number(config.get("command_period_s", 0.05), "command_period_s", positive=True)
        if self.command_period_s > 0.2:
            raise Refusal(
                f"`command_period_s` is {self.command_period_s} s. A base's own command timeout is commonly 0.5 s (diff_drive_controller's `cmd_vel_timeout`), "
                "and a stream slower than a fifth of that stutters the base: use 0.2 s or faster."
            )
        self.stale_after_s = _number(config.get("stale_after_s", 1.0), "stale_after_s", positive=True)
        self.open_wait_s = _number(config.get("open_wait_s", 5.0), "open_wait_s", positive=True)
        self.stop_settle_s = _number(config.get("stop_settle_s", 3.0), "stop_settle_s", positive=True)
        self.rest_tolerance = _number(config.get("rest_tolerance", 0.01), "rest_tolerance", positive=True)

    @staticmethod
    def _topic(config: dict, key: str, default: str) -> str:
        topic = config.get(key, default)
        if not isinstance(topic, str) or not re.fullmatch(r"/?[A-Za-z0-9_]+(/[A-Za-z0-9_]+)*", topic):
            raise Refusal(f"`{key}` must be a ROS topic name such as {default} (it is {topic!r}).")
        return topic


class Ros2BaseDriver:
    """The RHAL side: no joints, velocity motions under `vendor.ros2.base`, pose and twist from the odometry topic."""

    #: `emergency_stop` is written to be called from another thread while a motion streams: it takes the send lock, latches, publishes zero and starts the keeper, and the stream reads
    #: the latch every period. Saying so is what lets `serve` act on the host's stop while a velocity is running instead of when it returns (F234).
    stop_during_extension = True

    def __init__(self, make_transport) -> None:
        """`make_transport(settings)` builds the graph connection; `__main__` passes the one that talks to ROS, the suite one that does not."""
        self._make_transport = make_transport
        self._lock = threading.Lock()
        #: Held across every publish of a nonzero command and across the stop's latch-and-zero, so that no command asking for motion is ever sent after the stop has latched.
        self._send_lock = threading.Lock()
        self._motion = threading.Lock()
        self._wake = threading.Event()
        self._opened = False
        self._opening = False
        self._estop = False
        self._seq = 0
        self._settings: Settings | None = None
        self._transport: Transport | None = None
        self._message_type = ""
        self._descriptor = ""
        self._commanded = False
        #: When a zero command was last issued as the end of something (a motion, a hold, an emergency stop), and the base has not yet been seen at rest since.
        self._stop_asked_at: float | None = None
        self._keeper: threading.Thread | None = None

    # ── open ──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
    def open(self, config_json: str) -> int:
        # The join waits on the graph for seconds, and `emergency_stop` and `read_state` take the same lock: so the lock is held to claim the open and again to publish it, not across the wait.
        with self._lock:
            if self._opened or self._opening:
                return rhal.SKR_EPERM
            self._opening = True
            # A latch from an earlier session ends here, at the start of this one, so that a stop asked for while the join waits on the graph is kept.
            self._estop = False
        try:
            settings = Settings(config_json)
            transport = self._make_transport(settings)
            message_type = self._join(settings, transport)
        except Refusal as refusal:
            _say(refusal.message)
            return refusal.code
        except TransportError as error:
            _say(str(error))
            return rhal.SKR_EIO
        except Exception as error:  # noqa: BLE001 — whatever ROS raises that this driver did not foresee ends the open with its name, not the process.
            _say(f"opening failed on an unexpected {type(error).__name__}: {error}. This is a defect in the driver or in what ROS raised; the open is refused.")
            return rhal.SKR_EIO
        finally:
            with self._lock:
                self._opening = False
        with self._lock:
            self._settings = settings
            self._transport = transport
            self._message_type = message_type
            self._descriptor = self._build_descriptor(settings)
            self._opened = True
            self._commanded = False
            self._stop_asked_at = None
        return rhal.SKR_OK

    def _join(self, settings: Settings, transport: Transport) -> str:
        """Join the graph and refuse, naming the cause, unless there is a base on the other end of both topics. Returns the command message type."""
        transport.open()
        try:
            deadline = time.monotonic() + settings.open_wait_s
            while time.monotonic() < deadline and transport.odometry() is None:
                time.sleep(_POLL_S)
            first = transport.odometry()
            if first is None:
                publishers = transport.odometry_publishers()
                raise Refusal(
                    f"nothing arrived on {settings.odometry_topic} in {settings.open_wait_s:g} s "
                    f"({'the graph shows a publisher, so check its QoS and ROS_DOMAIN_ID match this process' if publishers else 'the graph shows no publisher on it'}). "
                    "Check the topic name with `ros2 topic list`, that the base's controller is active (diff_drive_controller is a lifecycle node: `ros2 control list_controllers`), "
                    "and that this process is in the same ROS_DOMAIN_ID.",
                    rhal.SKR_EIO,
                )
            if not first.child_frame_id:
                _say(f"{settings.odometry_topic} carries no child_frame_id, so the frame its twist is expressed in is unknown; the driver reports the twist as published.")
            # Each wait has its own budget: a slow odometry must not leave the command topic no time to be looked at.
            deadline = time.monotonic() + settings.open_wait_s
            types = transport.command_types()
            while not types and time.monotonic() < deadline:
                time.sleep(_POLL_S)
                types = transport.command_types()
            message_type = self._choose_type(settings, types)
            transport.prepare(message_type)
            deadline = time.monotonic() + settings.open_wait_s
            while transport.subscribers() < 1 and time.monotonic() < deadline:
                time.sleep(_POLL_S)
            if transport.subscribers() < 1:
                raise Refusal(
                    f"{settings.command_topic} has a subscriber of type {message_type}, but the driver's publisher did not connect to it in {settings.open_wait_s:g} s. "
                    "That is a QoS or ROS_DOMAIN_ID mismatch; compare `ros2 topic info -v` for the topic with this process's environment.",
                    rhal.SKR_EIO,
                )
        except BaseException:
            transport.close()
            raise
        return message_type

    @staticmethod
    def _choose_type(settings: Settings, types: tuple[str, ...]) -> str:
        topic = settings.command_topic
        if not types:
            raise Refusal(
                f"{topic} has no subscriber, so a command would go nowhere and nothing would fail. Check the topic name with `ros2 topic list`, and that the base's controller is active "
                "(diff_drive_controller is a lifecycle node: `ros2 control list_controllers`). Jazzy's controller listens on `<name>/cmd_vel`, not `/cmd_vel`.",
                rhal.SKR_EIO,
            )
        if settings.message_type != "auto":
            if settings.message_type not in types:
                raise Refusal(
                    f"the configuration says {settings.message_type} but the subscriber(s) on {topic} take {', '.join(types)}. A publisher of the other type is not connected to them, "
                    "so no command would arrive: set `message_type` to match, or to \"auto\".",
                    rhal.SKR_EIO,
                )
            return settings.message_type
        usable = [t for t in types if t in COMMAND_TYPES]
        if len(usable) == 1:
            return usable[0]
        if not usable:
            raise Refusal(f"the subscriber(s) on {topic} take {', '.join(types)}, which is not a twist this driver publishes ({', '.join(COMMAND_TYPES)}).", rhal.SKR_EIO)
        raise Refusal(
            f"{topic} has subscribers of both {' and '.join(usable)}. The driver will not guess which one is the base: set `message_type` to the one that is.",
            rhal.SKR_EIO,
        )

    def close(self) -> int:
        with self._lock:
            transport, settings = self._transport, self._settings
            was_open, commanded = self._opened, self._commanded
            self._opened = False
            self._transport = None
        self._wake.set()
        keeper = self._keeper
        if keeper is not None:
            keeper.join(timeout=1.0)
        if transport is not None:
            if was_open and commanded and settings is not None:
                # A session ends with the base told to stop, not following whatever it was last given.
                for _ in range(3):
                    try:
                        transport.publish(0.0, 0.0, 0.0)
                    except TransportError:
                        break
                    time.sleep(min(settings.command_period_s, 0.02))
            transport.close()
        self._wake.clear()
        return rhal.SKR_OK

    # ── read_state ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        with self._lock:
            transport, settings, estop = self._transport, self._settings, self._estop
            asked = self._stop_asked_at
        if transport is None or settings is None:
            return rhal.SKR_EPERM
        self._seq += 1
        out.seq = self._seq
        out.flags = rhal.HOMING_NOT_REQUIRED | (rhal.FLAG_ESTOP if estop else 0)
        out.fault_code = 0
        out.position_rad = []
        out.velocity_rad_s = []
        out.effort_nm = []
        out.base_pose = [math.nan] * 7
        out.base_twist = [0.0] * 6
        odometry = transport.odometry()
        now = rhal.acquisition_ns()
        if odometry is None:
            out.stamp_ns = now
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        out.stamp_ns = odometry.received_ns
        out.base_pose = [*odometry.position, *odometry.orientation]
        out.base_twist = [*odometry.linear, *odometry.angular]
        if (now - odometry.received_ns) / 1e9 > settings.stale_after_s:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        if asked is not None:
            if self._at_rest(odometry, settings):
                with self._lock:
                    if self._stop_asked_at == asked:
                        self._stop_asked_at = None
            elif time.monotonic() - asked > settings.stop_settle_s:
                # A stop was asked for, and the base is still going: the odometry says so, and nothing else in the state does.
                out.flags |= rhal.FLAG_FAULT | rhal.FLAG_DEGRADED
        return rhal.SKR_OK

    @staticmethod
    def _at_rest(odometry: Odometry, settings: Settings) -> bool:
        limit = settings.rest_tolerance
        return math.hypot(odometry.linear[0], odometry.linear[1]) <= limit and abs(odometry.angular[2]) <= limit

    # ── write_command ─────────────────────────────────────────────────────────────────────────────────────────────────────────────────
    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        # The shape before the permission, the way `drivers/pendant`, `drivers/pudu` and `drivers/mavlink` check it.
        if cmd.joint_count != 0:
            return rhal.SKR_EINVAL
        if cmd.mode not in (rhal.MODE_POSITION, rhal.MODE_VELOCITY, rhal.MODE_TORQUE):
            return rhal.SKR_EINVAL
        _say(f"joint commands are refused: this body has no joints, and the only route that moves the base is `extension` under {TOKEN}.")
        return rhal.SKR_EPERM

    # ── the extension: motions ────────────────────────────────────────────────────────────────────────────────────────────────────────
    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        if token != TOKEN:
            return rhal.SKR_ENOSYS, f"unknown token {token!r}. This driver serves {TOKEN}.".encode()
        if not self._opened or self._transport is None or self._settings is None:
            return rhal.SKR_EPERM, b"the driver is not open."
        try:
            words = data.decode("utf-8").split()
        except UnicodeDecodeError:
            return rhal.SKR_EINVAL, b"the request is text: `<motion> k=v k=v` in UTF-8."
        if not words:
            return rhal.SKR_EINVAL, f"empty request. Motions: {', '.join(MOTIONS)}.".encode()
        motion, args = words[0], {}
        for word in words[1:]:
            key, sep, value = word.partition("=")
            if not sep or not key:
                return rhal.SKR_EINVAL, f"malformed argument {word!r}: wanted k=v.".encode()
            args[key] = value
        if motion not in MOTIONS:
            return rhal.SKR_EINVAL, f"unknown motion {motion!r}. Motions: {', '.join(MOTIONS)}.".encode()
        if self._estop and motion != "status":
            return rhal.SKR_EPERM, (
                b"refused: the emergency stop is latched, and the driver is publishing zero velocity until it is released. Only `status` is answered."
            )
        try:
            reply = getattr(self, f"_motion_{motion}")(args)
        except Refusal as refusal:
            return refusal.code, refusal.message.encode()
        except TransportError as error:
            return rhal.SKR_EIO, f"the graph failed: {error}".encode()
        code = reply.pop("_code", rhal.SKR_OK)
        text = reply.pop("_error", None)
        if text is not None:
            return code, text.encode()
        return code, json.dumps(reply, separators=(",", ":")).encode()

    @staticmethod
    def _no_arguments(motion: str, args: dict) -> None:
        if args:
            raise Refusal(f"{motion} takes no arguments (it was given {', '.join(sorted(args))}).")

    def _motion_status(self, args: dict) -> dict:
        self._no_arguments("status", args)
        transport, settings = self._transport, self._settings
        assert transport is not None and settings is not None
        odometry = transport.odometry()
        age = None if odometry is None else round((rhal.acquisition_ns() - odometry.received_ns) / 1e9, 3)
        return {
            "command_topic": settings.command_topic,
            "odometry_topic": settings.odometry_topic,
            "message_type": self._message_type,
            "subscribers": transport.subscribers(),
            "base": settings.base,
            "holonomic": settings.holonomic,
            "max_linear": settings.max_linear,
            "max_angular": settings.max_angular,
            "simulator": settings.simulator,
            "stop_latched": self._estop,
            "odometry_age_s": age,
            "odometry_frame": None if odometry is None else odometry.frame_id,
            "twist_frame": None if odometry is None else odometry.child_frame_id,
            "at_rest": None if odometry is None else self._at_rest(odometry, settings),
            # The one thing a driver cannot read off a ROS graph, and the one a person needs when it dies mid-motion.
            "base_command_timeout": "unknown: a base that zeroes its target after a timeout stops when this driver goes away, one that holds its last command does not",
        }

    def _value(self, args: dict, name: str, limit: float | None, unit: str, *, required: bool = False) -> float:
        if name not in args:
            if required:
                raise Refusal(f"velocity needs {name}= ({unit}).")
            return 0.0
        try:
            number = float(args[name])
        except ValueError:
            raise Refusal(f"{name}={args[name]!r} is not a number.") from None
        if not math.isfinite(number):
            raise Refusal(f"{name}={args[name]!r} is not a finite number.")
        if limit is not None and abs(number) > limit:
            raise Refusal(
                f"{name}={number:g} {unit} is past the {limit:g} {unit} this base was configured to be driven at "
                f"(`max_{'angular' if name == 'wz' else 'linear'}` in the configuration). The driver refuses rather than clipping, so the base never does something other than what was asked."
            )
        return number

    def _motion_velocity(self, args: dict) -> dict:
        settings, transport = self._settings, self._transport
        assert settings is not None and transport is not None
        unknown = sorted(set(args) - {"vx", "vy", "wz", "duration"})
        if unknown:
            raise Refusal(f"velocity does not take {', '.join(unknown)}. It takes vx, vy, wz and duration.")
        vx = self._value(args, "vx", settings.max_linear, "m/s")
        vy = self._value(args, "vy", settings.max_linear, "m/s")
        wz = self._value(args, "wz", settings.max_angular, "rad/s")
        if vy != 0.0 and not settings.holonomic:
            raise Refusal("vy is refused: this base is configured as non-holonomic (`holonomic` is false), and a differential or Ackermann base cannot move sideways. Set `holonomic` true for a mecanum or omni base.")
        duration = self._value(args, "duration", None, "s", required=True)
        if not _MIN_DURATION_S <= duration <= _MAX_DURATION_S:
            raise Refusal(
                f"duration={duration:g} s is outside {_MIN_DURATION_S:g} to {_MAX_DURATION_S:g} s. A velocity is a stream the driver keeps up for exactly this long and then stops; "
                "a longer motion is a run of them, so that the stop is never more than a few seconds away from the person who asked for it."
            )
        if not self._motion.acquire(blocking=False):
            raise Refusal("another motion is still running; wait for it to return, or stop the base.", rhal.SKR_EPERM)
        try:
            return self._stream(vx, vy, wz, duration)
        finally:
            self._motion.release()

    def _motion_hold(self, args: dict) -> dict:
        self._no_arguments("hold", args)
        if not self._motion.acquire(blocking=False):
            raise Refusal("another motion is still running; wait for it to return, or stop the base.", rhal.SKR_EPERM)
        try:
            settings, transport = self._settings, self._transport
            assert settings is not None and transport is not None
            self._commanded = True
            return self._to_rest("hold", started=time.monotonic(), commands=0, start=self._fresh(), settings=settings, transport=transport)
        finally:
            self._motion.release()

    def _fresh(self) -> Odometry:
        """The newest odometry, refused unless it is recent: a base is never moved on a picture of it that is old."""
        transport, settings = self._transport, self._settings
        assert transport is not None and settings is not None
        odometry = transport.odometry()
        if odometry is None or (rhal.acquisition_ns() - odometry.received_ns) / 1e9 > settings.stale_after_s:
            age = "none has arrived" if odometry is None else f"the newest is {(rhal.acquisition_ns() - odometry.received_ns) / 1e9:.1f} s old"
            raise Refusal(
                f"refused: {settings.odometry_topic} is not current ({age}; `stale_after_s` is {settings.stale_after_s:g}). A base that cannot be seen is not moved. "
                "Check that its controller is still active, then ask again.",
                rhal.SKR_EIO,
            )
        return odometry

    def _stream(self, vx: float, vy: float, wz: float, duration: float) -> dict:
        settings, transport = self._settings, self._transport
        assert settings is not None and transport is not None
        start = self._fresh()
        self._commanded = True
        with self._lock:
            self._stop_asked_at = None
        self._wake.clear()
        begin = time.monotonic()
        end = begin + duration
        commands = 0
        ended = "completed"
        cause = ""
        tick = begin
        try:
            while True:
                now = time.monotonic()
                if self._estop:
                    ended, cause = "stopped", "the emergency stop was latched"
                    break
                if now >= end:
                    break
                newest = transport.odometry()
                if newest is None or (rhal.acquisition_ns() - newest.received_ns) / 1e9 > settings.stale_after_s:
                    ended, cause = "blind", f"{settings.odometry_topic} stopped arriving"
                    break
                if not self._send(vx, vy, wz):
                    ended, cause = "stopped", "the emergency stop was latched"
                    break
                commands += 1
                tick += settings.command_period_s
                self._wake.wait(max(0.0, min(tick, end) - time.monotonic()))
        except TransportError as error:
            ended, cause = "failed", str(error)
        reply = self._to_rest("velocity", started=begin, commands=commands, start=start, settings=settings, transport=transport)
        reply["requested"] = {"vx": vx, "vy": vy, "wz": wz, "duration": duration}
        if ended != "completed":
            return {
                "_code": rhal.SKR_EPERM if ended == "stopped" else rhal.SKR_EIO,
                "_error": f"the velocity ended early after {time.monotonic() - begin:.2f} s and {commands} command(s): {cause}. "
                + (reply.get("_error") or "The base was told to stop and the odometry shows it at rest."),
            }
        return reply

    def _send(self, vx: float, vy: float, wz: float) -> bool:
        """Publish a command that asks for motion, unless the emergency stop has latched. The check and the publish are one step against the stop's own (`emergency_stop`)."""
        with self._send_lock:
            if self._estop:
                return False
            transport = self._transport
            if transport is None:
                raise TransportError("the driver was closed under the motion.")
            transport.publish(vx, vy, wz)
            return True

    def _to_rest(self, motion: str, *, started: float, commands: int, start: Odometry, settings: Settings, transport: Transport) -> dict:
        """Tell the base to stop, keep telling it each period, and return when the odometry shows it at rest or `stop_settle_s` has gone by.

        The connection is the caller's, taken when the motion began: a driver closed under the motion still finishes its answer.
        """
        asked = time.monotonic()
        with self._lock:
            self._stop_asked_at = asked
        deadline = asked + settings.stop_settle_s
        zeroes = 0
        failure = ""
        at_rest = False
        newest: Odometry | None = None
        while True:
            try:
                transport.publish(0.0, 0.0, 0.0)
                zeroes += 1
            except TransportError as error:
                failure = str(error)
            newest = transport.odometry()
            if newest is not None and (rhal.acquisition_ns() - newest.received_ns) / 1e9 <= settings.stale_after_s and self._at_rest(newest, settings):
                at_rest = True
                break
            if time.monotonic() >= deadline:
                break
            # Not `_wake`: a stop latched mid-motion has set it, and the zero command must not be published as fast as the loop can spin.
            time.sleep(settings.command_period_s)
        if at_rest:
            with self._lock:
                if self._stop_asked_at == asked:
                    self._stop_asked_at = None
        reply = {
            "motion": motion,
            "at_rest": at_rest,
            "commands": commands,
            "zero_commands": zeroes,
            "elapsed_s": round(time.monotonic() - started, 3),
        }
        if newest is not None:
            reply["displacement"] = [round(newest.position[0] - start.position[0], 4), round(newest.position[1] - start.position[1], 4)]
            reply["yaw_change"] = round(_wrap(_yaw(newest.orientation) - _yaw(start.orientation)), 4)
            reply["final_twist"] = [round(newest.linear[0], 4), round(newest.linear[1], 4), round(newest.angular[2], 4)]
        if failure:
            reply["_code"] = rhal.SKR_EIO
            reply["_error"] = f"the zero command did not go: {failure}. The base was not seen to stop."
        elif not at_rest:
            reply["_code"] = rhal.SKR_EIO
            reply["_error"] = (
                f"the base did not come to rest within {settings.stop_settle_s:g} s of being told to stop"
                + ("" if newest is None else f" (its odometry still shows {math.hypot(newest.linear[0], newest.linear[1]):.3f} m/s and {abs(newest.angular[2]):.3f} rad/s)")
                + ". The driver keeps no latch for this; the state reads FAULT until the odometry shows it at rest. Stop the base by its own means."
            )
        return reply

    # ── emergency_stop ────────────────────────────────────────────────────────────────────────────────────────────────────────────────
    def emergency_stop(self) -> int:
        """Latch, publish zero at once, and keep publishing it until the latch is released. Never waits behind a motion in flight: the motion's own stream reads the latch."""
        keeper = None
        with self._send_lock:
            with self._lock:
                self._estop = True
                self._stop_asked_at = time.monotonic()
                transport = self._transport
                if transport is not None and self._keeper is None:
                    keeper = self._keeper = threading.Thread(target=self._keep_stopped, name="skynet-ros2-base-stop", daemon=True)
            self._wake.set()
            if transport is not None:
                try:
                    transport.publish(0.0, 0.0, 0.0)
                except TransportError:
                    pass  # the keeper below keeps trying, and the state does not call the base stopped until the odometry does
        if keeper is not None:
            keeper.start()
        return rhal.SKR_OK

    def _keep_stopped(self) -> None:
        while True:
            with self._lock:
                transport, settings, latched = self._transport, self._settings, self._estop
                if not latched or transport is None or settings is None:
                    # Leaves under the lock that `emergency_stop` starts the next one under, so a stop asked for just after a release always has a keeper.
                    if self._keeper is threading.current_thread():
                        self._keeper = None
                    return
            try:
                transport.publish(0.0, 0.0, 0.0)
            except TransportError:
                pass
            time.sleep(settings.command_period_s)

    def release_stop(self) -> int:
        """Clear the session latch (§48.2). The base is left where it is, stopped; nothing is resumed."""
        with self._lock:
            self._estop = False
        self._wake.clear()
        return rhal.SKR_OK

    # ── describe ──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    @staticmethod
    def _build_descriptor(settings: Settings) -> str:
        slug = re.sub(r"[^a-z0-9]+", "-", settings.command_topic.lower()).strip("-")
        # `cmd_vel` is the verb, and `cmd_vel_unstamped` is the same verb on Humble's controller, which takes a plain Twist there.
        slug = re.sub(r"-?cmd-vel(-unstamped)?$", "", slug).strip("-") or "base"
        parameters = [
            {"name": "vx", "unit": "m/s", "required": False, "min": -settings.max_linear, "max": settings.max_linear},
            {"name": "wz", "unit": "rad/s", "required": False, "min": -settings.max_angular, "max": settings.max_angular},
            {"name": "duration", "unit": "s", "required": True, "min": _MIN_DURATION_S, "max": _MAX_DURATION_S},
        ]
        if settings.holonomic:
            parameters.insert(1, {"name": "vy", "unit": "m/s", "required": False, "min": -settings.max_linear, "max": settings.max_linear})
        return json.dumps(
            {
                "schema_version": 1,
                "robot_slug": f"ros2-base-{slug}",
                "digest": None,
                "verified": False,
                "simulator": settings.simulator,
                "base": settings.base,
                "dof": 0,
                "joints": [],
                "end_effectors": [],
                "sensors": [],
                "control": {"modes": [], "rate_hz_max": None, "rate_hz_min": None, "command_latency_ms": None},
                # No envelope: the speeds are the operator's, not the base's rating, and the envelope is not where a guess goes.
                "envelope": None,
                "capabilities": [],
                "motions": [
                    {"name": "status", "parameters": []},
                    {"name": "velocity", "parameters": parameters},
                    {"name": "hold", "parameters": []},
                ],
                "extensions": [TOKEN],
                "solvers": [],
            }
        )


def _wrap(angle: float) -> float:
    """An angle difference as the turn that was made, in (-pi, pi]: two headings either side of the half turn are a small turn and not a whole circle's worth the other way."""
    return math.atan2(math.sin(angle), math.cos(angle))


def _yaw(quaternion: tuple[float, float, float, float]) -> float:
    x, y, z, w = quaternion
    return math.atan2(2.0 * (w * z + x * y), 1.0 - 2.0 * (y * y + z * z))
