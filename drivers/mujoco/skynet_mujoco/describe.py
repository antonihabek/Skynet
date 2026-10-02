"""The embodiment descriptor, built from an MJCF scene — §70.

> **Descriptors come from the scene**: MJCF for MuJoCo, USD for Isaac — the same generator shape as
> §63.4, with the same refusal to populate an envelope from model values.

So this is `skynet descriptor from-urdf` for MJCF, with one difference that runs all the way
through it: **in a simulator the model is the body**. There is no physical machine the file is an
approximation of, so a per-joint range read out of the scene is not a design value that might be
wrong about the hardware — it is exactly what this body does. The descriptor says so, and the
distinction is worth holding on to because it makes the *other* half sharper rather than softer:

**The envelope is still null, and simulation is the strongest reason for it yet.** A safety envelope
bounds what a machine may do to the people near it. A simulated robot has no people near it, no
datasheet, and no risk assessment; populating `envelope` from a scene file would mint a safety
figure out of a modeller's guess at a gain — and worse, a policy validated against that envelope
would carry the validation across to the hardware, which is exactly the transfer §70 forbids when
it says no simulated run may be presented as bench evidence.

Four other things this generator does not do:

* **It assigns no roles.** Not "suggests them and requires a flag" — assigns none at all, unless the
  configuration hands it a mapping. Role suggestion already exists, once, in
  `skynet descriptor from-urdf`, with the human-accept step §63.4 requires. A second implementation
  of "which joint is this, really" is a second answer to the same question, and the whole apparatus
  of the conformance corpus exists because two answers to one question is the failure mode this
  project is arranged around. The report points at the tool instead.
* **`digest` is null and `verified` is false.** Nobody has signed a scene file.
* **It does not guess the base.** A scene whose root body carries a free joint describes something
  that moves, and *how* it moves — wheels, legs, rotors — is not in the file. The driver refuses to
  open until the operator says, exactly as the URDF importer does.
* **It declares a camera only if it can render one.** See :mod:`skynet_mujoco.render`: a declared
  stream that nothing can read binds a policy's observation to nothing, and that bug has already
  been caught once in this repository by `skynet driver check`.
"""

from __future__ import annotations

import json
import math
from typing import Any

__all__ = [
    "BASE_KINDS",
    "DescribeResult",
    "Note",
    "build_descriptor",
    "commandable_joints",
    "JointBinding",
]

#: Base kinds a descriptor may declare, from `skynet_contracts::embodiment::BASE_KINDS`.
BASE_KINDS = ("fixed", "wheeled", "tracked", "legged", "aerial", "rail", "surface", "submerged")

# MuJoCo enum values, written out rather than imported, so this module can be read (and unit-tested
# for its pure parts) without MuJoCo present. They are checked against the library at import time in
# `bridge.py`, so a renumbering in some future MuJoCo fails loudly rather than silently mapping a
# hinge to a slide.
JNT_FREE, JNT_BALL, JNT_SLIDE, JNT_HINGE = 0, 1, 2, 3
TRN_JOINT, TRN_JOINTINPARENT = 0, 1
BIAS_NONE, BIAS_AFFINE = 0, 1
DYN_NONE = 0


class Note:
    """One line of the report, in the shape `skynet descriptor from-urdf` writes to stderr."""

    def __init__(self, level: str, message: str) -> None:
        self.level = level  # "action" | "info" | "warn"
        self.message = message

    def __repr__(self) -> str:  # pragma: no cover - debugging aid
        return f"Note({self.level!r}, {self.message!r})"


class JointBinding:
    """One RHAL joint: the actuator that commands it and the MuJoCo state it reads.

    The RHAL's three state arrays and its command arrays are indexed by *the same* joint index, so
    a joint that can be read and not commanded cannot be one of these. That is why the binding pairs
    an actuator with a joint rather than listing joints: `position_rad[i]` and `values[i]` must mean
    the same joint or a driver silently commands the wrong axis.
    """

    def __init__(
        self,
        *,
        name: str,
        actuator_id: int,
        joint_id: int,
        qpos_adr: int,
        dof_adr: int,
        mode: str,
        joint_type: str,
        limits: dict[str, float],
        gear: float = 1.0,
    ) -> None:
        self.name = name
        self.actuator_id = actuator_id
        self.joint_id = joint_id
        self.qpos_adr = qpos_adr
        self.dof_adr = dof_adr
        self.mode = mode
        self.joint_type = joint_type
        self.limits = limits
        #: The transmission's gear. The actuator acts on `gear · q`, so a setpoint is written as
        #: `gear · radians`, a torque as `newton-metres / gear`, and a force read back is multiplied
        #: by it — `DRY-COVERAGE-PLAN.md` F167, where every geared actuator was off by exactly this.
        self.gear = gear


