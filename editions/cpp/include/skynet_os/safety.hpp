// The Safety Kernel, shared. See `src/contracts/safety.cpp` for the reasoning.
//
// Separated from the contract because the resolved limit set is what §17's `GET /limits` publishes
// and what admission reads, and a second composition of the same ceilings would be exactly the
// drift taking the minimum exists to prevent.

#ifndef SKYNET_OS_SAFETY_HPP
#define SKYNET_OS_SAFETY_HPP

#include "skynet_os/json.hpp"
#include "skynet_os/kinematics.hpp"

#include <optional>
#include <string>
#include <vector>

namespace skynet::safety {

struct GuardUnavailable {
  std::string guard;
  std::vector<std::string> missing;
};

/// A guard that could run and has nothing to run on — §80's distinction from an unavailable one.
struct GuardInert {
  std::string guard;
  std::string reading;
};

struct Params {
  std::string tier;
  const Json* envelope = nullptr;
  const Json* transport = nullptr;
  const Json* profile = nullptr;
  const Json* session = nullptr;
  const Json* available = nullptr;
  /// The **flattened** frame list (§44.3), not the descriptor's graph.
  const std::vector<kinematics::Frame>* frames = nullptr;
};

/// Every source of ceilings, composed into the single set the kernel enforces.
///
/// Three sources, and the rule between them is always the minimum: the robot's declared envelope,
/// the transport plan's per-channel limits, and the operator's profile — plus §38.2's session
/// override. Taking the minimum rather than a precedence order means no source can raise a limit
/// another source lowered, including a malicious or mistaken operator profile, which is why the
/// profile is folded in here rather than applied afterwards.
struct Limits {
  std::string tier;
  std::string enforcement;
  std::optional<double> joint_velocity_max_rad_s;
  std::optional<double> joint_torque_max_nm;
  std::optional<double> base_velocity_max_m_s;
  Json min_control_hz = Json::null();
  std::optional<double> max_delta_per_cycle;
  Json per_channel = Json::object();
  /// §86.3's map: the mode each bound joint is commanded in, from the transport plan. Without it
  /// the two whole-body figures above could not be applied to a joint at all — nothing at this
  /// layer knew whether a channel carried radians per second or newton-metres.
  Json joint_modes = Json::object();
  /// §7.1, F159: the travel in force for each joint, `[min, max]` — the plan's intersected with
  /// the profile's. Held only on a channel commanded in position.
  Json travel = Json::object();
  Json proximity = Json::null();
  Json workspace = Json::array();
  Json self_collision_margin_m = Json::null();
  /// The capability grants the mode gate checks against, attached after resolution.
  Json grants = Json::array();
  const std::vector<kinematics::Frame>* frames = nullptr;
  std::vector<std::string> active_guards;
  std::vector<GuardUnavailable> unavailable_guards;
  std::vector<GuardInert> inert_guards;
  Json source = Json::null();
  /// What kind of figures these are (§5.3). Follows the provenance: the envelope's basis when
  /// the source is the envelope's, null when it fell through to the transport plan's.
  Json basis = Json::null();
  std::vector<Json> notices;

  [[nodiscard]] bool guard_active(std::string_view guard) const;
};

struct Clamp {
  std::string channel;
  double from = 0;
  double to = 0;
};

struct PoseTrip {
  std::string guard;
  std::string frame;
  std::optional<std::string> other;
  std::optional<double> margin_m;
};

struct Pose {
  bool evaluated = false;
  bool lookahead = false;
  bool truncated = false;
  std::vector<std::string> unresolved;
  std::vector<PoseTrip> trips;
};

struct Verdict {
  Json command = Json::object();
  /// Empty when no guard demanded anything — rendered as `null`.
  std::string response;
  std::vector<std::string> tripped;
  std::vector<Clamp> clamped;
  bool advisory = false;
  Pose pose;
};

/// Derived from the tier, never supplied. A caller cannot pass in the mode it would prefer.
[[nodiscard]] std::string resolve_enforcement(const std::string& tier);
/// Whether the tier has a stop path independent of the application processor.
[[nodiscard]] bool independent_stop_path(const std::string& tier);

[[nodiscard]] Limits resolve_limits(const Params& params);
/// Fills `active_guards`, `unavailable_guards` and `inert_guards`. Called by `resolve_limits`;
/// exposed so the order of the three lists has one definition.
void partition_guards(const Params& params, Limits& limits);

/// Evaluate one control cycle. The reference implementation of the hot path.
[[nodiscard]] Verdict evaluate_cycle(const Json& cycle, const Limits& limits, const Json* envelope,
                                     const Json* last_sequence,
                                     const std::vector<kinematics::Binding>& bindings);

}  // namespace skynet::safety

#endif  // SKYNET_OS_SAFETY_HPP
