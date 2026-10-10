"""skynet-dobot: a Dobot V4 arm (CR, CRA, E6 and Nova) over the controller's TCP interface, for drivers/python-host (DRY-COVERAGE-PLAN.md item 12.E.13)."""

from .bridge import BridgeRefusal, DobotBridge, TOKEN

__all__ = ["BridgeRefusal", "DobotBridge", "TOKEN"]