class DescribeResult:
    """What the generator produced, and everything it declined to fill in."""

    def __init__(
        self,
        *,
        descriptor: dict[str, Any],
        joints: list[JointBinding],
        notes: list[Note],
        cameras: list[str],
        free_root_joint: int | None,
        root_body: int,
    ) -> None:
        self.descriptor = descriptor
        self.joints = joints
        self.notes = notes
        self.cameras = cameras
        self.free_root_joint = free_root_joint
        self.root_body = root_body

    def json(self) -> str:
        """Canonical-ish JSON: sorted keys, no whitespace.

        Not `crates/skynet-json`'s canonical form — Python's `json` writes floats its own way — and
        it does not need to be, because nothing digests this. When something does, the digest is
        computed over the canonical bytes by the tool that signs it, which is the vendor's job
        (§5) and not a simulator's.
        """
        return json.dumps(self.descriptor, sort_keys=True, separators=(",", ":"))


def _name(mujoco: Any, model: Any, obj: int, index: int) -> str:
    got = mujoco.mj_id2name(model, obj, index)
    return got if got is not None else ""


def _actuator_mode(model: Any, index: int) -> str | None:
    """Which control mode an actuator implements, or `None` when this cannot be decided.

    MuJoCo does not store "this is a position servo"; `<position>`, `<velocity>` and `<motor>` are
    shorthands the compiler expands into gain and bias parameters, and what survives into the model
    is the arithmetic. So the mode is read back off the arithmetic:

    | Shape | Means |
    |---|---|
    | no bias term | force in, force out — a `<motor>`, so **torque** |
    | bias affine in length | a spring toward `ctrl` — a `<position>` servo |
    | bias affine in velocity only | a damper toward `ctrl` — a `<velocity>` servo |

    Anything else — a muscle, a user gain, an actuator with internal state — is `None`, and the
    caller excludes it and says which one. Guessing here would put a joint in the command array
    whose `ctrl` does not mean what the mode says it means, and every command after that is wrong
    in a way that looks like a tuning problem.
    """
    if int(model.actuator_dyntype[index]) != DYN_NONE:
        # An integrator or a filter sits between `ctrl` and the force. `ctrl` is then a rate or a
        # setpoint for internal state, and the RHAL's "values are radians / rad·s⁻¹ / Nm" is not
        # true of it.
        return None

    bias = int(model.actuator_biastype[index])
    if bias == BIAS_NONE:
        return "torque"
    if bias != BIAS_AFFINE:
        return None

    prm = model.actuator_biasprm[index]
    kp, kv = float(prm[1]), float(prm[2])
    if kp != 0.0:
        return "position"
    if kv != 0.0:
        return "velocity"
    return None


