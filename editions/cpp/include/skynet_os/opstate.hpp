// The operating state machine (§27), shared. See `src/contracts/opstate.cpp` for the reasoning.
//
// Exposed because §17's check-in reports the state, §49.1's warning tone is a function of the
// transition, and §33.4's reconcile barrier opens only in a safe state — three consumers of one
// machine, which is one machine rather than three.

#ifndef SKYNET_OS_OPSTATE_HPP
#define SKYNET_OS_OPSTATE_HPP

#include "skynet_os/json.hpp"

#include <string>
#include <string_view>

namespace skynet::opstate {

struct Transition {
  std::string from;
  std::string to;
  std::string reason;
  bool changed = false;
  bool warning = false;
};

/// Whether a state is one §33.4's barrier may open at: nothing held, nothing mid-task.
[[nodiscard]] bool is_safe_state(std::string_view state);
/// Whether a policy is commanding the body in this state.
[[nodiscard]] bool commands_autonomously(std::string_view state);
/// Whether a state requires an explicit human action to leave (§27). Exactly one does.
[[nodiscard]] bool is_sticky(std::string_view state);
/// Whether a warning tone must sound on this transition (§49.1).
[[nodiscard]] bool warning_required(std::string_view from_state, std::string_view to_state);

/// The state this robot is in, given where it was and what is true now. Pure, clockless and total.
[[nodiscard]] Transition evaluate(const std::string& previous, const Json& conditions);

}  // namespace skynet::opstate

#endif  // SKYNET_OS_OPSTATE_HPP
