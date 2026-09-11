// Observation assembly, shared. See `src/contracts/observation.cpp` for the reasoning.

#ifndef SKYNET_OS_OBSERVATION_HPP
#define SKYNET_OS_OBSERVATION_HPP

#include "skynet_os/json.hpp"

#include <optional>
#include <string>
#include <vector>

namespace skynet::observation {

/// One assembled observation frame (§25.1), with §26's record projection alongside it.
struct Frame {
  double target_ns = 0;
  Json streams = Json::array();
  std::optional<double> skew_ms;
  std::optional<double> skew_upper_ms;
  std::vector<std::string> undeclared_latency;
  std::vector<std::string> undeclared_budget;
  Json holds = Json::array();
  std::string verdict;
  bool deliverable = true;
  Json record = Json::object();
};

/// Assemble one observation frame for the given instant.
[[nodiscard]] Frame assemble(double target_ns, const Json& streams);

/// The most recent instant at which every stream has a sample.
[[nodiscard]] std::optional<double> latest_common_instant(double now_ns, const Json& streams);

}  // namespace skynet::observation

#endif  // SKYNET_OS_OBSERVATION_HPP
