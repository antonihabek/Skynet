// Semantic versions and ranges (§54.2, §8).
//
// Two questions with two different consumers, and the corpus holds both:
//
//   * **compare** — how two versions order. Everything that renders a version list, and the
//     OS-too-old check at admission.
//   * **satisfies** — whether a version falls inside a declared range. §54.2 made
//     `semver_satisfies` an operator a robot evaluates for a skill's preconditions, which is what
//     turned this from a utility into a contract.
//
// `is_valid_range` exists separately from `satisfies` for a reason worth keeping: `satisfies`
// answers false both for *"this version is outside that window"* and for *"that is not a window"*,
// and §54.2 needs those apart. A skill whose precondition names an unreadable range is a skill
// nobody can ever run, and its publisher is the one who has to hear about it; a battery that is
// too low is a different sentence with a different remedy.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/semver.hpp"
#include "skynet_os/text.hpp"

#include <algorithm>
#include <cstdlib>

namespace skynet::semver {
namespace {

/// Read a version component the way the reference edition does: as an IEEE-754 double.
///
/// **Not** as a 64-bit integer, and this is the one place where an exact integer type is a hazard
/// rather than a convenience. JavaScript's `Number()` and Rust's `f64` both round
/// `9007199254740993` to `9007199254740992`; a `long long` keeps it exactly. An edition that parsed
/// exactly would order two versions the reference edition calls equal — and would then *serialise*
/// them identically, because canonical form renders both as the rounded value. A disagreement
/// about ordering that is invisible in the output is the worst shape a divergence can take.
///
/// The corpus case is called `large-major` and its note says so in advance.
double component(const std::string& digits) { return std::strtod(digits.c_str(), nullptr); }

bool all_digits(std::string_view value) {
  return !value.empty() && std::all_of(value.begin(), value.end(), text::is_ascii_digit);
}

/// A prerelease identifier character: `[0-9A-Za-z.-]`.
bool is_prerelease_char(char ch) {
  return text::is_ascii_digit(ch) || text::is_ascii_alpha(ch) || ch == '.' || ch == '-';
}

/// Semver §11: numeric identifiers rank below alphanumeric, and more identifiers wins ties.
int compare_prerelease(const std::optional<std::string>& left,
                       const std::optional<std::string>& right) {
  if (left == right) {
    return 0;
  }
  // A version without a prerelease outranks one with it. 1.0.0 is newer than 1.0.0-rc1.
  if (!left.has_value()) {
    return 1;
  }
  if (!right.has_value()) {
    return -1;
  }

  const std::vector<std::string> left_parts = text::split(*left, '.');
  const std::vector<std::string> right_parts = text::split(*right, '.');
  const std::size_t length = std::max(left_parts.size(), right_parts.size());

  for (std::size_t at = 0; at < length; ++at) {
    if (at >= left_parts.size()) {
      return -1;
    }
    if (at >= right_parts.size()) {
      return 1;
    }
    const std::string& one = left_parts[at];
    const std::string& other = right_parts[at];
    const bool one_numeric = all_digits(one);
    const bool other_numeric = all_digits(other);
    if (one_numeric && other_numeric) {
      const double difference = component(one) - component(other);
      if (difference != 0) {
        return difference < 0 ? -1 : 1;
      }
      continue;
    }
    if (one_numeric) {
      return -1;
    }
    if (other_numeric) {
      return 1;
    }
    if (one != other) {
      return one < other ? -1 : 1;
    }
  }
  return 0;
}

struct Comparator {
  std::string op;
  Version version;
};

/// `^(>=|<=|>|<|\^|~|=)?\s*(.+)$` — the operator, then whatever is left, parsed as a version.
std::optional<Comparator> parse_comparator(const std::string& token) {
  const std::string trimmed = text::strip(token);
  std::size_t at = 0;
  std::string op;
  if (text::starts_with(trimmed, ">=") || text::starts_with(trimmed, "<=")) {
    op = trimmed.substr(0, 2);
    at = 2;
  } else if (!trimmed.empty() && (trimmed[0] == '>' || trimmed[0] == '<' || trimmed[0] == '^' ||
                                  trimmed[0] == '~' || trimmed[0] == '=')) {
    op = trimmed.substr(0, 1);
    at = 1;
  }
  while (at < trimmed.size() && (trimmed[at] == ' ' || trimmed[at] == '\t' || trimmed[at] == '\n' ||
                                 trimmed[at] == '\r' || trimmed[at] == '\f' || trimmed[at] == '\v')) {
    ++at;
  }
  const std::string rest = trimmed.substr(at);
  if (rest.empty()) {
    return std::nullopt;  // `(.+)` requires at least one character.
  }
  const std::optional<Version> version = parse(rest);
  if (!version.has_value()) {
    return std::nullopt;
  }
  Comparator out;
  out.op = op.empty() ? "=" : op;
  out.version = *version;
  return out;
}

bool satisfies_comparator(const Version& target, const Comparator& comparator) {
  const int ordering = compare(target, comparator.version);
  const std::string& op = comparator.op;
  if (op == ">=") return ordering >= 0;
  if (op == ">") return ordering > 0;
  if (op == "<=") return ordering <= 0;
  if (op == "<") return ordering < 0;
  if (op == "=") return ordering == 0;
  if (op == "^") {
    // Caret: compatible within the leftmost non-zero component.
    if (ordering < 0) {
      return false;
    }
    const Version& base = comparator.version;
    if (base.major > 0) {
      return target.major == base.major;
    }
    if (base.minor > 0) {
      return target.major == 0 && target.minor == base.minor;
    }
    return target.major == 0 && target.minor == 0 && target.patch == base.patch;
  }
  if (op == "~") {
    // Tilde: patch-level changes within the same minor.
    if (ordering < 0) {
      return false;
    }
    return target.major == comparator.version.major && target.minor == comparator.version.minor;
  }
  return false;
}

/// Every substring matching
/// `(?:>=|<=|>|<|\^|~|=)?\s*\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?`, in order.
///
/// Tokenised on the operator rather than on whitespace, so `">= 1.2.0"` and `">=1.2.0"` both parse.
/// A range written either way means the same thing to the person who wrote it.
std::vector<std::string> find_tokens(std::string_view group) {
  std::vector<std::string> tokens;
  std::size_t at = 0;
  while (at < group.size()) {
    std::size_t cursor = at;

    // The optional operator, longest alternative first — the same order the pattern lists them in.
    if (group.compare(cursor, 2, ">=") == 0 || group.compare(cursor, 2, "<=") == 0) {
      cursor += 2;
    } else if (cursor < group.size() &&
               (group[cursor] == '>' || group[cursor] == '<' || group[cursor] == '^' ||
                group[cursor] == '~' || group[cursor] == '=')) {
      cursor += 1;
    }
    while (cursor < group.size() && (group[cursor] == ' ' || group[cursor] == '\t' ||
                                     group[cursor] == '\n' || group[cursor] == '\r' ||
                                     group[cursor] == '\f' || group[cursor] == '\v')) {
      ++cursor;
    }

    const auto digits = [&](std::size_t& position) {
      const std::size_t start = position;
      while (position < group.size() && text::is_ascii_digit(group[position])) {
        ++position;
      }
      return position > start;
    };

    std::size_t probe = cursor;
    const bool numeric = digits(probe) && probe < group.size() && group[probe] == '.' &&
                         (++probe, digits(probe)) && probe < group.size() && group[probe] == '.' &&
                         (++probe, digits(probe));
    if (!numeric) {
      ++at;  // No match at this position; the engine advances one character and tries again.
      continue;
    }
    if (probe < group.size() && group[probe] == '-') {
      std::size_t prerelease = probe + 1;
      while (prerelease < group.size() && is_prerelease_char(group[prerelease])) {
        ++prerelease;
      }
      if (prerelease > probe + 1) {
        probe = prerelease;
      }
    }
    tokens.emplace_back(group.substr(at, probe - at));
    at = probe;
  }
  return tokens;
}

/// A range, parsed into its `||` groups.
///
/// A group that does not parse is an empty optional rather than an error for the whole range: one
/// unreadable alternative does not invalidate the readable ones. `is_valid_range` is the caller
/// that wants the stricter answer and asks for it by checking every group.
///
/// An empty outer optional means the range is a wildcard — every version satisfies it and there is
/// nothing to compare against.
std::optional<std::vector<std::optional<std::vector<Comparator>>>> parse_range(
    const std::string& range_text) {
  const std::string trimmed = text::strip(range_text);
  if (trimmed.empty() || trimmed == "*" || text::ascii_lower(trimmed) == "any") {
    return std::nullopt;
  }

  std::vector<std::optional<std::vector<Comparator>>> groups;
  std::size_t start = 0;
  for (;;) {
    const std::size_t separator = trimmed.find("||", start);
    const std::string group =
        trimmed.substr(start, separator == std::string::npos ? std::string::npos : separator - start);

    const std::vector<std::string> tokens = find_tokens(text::strip(group));
    if (tokens.empty()) {
      groups.emplace_back(std::nullopt);
    } else {
      std::vector<Comparator> comparators;
      bool readable = true;
      for (const std::string& token : tokens) {
        const std::optional<Comparator> comparator = parse_comparator(token);
        if (!comparator.has_value()) {
          readable = false;
          break;
        }
        comparators.push_back(*comparator);
      }
      if (readable) {
        groups.emplace_back(std::move(comparators));
      } else {
        groups.emplace_back(std::nullopt);
      }
    }

    if (separator == std::string::npos) {
      break;
    }
    start = separator + 2;
  }
  return groups;
}

}  // namespace

std::optional<Version> parse(const std::string& text) {
  // `^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$`
  const std::string trimmed = text::strip(text);
  std::size_t at = 0;
  const auto digits = [&]() -> std::optional<std::string> {
    const std::size_t start = at;
    while (at < trimmed.size() && text::is_ascii_digit(trimmed[at])) {
      ++at;
    }
    if (at == start) {
      return std::nullopt;
    }
    return trimmed.substr(start, at - start);
  };

  const std::optional<std::string> major = digits();
  if (!major.has_value() || at >= trimmed.size() || trimmed[at] != '.') {
    return std::nullopt;
  }
  ++at;
  const std::optional<std::string> minor = digits();
  if (!minor.has_value() || at >= trimmed.size() || trimmed[at] != '.') {
    return std::nullopt;
  }
  ++at;
  const std::optional<std::string> patch = digits();
  if (!patch.has_value()) {
    return std::nullopt;
  }

  std::optional<std::string> prerelease;
  if (at < trimmed.size()) {
    if (trimmed[at] != '-') {
      return std::nullopt;
    }
    const std::string tail = trimmed.substr(at + 1);
    if (tail.empty() || !std::all_of(tail.begin(), tail.end(), is_prerelease_char)) {
      return std::nullopt;
    }
    prerelease = tail;
    at = trimmed.size();
  }

  Version out;
  out.major = component(*major);
  out.minor = component(*minor);
  out.patch = component(*patch);
  out.prerelease = prerelease;
  out.raw = trimmed;
  return out;
}

bool is_valid_version(const std::string& text) { return parse(text).has_value(); }

int compare(const Version& left, const Version& right) {
  if (left.major != right.major) {
    return left.major < right.major ? -1 : 1;
  }
  if (left.minor != right.minor) {
    return left.minor < right.minor ? -1 : 1;
  }
  if (left.patch != right.patch) {
    return left.patch < right.patch ? -1 : 1;
  }
  return compare_prerelease(left.prerelease, right.prerelease);
}

int compare_versions(const std::string& left, const std::string& right) {
  const std::optional<Version> one = parse(left);
  const std::optional<Version> other = parse(right);
  // An unparseable version compares equal to everything, which is the reference edition's
  // behaviour and deliberate: this function has no way to report an error and a caller sorting a
  // list must not have one element throw. Callers that need to know ask `is_valid_version`.
  if (!one.has_value() || !other.has_value()) {
    return 0;
  }
  return compare(*one, *other);
}

bool satisfies(const std::string& version, const std::string& range_text) {
  const std::optional<Version> target = parse(version);
  if (!target.has_value()) {
    return false;
  }
  const auto groups = parse_range(range_text);
  if (!groups.has_value()) {
    return true;
  }
  for (const auto& group : *groups) {
    if (!group.has_value()) {
      continue;
    }
    const bool all_hold = std::all_of(group->begin(), group->end(), [&](const Comparator& one) {
      return satisfies_comparator(*target, one);
    });
    if (all_hold) {
      return true;
    }
  }
  return false;
}

bool is_valid_range(const std::string& range_text) {
  const auto groups = parse_range(range_text);
  if (!groups.has_value()) {
    return true;
  }
  return std::all_of(groups->begin(), groups->end(),
                     [](const auto& group) { return group.has_value(); });
}

}  // namespace skynet::semver

