// Canonical JSON, the C++ half.
//
// This is a deliberate fourth implementation of the same rule — `crates/skynet-json` is the first,
// `reference/typescript/canonical.ts` the second and `editions/python/skynet_os/canonical.py` the
// third — and the duplication is the point. §31.4 requires every edition to reproduce the corpus
// **byte-identically**, and a corpus whose expected values were produced by the same code that
// checks them would prove nothing.
//
// The rule, restated so this file can be read on its own:
//
//   1. No insignificant whitespace.
//   2. Object keys sorted by the byte sequence of their UTF-8 encoding.
//   3. Numbers are finite. An integral value below 2^53 is written as an integer, `-0` as `0`;
//      anything else as the shortest decimal that round-trips, always positional, never
//      exponential.
//   4. Strings escape `"`, `\` and the C0 controls and nothing else.
//   5. Absent fields are omitted; `null` is written as `null`. The two are different facts — the
//      whole system distinguishes "nobody declared a safety envelope" from "an envelope was
//      declared and is empty".
//
// # Rule 5 has no sentinel here, on purpose
//
// The Python edition needs an `OMIT` object because a dict cannot hold "this key is not here".
// C++ does not have that problem: a field that is absent is a `set()` that was never called, so
// there is no state in which a stale sentinel survives into output. Rule 5 becomes unrepresentable
// rather than enforced, which is the stronger of the two.
//
// # Rule 2 is applied by the writer, and the container keeps insertion order
//
// The obvious design is to keep the object sorted on insert, so an unsorted object cannot be
// emitted. It is wrong here, and the reason is worth stating because it cost a rewrite: **the
// reference edition iterates objects in insertion order**, and several contracts' output depends
// on it. `evaluate_cycle` walks the commanded channels and appends one entry to `clamped` per
// channel it reduced, so the order of that array is the order the channels appeared in the
// document. An edition whose container had silently sorted them would emit the same set of clamps
// in a different order — and the corpus compares element by element, which is exactly the "a
// warning in a different order today is a different clamp tomorrow" case §31.4 spends a rule on.
//
// So `JsonObject` preserves insertion order, and `canonical()` sorts the keys as it writes them.
// `std::char_traits<char>::lt` compares as `unsigned char`, so `std::string`'s ordering *is* the
// UTF-8 byte ordering rule 2 asks for — it differs from code-point ordering above the BMP, and an
// edition that sorted by code point would agree for ASCII and diverge on the first vendor channel
// name that was not.

#ifndef SKYNET_OS_JSON_HPP
#define SKYNET_OS_JSON_HPP

#include <cstdint>
#include <exception>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <variant>
#include <vector>

namespace skynet {

class Json;

/// A JSON array. `std::vector` over an incomplete type is permitted since C++17.
using JsonArray = std::vector<Json>;
/// A JSON object, in **insertion order** — canonical order is applied when writing, not stored.
/// See the header comment: the reference edition iterates in insertion order and contracts depend
/// on it.
using JsonObject = std::vector<std::pair<std::string, Json>>;

/// Thrown when input is not JSON, or when a value cannot be serialised in canonical form.
///
/// Serialisation failure is deliberately loud rather than lenient: a non-finite number emitted as
/// `null` would turn "this was not measured" into "this was measured as nothing", which is the
/// distinction §74.3 spends a whole section on.
class JsonError : public std::exception {
 public:
  explicit JsonError(std::string message) : message_(std::move(message)) {}
  [[nodiscard]] const char* what() const noexcept override { return message_.c_str(); }

 private:
  std::string message_;
};

class Json {
 public:
  enum class Kind { Null, Bool, Number, String, Array, Object };

  Json() = default;  //< Null. The default is the absent-ish value, as it is in every other edition.

  static Json null() { return Json{}; }
  static Json boolean(bool value);
  static Json number(double value);
  static Json string(std::string value);
  static Json array();
  static Json array(JsonArray items);
  static Json object();

  [[nodiscard]] Kind kind() const;

  [[nodiscard]] bool is_null() const { return kind() == Kind::Null; }
  [[nodiscard]] bool is_bool() const { return kind() == Kind::Bool; }
  [[nodiscard]] bool is_number() const { return kind() == Kind::Number; }
  [[nodiscard]] bool is_string() const { return kind() == Kind::String; }
  [[nodiscard]] bool is_array() const { return kind() == Kind::Array; }
  [[nodiscard]] bool is_object() const { return kind() == Kind::Object; }

  /// Strictly `true` — not "truthy".
  ///
  /// Several corpora and every edition's self-check turn on this: a `1`, a `"yes"` or a non-empty
  /// object in a boolean field is **not** a declaration. `is_true()` says so at the one place the
  /// question is asked, so no caller can accidentally ask the looser one.
  [[nodiscard]] bool is_true() const;

  /// The number, or `fallback` when this is not a number. Non-finite input never reaches here:
  /// the parser rejects it and `Json::number` throws.
  [[nodiscard]] double number_or(double fallback) const;
  /// The number, or nothing — for the fields where "absent" and "zero" are different facts.
  [[nodiscard]] std::optional<double> number() const;
  [[nodiscard]] std::string_view string_or(std::string_view fallback) const;
  [[nodiscard]] std::optional<std::string> string_value() const;

  /// The array's elements, or an empty span when this is not an array.
  [[nodiscard]] const JsonArray& items() const;
  /// The object's entries **in insertion order**, or empty when this is not an object. This is
  /// the order the reference edition's `Object.entries` yields for a parsed document, and the
  /// order contracts that walk an object must use. Canonical output is sorted by `canonical()`.
  [[nodiscard]] const JsonObject& entries() const;

  /// Look a key up. A missing key and an explicit `null` both read as a Null `Json`; callers that
  /// must tell them apart use `has()`.
  [[nodiscard]] const Json& operator[](std::string_view key) const;
  [[nodiscard]] bool has(std::string_view key) const;

  /// Set a key, replacing any existing one in place. A new key is appended, so insertion order is
  /// the order keys were first set.
  Json& set(std::string key, Json value);
  /// Set a key only when `value` holds something — rule 5's ordinary case, spelled once.
  Json& set_if(std::string key, const std::optional<double>& value);
  Json& set_if(std::string key, const std::optional<std::string>& value);
  /// Append to an array.
  Json& push(Json value);

  [[nodiscard]] std::size_t size() const;

  /// Serialise in canonical form.
  [[nodiscard]] std::string canonical() const;

  /// Parse one JSON document. Throws `JsonError` on anything that is not exactly one document.
  static Json parse(std::string_view text);

 private:
  std::variant<std::monostate, bool, double, std::string, JsonArray, JsonObject> value_;
};

/// Format one number in canonical form. Exposed because the corpus pins numbers directly.
[[nodiscard]] std::string format_number(double value);

}  // namespace skynet

#endif  // SKYNET_OS_JSON_HPP