def commandable_joints(mujoco: Any, model: Any) -> tuple[list[JointBinding], list[Note]]:
    """The joints the RHAL can carry, and a note for every actuator that could not be one.

    An excluded actuator is named rather than dropped quietly. A scene with a tendon-driven finger
    produces a robot with one fewer joint than the modeller expects, and the way that presents — a
    policy's action vector no longer matching the body — is far away from the cause.
    """
    bindings: list[JointBinding] = []
    notes: list[Note] = []

    for index in range(model.nu):
        name = _name(mujoco, model, mujoco.mjtObj.mjOBJ_ACTUATOR, index) or f"actuator{index}"
        trntype = int(model.actuator_trntype[index])

        if trntype not in (TRN_JOINT, TRN_JOINTINPARENT):
            notes.append(
                Note(
                    "warn",
                    f"Actuator `{name}` drives a tendon, a site or a body rather than a joint, so "
                    "there is no joint index the RHAL could give it. It is not in `joints[]`, and "
                    "this body therefore has fewer degrees of freedom than the scene does.",
                )
            )
            continue

        joint_id = int(model.actuator_trnid[index][0])
        joint_name = _name(mujoco, model, mujoco.mjtObj.mjOBJ_JOINT, joint_id) or f"joint{joint_id}"
        jnt_type = int(model.jnt_type[joint_id])

        if jnt_type == JNT_HINGE:
            limited = bool(model.jnt_limited[joint_id])
            joint_type = "revolute" if limited else "continuous"
        elif jnt_type == JNT_SLIDE:
            joint_type = "prismatic"
        else:
            notes.append(
                Note(
                    "warn",
                    f"Actuator `{name}` drives `{joint_name}`, which is a "
                    f"{'free' if jnt_type == JNT_FREE else 'ball'} joint: three or six degrees of "
                    "freedom behind one name. The RHAL's joint arrays carry one scalar per joint, "
                    "so it is not in `joints[]`.",
                )
            )
            continue

        mode = _actuator_mode(model, index)
        if mode is None:
            notes.append(
                Note(
                    "warn",
                    f"Actuator `{name}` is neither a position servo, a velocity servo nor a direct "
                    "force actuator — its gain, bias or dynamics say something this bridge cannot "
                    "read back as an RHAL control mode. It is not in `joints[]`, because a joint "
                    "whose `ctrl` does not mean what the mode says it means is worse than an "
                    "absent one.",
                )
            )
            continue

        limits: dict[str, float] = {}
        if bool(model.jnt_limited[joint_id]):
            low, high = model.jnt_range[joint_id]
            limits["position_min_rad"] = float(low)
            limits["position_max_rad"] = float(high)
        gear = float(model.actuator_gear[index][0])
        if gear == 0.0:
            notes.append(
                Note(
                    "warn",
                    f"Actuator `{name}` has a gear of zero, so nothing it does reaches `{joint_name}`. "
                    "It is not in `joints[]`.",
                )
            )
            continue
        ceilings: list[float] = []
        if bool(model.actuator_forcelimited[index]):
            low, high = model.actuator_forcerange[index]
            # One scalar ceiling, and the magnitude is what a channel is scaled against. An
            # asymmetric force range is a real thing in MJCF and the smaller magnitude is the one
            # that holds in both directions. The range bounds the actuator's force, and the joint
            # feels `gear` times it.
            ceilings.append(abs(gear) * float(min(abs(float(low)), abs(float(high)))))
        if mode == "torque" and bool(model.actuator_ctrllimited[index]):
            # A `<motor>`'s `ctrl` *is* its force (gain one, no bias), so a bounded `ctrl` bounds
            # the torque as surely as `forcerange` does — and it is how the menagerie's legged
            # bodies are written, which left all of them declaring no rating (F167).
            low, high = model.actuator_ctrlrange[index]
            gain = abs(float(model.actuator_gainprm[index][0]))
            ceilings.append(abs(gear) * gain * float(min(abs(float(low)), abs(float(high)))))
        if ceilings:
            limits["effort_max_nm"] = min(ceilings)

        bindings.append(
            JointBinding(
                name=joint_name,
                actuator_id=index,
                joint_id=joint_id,
                qpos_adr=int(model.jnt_qposadr[joint_id]),
                dof_adr=int(model.jnt_dofadr[joint_id]),
                mode=mode,
                joint_type=joint_type,
                limits=limits,
                gear=gear,
            )
        )

    seen: dict[str, int] = {}
    for binding in bindings:
        seen[binding.name] = seen.get(binding.name, 0) + 1
    for name, count in seen.items():
        if count > 1:
            notes.append(
                Note(
                    "warn",
                    f"Joint `{name}` is driven by {count} actuators. Both are in `joints[]` under "
                    "the same name, and a transport plan matching by exact name will bind to the "
                    "first. Rename one in the scene, or the plan is deciding by list order.",
                )
            )

    return bindings, notes


def _free_root_joint(model: Any, root_body: int) -> int | None:
    """The free joint carrying the root body, when there is one."""
    for index in range(model.njnt):
        if int(model.jnt_bodyid[index]) == root_body and int(model.jnt_type[index]) == JNT_FREE:
            return index
    return None


