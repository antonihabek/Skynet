"""Forward kinematics and the arithmetic underneath it — the Python edition of
``reference/typescript/kinematics.ts``, which is itself the second implementation of
``crates/skynet-kernel/src/real.rs``.

# Why the trigonometry is written out

``math.sin`` is not required to be correctly rounded, and neither is ECMAScript's, and neither is a
C library's, and a Cortex-M without double-precision hardware runs a fourth routine again. §31.4
requires two editions to emit **byte-identical** canonical JSON, so a pose computed through
``math.sin`` here and through fdlibm there would diverge on the first revolute joint — and the
divergence would look like a specification disagreement rather than what it is.

So the specification defines its own. Addition, subtraction, multiplication and division *are*
correctly rounded on every conforming platform, and CPython's ``float`` is an IEEE-754 double with
exactly those guarantees, so the same sequence of operations produces the same bits here as in the
reference. What follows is that sequence, matching it operation for operation.

Where a line looks redundant — an unconditional second Cody–Waite step, a fixed five Newton
iterations — it is because a branch or an early exit would make the number of operations depend on
the input, and an edition could then take a different number of them without noticing.

# Where Python is not JavaScript

**Every number is a double, and here that has to be made true.** JSON gives Python ``int`` for
``[0, 0, 1]`` and ``float`` for ``[0.0, 0.0, 1.0]``; JavaScript has one type. Integers are exact as
doubles up to 2⁵³ so the arithmetic agrees, but ``math.trunc`` returns an ``int`` where
``Math.trunc`` returns a double, and ``int`` in Python is arbitrary-precision. Every place that
matters converts explicitly, and the quadrant reduction is bounded before the conversion so an
enormous angle cannot turn into an exact integer nobody can represent as a double.

**``bool`` is a subclass of ``int``.** A joint value of ``True`` would be driven as 1 radian. Every
numeric admission checks for it.

**No ``undefined``.** The reference distinguishes an absent joint value from a joint value of null;
JSON does not, and `_value_of` returns ``None`` for both, which is what the reference's
``valueOf`` also collapses them to.
"""

from __future__ import annotations

import math
import struct
from typing import Any, Final

from .embodiment import COLLISION_SHAPES, FRAME_KINDS, validate_frames

__all__ = [
    "IDENTITY",
    "MAX_FRAMES",
    "check_frame_set",
    "collision_pairs",
    "compose",
    "cos",
    "flatten",
    "quaternion_product",
    "rotate",
    "sin",
    "solve",
    "solved_to_json",
    "sqrt",
    "statically_resolvable",
    "validate_frames",
]


# ── Square root ──────────────────────────────────────────────────────────────


def sqrt(x: float) -> float:
    """Newton's iteration from a bit-pattern seed, a fixed five times.

    ``math.sqrt`` is correctly rounded and would agree — but ``core`` has no square root at all, the
    Rust edition therefore computes one, and an oracle that took a shortcut the edition cannot take
    would be checking the wrong function.
    """
    x = float(x)
    if math.isnan(x) or x < 0:
        return math.nan
    if x == 0:
        return x
    if math.isinf(x):
        return x

    bits = struct.unpack(">Q", struct.pack(">d", x))[0]
    seed_bits = ((bits >> 1) + (1023 << 51)) & 0xFFFFFFFFFFFFFFFF
    y = struct.unpack(">d", struct.pack(">Q", seed_bits))[0]
    if y == 0 or not math.isfinite(y):
        y = x

    for _ in range(5):
        y = 0.5 * (y + x / y)
    return y


# ── Range reduction ──────────────────────────────────────────────────────────

#: 2/π, to the nearest double.
_TWO_OVER_PI: Final = 6.36619772367581382433e-1
#: π/2 with its low 33 bits cleared, so ``k * PIO2_1`` is exact for every admitted ``k``.
_PIO2_1: Final = 1.57079632673412561417e0
#: π/2 − PIO2_1, split so the second step recovers the bits the first dropped.
_PIO2_2: Final = 6.0771005063039659766e-11
#: The remainder of that split.
_PIO2_2T: Final = 2.02226624879595063154e-21

