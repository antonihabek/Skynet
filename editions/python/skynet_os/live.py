"""§54.2's live precondition context, assembled — Python edition.

Transcribed from ``reference/typescript/live.ts``. Every rule and message string is read from that
file, never from ``corpus/``.

§54.2 turns ``SkillManifest.preconditions`` from prose into predicates and then names the seven
branches those predicates may read::

    robot.state.*        joint positions, gripper, battery percent, base pose
    robot.guards.active  which guards are actually running (§7.2)
    robot.calibration.*  which parameter sets are valid, and their age (§45)
    robot.frames.*       whether the frames the skill needs resolve (§44)
    robot.tier           guest | host | certified
    skill.grants.*       capability tokens the operator granted this package
    env.*                operator-declared facts about the cell

── The rule that decides every field: absent is not null ────────────────────

§54.2's first rule is the one with teeth:

    **A precondition that cannot be evaluated refuses the skill**, naming the path that was
    unresolvable. Not "assume true", which is what free-text preconditions do by construction.

So a fact this robot does not know must be **absent** from the context, never present as ``null``
and never present as a plausible zero. A robot with no battery sensor reporting
``robot.state.battery_percent = 0`` would refuse every skill; one reporting ``100`` would run every
skill; one that omits the path refuses only the skills that asked, and names the path in the
refusal. Only the third is honest, and only the third is useful.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``if (anyJoint) state.joints = joints``
    A flag rather than ``len(joints) > 0``, and they are the same here only because a joint's name
    is its key. The flag is what the reference wrote and it is the one that stays correct if two
    joints ever share a name.

``if (pose)``
    Truthy on an object, so a ``base_pose`` of ``null`` publishes nothing and raises nothing —
    distinct from a pose whose numbers are not finite, which lands in ``unreadable``. ``is not
    None`` would agree; ``if pose:`` in Python would **not**, because an empty dict is falsy and
    ``{}`` here should reach the finite check and be reported unreadable.

``if (inputs.guards_active)``
    Truthy again, and this one carries the whole module in a field. An empty list is a kernel
    reporting **no guard is running**; absent is nobody having asked a kernel. JavaScript's ``[]``
    is truthy and Python's is not, so ``if inputs.get("guards_active"):`` would silently drop the
    first case — the one §33.1 exists to make observable.

``typeof inputs.tier === 'string'``
    A type test, not a truthiness test: an empty-string tier is published, because a tier of ``""``
    is a value somebody supplied and §54.2's rule is about facts nobody knows.

``new Set(values)`` then ``.sort()``
    Insertion order then UTF-16 order; a Python ``set`` is neither. Deduplication is done with a
    dict and the sort applied after.

``Number.isFinite``
    Rejects ``NaN`` and the infinities, which ``isinstance(x, float)`` does not, and booleans, which
    ``isinstance(True, int)`` lets through. A gripper of ``true`` is not a gripper at 1.
"""

from __future__ import annotations

import math
from typing import Any

from .js import js_trim as _js_trim, utf16_key as _utf16_key

__all__ = ["context_to_json", "explain_omissions", "live_context"]


def _addressable(key: Any) -> bool:
    """A key a dot path can address: non-empty, no dot, and no surrounding whitespace.

    The same rule as :func:`skynet_os.cell._usable_key`, **deliberately duplicated rather than
    shared**, because they are two rules that happen to agree: this one is about a name the *body*
    chose and that one is about a name the *operator* chose, and the two could reasonably diverge
    about whitespace without either being wrong.
    """
    if not isinstance(key, str):
        return False
    return len(key) > 0 and "." not in key and _js_trim(key) == key


