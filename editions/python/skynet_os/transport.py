"""§6's transport planner, Python edition.

Transcribed from ``resolveTransport`` and its helpers in ``reference/typescript/embodiment.ts``.
Every rule and message string is read from that file, never from ``corpus/``.

The question this answers is the one that makes a policy portable at all: a policy declares an
action space, a body declares joints, and something has to say how the first reaches the second —
by name, by canonical role, through task space, or not at all. It is pure: no database, no
filesystem, no clock, every input an argument, which is what lets the Hub and a disconnected robot
agree about what a robot will do before it does it.

── Where Python is quietly not JavaScript, here ──────────────────────────────

``Set`` and ``Map`` iterate in insertion order
    Python's ``set`` does not iterate in *any* promised order. Two lists in a plan are built by
    walking a set — ``unmapped`` roles and the retargeted chains — and both are **subjects on a
    warning the corpus compares element by element**. An edition using ``set`` would produce a
    correct plan with its warnings shuffled, differently on different runs. Both use dictionaries
    keyed in insertion order here, and :func:`_ordered_keys` says so at each use.

``toFixed(0)``
    ECMAScript picks the integer nearest the value and, **on a tie, the larger one**. Python's
    ``format`` rounds half to even, so a channel scaled to exactly 62.5% reads ``63%`` in the
    reference and ``62%`` here. :func:`_to_fixed_zero` implements the specification's rule.

strict ``undefined``
    ``channel.peak !== undefined`` decides whether an unscalable channel is *warned about* or
    passed over in silence, and ``source.dof !== undefined`` whether a mismatch is reported at all.
    A key present and null takes the other branch in the reference, and ``dict.get`` collapses them.

``options.allowRetarget === true``
    Strictly ``true``. Any other truthy value — ``1``, ``"yes"`` — does **not** grant retargeting,
    and the warning it produces says *"was not requested"* rather than *"has no solver"*, which is
    a different sentence to whoever reads it.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .canonical import OMIT
from .embodiment import canonical_motion
from .js import to_fixed as _to_fixed
from .roles import parse_role, role_key

__all__ = [
    "TRANSPORT_METHODS",
    "TRANSPORT_VERDICTS",
    "TRANSPORT_WARNING_CODES",
    "derive_verdict",
    "is_installable",
    "plan_to_json",
    "resolve_transport",
]

TRANSPORT_METHODS: Final = ("direct", "remap", "retarget", "synthesize", "drop", "blocked")

#: Every transport warning code, as a value rather than only as a type — a union vanishes at
#: compile time and cannot be compared against another edition's.
TRANSPORT_WARNING_CODES: Final = (
    "no_source_embodiment",
    "unverified_embodiment",
    "no_target_envelope",
    "dof_mismatch",
    "control_mode_unsupported",
    "rate_unsupported",
    "rate_below_watchdog",
    "scaled_down",
    "unscalable_channel",
    "retarget_without_solver",
    "unmapped_roles",
    "observation_missing",
    "observation_degraded",
    "latency_undeclared",
    # §84.3, appended — the order is part of what the corpus holds editions to.
    "action_space_mismatch",
    "action_space_ambiguous",
    "motion_parameter_unused",
)

#: Verdicts, worst to best. ``refused`` is a real refusal: the OS will not install the policy
#: against this body. The other three all run, and the difference is what the operator is told.
TRANSPORT_VERDICTS: Final = ("direct", "retargeted", "degraded", "refused")

_JOINT_KINDS: Final = ("joint_position", "joint_velocity", "joint_torque")


# ── The absence of a field, and ECMAScript's arithmetic ──────────────────────


class _Absent:
    """JavaScript's ``undefined``. Two rules here take opposite branches on it and on ``null``."""

    def __repr__(self) -> str:  # pragma: no cover — diagnostics only
        return "ABSENT"

    def __bool__(self) -> bool:
        return False


_ABSENT: Final = _Absent()


def _field(obj: Any, key: str) -> Any:
    """``obj?.key`` — :data:`_ABSENT` when the object is missing or the key is not present."""
    if not isinstance(obj, dict):
        return _ABSENT
    return obj[key] if key in obj else _ABSENT


def _or_none(value: Any) -> Any:
    """``value ?? null``."""
    return None if value is _ABSENT or value is None else value


def _ordered_keys(seen: dict[str, Any]) -> list[str]:
    """The insertion order a JavaScript ``Set`` iterates in.

    Written as a function so every place that needs it says why. A Python ``set`` has no promised
    order at all, and both callers put their result into a warning's ``subjects``, which the corpus
    compares element by element — so a set here is a plan that is correct and whose warnings are
    shuffled, differently between runs of the same input.
    """
    return list(seen.keys())


def _js_number(value: Any) -> str:
    """A number inside a template literal. ``String(50)`` is ``"50"``; ``str(50.0)`` is ``"50.0"``."""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float):
        if value != value:
            return "NaN"
        if value == math.inf:
            return "Infinity"
        if value == -math.inf:
            return "-Infinity"
        if value.is_integer() and abs(value) < 1e21:
            return str(int(value))
    return str(value)


