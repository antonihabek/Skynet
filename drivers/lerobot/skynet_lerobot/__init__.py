"""The LeRobot RHAL bridge — §16, §67.

Every robot LeRobot supports becomes a Guest-tier Skynet robot for the cost of one driver. The
ceiling is Guest and is a property of the language rather than of this code: the GIL and the garbage
collector cannot hold a real-time path (§31.3, §67.3).
"""

from .bridge import BridgeConfig, LeRobotBridge, open_bridge
from .roles import ROLE_TABLES, known_classes, roles_for

__all__ = [
    "BridgeConfig",
    "LeRobotBridge",
    "open_bridge",
    "ROLE_TABLES",
    "known_classes",
    "roles_for",
]