def _root_body(model: Any) -> int:
    """The single child of the world body — the robot's base link.

    A scene with two of them is a scene with two robots (or a robot and a table), and the descriptor
    describes one body. The caller reports it rather than picking.
    """
    children = [i for i in range(1, model.nbody) if int(model.body_parentid[i]) == 0]
    return children[0] if children else 0


def _quat_xyzw(wxyz: Any) -> list[float]:
    """MuJoCo stores quaternions `wxyz`; §44.4 orders them `xyzw`.

    A transposition here is a rotation that is wrong by a quarter turn about an unpredictable axis
    and is very hard to see in a number, so it is done once, here, and nowhere else.
    """
    w, x, y, z = (float(v) for v in wxyz)
    return [_round12(x), _round12(y), _round12(z), _round12(w)]


def _round12(value: float) -> float:
    """Rounded to a picoradian, so two machines produce the same bytes.

    The same rule `crates/skynet-urdf` applies, for the same reason: a descriptor whose digest
    depends on which laptop generated it is a descriptor nobody can verify.
    """
    scaled = round(value * 1e12) / 1e12
    return 0.0 if scaled == 0.0 else scaled


def build_frames(mujoco: Any, model: Any, root_body: int) -> tuple[Any, list[Note]]:
    """The frame graph (§44.3), from the body tree.

    Every body is a frame. A body reached through a movable joint is `dynamic` and names the joint
    and its axis; one rigidly attached to its parent is `static`. Both carry the parent-relative
    transform the scene declares, which together with the joint axes is a complete forward-kinematics
    specification — which is what §44.2 needs, because the kernel computes its own FK and subscribes
    to nothing.

    **Returns `None` when the scene cannot be expressed in the pinned shape.** A frame node carries
    at most one joint, and MJCF lets a body carry several. Synthesising intermediate frames would
    invent names that are in no file anybody can read, so the graph is omitted and §44.5's honest
    failure is taken instead: a body with no frame graph reports the pose guards unavailable with
    the reason named, rather than evaluating them against a tree that is missing a hinge.
    """
    notes: list[Note] = []
    joints_by_body: dict[int, list[int]] = {}
    for index in range(model.njnt):
        joints_by_body.setdefault(int(model.jnt_bodyid[index]), []).append(index)

    for body, joints in joints_by_body.items():
        if body == root_body and len(joints) == 1 and int(model.jnt_type[joints[0]]) == JNT_FREE:
            continue  # the base's own degrees of freedom, which `base` describes
        if len(joints) > 1:
            names = ", ".join(
                _name(mujoco, model, mujoco.mjtObj.mjOBJ_JOINT, j) or f"joint{j}" for j in joints
            )
            notes.append(
                Note(
                    "warn",
                    f"Body `{_name(mujoco, model, mujoco.mjtObj.mjOBJ_BODY, body)}` carries "
                    f"{len(joints)} joints ({names}). A frame node carries one, so `frames` is "
                    "null for this scene and the pose guards will report unavailable rather than "
                    "run against an incomplete tree. Split the body in the scene, or wait for the "
                    "specification to answer whether a node may carry a list.",
                )
            )
            return None, notes

    nodes = []
    for body in range(1, model.nbody):
        # Bodies outside the robot's own subtree are scenery — a table, a cube to pick up — and are
        # not part of this body's frame graph.
        if not _in_subtree(model, body, root_body):
            continue

        name = _name(mujoco, model, mujoco.mjtObj.mjOBJ_BODY, body) or f"body{body}"
        node: dict[str, Any] = {"name": name}

        if body == root_body:
            node["parent"] = None
            node["kind"] = "root"
        else:
            parent = int(model.body_parentid[body])
            node["parent"] = _name(mujoco, model, mujoco.mjtObj.mjOBJ_BODY, parent) or f"body{parent}"
            joints = joints_by_body.get(body, [])
            if joints:
                joint = joints[0]
                jnt_type = int(model.jnt_type[joint])
                node["kind"] = "dynamic"
                node["joint"] = _name(mujoco, model, mujoco.mjtObj.mjOBJ_JOINT, joint) or f"joint{joint}"
                node["joint_type"] = {
                    JNT_HINGE: "revolute" if bool(model.jnt_limited[joint]) else "continuous",
                    JNT_SLIDE: "prismatic",
                    JNT_BALL: "ball",
                    JNT_FREE: "floating",
                }[jnt_type]
                node["axis"] = [_round12(float(v)) for v in model.jnt_axis[joint]]
            else:
                node["kind"] = "static"
            node["translation"] = [_round12(float(v)) for v in model.body_pos[body]]
            node["rotation"] = _quat_xyzw(model.body_quat[body])
            # §44.3: a static transform is either the vendor's nominal design value or a measured
            # one for this individual machine, and the frame records which. Nothing here is
            # measured — there is no individual machine — so every one of them is `descriptor`.
            node["source"] = "descriptor"

        node["collision"] = _has_collision(model, body)
        shape = _collision_shape(mujoco, model, body)
        if shape is not None:
            node["collision_shape"] = shape
        mass = float(model.body_mass[body])
        node["mass_kg"] = mass if mass > 0.0 else None
        nodes.append(node)

    root_name = _name(mujoco, model, mujoco.mjtObj.mjOBJ_BODY, root_body) or "base_link"
    return {
        "root": root_name,
        "convention": "rep103",
        "generator": "mjcf",
        "nodes": nodes,
    }, notes


