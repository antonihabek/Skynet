"""Motor name to canonical role, per LeRobot robot class.

§67.2: *"SO-100/101 motor names line up with the canonical vocabulary almost one to one… A
hand-written mapping table per LeRobot robot class is a few dozen lines and unlocks cross-embodiment
transport for the whole fleet. This is the cheapest large win available."*

Two things about these tables are worth saying out loud, because both are places where a reader
could reasonably assume more than is there.

**The side is a convention, not a fact.** An SO-101 is a single arm. Nothing about it is right or
left, and the specification's own worked example writes ``right_arm``, so that is what these tables
use. On a bimanual rig one of the two arms is wrong, and :func:`roles_for` takes an override for
exactly that case. The bridge reports which table it used and whether anything was overridden, so a
mis-mapped arm is visible in the report rather than in the motion.

**An unmapped motor gets no role, and that is a working state.** §5.1: a joint with an unparseable
or absent role can only ever be matched by *exact name*, and it never silently maps to something
plausible. A body with no roles still runs — it just cannot participate in remapping, which is
truthful, because nobody has established what its joints mean.
"""

from __future__ import annotations

__all__ = ["ROLE_TABLES", "roles_for", "known_classes"]


#: The SO-100 and SO-101 follower arms, straight from §67.2.
_SO_ARM = {
    "shoulder_pan": "right_arm.shoulder.yaw",
    "shoulder_lift": "right_arm.shoulder.pitch",
    "elbow_flex": "right_arm.elbow.flex",
    "wrist_flex": "right_arm.wrist.flex",
    "wrist_roll": "right_arm.wrist.roll",
    "gripper": "right_hand.gripper.grip",
}

#: Koch v1.1 follower. The same six degrees of freedom under Dynamixel names; `elbow_flex` and the
#: wrist pair match the SO arms, and `shoulder_pan`/`shoulder_lift` are spelled identically.
_KOCH = dict(_SO_ARM)

#: LeKiwi is an SO-101 arm on a three-wheel omnidirectional base. The wheels are `base.wheel#0..2`
#: rather than anything more specific: the vocabulary has no way to say "120° apart", and the
#: transport planner does not need it to — a `base_velocity` channel is synthesised against the base
#: kind, not against individual wheels.
_LEKIWI = dict(_SO_ARM)
_LEKIWI.update(
    {
        "left_wheel": "base.wheel#0",
        "back_wheel": "base.wheel#1",
        "right_wheel": "base.wheel#2",
    }
)

#: HopeJR's arm. A larger name set than the SO arms and the same shape, with an extra shoulder axis.
_HOPE_JR_ARM = {
    "shoulder_pitch": "right_arm.shoulder.pitch",
    "shoulder_yaw": "right_arm.shoulder.yaw",
    "shoulder_roll": "right_arm.shoulder.roll",
    "elbow_flex": "right_arm.elbow.flex",
    "wrist_roll": "right_arm.wrist.roll",
    "wrist_yaw": "right_arm.wrist.yaw",
    "wrist_pitch": "right_arm.wrist.pitch",
}

#: Every table, keyed by the lowercase LeRobot robot class or `robot_type` string.
#:
#: A class that is not here is not a failure — it is a body whose joints nobody has established the
#: meaning of. `roles_for` returns nothing for it and the bridge says so in as many words.
ROLE_TABLES: dict[str, dict[str, str]] = {
    "so100_follower": _SO_ARM,
    "so101_follower": _SO_ARM,
    "so100": _SO_ARM,
    "so101": _SO_ARM,
    "koch_follower": _KOCH,
    "koch": _KOCH,
    "lekiwi": _LEKIWI,
    "lekiwi_client": _LEKIWI,
    "hope_jr_arm": _HOPE_JR_ARM,
    "hopejr_arm": _HOPE_JR_ARM,
}


def known_classes() -> list[str]:
    """The robot classes with a mapping table, sorted."""
    return sorted(ROLE_TABLES)


def roles_for(
    robot_class: str,
    motors: list[str],
    overrides: dict[str, str] | None = None,
) -> tuple[dict[str, str | None], str]:
    """Roles for these motors, and one sentence saying where they came from.

    Returns ``({motor: role or None}, provenance)``. Every motor appears, including the ones with no
    role — "six joints, four mapped" is the number a reader needs and a filtered dict hides the two.

    ``overrides`` wins over the table and is how a bimanual rig fixes the side, or how a vendor names
    a joint the table has never seen. An override that is not a motor on this body is ignored and
    named in the provenance, because silently accepting one is how a typo becomes an unmapped joint
    nobody can account for.
    """
    table = ROLE_TABLES.get(robot_class.lower(), {})
    overrides = overrides or {}

    mapped: dict[str, str | None] = {}
    for motor in motors:
        if motor in overrides:
            mapped[motor] = overrides[motor] or None
        else:
            mapped[motor] = table.get(motor)

    hit = sum(1 for role in mapped.values() if role)
    stray = sorted(set(overrides) - set(motors))

    if table:
        provenance = (
            f"{hit} of {len(motors)} motors mapped from the `{robot_class}` table. The side is a "
            "convention: an SO-style arm has no left or right, and the specification's worked "
            "example uses right_arm."
        )
    else:
        provenance = (
            f"no mapping table exists for `{robot_class}`, so {hit} of {len(motors)} motors carry a "
            "role and the rest carry none. A joint with no role can only ever be matched by exact "
            "name, which is truthful — nobody has established what these joints mean. Supply "
            "role_overrides, or add a table: it is a few dozen lines."
        )
    if overrides:
        provenance += f" {len(overrides)} role(s) overridden by configuration."
    if stray:
        provenance += (
            f" Ignored {len(stray)} override(s) naming motors this body does not have: "
            f"{', '.join(stray)}."
        )

    return mapped, provenance
