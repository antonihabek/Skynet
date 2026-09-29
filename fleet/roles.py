"""Accepted role tables for the arms whose joints the importer cannot name — item 10.8, F165.

§6's transport binds a skill's channel to a joint of the target body by exact name, or by canonical
role (§5.1) — and a role is a human's assertion (§63.4), never a guess. The importer can suggest one
from a joint's *words* and, since F166, from its axis; it cannot suggest one for `joint1` … `joint7`,
which is how xArm, Kinova, FANUC, ABB, Franka and most of the arms people own name their joints, and
it cannot say that a UR's `elbow_joint` is what a LeRobot policy calls `elbow_flex` (the vocabulary
has both words, and nothing says they are one joint). So a skill trained on one arm bound to almost
no other, and the report said so: 18 rows bound, none of them a numbered arm.

This module is the integrator's half of that, written down once instead of once per robot: **one
table per arm family**, in the convention LeRobot's own bridge uses for the SO-100/101
(`drivers/lerobot/skynet_lerobot/roles.py`, §67.2) so that the skill most of the community trains
transports to them. The alternative — teaching the importer to guess from the joint's place in a
chain — was refused, because a wrong guess binds a policy's channel to the wrong joint without
failing (`skynet descriptor from-urdf` says so in its own output).

**What makes a table safe to apply is that it is checked, and the check is not this file's.** Each
pattern carries geometric relations between its joints — the yaw joint's axis is vertical, the elbow
is parallel to the shoulder's pitch, the roll is perpendicular to the joint before it — and each is
tested against the axes *MuJoCo* computes for the body at its zero pose (`data.xaxis`), which is
the physics engine's own reading of the maker's file. A table that does not hold is **refused, by
name, and applied to nothing**: the Jaco arms with the 60° wrist, whose fifth axis is not
perpendicular to the fourth, are the rows that show it.

**What none of it is.** It is not the maker's word, and no maker has been asked. It says which joint
of this body plays the part a LeRobot policy's `elbow_flex` plays, on the evidence that the geometry
is the geometry of an arm whose joints are in that order. Binding is not working: a policy trained
on a five-degree-of-freedom arm holds the other joints of a seven-degree-of-freedom one (the plan
says so as `unmapped_roles`), and whether the skill still does anything is §59.4's research result,
which nobody has.
"""

from __future__ import annotations

import math
from typing import Any

YAW = "right_arm.shoulder.yaw"
PITCH = "right_arm.shoulder.pitch"
ELBOW = "right_arm.elbow.flex"
WRIST_FLEX = "right_arm.wrist.flex"
WRIST_ROLL = "right_arm.wrist.roll"
GRIP = "right_hand.gripper.grip"

# A pattern is the arm's joints in order from the base, each `(role, relation)`. A role of `None` is
# a joint that has a place in the chain and no counterpart in the SO-101's five — the second wrist
# axis of a UR, the roll of a forearm — and is held by the controller when a skill is transported to
# it. A relation is what must be true of that joint's axis at the zero pose, in the world frame:
#
#   vertical / horizontal      against the base's up axis, which is +z in every description here
#   parallel:<role>            with the axis of the joint given that role (either sense)
#   perpendicular:<role>       likewise, at a right angle
#   perpendicular_prev         at a right angle to the joint immediately before
_SO5 = [
    (YAW, "vertical"),
    (PITCH, "horizontal"),
    (ELBOW, f"parallel:{PITCH}"),
    (WRIST_FLEX, f"parallel:{PITCH}"),
    (WRIST_ROLL, "perpendicular_prev"),
    (GRIP, None),
]
# An arm with no wrist roll — the OpenMANIPULATOR-X. Its skill transport is refused, and correctly:
# the policy requires a roll, and the report now says the body has none rather than that it has no
# roles.
_FOUR = _SO5[:4]
# Universal Robots' layout, and every arm built like it: three parallel axes (shoulder, elbow, first
# wrist), a second wrist axis at a right angle, and the flange's roll.
_SIX_PARALLEL_WRIST = [
    (YAW, "vertical"),
    (PITCH, "horizontal"),
    (ELBOW, f"parallel:{PITCH}"),
    (WRIST_FLEX, f"parallel:{PITCH}"),
    (None, f"perpendicular:{PITCH}"),
    (WRIST_ROLL, "perpendicular_prev"),
]
# The industrial layout — ABB, FANUC, KUKA, Stäubli, DENSO, Kawasaki, xArm 6 — with a roll of the
# forearm between the elbow and a wrist that bends at a right angle to it.
_SIX_ROLL_MID = [
    (YAW, "vertical"),
    (PITCH, "horizontal"),
    (ELBOW, f"parallel:{PITCH}"),
    (None, f"perpendicular:{PITCH}"),
    (WRIST_FLEX, "perpendicular_prev"),
    (WRIST_ROLL, "perpendicular_prev"),
]
# The seven-degree-of-freedom layout — Franka, iiwa, Kinova Gen3, xArm 7, Flexiv, Sawyer: a roll of
# the upper arm after the shoulder and another of the forearm after the elbow.
_SEVEN = [
    (YAW, "vertical"),
    (PITCH, "horizontal"),
    (None, f"perpendicular:{PITCH}"),
    (ELBOW, f"parallel:{PITCH}"),
    (None, f"perpendicular:{PITCH}"),
    (WRIST_FLEX, f"parallel:{PITCH}"),
    (WRIST_ROLL, "perpendicular_prev"),
]

