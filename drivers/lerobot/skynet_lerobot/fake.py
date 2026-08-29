"""A LeRobot ``Robot`` with no hardware behind it.

The same idea as ``drivers/mock`` and for the same reason (§63.2): **a developer writes and tests
against it with no hardware at all** — and, more importantly, tests the *failure* paths that are
impossible to provoke safely on a real robot.

It is written from LeRobot's documented ``Robot`` interface: ``connect`` / ``disconnect`` /
``is_connected`` / ``get_observation`` / ``send_action`` / ``observation_features`` /
``action_features``, with motor features keyed ``"{motor}.pos"``. That is what the bridge is
verified against, and it is not the same thing as being verified against LeRobot — see the module
docstring of :mod:`skynet_lerobot.bridge`.

The faults are the point:

``stall``
    ``get_observation`` returns the same numbers forever. The stamp still advances, because the
    bridge stamps at read — so the *only* evidence is that the values stopped changing, which is
    exactly the case a real stalled bus presents and the hardest one to notice.
``nan``
    One motor reports nothing. The bridge must carry it through as NaN rather than as zero.
``raise``
    ``get_observation`` raises. A driver bug must not take the robot with it.
``no_torque_disable``
    The bus exposes no torque-disable, so ``emergency_stop`` is a software latch only and must say
    so rather than let an operator conclude that power was cut.
``send_fails``
    ``send_action`` raises. Commands must report ``SKR_EIO`` rather than silently succeeding.
"""

from __future__ import annotations

__all__ = ["FakeBus", "FakeSO101", "FakeCameraArm"]


class FakeBus:
    """The motor bus, to the extent the bridge touches it."""

    def __init__(self) -> None:
        self.torque_enabled = True
        self.disable_calls = 0

    def disable_torque(self) -> None:
        self.torque_enabled = False
        self.disable_calls += 1


class FakeSO101:
    """An SO-101 follower, in the shape LeRobot presents one."""

    #: The motor names §67.2 maps, in the order LeRobot lists them.
    MOTORS = [
        "shoulder_pan",
        "shoulder_lift",
        "elbow_flex",
        "wrist_flex",
        "wrist_roll",
        "gripper",
    ]

    robot_type = "so101_follower"

    def __init__(self, fault: str | None = None) -> None:
        self.fault = fault
        self.is_connected = False
        self.connect_calls = 0
        self.disconnect_calls = 0
        self.sent: list[dict[str, float]] = []
        self._position = {motor: 0.0 for motor in self.MOTORS}
        self.bus = None if fault == "no_torque_disable" else FakeBus()

    # ── LeRobot's Robot interface ────────────────────────────────────────────

    @property
    def action_features(self) -> dict[str, type]:
        return {f"{motor}.pos": float for motor in self.MOTORS}

    @property
    def observation_features(self) -> dict[str, type]:
        return {f"{motor}.pos": float for motor in self.MOTORS}

    def connect(self, calibrate: bool = True) -> None:
        self.connect_calls += 1
        self.is_connected = True

    def disconnect(self) -> None:
        self.disconnect_calls += 1
        self.is_connected = False

    def get_observation(self) -> dict[str, float]:
        if self.fault == "raise":
            raise RuntimeError("the serial port went away")
        observation = {f"{motor}.pos": value for motor, value in self._position.items()}
        if self.fault == "nan":
            observation["elbow_flex.pos"] = float("nan")
        return observation

    def send_action(self, action: dict[str, float]) -> dict[str, float]:
        if self.fault == "send_fails":
            raise RuntimeError("the bus did not acknowledge")
        self.sent.append(dict(action))
        if self.fault != "stall":
            for key, value in action.items():
                motor, _, _ = key.rpartition(".")
                if motor in self._position:
                    self._position[motor] = float(value)
        return action


class FakeCameraArm(FakeSO101):
    """The same arm with a wrist camera, so the observation split has something to split.

    LeRobot describes a camera with a shape tuple in ``observation_features`` and returns an array
    from ``get_observation``. This fake returns an object with ``shape`` and ``tobytes`` rather than
    a numpy array, because the bridge must not need numpy to work and this proves it does not.
    """

    robot_type = "so101_follower"

    class _Image:
        def __init__(self, height: int, width: int, channels: int) -> None:
            self.shape = (height, width, channels)
            self._raw = bytes(height * width * channels)

        def tobytes(self) -> bytes:
            return self._raw

    @property
    def observation_features(self) -> dict[str, object]:
        features: dict[str, object] = {f"{motor}.pos": float for motor in self.MOTORS}
        features["wrist"] = (24, 32, 3)
        return features

    def get_observation(self) -> dict[str, object]:
        observation: dict[str, object] = dict(super().get_observation())
        observation["wrist"] = self._Image(24, 32, 3)
        return observation
