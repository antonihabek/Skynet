"""A ROS 2 mobile base as `ros2_control`'s `diff_drive_controller` presents it, written here, for the offline suite.

It is the graph's side of `transport.Transport`: it takes `Twist` or `TwistStamped` commands, moves a body with an acceleration limit, and publishes its odometry. The behaviour the driver
has to be right about is the behaviour of the real controller, read off its documentation (`ros2_controllers`, `diff_drive_controller`) and held to the real one by `live.py`:

* **A command that stops arriving stops the base.** The controller zeroes its target when the newest command is older than `cmd_vel_timeout` (0.5 s by default). A driver that
  published one twist and waited would see the base stop under it, and a driver that published a twist and went away would leave a base that stops itself. `command_timeout_s=None`
  is the other kind of base, the one that holds its last command until it is told otherwise, which is what a driver that dies mid-motion has to be afraid of.
* **A twist of the wrong type is not delivered.** DDS matches a publisher to a subscriber by topic *and type*, so a `Twist` published at a `TwistStamped` subscriber reaches nobody and
  nothing fails: `subscribers()` says 0, and that is the only place it shows.
* **A controller that is not active has no subscriber and publishes nothing.** `diff_drive_controller` is a lifecycle node; until it is activated the topics have no endpoints.

Nothing here is ROS. The suite that uses it shows the driver's logic; `live.py` shows the logic is about the real thing.
"""

from __future__ import annotations

import math
import threading
import time
from dataclasses import dataclass, field

import skynet_rhal as rhal

from .transport import COMMAND_TYPES, TWIST_STAMPED, Odometry, TransportError


@dataclass
class Received:
    """One command the base got, for the suite to read back."""

    at: float
    vx: float
    vy: float
    wz: float
    type: str
    frame_id: str = ""
    #: The thread that published it, so a suite can tell a command said by the caller from one left to a thread started to say it later.
    thread: str = ""


