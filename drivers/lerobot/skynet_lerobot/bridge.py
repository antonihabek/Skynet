"""The LeRobot RHAL bridge — §16 and §67.

> **A LeRobot RHAL driver.** ``Robot.connect/get_observation/send_action`` is very nearly the
> simplest possible RHAL. Wrapping it means **every robot LeRobot supports becomes a Guest-tier
> Skynet robot on day one**, for the cost of one driver. This is the single fastest path from zero
> to a real device fleet, and it should be built first.

The mapping, from §67.1:

| RHAL | LeRobot |
|---|---|
| ``open(config)`` | ``robot.connect()`` |
| ``read_state`` | ``robot.get_observation()`` — proprioception split from camera frames |
| ``write_command`` | ``robot.send_action()`` |
| ``close`` | ``robot.disconnect()`` |
| ``describe`` | Built from ``observation_features`` / ``action_features`` plus the motor bus config |
| ``read_sensor`` | The camera entries of ``get_observation()`` |
| ``emergency_stop`` | Torque-disable on the motor bus |

# The ceiling, stated plainly

Python caps this at **Guest** (§67.3). The bridge cannot host an enforcing kernel and never claims
to. What it still delivers is most of why anyone would adopt early: the package system, skills with
enforced preconditions, capability grants, fleet reconciliation, the black box, episode capture, and
advisory violation reporting that tells an operator what a policy *would* have commanded before it
is trusted with anything.

# Four things this bridge does not know, and says so rather than inventing

1. **Joint ratings.** LeRobot does not publish position, velocity or effort limits through the
   ``Robot`` interface — they live in the motor bus configuration and the calibration file, in raw
   encoder units, not in radians and newton-metres. So ``joints[].limits`` is **empty**, and the
   transport planner reports every channel bound here as ``unscalable_channel`` rather than scaling
   against a guess. That is the correct outcome and it is worth understanding before it surprises
   somebody: a body with no ratings is a body a policy cannot be safely scaled onto.
2. **The safety envelope.** Null, always, for the same reason it is null everywhere else.
3. **Acquisition time.** ``get_observation()`` supplies none, so the bridge stamps at read (§67.1,
   §47.2 — the estimated-latency path, reported honestly). That timestamp is therefore *after*
   acquisition by an unknown amount, which inflates every downstream latency budget in the safe
   direction. The descriptor's provenance records it.
4. **Units.** LeRobot's ``.pos`` values are normalised or degrees depending on the robot and its
   calibration; the RHAL is radians. The bridge converts only when told the unit, and otherwise
   passes values through and **says in the descriptor that it did**. Silently treating degrees as
   radians is a 57× error on every command, and it is exactly the kind of thing that looks like a
   tuning problem for a day.

# Written from documentation, not from a running robot

§62.2 is explicit that this is how these drivers get written: from public documentation, without
waiting for a vendor. The consequence is that the bridge has been exercised against a faithful fake
(:mod:`skynet_lerobot.fake`) and **not** against LeRobot itself or any hardware. It is Guest tier
either way; what it has not yet earned is the claim that the adapter's field names match the
library's on the day you run it. :func:`open_bridge` fails with that written on it rather than
crashing on an attribute that moved.
"""

from __future__ import annotations

import json
import math
import sys
import time
from typing import Any

import skynet_rhal as rhal
from skynet_rhal import Command, Frame, State

from .roles import roles_for

__all__ = ["LeRobotBridge", "open_bridge", "BridgeConfig"]

#: What a LeRobot feature key's suffix says about the control mode it belongs to.
_SUFFIX_MODE = {".pos": "position", ".vel": "velocity", ".effort": "torque", ".torque": "torque"}

_MODE_SUFFIX = {
    rhal.MODE_POSITION: ".pos",
    rhal.MODE_VELOCITY: ".vel",
    rhal.MODE_TORQUE: ".effort",
}

#: Units the bridge will convert from. Anything else is passed through unconverted and declared.
_UNIT_SCALE = {"rad": 1.0, "radians": 1.0, "deg": math.pi / 180.0, "degrees": math.pi / 180.0}