#: PIO2_1 carries 33 significant bits, so ``k * PIO2_1`` is exact while ``k`` fits in 20 — about
#: 1.65 × 10⁶ radians, a quarter of a million revolutions. Past that the caller is told rather than
#: handed a pose computed from a reduction that has lost its low bits.
_MAX_QUADRANTS: Final = 1048576


def _round_half_away(v: float) -> float:
    """Round half away from zero. ECMAScript's ``Math.round`` sends −0.5 to −0; this does not."""
    shifted = v + 0.5 if v >= 0 else v - 0.5
    # `math.trunc` returns an int; the reference's returns a double. Converted back immediately, so
    # nothing downstream can be handed an integer Python would keep exactly and a double could not.
    return float(math.trunc(shifted))


def _reduce(x: float) -> tuple[float, int, bool]:
    """``(r, quadrant, exact)``."""
    k = _round_half_away(x * _TWO_OVER_PI)
    if math.isnan(k) or abs(k) >= _MAX_QUADRANTS:
        return 0.0, 0, False

    t = x - k * _PIO2_1
    w0 = k * _PIO2_2
    r0 = t - w0
    w1 = k * _PIO2_2T - (t - r0 - w0)
    r = r0 - w1

    quadrant = int(((math.fmod(k, 4.0)) + 4) % 4)
    return r, quadrant, True


# ── The kernel polynomials, on [-π/4, π/4] ───────────────────────────────────
#
# fdlibm's minimax coefficients, with fdlibm's own digits, so a reviewer can diff them against the
# published table without converting anything. The Rust and TypeScript editions write the same
# characters.

_S1: Final = -1.66666666666666324348e-1
_S2: Final = 8.33333333332248946124e-3
_S3: Final = -1.98412698298579493134e-4
_S4: Final = 2.75573137070700676789e-6
_S5: Final = -2.50507602534068634195e-8
_S6: Final = 1.58969099521155010221e-10

_C1: Final = 4.16666666666666019037e-2
_C2: Final = -1.38888888888741095749e-3
_C3: Final = 2.48015872894767294178e-5
_C4: Final = -2.75573143513906633035e-7
_C5: Final = 2.0875723212981748279e-9
_C6: Final = -1.13596475577881948265e-11


def _kernel_sin(x: float) -> float:
    z = x * x
    poly = _S1 + z * (_S2 + z * (_S3 + z * (_S4 + z * (_S5 + z * _S6))))
    return x + x * z * poly


def _kernel_cos(x: float) -> float:
    z = x * x
    poly = _C1 + z * (_C2 + z * (_C3 + z * (_C4 + z * (_C5 + z * _C6))))
    return 1 - 0.5 * z + z * z * poly


def sin(x: float) -> float:
    """Sine.

    NaN for a non-finite angle and for one outside the range the reduction is exact over. The NaN
    propagates into the pose, and `solve` reads a non-finite pose as an unresolved frame and names
    it — §7.2's rule applied to arithmetic: a guard that cannot compute its input reports rather
    than acting on a number it does not trust.
    """
    x = float(x)
    if not math.isfinite(x):
        return math.nan
    r, quadrant, exact = _reduce(x)
    if not exact:
        return math.nan
    if quadrant == 0:
        return _kernel_sin(r)
    if quadrant == 1:
        return _kernel_cos(r)
    if quadrant == 2:
        return -_kernel_sin(r)
    return -_kernel_cos(r)


