"""The body descriptor's frame graph (§44.3) — the Python edition of the frame half of
``reference/typescript/embodiment.ts``.

Two checks, and the split between them is the point.

`validate_frames` is **graph-only**: it is given a tree and asked whether the tree is well formed,
and it answers without ever seeing the descriptor the tree came out of. That leaves one whole class
of defect uncaught, because the two most important names in a dynamic frame — ``joint`` and
``mimics`` — do not refer to anything in the graph. They refer to the joint list.

`validate_frame_bindings` closes it. A joint name that resolves to nothing presents very far from
its cause: the kernel binds channels by joint name, finds nothing for that frame, `solve` marks it
unresolved, and both pose guards trip on it every cycle for as long as the descriptor says what it
says. That is correct behaviour and it is also useless — the robot holds still and the reason is a
spelling in a file nothing was checking. §44.5 asks for the refusal to arrive **while somebody is
still looking at the file**.

It does *not* require that every declared joint have a frame. A partial tree is legitimate — a
gripper whose fingers nobody modelled is a body with joints and no frames for them — and §44.5
already has the guard report unavailable naming the frame it wanted. "Complete" means every name the
graph uses resolves, not that the graph covers the body.

# Where Python is not JavaScript

**JSON has no ``undefined``, and this module needs the distinction.** ``node.mimic_multiplier !==
undefined`` is true for an explicit ``null`` and false for an absent key, and the two lead to
different faults: an absent coefficient is silence, a null one is a linkage computed from a value
that is not a number. Key **presence** is the faithful reading of that after a JSON round trip, so
this file asks ``"mimic_multiplier" in node`` where the reference asks ``!== undefined`` and
``node.get(...) is None`` where it asks ``=== undefined || === null``.
"""

from __future__ import annotations

import math
from typing import Any, Final

__all__ = [
    "COLLISION_SHAPES",
    "FRAME_KINDS",
    "detection_to_json",
    "frame_fault_to_json",
    "has_people_detection",
    "validate_frame_bindings",
    "validate_frames",
]

FRAME_KINDS: Final = ["root", "static", "dynamic"]
COLLISION_SHAPES: Final = ["capsule"]


def _fault(code: str, subjects: list[str], detail: str) -> dict[str, Any]:
    return {"code": code, "subjects": subjects, "detail": detail}


def _number(value: Any) -> str:
    """A number inside a sentence, rendered as the reference's template literal renders it.

    JavaScript writes a whole-valued double without a fractional part — ``${0}`` is ``0``, never
    ``0.0`` — so a Python ``float`` that arrived from JSON has to be printed the same way, or every
    detail string carrying a measurement diverges by two characters.
    """
    if isinstance(value, float) and math.isfinite(value) and value.is_integer():
        return str(int(value))
    return str(value)


def validate_frames(graph: dict[str, Any]) -> dict[str, Any] | None:
    """§44.3's graph rules, answered without ever seeing the descriptor the tree came out of."""
    nodes: list[dict[str, Any]] = graph.get("nodes", [])
    if len(nodes) == 0:
        return _fault("empty", [], "The graph declares no frames.")

    seen: set[str] = set()
    for node in nodes:
        if node["name"] in seen:
            return _fault(
                "duplicate_name",
                [node["name"]],
                f"Two frames are named {node['name']}, so a parent reference is ambiguous.",
            )
        seen.add(node["name"])

    roots = [node["name"] for node in nodes if node.get("parent") is None]
    if len(roots) > 1:
        return _fault(
            "several_roots",
            roots,
            f"{len(roots)} frames have no parent, so this is a forest and not a tree.",
        )
    if graph.get("root") not in seen:
        return _fault(
            "root_missing",
            [graph.get("root")],
            f"The declared root {graph.get('root')} is not one of the frames.",
        )

    for node in nodes:
        if node.get("kind") not in FRAME_KINDS:
            return _fault(
                "unknown_kind",
                [node["name"]],
                f"Frame {node['name']} has kind {node.get('kind')}, which is not root, static or dynamic.",
            )
        has_joint = node.get("joint") is not None
        if (node["kind"] == "dynamic") != has_joint:
            detail = (
                f"Frame {node['name']} is dynamic and names no joint."
                if node["kind"] == "dynamic"
                else f"Frame {node['name']} is {node['kind']} and names a joint."
            )
            return _fault("joint_mismatch", [node["name"]], detail)
        parent = node.get("parent")
        if parent is not None and parent not in seen:
            return _fault(
                "parent_missing",
                [node["name"], parent],
                f"Frame {node['name']} names the parent {parent}, which is not in the graph.",
            )

    # Every frame reaches the root by following parents. The walk is bounded by the node count, so a
    # cycle is a walk that ran out of frames rather than one that ran forever.
    by_name = {node["name"]: node for node in nodes}
    for node in nodes:
        here = node
        for _ in range(len(nodes) + 1):
            if here.get("parent") is None:
                break
            here = by_name[here["parent"]]
        if here.get("parent") is not None:
            return _fault(
                "cycle",
                [node["name"]],
                f"Following parents from {node['name']} never reaches the root.",
            )

    # The collision geometry. Checked here rather than by the guard that consumes it: a shape nobody
    # can read presents as a limb quietly missing from the collision test, and an ignore list naming
    # a frame that does not exist presents as a pair that trips when the operator believed they had
    # excluded it.
    for node in nodes:
        shape = node.get("collision_shape")
        if shape is not None:
            if shape.get("kind") not in COLLISION_SHAPES:
                return _fault(
                    "unknown_shape",
                    [node["name"]],
                    f"Frame {node['name']} declares collision geometry of kind {shape.get('kind')}, "
                    f"which this edition cannot evaluate.",
                )
            radius = shape.get("radius_m")
            finite_positive = (
                not isinstance(radius, bool)
                and isinstance(radius, (int, float))
                and math.isfinite(radius)
                and radius > 0
            )
            if not finite_positive:
                return _fault(
                    "unknown_shape",
                    [node["name"]],
                    f"Frame {node['name']}'s collision geometry has radius {_number(radius)}. A capsule of "
                    f"no radius bounds nothing, and a guard built on it would report clearance it has not "
                    f"established.",
                )
            if node.get("collision") is not True:
                return _fault(
                    "collision_mismatch",
                    [node["name"]],
                    f"Frame {node['name']} declares a collision shape and says it carries no collision "
                    f"geometry. One of the two is wrong and nothing here can tell which.",
                )
        for ignored in node.get("collision_ignore") or []:
            if not any(other["name"] == ignored for other in nodes):
                return _fault(
                    "ignore_missing",
                    [node["name"], ignored],
                    f"Frame {node['name']} is permitted to touch {ignored}, which is not one of the "
                    f"frames. An exception naming nothing excludes nothing.",
                )

    return None


