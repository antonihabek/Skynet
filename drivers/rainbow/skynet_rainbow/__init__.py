"""skynet-rainbow: a Rainbow Robotics RB arm (RB3, RB5, RB10, RB16 and the rest of the series) over the control box's command and data ports, for drivers/python-host (DRY-COVERAGE-PLAN.md item 12.E.14)."""

from .bridge import BridgeRefusal, RainbowBridge, TOKEN

__all__ = ["BridgeRefusal", "RainbowBridge", "TOKEN"]