@dataclass
class SimulatedBase:
    command_type: str = TWIST_STAMPED
    rate_hz: float = 50.0
    #: How long a command is believed. `None` is a base that holds its last command until it gets another.
    command_timeout_s: float | None = 0.5
    linear_accel: float = 2.0
    angular_accel: float = 4.0
    holonomic: bool = False
    frame_id: str = "odom"
    child_frame_id: str = "base_link"

    #: The lifecycle: an inactive controller has no endpoints and publishes nothing.
    active: bool = True
    #: Odometry stops arriving while the controller still takes commands (a dead publisher, a partitioned network).
    odometry_silent: bool = False
    #: The wheels stop answering: commands are accepted and the velocity stays what it was.
    stuck: bool = False
    #: A twist on the odometry topic whose frames are wrong, as a misconfigured base publishes: the empty string in `child_frame_id`.
    child_frame_blank: bool = False
    #: The controller has no subscriber on the command topic (a base whose command topic has another name) though its odometry is there.
    listening: bool = True
    #: A second subscriber on the command topic, of this other type (a base and a bridge, both listening).
    also_takes: str | None = None
    #: The subscriber is there and the type matches, and the publisher still never connects: a QoS or domain mismatch.
    unconnectable: bool = False
    #: Publishing raises, as it does when the context has been shut down under the driver.
    fail_publish: bool = False
    #: Publishing takes this long before the command is delivered (a slow graph, a full send queue).
    publish_delay_s: float = 0.0
    #: Commands dropped at random, in [0, 1], from a seeded generator (a lossy link).
    loss: float = 0.0
    seed: int = 1

    received: list[Received] = field(default_factory=list)
    opened: int = 0
    closed: int = 0

    def __post_init__(self) -> None:
        self._lock = threading.Lock()
        self._x = self._y = self._yaw = 0.0
        self._vx = self._vy = self._wz = 0.0
        self._target = (0.0, 0.0, 0.0)
        self._last_command = -math.inf
        self._latest: Odometry | None = None
        self._prepared: str | None = None
        self._thread: threading.Thread | None = None
        self._running = False
        self._random_state = self.seed & 0x7FFFFFFF

    # -- the graph's side ---------------------------------------------------------------------------------------------------------------
    def _random(self) -> float:
        # A linear congruential generator, so a lossy run is the same run every time and nothing here imports `random`'s global state.
        self._random_state = (self._random_state * 1103515245 + 12345) & 0x7FFFFFFF
        return self._random_state / 0x7FFFFFFF

    def start(self) -> None:
        if self._thread is not None:
            return
        self._running = True
        self._thread = threading.Thread(target=self._run, name="sim-base", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._running = False
        thread, self._thread = self._thread, None
        if thread is not None:
            thread.join(timeout=2.0)

    def truth(self) -> tuple[float, float, float, float, float, float]:
        """x, y, yaw, then vx, vy, wz as the body really has them (not as the driver read them)."""
        with self._lock:
            return (self._x, self._y, self._yaw, self._vx, self._vy, self._wz)

    def _run(self) -> None:
        dt = 1.0 / self.rate_hz
        previous = time.monotonic()
        while self._running:
            time.sleep(dt)
            now = time.monotonic()
            step, previous = now - previous, now
            with self._lock:
                target = self._target
                if self.command_timeout_s is not None and now - self._last_command > self.command_timeout_s:
                    target = (0.0, 0.0, 0.0)
                if not self.stuck and self.active:
                    self._vx = _toward(self._vx, target[0], self.linear_accel * step)
                    self._vy = _toward(self._vy, target[1], self.linear_accel * step) if self.holonomic else 0.0
                    self._wz = _toward(self._wz, target[2], self.angular_accel * step)
                    self._x += (self._vx * math.cos(self._yaw) - self._vy * math.sin(self._yaw)) * step
                    self._y += (self._vx * math.sin(self._yaw) + self._vy * math.cos(self._yaw)) * step
                    self._yaw = math.atan2(math.sin(self._yaw + self._wz * step), math.cos(self._yaw + self._wz * step))
                if self.active and not self.odometry_silent:
                    self._latest = Odometry(
                        received_ns=rhal.acquisition_ns(),
                        header_s=now,
                        frame_id=self.frame_id,
                        child_frame_id="" if self.child_frame_blank else self.child_frame_id,
                        position=(self._x, self._y, 0.0),
                        orientation=(0.0, 0.0, math.sin(self._yaw / 2.0), math.cos(self._yaw / 2.0)),
                        linear=(self._vx, self._vy, 0.0),
                        angular=(0.0, 0.0, self._wz),
                    )

    # -- transport.Transport ------------------------------------------------------------------------------------------------------------
    def open(self) -> None:
        self.opened += 1
        # A new connection has received nothing: a sample from the last one is not this one's first.
        with self._lock:
            self._latest = None
        self.start()

    def close(self) -> None:
        self.closed += 1
        self.stop()

    def odometry(self) -> Odometry | None:
        with self._lock:
            return self._latest

    def odometry_publishers(self) -> int:
        return 1 if self.active else 0

    def command_types(self) -> tuple[str, ...]:
        if not self.active or not self.listening:
            return ()
        return tuple(sorted({self.command_type, *([self.also_takes] if self.also_takes else [])}))

    def prepare(self, message_type: str) -> None:
        if message_type not in COMMAND_TYPES:
            raise TransportError(f"{message_type!r} is not a command type this driver publishes ({', '.join(COMMAND_TYPES)}).")
        self._prepared = message_type

    def subscribers(self) -> int:
        if not self.active or not self.listening or self.unconnectable:
            return 0
        return 1 if self._prepared in self.command_types() else 0

    def publish(self, vx: float, vy: float, wz: float) -> None:
        if self._prepared is None:
            raise TransportError("no command publisher: `prepare` was not called.")
        if self.fail_publish:
            raise TransportError("publishing failed (the context is shut down).")
        if self.publish_delay_s:
            time.sleep(self.publish_delay_s)
        now = time.monotonic()
        if self.subscribers() == 0:
            return  # delivered to nobody; DDS says nothing
        if self.loss and self._random() < self.loss:
            return
        with self._lock:
            self.received.append(Received(now, vx, vy, wz, self._prepared, "base_link" if self._prepared == TWIST_STAMPED else "", threading.current_thread().name))
            self._target = (vx, vy, wz)
            self._last_command = now


def _toward(value: float, target: float, step: float) -> float:
    if abs(target - value) <= step:
        return target
    return value + math.copysign(step, target - value)
