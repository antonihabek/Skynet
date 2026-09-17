"""The cell descriptor (§55.2) — the physical counterpart of an environment, Python edition.

Transcribed from ``reference/typescript/cell.ts`` and ``reference/typescript/shape.ts``. Every rule
and message string is read from those files, never from ``corpus/``.

§55.1 makes an environment a simulation artifact that only a simulator driver can load. §55.2 names
its real-world analogue and then refuses to make it a repo kind:

    The real-world analogue of an environment — where the fixtures are, which volumes are forbidden,
    where the safe parking pose is, what the operator has declared about the space — is a **cell
    descriptor**, and it belongs in the operator profile (§38), not in the repository catalogue.

    The reason is ownership. An environment is published, versioned and shared; a cell is
    site-specific, changes when somebody moves a table, and is nobody's intellectual property.
    Making it a repo kind would invite people to publish their factory layout to a marketplace, and
    would put safety-relevant workspace volumes behind a fork-and-star workflow.

So there is no publishing here, no versioning and no digest. A cell does exactly two jobs: it
supplies ``env.*`` to §54.2's precondition context, and it supplies allowed volumes to §7.1's fourth
guard as one layer of §38.2's stack — where layers compose by conjunction and can therefore only
narrow.

── The check that only exists because §54.2 does ────────────────────────────

``env.*`` is addressed by a **dot path**, so a fact whose key contains a dot is a fact no
precondition can ever name: ``{"door.closed": true}`` and ``{"door": {"closed": true}}`` are the same
path and different documents, and the first will never match. That is not a hypothetical typo — it
is the obvious way to write a hierarchical key for anybody who has not read ``readPath``.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``key.trim() === key``
    ``String.prototype.trim`` removes a **smaller** set than Python's ``str.strip()``, so a key
    wrapped in ``U+001C`` is unusable to Python and perfectly addressable to the reference. Every
    trim here is :func:`skynet_os.js.js_trim`.

``Object.keys(facts).sort()``
    UTF-16 code-unit order, and the sort is a **compatibility decision rather than a tidy one**: the
    Rust edition holds the facts in a ``BTreeMap``, which has no insertion order to offer, so
    reporting in the order somebody typed would make the two editions name the same faults in
    different orders. A canonical JSON document's key order *is* sorted, so this is document order
    by the time either edition sees one.

``fixture.volume && !shapeIsUsable(...)``
    Truthy, and it has to be: a ``volume`` that is present and ``null`` is a fixture nobody
    measured, not a fixture measured wrongly. ``"volume" in fixture`` would report a fault against
    an operator who wrote the field out and left it empty.

``Number.isFinite``
    Rejects ``NaN`` and the infinities, which Python's ``isinstance(x, float)`` does not, and
    rejects booleans, which ``isinstance(True, int)`` lets through. A ``true`` in a coordinate is not
    a coordinate of 1.
"""

from __future__ import annotations

import math
from typing import Any, Final

from .js import js_trim as _js_trim, utf16_key as _utf16_key

__all__ = [
    "CELL_FAULTS",
    "DERIVED_FACTS",
    "cell_environment",
    "cell_robot_layer",
    "cell_robot_slugs",
    "cell_workspace_layer",
    "context_to_json",
    "robots_to_json",
    "validate_cell",
    "validate_to_json",
]

#: Facts this file derives, which a cell's own ``facts`` may not shadow.
#:
#: A precondition reading ``env.cell`` must be reading the cell's name, on every robot, whatever
#: somebody typed into their configuration.
DERIVED_FACTS: Final = ("cell", "fixtures", "robot", "robots")

CELL_FAULTS: Final = (
    "unnamed",
    "fact_key_unusable",
    "fact_key_reserved",
    "volume_frame_missing",
    "volume_shape_invalid",
    "fixture_unnamed",
    "duplicate_fixture",
    "parking_channel_unnamed",
    "duplicate_parking_channel",
    # §87.1, and all five apply only from the second robot.
    "robot_unnamed",
    "duplicate_robot",
    "robot_unplaced",
    "robot_unbounded",
    "shared_volume",
)


def _fault(code: str, subjects: list[str], detail: str) -> dict[str, Any]:
    return {"code": code, "subjects": subjects, "detail": detail}