# ── Ratings and modes ────────────────────────────────────────────────────────


def _rating_for(kind: str, limits: dict[str, Any]) -> float | None:
    """Which joint rating bounds a channel of this kind."""
    if kind == "joint_velocity":
        return _or_none(_field(limits, "velocity_max_rad_s"))
    if kind == "joint_torque":
        return _or_none(_field(limits, "effort_max_nm"))
    if kind == "joint_position":
        if _field(limits, "position_max_rad") is _ABSENT or _field(limits, "position_min_rad") is _ABSENT:
            return None
        # Position is bounded by travel rather than by a single magnitude. This is the larger
        # absolute bound, which scaling and the rate limiter need; both ends travel beside it in
        # the clamp's `travel`, which is what the envelope guard holds (F159).
        return max(abs(limits["position_max_rad"]), abs(limits["position_min_rad"]))
    return None


def _control_mode_for(kind: str) -> str | None:
    return {
        "joint_position": "position",
        "joint_velocity": "velocity",
        "joint_torque": "torque",
        "cartesian": "cartesian",
    }.get(kind)


def _index_target(target: dict[str, Any]) -> dict[str, Any]:
    by_name: dict[str, Any] = {}
    by_role: dict[str, Any] = {}
    chains: dict[str, None] = {}
    roles: dict[str, None] = {}

    for joint in target["joints"]:
        by_name[joint["name"]] = joint
        parsed = parse_role(joint.get("role"))
        if parsed is None:
            continue
        key = role_key(parsed)
        # First declaration wins. A body declaring the same role twice is a descriptor bug, and
        # silently preferring the later one would make the mapping depend on array order.
        if key not in by_role:
            by_role[key] = joint
        chains[parsed.chain] = None
        roles[key] = None

    return {"by_name": by_name, "by_role": by_role, "chains": chains, "roles": roles}


# ── Resolution ───────────────────────────────────────────────────────────────


def resolve_transport(
    source: dict[str, Any], target: dict[str, Any], options: dict[str, Any] | None = None
) -> dict[str, Any]:
    """Resolve how a policy's action space reaches a body."""
    if options is None:
        options = {}
    warnings: list[dict[str, Any]] = []
    index = _index_target(target)
    bindings: list[dict[str, Any]] = []
    space = _action_space_of(source, target)
    source_motions = source.get("motions") or []

    if not target.get("verified"):
        warnings.append(
            {
                "code": "unverified_embodiment",
                "message": (
                    "This body description is not signed by its vendor. Its joint ratings are "
                    "unattested."
                ),
                "subjects": [target["robot_slug"]],
            }
        )

    if len(source["action_channels"]) == 0 and len(source_motions) == 0:
        warnings.append(
            {
                "code": "no_source_embodiment",
                "message": (
                    "The policy declares no action space, so nothing about its fit to this body "
                    "can be checked."
                ),
            }
        )

    if target.get("envelope") is None:
        warnings.append(
            {
                "code": "no_target_envelope",
                "message": (
                    "No safety envelope has been declared for this robot. Per-joint ratings are "
                    "the only ceilings available."
                ),
                "subjects": [target["robot_slug"]],
            }
        )

    source_dof = _field(source, "dof")
    if source_dof is not _ABSENT and source_dof != target["dof"]:
        warnings.append(
            {
                "code": "dof_mismatch",
                "message": (
                    f"The policy was trained on a {_js_number(source_dof)} degree-of-freedom body; "
                    f"this one has {_js_number(target['dof'])}."
                ),
            }
        )

    # A control mode is how a *joint* is commanded, so only a joint plan is asked about one.
    if space == "joints" and source["control_mode"] not in target["control"]["modes"]:
        warnings.append(
            {
                "code": "control_mode_unsupported",
                "message": f"This body does not accept {source['control_mode']} commands.",
                "subjects": [source["control_mode"]],
            }
        )

    if _field(target["control"], "command_latency_ms") is _ABSENT:
        warnings.append(
            {
                "code": "latency_undeclared",
                "message": (
                    "This body declares no command latency, so loop stability at the policy rate "
                    "cannot be predicted."
                ),
            }
        )

    # The two refusals that happen before anything is bound (§84.2, §84.3). Observations first and
    # the rate second, as on every other path, so their warnings arrive in one order.
    if space in ("ambiguous", "mismatch_motions", "mismatch_joints"):
        warnings.append(_action_space_warning(space, target))
        observations = _bind_observations(source.get("observations") or [], target, warnings)
        control_hz = _resolve_control_hz(source, target, options, warnings)
        plan = {
            "verdict": "refused",
            "robot_slug": target["robot_slug"],
            "source_digest": _or_none(_field(source, "digest")),
            "target_digest": target["digest"],
            "control_mode": source["control_mode"],
            "control_hz": control_hz,
            "bindings": [],
            "observations": observations,
            "clamp": _build_clamp(target, []),
            "warnings": warnings,
        }
        if source_motions:
            plan["motions"] = []
        return plan

    if space == "motions":
        motions = [_bind_motion(motion, target, warnings) for motion in source_motions]
        observations = _bind_observations(source.get("observations") or [], target, warnings)
        control_hz = _resolve_control_hz(source, target, options, warnings)
        # No `unmapped_roles`: its sentence is that the controller *holds* the joints the policy does
        # not drive, and on a program-level body the controller is running the motion.
        return {
            "verdict": _derive_motion_verdict(motions),
            "robot_slug": target["robot_slug"],
            "source_digest": _or_none(_field(source, "digest")),
            "target_digest": target["digest"],
            "control_mode": source["control_mode"],
            "control_hz": control_hz,
            "bindings": [],
            "motions": motions,
            "observations": observations,
            "clamp": _build_clamp(target, []),
            "warnings": warnings,
        }

    # Chains that cannot be satisfied joint-by-joint. Retargeting is a property of a whole
    # kinematic chain, never of one joint: you cannot send an elbow through task space while its
    # shoulder goes direct. So the chains that need it are decided first, and every channel on such
    # a chain is then marked together.
    retarget_chains = _decide_retarget_chains(source, index, target, options, warnings)

    # §86.2's record of which joint is already commanded in which mode, filled as the channels bind
    # and read by the binder that would be the second onto a joint.
    bound: dict[str, str] = {}
    per_joint_modes = _field(target["control"], "per_joint_modes") is True

    for channel in source["action_channels"]:
        bindings.append(
            _bind_channel(
                channel, target, index, retarget_chains, warnings, bound, per_joint_modes,
            )
        )

    observations = _bind_observations(source.get("observations") or [], target, warnings)
    control_hz = _resolve_control_hz(source, target, options, warnings)
    clamp = _build_clamp(target, bindings)
    verdict = derive_verdict(bindings)

    unmapped = [
        role
        for role in _ordered_keys(index["roles"])
        if not any(
            binding["source_role"] and _role_key_of(binding["source_role"]) == role
            for binding in bindings
        )
    ]
    if unmapped:
        subject = (
            "joint on this body is" if len(unmapped) == 1 else "joints on this body are"
        )
        warnings.append(
            {
                "code": "unmapped_roles",
                "message": (
                    f"{len(unmapped)} {subject} not driven by the policy and will be held by the "
                    "controller."
                ),
                "subjects": unmapped,
            }
        )

    return {
        "verdict": verdict,
        "robot_slug": target["robot_slug"],
        "source_digest": _or_none(_field(source, "digest")),
        "target_digest": target["digest"],
        "control_mode": source["control_mode"],
        "control_hz": control_hz,
        "bindings": bindings,
        "observations": observations,
        "clamp": clamp,
        "warnings": warnings,
    }


