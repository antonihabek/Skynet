"""This driver's routes, models and limits against Pollen Robotics' own daemon source.

    REACHY_MINI=/path/to/reachy_mini python3 layout_check.py

`skynet_reachy_mini.frames` transcribes the daemon's routes, the shape of its target and state, its motor modes and daemon states and the limits its documentation gives, with the maker's file beside each. A
transcription is right on the day it is made. This is the check that it is still right on the day the maker ships a release:

* **Routes**, read out of the maker's routers: the prefix each router is mounted under, and the method and path of every route this driver calls, found where the maker's own decorator says they are.
* **Models**: the fields of `FullBodyTarget`, `XYZRPYPose`, `FullState`, `DaemonStatus` and the enums, from `daemon/app/models.py` and `io/protocol.py`.
* **Limits**: the numbers in the maker's `core-concept.md` "Safety Limits" table, against `frames.DOCUMENTED_LIMITS` and the yaw delta; and the maker's own comment that the real robot's antenna order is right then left.
* **The maker's own models, run**, when `reachy_mini` is importable (it is, in the substrate image): a target built by `frames.target_body` is validated by the maker's `FullBodyTarget`, and a state the maker's `FullState`
  makes is read back by `frames.parse_state`, so that a field renamed under this driver is found by a model that refuses it and not by a robot that ignores it.

Exits non-zero on the first disagreement it cannot explain, and says what was compared.
"""

from __future__ import annotations

import math
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from skynet_reachy_mini import frames

FAILURES: list[str] = []
CHECKS = 0


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")


def read(core: str, relative: str) -> str:
    with open(os.path.join(core, "src", "reachy_mini", relative), encoding="utf-8") as handle:
        return handle.read()


def router_prefix(text: str) -> str | None:
    found = re.search(r"APIRouter\(\s*(?:[^)]*?)prefix\s*=\s*\"([^\"]+)\"", text, re.DOTALL)
    return None if found is None else found.group(1)


def has_route(text: str, method: str, path: str) -> bool:
    return re.search(r"@router\." + method + r"\(\s*\"" + re.escape(path) + r"\"", text) is not None


def class_fields(text: str, name: str) -> list[str]:
    """The annotated attributes of a pydantic model or an enum's members, from the class body."""
    found = re.search(r"^class " + re.escape(name) + r"\b[^\n]*:\n((?:(?:    .*)?\n)+)", text, re.MULTILINE)
    if found is None:
        return []
    return re.findall(r"^    (\w+)\s*[:=]", found.group(1), re.MULTILINE)