PATTERNS: dict[str, list[tuple[str | None, str | None]]] = {
    "so5": _SO5,
    "four_no_roll": _FOUR,
    "six_parallel_wrist": _SIX_PARALLEL_WRIST,
    "six_roll_mid": _SIX_ROLL_MID,
    "seven": _SEVEN,
}


def _numbered(prefix: str, count: int) -> list[str]:
    return [f"{prefix}{index}" for index in range(1, count + 1)]


_UR = ["shoulder_pan_joint", "shoulder_lift_joint", "elbow_joint", "wrist_1_joint", "wrist_2_joint", "wrist_3_joint"]
_TROSSEN = ["waist", "shoulder", "elbow", "forearm_roll", "wrist_angle", "wrist_rotate"]

#: description id → (pattern, the arm's joints in order from the base, by the body's own names).
#: A gripper joint follows the arm's when the pattern has a place for one. Written by hand, one
#: line per description, from each file's own joint names.
ARMS: dict[str, tuple[str, list[str]]] = {}


def _family(pattern: str, joints: list[str], *ids: str) -> None:
    for description in ids:
        ARMS[description] = (pattern, joints)


# ── the SO-100/101 convention itself, and the arms built like it ──
_family("so5", _numbered("", 6), "so_arm100_description", "so_arm101_description", "so_arm101_mj_description")
_family("so5", ["Rotation", "Pitch", "Elbow", "Wrist_Pitch", "Wrist_Roll", "Jaw"], "so_arm100_mj_description")
_family(
    "so5",
    ["base_rotation", "pitch", "elbow", "wrist_pitch", "wrist_roll", "gripper"],
    "low_cost_robot_arm_mj_description",
)
_family("so5", _numbered("joint", 5) + ["gripper_joint_1"], "omx_f_description")
_family("four_no_roll", _numbered("joint", 4), "open_manipulator_x_description")

# ── three parallel axes and a second wrist axis: Universal Robots, and the arms built like a UR ──
_family(
    "six_parallel_wrist",
    _UR,
    "ur3_official_description", "ur3e_description", "ur5_official_description", "ur5e_description",
    "ur5e_mj_description", "ur7e_description", "ur8long_description", "ur10_official_description",
    "ur10e_description", "ur10e_mj_description", "ur12e_description", "ur15_description",
    "ur16e_description", "ur18_description", "ur20_description", "ur30_description", "skynet:ur5e_live.urdf",
)
_family(
    "six_parallel_wrist",
    _numbered("joint", 6),
    "z1_description", "z1_mj_description", "yam_description", "yam_mj_description",
    "arx_l5_mj_description", "omy_3m_description", "omy_f3m_description", "omy_l100_description",
)
_family("six_parallel_wrist", _numbered("joint_", 6), "skynet:techman_tm7s_mock.urdf")
_family(
    "six_parallel_wrist",
    ["shoulder_joint", "upperArm_joint", "foreArm_joint", "wrist1_joint", "wrist2_joint", "wrist3_joint"],
    "skynet:aubo_i5s_mock.urdf",
)

