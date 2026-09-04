"""Version digests — the Python edition of ``reference/typescript/digest.ts``.

    sha256(
      "skynet.version.v1" \\n
      kind                \\n
      version             \\n
      canonical(manifest) \\n
      "<path>\\0<sha256>" \\n … per file, sorted by path
    )

The separator between a path and its hash is a **NUL** rather than a space: it is the one byte a
path may not contain, which `check_path` enforces, and that is what makes the framing unambiguous
rather than merely unlikely to collide. The reference writes it as an escape for a reason worth
repeating — the Hub's copy holds the raw byte, a terminal renders it as nothing, and that is how the
TypeScript implementation came to be written against a space and found out from a digest that did
not match.

# The two rules that exist so a robot and the Hub agree or neither computes an identity

A manifest carrying a number at or above 1e21 or below 1e-6 is **refused**, because that is where
`JSON.stringify` reaches for an exponent and §31.4's canonical rule forbids one. So is a manifest
whose object *keys* carry a character above U+FFFF, because the Hub orders keys by UTF-16 code unit
and the canonical rule orders them by UTF-8 byte; those two orders agree throughout the basic
multilingual plane and can disagree above it.

Refusing beats guessing, for the same reason a missing envelope refuses an install: a package that
does not install and a person reading a reason is a better outcome than a package whose identity is
one thing on the robot and another in the registry.

# Where Python is not JavaScript

**Sorting.** `sorted(paths)` in Python is by code point. The reference sorts with `<`, which is by
**UTF-16 code unit**, and the two disagree for every character above U+FFFF: as a surrogate pair
such a character begins at 0xD800 and therefore sorts *before* U+E000…U+FFFF, while its code point
sorts after. A file list containing an emoji and a private-use character would hash in a different
order in a naive edition — producing a different, silently wrong digest rather than an error.
`_utf16_key` is the reference's order.

**`bool` is a subclass of `int`.** A manifest value of `True` is a boolean in JSON and would be
scanned as a number here, so the range check tests for it first. `False` has magnitude 0 and passes
either way, which is exactly the kind of near-miss that makes this hard to notice.
"""

from __future__ import annotations

import hashlib
import math
from typing import Any, Final

from .canonical import canonical

__all__ = [
    "VERSION_DIGEST_DOMAIN",
    "check_path",
    "digest_to_json",
    "is_hex_sha256",
    "utf16_compare",
    "version_digest",
]

#: Present so a digest over a version cannot collide with a digest over anything else.
VERSION_DIGEST_DOMAIN: Final = "skynet.version.v1"

_JS_MIN_PLAIN: Final = 1e-6
_JS_MAX_PLAIN: Final = 1e21

_HEX: Final = frozenset("0123456789abcdef")

#: The separator between a path and its hash, written as a call rather than as a literal.
#: A raw NUL renders as nothing in an editor and in a terminal, which is how the reference
#: edition came to be written against a space and found out from a digest that did not match.
_NUL: Final = chr(0)


def is_hex_sha256(value: Any) -> bool:
    """Whether a string is a lowercase hex sha256."""
    return isinstance(value, str) and len(value) == 64 and all(ch in _HEX for ch in value)


def check_path(path: str) -> str | None:
    """Whether a relative path may be used as one, and if not, in what way.

    The file list is both what is hashed and what is written to disk, and every rule here closes a
    way for the second to leave the package's own directory or to name two things on two
    filesystems. Checked before hashing rather than at unpack time, because a digest computed over a
    traversal is a digest that attests to one.
    """
    if len(path) == 0:
        return "empty"
    if len(path.encode("utf-8")) > 1024:
        return "longer than 1024 bytes"
    if path.startswith("/"):
        return "absolute; paths are relative to the package root"
    if "\\" in path:
        return (
            "contains a backslash, which is a separator on one filesystem and an ordinary filename "
            "character on another"
        )
    for ch in path:
        code = ord(ch)
        if code < 0x20 or code == 0x7F:
            return "contains a control character"
    for component in path.split("/"):
        if component == "":
            return "has an empty component, so two spellings name one file"
        if component == ".":
            return "has a `.` component"
        if component == "..":
            return "has a `..` component, which leaves the package root"
        if component.endswith(" ") or component.endswith("."):
            return "has a component ending in a space or a dot, which some filesystems silently strip"
    return None


