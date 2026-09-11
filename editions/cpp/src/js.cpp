#include "skynet_os/js.hpp"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <limits>
#include <string_view>

namespace skynet::js {
namespace {

/// One code point's full case mapping. Unused slots are 0, which no mapping produces.
struct Mapping {
  char32_t from;
  char32_t to[3];
};

/// An inclusive run of code points holding a property.
struct Range {
  char32_t first;
  char32_t last;
};

// `kUpper`, `kLower`, `kCased`, `kCaseIgnorable` and `kUnicodeVersion`, generated from the engine
// the reference runs on by `tools/unicode-case.ts`. Sorted by code point by construction.
#include "unicode_case.inc"

constexpr char32_t kCapitalSigma = 0x03A3;
constexpr char32_t kFinalSigma = 0x03C2;

template <std::size_t N>
const Mapping* find_mapping(const Mapping (&table)[N], char32_t code_point) {
  const Mapping* end = table + N;
  const Mapping* found = std::lower_bound(
      table, end, code_point, [](const Mapping& one, char32_t wanted) { return one.from < wanted; });
  return found != end && found->from == code_point ? found : nullptr;
}

template <std::size_t N>
bool has_property(const Range (&table)[N], char32_t code_point) {
  const Range* end = table + N;
  const Range* after = std::upper_bound(
      table, end, code_point, [](char32_t wanted, const Range& one) { return wanted < one.first; });
  return after != table && (after - 1)->last >= code_point;
}

void append_mapped(std::u32string& out, const Mapping* mapping, char32_t code_point) {
  if (mapping == nullptr) {
    out.push_back(code_point);
    return;
  }
  for (const char32_t to : mapping->to) {
    if (to == 0) {
      break;
    }
    out.push_back(to);
  }
}

/// Unicode's Final_Sigma: a cased letter before, skipping case-ignorable characters, and none after.
///
/// A character that is both cased and case-ignorable — a modifier letter — is **stepped over** in
/// both directions rather than counted as the letter, which is what the engine does. Reading the
/// definition's regular expression literally gives the other answer; `tests/case_vectors.inc` holds
/// strings from all four classes so the two cannot be confused without the self-check saying so.
bool is_final_sigma(const std::u32string& value, std::size_t at) {
  bool preceded = false;
  for (std::size_t before = at; before > 0;) {
    --before;
    if (has_property(kCaseIgnorable, value[before])) {
      continue;
    }
    preceded = has_property(kCased, value[before]);
    break;
  }
  if (!preceded) {
    return false;
  }
  for (std::size_t after = at + 1; after < value.size(); ++after) {
    if (has_property(kCaseIgnorable, value[after])) {
      continue;
    }
    return !has_property(kCased, value[after]);
  }
  return true;
}

std::string encode(const std::u32string& value) {
  std::string out;
  for (const char32_t code_point : value) {
    append_utf8(out, code_point);
  }
  return out;
}

/// The shortest decimal digits that round-trip, and the exponent `E` with
/// `value = 0.d1d2… × 10^n`, in ECMAScript's `n`/`k`/`s` terms.
struct Digits {
  std::string s;  //< the significant digits, trailing zeros stripped
  int n = 0;      //< the decimal point's position: value = 0.s × 10^n
};

Digits shortest_digits(double value) {
  char buffer[64];
  int precision = 1;
  for (; precision <= 17; ++precision) {
    std::snprintf(buffer, sizeof(buffer), "%.*e", precision - 1, value);
    if (std::strtod(buffer, nullptr) == value) {
      break;
    }
  }
  if (precision > 17) {
    std::snprintf(buffer, sizeof(buffer), "%.17e", value);
  }

  const std::string rendered(buffer);
  const std::size_t marker = rendered.find('e');
  std::string mantissa = rendered.substr(0, marker);
  const int exponent = std::atoi(rendered.c_str() + marker + 1);
  if (!mantissa.empty() && mantissa.front() == '-') {
    mantissa.erase(mantissa.begin());
  }

  Digits out;
  for (const char ch : mantissa) {
    if (ch != '.') {
      out.s.push_back(ch);
    }
  }
  while (out.s.size() > 1 && out.s.back() == '0') {
    out.s.pop_back();
  }
  out.n = exponent + 1;
  return out;
}

/// ECMAScript's WhiteSpace plus LineTerminator, as UTF-8 byte sequences. Notably it includes
/// U+FEFF and excludes U+0085, which is where it differs from most hosts' trim.
const std::vector<std::string>& js_whitespace() {
  static const std::vector<std::string> set = {
      "\t",          "\n",          "\v",          "\f",          "\r",
      " ",           "\xc2\xa0",    "\xe1\x9a\x80", "\xe2\x80\x80", "\xe2\x80\x81",
      "\xe2\x80\x82", "\xe2\x80\x83", "\xe2\x80\x84", "\xe2\x80\x85", "\xe2\x80\x86",
      "\xe2\x80\x87", "\xe2\x80\x88", "\xe2\x80\x89", "\xe2\x80\x8a", "\xe2\x80\xa8",
      "\xe2\x80\xa9", "\xe2\x80\xaf", "\xe2\x81\x9f", "\xe3\x80\x80", "\xef\xbb\xbf",
  };
  return set;
}

}  // namespace

std::size_t space_at(std::string_view value, std::size_t at) {
  if (at >= value.size()) {
    return 0;
  }
  for (const std::string& one : js_whitespace()) {
    if (value.compare(at, one.size(), one) == 0) {
      return one.size();
    }
  }
  return 0;
}

std::u32string code_points(std::string_view value) {
  std::u32string out;
  std::size_t at = 0;
  while (at < value.size()) {
    const auto lead = static_cast<unsigned char>(value[at]);
    char32_t code_point = lead;
    std::size_t width = 1;
    if (lead >= 0xF0) {
      width = 4;
      code_point = lead & 0x07u;
    } else if (lead >= 0xE0) {
      width = 3;
      code_point = lead & 0x0Fu;
    } else if (lead >= 0xC0) {
      width = 2;
      code_point = lead & 0x1Fu;
    }
    if (at + width > value.size()) {
      out.push_back(0xFFFD);
      break;
    }
    for (std::size_t step = 1; step < width; ++step) {
      code_point = (code_point << 6) | (static_cast<unsigned char>(value[at + step]) & 0x3Fu);
    }
    at += width;
    out.push_back(code_point);
  }
  return out;
}

void append_utf8(std::string& out, char32_t code_point) {
  if (code_point < 0x80) {
    out.push_back(static_cast<char>(code_point));
  } else if (code_point < 0x800) {
    out.push_back(static_cast<char>(0xC0 | (code_point >> 6)));
    out.push_back(static_cast<char>(0x80 | (code_point & 0x3F)));
  } else if (code_point < 0x10000) {
    out.push_back(static_cast<char>(0xE0 | (code_point >> 12)));
    out.push_back(static_cast<char>(0x80 | ((code_point >> 6) & 0x3F)));
    out.push_back(static_cast<char>(0x80 | (code_point & 0x3F)));
  } else {
    out.push_back(static_cast<char>(0xF0 | (code_point >> 18)));
    out.push_back(static_cast<char>(0x80 | ((code_point >> 12) & 0x3F)));
    out.push_back(static_cast<char>(0x80 | ((code_point >> 6) & 0x3F)));
    out.push_back(static_cast<char>(0x80 | (code_point & 0x3F)));
  }
}

std::u32string to_upper(char32_t code_point) {
  std::u32string out;
  append_mapped(out, find_mapping(kUpper, code_point), code_point);
  return out;
}

std::u32string to_upper(const std::u32string& value) {
  // Upper-casing has no context rule, so a string is the concatenation of its code points' mappings.
  std::u32string out;
  for (const char32_t code_point : value) {
    append_mapped(out, find_mapping(kUpper, code_point), code_point);
  }
  return out;
}

std::string to_upper(std::string_view value) { return encode(to_upper(code_points(value))); }

std::u32string to_lower(const std::u32string& value) {
  std::u32string out;
  for (std::size_t at = 0; at < value.size(); ++at) {
    if (value[at] == kCapitalSigma && is_final_sigma(value, at)) {
      out.push_back(kFinalSigma);
      continue;
    }
    append_mapped(out, find_mapping(kLower, value[at]), value[at]);
  }
  return out;
}

std::string to_lower(std::string_view value) { return encode(to_lower(code_points(value))); }

std::string_view unicode_version() { return kUnicodeVersion; }

std::string number_to_string(double value) {
  if (std::isnan(value)) {
    return "NaN";
  }
  if (std::isinf(value)) {
    return value > 0 ? "Infinity" : "-Infinity";
  }
  if (value == 0) {
    return "0";  // covers -0, which ECMAScript renders as "0"
  }

  const std::string minus = value < 0 ? "-" : "";
  const Digits digits = shortest_digits(value);
  const std::string& s = digits.s;
  const int k = static_cast<int>(s.size());
  const int n = digits.n;

  if (k <= n && n <= 21) {
    return minus + s + std::string(static_cast<std::size_t>(n - k), '0');
  }
  if (0 < n && n <= 21) {
    return minus + s.substr(0, static_cast<std::size_t>(n)) + "." +
           s.substr(static_cast<std::size_t>(n));
  }
  if (-6 < n && n <= 0) {
    return minus + "0." + std::string(static_cast<std::size_t>(-n), '0') + s;
  }
  // Exponential. The exponent the specification prints is `n - 1`, and a positive one carries a
  // `+` while a negative one carries only its own sign.
  const int power = n - 1;
  const std::string mantissa = k == 1 ? s : s.substr(0, 1) + "." + s.substr(1);
  return minus + mantissa + "e" + (power > 0 ? "+" : "-") + std::to_string(std::abs(power));
}

double math_round(double value) {
  if (!std::isfinite(value)) {
    return value;
  }
  // `value - whole` is exact: below 2^52 the floor shares the value's exponent range and the
  // subtraction only clears bits, and at or above it every double is already an integer.
  const double whole = std::floor(value);
  return value - whole >= 0.5 ? whole + 1 : whole;
}

bool truthy(const Json& value) {
  switch (value.kind()) {
    case Json::Kind::Null: return false;
    case Json::Kind::Bool: return value.is_true();
    case Json::Kind::Number: {
      const double held = value.number_or(0);
      return !(held == 0 || std::isnan(held));
    }
    case Json::Kind::String: return !value.string_or("").empty();
    default: return true;
  }
}

double bare_number(const Json& object, std::string_view field, double fallback) {
  if (!object.has(field)) {
    return fallback;
  }
  const Json& value = object[field];
  return value.is_number() ? value.number_or(fallback) : std::numeric_limits<double>::quiet_NaN();
}

std::string interpolate(const Json* value) {
  if (value == nullptr) {
    return "undefined";
  }
  switch (value->kind()) {
    case Json::Kind::Null: return "null";
    case Json::Kind::Bool: return value->is_true() ? "true" : "false";
    case Json::Kind::Number: return number_to_string(value->number_or(0));
    case Json::Kind::String: return std::string(value->string_or(""));
    case Json::Kind::Array: {
      // `Array.prototype.join(',')`, which is what `${[1,2]}` is. Null and undefined elements
      // render as the empty string rather than as their own spellings.
      std::string out;
      bool first = true;
      for (const Json& item : value->items()) {
        if (!first) {
          out.push_back(',');
        }
        first = false;
        if (!item.is_null()) {
          out += interpolate(&item);
        }
      }
      return out;
    }
    default: return "[object Object]";
  }
}

std::string trim(std::string_view value) {
  std::size_t first = 0;
  for (;;) {
    const std::size_t width = space_at(value, first);
    if (width == 0) {
      break;
    }
    first += width;
  }
  std::size_t last = value.size();
  for (;;) {
    bool trimmed = false;
    for (const std::string& one : js_whitespace()) {
      if (last >= first + one.size() &&
          value.compare(last - one.size(), one.size(), one) == 0) {
        last -= one.size();
        trimmed = true;
        break;
      }
    }
    if (!trimmed) {
      break;
    }
  }
  return std::string(value.substr(first, last - first));
}

std::vector<std::uint16_t> utf16_units(std::string_view value) {
  std::vector<std::uint16_t> units;
  for (const char32_t code_point : code_points(value)) {
    if (code_point >= 0x10000) {
      const std::uint32_t rest = code_point - 0x10000;
      units.push_back(static_cast<std::uint16_t>(0xD800 + (rest >> 10)));
      units.push_back(static_cast<std::uint16_t>(0xDC00 + (rest & 0x3FF)));
    } else {
      units.push_back(static_cast<std::uint16_t>(code_point));
    }
  }
  return units;
}

std::size_t utf16_length(std::string_view value) { return utf16_units(value).size(); }

bool utf16_less(std::string_view left, std::string_view right) {
  const std::vector<std::uint16_t> one = utf16_units(left);
  const std::vector<std::uint16_t> other = utf16_units(right);
  return std::lexicographical_compare(one.begin(), one.end(), other.begin(), other.end());
}

std::string to_fixed(double value, int digits) {
  if (std::isnan(value)) {
    return "NaN";
  }
  if (std::isinf(value)) {
    return value > 0 ? "Infinity" : "-Infinity";
  }

  // ECMAScript takes the sign off first and then picks the integer `n` for which `n / 10^digits`
  // is nearest `x`, **the larger on a tie** — so ties are away from zero once the sign is restored.
  //
  // The decision is on the **exact binary value** of the double, so the expansion is printed in
  // full and rounded in decimal. `floor(x + 0.5)` is not the same rule: at 0.49999999999999994 the
  // sum rounds up to exactly 1.0 in binary and the shortcut answers 1, where the nearest integer to
  // the value itself is 0. That is the same input on which `roundHalfAway` is defined twice and two
  // ways in `reference/typescript`, and it is why there is one of these rather than two.
  const std::string sign = value < 0 ? "-" : "";
  const double magnitude = std::fabs(value);

  // A double's decimal expansion is finite: at most ~1080 digits after the point.
  std::vector<char> exact(1200 + 64, '\0');
  std::snprintf(exact.data(), exact.size(), "%.*f", 1100, magnitude);
  std::string text(exact.data());

  const std::size_t point = text.find('.');
  std::string whole = text.substr(0, point);
  std::string fraction = text.substr(point + 1);

  std::string kept = fraction.substr(0, static_cast<std::size_t>(digits));
  kept.append(static_cast<std::size_t>(digits) - kept.size(), '0');
  // Round up when the first discarded digit is 5 or more: 5-followed-by-zeros is exactly half and
  // half rounds away from zero, and 5-followed-by-anything is more than half.
  const bool round_up = fraction.size() > static_cast<std::size_t>(digits) &&
                        fraction[static_cast<std::size_t>(digits)] >= '5';

  if (round_up) {
    std::string carried = whole + kept;
    int at = static_cast<int>(carried.size()) - 1;
    for (; at >= 0; --at) {
      if (carried[static_cast<std::size_t>(at)] != '9') {
        ++carried[static_cast<std::size_t>(at)];
        break;
      }
      carried[static_cast<std::size_t>(at)] = '0';
    }
    if (at < 0) {
      carried.insert(carried.begin(), '1');
    }
    const std::size_t split = carried.size() - static_cast<std::size_t>(digits);
    whole = carried.substr(0, split);
    kept = carried.substr(split);
  }

  return digits == 0 ? sign + whole : sign + whole + "." + kept;
}

}  // namespace skynet::js
