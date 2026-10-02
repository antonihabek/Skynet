"""The two ways a bridge says no, shared by the bridge and by the motion sets it delegates to.

They live here and not in `bridge.py` because `motions.py` raises them and `bridge.py` imports `motions.py`: a module both import is
the only arrangement that needs no import inside a function.
"""

from __future__ import annotations

__all__ = ["BridgeRefusal", "Refused"]


class BridgeRefusal(Exception):
    """A reason this body did not come up, in the terms an integrator acts on."""


class Refused(Exception):
    """A motion that did not happen: the code it answers with, and the words."""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