namespace skynet::contracts {
namespace {

Json version_to_json(const std::optional<semver::Version>& parsed) {
  if (!parsed.has_value()) {
    return Json::null();
  }
  Json out = Json::object();
  out.set("major", Json::number(parsed->major));
  out.set("minor", Json::number(parsed->minor));
  out.set("patch", Json::number(parsed->patch));
  out.set("prerelease", parsed->prerelease.has_value() ? Json::string(*parsed->prerelease)
                                                       : Json::null());
  out.set("raw", Json::string(parsed->raw));
  return out;
}

}  // namespace

Json semver(const std::string& label, const Json& input) {
  if (label == "compare") {
    const std::string left(input["a"].string_or(""));
    const std::string right(input["b"].string_or(""));
    Json out = Json::object();
    out.set("compare", Json::number(semver::compare_versions(left, right)));
    out.set("a", version_to_json(semver::parse(left)));
    out.set("b", version_to_json(semver::parse(right)));
    return out;
  }
  if (label == "satisfies") {
    const std::string version(input["version"].string_or(""));
    const std::string range_text(input["range"].string_or(""));
    Json out = Json::object();
    out.set("version", Json::string(version));
    out.set("range", Json::string(range_text));
    out.set("satisfies", Json::boolean(semver::satisfies(version, range_text)));
    out.set("valid_version", Json::boolean(semver::is_valid_version(version)));
    out.set("valid_range", Json::boolean(semver::is_valid_range(range_text)));
    return out;
  }
  throw Unimplemented("semver", label);
}

}  // namespace skynet::contracts
