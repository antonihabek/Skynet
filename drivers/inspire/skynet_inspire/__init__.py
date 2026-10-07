"""skynet-inspire: an Inspire Robots RH56 dexterous hand over its RS-485 private protocol, for drivers/python-host (DRY-COVERAGE-PLAN.md item 12.E.8)."""

from .bridge import BridgeRefusal, InspireBridge, TOKEN

__all__ = ["BridgeRefusal", "InspireBridge", "TOKEN"]
