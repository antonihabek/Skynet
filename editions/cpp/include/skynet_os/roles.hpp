// The canonical role vocabulary (§5), shared. See `src/contracts/roles.cpp` for the reasoning.
//
// Exposed as a module rather than kept inside its corpus handler because §6's transport planner is
// the real consumer: roles are how a policy trained on one body reaches another, and a second role
// parser in the same edition would be two answers to "are these the same joint".

#ifndef SKYNET_OS_ROLES_HPP
#define SKYNET_OS_ROLES_HPP

#include "skynet_os/json.hpp"

#include <optional>
#include <string>

namespace skynet::roles {

/// A role that parsed. Never constructed for one that did not.
struct ParsedRole {
  std::string chain;
  std::string link;
  std::optional<std::string> axis;
  std::optional<double> index;
};

/// Parse a canonical role, or nothing.
///
/// Nothing rather than an error, and the caller reads it as *"this joint can only be matched by
/// exact name"*. Three ways to fail, and all three return the same nothing: the string does not
/// match the grammar, the chain is not in the vocabulary, or the axis is not.
[[nodiscard]] std::optional<ParsedRole> parse(const Json& role_field);
/// The same, over a string.
[[nodiscard]] std::optional<ParsedRole> parse(const std::string& role);

/// The canonical string form, so two parsed roles compare as keys.
[[nodiscard]] std::string key(const ParsedRole& parsed);

}  // namespace skynet::roles

#endif  // SKYNET_OS_ROLES_HPP