def _cell_name(cell: dict[str, Any]) -> str:
    """The cell's name, or the empty string.

    A document whose ``name`` is missing or is not a string has no name, and the empty string is how
    every edition spells that — ``None`` here where the Rust reader defaults to ``""`` would be a
    divergence in one byte of one field, found by a corpus case and not by anybody reading either
    file.
    """
    name = cell.get("name")
    return name if isinstance(name, str) else ""


def _usable_key(key: str) -> bool:
    """A key a dot path can address: non-empty, no dot, and no leading or trailing whitespace."""
    return len(key) > 0 and "." not in key and _js_trim(key) == key


def _finite(value: Any) -> bool:
    """``Number.isFinite``. A ``bool`` is not a number, whatever ``isinstance`` says."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return False
    return not (math.isnan(float(value)) or math.isinf(float(value)))


def _triple(value: Any) -> list[Any] | None:
    """Exactly three numbers, or nothing.

    The finiteness test is deliberately **not** here: a coordinate of ``Infinity`` is decoded and
    then reported as unusable geometry, which is a fault an operator can act on, rather than dropped
    at the boundary into a shape this edition does not recognise.
    """
    if not isinstance(value, list) or len(value) != 3:
        return None
    return None if any(isinstance(item, bool) or not isinstance(item, (int, float))
                       for item in value) else list(value)


def _shape(volume: Any) -> dict[str, Any] | None:
    """Decode one shape, or nothing when this edition does not recognise it."""
    if not isinstance(volume, dict):
        return None
    shape = volume.get("shape")
    if not isinstance(shape, dict):
        return None
    kind = shape.get("kind")
    if kind == "box":
        minimum = _triple(shape.get("min"))
        maximum = _triple(shape.get("max"))
        if minimum is None or maximum is None:
            return None
        return {"kind": "box", "min": minimum, "max": maximum}
    if kind == "sphere":
        centre = _triple(shape.get("centre"))
        radius = shape.get("radius_m")
        if centre is None or isinstance(radius, bool) or not isinstance(radius, (int, float)):
            return None
        return {"kind": "sphere", "centre": centre, "radius_m": radius}
    return None


def _shape_is_usable(volume: Any) -> bool:
    shape = _shape(volume)
    if shape is None:
        return False
    if shape["kind"] == "sphere":
        return (
            _finite(shape["radius_m"])
            and shape["radius_m"] > 0
            and all(_finite(value) for value in shape["centre"])
        )
    if not all(_finite(value) for value in shape["min"]):
        return False
    if not all(_finite(value) for value in shape["max"]):
        return False
    # A box whose min exceeds its max on any axis is empty, and an empty allowed volume is a cell
    # the robot may never be in — which is a configuration mistake rather than a lock-out to enforce
    # silently.
    return all(shape["min"][axis] <= shape["max"][axis] for axis in range(3))


def _frame_is_named(volume: Any) -> bool:
    if not isinstance(volume, dict):
        return False
    frame = volume.get("frame")
    return isinstance(frame, str) and _js_trim(frame) != ""


# ── §87.1's intersection test ────────────────────────────────────────────────


def _placed(shape: dict[str, Any], at: list[float]) -> dict[str, Any]:
    """A shape moved into the cell's frame by its robot's placement.

    Translation only. §44.4's transforms carry a rotation and this deliberately does not read one: a
    rotated box is not a box, so the test would need an oriented-box routine whose disagreements
    between eight editions would be in the last bits of a square root. A cell whose robots are
    mounted at an angle is one whose operator draws axis-aligned volumes large enough to hold the
    rotation, which over-reports sharing and never under-reports it.
    """
    if shape["kind"] == "sphere":
        return {
            "kind": "sphere",
            "centre": [shape["centre"][axis] + at[axis] for axis in range(3)],
            "radius_m": shape["radius_m"],
        }
    return {
        "kind": "box",
        "min": [shape["min"][axis] + at[axis] for axis in range(3)],
        "max": [shape["max"][axis] + at[axis] for axis in range(3)],
    }


def _box_point_distance_squared(box: dict[str, Any], point: list[float]) -> float:
    """Squared distance from a point to a box, zero inside."""
    total = 0.0
    for axis in range(3):
        value = point[axis]
        low = box["min"][axis]
        high = box["max"][axis]
        outside = low - value if value < low else value - high if value > high else 0.0
        total += outside * outside
    return total


def _intersects(a: dict[str, Any], b: dict[str, Any]) -> bool:
    """Whether two placed shapes share a point.

    **Inclusive, and that is the decision rather than an off-by-one.** ``margin`` counts zero as
    inside — a workspace trip is ``margin < 0`` — so two robots whose boxes meet on a face can each
    legally be at the point where they meet. A strict test would call that configuration disjoint,
    and the platform would have certified the one arrangement where both arms are allowed into the
    same millimetre.
    """
    if a["kind"] == "box" and b["kind"] == "box":
        return all(
            a["min"][axis] <= b["max"][axis] and b["min"][axis] <= a["max"][axis]
            for axis in range(3)
        )
    if a["kind"] == "sphere" and b["kind"] == "sphere":
        reach = a["radius_m"] + b["radius_m"]
        squared = sum(
            (a["centre"][axis] - b["centre"][axis]) ** 2 for axis in range(3)
        )
        return squared <= reach * reach
    box = a if a["kind"] == "box" else b
    sphere = a if a["kind"] == "sphere" else b
    return _box_point_distance_squared(box, sphere["centre"]) <= sphere["radius_m"] ** 2


def _read_placement(robot: dict[str, Any]) -> list[float] | None:
    """The placement, or ``None`` when the operator wrote none the arithmetic can use."""
    at = _triple(robot.get("at"))
    if at is None or not all(_finite(value) for value in at):
        return None
    return [float(value) for value in at]


def _placed_volumes(robot: dict[str, Any]) -> list[tuple[str, dict[str, Any]]]:
    """A robot's usable volumes, placed. A volume with no usable shape is not compared."""
    at = _read_placement(robot)
    if at is None:
        return []
    placed: list[tuple[str, dict[str, Any]]] = []
    for volume in robot.get("allowed") or []:
        if not _shape_is_usable(volume):
            continue
        shape = _shape(volume)
        assert shape is not None
        frame = volume.get("frame") if isinstance(volume, dict) else ""
        placed.append((frame if isinstance(frame, str) else "", _placed(shape, at)))
    return placed