def cos(x: float) -> float:
    """Cosine. Same domain rule as `sin`."""
    x = float(x)
    if not math.isfinite(x):
        return math.nan
    r, quadrant, exact = _reduce(x)
    if not exact:
        return math.nan
    if quadrant == 0:
        return _kernel_cos(r)
    if quadrant == 1:
        return -_kernel_sin(r)
    if quadrant == 2:
        return -_kernel_cos(r)
    return _kernel_sin(r)


# ── Transforms ───────────────────────────────────────────────────────────────

#: A rigid transform is ``{"translation": [x, y, z], "rotation": [x, y, z, w]}`` — metres, and a
#: quaternion ordered `xyzw` (§44.4).
IDENTITY: Final = {"translation": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0]}


def quaternion_product(a: list[float], b: list[float]) -> list[float]:
    """Hamilton product, ``xyzw`` in and ``xyzw`` out."""
    ax, ay, az, aw = (float(v) for v in a)
    bx, by, bz, bw = (float(v) for v in b)
    return [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]


def rotate(q: list[float], v: list[float]) -> list[float]:
    """Rotate a point by a quaternion. Active rotation, per §44.4."""
    qx, qy, qz, qw = (float(value) for value in q)
    v0, v1, v2 = (float(value) for value in v)
    tx = 2 * (qy * v2 - qz * v1)
    ty = 2 * (qz * v0 - qx * v2)
    tz = 2 * (qx * v1 - qy * v0)
    return [
        v0 + qw * tx + (qy * tz - qz * ty),
        v1 + qw * ty + (qz * tx - qx * tz),
        v2 + qw * tz + (qx * ty - qy * tx),
    ]


def compose(parent: dict[str, Any], local: dict[str, Any]) -> dict[str, Any]:
    """``parent`` followed by ``local``: the child's transform in the parent's parent."""
    rotated = rotate(parent["rotation"], local["translation"])
    return {
        "translation": [
            float(parent["translation"][0]) + rotated[0],
            float(parent["translation"][1]) + rotated[1],
            float(parent["translation"][2]) + rotated[2],
        ],
        "rotation": quaternion_product(parent["rotation"], local["rotation"]),
    }


def _is_finite_transform(t: dict[str, Any]) -> bool:
    return all(math.isfinite(float(v)) for v in t["translation"]) and all(
        math.isfinite(float(v)) for v in t["rotation"]
    )


# ── The flattened frame set ──────────────────────────────────────────────────

SCALAR_JOINTS: Final = ["revolute", "continuous", "prismatic"]
JOINT_KINDS: Final = ["revolute", "continuous", "prismatic", "ball", "floating"]

MAX_FRAMES: Final = 128
MAX_POSE_TRIPS: Final = 8
MAX_UNRESOLVED: Final = 8

def check_frame_set(frames: list[dict[str, Any]]) -> str | None:
    """Check the ordering invariant once, so that no cycle ever checks it again.

    The invariant — frame ``i``'s parent index is ``< i`` — is what makes forward kinematics a
    single forward pass and makes a cycle in the graph unrepresentable rather than a hang in a
    control loop.
    """
    if len(frames) == 0:
        return "empty"
    if len(frames) > MAX_FRAMES:
        return "too_large"
    if frames[0]["parent"] is not None:
        return "root_not_first"
    for index, frame in enumerate(frames):
        parent = frame["parent"]
        if parent is None:
            if index > 0:
                return "root_not_first"
        else:
            if parent >= len(frames):
                return "parent_out_of_range"
            if parent >= index:
                return "not_topological"
        for ignored in frame.get("ignore", []):
            if ignored >= len(frames):
                return "ignore_out_of_range"
    return None


def statically_resolvable(frames: list[dict[str, Any]], index: int) -> bool:
    """Whether a frame's pose is computable at all, whatever the joint values are.

    False when the walk to the root passes through a joint one scalar cannot describe. A static
    property, and knowing it statically turns "the guard holds the robot every cycle" into "the
    guard reported itself unavailable when the lease was granted, naming the frame" (§44.5).
    """
    here = index
    while True:
        if here < 0 or here >= len(frames):
            return False
        frame = frames[here]
        joint = frame.get("joint")
        if joint is not None and joint["kind"] not in SCALAR_JOINTS:
            return False
        if frame["parent"] is None:
            return True
        here = frame["parent"]


