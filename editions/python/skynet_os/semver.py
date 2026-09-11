"""Semantic versions and ranges (§54.2, §8).

Two questions with two different consumers, and the corpus holds both:

* **compare** — how two versions order. Everything that renders a version list, and the OS-too-old
  check at admission.
* **satisfies** — whether a version falls inside a declared range. §54.2 made `semver_satisfies` an
  operator a robot evaluates for a skill's preconditions, which is what turned this from a utility
  into a contract.

`is_valid_range` exists separately from `satisfies` for a reason worth keeping: `satisfies` answers
false both for *"this version is outside that window"* and for *"that is not a window"*, and §54.2
needs those apart. A skill whose precondition names an unreadable range is a skill nobody can ever
run, and its publisher is the one who has to hear about it; a battery that is too low is a different
sentence with a different remedy.
"""

from __future__ import annotations

import re
from typing import Any, Final

from .js import JS_SPACE as _JS_SPACE, js_trim as _js_trim

__all__ = [
    "ParsedVersion",
    "compare_to_json",
    "compare_versions",
    "is_valid_range",
    "is_valid_version",
    "parse_version",
    "satisfies",
    "satisfies_to_json",
    "version_to_json",
]

_SEMVER_RE: Final = re.compile(r"^([0-9]+)\.([0-9]+)\.([0-9]+)(?:-([0-9A-Za-z.-]+))?\Z")
_COMPARATOR_RE: Final = re.compile(r"^(>=|<=|>|<|\^|~|=)?" + _JS_SPACE + r"*(.+)\Z", re.S)
_TOKEN_RE: Final = re.compile(
    r"(?:>=|<=|>|<|\^|~|=)?" + _JS_SPACE + r"*[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?"
)
_NUMERIC_RE: Final = re.compile(r"^[0-9]+\Z")


def _number(text: str) -> float:
    """Read a version component the way the reference edition does: as an IEEE-754 double.

    **Not** as a Python ``int``, and this is the one place where Python's arbitrary-precision
    integers are a hazard rather than a convenience. JavaScript's ``Number()`` and Rust's ``f64``
    both round `9007199254740993` to `9007199254740992`; Python's ``int()`` keeps it exactly. An
    edition that parsed exactly would order two versions the reference edition calls equal — and
    would then *serialise* them identically, because canonical form renders both as the rounded
    value. A disagreement about ordering that is invisible in the output is the worst shape a
    divergence can take.

    The corpus case is called `large-major` and its note says so in advance: *"a value beyond an
    integer's exact range rounds rather than failing to parse. Both editions must round it the same
    way."* It failed on this edition's first corpus run, which is what the corpus is for.
    """
    return float(text)


class ParsedVersion:
    __slots__ = ("major", "minor", "patch", "prerelease", "raw")

    def __init__(self, major: float, minor: float, patch: float, prerelease: str | None, raw: str) -> None:
        self.major = major
        self.minor = minor
        self.patch = patch
        self.prerelease = prerelease
        self.raw = raw

    def __repr__(self) -> str:
        return f"ParsedVersion({self.raw!r})"


def parse_version(text: str | None) -> ParsedVersion | None:
    if text is None:
        return None
    trimmed = _js_trim(text)
    match = _SEMVER_RE.match(trimmed)
    if match is None:
        return None
    return ParsedVersion(
        major=_number(match.group(1)),
        minor=_number(match.group(2)),
        patch=_number(match.group(3)),
        prerelease=match.group(4),
        raw=trimmed,
    )


def is_valid_version(text: str | None) -> bool:
    return parse_version(text) is not None


def _compare_prerelease(a: str | None, b: str | None) -> int:
    """Semver §11: numeric identifiers rank below alphanumeric, and more identifiers wins ties."""
    if a == b:
        return 0
    # A version without a prerelease outranks one with it. 1.0.0 is newer than 1.0.0-rc1.
    if a is None:
        return 1
    if b is None:
        return -1

    a_parts = a.split(".")
    b_parts = b.split(".")
    length = max(len(a_parts), len(b_parts))

    for index in range(length):
        ai = a_parts[index] if index < len(a_parts) else None
        bi = b_parts[index] if index < len(b_parts) else None
        if ai is None:
            return -1
        if bi is None:
            return 1
        a_num = bool(_NUMERIC_RE.match(ai))
        b_num = bool(_NUMERIC_RE.match(bi))
        if a_num and b_num:
            diff = _number(ai) - _number(bi)
            if diff != 0:
                return -1 if diff < 0 else 1
            continue
        if a_num:
            return -1
        if b_num:
            return 1
        if ai != bi:
            return -1 if ai < bi else 1
    return 0


