// The small string operations the contracts share, and the character tests that are ASCII by
// grammar — a role's charset, a digit in a version.
//
// There is no trim and no case conversion here, and that is the point of this paragraph. There
// used to be both, ASCII-only, on the argument that every identifier the system compares is ASCII
// by grammar, so the ASCII operation was the correct one. It was not: the reference trims with
// `trim()` and lower-cases with `toLowerCase()` **before** it checks the grammar, and both reach
// outside ASCII — a byte-order mark is trimmed away, U+212A KELVIN SIGN lower-cases into the `k` of
// `neck`. The corpus caught both on 2026-09-11. They are `js::trim`, `js::to_lower` and
// `js::to_upper` now, and the ASCII versions were deleted rather than left beside them, so that
// reaching for one is a compile error rather than a divergence.
//
// `std::tolower` and friends are refused for the older reason: they consult the locale, and a
// Turkish one maps `I` to a dotless `ı`.

#ifndef SKYNET_OS_TEXT_HPP
#define SKYNET_OS_TEXT_HPP

#include <string>
#include <string_view>
#include <vector>

namespace skynet::text {

[[nodiscard]] constexpr bool is_ascii_digit(char ch) { return ch >= '0' && ch <= '9'; }
[[nodiscard]] constexpr bool is_ascii_lower(char ch) { return ch >= 'a' && ch <= 'z'; }
[[nodiscard]] constexpr bool is_ascii_upper(char ch) { return ch >= 'A' && ch <= 'Z'; }
[[nodiscard]] constexpr bool is_ascii_alpha(char ch) {
  return is_ascii_lower(ch) || is_ascii_upper(ch);
}

/// Split on a separator, keeping empty fields. Empty fields are kept because they are how a
/// grammar detects `torso..yaw` — a splitter that dropped them would make a malformed role parse.
[[nodiscard]] std::vector<std::string> split(std::string_view value, char separator);

/// Whether `value` starts with `prefix`.
[[nodiscard]] bool starts_with(std::string_view value, std::string_view prefix);

}  // namespace skynet::text

#endif  // SKYNET_OS_TEXT_HPP
