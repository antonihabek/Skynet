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
    "cell_workspace_layer",
    "context_to_json",
    "validate_cell",
    "validate_to_json",
]

#: Facts this file derives, which a cell's own ``facts`` may not shadow.
#:
#: A precondition reading ``env.cell`` must be reading the cell's name, on every robot, whatever
#: somebody typed into their configuration.
DERIVED_FACTS: Final = ("cell", "fixtures")

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

    return faults


def cell_environment(cell: dict[str, Any]) -> dict[str, Any]:
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