def collision_bodies(frames: list[dict[str, Any]]) -> int:
    """How many frames carry collision geometry this kernel can both read and place."""
    return sum(
        1
        for index, frame in enumerate(frames)
        if frame.get("collision") is not None and statically_resolvable(frames, index)
    )


def collision_pairs(frames: list[dict[str, Any]]) -> int:
    """How many pairs the self-collision guard tests each cycle."""
    n = collision_bodies(frames)
    return (n * (n - 1)) // 2


# ── Solving ──────────────────────────────────────────────────────────────────


def _motion(joint: dict[str, Any], value: float) -> dict[str, Any] | None:
    """The transform a joint value produces, in the joint's own frame.

    ``None`` where a scalar cannot describe the motion, or where the axis is degenerate. A
    zero-length axis is a body-description defect and **not** a fixed joint: reading it as one would
    silently weld a limb in the guard's model of the robot while it moved in the world.
    """
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
        return None
    if joint["kind"] not in SCALAR_JOINTS:
        return None
    axis = _unit(joint["axis"])
    if axis is None:
        return None

    value = float(value)
    if joint["kind"] == "prismatic":
        return {
            "translation": [axis[0] * value, axis[1] * value, axis[2] * value],
            "rotation": [0.0, 0.0, 0.0, 1.0],
        }
    half = 0.5 * value
    s = sin(half)
    c = cos(half)
    if not math.isfinite(s) or not math.isfinite(c):
        return None
    return {"translation": [0.0, 0.0, 0.0], "rotation": [axis[0] * s, axis[1] * s, axis[2] * s, c]}


def _unit(v: list[Any]) -> list[float] | None:
    v0, v1, v2 = (float(value) for value in v)
    squared = v0 * v0 + v1 * v1 + v2 * v2
    if not math.isfinite(squared) or squared <= 0:
        return None
    length = sqrt(squared)
    if not math.isfinite(length) or length <= 0:
        return None
    return [v0 / length, v1 / length, v2 / length]


def solve(
    frames: list[dict[str, Any]],
    bindings: list[dict[str, Any]],
    command: dict[str, Any],
) -> dict[str, Any]:
    """Compute every frame's pose in the root frame: one forward pass.

    A frame is **unresolved** when its parent is, when its joint has no value, when its joint kind
    needs more than a scalar, or when the arithmetic produced something that is not finite — and
    unresolved propagates down the chain, because a pose relative to an unknown pose is unknown.
    """
    poses: list[dict[str, Any] | None] = []
    unresolved: list[int] = []
    lookahead = True

    def value_of(joint_name: str) -> float | None:
        nonlocal lookahead
        binding = next((entry for entry in bindings if entry["joint"] == joint_name), None)
        if binding is None:
            return None
        if binding.get("channel") is None:
            lookahead = False
            return binding.get("measured")
        if binding["channel"] not in command:
            lookahead = False
            return binding.get("measured")
        return command[binding["channel"]]

    for index, frame in enumerate(frames):
        parent_pose: dict[str, Any] = IDENTITY
        if frame["parent"] is not None:
            resolved = poses[frame["parent"]]
            if resolved is None:
                poses.append(None)
                unresolved.append(index)
                continue
            parent_pose = resolved

        local = frame["local"]
        joint = frame.get("joint")
        if joint is not None:
            # A mimic is followed exactly one step. URDF permits no more, and a chain of them would
            # need a traversal with its own cycle check inside the control loop.
            driver = joint.get("mimic")
            raw = value_of(driver["joint"]) if driver else value_of(joint["name"])
            if raw is None or not driver:
                value = raw
            else:
                value = driver["multiplier"] * raw + driver["offset"]
            if value is None:
                poses.append(None)
                unresolved.append(index)
                continue
            moved = _motion(joint, value)
            if moved is None:
                poses.append(None)
                unresolved.append(index)
                continue
            local = compose(local, moved)

        pose = compose(parent_pose, local)
        if not _is_finite_transform(pose):
            poses.append(None)
            unresolved.append(index)
            continue
        poses.append(pose)

    return {"poses": poses, "unresolved": unresolved, "lookahead": lookahead}


