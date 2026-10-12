"""The link-chaos subject's second reader of the base: where it really is, by a path the faults do not touch.

    python3 chaos_tap.py <fd>      writes one record to <fd> for every odometry message: x, y, yaw, forward speed (four little-endian doubles)

`drivers/ros2_base/chaos.py` puts the kernel's link emulation on the loopback that the driver and the controller share, and sets the driver's DDS to UDP, so every packet the driver sends or
hears meets the profile. This reader is the other half of that: a participant on DDS's default transports, which between two processes of one host is shared memory, and shared memory is
not a network interface. It was measured (2026-10-12, Jazzy): with `lo` delayed by 200 ms the UDP-only listener saw the odometry 0.200 s late and this kind saw it 0.000 s late.
Nothing of this repository's is imported, so a defect in the driver's reading cannot be in the truth it is held to (`observer.py` is the same idea for the live block).

Discovery goes by UDP whatever the data goes by, so a reader started *after* a profile is set would find the controller late; the subject starts it first.
"""

from __future__ import annotations

import math
import os
import struct
import sys

import rclpy
from nav_msgs.msg import Odometry
from rclpy.qos import HistoryPolicy, QoSProfile, ReliabilityPolicy

RECORD = struct.Struct("<4d")


def main() -> int:
    descriptor = int(sys.argv[1])
    topic = os.environ.get("TAP_TOPIC", "/diff_drive_controller/odom")
    rclpy.init()
    node = rclpy.create_node("skynet_chaos_tap")

    def on_message(message: Odometry) -> None:
        pose, twist = message.pose.pose, message.twist.twist
        q = pose.orientation
        yaw = math.atan2(2.0 * (q.w * q.z + q.x * q.y), 1.0 - 2.0 * (q.y * q.y + q.z * q.z))
        try:
            os.write(descriptor, RECORD.pack(pose.position.x, pose.position.y, yaw, twist.linear.x))
        except OSError:
            raise SystemExit(0) from None   # the subject closed its end: this reader is done

    node.create_subscription(Odometry, topic, on_message, QoSProfile(reliability=ReliabilityPolicy.BEST_EFFORT, history=HistoryPolicy.KEEP_LAST, depth=5))
    try:
        while rclpy.ok():
            rclpy.spin_once(node, timeout_sec=0.1)
    except (KeyboardInterrupt, SystemExit):
        pass
    finally:
        node.destroy_node()
        if rclpy.ok():
            rclpy.shutdown()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