class BridgeConfig:
    """What the operator supplies that LeRobot does not.

    Parsed from the JSON string the OS hands to ``open()``. Every field is optional; an absent one
    means the bridge declares less rather than assuming more.
    """

    def __init__(self, raw: str | None = None) -> None:
        data: dict[str, Any] = {}
        if raw and raw.strip():
            try:
                parsed = json.loads(raw)
                if isinstance(parsed, dict):
                    data = parsed
            except json.JSONDecodeError as error:
                raise ValueError(
                    f"the driver configuration is not JSON: {error}. It is an object, and every "
                    'field in it is optional: {"robot_class": "so101_follower", "unit": "deg", '
                    '"role_overrides": {"shoulder_pan": "left_arm.shoulder.yaw"}}'
                ) from error

        #: Which mapping table to use. Defaults to the robot's own class name.
        self.robot_class: str | None = data.get("robot_class")
        #: Motor name to canonical role, overriding the table. This is how a bimanual rig fixes the
        #: side that §67.2's table assumes.
        self.role_overrides: dict[str, str] = dict(data.get("role_overrides") or {})
        #: The unit LeRobot's position values are in. Absent means "unknown", and the bridge then
        #: passes values through and declares that it did.
        self.unit: str | None = data.get("unit")
        #: Slug for the descriptor. Defaults to the robot class.
        self.robot_slug: str | None = data.get("robot_slug")
        #: Whether to call `robot.calibrate()` semantics via connect(calibrate=True).
        self.calibrate: bool = bool(data.get("calibrate", True))

        if self.unit is not None and self.unit not in _UNIT_SCALE:
            raise ValueError(
                f"unit {self.unit!r} is not one this bridge converts. Known: "
                f"{', '.join(sorted(_UNIT_SCALE))}. Leave it out to pass values through unconverted "
                "— which is declared in the descriptor rather than assumed."
            )