def _robot_slug(robot: Any) -> str:
    """A robot's slug, or the empty string — ``_cell_name``'s rule, one level down."""
    if not isinstance(robot, dict):
        return ""
    slug = robot.get("slug")
    return slug if isinstance(slug, str) else ""


def _robot_faults(cell: dict[str, Any]) -> list[dict[str, Any]]:
    """§87.1's five faults, or none.

    Split out rather than inlined because all five are conditional on the same thing — **two or more
    robots** — and a reader has to be able to see that condition once. One robot is §55.2's cell and
    has no pair to be disjoint from; the placement rule would otherwise refuse every cell anybody
    has ever written.

    Reported in declaration order, then the pairs in index order, so every edition names the same
    fault first. The disjointness walk skips a robot that already has a fault of its own, for
    :func:`validate_cell`'s duplicate-fixture reason.
    """
    robots = cell.get("robots") or []
    faults: list[dict[str, Any]] = []
    slugs: set[str] = set()
    comparable: list[tuple[str, list[tuple[str, dict[str, Any]]]]] = []
    several = len(robots) >= 2

    for index, robot in enumerate(robots):
        robot = robot if isinstance(robot, dict) else {}
        slug = _robot_slug(robot)
        if _js_trim(slug) == "":
            faults.append(
                _fault(
                    "robot_unnamed",
                    [f"robots[{index}]"],
                    "A robot a precondition cannot name is a robot no skill can ask about. "
                    "`env.robot` is its slug.",
                )
            )
            continue
        if slug in slugs:
            faults.append(
                _fault(
                    "duplicate_robot",
                    [slug],
                    "Two entries with one slug: a precondition asking about it would be asking "
                    "about whichever the reader happened to reach.",
                )
            )
            continue
        slugs.add(slug)

        # Everything below is about the pair, so a cell with one robot stops here.
        if not several:
            continue

        if _read_placement(robot) is None:
            faults.append(
                _fault(
                    "robot_unplaced",
                    [slug],
                    "This cell holds more than one robot, and this one carries no `at`. Its "
                    "volumes are in its own root frame (§44.2), so nothing can say whether they "
                    "overlap another robot's until somebody says where it stands.",
                )
            )
            continue
        volumes = _placed_volumes(robot)
        if not volumes:
            faults.append(
                _fault(
                    "robot_unbounded",
                    [slug],
                    "This cell holds more than one robot, and this one draws no volume the "
                    "workspace guard can evaluate. A robot that may be anywhere is not disjoint "
                    "from anything.",
                )
            )
            continue
        comparable.append((slug, volumes))

    for a in range(len(comparable)):
        for b in range(a + 1, len(comparable)):
            first_slug, first_volumes = comparable[a]
            second_slug, second_volumes = comparable[b]
            for one_frame, one_shape in first_volumes:
                for other_frame, other_shape in second_volumes:
                    if not _intersects(one_shape, other_shape):
                        continue
                    faults.append(
                        _fault(
                            "shared_volume",
                            [first_slug, second_slug, one_frame, other_frame],
                            f"{first_slug}'s {one_frame} and {second_slug}'s {other_frame} share "
                            "a point once both robots are placed. §87.1 gives each robot its own "
                            "kernel, so nothing on this machine evaluates the pair — the overlap "
                            "is a claim the platform cannot enforce rather than a hazard it has "
                            "measured.",
                        )
                    )

    return faults


