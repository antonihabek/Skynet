"""The MuJoCo simulator RHAL driver — §70.

The sim half of SCTS cannot run without a simulator driver, so this is load-bearing infrastructure
rather than a lesser path. It is Guest tier twice over: Python cannot hold a real-time path (§31.3),
and a simulator declares itself one (§35.3) so that no simulated run is ever presented as bench
evidence.
"""

from .bridge import BridgeConfig, MujocoBridge, TOOL_VERSION
from .describe import build_descriptor, commandable_joints

__all__ = [
    "BridgeConfig",
    "MujocoBridge",
    "TOOL_VERSION",
    "build_descriptor",
    "commandable_joints",
]