# ── Validating and flattening a descriptor's frame graph ─────────────────────


def _fault(code: str, subjects: list[str], detail: str) -> dict[str, Any]:
    return {"code": code, "subjects": subjects, "detail": detail}


def _number(value: Any) -> str:
    """A number inside a sentence, rendered as the reference's template literal renders it."""
    if isinstance(value, float) and value.is_integer() and math.isfinite(value):
        return str(int(value))
    return str(value)


def flatten(graph: dict[str, Any]) -> dict[str, Any]:
    """Flatten a §44.3 frame graph into the order the kernel reads.

    Breadth-first from the root, so a parent is always emitted before its children and the ordering
    invariant holds by construction rather than by a sort that has to be checked afterwards. The
    output order is a function of the tree and of the descriptor's own node order among siblings,
    which makes it stable across editions — a requirement, because verdicts name frames and the
    corpus compares those names in order.

    Returns ``{"frames": [...]}`` or ``{"fault": {...}}``.
    """
    invalid = validate_frames(graph)
    if invalid is not None:
        return {"fault": invalid}

    nodes: list[dict[str, Any]] = graph["nodes"]
    if len(nodes) > MAX_FRAMES:
        return {
            "fault": _fault(
                "too_large",
                [],
                f"The body declares {len(nodes)} frames and this kernel is built for {MAX_FRAMES}. "
                f"Half a frame graph is a robot with a limb missing from its own collision model, "
                f"so the graph is refused rather than truncated.",
            )
        }

    root_index = next((index for index, node in enumerate(nodes) if node["name"] == graph["root"]), -1)
    if root_index < 0:
        return {
            "fault": _fault(
                "root_missing",
                [graph["root"]],
                f"The declared root {graph['root']} is not one of the frames.",
            )
        }

    order: list[int] = [root_index]
    cursor = 0
    while cursor < len(order):
        parent = nodes[order[cursor]]
        for index, node in enumerate(nodes):
            if node.get("parent") == parent["name"]:
                order.append(index)
        cursor += 1

    if len(order) != len(nodes):
        orphan = next((node["name"] for index, node in enumerate(nodes) if index not in order), "")
        return {
            "fault": _fault(
                "unreachable",
                [orphan],
                f"Frame {orphan} is not reachable from the root {graph['root']}. Its parent chain is "
                f"valid and leads somewhere else, which is a second tree in one document.",
            )
        }

    position = [0] * len(nodes)
    for slot, index in enumerate(order):
        position[index] = slot

    frames: list[dict[str, Any]] = []
    for index in order:
        node = nodes[index]
        if node.get("parent") is None:
            parent_slot = None
        else:
            parent_slot = position[next(i for i, other in enumerate(nodes) if other["name"] == node["parent"])]

        joint = None
        if node.get("joint") is not None:
            joint_type = node.get("joint_type")
            if joint_type is None:
                return {
                    "fault": _fault(
                        "unknown_joint_type",
                        [node["name"]],
                        f"Frame {node['name']} names a joint and no joint type, so nothing can tell "
                        f"whether it turns or slides.",
                    )
                }
            if joint_type not in JOINT_KINDS:
                return {
                    "fault": _fault(
                        "unknown_joint_type",
                        [node["name"]],
                        f"Frame {node['name']} names joint type {joint_type}, which is not one of the "
                        f"five §44.3 admits. Reading it as revolute is how a prismatic axis becomes a "
                        f"rotation.",
                    )
                }
            joint = {
                "name": node["joint"],
                "kind": joint_type,
                # URDF's default, stated: the axis is `1 0 0` when the file is silent.
                "axis": node["axis"] if node.get("axis") is not None else [1, 0, 0],
            }
            if node.get("mimics") is not None:
                joint["mimic"] = {
                    "joint": node["mimics"],
                    "multiplier": node["mimic_multiplier"] if node.get("mimic_multiplier") is not None else 1,
                    "offset": node["mimic_offset"] if node.get("mimic_offset") is not None else 0,
                }

        ignore: list[int] = []
        for name in node.get("collision_ignore") or []:
            found = next((i for i, other in enumerate(nodes) if other["name"] == name), -1)
            if found >= 0:
                ignore.append(position[found])
        ignore.sort()

        frame: dict[str, Any] = {
            "name": node["name"],
            "parent": parent_slot,
            "local": {
                "translation": node["translation"] if node.get("translation") is not None else [0, 0, 0],
                # Absent means no rotation, and the identity quaternion is `xyzw = 0 0 0 1` — not
                # `0 0 0 0`, which is not a rotation at all and would collapse every pose below it.
                "rotation": node["rotation"] if node.get("rotation") is not None else [0, 0, 0, 1],
            },
            "ignore": ignore,
        }
        if joint is not None:
            frame["joint"] = joint
        if node.get("collision_shape") is not None:
            shape = node["collision_shape"]
            frame["collision"] = {"a": shape["a"], "b": shape["b"], "radius_m": shape["radius_m"]}

        frames.append(frame)

    return {"frames": frames}


