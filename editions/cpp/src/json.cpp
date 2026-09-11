#include "skynet_os/json.hpp"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>

namespace skynet {
namespace {

/// 2^53. Above this an integral double can no longer be trusted to round-trip through a decimal
/// integer, so the shortest-round-trip path takes over.
constexpr double kIntSafe = 9007199254740992.0;

/// The empty results handed back on a kind mismatch, so no accessor has to return a pointer and no
/// caller has to hold a null check.
const JsonArray& empty_array() {
  static const JsonArray value;
  return value;
}
const JsonObject& empty_object() {
  static const JsonObject value;
  return value;
}
const Json& null_json() {
  static const Json value;
  return value;
}

/// Turn `-4.4e-01` into `-0.44`.
///
/// `%e` always renders as `<digits>e<sign><exponent>`, which is the only shape this handles — the
/// same assumption the reference edition makes about `toExponential`. The exponent's digit count
/// varies by platform (two on glibc, three on some Windows runtimes), so it is parsed rather than
/// sliced.
std::string exponential_to_positional(const std::string& rendered) {
  const std::size_t marker = rendered.find('e');
  if (marker == std::string::npos) {
    return rendered;
  }

  std::string mantissa = rendered.substr(0, marker);
  const int exponent = std::atoi(rendered.c_str() + marker + 1);

  const bool negative = !mantissa.empty() && mantissa.front() == '-';
  if (negative) {
    mantissa.erase(mantissa.begin());
  }

  std::string digits;
  digits.reserve(mantissa.size());
  for (const char ch : mantissa) {
    if (ch != '.') {
      digits.push_back(ch);
    }
  }
  const int digit_count = static_cast<int>(digits.size());

  // The first digit carries weight 10^exponent, so the point falls after `exponent + 1` digits.
  const int point = exponent + 1;

  std::string out;
  if (negative) {
    out.push_back('-');
  }
  if (point <= 0) {
    out += "0.";
    out.append(static_cast<std::size_t>(-point), '0');
    out += digits;
  } else if (point >= digit_count) {
    out += digits;
    out.append(static_cast<std::size_t>(point - digit_count), '0');
  } else {
    out += digits.substr(0, static_cast<std::size_t>(point));
    out.push_back('.');
    out += digits.substr(static_cast<std::size_t>(point));
  }
  return out;
}

void write_string(std::string_view value, std::string& out) {
  out.push_back('"');
  // Byte-wise, not code-point-wise, and that is correct rather than lazy: every byte needing an
  // escape is below 0x20 or is a quote or a backslash, and every byte of a multi-byte UTF-8
  // sequence is >= 0x80. So this copies UTF-8 through untouched and produces the same bytes as an
  // edition that decoded it first.
  for (const char raw : value) {
    const auto byte = static_cast<unsigned char>(raw);
    switch (byte) {
      case '"': out += "\\\""; continue;
      case '\\': out += "\\\\"; continue;
      case '\b': out += "\\b"; continue;
      case '\f': out += "\\f"; continue;
      case '\n': out += "\\n"; continue;
      case '\r': out += "\\r"; continue;
      case '\t': out += "\\t"; continue;
      default: break;
    }
    if (byte < 0x20) {
      char escape[8];
      std::snprintf(escape, sizeof(escape), "\\u%04x", byte);
      out += escape;
    } else {
      out.push_back(raw);
    }
  }
  out.push_back('"');
}

void write_value(const Json& value, std::string& out) {
  switch (value.kind()) {
    case Json::Kind::Null:
      out += "null";
      return;
    case Json::Kind::Bool:
      out += value.is_true() ? "true" : "false";
      return;
    case Json::Kind::Number:
      out += format_number(value.number_or(0.0));
      return;
    case Json::Kind::String:
      write_string(value.string_or(""), out);
      return;
    case Json::Kind::Array: {
      out.push_back('[');
      bool first = true;
      for (const Json& item : value.items()) {
        if (!first) {
          out.push_back(',');
        }
        first = false;
        write_value(item, out);
      }
      out.push_back(']');
      return;
    }
    case Json::Kind::Object: {
      out.push_back('{');
      // The container holds insertion order, because the reference iterates in it; rule 2 is
      // applied here, on the way out. Sorting pointers rather than entries so a large object is
      // not copied to be written.
      const JsonObject& entries = value.entries();
      std::vector<const std::pair<std::string, Json>*> order;
      order.reserve(entries.size());
      for (const auto& entry : entries) {
        order.push_back(&entry);
      }
      std::sort(order.begin(), order.end(), [](const auto* left, const auto* right) {
        return left->first < right->first;
      });
      bool first = true;
      for (const auto* entry : order) {
        if (!first) {
          out.push_back(',');
        }
        first = false;
        write_string(entry->first, out);
        out.push_back(':');
        write_value(entry->second, out);
      }
      out.push_back('}');
      return;
    }
  }
  throw JsonError("cannot serialise a value of unknown kind");
}

// The parser. Recursive descent with a bounded depth: §7.5 of the coding standard says to bound
// recursion on untrusted input, and while a corpus file is not untrusted, the parser is this
// edition's boundary — one that is safe only for its current caller is one call site away from not
// being safe at all.

constexpr int kMaxDepth = 200;

class Parser {
 public:
  explicit Parser(std::string_view text) : text_(text) {}

