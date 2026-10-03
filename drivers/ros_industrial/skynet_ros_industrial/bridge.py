"""The RHAL driver: a robot arm whose controller runs a ROS-Industrial `simple_message` server, over the controller's own wire.

§4's five mandatory entry points over :mod:`skynet_ros_industrial.frames`. It links nothing of the maker's: `industrial_core` is BSD-3-Clause and the wire was read out of its own files, and
`layout_check.py` builds the maker's library and requires every byte this driver makes to be the maker's, and every byte the maker makes to be read the way the maker means it.

# What is on the wire, and what is not

ROS-Industrial's protocol carries a joint trajectory point (ten positions, a velocity *ratio*, a duration) to the controller and a joint feedback and a status back. It carries **no joint
limits, no joint count, no speed ceiling, no model name and no word on whether the controller is a simulation**. Everything a body's descriptor needs and the wire does not say is therefore
the configuration's, and this driver does not guess it:

* `joints`: one entry per joint with `name`, `role` and the joint's `min_rad`, `max_rad` and `max_speed_rad_s`. A joint with no range is a joint the safety kernel cannot bound, and a driver
  that made one up would bound the arm with a number nobody measured (§5.2). The limits belong to the arm, from the maker's own datasheet or description package, and the operator asserts them.
* `simulator`: **no default**. Nothing on the wire says whether the controller is a simulation, and a bench run described as a simulation costs a tier it could have had while a
  simulation described as a bench run is a false claim.
* `byte_order` and `real_bytes`: how the controller's build spells an int and a real (`BYTE_SWAPPING`, `FLOAT64` in the maker's headers). The default is the maker's: little-endian, 4-byte reals.

# What it refuses to bring up

* **A controller that says it is e-stopped, in error, with its drives off or unable to move.** `STATUS` is a tri-state per field (true, false, unknown), and an unknown field is not read as
  healthy or as faulted: it is said aloud, once, and the others are still held to. An error is never cleared here: it is the evidence of why the arm stopped.
* **A controller with no state.** No `JOINT_FEEDBACK` (or the older `JOINT`) and no `STATUS` within the open timeout is a body with no state, and a body with no state is one nothing can be
  stopped by what it says.
* **A motion port that does not answer a ping** (`ping`, on by default; some servers do not answer one, and the configuration can say so).

# What a command does

`write_command` sends one `JOINT_TRAJ_PT` as a service request: the target positions, a velocity ratio and a duration. A trajectory starts at sequence 0 and the next point is the previous plus
one; a stop or a refused point starts a new one. **The driver does the speed arithmetic the maker's own client does** (`JointTrajectoryInterface::calc_velocity`): the ratio is the largest
`|step| / (duration * max_speed)` over the joints, and a step no joint can make in the time it was given is refused before it is sent, never clipped, because the controller would slow the
move and the loop above it believes the arm is where it asked. The replies are read on a thread of their own and counted; a point the controller answered with FAILURE is told to the next
command, which refuses and starts a new trajectory.

# The stop

`emergency_stop` latches and sends `STOP_TRAJECTORY` (the maker's own sequence number for "stop the current motion as soon as possible"), under the motion socket's send lock with a bounded
wait, never on a reply. It is a **software stop**: the controller's emergency-stop input is a circuit, `safety.estop.independent` is a bench test nobody passes from here, and what the
controller does with a stop trajectory is its vendor's (the maker's header says "should").
"""

from __future__ import annotations

import json
import math
import socket
import sys
import threading
import time

import skynet_rhal as rhal

from . import frames

__all__ = ["BridgeRefusal", "RosIndustrialBridge"]

_OPEN_TIMEOUT_S = 3.0
#: How long the send lock may be waited for by a stop. A point is under a hundred bytes and a send of it does not block, so this is a bound and not an expectation.
_STOP_LOCK_S = 0.05
_DEFAULT_PORTS = {"motion": frames.Port.MOTION, "state": frames.Port.STATE}
#: How many points the controller may be behind in answering before the driver says so and stops sending.
_MAX_UNANSWERED = 8
#: The lowest velocity ratio a point is sent with: a step of nothing is a hold, and a ratio of exactly 0 is a number controllers read as "use the default".
_MIN_RATIO = 0.01
_STALE_S = 0.5
#: How long a ping may take to be answered. The maker's own server (`MessageManager::spin`) sleeps five seconds after a client goes before it accepts the next, so a session opened
#: within five seconds of the last one's end is accepted late and pinged late, and a driver that waited the connection's three seconds refused a controller that was merely waiting (F241).
_PING_TIMEOUT_S = 8.0
_KNOWN_KEYS = {"host", "ports", "joints", "simulator", "robot", "byte_order", "real_bytes", "rate_hz", "point_duration_s", "ping", "ping_timeout_ms", "stale_ms", "open_timeout_ms"}


