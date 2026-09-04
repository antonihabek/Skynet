"""The canonical role vocabulary (§5).

A role is how two different robots' joints are recognised as the same joint. `left_arm.shoulder.pitch`
on a Unitree H1 and on a Figure 02 are different wire names for the same degree of freedom, and the
role is what lets a policy trained on one be transported onto the other.

The extensibility rule is the load-bearing one, and it is a refusal: **an unrecognised role parses to
nothing, never to a nearby guess.** A body the vocabulary does not describe is still usable — it just
cannot take part in cross-body remapping, which is the truthful outcome, because nobody has
established what its joints mean. A vocabulary that quietly mapped `tentacle.segment.flex` onto the
nearest arm would produce a plan that looks complete and moves the wrong thing.
"""

from __future__ import annotations

import re
from typing import Any, Final

__all__ = [
    "CANONICAL_AXES",
    "CANONICAL_CHAINS",
    "ParsedRole",
    "parse_role",
    "role_key",
    "role_to_json",
]

#: The chains a role may name. `aux` and `tail` are in the list so that a body the specification did
#: not anticipate can still declare something that parses, rather than being pushed into a
#: wrong-but-close chain because the vocabulary had no honest slot for it.
CANONICAL_CHAINS: Final = (
    "left_arm", "right_arm", "left_leg", "right_leg",
    "left_hand", "right_hand", "torso", "neck", "head", "base", "tail", "aux",
)

#: Axis of motion, for a link with more than one degree of freedom. A three-DoF shoulder is three
#: joints in every real driver, and the suffix is what keeps `left_arm.shoulder.pitch` from
#: colliding with `left_arm.shoulder.roll` while both stay recognisable as the same shoulder.
CANONICAL_AXES: Final = (
    "pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip",
)

_ROLE_PATTERN: Final = re.compile(r"^([a-z_]+)\.([a-z0-9_]+)(?:\.([a-z]+))?(?:#(\d+))?$")


class ParsedRole:
    """A role that parsed. Never constructed for one that did not — see :func:`parse_role`."""

    __slots__ = ("chain", "link", "axis", "index")

    def __init__(self, chain: str, link: str, axis: str | None, index: int | None) -> None:
        self.chain = chain
        self.link = link
        self.axis = axis
        self.index = index

    def __repr__(self) -> str:
        return f"ParsedRole({role_key(self)!r})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, ParsedRole):
            return NotImplemented
        return (
            self.chain == other.chain
            and self.link == other.link
            and self.axis == other.axis
            and self.index == other.index
        )


def parse_role(role: str | None) -> ParsedRole | None:
    """Parse a canonical role, or return ``None``.

    ``None`` rather than an exception, and the caller reads it as *"this joint can only be matched
    by exact name"*. Three ways to fail, and all three return the same nothing: the string does not
    match the grammar, the chain is not in the vocabulary, or the axis is not.
    """
    if not role:
        return None

    match = _ROLE_PATTERN.match(role.strip().lower())
    if match is None:
        return None

    chain, link, axis, index = match.groups()
    if chain not in CANONICAL_CHAINS:
        return None
    if axis and axis not in CANONICAL_AXES:
        return None

    return ParsedRole(chain=chain, link=link, axis=axis or None, index=int(index) if index else None)


def role_key(parsed: ParsedRole) -> str:
    """The canonical string form, so two parsed roles compare as keys."""
    axis = f".{parsed.axis}" if parsed.axis else ""
    index = "" if parsed.index is None else f"#{parsed.index}"
    return f"{parsed.chain}.{parsed.link}{axis}{index}"


def role_to_json(parsed: ParsedRole | None) -> Any:
    """The corpus shape: five fields, or ``null`` for a role that did not parse."""
    if parsed is None:
        return None
    return {
        "chain": parsed.chain,
        "link": parsed.link,
        "axis": parsed.axis,
        "index": parsed.index,
        "key": role_key(parsed),
    }