def validate_frame_bindings(graph: dict[str, Any], joints: list[str]) -> dict[str, Any] | None:
    """Check a frame graph against the joint list of the body it describes."""
    declared = set(joints)

    for node in graph.get("nodes", []):
        if node.get("kind") == "dynamic" and node.get("joint") is not None:
            if node["joint"] not in declared:
                return _fault(
                    "dangling_joint",
                    [node["name"], node["joint"]],
                    f"Frame {node['name']} moves with {node['joint']}, which the body does not "
                    f"declare. Nothing binds it, so its pose never resolves and both pose guards "
                    f"trip on it every cycle.",
                )

        mimics = node.get("mimics")
        # Presence, not truthiness: an explicit null coefficient is a declared value that is not a
        # number, and the reference tells it apart from an absent one.
        has_coefficient = "mimic_multiplier" in node or "mimic_offset" in node

        if mimics is None:
            if has_coefficient:
                return _fault(
                    "mimic_coefficient",
                    [node["name"]],
                    f"Frame {node['name']} declares a mimic multiplier or offset and no `mimics`. "
                    f"A coefficient with nothing to multiply describes a linkage that does not exist.",
                )
            continue

        if mimics not in declared:
            return _fault(
                "mimic_missing",
                [node["name"], mimics],
                f"Frame {node['name']} follows {mimics}, which the body does not declare. "
                f"A linkage read from a joint nobody drives never resolves, and takes every frame "
                f"below it with it.",
            )

        if node.get("joint") == mimics:
            return _fault(
                "mimic_self",
                [node["name"], mimics],
                f"Frame {node['name']} follows the joint it is driven by. "
                f"`value = multiplier × driver + offset` over one value is a fixed point, not a "
                f"linkage.",
            )

        for label in ("mimic_multiplier", "mimic_offset"):
            if label not in node:
                continue
            value = node[label]
            finite = (
                not isinstance(value, bool)
                and isinstance(value, (int, float))
                and math.isfinite(value)
            )
            if not finite:
                return _fault(
                    "mimic_coefficient",
                    [node["name"]],
                    f"Frame {node['name']} declares {label} as a value that is not a finite number. "
                    f"A linkage computed from it produces a pose that is not one.",
                )

    return None


def frame_fault_to_json(fault: dict[str, Any] | None) -> Any:
    """``shapeFrameFault`` — present and null, so "checked and sound" cannot read as "not checked"."""
    if fault is None:
        return {"fault": None}
    return {"fault": {"code": fault["code"], "subjects": fault["subjects"], "detail": fault["detail"]}}


# ── §5 detects_people ────────────────────────────────────────────────────────


def has_people_detection(descriptor: dict[str, Any]) -> bool:
    """Does this body declare something that reports person distance? (§5, §7.1's ``proximity``.)

    The derivation ``resolve_limits``'s seventh ``available`` input has been waiting for. Its two
    neighbours are read straight off the descriptor — a sensor whose modality is ``imu``, a base
    that is ``legged`` — and this one could not be, because §5's modalities say what a sensor
    *produces* and detection is what somebody does with the output.

    **Any one sensor is enough.** The question is about the robot rather than about a stream, so
    there is no quorum: two detectors do not make the claim stronger and one that stopped working
    does not make it weaker, which is what the bench and field suites are for.

    **A declaration is honoured and not checked against the modality**, because only the vendor
    knows whether their depth stream is one a person is measured in — see the field's own note in
    ``reference/typescript/embodiment.ts``.

    ── The transcription trap ────────────────────────────────────────────────

    ``is True``, never ``== True`` and never truthiness. The reference tests ``=== true``, which
    admits the boolean and nothing else; ``==`` in Python admits ``1``, and a bare ``if`` admits
    every non-empty string — so ``detects_people: "no"`` would read as a declaration that this
    sensor detects people. Fail-closed is the only defensible direction on a guard input, and the
    corpus pins it with ``a-declaration-that-is-not-a-boolean``.
    """
    return any(sensor.get("detects_people") is True for sensor in descriptor.get("sensors", []))


def detection_to_json(descriptor: dict[str, Any]) -> Any:
    """The corpus's shape: one derived boolean, off a whole descriptor."""
    return {"people_detection": has_people_detection(descriptor)}