def _utf16_key(value: str) -> tuple[int, ...]:
    """A sort key ordering by UTF-16 code unit, which is what JavaScript's ``<`` does."""
    encoded = value.encode("utf-16-be")
    return tuple(int.from_bytes(encoded[i:i + 2], "big") for i in range(0, len(encoded), 2))


def utf16_compare(a: str, b: str) -> int:
    left, right = _utf16_key(a), _utf16_key(b)
    if left < right:
        return -1
    return 1 if left > right else 0


def _scan(value: Any, at: str) -> dict[str, Any] | None:
    """The first value in a manifest whose serialisation the two canonicalisers could disagree on."""
    # Before the number branch: a JSON boolean is not a number, and in Python it is an `int`.
    if isinstance(value, bool):
        return None

    if isinstance(value, (int, float)):
        magnitude = abs(float(value))
        diverges = (
            not math.isfinite(magnitude)
            or magnitude >= _JS_MAX_PLAIN
            or (magnitude != 0 and magnitude < _JS_MIN_PLAIN)
        )
        if diverges:
            return {"kind": "number_out_of_range", "field": at if at != "" else "(root)", "value": value}
        return None

    if isinstance(value, list):
        for index, item in enumerate(value):
            found = _scan(item, f"{at}[{index}]")
            if found is not None:
                return found
        return None

    if isinstance(value, dict):
        for key, item in value.items():
            for ch in key:
                if ord(ch) > 0xFFFF:
                    return {"kind": "key_outside_bmp", "at": at if at != "" else "(root)", "key": key}
            found = _scan(item, key if at == "" else f"{at}.{key}")
            if found is not None:
                return found
    return None


def version_digest(kind: str, version: str, manifest: Any, files: list[dict[str, Any]]) -> dict[str, Any]:
    """Compute the digest of a version, or say precisely why it cannot be computed.

    Returns ``{"ok": True, "digest": …}`` or ``{"ok": False, "error": {…}}``; every error names the
    input rather than saying "failed".
    """
    bad = _scan(manifest if manifest is not None else {}, "")
    if bad is not None:
        return {"ok": False, "error": bad}

    for file in files:
        path = file.get("path", "")
        detail = check_path(path)
        if detail is not None:
            return {"ok": False, "error": {"kind": "path", "path": path, "detail": detail}}
        if not is_hex_sha256(file.get("sha256")):
            return {"ok": False, "error": {"kind": "sha256", "path": path, "value": file.get("sha256")}}

    ordered = sorted(files, key=lambda file: _utf16_key(file["path"]))
    for index in range(1, len(ordered)):
        if ordered[index]["path"] == ordered[index - 1]["path"]:
            return {"ok": False, "error": {"kind": "duplicate_path", "path": ordered[index]["path"]}}

    try:
        manifest_text = canonical(manifest if manifest is not None else {})
    except Exception as error:  # noqa: BLE001 — the reference reports the message, and so does this
        return {"ok": False, "error": {"kind": "manifest", "detail": str(error)}}

    body = "\n".join([
        VERSION_DIGEST_DOMAIN,
        kind,
        version,
        manifest_text,
        # `_NUL`, not a literal — see its declaration.
        "\n".join(_NUL.join([file["path"], file["sha256"]]) for file in ordered),
    ])

    return {"ok": True, "digest": hashlib.sha256(body.encode("utf-8")).hexdigest()}


def digest_to_json(kind: str, version: str, manifest: Any, files: list[dict[str, Any]]) -> Any:
    """``shapeDigest`` — the digest, or the error with whatever it is *about*.

    The subject is a path for three of the six error kinds, a field for one, a location and a key
    for another, and nothing for the sixth. Flattening them into one field is what lets a reader
    see which input to go and fix without knowing the error taxonomy.
    """
    outcome = version_digest(kind, version, manifest, files)
    if outcome["ok"]:
        return {"digest": outcome["digest"], "error": None}

    error = outcome["error"]
    kind_name = error["kind"]

    if kind_name in ("path", "duplicate_path", "sha256"):
        subject: Any = error["path"]
    elif kind_name == "number_out_of_range":
        subject = error["field"]
    elif kind_name == "key_outside_bmp":
        subject = f"{error['at']} :: {error['key']}"
    else:
        subject = None

    detail = error["detail"] if kind_name in ("path", "manifest") else None

    return {"digest": None, "error": {"kind": kind_name, "subject": subject, "detail": detail}}