def _in_subtree(model: Any, body: int, root: int) -> bool:
    while body > 0:
        if body == root:
            return True
        body = int(model.body_parentid[body])
    return root == 0


def _collision_shape(mujoco: Any, model: Any, body: int) -> dict[str, Any] | None:
    """§44.3's `collision_shape`: this body's contact geometry as one sphere-swept segment.

    A body may carry several geoms and the kernel evaluates one capsule per frame, so what is
    emitted is a capsule that contains **all** of them: the segment through the two most distant
    geom centres, with a radius large enough to swallow every geom's own bound. That is loose for a
    body made of scattered small geoms, and loose in the only direction a collision bound may err —
    the shape contains what it stands for, so the guard trips early rather than late.

    `None` where nothing can be bounded: a mesh or a height field has no radius here to reason
    about, and inventing one produces a guard that either trips constantly or misses the geometry.
    §44.3 keeps "there is collision geometry" and "here is its shape" as two facts for exactly this
    case.
    """
    geom_type = mujoco.mjtGeom
    start = int(model.body_geomadr[body])
    bounds: list[tuple[list[float], float]] = []
    for geom in range(start, start + int(model.body_geomnum[body])):
        if int(model.geom_contype[geom]) == 0 and int(model.geom_conaffinity[geom]) == 0:
            continue
        kind = int(model.geom_type[geom])
        size = [float(v) for v in model.geom_size[geom]]
        if kind == int(geom_type.mjGEOM_SPHERE):
            radius = size[0]
        elif kind in (int(geom_type.mjGEOM_CAPSULE), int(geom_type.mjGEOM_CYLINDER)):
            # MuJoCo's size is (radius, half-length) about the geom's own z.
            radius = size[0] + size[1]
        elif kind == int(geom_type.mjGEOM_BOX):
            radius = (size[0] ** 2 + size[1] ** 2 + size[2] ** 2) ** 0.5
        else:
            # Mesh, plane, height field, ellipsoid: nothing this reduction can bound honestly.
            return None
        if not (radius > 0.0):
            return None
        bounds.append(([float(v) for v in model.geom_pos[geom]], radius))

    if not bounds:
        return None

    # The two most distant centres carry the segment; every geom's bound sets the radius.
    a, b = bounds[0][0], bounds[0][0]
    span = -1.0
    for i, (pi, _) in enumerate(bounds):
        for pj, _ in bounds[i:]:
            d = sum((pi[k] - pj[k]) ** 2 for k in range(3)) ** 0.5
            if d > span:
                span, a, b = d, pi, pj
    # A geom whose centre is off the segment needs the radius widened by how far off it is. The
    # smaller distance to an endpoint bounds the perpendicular distance from above, so this
    # certainly contains every geom and may be looser than necessary — the permitted direction.
    radius = 0.0
    for p, r in bounds:
        to_a = sum((p[k] - a[k]) ** 2 for k in range(3)) ** 0.5
        to_b = sum((p[k] - b[k]) ** 2 for k in range(3)) ** 0.5
        radius = max(radius, r + min(to_a, to_b))

    return {
        "kind": "capsule",
        "a": [_round12(v) for v in a],
        "b": [_round12(v) for v in b],
        "radius_m": _round12(radius),
    }