def validate_cell(cell: dict[str, Any]) -> list[dict[str, Any]]:
    """Every fault in a cell descriptor.

    Fact keys are walked in sorted order; everything else in the order it was written.

    A **list** rather than the first one, unlike ``validate_frames`` next door, and the difference is
    about who is reading. A frame graph that does not form a tree cannot be analysed further, so one
    fault is all there is to say. A cell descriptor's problems are independent, an operator is
    editing it by hand, and ``admit_package``'s reason applies unchanged: fixing four problems one
    deployment at a time is how people end up giving up and granting everything.
    """
    faults: list[dict[str, Any]] = []

    name = cell.get("name")
    if not isinstance(name, str) or _js_trim(name) == "":
        faults.append(
            _fault("unnamed", [], "A cell has no identity but its name, and this one has none.")
        )

    facts = cell.get("facts")
    facts = facts if isinstance(facts, dict) else {}
    for key in sorted(facts, key=_utf16_key):
        if not _usable_key(key):
            faults.append(
                _fault(
                    "fact_key_unusable",
                    [key],
                    f"A precondition addresses this as env.{key}, and a dot path cannot name it. "
                    "Nest the object instead of putting a dot in the key.",
                )
            )
            continue
        if key in DERIVED_FACTS:
            faults.append(
                _fault(
                    "fact_key_reserved",
                    [key],
                    f"env.{key} is derived from the cell itself, so a fact of that name would be "
                    "read by nobody and would make a precondition true for the wrong reason.",
                )
            )

    for index, volume in enumerate(cell.get("allowed") or []):
        if not _frame_is_named(volume):
            faults.append(
                _fault(
                    "volume_frame_missing",
                    [f"allowed[{index}]"],
                    "A volume constrains a frame, and this one names none. The workspace guard "
                    "would have nothing to check it against.",
                )
            )
        if not _shape_is_usable(volume):
            faults.append(
                _fault(
                    "volume_shape_invalid",
                    [f"allowed[{index}]"],
                    "This volume has no usable geometry — a box whose minimum exceeds its maximum, "
                    "a sphere with no radius, or a shape this edition does not know.",
                )
            )

    seen_fixtures: set[str] = set()
    for index, fixture in enumerate(cell.get("fixtures") or []):
        fixture = fixture if isinstance(fixture, dict) else {}
        fixture_name = fixture.get("name")
        if not isinstance(fixture_name, str) or _js_trim(fixture_name) == "":
            faults.append(
                _fault(
                    "fixture_unnamed",
                    [f"fixtures[{index}]"],
                    "A fixture a precondition cannot name is a fixture the OS cannot check for.",
                )
            )
            continue
        if fixture_name in seen_fixtures:
            faults.append(
                _fault(
                    "duplicate_fixture",
                    [fixture_name],
                    "Two fixtures with one name: a precondition asking for it would be asking "
                    "about whichever the reader happened to reach.",
                )
            )
            continue
        seen_fixtures.add(fixture_name)
        # Truthy, deliberately: a `volume` that is present and null is a fixture nobody measured.
        if fixture.get("volume") and not _shape_is_usable(fixture.get("volume")):
            faults.append(
                _fault(
                    "volume_shape_invalid",
                    [fixture_name],
                    "This fixture has been given a volume with no usable geometry. A fixture may "
                    "be named without being measured; it may not be measured wrongly.",
                )
            )

    seen_channels: set[str] = set()
    for index, entry in enumerate(cell.get("parking_pose") or []):
        entry = entry if isinstance(entry, dict) else {}
        channel = entry.get("channel")
        if not isinstance(channel, str) or _js_trim(channel) == "":
            faults.append(
                _fault(
                    "parking_channel_unnamed",
                    [f"parking_pose[{index}]"],
                    "A parking pose is a command, and a command names its channel.",
                )
            )
            continue
        if channel in seen_channels:
            faults.append(
                _fault(
                    "duplicate_parking_channel",
                    [channel],
                    "One channel, two parking positions. Which one the robot drives to would "
                    "depend on the order a reader happened to walk the list.",
                )
            )
            continue
        seen_channels.add(channel)

    faults.extend(_robot_faults(cell))

    return faults