# ── a roll of the forearm between the elbow and the wrist: the industrial layout ──
_family(
    "six_roll_mid",
    _numbered("joint_", 6),
    "fanuc_m710ic_description", "edo_description", "gen3_lite_description",
    "skynet:abb_irb120_mock.urdf", "skynet:abb_irb1200_egm.urdf", "skynet:denso_vs060_mock.urdf",
    "skynet:doosan_m0609_mock.urdf", "skynet:doosan_m0617_mock.urdf", "skynet:doosan_m1013_mock.urdf",
    "skynet:doosan_m1509_mock.urdf", "skynet:doosan_a0509_mock.urdf", "skynet:doosan_a0912_mock.urdf",
    "skynet:doosan_e0509_mock.urdf", "skynet:doosan_h2017_mock.urdf", "skynet:doosan_h2515_mock.urdf",
    "skynet:fanuc_lrmate200id_mock.urdf", "skynet:kuka_kr6r900_rsi.urdf", "skynet:staubli_tx260l_mock.urdf",
)
_family(
    "six_roll_mid",
    _numbered("joint", 6),
    "xarm6_description", "piper_description", "piper_mj_description", "menagerie:ufactory_lite6/lite6.xml",
    "skynet:kawasaki_rs007l_mock.urdf",
)
_family("six_roll_mid", _numbered("rv4fr_joint_", 6), "skynet:mitsubishi_rv4fr_mock.urdf")
_family("six_roll_mid", _TROSSEN, "viper_mj_description", "widow_mj_description")
# Kinova's JACO2. The straight-wrist arms (`s`) hold; the ones with a 60° wrist (`n`) do not, and
# are listed so the report says why rather than leaving them unmentioned.
for _arm in ("j2s6s200", "j2s6s300", "j2n6s200", "j2n6s300"):
    _family("six_roll_mid", _numbered(f"{_arm}_joint_", 6), f"{_arm}_description")
_family("six_roll_mid", _numbered("j2s6s200_joint_", 6), "gen2_description")

# ── seven degrees of freedom ──
_family(
    "seven",
    _numbered("joint", 7),
    "enlight_description", "rizon10_description", "rizon4_description", "rizon4_mj_description",
    "xarm7_description", "xarm7_mj_description", "iiwa14_mj_description", "panda_mj_description",
)
_family("seven", _numbered("fer_joint", 7), "fer_description")
_family("seven", _numbered("fr3_joint", 7), "fr3_description", "fr3_mj_description")
_family("seven", _numbered("fr3v2_1_joint", 7), "fr3_v2_1_description")
_family("seven", _numbered("fr3v2_joint", 7), "fr3_v2_description", "fr3_v2_mj_description")
_family("seven", _numbered("panda_joint", 7), "panda_description")
_family("seven", _numbered("iiwa_joint_", 7), "iiwa14_description", "iiwa7_description")
_family("seven", _numbered("joint_", 7), "gen3_description", "gen3_mj_description", "skynet:kinova_gen3_mock.urdf")
_family("seven", _numbered("j2s7s300_joint_", 7), "j2s7s300_description")
_family("seven", _numbered("j2n7s300_joint_", 7), "j2n7s300_description")
_family("seven", [f"right_j{index}" for index in range(7)], "sawyer_mj_description")

#: How far off, in degrees, an axis may be from a relation and still hold it. A file's own rounding is
#: far inside either figure, and a Jaco's 30° tilt is far outside.
PARALLEL_DEG = 8.0
PERPENDICULAR_DEG = 6.0


def _cosine(a: tuple[float, float, float], b: tuple[float, float, float]) -> float:
    dot = sum(x * y for x, y in zip(a, b))
    norm = math.sqrt(sum(x * x for x in a)) * math.sqrt(sum(y * y for y in b))
    return dot / norm if norm else 0.0


def _between_lines(cosine: float) -> float:
    """The angle between two axes taken as lines, in degrees, 0 (parallel) to 90 (perpendicular)."""
    return math.degrees(math.acos(min(1.0, abs(cosine))))


def zero_pose_axes(scene: str) -> dict[str, tuple[int, tuple[float, float, float]]]:
    """Every hinge and slide joint of a scene: `name → (index in the model, world axis at zero)`.

    MuJoCo's own `data.xaxis` after `mj_forward` at `qpos0` — the axis of each joint in the world
    frame with every joint at its reference position, which is the pose the maker's file describes.
    """
    import mujoco

    model = mujoco.MjModel.from_xml_path(scene)
    data = mujoco.MjData(model)
    mujoco.mj_forward(model, data)
    axes: dict[str, tuple[int, tuple[float, float, float]]] = {}
    for index in range(model.njnt):
        if int(model.jnt_type[index]) not in (int(mujoco.mjtJoint.mjJNT_HINGE), int(mujoco.mjtJoint.mjJNT_SLIDE)):
            continue
        name = mujoco.mj_id2name(model, mujoco.mjtObj.mjOBJ_JOINT, index)
        x, y, z = (float(component) for component in data.xaxis[index])
        axes[name] = (index, (x, y, z))
    return axes