class BridgeRefusal(Exception):
    """A reason this body did not come up, in the terms an integrator acts on."""


class Unreachable(BridgeRefusal):
    """The controller did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""


class _Link:
    """One TCP connection to one of the controller's two servers, read on a thread of its own, with the newest of each message kept."""

    def __init__(self, sock: socket.socket, fmt: frames.Format, name: str) -> None:
        self.sock = sock
        self._fmt = fmt
        self._stream = frames.Stream(fmt)
        #: Guards what the reader thread keeps (`latest`, `replies`); `send_lock` is what a send holds, so a command's bytes are never in the middle of a stop's.
        self.lock = threading.Lock()
        self.send_lock = threading.Lock()
        self.latest: dict[int, tuple[frames.Message, int]] = {}
        self.replies: list[frames.Message] = []
        self.dead: str | None = None
        self._closing = False
        self.thread = threading.Thread(target=self._run, daemon=True, name=f"rosi-{name}")
        self.thread.start()

    def _run(self) -> None:
        self.sock.settimeout(0.25)
        while not self._closing:
            try:
                data = self.sock.recv(4096)
            except TimeoutError:
                continue
            except OSError as error:
                self.dead = f"the socket failed: {error}"
                return
            if not data:
                self.dead = "the controller closed the socket"
                return
            acquired = rhal.acquisition_ns()
            try:
                messages = self._stream.feed(data)
            except frames.ProtocolError as error:
                self.dead = str(error)
                return
            with self.lock:
                for message in messages:
                    if message.comm == frames.Comm.SERVICE_REPLY:
                        self.replies.append(message)
                    else:
                        self.latest[message.type] = (message, acquired)

    def newest(self, *types: int) -> tuple[frames.Message | None, int]:
        """The most recent message of any of these types, and when it arrived."""
        with self.lock:
            found = [self.latest[t] for t in types if t in self.latest]
        if not found:
            return None, 0
        return max(found, key=lambda pair: pair[1])

    def take_replies(self) -> list[frames.Message]:
        with self.lock:
            out, self.replies = self.replies, []
        return out

    def close(self) -> None:
        self._closing = True
        try:
            self.sock.close()
        except OSError:
            pass