def cell_robot_slugs(cell: dict[str, Any]) -> list[str]:
    """Every slug the cell names, sorted and without the ones it got wrong (§87.1).

    Sorted for :func:`validate_cell`'s reason one field along: a precondition asks ``includes``, so
    order is not semantic — but two editions emitting one list in two orders is a corpus failure,
    and sorted is the order every edition can produce. A duplicate appears once, because
    ``env.robots`` answers *which robots*, and ``duplicate_robot`` is where the document's mistake
    is reported.
    """
    slugs: list[str] = []
    for robot in cell.get("robots") or []:
        slug = _robot_slug(robot)
        if _js_trim(slug) == "" or slug in slugs:
            continue
        slugs.append(slug)
    return sorted(slugs, key=_utf16_key)


def cell_robot_layer(cell: dict[str, Any], slug: str) -> dict[str, Any] | None:
    """The layer one robot of a shared cell contributes, or ``None`` (§87.1).

    A **second** layer rather than a replacement for :func:`cell_workspace_layer`, because §38.2's
    layers compose by conjunction and two layers is how a cell-wide volume and a per-robot one both
    hold. A caller that substituted this for the cell's own would be letting a robot's own volume
    *widen* what the cell allowed everything, which is the one thing a layer may not do.

    The volumes are **unplaced**, in the robot's own root frame, because that is the frame the
    ``workspace`` guard resolves poses in (§44.2).
    """
    if _js_trim(slug) == "":
        return None
    for robot in cell.get("robots") or []:
        if _robot_slug(robot) != slug:
            continue
        volumes = [
            volume
            for volume in ((robot.get("allowed") if isinstance(robot, dict) else None) or [])
            if _shape_is_usable(volume) and _frame_is_named(volume)
        ]
        if not volumes:
            return None
        return {"name": f"cell:{_cell_name(cell)}:{slug}", "volumes": volumes}
    return None


def cell_environment(cell: dict[str, Any], subject: str | None = None) -> dict[str, Any]:
    """The ``env.*`` branch of a precondition context (§54.2).

    Two derived facts and then the operator's own. Derived first and unshadowable, which
    :func:`validate_cell` enforces rather than this function silently arbitrating: a cell whose facts
    overwrote its own name would answer a precondition truthfully about the wrong thing.

    A cell with faults still produces a context. Refusing to would mean an operator's typo in a
    fixture name took away the battery check as well, and §54.2 already has an answer for a fact that
    cannot be read: the precondition naming it is refused, individually, with the path named.
    """
    facts = cell.get("facts")
    facts = facts if isinstance(facts, dict) else {}
    environment: dict[str, Any] = {
        key: value
        for key, value in facts.items()
        if _usable_key(key) and key not in DERIVED_FACTS
    }
    environment["cell"] = _cell_name(cell)
    environment["fixtures"] = [
        fixture["name"]
        for fixture in (cell.get("fixtures") or [])
        if isinstance(fixture, dict)
        and isinstance(fixture.get("name"), str)
        and _js_trim(fixture["name"]) != ""
    ]
    # §87.1. `env.robot` answers *which robot does this cell say I am*, so a slug the cell has not
    # been told about is reported as the empty string rather than echoed back: an edition that
    # echoed it would let a precondition assert membership of a cell by asserting it.
    declared = cell_robot_slugs(cell)
    environment["robot"] = subject if isinstance(subject, str) and subject in declared else ""
    environment["robots"] = declared
    return environment


