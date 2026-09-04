"""Declarative predicate evaluation — the Python edition of ``reference/typescript/predicate.ts``.

Compatibility rules are data rather than code, so this module is pure and **total**: it never
raises on a malformed rule, and a rule it cannot decide does not pass.

# The two answers

``evaluate_predicate_outcome`` returns three values — satisfied, unsatisfied, unevaluable — because
§54.2 asks a question a boolean cannot answer:

    A precondition that cannot be evaluated refuses the skill, **naming the path that was
    unresolvable**. Not "assume true", which is what free-text preconditions do by construction.

``evaluate_predicate`` is ``satisfied(...)`` of that outcome and never a second implementation. Two
functions computing the same thing by two routes is the drift the corpus exists to catch, and here
there would be nothing for the refinement to disagree with.

# Three places where Python is not JavaScript, and the specification is JavaScript's

The reference edition is the specification, so where the host languages differ this file follows
the reference and says why.

1. **``bool`` is a subclass of ``int``.** ``True == 1`` is true in Python and ``true === 1`` is
   false in JavaScript. `_strict_equal` therefore compares types before values, or a rule reading
   ``eq(robot.state.enabled, 1)`` would fire here and not in the reference.

2. **``Number(str)`` is not ``float(str)``.** The comparison operators coerce strings, and
   JavaScript's coercion reads ``0x1f`` as 31, ``1e3`` as 1000, an empty or blank string as 0, and
   ``Infinity`` as infinity — which then fails the finiteness check and refuses. ``float()`` raises
   on the first of those and accepts ``nan``, which JavaScript does not. `_js_number` is the
   reference's grammar, and the corpus context carries ``hex``, ``exponent`` and ``infinite``
   fields precisely to hold an edition to it.

3. **Integers are arbitrary-precision.** A JSON number in the reference is an IEEE-754 double
   throughout. Comparison is the only thing this module does with numbers and Python's mixed
   int/float comparison is exact, so the two agree here; the place it bites is `semver`, where it
   already has a paragraph of its own.
"""

from __future__ import annotations

import math
import re
from typing import Any, Final

from .semver import is_valid_range, is_valid_version, satisfies

__all__ = [
    "OUTCOMES",
    "PREDICATE_OPS",
    "UNEVALUABLE_REASONS",
    "PredicateOutcome",
    "collect_predicate_detail",
    "evaluate_predicate",
    "evaluate_predicate_outcome",
    "predicate_to_json",
    "read_path",
    "resolve_value",
    "satisfied",
]

#: §54.2's refusal names something from a closed list; a free-text reason is the same nothing as a
#: free-text precondition. ``corpus/vocabulary/closed-sets.json`` holds every edition to this list.
UNEVALUABLE_REASONS: Final = [
    "malformed",
    "unknown_op",
    "missing_path",
    "missing_operand",
    "unresolved_path",
    "not_a_number",
    "not_a_version",
]

OUTCOMES: Final = ["satisfied", "unsatisfied", "unevaluable"]

PREDICATE_OPS: Final = [
    "and", "or", "not",
    "eq", "neq", "in", "gte", "lte", "gt", "lt",
    "includes", "includes_all", "includes_any", "exists", "semver_satisfies",
]

#: A canonical decimal index, which is what a JSON array member is named.
_INDEX_RE: Final = re.compile(r"^(0|[1-9]\d*)$")

#: ``Number(string)``'s grammar, minus the legacy octal escape hatch the reference cannot produce.
_DECIMAL_RE: Final = re.compile(r"^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$")
_RADIX_RE: Final = re.compile(r"^0([xX][0-9a-fA-F]+|[oO][0-7]+|[bB][01]+)$")

#: The sentinel for "this path named nothing". JSON has no `undefined`, so a real `null` in the
#: context has to remain distinguishable from an absent member — see `read_path`.
_ABSENT: Final = object()


class PredicateOutcome:
    """Why a predicate decided, or could not."""

    __slots__ = ("outcome", "reason", "path", "op")

    def __init__(self, outcome: str, reason: str | None, path: str | None, op: str | None) -> None:
        self.outcome = outcome
        self.reason = reason
        self.path = path
        self.op = op

    def __repr__(self) -> str:  # pragma: no cover — debugging only
        return f"PredicateOutcome({self.outcome!r}, {self.reason!r}, {self.path!r}, {self.op!r})"


def _decided(value: bool) -> PredicateOutcome:
    return PredicateOutcome("satisfied" if value else "unsatisfied", None, None, None)


def _unevaluable(reason: str, path: str | None, op: str | None) -> PredicateOutcome:
    return PredicateOutcome("unevaluable", reason, path, op)


def satisfied(outcome: PredicateOutcome) -> bool:
    """Whether an outcome permits whatever it was guarding. Only ``satisfied`` does."""
    return outcome.outcome == "satisfied"


