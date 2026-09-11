// The handful of JavaScript expressions C++ has no equivalent for.
//
// Every contract here is a transcription of `reference/typescript/*`, and most of the
// transcription is uneventful. These are the places where a line that looks identical in both
// languages **means** something different, and where a C++-shaped rewrite would be wrong in a way
// no reviewer would see by reading either file. They are a module rather than a copy per caller
// because a divergence helper that exists twice is one that will eventually disagree with itself.
//
// **`String.prototype.trim`** removes a *smaller* set than most languages' trim: ECMAScript's
// `WhiteSpace` plus `LineTerminator`, which includes U+FEFF and excludes the C1 controls. The
// difference decides whether a name is blank.
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

/// What `${value}` renders, for any JSON value. Absence is the six characters `undefined`, not the
/// empty string: a profile with no name reads `The operator profile "undefined" …` in the
/// reference, and an edition that printed `""` there would produce a different message.
[[nodiscard]] std::string interpolate(const Json* value);

/// `String.prototype.trim` — the specification's set, not the host's larger one.
[[nodiscard]] std::string trim(std::string_view value);

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