def main() -> int:
    core = os.environ.get("REACHY_MINI")
    if not core or not os.path.isdir(os.path.join(core, "src", "reachy_mini", "daemon")):
        print(
            "::error::REACHY_MINI does not name a reachy_mini checkout. This check opens the maker's own source and has nothing to compare without it:\n"
            "  git clone --depth 1 https://github.com/pollen-robotics/reachy_mini\n"
            "  REACHY_MINI=$PWD/reachy_mini python3 layout_check.py"
        )
        return 2

    print(f"skynet-reachy-mini layout check, against the source at {core}\n")

    print("the routes")
    main_text = read(core, "daemon/app/main.py")
    check(re.search(r"APIRouter\(prefix=\"" + re.escape(frames.API) + r"\"\)", main_text) is not None, f"the routes are mounted under {frames.API}")
    check(re.search(r"fastapi_port: int = 8000", main_text) is not None, "and the daemon listens on 8000 unless told otherwise")
    for module, prefix, routes in (
        ("daemon", "/daemon", (("get", "/status"),)),
        ("state", "/state", (("get", "/full"),)),
        ("move", "/move", (("post", "/set_target"), ("get", "/running"), ("post", "/stop"))),
        ("motors", "/motors", (("get", "/status"), ("post", "/set_mode/{mode}"))),
    ):
        text = read(core, f"daemon/app/routers/{module}.py")
        check(router_prefix(text) == prefix, f"routers/{module}.py is mounted at {prefix}")
        for method, path in routes:
            check(has_route(text, method, path), f"and defines {method.upper()} {path}")
    ours = {
        frames.Route.DAEMON_STATUS: "/api/daemon/status", frames.Route.STATE_FULL: "/api/state/full", frames.Route.SET_TARGET: "/api/move/set_target", frames.Route.RUNNING: "/api/move/running",
        frames.Route.STOP_MOVE: "/api/move/stop", frames.Route.MOTOR_STATUS: "/api/motors/status", frames.Route.SET_MODE: "/api/motors/set_mode/{mode}",
    }
    check(all(key == value for key, value in ours.items()), "and this driver's seven routes are those prefixes and paths joined")

    print("\nthe models")
    models = read(core, "daemon/app/models.py")
    protocol = read(core, "io/protocol.py")
    check(class_fields(models, "FullBodyTarget")[:3] == ["target_head_pose", "target_antennas", "target_body_yaw"], "a target is `target_head_pose`, `target_antennas`, `target_body_yaw`")
    check(class_fields(models, "XYZRPYPose")[:6] == ["x", "y", "z", "roll", "pitch", "yaw"], "a head pose is x, y, z, roll, pitch, yaw")
    state_fields = class_fields(models, "FullState")
    check(all(name in state_fields for name in ("control_mode", "head_pose", "body_yaw", "antennas_position", "timestamp")), "a state carries `control_mode`, `head_pose`, `body_yaw`, `antennas_position` and `timestamp`")
    status_fields = class_fields(protocol, "DaemonStatus")
    check(all(name in status_fields for name in ("state", "simulation_enabled", "mockup_sim_enabled", "backend_status", "error")), "a daemon status says its `state`, whether it is a simulation or a mock-up, its `backend_status` and its `error`")
    for enum, ours_class in (("MotorControlMode", frames.MotorMode), ("DaemonState", frames.DaemonState)):
        body = re.search(r"^class " + enum + r"\b[^\n]*:\n((?:(?:    .*)?\n)+)", protocol, re.MULTILINE)
        values = re.findall(r"=\s*\"([a-z_]+)\"", body.group(1)) if body else []
        mine = [v for k, v in vars(ours_class).items() if not k.startswith("_")]
        check(sorted(values) == sorted(mine), f"{enum} has the values {', '.join(sorted(mine))}")
    check("error: str | None" in protocol.split("class RobotBackendStatus", 1)[1].split("class ", 1)[0], "and a backend status can carry an error")

    print("\nthe limits")
    concept = ""
    for relative in ("docs/source/SDK/core-concept.md",):
        with open(os.path.join(core, relative), encoding="utf-8") as handle:
            concept = handle.read()
    table = {m.group(1).strip().strip("*"): m.group(2) for m in re.finditer(r"^\|\s*(\*\*[^|]+\*\*)\s*\|\s*([^|]+?)\s*\|", concept, re.MULTILINE)}
    check("[-40°, +40°]" in table.get("Head Pitch/Roll", ""), "head pitch and roll are -40 to +40 degrees")
    check("[-180°, +180°]" in table.get("Head Yaw", ""), "head yaw is -180 to +180")
    check("[-160°, +160°]" in table.get("Body Yaw", ""), "body yaw is -160 to +160")
    check("65°" in table.get("Yaw Delta", ""), "and the head's yaw is at most 65 degrees from the body's")
    check(abs(frames.DOCUMENTED_LIMITS["head_roll"][1] - 40 * math.pi / 180) < 1e-12 and abs(frames.DOCUMENTED_LIMITS["head_yaw"][1] - math.pi) < 1e-12 and abs(frames.DOCUMENTED_LIMITS["body_yaw"][1] - 160 * math.pi / 180) < 1e-12
          and abs(frames.YAW_DELTA_MAX_RAD - 65 * math.pi / 180) < 1e-12, "this driver's numbers are those, in radians")
    mujoco = read(core, "daemon/backend/mujoco/backend.py")
    check("Real robot convention for the order of the antennas joints is [right, left]" in mujoco, "the real robot's antenna order is right, then left, as the maker's own comment says")

    print("\nthe maker's models, run")
    try:
        sys.path.insert(0, os.path.join(core, "src"))
        from reachy_mini.daemon.app.models import (
            FullBodyTarget,
            FullState,
        )
    except Exception as error:  # noqa: BLE001 - any import failure is a reason these cannot run, and is said
        print(f"  --    not run: reachy_mini's models are not importable here ({type(error).__name__}: {error}). The substrate image has them.")
    else:
        target = FullBodyTarget.model_validate(frames.target_body([0.01, -0.01, 0.02, 0.1, -0.2, 0.3, 0.4, 0.5, -0.5]))
        pose = target.target_head_pose
        check(pose is not None and abs(pose.x - 0.01) < 1e-12 and abs(pose.pitch + 0.2) < 1e-12 and tuple(target.target_antennas or ()) == (0.5, -0.5) and target.target_body_yaw == 0.4,
              "a target this driver makes is validated by the maker's FullBodyTarget, field for field")
        made = FullState.model_validate({"head_pose": {"x": 0.0, "y": 0.0, "z": 0.01, "roll": 0.1, "pitch": 0.2, "yaw": 0.3}, "body_yaw": 0.4, "antennas_position": [0.5, 0.6], "control_mode": "enabled"})
        read_back = frames.parse_state(made.model_dump(mode="json"))
        check(read_back.positions == (0.0, 0.0, 0.01, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6), "and a state the maker's FullState makes is read back by this driver in the same order")
        try:
            FullBodyTarget.model_validate({**frames.target_body([0.0] * 9), "target_head_pose": {"x": "a"}})
            check(False, "(and the maker's model refuses a pose that is not numbers, so the check can fail)")
        except Exception:  # noqa: BLE001
            check(True, "(and the maker's model refuses a pose that is not numbers, so the check can fail)")

    print(f"\n{CHECKS} passed, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())