def _finite(value: Any) -> bool:
    """``typeof value === 'number' && Number.isFinite(value)``."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return False
    number = float(value)
    return not (math.isnan(number) or math.isinf(number))


def _str_list(values: Any) -> list[str]:
    """``[...new Set(values)].sort()`` — deduplicated, then in UTF-16 order."""
    seen: dict[Any, None] = {}
    for value in values or []:
        seen.setdefault(value, None)
    return sorted(seen, key=lambda value: _utf16_key(value) if isinstance(value, str) else ())


def live_context(inputs: dict[str, Any]) -> dict[str, Any]:
    """Build §54.2's context.

    Every branch is omitted rather than emitted empty when nothing is known about it, with one
    deliberate exception: ``skill.grants.tokens`` is emitted as an empty array.

    The reason is that a grant is a **file the daemon owns**. "This package was granted nothing" is
    an answer the OS always has, and a precondition should be able to refuse on it and say so. Every
    other branch is something the OS learns from somewhere else — a driver, a kernel, a clock, an
    operator's survey of a room — and any of them can be missing, at which point §54.2's first rule
    is the honest answer and an empty array is a claim.
    """
    unaddressable: list[str] = []
    unreadable: list[str] = []

    # ── robot.state ──────────────────────────────────────────────────────────
    joints: dict[str, Any] = {}
    velocity: dict[str, Any] = {}
    effort: dict[str, Any] = {}
    any_joint = False
    any_velocity = False
    any_effort = False
    for reading in inputs.get("joints") or []:
        name = reading.get("name")
        if not _addressable(name):
            unaddressable.append(name)
            continue
        # Velocity and effort are read **before** the position check, and separately from it,
        # because they are separate facts about the same joint: a joint whose encoder is dead and
        # whose drive still reports current has an unreadable position and a readable effort, and
        # folding them would lose the second. A non-finite one is simply absent — it is not added to
        # `unreadable`, which names the joints whose *position* could not be read.
        if _finite(reading.get("velocity")):
            velocity[name] = reading["velocity"]
            any_velocity = True
        if _finite(reading.get("effort")):
            effort[name] = reading["effort"]
            any_effort = True
        if not _finite(reading.get("position")):
            unreadable.append(name)
            continue
        joints[name] = reading["position"]
        any_joint = True

    state: dict[str, Any] = {}
    if any_joint:
        state["joints"] = joints
    if any_velocity:
        state["velocity"] = velocity
    if any_effort:
        state["effort"] = effort
    if _finite(inputs.get("gripper")):
        state["gripper"] = inputs["gripper"]
    if _finite(inputs.get("battery_percent")):
        state["battery_percent"] = inputs["battery_percent"]
    pose = inputs.get("base_pose")
    # `if (pose)` in the reference: an object is truthy and `null` is not. Written as a `None` test
    # because an empty object must reach the finite check below and be reported unreadable, which is
    # what `if pose:` in Python would skip.
    if pose is not None:
        xyz = list(pose.get("xyz") or [])
        quaternion = list(pose.get("quaternion") or [])
        if (
            len(xyz) == 3
            and len(quaternion) == 4
            and all(_finite(value) for value in xyz)
            and all(_finite(value) for value in quaternion)
        ):
            state["base_pose"] = {
                "x": xyz[0],
                "y": xyz[1],
                "z": xyz[2],
                "qx": quaternion[0],
                "qy": quaternion[1],
                "qz": quaternion[2],
                "qw": quaternion[3],
            }
        else:
            unreadable.append("base_pose")
    # §54.2's table says `robot.state.*` and then lists four examples; §27's state is inside that
    # branch and is the fact a precondition most obviously wants — *"only when the robot is
    # running"* is the first precondition anybody writes. It is published under its own name rather
    # than inside a status object so the path stays stable if §27 ever grows a second field.
    if inputs.get("operating_state"):
        state["operating"] = inputs["operating_state"]
    # §24's flags. Absent as a group when nobody read a driver: an `estop: false` from a caller that
    # has not read one would be a claim rather than an observation, which is the same distinction
    # `robot.guards.active` draws one branch along.
    drive = inputs.get("drive")
    if drive is not None:
        state["estop"] = bool(drive.get("estop"))
        state["degraded"] = bool(drive.get("degraded"))
        state["braked"] = bool(drive.get("braked"))
        state["homing"] = drive.get("homing") or ""
        # A **zero** is published — the one field in this module where the rule points the other
        # way, because zero is what a healthy drive reports and is therefore a measurement.
        if _finite(drive.get("fault_code")):
            state["fault_code"] = drive["fault_code"]

    # ── robot.guards ─────────────────────────────────────────────────────────
    guards: dict[str, Any] = {}
    # Truthy on a list, and an **empty list is truthy in JavaScript**. That is the distinction this
    # field exists for: `[]` is a kernel reporting that no guard is running, and absent is nobody
    # having asked a kernel — §33.1 keeps `skynetd` on the far side of that line. A Python truth
    # test would collapse the two and lose the observation.
    if inputs.get("guards_active") is not None:
        guards["active"] = _str_list(inputs["guards_active"])
    if len(inputs.get("guards_unavailable") or []) > 0:
        guards["unavailable"] = _str_list(inputs.get("guards_unavailable") or [])

    # ── robot.calibration ────────────────────────────────────────────────────
    calibration: dict[str, Any] = {}
    any_calibration = False
    for fact in inputs.get("calibration") or []:
        parameter_set = fact.get("set")
        if not _addressable(parameter_set):
            unaddressable.append(parameter_set)
            continue
        validity = fact.get("validity") or {}
        row: dict[str, Any] = {
            "valid": validity.get("state") == "valid",
            "state": validity.get("state"),
        }
        if _finite(fact.get("age_s")):
            row["age_s"] = fact["age_s"]
        calibration[parameter_set] = row
        any_calibration = True

    # ── robot ────────────────────────────────────────────────────────────────
    robot: dict[str, Any] = {}
    if len(state) > 0:
        robot["state"] = state
    if len(guards) > 0:
        robot["guards"] = guards
    if any_calibration:
        robot["calibration"] = calibration
    frames: dict[str, Any] = {}
    if len(inputs.get("frames_resolved") or []) > 0:
        frames["resolved"] = _str_list(inputs.get("frames_resolved") or [])
    fault = inputs.get("frames_fault")
    if fault:
        frames["fault"] = {
            "code": fault.get("code"),
            "detail": fault.get("detail"),
            "subjects": _str_list(fault.get("subjects") or []),
        }
    if len(frames) > 0:
        robot["frames"] = frames
    if isinstance(inputs.get("tier"), str):
        robot["tier"] = inputs["tier"]

    # ── skill.grants ─────────────────────────────────────────────────────────
    #
    # A list rather than an object, and the reason is a dot. `cell.ts` found, from the other side,
    # that a fact key containing one is a key no precondition can address — and a **capability token
    # contains a dot by construction** (`arm.left`, `sensing.camera`), so `skill.grants.<token>` is
    # unaddressable for every token that exists and nobody can rename them. The shape changes rather
    # than the names.
    skill = {"grants": {"tokens": _str_list(inputs.get("grants") or [])}}

    context: dict[str, Any] = {"robot": robot, "skill": skill}
    env = inputs.get("env")
    if env:
        context["env"] = env

    return {
        "context": context,
        "unaddressable": _str_list(unaddressable),
        "unreadable": _str_list(unreadable),
    }


def explain_omissions(built: dict[str, Any]) -> list[str]:
    """What a console shows about a context it could not fully build.

    One sentence per omission, naming the path that will be unresolvable, because §54.2's refusal
    names a path and an operator has to be able to get from that path back to the reason.
    """
    lines: list[str] = []
    for name in built["unaddressable"]:
        lines.append(
            f"`{name}` carries a dot, so no precondition can address it — a dot path would read "
            "the name as two keys. Nothing can be done about that here: the body chose the name."
        )
    for name in built["unreadable"]:
        lines.append(
            f"`{name}` did not report a finite value, so it is absent rather than published. §54.2 "
            "refuses a precondition that names it, which is the honest answer to a sensor "
            "reporting NaN."
        )
    return lines


# ── The wire shapes ──────────────────────────────────────────────────────────


def context_to_json(case_input: dict[str, Any]) -> Any:
    """``live/context`` — §54.2's context, with the two omission lists beside it.

    Beside, because a path under ``robot.`` describing the context itself would be a path a
    precondition could read — and a skill whose precondition asserts something about the
    completeness of its own evaluation context is a skill asserting something about the OS rather
    than about the robot.
    """
    built = live_context(case_input)
    return {
        "context": built["context"],
        "unaddressable": list(built["unaddressable"]),
        "unreadable": list(built["unreadable"]),
    }