def _role_key_of(role: str) -> str | None:
    parsed = parse_role(role)
    return role_key(parsed) if parsed else None


# ── Transport over motions, §84.3 ────────────────────────────────────────────


def _action_space_of(source: dict[str, Any], target: dict[str, Any]) -> str:
    """Which negotiation a plan is. See ``actionSpaceOf`` in the reference for the table."""
    emits_motions = len(source.get("motions") or []) > 0
    emits_channels = len(source["action_channels"]) > 0
    takes_motions = len(target.get("motions") or []) > 0
    takes_joints = len(target["control"]["modes"]) > 0

    if emits_motions and emits_channels:
        return "ambiguous"
    if emits_motions:
        return "motions" if takes_motions else "mismatch_motions"
    if emits_channels and takes_motions and not takes_joints:
        return "mismatch_joints"
    return "joints"


def _action_space_warning(space: str, target: dict[str, Any]) -> dict[str, Any]:
    if space == "ambiguous":
        return {
            "code": "action_space_ambiguous",
            "message": (
                "The policy declares both joint channels and named motions. They are two "
                "architectures, and a package that claims both has not decided which machine it "
                "is for."
            ),
        }
    return {
        "code": "action_space_mismatch",
        "message": (
            "The policy commands named motions and this body is not commanded that way."
            if space == "mismatch_motions"
            else "The policy commands joint channels and this body accepts only named motions."
        ),
        "subjects": [target["robot_slug"]],
    }


def _distinct_motions(target: dict[str, Any]) -> list[dict[str, Any]]:
    """The body's motions with a repeated name removed, first declaration winning (§5.1's rule)."""
    seen: dict[str, None] = {}
    out: list[dict[str, Any]] = []
    for motion in target.get("motions") or []:
        if motion["name"] in seen:
            continue
        seen[motion["name"]] = None
        out.append(motion)
    return out


