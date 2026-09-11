// Declarative predicate evaluation — the C++ edition of `reference/typescript/predicate.ts`.
//
// Compatibility rules are data rather than code, so this contract is pure and **total**: it never
// throws on a malformed rule, and a rule it cannot decide does not pass.
//
// # The two answers
//
// `evaluate_outcome` returns three values — satisfied, unsatisfied, unevaluable — because §54.2
// asks a question a boolean cannot answer:
//
//     A precondition that cannot be evaluated refuses the skill, **naming the path that was
//     unresolvable**. Not "assume true", which is what free-text preconditions do by construction.
//
// `evaluate` is `satisfied(...)` of that outcome and never a second implementation. Two functions
// computing the same thing by two routes is the drift the corpus exists to catch, and here there
// would be nothing for the refinement to disagree with.
//
// # Two places where C++ is not JavaScript, and the specification is JavaScript's
//
// 1. **`undefined` does not exist here.** JSON has no `undefined`, so "this path named nothing"
//    has to stay distinguishable from a real `null` in the context — `exists` gives the same
//    answer for both and `eq` does not. A null `Json` cannot carry that distinction, so absence is
//    a null **pointer** and presence is a pointer to a value that may itself be null.
//
// 2. **`Number(string)` is not `strtod`.** The comparison operators coerce strings, and
//    JavaScript's coercion reads `0x1f` as 31, `1e3` as 1000, a blank string as 0, and `Infinity`
//    as infinity — which then fails the finiteness check and refuses. `strtod` accepts `0x1f` as a
//    hex float, `nan`, and a leading-garbage prefix, none of which JavaScript does. `js_number` is
//    the reference's grammar, and the corpus context carries `hex`, `exponent` and `infinite`
//    fields precisely to hold an edition to it.

#include "skynet_os/contracts.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/predicate.hpp"
#include "skynet_os/semver.hpp"
#include "skynet_os/text.hpp"

#include <algorithm>
#include <cmath>
#include <cstdlib>