def solved_to_json(graph: dict[str, Any], bindings: list[dict[str, Any]], command: dict[str, Any]) -> Any:
    """``shapeSolved`` — every frame's pose, present and null rather than absent when unresolved.

    "This frame has no pose" is the fact the guards act on, so it is rendered rather than omitted:
    a consumer reading an absent key cannot tell it from a frame nobody mentioned.
    """
    flattened = flatten(graph)
    if "fault" in flattened:
        raise ValueError(f"the graph does not flatten: {flattened['fault']['code']}")
    frames = flattened["frames"]
    solved = solve(frames, bindings, command)

    return {
        "frames": [
            {
                "name": frame["name"],
                "resolved": solved["poses"][index] is not None,
                "translation": solved["poses"][index]["translation"] if solved["poses"][index] else None,
                "rotation": solved["poses"][index]["rotation"] if solved["poses"][index] else None,
            }
            for index, frame in enumerate(frames)
        ],
        "lookahead": solved["lookahead"],
        "unresolved": [frames[index]["name"] for index in solved["unresolved"]],
        "collision_pairs": collision_pairs(frames),
    }


# ── Volumes ──────────────────────────────────────────────────────────────────


def margin(shape: dict[str, Any], point: list[float]) -> float:
    """Signed distance from a point to a volume's boundary: **positive inside**.

    A layer of the operator's workspace configuration (§38.2) is a set of these, and layers compose
    by conjunction — a frame must be inside at least one volume of every layer that names it. That
    is "each layer may only narrow" as a predicate rather than as geometry: intersecting a box with
    a sphere is a shape neither of them is, and a composition rule producing a third shape would be
    one no operator could check by reading their own configuration.
    """
    point = [float(value) for value in point]

    if shape["kind"] == "sphere":
        centre = [float(value) for value in shape["centre"]]
        dx = point[0] - centre[0]
        dy = point[1] - centre[1]
        dz = point[2] - centre[2]
        return float(shape["radius_m"]) - sqrt(dx * dx + dy * dy + dz * dz)

    low = [float(value) for value in shape["min"]]
    high = [float(value) for value in shape["max"]]
    outside = [0.0, 0.0, 0.0]
    any_outside = False
    inside_margin = math.inf
    for axis in range(3):
        below = low[axis] - point[axis]
        above = point[axis] - high[axis]
        if below > 0:
            outside[axis] = below
            any_outside = True
        elif above > 0:
            outside[axis] = above
            any_outside = True
        to_low = point[axis] - low[axis]
        to_high = high[axis] - point[axis]
        nearest = to_low if to_low < to_high else to_high
        if nearest < inside_margin:
            inside_margin = nearest
    if any_outside:
        return -sqrt(outside[0] * outside[0] + outside[1] * outside[1] + outside[2] * outside[2])
    return inside_margin