def _holds(
    relation: str,
    axis: tuple[float, float, float],
    placed: dict[str, tuple[float, float, float]],
    previous: tuple[float, float, float],
) -> tuple[bool, str, str]:
    """Whether one relation holds, in what words it was asked, and how far off it was."""
    kind, _, target = relation.partition(":")
    if kind in ("vertical", "horizontal"):
        angle = _between_lines(_cosine(axis, (0.0, 0.0, 1.0)))
        if kind == "vertical":
            return angle <= PARALLEL_DEG, "vertical (the base's up axis is +z)", f"{angle:.0f}° from vertical"
        return 90 - angle <= PERPENDICULAR_DEG, "horizontal (the base's up axis is +z)", f"{90 - angle:.0f}° from horizontal"
    against = "the joint before it" if kind == "perpendicular_prev" else f"the joint asserted {target}"
    angle = _between_lines(_cosine(axis, previous if kind == "perpendicular_prev" else placed[target]))
    if kind == "parallel":
        return angle <= PARALLEL_DEG, f"parallel to {against}", f"{angle:.0f}° from parallel"
    return 90 - angle <= PERPENDICULAR_DEG, f"perpendicular to {against}", f"{90 - angle:.0f}° from perpendicular"


def judge(description: str, axes: dict[str, tuple[int, tuple[float, float, float]]]) -> dict[str, Any]:
    """Apply a description's table to the body's joint axes: accepted with its roles, or refused with why."""
    if description not in ARMS:
        return {"outcome": "not_applicable", "detail": "no accepted table for this description"}
    pattern_name, names = ARMS[description]
    pattern = PATTERNS[pattern_name]

    def refused(detail: str) -> dict[str, Any]:
        return {"outcome": "refused", "table": pattern_name, "detail": detail}

    if len(names) > len(pattern):
        return refused(f"the table lists {len(names)} joints and its pattern has {len(pattern)} places")
    missing = [name for name in names if name not in axes]
    if missing:
        return refused(
            f"{', '.join(missing)} {'is' if len(missing) == 1 else 'are'} not among this body's movable joints, "
            f"which are {', '.join(sorted(axes, key=lambda name: axes[name][0]))}"
        )
    indices = [axes[name][0] for name in names]
    if indices != sorted(indices):
        return refused("the listed joints are not in order from the base in the body's own tree")
    between = [name for name, (index, _) in axes.items() if indices[0] < index < indices[-1] and name not in names]
    if between:
        return refused(f"the body has {', '.join(between)} between the arm's first and last joint, which the table omits")

    held = 0
    placed: dict[str, tuple[float, float, float]] = {}
    for place, name in enumerate(names):
        role, relation = pattern[place]
        axis = axes[name][1]
        if role is not None:
            placed[role] = axis
        if relation is None:
            continue
        ok, wanted, found = _holds(relation, axis, placed, axes[names[place - 1]][1] if place else axis)
        if not ok:
            return refused(
                f"{name} is asserted {role or 'a joint of the chain with no counterpart'} and must be {wanted}; at the zero "
                f"pose its axis is ({axis[0]:.2f}, {axis[1]:.2f}, {axis[2]:.2f}), {found}"
            )
        held += 1

    roles = {name: pattern[place][0] for place, name in enumerate(names) if pattern[place][0] is not None}
    return {
        "outcome": "accepted",
        "table": pattern_name,
        "assigned": len(roles),
        "without_counterpart": [name for place, name in enumerate(names) if pattern[place][0] is None],
        "relations_held": held,
        "_roles": roles,
    }


def stage_roles(entry: dict[str, Any], scene: str) -> dict[str, Any]:
    """The `roles` stage: this description's accepted table, checked against the body MuJoCo built."""
    if entry["id"] not in ARMS:
        return {"outcome": "not_applicable", "detail": "no accepted table for this description"}
    return judge(entry["id"], zero_pose_axes(scene))