# ── Reading the context ──────────────────────────────────────────────────────


def read_path(context: Any, path: Any) -> Any:
    """Read ``a.b.c`` out of a context document, or ``_ABSENT`` for any missing link.

    **Members only.** An array segment must be a decimal index, so ``capabilities.length`` does not
    resolve: ``length`` is a property of a JavaScript array rather than a member of the JSON
    document, and an edition without JavaScript arrays could not reproduce it. The same restriction
    keeps a path off the prototype chain in the reference, and off ``dict`` methods here.
    """
    if not isinstance(path, str) or path == "":
        return _ABSENT

    current: Any = context
    for segment in path.split("."):
        if current is None or current is _ABSENT:
            return _ABSENT
        if isinstance(current, list):
            if not _INDEX_RE.match(segment):
                return _ABSENT
            index = int(segment)
            if index >= len(current):
                return _ABSENT
            current = current[index]
            continue
        if not isinstance(current, dict):
            return _ABSENT
        if segment not in current:
            return _ABSENT
        current = current[segment]
    return current


def _referenced_path(value: Any) -> str | None:
    """The path a rule value references, or ``None`` if it is a literal."""
    if not isinstance(value, dict):
        return None
    ref = value.get("__ref")
    if isinstance(ref, str):
        return ref
    # `{ path: … }` is accepted as an alias so hand-written rules read naturally.
    inner = value.get("path")
    if isinstance(inner, str) and len(value) == 1:
        return inner
    return None


def resolve_value(value: Any, context: Any) -> Any:
    """A rule value, or what it references. ``_ABSENT`` when the reference names nothing."""
    ref = _referenced_path(value)
    return value if ref is None else read_path(context, ref)


def _path_of(predicate: dict[str, Any]) -> str | None:
    path = predicate.get("path")
    return path if isinstance(path, str) and path != "" else None


def _children_of(predicate: dict[str, Any]) -> list[Any] | None:
    """The children of a composite, or ``None`` when ``predicates`` is present and is not a list.

    Absent and non-list are deliberately different answers. Absent is *no children*, and a
    conjunction over no children is satisfied — a skill declaring no preconditions does not fail
    them. A ``predicates`` that is a string is a **broken rule**, and answering "no children" there
    would turn it into a pass, which is the failure this module is written to stop.
    """
    children = predicate.get("predicates")
    if children is None:
        return []
    return children if isinstance(children, list) else None


def _to_list(value: Any) -> list[Any]:
    if isinstance(value, list):
        return value
    if value is None or value is _ABSENT:
        return []
    return [value]


def _js_number(value: Any) -> float | None:
    """``Number(value)``, restricted to what the reference then accepts as finite.

    JavaScript's string coercion is not Python's. It reads ``0x1f``, ``0o17`` and ``0b101`` as
    integers, ``1e3`` as 1000, a blank string as 0, and ``Infinity`` as infinity — and rejects the
    things ``float()`` accepts, notably ``nan`` and ``1_000``. The reference then discards anything
    non-finite, which is why ``Infinity`` refuses here rather than comparing as the largest number.
    """
    if isinstance(value, bool):
        # `Number(true)` is 1 in JavaScript, but the reference's `toNumber` accepts only a number
        # or a string, and a boolean is neither. It refuses, and so does this.
        return None
    if isinstance(value, (int, float)):
        return float(value) if math.isfinite(value) else None
    if not isinstance(value, str):
        return None

    text = value.strip()
    if text == "":
        # The reference guards this before coercing: `Number('')` is 0, which would make a blank
        # field compare as zero rather than refuse.
        return None
    if text in ("Infinity", "+Infinity", "-Infinity"):
        return None  # Parsed, then discarded as non-finite. Two steps, one answer.

    sign = 1.0
    body = text
    if _RADIX_RE.match(body):
        base = {"x": 16, "o": 8, "b": 2}[body[1].lower()]
        return float(int(body[2:], base))
    if not _DECIMAL_RE.match(body):
        return None
    parsed = float(body) * sign
    return parsed if math.isfinite(parsed) else None


# ── Operands ─────────────────────────────────────────────────────────────────


class _Operand:
    """One side of a comparison.

    ``path`` is where the value came from, or ``None`` for a literal, and it is load-bearing twice:
    it is what a refusal names, and it is how `_identical` decides object equality without a
    reference to compare.
    """

    __slots__ = ("value", "path", "resolved")

    def __init__(self, value: Any, path: str | None, resolved: bool) -> None:
        self.value = value
        self.path = path
        self.resolved = resolved


def _from_path(context: Any, path: str) -> _Operand:
    value = read_path(context, path)
    return _Operand(value, path, value is not _ABSENT)


