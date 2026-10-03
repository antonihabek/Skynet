"""A simulated controller with a ROS-Industrial server on it, written here: the dry substrate for `selftest.py` and for `skynet driver check`.

It is what a controller's `simple_message` servers do, as far as the protocol's own documentation says and no further (`industrial_robot_client`'s streamer, the maker's `JointTrajPt` and
`RobotStatus` headers), and every behaviour that is *this file's choice* is marked so, because a simulator that is more specific than the maker is a simulator that agrees with its author:

* **The motion port** takes a `JOINT_TRAJ_PT` as a service request and answers with a service reply. A point with sequence 0 starts a trajectory, the next point is the previous plus one, and
  a stop (`STOP_TRAJECTORY`) ends it and flushes what is queued. Any other sequence number is a FAILURE (*this file's choice*: the maker's header says
  only that a server "should" answer, and what a real controller's server does is its vendor's). A full queue is a FAILURE too (`queue_depth`, *this file's choice*: the streamer retries a point that failed).
* **The state port** pushes a `JOINT_FEEDBACK` at `feedback_hz` and a `STATUS` at `status_hz` to whoever connects, with no request.
* **The motion** is a straight line in joint space from where the arm is to each queued point in turn, taking the longer of the point's `duration` and what its velocity ratio (a fraction of
  each joint's own maximum, `max_speed`) allows. A point outside a joint's range is **acknowledged and then the controller faults**: it stops, flushes, raises an error and refuses further
  points until `clear_error()`. That is the behaviour the maker's own xArm sibling showed (F175) and the reason the driver checks a range before it sends.

It is not a model of any one robot's controller. `fault()`, `estop()`, `drives(False)` and `silence()` are how a test makes it behave the way the real ones do when something is wrong.
"""

from __future__ import annotations

import socket
import threading
import time

from . import frames

__all__ = ["SimulatedController"]

#: A six-joint arm's ranges and speed ceilings, *this file's choice*: every real controller reports none of this over simple_message, so the driver is told them in its configuration.
SIX_JOINT_LIMITS = [(-2.9, 2.9), (-1.9, 1.9), (-2.5, 2.5), (-3.1, 3.1), (-2.0, 2.0), (-6.2, 6.2)]
SIX_JOINT_MAX_SPEED = 3.0


