"""Canonical JSON, the Python half.

This is a deliberate third implementation of the same rule — ``crates/skynet-json`` is the first and
``reference/typescript/canonical.ts`` the second — and the duplication is the point. §31.4 requires
every edition to reproduce the corpus **byte-identically**, and a corpus whose expected values were
produced by the same code that checks them would prove nothing. The rule is written down once in the
specification and implemented once per edition; the corpus is where the implementations meet.

The rule, restated so this file can be read on its own:

1. No insignificant whitespace.
2. Object keys sorted by the byte sequence of their UTF-8 encoding.
3. Numbers are finite. An integral value below 2**53 is written as an integer, ``-0`` as ``0``;
   anything else as the shortest decimal that round-trips, **always positional, never exponential**.
4. Strings escape ``"``, ``\\`` and the C0 controls and nothing else.
5. Absent fields are omitted; ``null`` is written as ``null``. The two are different facts — the
   whole system distinguishes "nobody declared a safety envelope" from "an envelope was declared and
   is empty" — which is why :data:`OMIT` exists rather than using ``None`` for both.

Two things Python makes easy to get wrong, and how they are handled:

**``bool`` is an ``int``.** ``isinstance(True, int)`` is true, so a naive number branch renders
``True`` as ``1``. Booleans are therefore tested first, everywhere.

**``int`` and ``float`` are different types where JavaScript has one.** ``json.load`` gives ``1`` as
an ``int`` and ``1.0`` as a ``float``, and the reference edition has no such distinction: both are
``number`` and both render ``1``. So integrality decides the rendering here, never the Python type.
An edition that let ``1.0`` render as ``1.0`` would diverge from the corpus on the first velocity
ceiling anybody declared as a whole number.
"""

from __future__ import annotations

from typing import Any, Final

__all__ = ["OMIT", "canonical", "format_number"]

#: 2**53. Above this an integral float can no longer be trusted to round-trip through a decimal
#: integer, so the shortest-round-trip path takes over.
_INT_SAFE: Final = 9007199254740992


class _Omit:
    """The absence of a field, as distinct from ``null``.

    JavaScript spells this ``undefined`` and Python has no equivalent that survives a dictionary,
    so it is a sentinel. A field set to :data:`OMIT` is dropped; a field set to ``None`` is written
    as ``null``. Rule 5 is the whole reason this class exists.
    """

    _instance: "_Omit | None" = None

    def __new__(cls) -> "_Omit":
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __repr__(self) -> str:
        return "OMIT"

    def __bool__(self) -> bool:
        return False


#: Sentinel for a field that is not present at all. See :class:`_Omit`.
OMIT: Final = _Omit()


def _exponential_to_positional(rendered: str) -> str:
    """Turn ``-4.4e-01`` into ``-0.44``.

    Python's ``%e`` formatting always renders as ``<digits>e<sign><exponent>``, which is the only
    shape this handles — the same assumption the reference edition makes about ``toExponential``.
    """
    marker = rendered.find("e")
    if marker < 0:
        return rendered

    mantissa = rendered[:marker]
    exponent = int(rendered[marker + 1:])

    negative = mantissa.startswith("-")
    if negative:
        mantissa = mantissa[1:]

    digits = mantissa.replace(".", "")
    digit_count = len(digits)

    # The first digit carries weight 10**exponent, so the point falls after `exponent + 1` digits.
    point = exponent + 1

    out = "-" if negative else ""
    if point <= 0:
        out += "0." + "0" * (-point) + digits
    elif point >= digit_count:
        out += digits + "0" * (point - digit_count)
    else:
        out += digits[:point] + "." + digits[point:]
    return out


def format_number(value: float | int) -> str:
    """Format a number in canonical form.

    Raises on a non-finite value rather than emitting ``null``: a NaN that serialised to ``null``
    would turn "this was not measured" into "this was measured as nothing", which is the distinction
    §74.3 spends a whole section on.

    The shortest-round-trip search mirrors the reference edition's exactly — ascending precision
    until the rendered value parses back equal — rather than leaning on ``repr``. Python's ``repr``
    happens to produce the shortest round-trip too, but "happens to agree today" is not what a
    conformance corpus is held together by.
    """
    if isinstance(value, bool):
        raise TypeError("a boolean is not a number here; canonical() handles it separately")

    numeric = float(value)
    if numeric != numeric or numeric in (float("inf"), float("-inf")):
        raise ValueError(f"cannot serialise a non-finite number: {value}")

    if numeric == 0:
        return "0"  # covers -0.0

    if numeric.is_integer() and abs(numeric) < _INT_SAFE:
        return str(int(numeric))

    for precision in range(1, 18):
        rendered = f"{numeric:.{precision - 1}e}"
        if float(rendered) == numeric:
            return _exponential_to_positional(rendered)
    return _exponential_to_positional(f"{numeric:.16e}")


_ESCAPES: Final = {
    '"': '\\"',
    "\\": "\\\\",
    "\b": "\\b",
    "\f": "\\f",
    "\n": "\\n",
    "\r": "\\r",
    "\t": "\\t",
}


def _write_string(value: str, out: list[str]) -> None:
    out.append('"')
    for ch in value:
        escape = _ESCAPES.get(ch)
        if escape is not None:
            out.append(escape)
        elif ord(ch) < 0x20:
            out.append(f"\\u{ord(ch):04x}")
        else:
            out.append(ch)
    out.append('"')


def _write_value(value: Any, out: list[str]) -> None:
    if value is None:
        out.append("null")
        return
    # Before the number branch, always: bool is a subclass of int in Python, and `True` rendered as
    # `1` is a divergence no test of the happy path would catch.
    if isinstance(value, bool):
        out.append("true" if value else "false")
        return
    if isinstance(value, (int, float)):
        out.append(format_number(value))
        return
    if isinstance(value, str):
        _write_string(value, out)
        return
    if isinstance(value, _Omit):
        # Only reachable as a list element; a mapping's omitted fields are dropped below. The
        # reference edition writes `null` here for the same reason: a hole in an array is a
        # position, and dropping it would renumber every element after it.
        out.append("null")
        return
    if isinstance(value, (list, tuple)):
        out.append("[")
        for index, item in enumerate(value):
            if index > 0:
                out.append(",")
            _write_value(item, out)
        out.append("]")
        return
    if isinstance(value, dict):
        keys = [key for key, held in value.items() if not isinstance(held, _Omit)]
        # Sorted by UTF-8 bytes, not by code point. They differ above the BMP, and a key ordering
        # that agreed for ASCII and diverged for a vendor's non-Latin channel name would be found
        # by a customer rather than by this corpus.
        keys.sort(key=lambda key: key.encode("utf-8"))
        out.append("{")
        for index, key in enumerate(keys):
            if index > 0:
                out.append(",")
            _write_string(key, out)
            out.append(":")
            _write_value(value[key], out)
        out.append("}")
        return

    raise TypeError(f"cannot serialise {type(value).__name__} in canonical form")


def canonical(value: Any) -> str:
    """Serialise a value in canonical form."""
    out: list[str] = []
    _write_value(value, out)
    return "".join(out)
