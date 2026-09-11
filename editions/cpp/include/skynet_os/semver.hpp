// Semantic versions, shared. See `src/contracts/semver.cpp` for the reasoning.
//
// Declared in a header of its own rather than inside the contract, because admission (§8, the
// os-too-old check) and preconditions (§54.2, `semver_satisfies`) both need it, and two version
// comparators in one edition is the drift Law III forbids.

#ifndef SKYNET_OS_SEMVER_HPP
#define SKYNET_OS_SEMVER_HPP

#include <optional>
#include <string>

namespace skynet::semver {

/// A version that parsed. Components are doubles rather than integers on purpose — see
/// `component()` in the implementation.
struct Version {
  double major = 0;
  double minor = 0;
  double patch = 0;
  std::optional<std::string> prerelease;
  std::string raw;
};

[[nodiscard]] std::optional<Version> parse(const std::string& text);
[[nodiscard]] bool is_valid_version(const std::string& text);

/// -1 if left orders before right, 0 if equal, 1 if after.
[[nodiscard]] int compare(const Version& left, const Version& right);
/// The same, over strings. An unparseable version compares equal to everything.
[[nodiscard]] int compare_versions(const std::string& left, const std::string& right);

/// Does `version` fall inside `range_text`? Supports `*`, space-separated AND comparators
/// (`>=1.2.0 <2.0.0`) and `||` OR groups.
[[nodiscard]] bool satisfies(const std::string& version, const std::string& range_text);

/// Is `range_text` a range this edition can read at all? Distinct from `satisfies` returning
/// false — see the contract's header comment.
[[nodiscard]] bool is_valid_range(const std::string& range_text);

}  // namespace skynet::semver

#endif  // SKYNET_OS_SEMVER_HPP