class SimulatedController:
    def __init__(self, joints: int = 6, fmt: frames.Format = frames.DEFAULT_FORMAT, feedback_hz: float = 50.0, status_hz: float = 10.0, queue_depth: int = 8,
                 limits: list[tuple[float, float]] | None = None, max_speed: float = SIX_JOINT_MAX_SPEED) -> None:
        self.format = fmt
        self.joints = joints
        self.feedback_hz = feedback_hz
        self.status_hz = status_hz
        self.queue_depth = queue_depth
        self.limits = limits or SIX_JOINT_LIMITS[:joints]
        self.max_speed = max_speed
        self.ports = {"motion": 0, "state": 0}
        self._lock = threading.Lock()
        self._positions = [0.0] * joints
        self._velocities = [0.0] * joints
        self._queue: list[frames.TrajPt] = []
        self._expected_seq = 0
        self._segment: tuple[float, list[float], list[float]] | None = None  # (started, from, to); its length is `_segment_s`
        self._segment_s = 0.0
        self._error_code = 0
        self._e_stopped = False
        self._drives = True
        self._mode = frames.RobotMode.AUTO
        self._silent = False
        #: What a test turns on: a motion server that does not answer a ping, status fields a controller says it does not know (-1), and a controller old enough to send `JOINT` (positions only).
        self.answer_ping = True
        self.ping_delay_s = 0.0
        self.unknown: set[str] = set()
        self.older_message = False
        #: Which fields of a feedback message the controller says are valid (`None`: time, position and velocity), whether its status is sent at all, whether an error code comes with
        #: the `in_error` flag, and what a ping is answered with.
        self.feedback_valid: int | None = None
        self.status_silent = False
        self.error_flag = True
        #: `None`: motion is possible unless an error, an e-stop or drives that are off say otherwise; a bool is what the controller reports whatever else is true (a pendant in manual).
        self.motion_possible: bool | None = None
        self.ping_reply = frames.Reply.SUCCESS
        self._stopping = False
        self._servers: list[socket.socket] = []
        self._state_clients: list[socket.socket] = []
        self._motion_clients: list[socket.socket] = []
        #: Everything the motion port received and answered, for a test to read.
        self.received: list[frames.Message] = []
        self.points: list[frames.TrajPt] = []
        self.pings = 0
        self.stops = 0
        self.replies_withheld = 0

    def __enter__(self) -> SimulatedController:
        return self.start()

    def __exit__(self, *exc: object) -> None:
        self.stop()

    # ── what a test does to it ───────────────────────────────────────────────

    def start(self) -> SimulatedController:
        for name in ("motion", "state"):
            server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            server.bind(("127.0.0.1", 0))
            server.listen(4)
            self.ports[name] = server.getsockname()[1]
            self._servers.append(server)
            threading.Thread(target=self._accept, args=(server, name), daemon=True, name=f"rosi-sim-{name}").start()
        threading.Thread(target=self._physics, daemon=True, name="rosi-sim-physics").start()
        threading.Thread(target=self._publish, daemon=True, name="rosi-sim-state").start()
        return self

    def stop(self) -> None:
        self._stopping = True
        for sock in self._servers + self._state_clients + self._motion_clients:
            try:
                sock.close()
            except OSError:
                pass

    def place(self, positions: list[float]) -> None:
        with self._lock:
            self._positions = list(positions) + [0.0] * (self.joints - len(positions))

    def positions(self) -> list[float]:
        with self._lock:
            return list(self._positions)

    def fault(self, code: int) -> None:
        with self._lock:
            self._error_code = code
            self._flush()

    def clear_error(self) -> None:
        with self._lock:
            self._error_code = 0

    def estop(self, on: bool = True) -> None:
        with self._lock:
            self._e_stopped = on
            if on:
                self._flush()

    def drives(self, on: bool) -> None:
        with self._lock:
            self._drives = on
            if not on:
                self._flush()

    def silence(self, on: bool = True) -> None:
        """Stop publishing state (a controller that has gone quiet, or a cable that came out of the state port only)."""
        self._silent = on

    def in_motion(self) -> bool:
        with self._lock:
            return self._segment is not None or bool(self._queue)

    def drop_motion_clients(self) -> None:
        for sock in self._motion_clients:
            try:
                sock.close()
            except OSError:
                pass
        self._motion_clients = []

    # ── the two servers ──────────────────────────────────────────────────────

    def _accept(self, server: socket.socket, name: str) -> None:
        while not self._stopping:
            try:
                client, _ = server.accept()
            except OSError:
                return
            client.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            if name == "state":
                self._state_clients.append(client)
            else:
                self._motion_clients.append(client)
                threading.Thread(target=self._serve_motion, args=(client,), daemon=True, name="rosi-sim-motion-client").start()

    def _serve_motion(self, client: socket.socket) -> None:
        stream = frames.Stream(self.format)
        while not self._stopping:
            try:
                data = client.recv(4096)
            except OSError:
                return
            if not data:
                return
            try:
                messages = stream.feed(data)
            except frames.ProtocolError:
                client.close()
                return
            for message in messages:
                self.received.append(message)
                reply = self._handle(message)
                if reply is None:
                    continue
                try:
                    client.sendall(reply)
                except OSError:
                    return

    def _handle(self, message: frames.Message) -> bytes | None:
        fmt = self.format
        if message.type == frames.MsgType.PING and message.comm == frames.Comm.SERVICE_REQUEST:
            self.pings += 1
            if not self.answer_ping:
                return None
            if self.ping_delay_s:
                time.sleep(self.ping_delay_s)
            return frames.encode(frames.MsgType.PING, frames.Comm.SERVICE_REPLY, self.ping_reply, b"", fmt)
        if message.type != frames.MsgType.JOINT_TRAJ_PT or message.comm != frames.Comm.SERVICE_REQUEST:
            return None
        try:
            point = frames.parse_joint_traj_pt(message.body, fmt)
        except frames.ProtocolError:
            return self._reply(frames.Reply.FAILURE)
        self.points.append(point)
        if self.replies_withheld > 0:
            self.replies_withheld -= 1
            return None
        with self._lock:
            if point.seq == frames.Seq.STOP_TRAJECTORY:
                self.stops += 1
                self._flush()
                return self._reply(frames.Reply.SUCCESS)
            if self._error_code or self._e_stopped or not self._drives or self._mode != frames.RobotMode.AUTO:
                return self._reply(frames.Reply.FAILURE)
            if point.seq == 0:
                self._queue = []
                self._segment = None
                self._expected_seq = 0
            if point.seq != self._expected_seq or len(self._queue) >= self.queue_depth:
                return self._reply(frames.Reply.FAILURE)
            self._queue.append(point)
            self._expected_seq += 1
            return self._reply(frames.Reply.SUCCESS)

    def _reply(self, code: int) -> bytes:
        return frames.encode(frames.MsgType.JOINT_TRAJ_PT, frames.Comm.SERVICE_REPLY, code, b"", self.format)

    def _flush(self) -> None:
        self._queue = []
        self._segment = None
        self._velocities = [0.0] * self.joints
        self._expected_seq = 0

    # ── the arm ──────────────────────────────────────────────────────────────

    def _physics(self) -> None:
        while not self._stopping:
            time.sleep(0.005)
            now = time.monotonic()
            with self._lock:
                if self._segment is None and self._queue:
                    self._start_next(now)
                if self._segment is None:
                    self._velocities = [0.0] * self.joints
                    continue
                started, origin, target = self._segment
                done = min(1.0, (now - started) / self._segment_s) if self._segment_s > 0 else 1.0
                previous = list(self._positions)
                self._positions = [a + (b - a) * done for a, b in zip(origin, target)]
                span = max(now - getattr(self, "_last", now - 0.005), 1e-3)
                self._velocities = [(p - q) / span for p, q in zip(self._positions, previous)]
                self._last = now
                if done >= 1.0:
                    self._segment = None

    def _start_next(self, now: float) -> None:
        point = self._queue.pop(0)
        target = list(point.joints[: self.joints])
        for value, (low, high) in zip(target, self.limits):
            if not low <= value <= high:
                # Acknowledged already; the fault comes when the controller tries to go there.
                self._error_code = 4021
                self._flush()
                return
        origin = list(self._positions)
        travel = max((abs(b - a) / (max(point.velocity, 1e-3) * self.max_speed) for a, b in zip(origin, target)), default=0.0)
        self._segment_s = max(point.duration, travel)
        self._segment = (now, origin, target)

    # ── what it publishes ────────────────────────────────────────────────────

    def _publish(self) -> None:
        last_status = 0.0
        while not self._stopping:
            time.sleep(1.0 / self.feedback_hz)
            if self._silent:
                continue
            with self._lock:
                positions, velocities = list(self._positions), list(self._velocities)
                status = frames.Status(
                    frames.TriState.TRUE if self._drives else frames.TriState.FALSE,
                    frames.TriState.TRUE if self._e_stopped else frames.TriState.FALSE,
                    self._error_code,
                    frames.TriState.TRUE if (self._error_code and self.error_flag) else frames.TriState.FALSE,
                    frames.TriState.TRUE if (self._segment is not None or self._queue) else frames.TriState.FALSE,
                    self._mode,
                    (frames.TriState.TRUE if self.motion_possible else frames.TriState.FALSE) if self.motion_possible is not None
                    else frames.TriState.FALSE if (self._error_code or self._e_stopped or not self._drives) else frames.TriState.TRUE)
            fmt = self.format
            if self.older_message:
                out = [frames.encode(frames.MsgType.JOINT, frames.Comm.TOPIC, frames.Reply.INVALID, frames.joint_position(0, positions, fmt), fmt)]
            else:
                out = [frames.encode(frames.MsgType.JOINT_FEEDBACK, frames.Comm.TOPIC, frames.Reply.INVALID,
                                     frames.joint_feedback(0, self.feedback_valid if self.feedback_valid is not None else frames.Valid.TIME | frames.Valid.POSITION | frames.Valid.VELOCITY,
                                                             time.monotonic() % 1e6, positions, velocities, [0.0] * self.joints, fmt), fmt)]
            now = time.monotonic()
            if not self.status_silent and now - last_status >= 1.0 / self.status_hz:
                last_status = now
                status = frames.Status(*(frames.TriState.UNKNOWN if name in self.unknown else getattr(status, name) for name in frames.Status.__dataclass_fields__))
                out.append(frames.encode(frames.MsgType.STATUS, frames.Comm.TOPIC, frames.Reply.INVALID, frames.robot_status(status, fmt), fmt))
            for client in list(self._state_clients):
                try:
                    client.sendall(b"".join(out))
                except OSError:
                    self._state_clients.remove(client)

