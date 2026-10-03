"""A second reader of the base, in a process of its own: the opinion `live.py` checks the driver against.

It subscribes to the three topics a base's story is told on, with `rclpy` directly and none of this repository's code, and writes one JSON line per message to standard output
with the wall-clock time it arrived:

    joint     the wheels' joint state (position, velocity), published by `joint_state_broadcaster` off the hardware interface — what the wheels did
    odom      the controller's odometry — what the driver reads
    command   what arrived on the command topic — what the driver sent, as a second subscriber on it receives it

    python3 observer.py <command topic> <command message: Twist|TwistStamped> <odometry topic> <joint states topic>

It exists because the driver cannot be its own witness. The driver says it published 0.2 m/s; the observer says what arrived on the topic and what the wheels did about it, and the
two meet only in `live.py`'s arithmetic.
"""

from __future__ import annotations

import json
import math
import sys
import time

import rclpy
from geometry_msgs.msg import Twist, TwistStamped
from nav_msgs.msg import Odometry
from rclpy.qos import DurabilityPolicy, HistoryPolicy, QoSProfile, ReliabilityPolicy
from sensor_msgs.msg import JointState


def emit(**fields) -> None:
    fields["t"] = time.time()
    print(json.dumps(fields, separators=(",", ":")), flush=True)


def main() -> int:
    command_topic, command_type, odometry_topic, joint_topic = sys.argv[1:5]
    rclpy.init()
    node = rclpy.create_node("skynet_ros2_base_observer")
    qos = QoSProfile(reliability=ReliabilityPolicy.BEST_EFFORT, durability=DurabilityPolicy.VOLATILE, history=HistoryPolicy.KEEP_LAST, depth=10)

    def on_joint(message: JointState) -> None:
        emit(k="joint", name=list(message.name), position=list(message.position), velocity=list(message.velocity))

    def on_odom(message: Odometry) -> None:
        q = message.pose.pose.orientation
        emit(
            k="odom",
            frame=message.header.frame_id,
            child=message.child_frame_id,
            x=message.pose.pose.position.x,
            y=message.pose.pose.position.y,
            z=message.pose.pose.position.z,
            q=[q.x, q.y, q.z, q.w],
            yaw=math.atan2(2.0 * (q.w * q.z + q.x * q.y), 1.0 - 2.0 * (q.y * q.y + q.z * q.z)),
            vx=message.twist.twist.linear.x,
            vy=message.twist.twist.linear.y,
            wz=message.twist.twist.angular.z,
        )

    def on_command(message) -> None:
        twist = message.twist if command_type == "TwistStamped" else message
        fields = {"vx": twist.linear.x, "vy": twist.linear.y, "wz": twist.angular.z}
        if command_type == "TwistStamped":
            fields["frame"] = message.header.frame_id
            fields["stamp"] = message.header.stamp.sec + message.header.stamp.nanosec * 1e-9
        emit(k="command", **fields)

    node.create_subscription(JointState, joint_topic, on_joint, qos)
    node.create_subscription(Odometry, odometry_topic, on_odom, qos)
    node.create_subscription(TwistStamped if command_type == "TwistStamped" else Twist, command_topic, on_command, qos)
    emit(k="ready")
    try:
        rclpy.spin(node)
    except KeyboardInterrupt:
        pass
    finally:
        node.destroy_node()
        rclpy.try_shutdown()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
