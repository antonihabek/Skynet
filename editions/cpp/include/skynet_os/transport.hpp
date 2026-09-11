// §6's transport planner, shared. See `src/contracts/transport.cpp` for the reasoning.
//
// Exposed because admission (§8) reads a plan's verdict to decide whether a package may install,
// and because §7.6's resolved limits fold in the plan's per-channel clamp.

#ifndef SKYNET_OS_TRANSPORT_HPP
#define SKYNET_OS_TRANSPORT_HPP

#include "skynet_os/json.hpp"

namespace skynet::transport {

/// Resolve how a policy's action space reaches a body. Pure: no clock, no store, every input an
/// argument, which is what lets the Hub and a disconnected robot agree about what a robot will do
/// before it does it.
[[nodiscard]] Json resolve(const Json& source, const Json& target, const Json& options);

}  // namespace skynet::transport

#endif  // SKYNET_OS_TRANSPORT_HPP