def _has_collision(model: Any, body: int) -> bool:
    start = int(model.body_geomadr[body])
    for geom in range(start, start + int(model.body_geomnum[body])):
        if int(model.geom_contype[geom]) != 0 or int(model.geom_conaffinity[geom]) != 0:
            return True
    return False


def build_descriptor(
    mujoco: Any,
    model: Any,
    *,
    robot_slug: str,
    rate_hz: float,
    base: str | None,
    role_overrides: dict[str, str],
    end_effectors: list[dict[str, Any]] | None = None,
    cameras: list[str],
    camera_size: tuple[int, int] | None,
    concurrent_read: bool,
    scene_source: str,
    scene_sha256: str | None,
    tool_version: str,
    renderer_note: str,
) -> DescribeResult:
    """Everything above, assembled.

    `base` may be `None` only when the scene's root is not free-jointed; the caller enforces that,
    because a driver that opened a wheeled robot as a fixed one would refuse every base-velocity
    channel rather than driving it, and the operator would spend a day looking at the policy.
    """
    root_body = _root_body(model)
    bindings, notes = commandable_joints(mujoco, model)
    frames, frame_notes = build_frames(mujoco, model, root_body)
    notes.extend(frame_notes)

    free_root = _free_root_joint(model, root_body)
    resolved_base = base or "fixed"
    declared_effectors = declare_end_effectors(end_effectors or [], bindings, role_overrides)

    joints = []
    for binding in bindings:
        role = role_overrides.get(binding.name)
        joints.append(
            {
                "name": binding.name,
                "role": role,
                "type": binding.joint_type,
                "limits": dict(binding.limits),
                "control_modes": [binding.mode],
            }
        )

    sensors = []
    for camera in cameras:
        width, height = camera_size if camera_size else (None, None)
        sensors.append(
            {
                "name": camera,
                "role": None,
                "modality": "rgb",
                "width": width,
                "height": height,
                # Rendered on demand: the stream's rate is whatever the OS asks for, so there is no
                # frame rate that is a property of the body. Absent, rather than the control rate,
                # which would be a claim about a camera nobody configured.
                "fps": None,
                "frame": _camera_frame(mujoco, model, camera),
                # §89.1. `render` sets it from whether a render thread exists, and it is written
                # as exactly `true` or `false`: anything but `true` reads as undeclared, and a
                # descriptor that said something else would be a promise nobody could parse.
                "concurrent_read": concurrent_read,
            }
        )

    modes = sorted({binding.mode for binding in bindings})
    period_ms = 1000.0 / rate_hz if rate_hz > 0 else None

    descriptor: dict[str, Any] = {
        "schema_version": 1,
        "robot_slug": robot_slug,
        "digest": None,
        "verified": False,
        # §35.3, §70. The flag that keeps every simulated run out of the bench evidence: it caps the
        # certification award at guest whatever the driver's own ceiling is, and it is stamped on
        # every check-in and every evaluation result.
        "simulator": True,
        "base": resolved_base,
        "dof": float(len(bindings)),
        "joints": joints,
        # MJCF has no notion of an end effector — a gripper is a body and a joint like any other —
        # and a transport plan routes a `gripper` channel to a declared effector. So a hand that
        # exists in the kinematics and not here is refused, and the honest fix is a person declaring
        # it with its kind, dof and grip force. Guessing one from a joint named `gripper` is the
        # same mistake as guessing an envelope. The person declares it in the driver configuration
        # (`end_effectors`), as they assert a role (`role_overrides`), and it is recorded as theirs.
        "end_effectors": declared_effectors,
        "sensors": sensors,
        "control": {
            "modes": modes,
            # §86.1: this driver reads the per-joint mode array, and a body whose joints sit in two
            # modes — a mobile manipulator's velocity wheels and position arm — is commandable only
            # because it does (DRY-COVERAGE-PLAN.md F169).
            "per_joint_modes": True,
            "rate_hz_max": rate_hz,
            "rate_hz_min": rate_hz,
            # Exactly one control period, and this is the one descriptor field a simulator can fill
            # in with more confidence than any hardware vendor: `write_command` sets `ctrl` and the
            # next `read_state` integrates it, so the delay from command accepted to force applied
            # is one period by construction rather than by measurement.
            "command_latency_ms": period_ms,
        },
        "envelope": None,
        # The software stop this driver has — `emergency_stop` zeroes every `ctrl`, latches, shows
        # the latch in the next state and refuses every later command, and the suite covers all
        # three — declared in F63's vocabulary: the capability is the software stop, and the rated
        # one is the envelope's to withhold. A simulated stop cuts nothing, which is what the
        # absent envelope and `safety.estop.independent` already say.
        "capabilities": ["safety.estop"],
        "solvers": [],
        "frames": frames,
        "provenance": {
            "tool": f"skynet-mujoco {tool_version}",
            "simulator": "mujoco",
            "driver_ceiling": "guest",
            "ceiling_reason": (
                "Two independent caps, either of which would be enough. It is Python, which cannot "
                "hold a real-time path (§31.3); and it is a simulator, which §35.3 forces to guest "
                "so that no simulated run can be presented as bench evidence."
            ),
            "scene": scene_source,
            "scene_sha256": scene_sha256,
            "limits": (
                "Read from the scene. In a simulator these are not design values that might be "
                "wrong about the hardware — the model is the body, and a joint range in the file is "
                "exactly what this body does. Transport plans built against them are exact for this "
                "body and say nothing about any physical robot the scene resembles."
            ),
            "envelope": (
                "Not declared, and simulation is the strongest reason for it rather than the "
                "weakest. A safety envelope bounds what a machine may do to the people near it; "
                "there is no machine and there are no people. A policy validated against an "
                "envelope minted from a scene file would carry that validation to hardware, which "
                "is the transfer §70 exists to prevent."
            ),
            "timestamps": (
                "Sim time, from the simulator's own clock (§35.3). Sim rarely runs at wall speed, "
                "and because the kernel's time is supplied by the RHAL, watchdogs, rate limits and "
                "deadlines are all measured in the units the simulator is producing. Nothing here "
                "is wall-clock, and comparing these stamps to a wall clock is meaningless."
            ),
            "roles": (
                "None assigned. This generator does not suggest roles: `skynet descriptor "
                "from-urdf` already does, once, with the human-accept step §63.4 requires, and a "
                "second implementation of 'which joint is this' is a second answer to one question. "
                "Supply `role_overrides` in the driver configuration, or import the URDF."
                if not role_overrides
                else f"{len(role_overrides)} assigned from `role_overrides` in the driver "
                "configuration — an operator's assertion, not this tool's inference."
            ),
            "end_effectors": (
                "None declared. MJCF has no end effector, a gripper is a body and a joint like any other, "
                "and guessing one from a joint's name is guessing an envelope. A transport plan routes a "
                "`gripper` channel to a declared effector, so a body without one refuses a policy that "
                "needs a hand. Declare it with `end_effectors` in the driver configuration."
                if not declared_effectors
                else f"{len(declared_effectors)} declared by the operator in `end_effectors` in the driver "
                "configuration — an assertion about which joint is the hand and what kind it is, not "
                "something this tool read out of the scene."
            ),
            "base": (
                "Declared by the operator: the scene's root body carries a free joint, so it moves, "
                "and how it moves is not in the file."
                if free_root is not None
                else "The scene's root body is fixed to the world, so the base is fixed. Nothing "
                "was assumed."
            ),
            "localisation": (
                "`base_pose` is the simulator's ground truth, exact and free of noise. No physical "
                "robot has this. A policy that depends on it will not have it on hardware, and that "
                "dependency is invisible until the day it is not."
            ),
            "cameras": renderer_note,
        },
    }

    if free_root is not None and base is None:
        notes.append(
            Note(
                "action",
                "The root body carries a free joint, so this body moves, and the scene does not say "
                "how. Set `base` in the driver configuration to one of wheeled, tracked, legged, "
                "aerial, rail, surface or submerged.",
            )
        )

    notes.insert(
        0,
        Note(
            "action",
            "`envelope` is null and no configuration changes that. There is no machine to bound and "
            "no risk assessment to read. Guards that need one report unavailable, which is the "
            "correct state for a body that cannot hurt anybody.",
        ),
    )
    notes.append(
        Note(
            "info",
            f"{len(bindings)} of {model.nu} actuators became RHAL joints. "
            f"{sum(1 for b in bindings if 'effort_max_nm' in b.limits)} declare a force ceiling and "
            f"{sum(1 for b in bindings if 'position_min_rad' in b.limits)} a travel range; a channel "
            "bound to a joint with neither is reported unscalable rather than scaled against a "
            "guess.",
        )
    )
    if not role_overrides:
        notes.append(
            Note(
                "info",
                "No roles assigned. A policy channel can still bind by exact joint name; matching "
                "by role needs `role_overrides` in the driver configuration, or a descriptor "
                "imported from the robot's URDF with `skynet descriptor from-urdf "
                "--accept-suggested-roles`.",
            )
        )

    return DescribeResult(
        descriptor=descriptor,
        joints=bindings,
        notes=notes,
        cameras=cameras,
        free_root_joint=free_root,
        root_body=root_body,
    )


