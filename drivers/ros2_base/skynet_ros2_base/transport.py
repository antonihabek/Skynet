"""What the ROS 2 base driver needs of a ROS graph, and the one implementation of that which talks to a real one.

A ROS 2 mobile base is, to a driver, two topics: velocity commands go in as `geometry_msgs/msg/Twist` (or, since Jazzy's `diff_drive_controller`, `TwistStamped`) and
`nav_msgs/msg/Odometry` comes out. Everything the driver does is one of: read the newest odometry, say how old it is, publish a twist, and ask the graph who is on the
other end of the command topic. Those five things are `Transport`, so the driver's logic runs against the real graph (`RclpyTransport`, below) and against the
simulated one written for the offline suite (`sim.py`) without a branch in it.

`RclpyTransport` is the only module here that imports ROS. It is imported when the driver opens, so a machine without ROS gets one error that says what to source, not a traceback
at import time, and the offline suite never needs ROS at all.
"""

from __future__ import annotations

import threading
from dataclasses import dataclass
from typing import Protocol

import skynet_rhal as rhal

TWIST = "geometry_msgs/msg/Twist"
TWIST_STAMPED = "geometry_msgs/msg/TwistStamped"
COMMAND_TYPES = (TWIST, TWIST_STAMPED)

#: The odometry subscription asks for best effort: a best-effort subscriber matches a publisher of either reliability, a reliable one matches only a reliable publisher, and
#: `diff_drive_controller` and most bases publish with the system default (reliable) while some of the vendors' publish sensor-data QoS (best effort). The command publisher is
#: the other way round, reliable, because a reliable publisher matches a subscriber of either reliability and a best-effort one matches only best-effort subscribers.
_ODOMETRY_DEPTH = 5
_COMMAND_DEPTH = 10


class TransportError(Exception):
    """The graph could not be reached, in words that name the cause and the next action."""


@dataclass(frozen=True)
class Odometry:
    """One `nav_msgs/msg/Odometry`, as the driver reads it: the position and orientation in `frame_id`, the twist in `child_frame_id`."""

    #: `rhal.acquisition_ns()` when this process received it. ROS time is the base's (or a simulator's) and can be anything; this is the clock the OS's budgets are in.
    received_ns: int
    #: The message's own header stamp, seconds on the sender's clock; only ever compared with the previous one's, to see that the sender's clock advances.
    header_s: float
    frame_id: str
    child_frame_id: str
    position: tuple[float, float, float]
    #: x, y, z, w — ROS's order, which is also the order of the RHAL state's `qx qy qz qw`.
    orientation: tuple[float, float, float, float]
    linear: tuple[float, float, float]
    angular: tuple[float, float, float]


class Transport(Protocol):
    def open(self) -> None:
        """Join the graph. Raises `TransportError` when it cannot."""

    def close(self) -> None: ...

    def odometry(self) -> Odometry | None:
        """The newest odometry received, or `None` if none has arrived."""

    def odometry_publishers(self) -> int:
        """How many publishers the graph says there are on the odometry topic."""

    def command_types(self) -> tuple[str, ...]:
        """The message types of the subscribers the graph says are on the command topic, each once, sorted. Empty when nothing is subscribed."""

    def prepare(self, message_type: str) -> None:
        """Create the command publisher for this type. Raises `TransportError` for a type that is not one of `COMMAND_TYPES`."""

    def subscribers(self) -> int:
        """How many subscribers the prepared publisher is connected to (a type or QoS mismatch is a subscriber that is not counted)."""

    def publish(self, vx: float, vy: float, wz: float) -> None:
        """Publish one command of the prepared type. Raises `TransportError` when the publish fails."""