def compare_versions(a: str | ParsedVersion | None, b: str | ParsedVersion | None) -> int:
    """-1 if a < b, 0 if equal, 1 if a > b.

    An unparseable version compares equal to everything, which is the reference edition's behaviour
    and deliberate: this function has no way to report an error and a caller sorting a list must not
    have one element throw. The callers that need to know ask :func:`is_valid_version`.
    """
    pa = parse_version(a) if isinstance(a, str) or a is None else a
    pb = parse_version(b) if isinstance(b, str) or b is None else b
    if pa is None or pb is None:
        return 0
    if pa.major != pb.major:
        return -1 if pa.major < pb.major else 1
    if pa.minor != pb.minor:
        return -1 if pa.minor < pb.minor else 1
    if pa.patch != pb.patch:
        return -1 if pa.patch < pb.patch else 1
    return _compare_prerelease(pa.prerelease, pb.prerelease)


class _Comparator:
    __slots__ = ("op", "version")

    def __init__(self, op: str, version: ParsedVersion) -> None:
        self.op = op
        self.version = version


def _parse_comparator(token: str) -> _Comparator | None:
    match = _COMPARATOR_RE.match(_js_trim(token))
    if match is None:
        return None
    version = parse_version(match.group(2))
    if version is None:
        return None
    return _Comparator(op=match.group(1) or "=", version=version)


def _satisfies_comparator(target: ParsedVersion, comparator: _Comparator) -> bool:
    cmp = compare_versions(target, comparator.version)
    op = comparator.op
    if op == ">=":
        return cmp >= 0
    if op == ">":
        return cmp > 0
    if op == "<=":
        return cmp <= 0
    if op == "<":
        return cmp < 0
    if op == "=":
        return cmp == 0
    if op == "^":
        # Caret: compatible within the leftmost non-zero component.
        if cmp < 0:
            return False
        base = comparator.version
        if base.major > 0:
            return target.major == base.major
        if base.minor > 0:
            return target.major == 0 and target.minor == base.minor
        return target.major == 0 and target.minor == 0 and target.patch == base.patch
    if op == "~":
        # Tilde: patch-level changes within the same minor.
        if cmp < 0:
            return False
        return target.major == comparator.version.major and target.minor == comparator.version.minor
    return False


def _parse_range(range_text: str) -> list[list[_Comparator] | None] | None:
    """A range, parsed into its `||` groups.

    A group that does not parse is ``None`` rather than an error for the whole range: one unreadable
    alternative does not invalidate the readable ones. :func:`is_valid_range` is the caller that
    wants the stricter answer and asks for it by checking every group.

    ``None`` for the whole result means the range is a wildcard — every version satisfies it and
    there is nothing to compare against.
    """
    trimmed = _js_trim(range_text)
    if not trimmed or trimmed == "*" or trimmed.lower() == "any":
        return None

    groups: list[list[_Comparator] | None] = []
    for group in trimmed.split("||"):
        # Tokenised on the operator rather than on whitespace, so ">= 1.2.0" and ">=1.2.0" both
        # parse. A range written either way means the same thing to the person who wrote it.
        tokens = _TOKEN_RE.findall(_js_trim(group))
        if not tokens:
            groups.append(None)
            continue
        comparators = [_parse_comparator(token) for token in tokens]
        if any(comparator is None for comparator in comparators):
            groups.append(None)
            continue
        groups.append([comparator for comparator in comparators if comparator is not None])
    return groups


def satisfies(version: str, range_text: str) -> bool:
    """Does ``version`` fall inside ``range_text``?

    Supports ``*``, space-separated AND comparators (``>=1.2.0 <2.0.0``) and ``||`` OR groups.
    """
    target = parse_version(version)
    if target is None:
        return False

    groups = _parse_range(range_text)
    if groups is None:
        return True

    return any(
        group is not None and all(_satisfies_comparator(target, c) for c in group)
        for group in groups
    )


def is_valid_range(range_text: str) -> bool:
    """Is ``range_text`` a range this edition can read at all? See the module header."""
    groups = _parse_range(range_text)
    return groups is None or all(group is not None for group in groups)


# ── Corpus shapes ────────────────────────────────────────────────────────────


def version_to_json(parsed: ParsedVersion | None) -> Any:
    if parsed is None:
        return None
    return {
        "major": parsed.major,
        "minor": parsed.minor,
        "patch": parsed.patch,
        "prerelease": parsed.prerelease,
        "raw": parsed.raw,
    }


def compare_to_json(a: str, b: str) -> Any:
    return {
        "compare": compare_versions(a, b),
        "a": version_to_json(parse_version(a)),
        "b": version_to_json(parse_version(b)),
    }


def satisfies_to_json(version: str, range_text: str) -> Any:
    return {
        "version": version,
        "range": range_text,
        "satisfies": satisfies(version, range_text),
        "valid_version": is_valid_version(version),
        "valid_range": is_valid_range(range_text),
    }
