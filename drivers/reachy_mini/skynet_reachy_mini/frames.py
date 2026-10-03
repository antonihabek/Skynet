"""The Reachy Mini daemon's REST interface: what the daemon's own routes take and give, and nothing about what to do with it.

Pure, like `skynet_ros_industrial.frames`: no sockets, no clocks, no state. Pollen Robotics publish the daemon (`reachy_mini`, Apache-2.0, not vendored here) and its REST API is FastAPI's, so the routes
and the shapes below were read out of its own source (`reachy_mini/daemon/app/routers/*.py`, `daemon/app/models.py`, `io/protocol.py` at the commit `sim/Dockerfile` pins) and `layout_check.py` opens
that source and requires every one to still be there. A transcription with no citation is a rumour.

# The body

A Reachy Mini is a head on a Stewart platform, a body that turns, and two antennas. The daemon takes and reports them as **a head pose** (x, y, z in metres, roll, pitch, yaw in radians, the head's
own frame), **a body yaw** and **two antenna angles in the order the real robot uses: right, then left** (`MuJocoBackend` swaps its own order to match, with a comment saying so). That is nine
single-axis values, and they are the nine joints this driver describes. The six Stewart-platform motors behind the head pose are not exposed to a client that sets a pose: the daemon solves the
inverse kinematics, and `head_joints` in the state is read-only here.

# What the daemon says about limits

`docs/source/SDK/core-concept.md` in the maker's repository gives the head's pitch and roll as ±40°, the head's yaw as ±180°, the body's yaw as ±160° and **the head's yaw as at most 65° from the body's**, and
says the SDK clamps to the nearest valid position. It gives no translation range for the head and none for the antennas, so those are the configuration's.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

__all__ = [
    "API",
    "DOCUMENTED_LIMITS",
    "JOINTS",
    "YAW_DELTA_MAX_RAD",
    "DaemonState",
    "MotorMode",
    "Pose",
    "ProtocolError",
    "Route",
    "State",
    "parse_state",
    "target_body",
]

#: `APIRouter(prefix="/api")` in `daemon/app/main.py`.
API = "/api"


class Route:
    """The routes this driver uses, each with the router file that defines it."""

    DAEMON_STATUS = API + "/daemon/status"                # routers/daemon.py: GET /status -> DaemonStatus
    STATE_FULL = API + "/state/full"                      # routers/state.py: GET /full -> FullState
    SET_TARGET = API + "/move/set_target"                 # routers/move.py: POST /set_target, a FullBodyTarget
    RUNNING = API + "/move/running"                       # routers/move.py: GET /running -> [{uuid}]
    STOP_MOVE = API + "/move/stop"                        # routers/move.py: POST /stop, {uuid}
    MOTOR_STATUS = API + "/motors/status"                 # routers/motors.py: GET /status -> {mode}
    SET_MODE = API + "/motors/set_mode/{mode}"            # routers/motors.py: POST /set_mode/{mode}


class MotorMode:
    """`MotorControlMode` (`io/protocol.py`)."""

    ENABLED = "enabled"
    DISABLED = "disabled"
    GRAVITY_COMPENSATION = "gravity_compensation"


class DaemonState:
    """`DaemonState` (`io/protocol.py`)."""

    NOT_INITIALIZED = "not_initialized"
    STARTING = "starting"
    RUNNING = "running"
    STOPPING = "stopping"
    STOPPED = "stopped"
    ERROR = "error"


#: The nine joints, in the order a command and a state carry them: the head pose's six (`XYZRPYPose`), the body's yaw, then the antennas, right and then left.
JOINTS: tuple[tuple[str, str], ...] = (
    ("head_x", "prismatic"), ("head_y", "prismatic"), ("head_z", "prismatic"),
    ("head_roll", "revolute"), ("head_pitch", "revolute"), ("head_yaw", "revolute"),
    ("body_yaw", "revolute"),
    ("antenna_right", "revolute"), ("antenna_left", "revolute"),
)

_DEG = math.pi / 180.0

#: What the maker's documentation (`core-concept.md`, "Safety Limits") gives, in radians, by joint name: pitch and roll ±40°, head yaw ±180°, body yaw ±160°. The others have no documented range.
DOCUMENTED_LIMITS: dict[str, tuple[float, float]] = {
    "head_roll": (-40.0 * _DEG, 40.0 * _DEG),
    "head_pitch": (-40.0 * _DEG, 40.0 * _DEG),
    "head_yaw": (-180.0 * _DEG, 180.0 * _DEG),
    "body_yaw": (-160.0 * _DEG, 160.0 * _DEG),
}

#: "Yaw Delta: Max 65° difference between Head and Body Yaw" (`core-concept.md`).
YAW_DELTA_MAX_RAD = 65.0 * _DEG


class ProtocolError(Exception):
    """A reply that is not what the daemon's own models say it is."""


@dataclass(frozen=True)
class Pose:
    x: float
    y: float
    z: float
    roll: float
    pitch: float
    yaw: float


@dataclass(frozen=True)
class State:
    """What `GET /api/state/full` says, in this driver's nine joints."""

    positions: tuple[float, ...]
    control_mode: str | None
    timestamp: str | None


def target_body(values: list[float] | tuple[float, ...]) -> dict:
    """A `FullBodyTarget` (`daemon/app/models.py`) from the nine joint values: the head pose as `XYZRPYPose`, the antennas as a pair and the body yaw."""
    if len(values) != len(JOINTS):
        raise ProtocolError(f"{len(values)} values for {len(JOINTS)} joints")
    for name, value in zip((j[0] for j in JOINTS), values):
        if not math.isfinite(value):
            raise ProtocolError(f"{name} is {value}: a target nobody can act on")
    x, y, z, roll, pitch, yaw, body, right, left = values
    return {
        "target_head_pose": {"x": x, "y": y, "z": z, "roll": roll, "pitch": pitch, "yaw": yaw},
        "target_antennas": [right, left],
        "target_body_yaw": body,
    }


def parse_state(document: object) -> State:
    """A `FullState` (`daemon/app/models.py`) with the head pose as `XYZRPYPose` (the route's default), the body yaw and the antennas: refused if any of the three is missing or is not numbers."""
    if not isinstance(document, dict):
        raise ProtocolError("the state is not an object")
    pose = document.get("head_pose")
    if not isinstance(pose, dict):
        raise ProtocolError("the state has no `head_pose` as x, y, z, roll, pitch, yaw (a matrix is what `use_pose_matrix=true` asks for, and this driver does not)")
    try:
        head = [float(pose[key]) for key in ("x", "y", "z", "roll", "pitch", "yaw")]
    except (KeyError, TypeError, ValueError) as error:
        raise ProtocolError(f"`head_pose` is not six numbers: {error}") from error
    body = document.get("body_yaw")
    antennas = document.get("antennas_position")
    if isinstance(body, bool) or not isinstance(body, (int, float)):
        raise ProtocolError("the state has no `body_yaw`")
    if not isinstance(antennas, list) or len(antennas) != 2 or any(isinstance(a, bool) or not isinstance(a, (int, float)) for a in antennas):
        raise ProtocolError("the state has no `antennas_position` as two numbers")
    mode = document.get("control_mode")
    stamp = document.get("timestamp")
    values = (*head, float(body), float(antennas[0]), float(antennas[1]))
    if not all(math.isfinite(v) for v in values):
        raise ProtocolError("the state carries a number that is not finite")
    return State(values, mode if isinstance(mode, str) else None, stamp if isinstance(stamp, str) else None)