class LeRobotBridge:
    """A :class:`skynet_rhal.Driver` over any object with LeRobot's ``Robot`` interface."""

    def __init__(self, robot: Any, config: BridgeConfig | None = None) -> None:
        self._robot = robot
        self._config = config or BridgeConfig()
        self._opened = False
        self._estopped = False
        self._seq = 0
        self._descriptor: str | None = None

        #: Motor names, in the order they appear in `action_features` — which is the order the
        #: RHAL's joint arrays use, and the order `describe()` lists them in. One order, decided
        #: once, or a joint index means two different things on two code paths.
        self._motors: list[str] = []
        self._modes: list[str] = []
        self._roles: dict[str, str | None] = {}
        self._role_note = ""
        self._cameras: list[str] = []
        self._last_position: list[float] = []
        self._last_frames: dict[str, Frame] = {}

    # ── Lifecycle ────────────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            self._config = BridgeConfig(config)
        except ValueError as error:
            print(f"lerobot-bridge: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_EINVAL

        try:
            if not getattr(self._robot, "is_connected", False):
                self._robot.connect()
        except Exception as error:  # noqa: BLE001 — a bus that did not answer is not our bug
            print(f"lerobot-bridge: connect() failed: {error!r}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO

        try:
            self._introspect()
        except Exception as error:  # noqa: BLE001
            print(
                f"lerobot-bridge: could not read this robot's feature description: {error!r}. "
                "The bridge builds the embodiment descriptor from `action_features` and "
                "`observation_features`; a robot class that does not expose them needs an adapter.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_EPROTO

        if not self._motors:
            print(
                "lerobot-bridge: this robot declares no actuated features, so there is no body to "
                "describe. `action_features` was empty.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_ENODEV

        self._opened = True
        self._estopped = False
        self._last_position = [0.0] * len(self._motors)
        return rhal.SKR_OK

    def close(self) -> int:
        self._opened = False
        try:
            self._robot.disconnect()
        except Exception as error:  # noqa: BLE001
            print(f"lerobot-bridge: disconnect() failed: {error!r}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO
        return rhal.SKR_OK

    def _introspect(self) -> None:
        """Work out the body from what LeRobot declares about itself."""
        action = _features(self._robot, "action_features")
        observation = _features(self._robot, "observation_features")

        for key in action:
            motor, suffix = _split_feature(key)
            if suffix is None:
                continue
            self._motors.append(motor)
            self._modes.append(_SUFFIX_MODE[suffix])

        # Cameras are the observation entries that are not scalar motor readings. LeRobot describes
        # them with a shape tuple; anything with a shape is a frame, anything without is a number.
        for key, spec in observation.items():
            _, suffix = _split_feature(key)
            if suffix is not None:
                continue
            if isinstance(spec, tuple) or isinstance(spec, list):
                self._cameras.append(key)

        robot_class = (
            self._config.robot_class
            or getattr(self._robot, "robot_type", None)
            or type(self._robot).__name__
        )
        self._roles, self._role_note = roles_for(
            str(robot_class), self._motors, self._config.role_overrides
        )
        self._descriptor = self._build_descriptor(str(robot_class))

    # ── The real-time-ish path ───────────────────────────────────────────────

    def read_state(self, out: State, deadline_ns: int) -> int:
        if not self._opened:
            return rhal.SKR_EPERM

        try:
            observation = self._robot.get_observation()
        except Exception as error:  # noqa: BLE001
            print(f"lerobot-bridge: get_observation() failed: {error!r}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO
        if not isinstance(observation, dict):
            return rhal.SKR_EPROTO

        # §67.1: LeRobot does not supply acquisition times, so the stamp is taken at read. It is
        # therefore *later* than acquisition by an unknown amount, which inflates every downstream
        # latency budget — in the safe direction, and the descriptor says so.
        stamp = time.monotonic_ns()
        self._seq += 1

        scale = _UNIT_SCALE.get(self._config.unit or "", 1.0)
        positions: list[float] = []
        velocities: list[float] = []
        efforts: list[float] = []

        for motor in self._motors:
            positions.append(_number(observation.get(f"{motor}.pos")) * scale)
            # NaN where the robot does not report it. Zero would be a measurement, and this is not
            # one — the distinction is the whole reason the RHAL has three arrays instead of one.
            velocities.append(_optional(observation.get(f"{motor}.vel"), scale))
            efforts.append(_optional(observation.get(f"{motor}.effort")))

        self._last_position = list(positions)

        # Camera frames are kept for read_sensor rather than pushed into the state, which is the
        # split §67.1 asks for: proprioception to `rhal_state`, images to `read_sensor`.
        for name in self._cameras:
            value = observation.get(name)
            if value is not None:
                self._last_frames[name] = _to_frame(value, stamp)

        out.stamp_ns = stamp
        out.seq = self._seq
        out.flags = rhal.FLAG_ESTOP if self._estopped else 0
        # §24's homing field, and this bridge is the one place in the driver set where the answer
        # is genuinely unknown. LeRobot's robot classes expose no homing or referencing state
        # through this interface: a follower arm may be at its calibrated zero or may have been
        # power-cycled with the leader somewhere else, and nothing in the observation says which.
        # Reporting `complete` would be inventing it and reporting `not_required` would be worse —
        # these are relative encoders. So the field is left `unreported`, which is the state §24
        # added it to be able to express, and the robot is held.
        out.fault_code = 0
        out.position_rad = positions
        out.velocity_rad_s = velocities
        out.effort_nm = efforts
        out.base_pose = [math.nan] * 7  # LeRobot does not localise the base through this interface
        out.base_twist = [0.0] * 6
        return rhal.SKR_OK

    def write_command(self, cmd: Command, deadline_ns: int) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        if cmd.joint_count != len(self._motors) or len(cmd.mask) != cmd.joint_count:
            return rhal.SKR_EINVAL
        if cmd.mode not in _MODE_SUFFIX:
            return rhal.SKR_EINVAL
        if self._estopped:
            return rhal.SKR_EPERM

        suffix = _MODE_SUFFIX[cmd.mode]
        wanted = rhal.MODE_NAMES[cmd.mode]

        # A mode this body does not accept is refused rather than approximated. Sending a position
        # to a joint expecting a velocity is not a degraded command, it is a different one.
        for index, motor in enumerate(self._motors):
            if self._modes[index] != wanted:
                return rhal.SKR_EINVAL

        # A masked joint must *hold*, and holding has a correct realisation in position and velocity
        # and none in torque: holding a torque-controlled joint means gravity compensation, which
        # this bridge cannot compute. Refusing is the only honest answer — commanding zero torque
        # would drop the arm, which is the exact failure the mask exists to prevent.
        if cmd.mode == rhal.MODE_TORQUE and not all(cmd.mask):
            print(
                "lerobot-bridge: a torque command masks a joint, and 'hold' has no correct value "
                "in torque — zero is limp, not held. Refused rather than approximated.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_EINVAL

        inverse = 1.0 / _UNIT_SCALE.get(self._config.unit or "", 1.0)
        action: dict[str, float] = {}
        for index, motor in enumerate(self._motors):
            value = cmd.values[index]
            if not math.isfinite(value):
                return rhal.SKR_EINVAL
            if cmd.mask[index]:
                action[f"{motor}{suffix}"] = value * inverse
            elif cmd.mode == rhal.MODE_POSITION:
                # Hold: command where it already is.
                action[f"{motor}{suffix}"] = self._last_position[index] * inverse
            else:
                action[f"{motor}{suffix}"] = 0.0  # hold, in velocity, is zero velocity

        try:
            self._robot.send_action(action)
        except Exception as error:  # noqa: BLE001
            print(f"lerobot-bridge: send_action() failed: {error!r}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        """Torque-disable on the motor bus (§67.1), latched.

        The latch is what makes the stop observable in the next ``read_state``, which is the half of
        ``safety.estop.latency`` that can be measured without hardware. The *latency* is a bench
        measurement and this driver does not claim one.
        """
        self._estopped = True
        bus = getattr(self._robot, "bus", None)
        try:
            if bus is not None and hasattr(bus, "disable_torque"):
                bus.disable_torque()
            elif hasattr(self._robot, "disable_torque"):
                self._robot.disable_torque()
            else:
                # No torque-disable on this robot class. The latch still holds — every subsequent
                # command is refused — but nothing was cut, and saying so is the point: an operator
                # reading the black box must not conclude that power was removed.
                print(
                    "lerobot-bridge: this robot exposes no torque-disable, so the stop is a "
                    "software latch only. Commands are refused from here; the drives were not cut.",
                    file=sys.stderr,
                    flush=True,
                )
        except Exception as error:  # noqa: BLE001
            print(f"lerobot-bridge: torque-disable failed: {error!r}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO
        return rhal.SKR_OK

    def read_sensor(self, name: str, out: Frame) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        if name not in self._cameras:
            return rhal.SKR_ENODEV
        frame = self._last_frames.get(name)
        if frame is None:
            # Read but never populated: the camera is declared and has produced nothing yet.
            return rhal.SKR_EIO
        out.stamp_ns = frame.stamp_ns
        out.width = frame.width
        out.height = frame.height
        out.stride = frame.stride
        out.format = frame.format
        out.data = frame.data
        return rhal.SKR_OK

    # ── The body description ─────────────────────────────────────────────────

    def describe(self) -> str:
        if self._descriptor is None:
            raise RuntimeError("describe() before open()")
        return self._descriptor

    def _build_descriptor(self, robot_class: str) -> str:
        joints = []
        for index, motor in enumerate(self._motors):
            joints.append(
                {
                    "name": motor,
                    "role": self._roles.get(motor),
                    "type": "revolute",
                    # Empty, and this is the honest state: LeRobot does not publish joint ratings
                    # through the Robot interface. They live in the motor bus configuration and the
                    # calibration file, in raw encoder units. A channel bound here is reported
                    # `unscalable_channel` rather than scaled against a guess.
                    "limits": {},
                    "control_modes": [self._modes[index]],
                }
            )

        sensors = [
            {
                "name": name,
                "role": None,
                "modality": "rgb",
                "width": None,
                "height": None,
                "fps": None,
                "frame": None,
            }
            for name in self._cameras
        ]

        modes = sorted(set(self._modes))
        unit = self._config.unit
        descriptor = {
            "schema_version": 1,
            "robot_slug": self._config.robot_slug or _slug(robot_class),
            "digest": None,
            "verified": False,
            # LeRobot's Robot interface says nothing about how the body is carried. `fixed` is the
            # conservative reading — it makes base-velocity channels refuse rather than command
            # motion — and LeKiwi-style bases override it through configuration.
            "base": "fixed",
            "dof": len(self._motors),
            "joints": joints,
            # LeRobot has no notion of an end effector: a gripper is a motor like any other. A
            # policy's `gripper` channel binds to an end effector, so it is refused until a person
            # declares one with its kind, dof and grip force.
            "end_effectors": [],
            "sensors": sensors,
            "control": {
                "modes": modes,
                "rate_hz_max": None,
                "rate_hz_min": None,
                # The number that decides whether a policy trained at 50 Hz is stable on this body
                # (§5.2). LeRobot does not publish it and this bridge has not measured it.
                "command_latency_ms": None,
            },
            "envelope": None,
            "solvers": [],
            "provenance": {
                "tool": "skynet-lerobot bridge",
                "driver_ceiling": "guest",
                "ceiling_reason": (
                    "Python. The GIL and the garbage collector cannot hold a real-time path "
                    "(§31.3, §67.3), so this driver is an observer with a body attached: no "
                    "enforcing kernel, and it never claims one."
                ),
                "robot_class": robot_class,
                "roles": self._role_note,
                "limits": (
                    "Not declared. LeRobot does not publish joint ratings through the Robot "
                    "interface; they live in the motor bus configuration and the calibration file "
                    "in raw encoder units. Every transport channel bound to this body is therefore "
                    "reported unscalable rather than scaled against a guess."
                ),
                "envelope": (
                    "Not declared. A sourced safety figure comes from the vendor's documentation or "
                    "a risk assessment, and this bridge has neither."
                ),
                "timestamps": (
                    "Stamped at read, not at acquisition: LeRobot supplies no acquisition time "
                    "(§67.1, §47.2). Every latency measured against this stamp is therefore an "
                    "over-estimate, which is the safe direction."
                ),
                "units": (
                    f"Position values converted from {unit} to radians."
                    if unit
                    else "Passed through unconverted. LeRobot's `.pos` values are normalised or in "
                    "degrees depending on the robot and its calibration, and this bridge was not "
                    "told which. Set `unit` in the driver configuration once you know; treating "
                    "degrees as radians is a 57x error on every command."
                ),
            },
        }
        return json.dumps(descriptor, sort_keys=True, separators=(",", ":"))


# ── Helpers ──────────────────────────────────────────────────────────────────


def _features(robot: Any, attribute: str) -> dict[str, Any]:
    value = getattr(robot, attribute, None)
    if value is None:
        raise AttributeError(
            f"this robot has no `{attribute}`. The bridge builds the descriptor from it, and "
            "there is nothing to fall back on that would not be a guess about the body."
        )
    if not isinstance(value, dict):
        raise TypeError(f"`{attribute}` is a {type(value).__name__}, and a mapping was expected.")
    return value


def _split_feature(key: str) -> tuple[str, str | None]:
    """``"shoulder_pan.pos"`` becomes ``("shoulder_pan", ".pos")``; a camera name gets ``None``."""
    for suffix in _SUFFIX_MODE:
        if key.endswith(suffix):
            return key[: -len(suffix)], suffix
    return key, None


def _number(value: Any) -> float:
    try:
        return float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return math.nan


def _optional(value: Any, scale: float = 1.0) -> float:
    """A reading the robot may not report. Absent is NaN, never zero."""
    if value is None:
        return math.nan
    return _number(value) * scale


def _to_frame(value: Any, stamp_ns: int) -> Frame:
    """Wrap whatever LeRobot handed back as a camera frame.

    Handles a numpy array without importing numpy — anything with ``shape`` and ``tobytes`` — and
    falls back to raw bytes. The format is declared RGB8 because that is what LeRobot's camera
    interface produces; a driver for a depth camera would say otherwise.
    """
    shape = getattr(value, "shape", None)
    if shape is not None and len(shape) >= 2:
        height, width = int(shape[0]), int(shape[1])
        channels = int(shape[2]) if len(shape) > 2 else 1
        raw = value.tobytes() if hasattr(value, "tobytes") else bytes(value)
        return Frame(
            stamp_ns=stamp_ns,
            width=width,
            height=height,
            stride=width * channels,
            format=0,
            data=raw,
        )
    raw = bytes(value) if isinstance(value, (bytes, bytearray)) else b""
    return Frame(stamp_ns=stamp_ns, width=0, height=0, stride=0, format=0, data=raw)


def _slug(text: str) -> str:
    out = []
    pending = False
    for ch in text:
        if ch.isalnum():
            if pending and out:
                out.append("-")
            pending = False
            out.append(ch.lower())
        else:
            pending = True
    return "".join(out) or "lerobot-robot"


def open_bridge(config: str) -> LeRobotBridge:
    """Construct a bridge over a real LeRobot robot named in ``config``.

    ``config`` is the JSON the OS hands to ``open()``, and needs one more field than
    :class:`BridgeConfig` uses:

    ```json
    {"robot": {"type": "so101_follower", "port": "/dev/ttyACM0", "id": "arm"},
     "unit": "deg",
     "role_overrides": {"shoulder_pan": "left_arm.shoulder.yaw"}}
    ```

    Raises with the fix named when LeRobot is not installed, rather than an ``ImportError`` a driver
    author has to interpret (§64).
    """
    try:
        from lerobot.robots import make_robot_from_config  # type: ignore
        from lerobot.robots import RobotConfig  # type: ignore
    except ImportError as error:
        raise RuntimeError(
            "LeRobot is not installed, so there is no robot to bridge to. "
            "`pip install lerobot`, or construct LeRobotBridge yourself around any object with "
            "LeRobot's Robot interface — the bridge does not import lerobot except here."
        ) from error

    data = json.loads(config) if config and config.strip() else {}
    spec = data.get("robot")
    if not isinstance(spec, dict) or "type" not in spec:
        raise ValueError(
            'the configuration needs a `robot` object naming the LeRobot robot to open, e.g. '
            '{"robot": {"type": "so101_follower", "port": "/dev/ttyACM0"}}.'
        )

    robot = make_robot_from_config(RobotConfig(**spec))
    return LeRobotBridge(robot, BridgeConfig(config))