def _from_value(value: Any, context: Any) -> _Operand:
    ref = _referenced_path(value)
    if ref is None:
        # A literal is always resolved, even a literal `null`.
        return _Operand(value, None, True)
    return _from_path(context, ref)


def _element(collection: _Operand, item: Any, index: int) -> _Operand:
    """An element of a collection operand, which is a member of the context in its own right."""
    path = None if collection.path is None else f"{collection.path}.{index}"
    return _Operand(item, path, True)


def _first_unresolved(op: str, *operands: _Operand) -> PredicateOutcome | None:
    for operand in operands:
        if not operand.resolved:
            return _unevaluable("unresolved_path", operand.path, op)
    return None


def _strict_equal(left: Any, right: Any) -> bool:
    """``===`` between two primitives, with Python's ``bool``-is-an-``int`` hole closed.

    ``True == 1`` is true in Python and ``true === 1`` is false in JavaScript, so a rule comparing a
    flag to a number would fire in this edition and not in the reference — a divergence that
    produces a *different compatibility answer*, not a different message.
    """
    if isinstance(left, bool) != isinstance(right, bool):
        return False
    return left == right


def _identical(left: _Operand, right: _Operand) -> bool:
    """The reference's equality rule, written here because ``===`` on objects is the host's.

    Two operands are identical when both are primitives and equal, or when both were read from the
    **same path of the same context** — the only way two non-primitive operands can be one object.
    A literal object or array in a rule is therefore never equal to anything, which is what ``===``
    does and is worth knowing before writing ``{"op":"eq","value":["a"]}`` and wondering why it
    never fires. ``includes`` is the operator for that.
    """
    def primitive(value: Any) -> bool:
        return value is None or not isinstance(value, (dict, list))

    if primitive(left.value) and primitive(right.value):
        return _strict_equal(left.value, right.value)
    if left.path is None or right.path is None:
        return False
    return left.path == right.path


def _contains(collection: _Operand, needle: _Operand) -> bool:
    items = _to_list(collection.value)
    return any(_identical(_element(collection, item, index), needle) for index, item in enumerate(items))


# ── The evaluator ────────────────────────────────────────────────────────────