class RclpyTransport:
    """A node in the ROS 2 graph, spun on a thread of its own."""

    def __init__(self, command_topic: str, odometry_topic: str, *, domain_id: int | None = None, frame_id: str = "base_link") -> None:
        self._command_topic = command_topic
        self._odometry_topic = odometry_topic
        self._domain_id = domain_id
        self._frame_id = frame_id
        self._lock = threading.Lock()
        self._latest: Odometry | None = None
        self._context = None
        self._node = None
        self._executor = None
        self._thread: threading.Thread | None = None
        self._publisher = None
        self._type = ""
        self._messages: dict = {}

    def open(self) -> None:
        try:
            import rclpy
            from geometry_msgs.msg import Twist, TwistStamped
            from nav_msgs.msg import Odometry as OdometryMessage
            from rclpy.context import Context
            from rclpy.executors import SingleThreadedExecutor
            from rclpy.qos import DurabilityPolicy, HistoryPolicy, QoSProfile, ReliabilityPolicy
        except ImportError as error:
            raise TransportError(
                f"ROS 2's Python client (rclpy) cannot be imported here ({error}). Source the ROS 2 install before the driver starts, "
                "for example `. /opt/ros/jazzy/setup.sh`, so that rclpy and the message packages geometry_msgs and nav_msgs are on this interpreter's path."
            ) from error
        self._messages = {TWIST: Twist, TWIST_STAMPED: TwistStamped}
        context = Context()
        try:
            rclpy.init(args=None, context=context, domain_id=self._domain_id)
            node = rclpy.create_node(f"skynet_ros2_base_{id(self) & 0xFFFF:x}", context=context)
        except Exception as error:  # noqa: BLE001 — rclpy raises its own types for a bad domain id or a missing RMW.
            raise TransportError(f"ROS 2 would not start a node ({error}). Check ROS_DOMAIN_ID (0 to 232) and RMW_IMPLEMENTATION.") from error
        qos = QoSProfile(
            reliability=ReliabilityPolicy.BEST_EFFORT, durability=DurabilityPolicy.VOLATILE, history=HistoryPolicy.KEEP_LAST, depth=_ODOMETRY_DEPTH
        )
        node.create_subscription(OdometryMessage, self._odometry_topic, self._on_odometry, qos)
        executor = SingleThreadedExecutor(context=context)
        executor.add_node(node)
        self._context, self._node, self._executor = context, node, executor
        self._thread = threading.Thread(target=executor.spin, name="skynet-ros2-base-spin", daemon=True)
        self._thread.start()

    def _on_odometry(self, message) -> None:
        stamp = rhal.acquisition_ns()
        pose, twist = message.pose.pose, message.twist.twist
        sample = Odometry(
            received_ns=stamp,
            header_s=message.header.stamp.sec + message.header.stamp.nanosec * 1e-9,
            frame_id=message.header.frame_id,
            child_frame_id=message.child_frame_id,
            position=(pose.position.x, pose.position.y, pose.position.z),
            orientation=(pose.orientation.x, pose.orientation.y, pose.orientation.z, pose.orientation.w),
            linear=(twist.linear.x, twist.linear.y, twist.linear.z),
            angular=(twist.angular.x, twist.angular.y, twist.angular.z),
        )
        with self._lock:
            self._latest = sample

    def close(self) -> None:
        executor, node, context, thread = self._executor, self._node, self._context, self._thread
        self._executor = self._node = self._context = self._thread = self._publisher = None
        try:
            if executor is not None:
                executor.shutdown(timeout_sec=1.0)
            if node is not None:
                node.destroy_node()
            if context is not None:
                context.try_shutdown()
        finally:
            if thread is not None:
                thread.join(timeout=2.0)

    def odometry(self) -> Odometry | None:
        with self._lock:
            return self._latest

    def odometry_publishers(self) -> int:
        if self._node is None:
            return 0
        return len(self._node.get_publishers_info_by_topic(self._odometry_topic))

    def command_types(self) -> tuple[str, ...]:
        if self._node is None:
            return ()
        return tuple(sorted({info.topic_type for info in self._node.get_subscriptions_info_by_topic(self._command_topic)}))

    def prepare(self, message_type: str) -> None:
        if message_type not in self._messages:
            raise TransportError(f"{message_type!r} is not a command type this driver publishes ({', '.join(COMMAND_TYPES)}).")
        if self._node is None:
            raise TransportError("the node is not open.")
        from rclpy.qos import DurabilityPolicy, HistoryPolicy, QoSProfile, ReliabilityPolicy

        qos = QoSProfile(reliability=ReliabilityPolicy.RELIABLE, durability=DurabilityPolicy.VOLATILE, history=HistoryPolicy.KEEP_LAST, depth=_COMMAND_DEPTH)
        self._publisher = self._node.create_publisher(self._messages[message_type], self._command_topic, qos)
        self._type = message_type

    def subscribers(self) -> int:
        return 0 if self._publisher is None else self._publisher.get_subscription_count()

    def publish(self, vx: float, vy: float, wz: float) -> None:
        if self._publisher is None or self._node is None:
            raise TransportError("no command publisher: `prepare` was not called.")
        message = self._messages[self._type]()
        twist = message
        if self._type == TWIST_STAMPED:
            message.header.stamp = self._node.get_clock().now().to_msg()
            message.header.frame_id = self._frame_id
            twist = message.twist
        twist.linear.x, twist.linear.y, twist.angular.z = float(vx), float(vy), float(wz)
        try:
            self._publisher.publish(message)
        except Exception as error:  # noqa: BLE001 — rclpy raises RCLError and context errors; all of them mean the command did not go.
            raise TransportError(f"publishing on {self._command_topic} failed ({error}).") from error