END_EFFECTOR_KINDS = ("gripper", "dexterous_hand", "suction", "tool")


def declare_end_effectors(
    declared: list[dict[str, Any]], bindings: list[Any], role_overrides: dict[str, str]
) -> list[dict[str, Any]]:
    """The operator's end effectors, as the descriptor's `end_effectors`, each one a joint this body has.

    The configuration says which joint is the hand and what kind it is — `{"joint": "gripper", "kind":
    "gripper"}` — and optionally its rated grip force and opening. The descriptor's effector takes the
    joint's name (a transport plan binds a `gripper` channel to the effector by that name, and the kernel
    loop commands a joint at the channel's own index, so the two have to be the same word) and the role
    the operator gave the joint, if any. A joint this body does not have, a kind the schema does not
    know, or two effectors on one joint is a refusal with the list of what was possible: a hand declared
    on nothing would route a policy's gripper channel to a name no joint answers to.
    """
    known = {binding.name for binding in bindings}
    out: list[dict[str, Any]] = []
    seen: set[str] = set()
    for entry in declared:
        if not isinstance(entry, dict) or not isinstance(entry.get("joint"), str):
            raise ValueError(
                f"end_effectors entry {entry!r} is not {{\"joint\": <a joint of this body>, \"kind\": "
                f"<{', '.join(END_EFFECTOR_KINDS)}>}}."
            )
        joint, kind = entry["joint"], entry.get("kind", "gripper")
        if joint not in known:
            raise ValueError(
                f"end_effectors names joint {joint!r}, which this body does not have. Its joints are "
                f"{', '.join(sorted(known))}."
            )
        if kind not in END_EFFECTOR_KINDS:
            raise ValueError(f"end_effectors kind {kind!r} is not one of {', '.join(END_EFFECTOR_KINDS)}.")
        if joint in seen:
            raise ValueError(f"end_effectors declares joint {joint!r} twice.")
        seen.add(joint)
        effector: dict[str, Any] = {"name": joint, "role": role_overrides.get(joint), "kind": kind, "dof": 1.0}
        for key in ("max_grip_force_n", "max_opening_m"):
            value = entry.get(key)
            if value is None:
                continue
            if not (isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value > 0.0):
                raise ValueError(f"end_effectors {key} is {value!r}; a rating is a positive number.")
            effector[key] = float(value)
        out.append(effector)
    return out


def _camera_frame(mujoco: Any, model: Any, camera: str) -> str | None:
    """The frame a camera's readings are expressed in — the body it is mounted on (§44)."""
    index = mujoco.mj_name2id(model, mujoco.mjtObj.mjOBJ_CAMERA, camera)
    if index < 0:
        return None
    body = int(model.cam_bodyid[index])
    return _name(mujoco, model, mujoco.mjtObj.mjOBJ_BODY, body) or None


def nan() -> float:
    return math.nan
