// The small string operations the contracts share.
//
// ASCII by name, everywhere, and that is a decision rather than an oversight. `std::tolower` is
// locale-dependent: under a Turkish locale it maps `I` to a dotless `ı`, so a role or a vocabulary
// word would classify differently on one operator's workstation than on another's. Every
// identifier this system compares — roles, closed-set words, semantic versions, package names —
// is ASCII by grammar, so the ASCII operation is both the correct one and the one that cannot vary
// by environment.

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

/// Lowercase the ASCII letters and leave every other byte alone — including every byte of a UTF-8
/// sequence, all of which are >= 0x80 and none of which this touches.
[[nodiscard]] std::string ascii_lower(std::string_view value);

/// Trim the ASCII whitespace Python's `str.strip()` trims: space, tab, newline, carriage return,
/// form feed and vertical tab.
[[nodiscard]] std::string strip(std::string_view value);

/// Split on a separator, keeping empty fields. Empty fields are kept because they are how a
/// grammar detects `torso..yaw` — a splitter that dropped them would make a malformed role parse.
[[nodiscard]] std::vector<std::string> split(std::string_view value, char separator);

/// Whether `value` starts with `prefix`.
[[nodiscard]] bool starts_with(std::string_view value, std::string_view prefix);

}  // namespace skynet::text

#endif  // SKYNET_OS_TEXT_HPP
