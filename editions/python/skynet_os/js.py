"""The handful of JavaScript expressions Python has no equivalent for.

Every module here is a transcription of ``reference/typescript/*``, and most of the transcription is
uneventful. These four are the places where a line that looks identical in both languages **means**
something different, and where a Python-shaped rewrite would be wrong in a way no reviewer would see
by reading either file.

They lived in :mod:`skynet_os.identity` first, moved here when :mod:`skynet_os.checkin` needed the
same two, and are a module rather than a third copy because a divergence helper that exists twice is
a divergence helper that will eventually disagree with itself.

**``String.prototype.trim``** removes a *smaller* set than Python's ``str.strip()``: ECMAScript's
``WhiteSpace`` plus ``LineTerminator``, which includes ``U+FEFF`` and excludes the C1 controls.
``"\x1c".strip()`` is empty to Python and ``"\x1c"`` to the reference, and the difference decides
whether a name is blank.

**``.length``, and indexing** are in UTF-16 code units. Eight characters above ``U+FFFF`` are sixteen
of them, and a refusal that reports a length is reporting that number.

**The default sort** is by UTF-16 code unit, which is *not* code-point order for anything above the
basic plane, and *not* what Python's ``sorted()`` does.

**A number in a template literal** is not ``str(x)``: an integral float renders without its ``.0``,
and the exponent threshold is ``1e21`` rather than Python's ``1e16``.
"""

from __future__ import annotations

import math
from typing import Any, Final

__all__ = ["js_number", "js_trim", "utf16_at", "utf16_key", "utf16_length", "utf16_units"]

#: ECMAScript's ``WhiteSpace`` plus ``LineTerminator``. Python's ``strip()`` removes more.
_JS_WHITESPACE: Final = (
    "\t\n\v\f\r             "
    "     　﻿"
)


def js_trim(value: str) -> str:
    """``String.prototype.trim`` — the specification's set, not Python's larger one."""
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