def evaluate_predicate_outcome(predicate: Any, context: Any) -> PredicateOutcome:
    """The evaluator. Everything else in this module is a view of it."""
    if not isinstance(predicate, dict) or not isinstance(predicate.get("op"), str):
        return _unevaluable("malformed", None, None)

    op: str = predicate["op"]

    # Order is part of the answer for the composites: a refusal names *a* predicate, and which one
    # it names must not depend on how an edition iterates.
    if op == "and":
        children = _children_of(predicate)
        if children is None:
            return _unevaluable("malformed", None, op)
        for child in children:
            outcome = evaluate_predicate_outcome(child, context)
            if outcome.outcome != "satisfied":
                return outcome
        # Nothing was asked for. A skill declaring no preconditions is not one that fails them.
        return _decided(True)

    if op == "or":
        children = _children_of(predicate)
        if children is None:
            return _unevaluable("malformed", None, op)
        first_unevaluable: PredicateOutcome | None = None
        for child in children:
            outcome = evaluate_predicate_outcome(child, context)
            if outcome.outcome == "satisfied":
                return outcome
            if outcome.outcome == "unevaluable" and first_unevaluable is None:
                first_unevaluable = outcome
        # An alternative that held would have returned already; an alternative nobody could read
        # means this disjunction is not known to be false, only unread.
        return first_unevaluable if first_unevaluable is not None else _decided(False)

    if op == "not":
        children = _children_of(predicate)
        if children is None:
            return _unevaluable("malformed", None, op)
        child = predicate.get("predicate")
        if child is None:
            child = children[0] if children else None
        if child is None:
            return _unevaluable("missing_operand", None, op)
        outcome = evaluate_predicate_outcome(child, context)
        # Ignorance does not negate into knowledge.
        if outcome.outcome == "unevaluable":
            return outcome
        return _decided(outcome.outcome == "unsatisfied")

    if op == "exists":
        path = _path_of(predicate)
        if path is None:
            return _unevaluable("missing_path", None, op)
        # The one operator that answers *about* resolution, so it never refuses for lack of it.
        value = read_path(context, path)
        if value is None or value is _ABSENT:
            return _decided(False)
        if isinstance(value, list):
            return _decided(len(value) > 0)
        if isinstance(value, str):
            return _decided(value.strip() != "")
        return _decided(True)

    if op in ("eq", "neq"):
        path = _path_of(predicate)
        if path is None:
            return _unevaluable("missing_path", None, op)
        left = _from_path(context, path)
        right = _from_value(predicate.get("value"), context)
        refused = _first_unresolved(op, left, right)
        if refused is not None:
            return refused
        equal = _identical(left, right)
        return _decided(equal if op == "eq" else not equal)

    if op == "in":
        path = _path_of(predicate)
        if path is None:
            return _unevaluable("missing_path", None, op)
        needle = _from_path(context, path)
        collection = _from_value(predicate.get("value"), context)
        refused = _first_unresolved(op, needle, collection)
        if refused is not None:
            return refused
        return _decided(_contains(collection, needle))

    if op == "includes":
        # The context value is a collection that must contain the rule's value.
        path = _path_of(predicate)
        if path is None:
            return _unevaluable("missing_path", None, op)
        collection = _from_path(context, path)
        needle = _from_value(predicate.get("value"), context)
        refused = _first_unresolved(op, collection, needle)
        if refused is not None:
            return refused
        return _decided(_contains(collection, needle))

    if op in ("includes_all", "includes_any"):
        path = _path_of(predicate)
        if path is None:
            return _unevaluable("missing_path", None, op)
        collection = _from_path(context, path)
        wanted = _from_value(predicate.get("value"), context)
        refused = _first_unresolved(op, collection, wanted)
        if refused is not None:
            return refused
        items = _to_list(wanted.value)
        each = [_contains(collection, _element(wanted, item, index)) for index, item in enumerate(items)]
        # An empty requirement is vacuously satisfied — nothing was asked for. Reachable only from a
        # requirement that *is* empty, never from one nobody could read.
        if op == "includes_any":
            return _decided(len(items) == 0 or any(each))
        return _decided(all(each))

    if op in ("gte", "lte", "gt", "lt"):
        path = _path_of(predicate)
        if path is None:
            return _unevaluable("missing_path", None, op)
        left_operand = _from_path(context, path)
        right_operand = _from_value(predicate.get("value"), context)
        refused = _first_unresolved(op, left_operand, right_operand)
        if refused is not None:
            return refused
        left = _js_number(left_operand.value)
        if left is None:
            return _unevaluable("not_a_number", left_operand.path, op)
        right = _js_number(right_operand.value)
        if right is None:
            return _unevaluable("not_a_number", right_operand.path, op)
        if op == "gte":
            return _decided(left >= right)
        if op == "lte":
            return _decided(left <= right)
        if op == "gt":
            return _decided(left > right)
        return _decided(left < right)

    if op == "semver_satisfies":
        path = _path_of(predicate)
        if path is None:
            return _unevaluable("missing_path", None, op)
        version_operand = _from_path(context, path)
        range_operand = _from_value(predicate.get("value"), context)
        refused = _first_unresolved(op, version_operand, range_operand)
        if refused is not None:
            return refused
        version = version_operand.value
        range_text = range_operand.value
        # A version this edition cannot parse is a fact about the declaration, not a version that
        # fell outside the window — so it is named rather than counted as a miss.
        if not isinstance(version, str) or not is_valid_version(version):
            return _unevaluable("not_a_version", version_operand.path, op)
        if not isinstance(range_text, str) or not is_valid_range(range_text):
            return _unevaluable("not_a_version", range_operand.path, op)
        return _decided(satisfies(version, range_text))

    return _unevaluable("unknown_op", _path_of(predicate), op)


def evaluate_predicate(predicate: Any, context: Any) -> bool:
    """The compatibility engine's answer. Derived from the outcome, never reimplemented."""
    return satisfied(evaluate_predicate_outcome(predicate, context))


def collect_predicate_detail(predicate: Any, context: Any) -> dict[str, Any]:
    """Every path the predicate tree mentions, with what it read, so a finding can show *why*."""
    detail: dict[str, Any] = {}

    def walk(node: Any) -> None:
        if not isinstance(node, dict):
            return
        path = _path_of(node)
        if path is not None:
            value = read_path(context, path)
            detail[path] = None if value is _ABSENT else value
        # A `__ref` value is as much part of the comparison as the left-hand path.
        ref = _referenced_path(node.get("value"))
        if ref is not None:
            value = read_path(context, ref)
            detail[ref] = None if value is _ABSENT else value
        if node.get("predicate") is not None:
            walk(node["predicate"])
        for child in _children_of(node) or []:
            walk(child)

    walk(predicate)
    return detail


def predicate_to_json(predicate: Any, context: Any) -> Any:
    """``shapePredicate`` — the boolean and the outcome pinned side by side.

    Both, because the boolean is what the compatibility engine uses and the outcome is what §54.2
    requires, and the corpus freezes them together so a future edition cannot satisfy one and
    quietly disagree on the other.
    """
    outcome = evaluate_predicate_outcome(predicate, context)
    return {
        "satisfied": evaluate_predicate(predicate, context),
        "outcome": outcome.outcome,
        "reason": outcome.reason,
        "path": outcome.path,
        "op": outcome.op,
    }