def _bind_motion(
    motion: dict[str, Any], target: dict[str, Any], warnings: list[dict[str, Any]]
) -> dict[str, Any]:
    body = _distinct_motions(target)

    # 1. The controller's own name.
    for candidate in body:
        if candidate["name"] == motion["name"]:
            return _finish_motion_binding(
                motion, candidate, "direct", "Motion name matches exactly.", warnings
            )

    # 2. The canonical motion, when exactly one body motion is it.
    canonical = canonical_motion(motion.get("motion"))
    same = (
        []
        if canonical is None
        else [c for c in body if canonical_motion(c.get("motion")) == canonical]
    )
    if len(same) == 1:
        return _finish_motion_binding(
            motion,
            same[0],
            "remap",
            f'Mapped by motion {canonical} to "{same[0]["name"]}".',
            warnings,
        )

    # 3. Optional motions may be dropped; required ones are a refusal.
    if canonical is None:
        why = (
            "The motion declares no canonical motion, so it can only match a motion of the same "
            "name, and none exists"
        )
    elif len(same) > 1:
        why = (
            f"This body has {len(same)} {canonical} motions and none named "
            f'"{motion["name"]}", so the vocabulary cannot choose one'
        )
    else:
        why = f"This body has no {canonical} motion"

    # `=== true`: only a boolean true makes a motion optional.
    if motion.get("optional") is True:
        return {
            "source_motion": motion["name"],
            "method": "drop",
            "target": None,
            "missing": [],
            "reason": f"{why}; the policy declares the motion optional.",
        }
    return {
        "source_motion": motion["name"],
        "method": "blocked",
        "target": None,
        "missing": [],
        "reason": f"{why}; the policy requires it.",
    }


def _finish_motion_binding(
    motion: dict[str, Any],
    body: dict[str, Any],
    method: str,
    reason: str,
    warnings: list[dict[str, Any]],
) -> dict[str, Any]:
    supplied = motion.get("parameters") or []
    body_parameters = body.get("parameters") or []

    # `required !== false` — only a boolean false releases a parameter. `is not False` rather than
    # `!= False`, because `0 == False` in Python and the reference reads a zero as required.
    missing = [
        parameter["name"]
        for parameter in body_parameters
        if parameter.get("required") is not False and parameter["name"] not in supplied
    ]
    if missing:
        return {
            "source_motion": motion["name"],
            "method": "blocked",
            "target": body["name"],
            "missing": missing,
            "reason": (
                f'"{body["name"]}" requires a parameter the policy does not supply: '
                f"{', '.join(missing)}."
            ),
        }

    seen: dict[str, None] = {}
    unused: list[str] = []
    for name in supplied:
        if name in seen:
            continue
        seen[name] = None
        if not any(parameter["name"] == name for parameter in body_parameters):
            unused.append(name)
    if unused:
        warnings.append(
            {
                "code": "motion_parameter_unused",
                "message": (
                    f'"{body["name"]}" takes no parameter named {", ".join(unused)}, so the '
                    "policy's value reaches nothing."
                ),
                "subjects": [f"{motion['name']}.{name}" for name in unused],
            }
        )

    return {
        "source_motion": motion["name"],
        "method": method,
        "target": body["name"],
        "missing": [],
        "reason": reason,
    }


def _derive_motion_verdict(bindings: list[dict[str, Any]]) -> str:
    if any(b["method"] == "blocked" for b in bindings):
        return "refused"
    if any(b["method"] == "drop" for b in bindings):
        return "degraded"
    return "direct"


def _decide_retarget_chains(
    source: dict[str, Any],
    index: dict[str, Any],
    target: dict[str, Any],
    options: dict[str, Any],
    warnings: list[dict[str, Any]],
) -> dict[str, None]:
    """Which chains must go through task space, and whether this body can actually do that.

    A chain qualifies when at least one of its joint channels has no direct or role match. It is
    only *granted* when the caller asked for retargeting and the body has a solver — otherwise the
    chain's channels fall through to ``blocked``, which is the honest outcome. A plan that promised
    retargeting on a robot with no IK solver would fail at the first command.
    """
    needs: dict[str, None] = {}

    for channel in source["action_channels"]:
        if channel["kind"] not in _JOINT_KINDS:
            continue
        if channel.get("joint") and channel["joint"] in index["by_name"]:
            continue

        parsed = parse_role(channel.get("role"))
        if parsed is None:
            continue
        if role_key(parsed) in index["by_role"]:
            continue
        # The chain exists on this body but this particular joint does not — a six-joint arm asked
        # to run a seven-joint policy. That is exactly the case task space can rescue.
        if parsed.chain in index["chains"]:
            needs[parsed.chain] = None

    if not needs:
        return needs

    # Strictly `true`. A truthy-but-not-true value does not grant retargeting, and the warning it
    # produces names the wrong cause if this is read loosely.
    can_retarget = options.get("allowRetarget") is True and len(target["solvers"]) > 0
    if not can_retarget:
        warnings.append(
            {
                "code": "retarget_without_solver",
                "message": (
                    "Task-space retargeting is required but this body has no inverse-kinematics "
                    "solver installed."
                    if options.get("allowRetarget")
                    else "Task-space retargeting would be required, and was not requested."
                ),
                "subjects": _ordered_keys(needs),
            }
        )
        return {}

    return needs


