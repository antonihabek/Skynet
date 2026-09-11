// Declarative predicate evaluation, shared. See `src/contracts/predicate.cpp` for the reasoning.
//
// Declared separately from the contract because §54.2's preconditions, §8's admission rules and
// the compatibility engine all evaluate the same predicates, and a second evaluator is the drift
// the corpus exists to catch.

#ifndef SKYNET_OS_PREDICATE_HPP
#define SKYNET_OS_PREDICATE_HPP

#include "skynet_os/json.hpp"

#include <optional>
#include <string>

namespace skynet::predicate {

/// Why a predicate decided, or could not.
struct Outcome {
  /// `satisfied`, `unsatisfied` or `unevaluable`.
  std::string outcome;
  /// One of §54.2's closed `unevaluable_reasons`, never free text.
  std::optional<std::string> reason;
  /// The path a refusal names — the whole point of the third outcome.
  std::optional<std::string> path;
  std::optional<std::string> op;
};

/// Read `a.b.c` out of a context document. Returns nullptr for any missing link, which is how
/// "this path named nothing" stays distinguishable from a member whose value is `null`.
[[nodiscard]] const Json* read_path(const Json& context, const std::string& path);

/// The evaluator. Everything else here is a view of it.
[[nodiscard]] Outcome evaluate_outcome(const Json& node, const Json& context);

/// The compatibility engine's answer. Derived from the outcome, never reimplemented.
[[nodiscard]] bool evaluate(const Json& node, const Json& context);

/// Every path the predicate tree mentions, with what it read, so a finding can show *why*.
[[nodiscard]] Json detail(const Json& node, const Json& context);

}  // namespace skynet::predicate

#endif  // SKYNET_OS_PREDICATE_HPP
