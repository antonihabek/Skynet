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
``disable_fails`` / ``enable_fails``
    The bus refuses to disable, or to re-enable, torque. A stop that cannot be carried out, and a release that
    cannot, are both things a real bus does (a dead cable, a servo that lost power) and neither may read as done.
"""

from __future__ import annotations

import threading
import time

__all__ = ["FakeBus", "FakeSO101", "FakeCameraArm", "FakeModalArm", "FakeReachy2Shape"]


class FakeBus:
    """The motor bus, to the extent the bridge touches it."""

    def __init__(self, fault: str | None = None) -> None:
        self.fault = fault
        self.torque_enabled = True
        self.disable_calls = 0
        self.enable_calls = 0

    def disable_torque(self) -> None:
        if self.fault == "disable_fails":
            raise RuntimeError("the bus did not acknowledge the torque-disable")
        self.torque_enabled = False
        self.disable_calls += 1

    def enable_torque(self) -> None:
        if self.fault == "enable_fails":
            raise RuntimeError("the bus did not acknowledge the torque-enable")
        self.torque_enabled = True
        self.enable_calls += 1


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
        self.bus = None if fault == "no_torque_disable" else FakeBus(fault)

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


class FakeModalArm(FakeSO101):
    """The same arm, commanded in another mode: its action features end in ``.vel`` or ``.effort`` where the
    SO-101's end in ``.pos``, and it still reports positions. LeRobot robots do exist that take velocity or torque,
    and the bridge's refusals (a torque hold has no correct value) are only reachable with one."""

    def __init__(self, suffix: str, fault: str | None = None) -> None:
        super().__init__(fault)
        self.suffix = suffix

    @property
    def action_features(self) -> dict[str, type]:
        return {f"{motor}.{self.suffix}": float for motor in self.MOTORS}

    def send_action(self, action: dict[str, float]) -> dict[str, float]:
        if self.fault == "send_fails":
            raise RuntimeError("the bus did not acknowledge")
        self.sent.append(dict(action))
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


class FakeReachy2Shape:
    """A robot shaped like LeRobot's `Reachy2Robot`: joints in degrees, and features that are not joints.

    What made the real one break the bridge, reproduced with no library: `action_features` lists
    `mobile_base.vx`, `.vy` and `.vtheta` beside the joints, they carry no `.pos`/`.vel` suffix, and
    `send_action` raises `KeyError` when any of them is missing. There is no torque-disable on it either
    (`turn_off` drops the arms), so a stop can only be a freeze.
    """

    robot_type = "reachy2"
    MOTORS = ["neck_yaw", "l_shoulder_pitch", "l_elbow_pitch", "r_shoulder_pitch", "r_elbow_pitch", "r_gripper"]
    AUX = ("mobile_base.vx", "mobile_base.vy", "mobile_base.vtheta")

    class _Sdk:
        """The one thing of Pollen's SDK the bridge reads that is not in `Robot`'s interface: the timestamp of the last state its stream delivered (`ReachySDK.get_update_timestamp`, in nanoseconds)."""

        def __init__(self, owner: "FakeReachy2Shape") -> None:
            self._owner = owner

        def get_update_timestamp(self) -> int:
            return self._owner.clock_ns

    def __init__(self, aux: tuple[str, ...] | None = None, robot_type: str = "reachy2") -> None:
        self.robot_type = robot_type
        self.aux = self.AUX if aux is None else aux
        self.is_connected = False
        self.sent: list[dict[str, float]] = []
        self.position = {motor: 0.0 for motor in self.MOTORS}
        #: The stream's clock: a tick every 10 ms of its own, whether or not anyone reads, as a live stream's is. While `stalled` is set it does not move: a stream that has stopped leaves a cache that reads
        #: as before and a clock that stands still.
        self._origin = time.monotonic()
        self._stalled_for = 0.0
        self._stalled_since: float | None = None
        self.reachy = self._Sdk(self)
        #: Set to make a call into the robot class block for as long as it stays set, as Pollen's SDK does against a core that has stopped answering (a gRPC call with no deadline).
        self.hang = threading.Event()
        self.entered = threading.Event()
        #: How many calls into the robot class there have been (`get_observation` and `send_action`), to show that a bridge whose last call has not returned makes no more.
        self.calls = 0

    @property
    def action_features(self) -> dict[str, type]:
        return {**{f"{m}.pos": float for m in self.MOTORS}, **{key: float for key in self.aux}}

    @property
    def observation_features(self) -> dict[str, type]:
        return self.action_features

    def connect(self, calibrate: bool = False) -> None:
        self.is_connected = True

    def disconnect(self) -> None:
        self.is_connected = False

    @property
    def clock_ns(self) -> int:
        now = self._stalled_since if self._stalled_since is not None else time.monotonic()
        return 1_000_000_000 + int((now - self._origin - self._stalled_for) * 100) * 10_000_000   # a stream at 100 Hz

    @property
    def stalled(self) -> bool:
        return self._stalled_since is not None

    @stalled.setter
    def stalled(self, value: bool) -> None:
        if value and self._stalled_since is None:
            self._stalled_since = time.monotonic()
        elif not value and self._stalled_since is not None:
            self._stalled_for += time.monotonic() - self._stalled_since
            self._stalled_since = None

    def _block_while_hung(self) -> None:
        self.calls += 1
        if self.hang.is_set():
            self.entered.set()
            while self.hang.is_set():
                time.sleep(0.005)

    def get_observation(self) -> dict[str, float]:
        self._block_while_hung()
        return {f"{m}.pos": v for m, v in self.position.items()}

    def send_action(self, action: dict[str, float]) -> dict[str, float]:
        self._block_while_hung()
        for key in self.aux:
            if key not in action:
                raise KeyError(key.rsplit(".", 1)[-1])  # what the real one raises
        self.sent.append(dict(action))
        for key, value in action.items():
            motor, _, suffix = key.rpartition(".")
            if suffix == "pos" and motor in self.position:
                self.position[motor] = float(value)
        return action