# ── Distance between two capsules ────────────────────────────────────────────


def _sub(a: list[float], b: list[float]) -> list[float]:
    return [float(a[0]) - float(b[0]), float(a[1]) - float(b[1]), float(a[2]) - float(b[2])]


def _dot(a: list[float], b: list[float]) -> float:
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def _clamp01(v: float) -> float:
    if v > 1:
        return 1.0
    if v > 0:
        return v
    return 0.0


def segment_distance_squared(p1: list[float], q1: list[float], p2: list[float], q2: list[float]) -> float:
    """Closest distance between two segments, **squared**.

    Squared, because the self-collision guard's decision compares it against a squared threshold and
    no square root can therefore move a verdict. The root is taken only for the number the report
    carries.
    """
    d1 = _sub(q1, p1)
    d2 = _sub(q2, p2)
    r = _sub(p1, p2)
    a = _dot(d1, d1)
    e = _dot(d2, d2)
    f = _dot(d2, r)

    epsilon = 1e-18
    if a <= epsilon and e <= epsilon:
        return _dot(r, r)
    if a <= epsilon:
        s = 0.0
        t = _clamp01(f / e)
    else:
        c = _dot(d1, r)
        if e <= epsilon:
            t = 0.0
            s = _clamp01(-c / a)
        else:
            b = _dot(d1, d2)
            denominator = a * e - b * b
            s = _clamp01((b * f - c * e) / denominator) if denominator > epsilon else 0.0
            t = (b * s + f) / e
            if t < 0:
                t = 0.0
                s = _clamp01(-c / a)
            elif t > 1:
                t = 1.0
                s = _clamp01((b - c) / a)

    p1f = [float(v) for v in p1]
    p2f = [float(v) for v in p2]
    c1 = [p1f[0] + d1[0] * s, p1f[1] + d1[1] * s, p1f[2] + d1[2] * s]
    c2 = [p2f[0] + d2[0] * t, p2f[1] + d2[1] * t, p2f[2] + d2[2] * t]
    diff = _sub(c1, c2)
    return _dot(diff, diff)


def capsule_in_world(pose: dict[str, Any], capsule: dict[str, Any]) -> tuple[list[float], list[float]]:
    """A capsule's two endpoints, in the root frame."""
    a = rotate(pose["rotation"], capsule["a"])
    b = rotate(pose["rotation"], capsule["b"])
    translation = [float(value) for value in pose["translation"]]
    return (
        [translation[0] + a[0], translation[1] + a[1], translation[2] + a[2]],
        [translation[0] + b[0], translation[1] + b[1], translation[2] + b[2]],
    )


def ignores(frames: list[dict[str, Any]], a: int, b: int) -> bool:
    """Whether ``a`` lists ``b``, or ``b`` lists ``a``. Symmetric on purpose."""
    return b in frames[a].get("ignore", []) or a in frames[b].get("ignore", [])


def adjacent(frames: list[dict[str, Any]], a: int, b: int) -> bool:
    """Whether one frame is the other's parent. Excluded from the collision test unconditionally."""
    return frames[a]["parent"] == b or frames[b]["parent"] == a


def distance(squared: float) -> float:
    """The specified square root, for a consumer reporting a distance."""
    return sqrt(abs(squared))
