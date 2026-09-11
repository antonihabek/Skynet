// The handful of JavaScript expressions C++ has no equivalent for.
//
// Every contract here is a transcription of `reference/typescript/*`, and most of the
// transcription is uneventful. These are the places where a line that looks identical in both
// languages **means** something different, and where a C++-shaped rewrite would be wrong in a way
// no reviewer would see by reading either file. They are a module rather than a copy per caller
// because a divergence helper that exists twice is one that will eventually disagree with itself.
//
// **`String.prototype.trim`** removes a different set from every other language's trim: ECMAScript's
// `WhiteSpace` plus `LineTerminator`, which includes U+FEFF and excludes U+0085. The difference
// decides whether a name is blank. The same set is JavaScript's `\s`, so a pattern transcribed from
// the reference steps over `space_at`, never over ASCII whitespace.
//
// **`toUpperCase` and `toLowerCase`** are full Unicode mappings: `ß` is `SS`, and U+212A KELVIN SIGN
// lower-cases to the ASCII `k`. Nothing in the C++ standard library does either — `std::toupper` is
// one byte, one locale, one-to-one — so the tables are generated from the engine the reference runs
// on (`tools/unicode-case.ts`) and applied here, with `toLowerCase`'s one context rule.
//
// **`.length`, and indexing** are in UTF-16 code units. Eight characters above U+FFFF are sixteen
// of them, and a refusal that reports a length is reporting that number.
//
// **The default sort** is by UTF-16 code unit, which is *not* code-point order for anything above
// the basic plane, and *not* what a byte-wise `std::string` comparison does.
//
// **A number in a template literal** is `Number::toString`, which is positional between 1e-7 and
// 1e21 and exponential outside it. Canonical JSON is positional everywhere (it spells 1e21 out in
// full), so `format_number` is the wrong tool for a message string and this is the right one.
//
// **`toFixed`** rounds ties **away from zero**, and decides on the *exact binary value* of the
// double rather than on a decimal rendering of it.

#ifndef SKYNET_OS_JS_HPP
#define SKYNET_OS_JS_HPP

#include "skynet_os/json.hpp"

#include <cstdint>
#include <string>
#include <string_view>
#include <vector>

namespace skynet::js {

/// `String(n)` — ECMAScript's `Number::toString`, for a template literal.
[[nodiscard]] std::string number_to_string(double value);

/// `Math.round`: the nearest integer, ties toward +∞ — so -2.5 is -2, unlike a half-away rounding.
/// Decided on the value itself: `floor(x + 0.5)` answers 1 at 0.49999999999999994, because the sum
/// rounds up to exactly 1.0 in binary, where the nearest integer is 0.
[[nodiscard]] double math_round(double value);

/// JavaScript truthiness, for an `if (x)` or an `&&` in the reference: `0`, `''`, `null`,
/// `undefined`, `NaN` and `false` are falsy, and an empty array or object **is not**. Distinct from
/// `Json::is_true`, which is `x === true` and is the test most fields want.
[[nodiscard]] bool truthy(const Json& value);

/// `object.field ?? fallback`, for a field the reference types as a bare `number`.
///
/// Absent takes the fallback. A `null` is NaN — not absent — because the reference's type admits no
/// null, so the only way one reaches a case's input is `JSON.stringify` writing a NaN
/// (corpus/README.md), and `NaN ?? fallback` is NaN. Anything else that is not a number is NaN too.
[[nodiscard]] double bare_number(const Json& object, std::string_view field, double fallback);

/// What `${value}` renders, for any JSON value. Absence is the six characters `undefined`, not the
/// empty string: a profile with no name reads `The operator profile "undefined" …` in the
/// reference, and an edition that printed `""` there would produce a different message.
[[nodiscard]] std::string interpolate(const Json* value);

/// `String.prototype.trim` — the specification's set, not the host's.
[[nodiscard]] std::string trim(std::string_view value);

/// The width in bytes of one character of JavaScript's `\s` at `at`, or 0 when there is none there.
[[nodiscard]] std::size_t space_at(std::string_view value, std::size_t at);

/// The code points of a UTF-8 string. A sequence cut short at the end is one U+FFFD, which is what
/// a decoder produces and what a length therefore counts.
[[nodiscard]] std::u32string code_points(std::string_view value);
/// Append one code point to `out` as UTF-8.
void append_utf8(std::string& out, char32_t code_point);

/// `toUpperCase` of one code point: one, two or three of them, because the mapping is the full one.
[[nodiscard]] std::u32string to_upper(char32_t code_point);
/// `String.prototype.toUpperCase`.
[[nodiscard]] std::string to_upper(std::string_view value);
[[nodiscard]] std::u32string to_upper(const std::u32string& value);
/// `String.prototype.toLowerCase`, final sigma included.
[[nodiscard]] std::string to_lower(std::string_view value);
[[nodiscard]] std::u32string to_lower(const std::u32string& value);
/// The Unicode version the case tables were generated under.
[[nodiscard]] std::string_view unicode_version();

/// The string as UTF-16 code units, which is what JavaScript indexes and measures.
[[nodiscard]] std::vector<std::uint16_t> utf16_units(std::string_view value);
/// `value.length`. Eight emoji are sixteen of these.
[[nodiscard]] std::size_t utf16_length(std::string_view value);

/// Whether `left` sorts before `right` by UTF-16 code unit, which is what JavaScript's `<` and its
/// default sort do — not code-point order, for anything above the basic plane.
[[nodiscard]] bool utf16_less(std::string_view left, std::string_view right);

/// `Number.prototype.toFixed(digits)`.
[[nodiscard]] std::string to_fixed(double value, int digits);

}  // namespace skynet::js

#endif  // SKYNET_OS_JS_HPP