  Json parse_document() {
    skip_whitespace();
    Json value = parse_value(0);
    skip_whitespace();
    if (at_ < text_.size()) {
      throw JsonError("trailing content after the JSON document at byte " + std::to_string(at_));
    }
    return value;
  }

 private:
  std::string_view text_;
  std::size_t at_ = 0;

  [[noreturn]] void fail(const std::string& what) const {
    throw JsonError(what + " at byte " + std::to_string(at_));
  }

  [[nodiscard]] char peek() const { return at_ < text_.size() ? text_[at_] : '\0'; }

  void skip_whitespace() {
    while (at_ < text_.size()) {
      const char ch = text_[at_];
      if (ch == ' ' || ch == '\t' || ch == '\n' || ch == '\r') {
        ++at_;
      } else {
        break;
      }
    }
  }

  void expect(char ch) {
    if (peek() != ch) {
      fail(std::string("expected ") + ch);
    }
    ++at_;
  }

  bool literal(std::string_view word) {
    if (text_.compare(at_, word.size(), word) == 0) {
      at_ += word.size();
      return true;
    }
    return false;
  }

  Json parse_value(int depth) {
    if (depth > kMaxDepth) {
      fail("nesting deeper than " + std::to_string(kMaxDepth));
    }
    switch (peek()) {
      case '{': return parse_object(depth);
      case '[': return parse_array(depth);
      case '"': return Json::string(parse_string());
      case 't':
        if (literal("true")) return Json::boolean(true);
        fail("expected true");
      case 'f':
        if (literal("false")) return Json::boolean(false);
        fail("expected false");
      case 'n':
        if (literal("null")) return Json::null();
        fail("expected null");
      default: return parse_number();
    }
  }

  Json parse_object(int depth) {
    expect('{');
    Json out = Json::object();
    skip_whitespace();
    if (peek() == '}') {
      ++at_;
      return out;
    }
    for (;;) {
      skip_whitespace();
      std::string key = parse_string();
      skip_whitespace();
      expect(':');
      skip_whitespace();
      out.set(std::move(key), parse_value(depth + 1));
      skip_whitespace();
      if (peek() == ',') {
        ++at_;
        continue;
      }
      expect('}');
      return out;
    }
  }

  Json parse_array(int depth) {
    expect('[');
    Json out = Json::array();
    skip_whitespace();
    if (peek() == ']') {
      ++at_;
      return out;
    }
    for (;;) {
      skip_whitespace();
      out.push(parse_value(depth + 1));
      skip_whitespace();
      if (peek() == ',') {
        ++at_;
        continue;
      }
      expect(']');
      return out;
    }
  }