def cell_workspace_layer(cell: dict[str, Any]) -> dict[str, Any] | None:
    """The cell's contribution to the ``workspace`` guard, or ``None`` when it draws no volumes.

    One layer of §38.2's stack, named after the cell so an operator reading a workspace trip can tell
    which layer stopped the robot. Layers compose by conjunction — a frame must be inside at least
    one volume of *every* layer that names it — so adding this one can only narrow what the site
    above it allowed, which is §38.2's rule holding by construction rather than by a check.

    **A cell that draws nothing adds no layer**, and that is not the same as a cell that allows
    nothing. An empty layer would forbid every frame it named, and a site that has not yet measured
    its cell would find the robot refusing to move with no volume anywhere to explain why.
    """
    volumes = [
        volume
        for volume in (cell.get("allowed") or [])
        if _shape_is_usable(volume) and _frame_is_named(volume)
    ]
    if not volumes:
        return None
    return {"name": f"cell:{_cell_name(cell)}", "volumes": volumes}


# ── The wire shapes ──────────────────────────────────────────────────────────


def validate_to_json(case_input: dict[str, Any]) -> Any:
    """``cell/validate`` — what an operator sees when they save it."""
    return {
        "faults": [
            {"code": fault["code"], "subjects": list(fault["subjects"]), "detail": fault["detail"]}
            for fault in validate_cell(case_input["cell"])
        ]
    }


def _shape_json(shape: dict[str, Any]) -> dict[str, Any]:
    if shape["kind"] == "sphere":
        return {"kind": "sphere", "centre": list(shape["centre"]), "radius_m": shape["radius_m"]}
    return {"kind": "box", "min": list(shape["min"]), "max": list(shape["max"])}


def context_to_json(case_input: dict[str, Any]) -> Any:
    """``cell/context`` — what a precondition and the workspace guard read out of it afterwards.

    ``layer`` is nullable rather than absent, and the distinction is §55.2's: a cell nobody has
    measured and a cell that permits nothing are different facts, and only one of them should stop a
    robot.
    """
    cell = case_input["cell"]
    layer = cell_workspace_layer(cell)
    return {
        "env": cell_environment(cell),
        "layer": None
        if layer is None
        else {
            "name": layer["name"],
            "volumes": [
                {"frame": volume["frame"], "shape": _shape_json(_shape(volume))}
                for volume in layer["volumes"]
            ],
        },
    }


def robots_to_json(case_input: dict[str, Any]) -> Any:
    """``cell/robots`` — §87.1's three projections of one document.

    One document rather than three, because they are one question — *what does this cell say about
    its robots* — and an edition could agree about the disjointness arithmetic while disagreeing
    about which layer a robot contributes, which is the half a workspace guard reads.
    """
    cell = case_input["cell"]
    subject = case_input.get("robot")
    layers = []
    for robot in cell.get("robots") or []:
        slug = _robot_slug(robot)
        layer = cell_robot_layer(cell, slug)
        layers.append(
            {
                "slug": slug,
                "layer": None
                if layer is None
                else {
                    "name": layer["name"],
                    "volumes": [
                        {"frame": volume["frame"], "shape": _shape_json(_shape(volume))}
                        for volume in layer["volumes"]
                    ],
                },
            }
        )
    return {
        "env": cell_environment(cell, subject if isinstance(subject, str) else None),
        "slugs": cell_robot_slugs(cell),
        "layers": layers,
        "faults": [
            {"code": fault["code"], "subjects": list(fault["subjects"]), "detail": fault["detail"]}
            for fault in validate_cell(cell)
        ],
    }