namespace skynet::predicate {
namespace {

Outcome decided(bool value) {
  Outcome out;
  out.outcome = value ? "satisfied" : "unsatisfied";
  return out;
}

Outcome unevaluable(std::string reason, std::optional<std::string> path,
                    std::optional<std::string> op) {
  Outcome out;
  out.outcome = "unevaluable";
  out.reason = std::move(reason);
  out.path = std::move(path);
  out.op = std::move(op);
  return out;
}

/// A canonical decimal index, which is what a JSON array member is named: `^(0|[1-9]\d*)$`.
bool is_index(std::string_view segment) {
  if (segment.empty() || !std::all_of(segment.begin(), segment.end(), text::is_ascii_digit)) {
    return false;
  }
  return segment == "0" || segment[0] != '0';
}

/// `^0([xX][0-9a-fA-F]+|[oO][0-7]+|[bB][01]+)$`, and the base it names.
std::optional<std::pair<int, std::string>> radix_literal(const std::string& body) {
  if (body.size() < 3 || body[0] != '0') {
    return std::nullopt;
  }
  // ASCII by grammar — `0x`, `0X` and the rest are StringNumericLiteral's own spellings — and not
  // `std::tolower`, which consults the locale.
  const char marker = text::is_ascii_upper(body[1]) ? static_cast<char>(body[1] - 'A' + 'a') : body[1];
  int base = 0;
  bool (*admits)(char) = nullptr;
  if (marker == 'x') {
    base = 16;
    admits = [](char ch) {
      return text::is_ascii_digit(ch) || (ch >= 'a' && ch <= 'f') || (ch >= 'A' && ch <= 'F');
    };
  } else if (marker == 'o') {
    base = 8;
    admits = [](char ch) { return ch >= '0' && ch <= '7'; };
  } else if (marker == 'b') {
    base = 2;
    admits = [](char ch) { return ch == '0' || ch == '1'; };
  } else {
    return std::nullopt;
  }
  const std::string digits = body.substr(2);
  if (digits.empty() || !std::all_of(digits.begin(), digits.end(), admits)) {
    return std::nullopt;
  }
  return std::pair<int, std::string>(base, digits);
}

/// `^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$` — `Number(string)`'s decimal grammar.
bool is_decimal_literal(const std::string& body) {
  std::size_t at = 0;
  if (at < body.size() && (body[at] == '+' || body[at] == '-')) {
    ++at;
  }
  const std::size_t integer_start = at;
  while (at < body.size() && text::is_ascii_digit(body[at])) {
    ++at;
  }
  const bool had_integer = at > integer_start;
  if (had_integer) {
    if (at < body.size() && body[at] == '.') {
      ++at;
      while (at < body.size() && text::is_ascii_digit(body[at])) {
        ++at;
      }
    }
  } else {
    if (at >= body.size() || body[at] != '.') {
      return false;
    }
    ++at;
    const std::size_t fraction_start = at;
    while (at < body.size() && text::is_ascii_digit(body[at])) {
      ++at;
    }
    if (at == fraction_start) {
      return false;
    }
  }
  if (at < body.size() && (body[at] == 'e' || body[at] == 'E')) {
    ++at;
    if (at < body.size() && (body[at] == '+' || body[at] == '-')) {
      ++at;
    }
    const std::size_t exponent_start = at;
    while (at < body.size() && text::is_ascii_digit(body[at])) {
      ++at;
    }
    if (at == exponent_start) {
      return false;
    }
  }
  return at == body.size();
}

/// The path a rule value references, or nothing if it is a literal.
std::optional<std::string> referenced_path(const Json& value) {
  if (!value.is_object()) {
    return std::nullopt;
  }
  if (const std::optional<std::string> ref = value["__ref"].string_value(); ref.has_value()) {
    return ref;
  }
  // `{ path: … }` is accepted as an alias so hand-written rules read naturally — but only when it
  // is the whole object, or `{op, path, value}` would reference itself.
  if (value.size() == 1) {
    if (const std::optional<std::string> inner = value["path"].string_value(); inner.has_value()) {
      return inner;
    }
  }
  return std::nullopt;
}

std::optional<std::string> path_of(const Json& node) {
  const std::optional<std::string> path = node["path"].string_value();
  if (!path.has_value() || path->empty()) {
    return std::nullopt;
  }
  return path;
}

/// The children of a composite, or nothing when `predicates` is present and is not a list.
///
/// Absent and non-list are deliberately different answers. Absent is *no children*, and a
/// conjunction over no children is satisfied — a skill declaring no preconditions does not fail
/// them. A `predicates` that is a string is a **broken rule**, and answering "no children" there
/// would turn it into a pass, which is the failure this contract is written to stop.
const JsonArray* children_of(const Json& node, bool& malformed) {
  malformed = false;
  static const JsonArray none;
  if (!node.has("predicates") || node["predicates"].is_null()) {
    return &none;
  }
  if (!node["predicates"].is_array()) {
    malformed = true;
    return nullptr;
  }
  return &node["predicates"].items();
}

bool is_primitive(const Json* value) {
  return value == nullptr || (!value->is_object() && !value->is_array());
}

/// `===` between two primitives.
///
/// `true === 1` is false in JavaScript, and this edition gets that for free: a `Json` holding a
/// boolean and one holding a number are different kinds. It is spelled out anyway because the
/// Python edition needs a rule here and a reader comparing the two should see the same check.
bool strict_equal(const Json* left, const Json* right) {
  if (left == nullptr || right == nullptr) {
    return left == right;
  }
  if (left->kind() != right->kind()) {
    return false;
  }
  switch (left->kind()) {
    case Json::Kind::Null: return true;
    case Json::Kind::Bool: return left->is_true() == right->is_true();
    case Json::Kind::Number: return left->number_or(0) == right->number_or(0);
    case Json::Kind::String: return left->string_or("") == right->string_or("");
    default: return false;  // Objects and arrays never reach here; `identical` handles them.
  }
}

std::vector<const Json*> to_list(const Json* value) {
  std::vector<const Json*> out;
  if (value == nullptr || value->is_null()) {
    return out;
  }
  if (value->is_array()) {
    for (const Json& item : value->items()) {
      out.push_back(&item);
    }
    return out;
  }
  out.push_back(value);
  return out;
}

/// `Number(value)`, restricted to what the reference then accepts as finite.
std::optional<double> js_number(const Json* value) {
  if (value == nullptr) {
    return std::nullopt;
  }
  if (value->is_bool()) {
    // `Number(true)` is 1 in JavaScript, but the reference's `toNumber` accepts only a number or a
    // string, and a boolean is neither. It refuses, and so does this.
    return std::nullopt;
  }
  if (value->is_number()) {
    const double held = value->number_or(0);
    return std::isfinite(held) ? std::optional<double>(held) : std::nullopt;
  }
  if (!value->is_string()) {
    return std::nullopt;
  }

  const std::string body = js::trim(value->string_or(""));
  if (body.empty()) {
    // The reference guards this before coercing: `Number('')` is 0, which would make a blank field
    // compare as zero rather than refuse.
    return std::nullopt;
  }
  if (body == "Infinity" || body == "+Infinity" || body == "-Infinity") {
    return std::nullopt;  // Parsed, then discarded as non-finite. Two steps, one answer.
  }
  if (const auto radix = radix_literal(body); radix.has_value()) {
    return static_cast<double>(std::strtoll(radix->second.c_str(), nullptr, radix->first));
  }
  if (!is_decimal_literal(body)) {
    return std::nullopt;
  }
  const double parsed = std::strtod(body.c_str(), nullptr);
  return std::isfinite(parsed) ? std::optional<double>(parsed) : std::nullopt;
}

/// One side of a comparison.
///
/// `path` is where the value came from, or nothing for a literal, and it is load-bearing twice: it
/// is what a refusal names, and it is how `identical` decides object equality without a reference
/// to compare.
struct Operand {
  const Json* value = nullptr;
  std::optional<std::string> path;
  bool resolved = false;
};

Operand from_path(const Json& context, const std::string& path) {
  Operand out;
  out.value = read_path(context, path);
  out.path = path;
  out.resolved = out.value != nullptr;
  return out;
}

Operand from_value(const Json& value, const Json& context) {
  const std::optional<std::string> ref = referenced_path(value);
  if (!ref.has_value()) {
    Operand out;  // A literal is always resolved, even a literal `null`.
    out.value = &value;
    out.resolved = true;
    return out;
  }
  return from_path(context, *ref);
}

/// An element of a collection operand, which is a member of the context in its own right.
Operand element(const Operand& collection, const Json* item, std::size_t index) {
  Operand out;
  out.value = item;
  out.resolved = true;
  if (collection.path.has_value()) {
    out.path = *collection.path + "." + std::to_string(index);
  }
  return out;
}

/// The reference's equality rule, written out because `===` on objects is the host's.
///
/// Two operands are identical when both are primitives and equal, or when both were read from the
/// **same path of the same context** — the only way two non-primitive operands can be one object.
/// A literal object or array in a rule is therefore never equal to anything, which is what `===`
/// does and is worth knowing before writing `{"op":"eq","value":["a"]}` and wondering why it never
/// fires. `includes` is the operator for that.
bool identical(const Operand& left, const Operand& right) {
  if (is_primitive(left.value) && is_primitive(right.value)) {
    return strict_equal(left.value, right.value);
  }
  if (!left.path.has_value() || !right.path.has_value()) {
    return false;
  }
  return *left.path == *right.path;
}

bool contains(const Operand& collection, const Operand& needle) {
  const std::vector<const Json*> items = to_list(collection.value);
  for (std::size_t at = 0; at < items.size(); ++at) {
    if (identical(element(collection, items[at], at), needle)) {
      return true;
    }
  }
  return false;
}

std::optional<Outcome> first_unresolved(const std::string& op, const Operand& left,
                                        const Operand& right) {
  if (!left.resolved) {
    return unevaluable("unresolved_path", left.path, op);
  }
  if (!right.resolved) {
    return unevaluable("unresolved_path", right.path, op);
  }
  return std::nullopt;
}

}  // namespace

const Json* read_path(const Json& context, const std::string& path) {
  if (path.empty()) {
    return nullptr;
  }
  const Json* current = &context;
  for (const std::string& segment : text::split(path, '.')) {
    if (current == nullptr || current->is_null()) {
      return nullptr;
    }
    if (current->is_array()) {
      // **Members only.** An array segment must be a decimal index, so `capabilities.length` does
      // not resolve: `length` is a property of a JavaScript array rather than a member of the JSON
      // document, and an edition without JavaScript arrays could not reproduce it.
      if (!is_index(segment)) {
        return nullptr;
      }
      const std::size_t index = static_cast<std::size_t>(std::strtoull(segment.c_str(), nullptr, 10));
      if (index >= current->items().size()) {
        return nullptr;
      }
      current = &current->items()[index];
      continue;
    }
    if (!current->is_object() || !current->has(segment)) {
      return nullptr;
    }
    current = &(*current)[segment];
  }
  return current;
}

Outcome evaluate_outcome(const Json& node, const Json& context) {
  if (!node.is_object() || !node["op"].is_string()) {
    return unevaluable("malformed", std::nullopt, std::nullopt);
  }
  const std::string op(node["op"].string_or(""));

  // Order is part of the answer for the composites: a refusal names *a* predicate, and which one it
  // names must not depend on how an edition iterates.
  if (op == "and" || op == "or" || op == "not") {
    bool malformed = false;
    const JsonArray* children = children_of(node, malformed);
    if (malformed) {
      return unevaluable("malformed", std::nullopt, op);
    }

    if (op == "and") {
      for (const Json& child : *children) {
        const Outcome outcome = evaluate_outcome(child, context);
        if (outcome.outcome != "satisfied") {
          return outcome;
        }
      }
      // Nothing was asked for. A skill declaring no preconditions is not one that fails them.
      return decided(true);
    }

    if (op == "or") {
      std::optional<Outcome> first_unevaluable;
      for (const Json& child : *children) {
        const Outcome outcome = evaluate_outcome(child, context);
        if (outcome.outcome == "satisfied") {
          return outcome;
        }
        if (outcome.outcome == "unevaluable" && !first_unevaluable.has_value()) {
          first_unevaluable = outcome;
        }
      }
      // An alternative that held would have returned already; an alternative nobody could read
      // means this disjunction is not known to be false, only unread.
      return first_unevaluable.has_value() ? *first_unevaluable : decided(false);
    }

    const Json* child = nullptr;
    if (node.has("predicate") && !node["predicate"].is_null()) {
      child = &node["predicate"];
    } else if (!children->empty()) {
      child = &children->front();
    }
    if (child == nullptr) {
      return unevaluable("missing_operand", std::nullopt, op);
    }
    const Outcome outcome = evaluate_outcome(*child, context);
    if (outcome.outcome == "unevaluable") {
      return outcome;  // Ignorance does not negate into knowledge.
    }
    return decided(outcome.outcome == "unsatisfied");
  }

  const std::optional<std::string> path = path_of(node);

  if (op == "exists") {
    if (!path.has_value()) {
      return unevaluable("missing_path", std::nullopt, op);
    }
    // The one operator that answers *about* resolution, so it never refuses for lack of it.
    const Json* value = read_path(context, *path);
    if (value == nullptr || value->is_null()) {
      return decided(false);
    }
    if (value->is_array()) {
      return decided(!value->items().empty());
    }
    if (value->is_string()) {
      return decided(!js::trim(value->string_or("")).empty());
    }
    return decided(true);
  }

  if (op == "eq" || op == "neq") {
    if (!path.has_value()) {
      return unevaluable("missing_path", std::nullopt, op);
    }
    const Operand left = from_path(context, *path);
    const Operand right = from_value(node["value"], context);
    if (const auto refused = first_unresolved(op, left, right); refused.has_value()) {
      return *refused;
    }
    const bool equal = identical(left, right);
    return decided(op == "eq" ? equal : !equal);
  }

  if (op == "in") {
    if (!path.has_value()) {
      return unevaluable("missing_path", std::nullopt, op);
    }
    const Operand needle = from_path(context, *path);
    const Operand collection = from_value(node["value"], context);
    if (const auto refused = first_unresolved(op, needle, collection); refused.has_value()) {
      return *refused;
    }
    return decided(contains(collection, needle));
  }

  if (op == "includes") {
    // The context value is a collection that must contain the rule's value.
    if (!path.has_value()) {
      return unevaluable("missing_path", std::nullopt, op);
    }
    const Operand collection = from_path(context, *path);
    const Operand needle = from_value(node["value"], context);
    if (const auto refused = first_unresolved(op, collection, needle); refused.has_value()) {
      return *refused;
    }
    return decided(contains(collection, needle));
  }

  if (op == "includes_all" || op == "includes_any") {
    if (!path.has_value()) {
      return unevaluable("missing_path", std::nullopt, op);
    }
    const Operand collection = from_path(context, *path);
    const Operand wanted = from_value(node["value"], context);
    if (const auto refused = first_unresolved(op, collection, wanted); refused.has_value()) {
      return *refused;
    }
    const std::vector<const Json*> items = to_list(wanted.value);
    bool all_present = true;
    bool any_present = false;
    for (std::size_t at = 0; at < items.size(); ++at) {
      const bool present = contains(collection, element(wanted, items[at], at));
      all_present = all_present && present;
      any_present = any_present || present;
    }
    // An empty requirement is vacuously satisfied — nothing was asked for. Reachable only from a
    // requirement that *is* empty, never from one nobody could read.
    if (op == "includes_any") {
      return decided(items.empty() || any_present);
    }
    return decided(all_present);
  }

  if (op == "gte" || op == "lte" || op == "gt" || op == "lt") {
    if (!path.has_value()) {
      return unevaluable("missing_path", std::nullopt, op);
    }
    const Operand left_operand = from_path(context, *path);
    const Operand right_operand = from_value(node["value"], context);
    if (const auto refused = first_unresolved(op, left_operand, right_operand);
        refused.has_value()) {
      return *refused;
    }
    const std::optional<double> left = js_number(left_operand.value);
    if (!left.has_value()) {
      return unevaluable("not_a_number", left_operand.path, op);
    }
    const std::optional<double> right = js_number(right_operand.value);
    if (!right.has_value()) {
      return unevaluable("not_a_number", right_operand.path, op);
    }
    if (op == "gte") return decided(*left >= *right);
    if (op == "lte") return decided(*left <= *right);
    if (op == "gt") return decided(*left > *right);
    return decided(*left < *right);
  }

  if (op == "semver_satisfies") {
    if (!path.has_value()) {
      return unevaluable("missing_path", std::nullopt, op);
    }
    const Operand version_operand = from_path(context, *path);
    const Operand range_operand = from_value(node["value"], context);
    if (const auto refused = first_unresolved(op, version_operand, range_operand);
        refused.has_value()) {
      return *refused;
    }
    // A version this edition cannot parse is a fact about the declaration, not a version that fell
    // outside the window — so it is named rather than counted as a miss.
    const std::optional<std::string> version = version_operand.value->string_value();
    if (!version.has_value() || !semver::is_valid_version(*version)) {
      return unevaluable("not_a_version", version_operand.path, op);
    }
    const std::optional<std::string> range_text = range_operand.value->string_value();
    if (!range_text.has_value() || !semver::is_valid_range(*range_text)) {
      return unevaluable("not_a_version", range_operand.path, op);
    }
    return decided(semver::satisfies(*version, *range_text));
  }

  return unevaluable("unknown_op", path, op);
}

bool evaluate(const Json& node, const Json& context) {
  return evaluate_outcome(node, context).outcome == "satisfied";
}

Json detail(const Json& node, const Json& context) {
  Json out = Json::object();
  // Every path the predicate tree mentions, with what it read, so a finding can show *why*.
  const auto record = [&](const std::string& where) {
    const Json* value = read_path(context, where);
    out.set(where, value == nullptr ? Json::null() : *value);
  };
  const auto walk = [&](auto&& self, const Json& current) -> void {
    if (!current.is_object()) {
      return;
    }
    if (const std::optional<std::string> where = path_of(current); where.has_value()) {
      record(*where);
    }
    // A `__ref` value is as much part of the comparison as the left-hand path.
    if (const std::optional<std::string> ref = referenced_path(current["value"]); ref.has_value()) {
      record(*ref);
    }
    if (current.has("predicate") && !current["predicate"].is_null()) {
      self(self, current["predicate"]);
    }
    bool malformed = false;
    if (const JsonArray* children = children_of(current, malformed); children != nullptr) {
      for (const Json& child : *children) {
        self(self, child);
      }
    }
  };
  walk(walk, node);
  return out;
}

}  // namespace skynet::predicate

namespace skynet::contracts {

Json predicate(const std::string& /*label*/, const Json& input) {
  const skynet::predicate::Outcome outcome =
      skynet::predicate::evaluate_outcome(input["predicate"], input["context"]);
  // Both the boolean and the outcome, because the boolean is what the compatibility engine uses
  // and the outcome is what §54.2 requires. The corpus freezes them together so a future edition
  // cannot satisfy one and quietly disagree on the other.
  Json out = Json::object();
  out.set("satisfied", Json::boolean(outcome.outcome == "satisfied"));
  out.set("outcome", Json::string(outcome.outcome));
  out.set("reason", outcome.reason.has_value() ? Json::string(*outcome.reason) : Json::null());
  out.set("path", outcome.path.has_value() ? Json::string(*outcome.path) : Json::null());
  out.set("op", outcome.op.has_value() ? Json::string(*outcome.op) : Json::null());
  return out;
}

}  // namespace skynet::contracts
