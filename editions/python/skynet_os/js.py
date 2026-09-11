"""The handful of JavaScript expressions Python has no equivalent for.

Every module here is a transcription of ``reference/typescript/*``, and most of the transcription is
uneventful. These four are the places where a line that looks identical in both languages **means**
something different, and where a Python-shaped rewrite would be wrong in a way no reviewer would see
by reading either file.

They lived in :mod:`skynet_os.identity` first, moved here when :mod:`skynet_os.checkin` needed the
same two, and are a module rather than a third copy because a divergence helper that exists twice is
a divergence helper that will eventually disagree with itself.

**``String.prototype.trim``** removes a different set from Python's ``str.strip()``: ECMAScript's
``WhiteSpace`` plus ``LineTerminator``, which includes ``U+FEFF`` and excludes ``U+0085`` and the
separators ``U+001C``–``U+001F``. ``"\x1c".strip()`` is empty to Python and ``"\x1c"`` to the
reference, and the difference decides whether a name is blank. The same set is JavaScript's
``\\s``, which a Python ``str`` pattern's ``\\s`` is not — :data:`JS_SPACE` is the class to use.

**``\\d``** in a JavaScript pattern is ``[0-9]``. In a Python ``str`` pattern it is every Unicode
decimal digit, and ``int()`` and ``float()`` read them all, so a version written in Arabic-Indic
digits is a version to Python and nothing to the reference. Patterns in this package spell
``[0-9]``.

Until 2026-09-11 this module held the trim rule and eight other modules did not use it: they called
``.strip()``, and their patterns used ``\\s`` and ``\\d``. The corpus had no input that could tell. It
has now, and ``self_check.py`` scans the package for both spellings.

**``.length``, and indexing** are in UTF-16 code units. Eight characters above ``U+FFFF`` are sixteen
of them, and a refusal that reports a length is reporting that number.

**The default sort** is by UTF-16 code unit, which is *not* code-point order for anything above the
basic plane, and *not* what Python's ``sorted()`` does.

**A number in a template literal** is not ``str(x)``: an integral float renders without its ``.0``,
and the exponent threshold is ``1e21`` rather than Python's ``1e16``.

**``toFixed``** rounds ties **away from zero** where Python's ``format`` rounds them to **even**, so
``(62.5).toFixed(0)`` is ``63`` and ``f"{62.5:.0f}"`` is ``62``.
"""

from __future__ import annotations

import math
from decimal import ROUND_HALF_UP, Decimal
from typing import Any, Final

__all__ = [
    "JS_SPACE",
    "js_number",
    "js_trim",
    "to_fixed",
    "utf16_at",
    "utf16_key",
    "utf16_length",
    "utf16_units",
]

#: ECMAScript's ``WhiteSpace`` plus ``LineTerminator``, by code point. Written as numbers because the
#: string it used to be was twenty-five characters, most of them invisible, and a set nobody can
#: read is a set nobody can check.
_JS_WHITESPACE_CODE_POINTS: Final = (
    0x0009, 0x000A, 0x000B, 0x000C, 0x000D, 0x0020, 0x00A0, 0x1680,
    *range(0x2000, 0x200B),
    0x2028, 0x2029, 0x202F, 0x205F, 0x3000, 0xFEFF,
)
_JS_WHITESPACE: Final = "".join(map(chr, _JS_WHITESPACE_CODE_POINTS))

#: One character of JavaScript's \s, as a pattern: write this where a JavaScript pattern says \s.
#: None of the set is special inside a character class.
JS_SPACE: Final = "[" + _JS_WHITESPACE + "]"


def js_trim(value: str) -> str:
    """``String.prototype.trim`` — the specification's set, not Python's."""
    return value.strip(_JS_WHITESPACE)


def utf16_units(value: str) -> list[int]:
    """The string as UTF-16 code units, which is what JavaScript indexes and measures."""
    encoded = value.encode("utf-16-be", errors="surrogatepass")
    return [(encoded[i] << 8) | encoded[i + 1] for i in range(0, len(encoded), 2)]


def utf16_length(value: str) -> int:
    """``value.length``. Eight emoji are sixteen of these."""
    return len(utf16_units(value))


def utf16_at(value: str, index: int) -> str:
    """``value[index]`` — one code unit, which for an astral character is half of one."""
    unit = utf16_units(value)[index]
    return chr(unit) if not 0xD800 <= unit <= 0xDFFF else "\ufffd"


def utf16_key(value: str) -> tuple[int, ...]:
    """A sort key ordering by UTF-16 code unit, which is what JavaScript's ``<`` and sort do."""
    return tuple(utf16_units(value))


def js_number(value: Any) -> str:
    """A number as a template literal renders it."""
    if isinstance(value, bool):
        return "true" if value else "false"
    if value is None:
        return "null"
    if isinstance(value, float):
        if value != value:
            return "NaN"
        if value in (math.inf, -math.inf):
            return "Infinity" if value > 0 else "-Infinity"
        if value.is_integer() and abs(value) < 1e21:
            return str(int(value))
    return str(value)


def to_fixed(value: Any, digits: int) -> str:
    """``Number.prototype.toFixed(digits)``.

    ECMAScript takes the sign off first and then picks the integer ``n`` for which ``n / 10**digits``
    is nearest ``x``, **the larger on a tie** — so ties are away from zero once the sign is restored,
    where Python's ``format`` rounds them to even.

    Two details make the obvious shortcuts wrong, and both have a caller in this edition:

    * It decides on the **exact binary value** of the double, which is why this goes through
      ``Decimal(value)`` rather than a decimal string: ``Decimal(0.1)`` is the number the machine
      holds and ``Decimal("0.1")`` is not.
    * ``floor(x + 0.5)`` is not the same rule. At ``0.49999999999999994`` the sum rounds up to
      exactly ``1.0`` in binary and the shortcut answers ``1``, where the nearest integer to the
      value itself is ``0``. That is the same input on which ``roundHalfAway`` is defined twice and
      two ways in ``reference/typescript``, and it is why there is one of these rather than two.
    """
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return "NaN"
    number = float(value)
    if number != number:
        return "NaN"
    if math.isinf(number):
        return "Infinity" if number > 0 else "-Infinity"
    sign = "-" if number < 0 else ""
    exact = Decimal(abs(number))
    quantum = Decimal(1).scaleb(-digits)
    return sign + str(exact.quantize(quantum, rounding=ROUND_HALF_UP))