  /// Encode one code point as UTF-8. The `\u` escape is the only place the parser holds a code
  /// point rather than bytes, so this is the only place encoding happens.
  static void append_utf8(std::uint32_t code_point, std::string& out) {
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

  std::uint32_t parse_hex4() {
    if (at_ + 4 > text_.size()) {
      fail("truncated unicode escape");
    }
    std::uint32_t value = 0;
    for (int index = 0; index < 4; ++index) {
      const char ch = text_[at_ + static_cast<std::size_t>(index)];
      value <<= 4;
      if (ch >= '0' && ch <= '9') {
        value |= static_cast<std::uint32_t>(ch - '0');
      } else if (ch >= 'a' && ch <= 'f') {
        value |= static_cast<std::uint32_t>(ch - 'a' + 10);
      } else if (ch >= 'A' && ch <= 'F') {
        value |= static_cast<std::uint32_t>(ch - 'A' + 10);
      } else {
        fail("not a hex digit in a unicode escape");
      }
    }
    at_ += 4;
    return value;
  }

  std::string parse_string() {
    expect('"');
    std::string out;
    for (;;) {
      if (at_ >= text_.size()) {
        fail("unterminated string");
      }
      const char ch = text_[at_++];
      if (ch == '"') {
        return out;
      }
      if (ch != '\\') {
        out.push_back(ch);
        continue;
      }
      if (at_ >= text_.size()) {
        fail("unterminated escape");
      }
      const char escape = text_[at_++];
      switch (escape) {
        case '"': out.push_back('"'); break;
        case '\\': out.push_back('\\'); break;
        case '/': out.push_back('/'); break;
        case 'b': out.push_back('\b'); break;
        case 'f': out.push_back('\f'); break;
        case 'n': out.push_back('\n'); break;
        case 'r': out.push_back('\r'); break;
        case 't': out.push_back('\t'); break;
        case 'u': {
          std::uint32_t code_point = parse_hex4();
          // A high surrogate is half a code point. Joining the pair here rather than emitting both
          // halves is what keeps a non-BMP key sorting by its real UTF-8 bytes.
          if (code_point >= 0xD800 && code_point <= 0xDBFF && at_ + 1 < text_.size() &&
              text_[at_] == '\\' && text_[at_ + 1] == 'u') {
            const std::size_t resume = at_;
            at_ += 2;
            const std::uint32_t low = parse_hex4();
            if (low >= 0xDC00 && low <= 0xDFFF) {
              code_point = 0x10000 + ((code_point - 0xD800) << 10) + (low - 0xDC00);
            } else {
              at_ = resume;  // Not a pair after all; the lone high surrogate stands.
            }
          }
          append_utf8(code_point, out);
          break;
        }
        default: fail("unknown escape");
      }
    }
  }

  Json parse_number() {
    const std::size_t start = at_;
    if (peek() == '-') {
      ++at_;
    }
    while (at_ < text_.size() && ((text_[at_] >= '0' && text_[at_] <= '9') || text_[at_] == '.' ||
                                  text_[at_] == 'e' || text_[at_] == 'E' || text_[at_] == '+' ||
                                  text_[at_] == '-')) {
      ++at_;
    }
    if (at_ == start) {
      fail("expected a value");
    }
    const std::string literal_text(text_.substr(start, at_ - start));
    char* end = nullptr;
    const double value = std::strtod(literal_text.c_str(), &end);
    if (end == nullptr || *end != '\0') {
      at_ = start;
      fail("not a number");
    }
    // JSON has no NaN and no Infinity, and a literal that overflowed to infinity is a document this
    // edition must not pretend to have read. Loud, per §3.6's fail-closed rule.
    if (!std::isfinite(value)) {
      at_ = start;
      fail("a non-finite number is not JSON");
    }
    return Json::number(value);
  }
};

}  // namespace

std::string format_number(double value) {
  if (!std::isfinite(value)) {
    throw JsonError("cannot serialise a non-finite number");
  }
  if (value == 0.0) {
    return "0";  // covers -0.0, which rule 3 writes as 0
  }

  double integral = 0.0;
  if (std::modf(value, &integral) == 0.0 && std::fabs(value) < kIntSafe) {
    return std::to_string(static_cast<long long>(value));
  }

  // Ascending precision until the rendered value parses back equal, mirroring the reference
  // edition exactly rather than leaning on the platform's own shortest-round-trip printer.
  // "Happens to agree today" is not what a conformance corpus is held together by.
  char buffer[64];
  for (int precision = 1; precision <= 17; ++precision) {
    std::snprintf(buffer, sizeof(buffer), "%.*e", precision - 1, value);
    if (std::strtod(buffer, nullptr) == value) {
      return exponential_to_positional(buffer);
    }
  }
  std::snprintf(buffer, sizeof(buffer), "%.16e", value);
  return exponential_to_positional(buffer);
}

Json Json::boolean(bool value) {
  Json out;
  out.value_ = value;
  return out;
}

Json Json::number(double value) {
  if (!std::isfinite(value)) {
    throw JsonError("cannot hold a non-finite number");
  }
  Json out;
  out.value_ = value;
  return out;
}

Json Json::string(std::string value) {
  Json out;
  out.value_ = std::move(value);
  return out;
}

Json Json::array() {
  Json out;
  out.value_ = JsonArray{};
  return out;
}

Json Json::array(JsonArray items) {
  Json out;
  out.value_ = std::move(items);
  return out;
}

Json Json::object() {
  Json out;
  out.value_ = JsonObject{};
  return out;
}

Json::Kind Json::kind() const {
  switch (value_.index()) {
    case 0: return Kind::Null;
    case 1: return Kind::Bool;
    case 2: return Kind::Number;
    case 3: return Kind::String;
    case 4: return Kind::Array;
    default: return Kind::Object;
  }
}

bool Json::is_true() const {
  const bool* held = std::get_if<bool>(&value_);
  return held != nullptr && *held;
}

double Json::number_or(double fallback) const {
  const double* held = std::get_if<double>(&value_);
  return held != nullptr ? *held : fallback;
}

std::optional<double> Json::number() const {
  const double* held = std::get_if<double>(&value_);
  if (held == nullptr) {
    return std::nullopt;
  }
  return *held;
}

std::string_view Json::string_or(std::string_view fallback) const {
  const std::string* held = std::get_if<std::string>(&value_);
  return held != nullptr ? std::string_view(*held) : fallback;
}

std::optional<std::string> Json::string_value() const {
  const std::string* held = std::get_if<std::string>(&value_);
  if (held == nullptr) {
    return std::nullopt;
  }
  return *held;
}

const JsonArray& Json::items() const {
  const JsonArray* held = std::get_if<JsonArray>(&value_);
  return held != nullptr ? *held : empty_array();
}

const JsonObject& Json::entries() const {
  const JsonObject* held = std::get_if<JsonObject>(&value_);
  return held != nullptr ? *held : empty_object();
}

namespace {
/// Linear, because the container is in insertion order rather than sorted (see the header) and
/// because the objects this edition reads are small: a cycle's channel map, a descriptor node, a
/// notice. One helper, so lookup and insert cannot disagree about what "the same key" means.
const std::pair<std::string, Json>* find_entry(const JsonObject& entries, std::string_view key) {
  for (const auto& entry : entries) {
    if (entry.first == key) {
      return &entry;
    }
  }
  return nullptr;
}
}  // namespace

const Json& Json::operator[](std::string_view key) const {
  const JsonObject* held = std::get_if<JsonObject>(&value_);
  if (held == nullptr) {
    return null_json();
  }
  const auto* found = find_entry(*held, key);
  return found == nullptr ? null_json() : found->second;
}

bool Json::has(std::string_view key) const {
  const JsonObject* held = std::get_if<JsonObject>(&value_);
  return held != nullptr && find_entry(*held, key) != nullptr;
}

Json& Json::set(std::string key, Json value) {
  JsonObject* held = std::get_if<JsonObject>(&value_);
  if (held == nullptr) {
    value_ = JsonObject{};
    held = std::get_if<JsonObject>(&value_);
  }
  for (auto& entry : *held) {
    if (entry.first == key) {
      // Replaced in place, so re-setting a key does not move it to the end. A document read and
      // written back keeps the order it arrived in.
      entry.second = std::move(value);
      return *this;
    }
  }
  held->emplace_back(std::move(key), std::move(value));
  return *this;
}

Json& Json::set_if(std::string key, const std::optional<double>& value) {
  if (value.has_value()) {
    set(std::move(key), Json::number(*value));
  }
  return *this;
}

Json& Json::set_if(std::string key, const std::optional<std::string>& value) {
  if (value.has_value()) {
    set(std::move(key), Json::string(*value));
  }
  return *this;
}

Json& Json::push(Json value) {
  JsonArray* held = std::get_if<JsonArray>(&value_);
  if (held == nullptr) {
    value_ = JsonArray{};
    held = std::get_if<JsonArray>(&value_);
  }
  held->push_back(std::move(value));
  return *this;
}

std::size_t Json::size() const {
  if (const JsonArray* array_held = std::get_if<JsonArray>(&value_)) {
    return array_held->size();
  }
  if (const JsonObject* object_held = std::get_if<JsonObject>(&value_)) {
    return object_held->size();
  }
  if (const std::string* string_held = std::get_if<std::string>(&value_)) {
    return string_held->size();
  }
  return 0;
}

std::string Json::canonical() const {
  std::string out;
  write_value(*this, out);
  return out;
}

Json Json::parse(std::string_view text) {
  Parser parser(text);
  return parser.parse_document();
}

}  // namespace skynet