def _bind_channel(
    channel: dict[str, Any],
    target: dict[str, Any],
    index: dict[str, Any],
    retarget_chains: dict[str, None],
    warnings: list[dict[str, Any]],
    bound: dict[str, str],
    per_joint_modes: bool,
) -> dict[str, Any]:
    parsed = parse_role(channel.get("role"))
    # `mode` on the base rather than on each literal below: a binding that commands no joint has no
    # mode (§86.2), and there are eleven places that produce one.
    base = {
        "source_index": channel["index"],
        "source_role": _or_none(_field(channel, "role")),
        "mode": None,
    }

    # Non-joint channels are matched against the body's own facilities rather than its joints.
    if channel["kind"] not in _JOINT_KINDS:
        return _bind_non_joint_channel(channel, base, target)

    # 1. Exact wire name. Two vendors agreeing on a joint name is rare and unambiguous when it
    #    happens, so it outranks role matching.
    if channel.get("joint"):
        exact = index["by_name"].get(channel["joint"])
        if exact:
            return _finish_joint_binding(
                base, channel, exact, "direct", "Joint name matches exactly.", warnings,
                bound, per_joint_modes,
            )

    # 2. Canonical role. This is the mechanism that makes bodies interoperable at all.
    if parsed:
        key = role_key(parsed)
        by_role = index["by_role"].get(key)
        if by_role:
            return _finish_joint_binding(
                base,
                channel,
                by_role,
                "remap",
                f'Mapped by role {key} to "{by_role["name"]}".',
                warnings,
                bound,
                per_joint_modes,
            )

        # 3. Task space, when the chain was granted it above.
        if parsed.chain in retarget_chains:
            return {
                **base,
                "method": "retarget",
                "target": None,
                "scale": 1,
                "limit": None,
                "reason": (
                    f"Driven through task space on the {parsed.chain} chain; this body has no "
                    f"{key}."
                ),
            }

    # 4. Optional channels may be dropped; required ones are a refusal.
    if channel.get("optional"):
        return {
            **base,
            "method": "drop",
            "target": None,
            "scale": 0,
            "limit": None,
            "reason": "No equivalent on this body, and the policy declares the channel optional.",
        }

    return {
        **base,
        "method": "blocked",
        "target": None,
        "scale": 1,
        "limit": None,
        "reason": (
            f"This body has no {role_key(parsed)} and the policy requires it."
            if parsed
            else (
                "The channel declares no canonical role, so it can only match a joint of the same "
                "name, and none exists."
            )
        ),
    }


def _bind_non_joint_channel(
    channel: dict[str, Any], base: dict[str, Any], target: dict[str, Any]
) -> dict[str, Any]:
    if channel["kind"] == "gripper":
        effector = next(
            (
                e
                for e in target["end_effectors"]
                if e["kind"] == "gripper" or e["kind"] == "dexterous_hand"
            ),
            None,
        )
        if not effector:
            if channel.get("optional"):
                return {
                    **base, "method": "drop", "target": None, "scale": 0, "limit": None,
                    "reason": "This body has no gripper; the channel is optional.",
                }
            return {
                **base, "method": "blocked", "target": None, "scale": 1, "limit": None,
                "reason": "This body has no gripper and the policy requires one.",
            }
        return {
            **base,
            "method": "remap",
            "target": effector["name"],
            "scale": 1,
            "limit": _or_none(_field(effector, "max_grip_force_n")),
            "reason": f'Driven through the {effector["kind"]} "{effector["name"]}".',
        }

    if channel["kind"] == "base_velocity":
        if target["base"] == "fixed":
            if channel.get("optional"):
                return {
                    **base, "method": "drop", "target": None, "scale": 0, "limit": None,
                    "reason": "This body has a fixed base; the channel is optional.",
                }
            return {
                **base, "method": "blocked", "target": None, "scale": 1, "limit": None,
                "reason": (
                    "The policy commands base motion and this body has a fixed base."
                ),
            }
        limit = _or_none(_field(target.get("envelope"), "base_velocity_max_m_s"))
        # A wheeled base driven by a policy trained on a legged one is a synthesis, not a remap:
        # the command means the same thing, and nothing below the base interface is comparable.
        method = "remap" if target["base"] == "legged" else "synthesize"
        return {
            **base,
            "method": method,
            "target": "base",
            "scale": _scale_for(_field(channel, "peak"), limit),
            "limit": limit,
            "reason": f'Base velocity applied to this {target["base"]} base.',
        }

    if channel["kind"] == "cartesian":
        effector = next((e for e in target["end_effectors"] if e["kind"] != "none"), None)
        if not effector or len(target["solvers"]) == 0:
            if channel.get("optional"):
                return {
                    **base, "method": "drop", "target": None, "scale": 0, "limit": None,
                    "reason": (
                        "End-effector pose control is unavailable on this body; the channel is "
                        "optional."
                    ),
                }
            return {
                **base, "method": "blocked", "target": None, "scale": 1, "limit": None,
                "reason": (
                    "End-effector pose control needs an end effector and a solver; this body has "
                    "neither."
                ),
            }
        return {
            **base,
            "method": "retarget",
            "target": effector["name"],
            "scale": 1,
            "limit": None,
            "reason": f'Solved onto "{effector["name"]}" using {target["solvers"][0]}.',
        }

    # `discrete` — a mode switch or a named action. It carries no physical units, so nothing here
    # can bound it; the package's capability grants are what gate it.
    return {
        **base,
        "method": "direct",
        "target": None,
        "scale": 1,
        "limit": None,
        "reason": "Discrete command passed through; it commands no actuator directly.",
    }