class RosIndustrialBridge:
    """A body made of one arm's joints behind a ROS-Industrial server."""

    def __init__(self) -> None:
        self._opened = False
        self._estop = False
        self._descriptor: str | None = None
        self._seq = 0
        self._motion: _Link | None = None
        self._state: _Link | None = None
        self._format = frames.Format()
        self._joints: list[dict] = []
        self._limits: list[tuple[float, float]] = []
        self._speeds: list[float] = []
        self._count = 0
        self._simulated = True
        self._rate_hz = 10.0
        self._duration_s = 0.1
        self._stale_s = _STALE_S
        self._held: list[float] = []
        self._next_point = 0  # the sequence number the next point carries
        self._sent = 0
        self._answered = 0
        self._failed: str | None = None
        self._status_notes: set[str] = set()

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
            _say(f"the controller could not be reached: {error}")
            return rhal.SKR_EIO
        self._opened = True
        return rhal.SKR_OK

    def _open(self, settings: dict) -> None:
        unknown = sorted(set(settings) - _KNOWN_KEYS)
        if unknown:
            raise BridgeRefusal(f"the configuration has {', '.join(map(repr, unknown))}, which this driver does not read. The keys it reads are {', '.join(sorted(_KNOWN_KEYS))}.")
        host = settings.get("host")
        if not isinstance(host, str) or not host:
            raise BridgeRefusal(
                "no `host` in the configuration. This driver does not scan for a controller, and one that guessed an address would command whichever arm answered first. "
                'Give `{"host": "192.168.1.31", "simulator": false, "joints": [{"name": "joint_1", "role": "…", "min_rad": -2.9, "max_rad": 2.9, "max_speed_rad_s": 3.0}, …]}`.')
        simulator = settings.get("simulator")
        if not isinstance(simulator, bool):
            raise BridgeRefusal(
                "`simulator` is not in the configuration, or is not true or false. Nothing a ROS-Industrial server says tells whether the controller is a simulation, and this driver will not "
                "guess: a bench run described as a simulation costs a tier it could have had, and a simulation described as a bench run is a false claim.")
        self._simulated = simulator
        try:
            self._format = frames.Format(byte_order=settings.get("byte_order", "little"), real_bytes=settings.get("real_bytes", 4))
        except ValueError as error:
            raise BridgeRefusal(f"{error}. These are the maker's `BYTE_SWAPPING` and `FLOAT64` builds, and `INT64` is not supported.") from error
        ports = dict(_DEFAULT_PORTS)
        ports.update(settings.get("ports") or {})
        self._load_joints(settings.get("joints"))
        self._rate_hz = _bounded(settings, "rate_hz", 10.0, 0.5, 100.0)
        self._duration_s = _bounded(settings, "point_duration_s", 1.0 / self._rate_hz, 0.01, 10.0)
        self._stale_s = _bounded(settings, "stale_ms", _STALE_S * 1000, 10.0, 30000.0) / 1000.0
        open_s = _bounded(settings, "open_timeout_ms", _OPEN_TIMEOUT_S * 1000, 100.0, 60000.0) / 1000.0
        ping = settings.get("ping", True)
        if not isinstance(ping, bool):
            raise BridgeRefusal(f"`ping` of {ping!r} is not true or false.")
        ping_s = _bounded(settings, "ping_timeout_ms", _PING_TIMEOUT_S * 1000, 50.0, 60000.0) / 1000.0

        self._state = _Link(self._connect(host, int(ports["state"]), "state", open_s), self._format, "state")
        feedback, status = self._first(open_s)
        self._check_status(status)
        self._held = list(feedback[: self._count])
        self._motion = _Link(self._connect(host, int(ports["motion"]), "motion", open_s), self._format, "motion")
        if ping:
            self._ping(ping_s)
        self._seq = 0
        self._next_point = 0
        self._sent = self._answered = 0
        self._failed = None
        self._descriptor = self._build_descriptor(settings.get("robot"))

    def _load_joints(self, joints: object) -> None:
        if not isinstance(joints, list) or not joints:
            raise BridgeRefusal(
                "no `joints` in the configuration. A ROS-Industrial server does not say how many joints the arm has or what any of them is for: a role is a human's assertion (§63.4), and "
                'so is a range. Give `{"joints": [{"name": "joint_1", "role": "…", "min_rad": -2.9, "max_rad": 2.9, "max_speed_rad_s": 3.0}, …]}`.')
        if len(joints) > frames.MAX_JOINTS:
            raise BridgeRefusal(f"the configuration names {len(joints)} joints and a `simple_message` carries {frames.MAX_JOINTS}: the maker's own `JointData::MAX_NUM_JOINTS`.")
        limits, speeds = [], []
        for index, entry in enumerate(joints, start=1):
            if not isinstance(entry, dict) or not entry.get("name"):
                raise BridgeRefusal(f"joint {index} has no `name`; a joint the OS cannot name is one no policy channel can bind to.")
            low, high, speed = entry.get("min_rad"), entry.get("max_rad"), entry.get("max_speed_rad_s")
            for key, value in (("min_rad", low), ("max_rad", high), ("max_speed_rad_s", speed)):
                if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
                    raise BridgeRefusal(
                        f"joint {entry['name']!r} has no usable `{key}`. The protocol carries no limits, so the arm's own go in the configuration, from its maker's datasheet or description "
                        "package: a driver that made one up would bound the arm with a number nobody measured (§5.2).")
            if not low < high:
                raise BridgeRefusal(f"joint {entry['name']!r}: `min_rad` {low} is not below `max_rad` {high}.")
            if speed <= 0:
                raise BridgeRefusal(f"joint {entry['name']!r}: `max_speed_rad_s` {speed} is not above zero.")
            limits.append((float(low), float(high)))
            speeds.append(float(speed))
        self._joints, self._limits, self._speeds, self._count = joints, limits, speeds, len(joints)

    def _connect(self, host: str, port: int, what: str, timeout: float) -> socket.socket:
        try:
            sock = socket.create_connection((host, port), timeout=timeout)
        except OSError as error:
            raise Unreachable(
                f"the {what} server ({host}:{port}) did not answer: {error}. A controller that is off, one on another address and one whose firewall drops the port look the same from here."
            ) from error
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        return sock

    def _first(self, timeout: float) -> tuple[list[float], frames.Status]:
        """The first feedback and the first status, or why there are none."""
        assert self._state is not None
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            feedback = self._feedback()
            status = self._status()
            if feedback is not None and status is not None:
                return feedback[0], status
            if self._state.dead:
                raise Unreachable(f"the state server's stream ended before a feedback and a status arrived: {self._state.dead}")
            time.sleep(0.005)
        raise Unreachable(
            f"no joint feedback and status arrived from the state server within {timeout:.1f} s. A body with no state from its controller is one nothing can be measured or stopped by. "
            "A controller that publishes state only on request, or on a port other than 11002, needs `ports` in the configuration.")

    def _feedback(self) -> tuple[list[float], list[float] | None, int] | None:
        """The newest positions (and velocities, when the controller says they are valid) and when they arrived."""
        assert self._state is not None
        message, stamp = self._state.newest(frames.MsgType.JOINT_FEEDBACK, frames.MsgType.JOINT)
        if message is None:
            return None
        try:
            if message.type == frames.MsgType.JOINT_FEEDBACK:
                feedback = frames.parse_joint_feedback(message.body, self._format)
                if not feedback.valid & frames.Valid.POSITION:
                    return None
                velocities = list(feedback.velocities) if feedback.valid & frames.Valid.VELOCITY else None
                return list(feedback.positions), velocities, stamp
            return list(frames.parse_joint_position(message.body, self._format)[1]), None, stamp
        except frames.ProtocolError:
            return None

    def _status(self) -> frames.Status | None:
        assert self._state is not None
        message, _ = self._state.newest(frames.MsgType.STATUS)
        if message is None:
            return None
        try:
            return frames.parse_robot_status(message.body, self._format)
        except frames.ProtocolError:
            return None

    def _check_status(self, status: frames.Status) -> None:
        T, U = frames.TriState.TRUE, frames.TriState.UNKNOWN
        if status.e_stopped == T:
            raise BridgeRefusal("the controller says it is e-stopped. This body does not come up over an emergency stop: release it at the controller.")
        if status.in_error == T or status.error_code != 0:
            raise BridgeRefusal(
                f"the controller holds error {status.error_code}. This body does not come up over a latched error: clear it at the controller deliberately, after reading why — a driver that "
                "cleared it here would restart a machine whose reason for stopping nobody had looked at.")
        if status.drives_powered == frames.TriState.FALSE:
            raise BridgeRefusal("the controller says its drives are not powered. Power them at the controller: this driver does not.")
        if status.motion_possible == frames.TriState.FALSE:
            raise BridgeRefusal("the controller says motion is not possible (a teach pendant in manual mode, a protective stop, no servo on). This driver does not change that.")
        if status.in_motion == T:
            raise BridgeRefusal("the arm is moving and this session did not move it. Somebody else is commanding the controller, and two masters is two setpoint streams into one arm.")
        unknown = [name for name in ("drives_powered", "e_stopped", "in_error", "in_motion", "motion_possible") if getattr(status, name) == U]
        if unknown:
            _say(f"the controller reports {', '.join(unknown)} as unknown (-1), which is not read as healthy or as faulted. The rest of its status is held to.")

    def _ping(self, timeout: float) -> None:
        assert self._motion is not None
        self._send(self._motion, frames.ping(self._format))
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for reply in self._motion.take_replies():
                if reply.type == frames.MsgType.PING and reply.reply == frames.Reply.SUCCESS:
                    return
            if self._motion.dead:
                raise Unreachable(f"the motion server closed the connection when pinged: {self._motion.dead}")
            time.sleep(0.005)
        raise Unreachable(
            f"the motion server did not answer a ping within {timeout:.1f} s. A server that is not `simple_message` will not, and neither will some that are: set `ping` to false if this "
            "controller's does not answer one. A server that was in a session a moment ago may be slow to accept the next (the maker's own sleeps five seconds): `ping_timeout_ms` is how long to wait.")

    def close(self) -> int:
        if self._motion is not None and self._opened:
            try:
                self._send_stop()
            except (BridgeRefusal, OSError):
                pass  # closing a body whose controller has gone away is still closing it
        self._teardown()
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened or self._state is None:
            return rhal.SKR_EPERM
        self._seq += 1
        out.seq = self._seq
        out.flags = rhal.HOMING_NOT_REQUIRED
        out.fault_code = 0
        newest = self._feedback()
        status = self._status()
        if newest is None:
            return rhal.SKR_ETIMEDOUT
        positions, velocities, stamp = newest
        out.stamp_ns = stamp
        out.position_rad = list(positions[: self._count])
        # NaN, not zero, for what the controller did not say it measured (§24): a velocity the feedback marks invalid, and an effort no `simple_message` carries.
        out.velocity_rad_s = list(velocities[: self._count]) if velocities is not None else [math.nan] * self._count
        out.effort_nm = [math.nan] * self._count
        if self._estop or (status is not None and status.e_stopped == frames.TriState.TRUE):
            out.flags |= rhal.FLAG_ESTOP
        if status is not None and (status.in_error == frames.TriState.TRUE or status.error_code != 0):
            # FAULT for the cause, and DEGRADED because it is the bit §27's `rhal` condition reads (12.C.6, drivers/fault-vocabulary.json).
            out.flags |= rhal.FLAG_FAULT | rhal.FLAG_DEGRADED
            out.fault_code = status.error_code
        age = (rhal.acquisition_ns() - stamp) / 1e9
        if self._state.dead or age > self._stale_s:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened or self._motion is None or self._state is None:
            return rhal.SKR_EPERM
        if cmd.joint_count != self._count:
            return rhal.SKR_EINVAL
        if cmd.mode != rhal.MODE_POSITION:
            # A trajectory point is a position. Velocity and torque are not modes of this protocol, and shipping them anyway is how a torque command becomes a position one.
            return rhal.SKR_EINVAL
        if self._estop:
            return rhal.SKR_EPERM
        status = self._status()
        if status is not None and (status.e_stopped == frames.TriState.TRUE or status.in_error == frames.TriState.TRUE or status.error_code != 0
                                   or status.motion_possible == frames.TriState.FALSE or status.drives_powered == frames.TriState.FALSE):
            return rhal.SKR_EPERM
        if self._motion.dead:
            _say(f"the motion server's connection is gone: {self._motion.dead}")
            return rhal.SKR_EIO
        self._settle_replies()
        if self._failed is not None:
            reason, self._failed = self._failed, None
            self._next_point = 0  # a new trajectory: the controller has a gap in the old one
            _say(reason)
            return rhal.SKR_EIO
        if self._sent - self._answered >= _MAX_UNANSWERED:
            _say(f"the controller has not answered the last {self._sent - self._answered} points. A server that is not answering is not one to queue more behind.")
            return rhal.SKR_EIO

        # Every setpoint is checked before any bytes go out, and nothing is stored until all of them pass (F73's shape).
        base = self._held or [0.0] * self._count
        targets = list(base)
        for index in range(self._count):
            applied = cmd.mask[index] if index < len(cmd.mask) else 1
            if not applied:
                continue
            value = cmd.values[index]
            low, high = self._limits[index]
            if not math.isfinite(value) or not low <= value <= high:
                return rhal.SKR_EINVAL
            targets[index] = value
        ratio = max(abs(target - previous) / (self._duration_s * speed) for target, previous, speed in zip(targets, base, self._speeds))
        if ratio > 1.0:
            return rhal.SKR_EINVAL
        try:
            body = frames.joint_traj_pt(self._next_point, targets, max(ratio, _MIN_RATIO), self._duration_s, self._format)
            self._send(self._motion, frames.encode(frames.MsgType.JOINT_TRAJ_PT, frames.Comm.SERVICE_REQUEST, frames.Reply.INVALID, body, self._format))
        except (frames.ProtocolError, OSError) as error:
            _say(f"the point could not be sent: {error}")
            return rhal.SKR_EIO
        self._next_point += 1
        self._sent += 1
        self._held = targets
        return rhal.SKR_OK

    def _settle_replies(self) -> None:
        """Count the controller's answers, and keep the first failure to tell the next command."""
        assert self._motion is not None
        for reply in self._motion.take_replies():
            if reply.type != frames.MsgType.JOINT_TRAJ_PT:
                continue
            self._answered += 1
            if reply.reply == frames.Reply.FAILURE and self._failed is None:
                self._failed = ("the controller answered a trajectory point with FAILURE: a gap in the sequence, a full queue, or a point it would not take. "
                                "The trajectory is abandoned and the next command starts a new one.")

    def emergency_stop(self) -> int:
        self._estop = True
        try:
            self._send_stop()
        except (BridgeRefusal, OSError):
            pass  # the latch is set; a socket that failed here is one `read_state` will report on
        return rhal.SKR_OK

    def _send_stop(self) -> None:
        link = self._motion
        if link is None:
            return
        # The maker's own client (`JointTrajectoryInterface::trajectoryStop`) sends the stop with every joint at zero. This one sends the arm's own positions, so that a server which read
        # the stop as a point would be sent where it already is and not to the zero pose.
        frame = frames.encode(frames.MsgType.JOINT_TRAJ_PT, frames.Comm.SERVICE_REQUEST, frames.Reply.INVALID,
                              frames.joint_traj_pt(frames.Seq.STOP_TRAJECTORY, list(self._held or [0.0] * max(self._count, 1)), 0.0, 0.0, self._format), self._format)
        if not link.send_lock.acquire(timeout=_STOP_LOCK_S):
            # A send holds this for microseconds. If it is held for fifty milliseconds the socket is blocked, and the only stop left is to drop the connection.
            link.close()
            raise BridgeRefusal("the motion socket was blocked, so the connection was dropped to stop the stream.")
        try:
            link.sock.sendall(frame)
        finally:
            link.send_lock.release()
        self._next_point = 0

    def release_stop(self) -> int:
        """Clear the session latch and say whether the arm can run again (§48.2)."""
        self._estop = False
        if self._state is None or self._motion is None:
            return rhal.SKR_OK
        status = self._status()
        if status is not None and (status.e_stopped == frames.TriState.TRUE or status.in_error == frames.TriState.TRUE or status.error_code != 0):
            self._estop = True
            _say(
                f"the controller holds {'an e-stop' if status.e_stopped == frames.TriState.TRUE else f'error {status.error_code}'}, which a session's stop does not clear. "
                "Read it, clear it at the controller, and open a new session.")
            return rhal.SKR_EIO
        newest = self._feedback()
        if newest is not None:
            self._held = list(newest[0][: self._count])
        self._next_point = 0
        self._failed = None
        return rhal.SKR_OK

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── plumbing ─────────────────────────────────────────────────────────────

    def _send(self, link: _Link, frame: bytes) -> None:
        with link.send_lock:
            link.sock.sendall(frame)

    def _teardown(self) -> None:
        for link in (self._motion, self._state):
            if link is not None:
                link.close()
        self._motion = self._state = None

    def _build_descriptor(self, robot: object) -> str:
        slug = robot if isinstance(robot, str) and robot else f"ros-industrial-{self._count}dof"
        joints = []
        for entry, (low, high), speed in zip(self._joints, self._limits, self._speeds):
            joints.append({
                "name": entry["name"],
                "role": entry.get("role"),
                "type": "revolute",
                "limits": {"position_min_rad": low, "position_max_rad": high, "velocity_max_rad_s": speed},
                "control_modes": ["position"],
            })
        descriptor = {
            "schema_version": 1,
            "robot_slug": slug,
            "digest": None,
            "verified": False,
            "simulator": self._simulated,
            "base": "fixed",
            "dof": self._count,
            "joints": joints,
            "end_effectors": [],
            "sensors": [],
            "control": {"modes": ["position"], "rate_hz_max": self._rate_hz, "rate_hz_min": 0.5},
            # Null, not an empty object: nobody has declared a safety envelope for this body, and a joint range from a configuration is not a risk assessment. No torque rating either:
            # no `simple_message` carries one.
            "envelope": None,
            "capabilities": ["safety.estop"],
            "extensions": [],
            "solvers": [],
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
    print(f"skynet-ros-industrial: {message}", file=sys.stderr, flush=True)