def _finish_joint_binding(
    base: dict[str, Any],
    channel: dict[str, Any],
    joint: dict[str, Any],
    method: str,
    reason: str,
    warnings: list[dict[str, Any]],
    bound: dict[str, str],
    per_joint_modes: bool,
) -> dict[str, Any]:
    rating = _rating_for(channel["kind"], joint["limits"])
    peak = _field(channel, "peak")
    scale = _scale_for(peak, rating)
    mode = _control_mode_for(channel["kind"])

    if mode and mode not in joint["control_modes"]:
        return {
            **base,
            "method": "blocked",
            "target": joint["name"],
            "scale": 1,
            "limit": rating,
            "mode": mode,
            "reason": f'"{joint["name"]}" does not accept {mode} commands.',
        }

    # §86.2. One joint, one mode, per cycle. A second channel onto a joint already bound in another
    # mode would meet it as one entry in `values` and one in `modes`, and the command would carry
    # whichever was written last. First-wins, as §84.3's binder does with a repeated motion.
    already = bound.get(joint["name"]) if mode else None
    if mode and already is not None and already != mode:
        warnings.append(
            {
                "code": "joint_mode_conflict",
                "message": (
                    f'"{joint["name"]}" is already bound in {already}; a joint carries one mode '
                    f"per cycle, so this {mode} channel is blocked."
                ),
                "subjects": [joint["name"]],
            }
        )
        return {
            **base,
            "method": "blocked",
            "target": joint["name"],
            "scale": 1,
            "limit": rating,
            "mode": mode,
            "reason": (
                f'"{joint["name"]}" is commanded in {already} by an earlier channel, and a joint '
                "takes one mode per cycle."
            ),
        }

    # §86.1's other half. A driver that does not read the per-joint array takes one mode for the
    # whole command, so a channel in a second mode cannot be delivered to it at all. Blocked and
    # named: the alternative an integration reaches for is to command the joints sharing the
    # majority mode and drop the rest.
    elsewhere = next((other for other in bound.values() if other != mode), None) if mode else None
    if mode and elsewhere is not None and not per_joint_modes:
        warnings.append(
            {
                "code": "mixed_modes_unsupported",
                "message": (
                    f"This body is already commanded in {elsewhere} and its driver reads one mode "
                    f'per command, so the {mode} channel on "{joint["name"]}" cannot be delivered.'
                ),
                "subjects": [joint["name"]],
            }
        )
        return {
            **base,
            "method": "blocked",
            "target": joint["name"],
            "scale": 1,
            "limit": rating,
            "mode": mode,
            "reason": (
                "This driver does not declare per-joint modes (\u00a786.1), and the body is "
                f"already commanded in {elsewhere}."
            ),
        }

    if mode:
        bound[joint["name"]] = mode

    if scale < 1:
        warnings.append(
            {
                "code": "scaled_down",
                "message": (
                    f'Commands to "{joint["name"]}" are scaled to '
                    f"{_to_fixed(scale * 100, 0)}% because the policy was trained on a stronger "
                    "joint."
                ),
                "subjects": [joint["name"]],
            }
        )
    elif peak is not _ABSENT and rating is None:
        warnings.append(
            {
                "code": "unscalable_channel",
                "message": (
                    f'"{joint["name"]}" declares no rating for this control mode, so the policy\'s '
                    "commands cannot be scaled to it."
                ),
                "subjects": [joint["name"]],
            }
        )

    # §7.1, F159. A position channel is held to the joint's travel, and a travel needs both ends. A
    # joint that declares one has neither a travel nor a magnitude, so the kernel bounds its position
    # by nothing — said here, per channel. `=== undefined` in the reference, so a declared null is an
    # end.
    limits = joint["limits"]
    if mode == "position" and (
        (_field(limits, "position_min_rad") is _ABSENT) != (_field(limits, "position_max_rad") is _ABSENT)
    ):
        warnings.append(
            {
                "code": "travel_one_end",
                "message": (
                    f'"{joint["name"]}" declares one end of its travel and not the other, so the '
                    "Safety Kernel bounds its position by nothing."
                ),
                "subjects": [joint["name"]],
            }
        )

    return {
        **base,
        "method": method,
        "target": joint["name"],
        "scale": scale,
        "limit": rating,
        "mode": mode,
        "reason": reason,
    }


def _scale_for(peak: Any, rating: float | None) -> float:
    """How far a channel must be scaled to stay inside a joint's rating.

    Returns 1 when either number is missing, and a warning is raised separately. Guessing a ratio
    from one known side would be inventing the other.
    """
    if peak is _ABSENT or rating is None:
        return 1
    if peak <= 0:
        return 1
    return min(1, rating / peak)


def _bind_observations(
    required: list[dict[str, Any]], target: dict[str, Any], warnings: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    for requirement in required:
        exact = next(
            (
                s
                for s in target["sensors"]
                if s["name"] == requirement["name"] and s["modality"] == requirement["modality"]
            ),
            None,
        )
        candidate = exact if exact is not None else _match_sensor_by_role(requirement, target)

        if not candidate:
            if not requirement.get("optional"):
                warnings.append(
                    {
                        "code": "observation_missing",
                        "message": (
                            f'The policy reads a {requirement["modality"]} stream called '
                            f'"{requirement["name"]}" and this body has none.'
                        ),
                        "subjects": [requirement["name"]],
                    }
                )
            out.append(
                {
                    "required": requirement["name"],
                    "method": "missing",
                    "target": None,
                    "reason": (
                        "Not present on this body; the policy declares it optional."
                        if requirement.get("optional")
                        else "Not present on this body."
                    ),
                }
            )
            continue

        degraded = (
            (
                _field(requirement, "width") is not _ABSENT
                and _zero_default(_field(candidate, "width")) < requirement["width"]
            )
            or (
                _field(requirement, "height") is not _ABSENT
                and _zero_default(_field(candidate, "height")) < requirement["height"]
            )
            or (
                _field(requirement, "fps") is not _ABSENT
                and _zero_default(_field(candidate, "fps")) < requirement["fps"]
            )
        )

        if degraded:
            warnings.append(
                {
                    "code": "observation_degraded",
                    "message": (
                        f'"{candidate["name"]}" is lower resolution or slower than the stream the '
                        "policy was trained on."
                    ),
                    "subjects": [candidate["name"]],
                }
            )
            out.append(
                {
                    "required": requirement["name"],
                    "method": "downscale",
                    "target": candidate["name"],
                    "reason": (
                        "Present but below the trained resolution or frame rate; frames are "
                        "upscaled."
                    ),
                }
            )
            continue

        out.append(
            {
                "required": requirement["name"],
                "method": "direct" if exact is not None else "remap",
                "target": candidate["name"],
                "reason": (
                    "Sensor name and modality match."
                    if exact is not None
                    else f'Matched by role to "{candidate["name"]}".'
                ),
            }
        )
    return out


def _zero_default(value: Any) -> Any:
    """``value ?? 0`` — a sensor that declares no width is treated as zero-wide, not as a match."""
    return 0 if value is _ABSENT or value is None else value


def _match_sensor_by_role(
    requirement: dict[str, Any], target: dict[str, Any]
) -> dict[str, Any] | None:
    parsed = parse_role(requirement.get("role"))
    if parsed is None:
        return None
    key = role_key(parsed)
    for sensor in target["sensors"]:
        if sensor["modality"] != requirement["modality"]:
            continue
        sensor_role = parse_role(sensor.get("role"))
        if sensor_role is not None and role_key(sensor_role) == key:
            return sensor
    return None


def _resolve_control_hz(
    source: dict[str, Any],
    target: dict[str, Any],
    options: dict[str, Any],
    warnings: list[dict[str, Any]],
) -> float | None:
    """The rate the loop will actually run at.

    Clamped down to what the body accepts, never up: running a policy faster than it was trained
    changes its dynamics, and running it slower is a documented degradation rather than a silent
    one. The watchdog check is the one that matters most — a loop slower than the controller's
    timeout faults *while the robot is moving*, which is worse than refusing to start.
    """
    requested = _field(options, "requestedHz")
    if requested is _ABSENT or requested is None:
        requested = _field(source, "control_hz")
    if requested is _ABSENT:
        return None

    resolved = requested
    maximum = _field(target["control"], "rate_hz_max")
    if maximum is not _ABSENT and requested > maximum:
        resolved = maximum
        warnings.append(
            {
                "code": "rate_unsupported",
                "message": (
                    f"The policy runs at {_js_number(requested)} Hz; this body accepts at most "
                    f"{_js_number(maximum)} Hz, so the loop is clamped."
                ),
            }
        )

    watchdog_ms = _field(target.get("envelope"), "watchdog_ms")
    if watchdog_ms is not _ABSENT and watchdog_ms > 0:
        min_hz = 1000 / watchdog_ms
        if resolved < min_hz:
            warnings.append(
                {
                    "code": "rate_below_watchdog",
                    "message": (
                        f"At {_js_number(resolved)} Hz the controller's {_js_number(watchdog_ms)} "
                        "ms watchdog fires between commands."
                    ),
                }
            )

    return resolved


def _build_clamp(target: dict[str, Any], bindings: list[dict[str, Any]]) -> dict[str, Any] | None:
    envelope = target.get("envelope")
    per_channel: dict[str, Any] = {}
    joint_modes: dict[str, str] = {}
    travel: dict[str, list[Any]] = {}

    for binding in bindings:
        # A blocked binding commands nothing, so it contributes neither a ceiling nor a mode. It
        # carries `limit` — the rating of the joint it could not command — so a reader can see what
        # was on offer, and that is the only thing it is for.
        if binding["method"] == "blocked":
            continue
        if binding["target"] and binding["limit"] is not None:
            per_channel[binding["target"]] = binding["limit"]
        # §86.3. Without this the kernel cannot read a ceiling in the right units.
        if binding["target"] and binding["mode"] is not None:
            joint_modes[binding["target"]] = binding["mode"]
        # §7.1, F159. Both ends of a position joint's travel. `find`, so the first joint of that name.
        if binding["target"] and binding["mode"] == "position":
            joint = next((j for j in target["joints"] if j["name"] == binding["target"]), None)
            limits = joint["limits"] if joint is not None else _ABSENT
            low, high = _field(limits, "position_min_rad"), _field(limits, "position_max_rad")
            if low is not _ABSENT and high is not _ABSENT:
                travel[binding["target"]] = [low, high]

    if not envelope and len(per_channel) == 0 and len(joint_modes) == 0 and len(travel) == 0:
        return None

    watchdog_ms = _field(envelope, "watchdog_ms")
    clamp: dict[str, Any] = {}
    for key in ("joint_velocity_max_rad_s", "joint_torque_max_nm", "base_velocity_max_m_s"):
        value = _field(envelope, key)
        if value is not _ABSENT:
            clamp[key] = value
    if watchdog_ms is not _ABSENT and watchdog_ms > 0:
        clamp["min_control_hz"] = 1000 / watchdog_ms
    clamp["per_channel"] = per_channel
    # Absent — not empty — on a plan that binds no joint, so a motion plan and a plan over a gripper
    # and a base are the same bytes they were before §86.
    if len(joint_modes) > 0:
        clamp["joint_modes"] = joint_modes
    # Absent from a plan that binds no joint in position, for the reason `joint_modes` is.
    if len(travel) > 0:
        clamp["travel"] = travel
    source = _field(envelope, "source")
    if source is not _ABSENT:
        clamp["source"] = source
    return clamp


def derive_verdict(bindings: list[dict[str, Any]]) -> str:
    """The plan's verdict is the worst binding in it.

    One blocked required channel refuses the whole plan, because a policy missing an actuator it
    depends on does not do most of its job — it does something nobody has characterised.
    """
    if any(b["method"] == "blocked" for b in bindings):
        return "refused"
    if any(b["method"] == "drop" for b in bindings):
        return "degraded"
    if any(b["method"] in ("retarget", "synthesize") for b in bindings):
        return "retargeted"
    return "direct"


def is_installable(plan: dict[str, Any]) -> bool:
    """Whether the OS will install a policy under this plan. Refusals are the only hard stop."""
    return plan["verdict"] != "refused"


# ── The wire shape ───────────────────────────────────────────────────────────


def plan_to_json(case_input: dict[str, Any]) -> Any:
    """``transport/*`` and ``fuzz/transport`` — one resolved plan."""
    plan = resolve_transport(
        case_input["source"], case_input["target"], case_input.get("options") or {}
    )
    clamp = plan["clamp"]
    return {
        "verdict": plan["verdict"],
        "robot_slug": plan["robot_slug"],
        "source_digest": plan["source_digest"],
        "target_digest": plan["target_digest"],
        "control_mode": plan["control_mode"],
        "control_hz": plan["control_hz"],
        "bindings": [
            {
                "source_index": b["source_index"],
                "source_role": b["source_role"],
                "method": b["method"],
                "target": b["target"],
                "scale": b["scale"],
                "limit": b["limit"],
                "reason": b["reason"],
            }
            for b in plan["bindings"]
        ],
        # §84.3 — absent from a joint plan, so every plan written before motions is unchanged.
        "motions": OMIT
        if "motions" not in plan
        else [
            {
                "source_motion": m["source_motion"],
                "method": m["method"],
                "target": m["target"],
                "missing": m["missing"],
                "reason": m["reason"],
            }
            for m in plan["motions"]
        ],
        "observations": [
            {
                "required": o["required"],
                "method": o["method"],
                "target": o["target"],
                "reason": o["reason"],
            }
            for o in plan["observations"]
        ],
        "clamp": None
        if clamp is None
        else {
            "joint_velocity_max_rad_s": clamp.get("joint_velocity_max_rad_s", OMIT),
            "joint_torque_max_nm": clamp.get("joint_torque_max_nm", OMIT),
            "base_velocity_max_m_s": clamp.get("base_velocity_max_m_s", OMIT),
            "min_control_hz": clamp.get("min_control_hz", OMIT),
            "per_channel": clamp["per_channel"],
            "joint_modes": clamp.get("joint_modes", OMIT),
            "travel": clamp.get("travel", OMIT),
            "source": clamp.get("source", OMIT),
        },
        "warnings": [
            {
                "code": w["code"],
                "message": w["message"],
                "subjects": w["subjects"] if w.get("subjects") is not None else OMIT,
            }
            for w in plan["warnings"]
        ],
    }
