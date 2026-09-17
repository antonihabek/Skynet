// §6's transport planner, C++ edition.
//
// Transcribed from `resolveTransport` and its helpers in `reference/typescript/embodiment.ts`.
// Every rule and message string is read from that file, never from `corpus/`.
//
// The question this answers is the one that makes a policy portable at all: a policy declares an
// action space, a body declares joints, and something has to say how the first reaches the second —
// by name, by canonical role, through task space, or not at all. It is pure: no database, no
// filesystem, no clock, every input an argument, which is what lets the Hub and a disconnected
// robot agree about what a robot will do before it does it.
//
// ── Where C++ is quietly not JavaScript, here ───────────────────────────────
//
// `Set` and `Map` iterate in insertion order
//     Two lists in a plan are built by walking a set — `unmapped` roles and the retargeted chains —
//     and both are **subjects on a warning the corpus compares element by element**. An edition
//     using an ordered or hashed container would produce a correct plan with its warnings in a
//     different order. Both use insertion-ordered vectors here, and `ordered_insert` says so.
//
// `toFixed(0)`
//     ECMAScript picks the integer nearest the value and, **on a tie, the larger one**. A channel
//     scaled to exactly 62.5% reads `63%` in the reference, and a printf that rounds half to even
//     would read `62%`.
//
// strict `undefined`
//     `channel.peak !== undefined` decides whether an unscalable channel is *warned about* or
//     passed over in silence, and `source.dof !== undefined` whether a mismatch is reported at all.
//     A key present and null takes the other branch in the reference.
//
// `options.allowRetarget === true`
//     Strictly `true`. Any other truthy value — `1`, `"yes"` — does **not** grant retargeting, and
//     the warning it produces says *"was not requested"* rather than *"has no solver"*, which is a
//     different sentence to whoever reads it.

#include "skynet_os/contracts.hpp"
#include "skynet_os/embodiment.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/roles.hpp"
#include "skynet_os/transport.hpp"

#include <algorithm>
#include <cmath>
#include <optional>

namespace skynet::transport {
namespace {

/// The three channel kinds that name a joint. Everything else is matched against the body's own
/// facilities rather than against its joints.
bool is_joint_kind(std::string_view kind) {
  return kind == "joint_position" || kind == "joint_velocity" || kind == "joint_torque";
}

/// `obj?.key` — nullptr when the object is missing or the key is not present.
const Json* field(const Json& obj, std::string_view key) {
  return obj.is_object() && obj.has(key) ? &obj[key] : nullptr;
}

/// `value ?? null`.
Json or_null(const Json* value) {
  return value == nullptr || value->is_null() ? Json::null() : *value;
}

/// Append when absent. The insertion order a JavaScript `Set` iterates in, written as a function so
/// every place that needs it says why: both callers put their result into a warning's `subjects`,
/// which the corpus compares element by element.
void ordered_insert(std::vector<std::string>& into, const std::string& value) {
  if (std::find(into.begin(), into.end(), value) == into.end()) {
    into.push_back(value);
  }
}

Json warning(std::string code, std::string message) {
  Json out = Json::object();
  out.set("code", Json::string(std::move(code)));
  out.set("message", Json::string(std::move(message)));
  return out;
}

Json warning_with(std::string code, std::string message, const std::vector<std::string>& subjects) {
  Json out = warning(std::move(code), std::move(message));
  Json list = Json::array();
  for (const std::string& subject : subjects) {
    list.push(Json::string(subject));
  }
  out.set("subjects", list);
  return out;
}

/// Which joint rating bounds a channel of this kind.
Json rating_for(std::string_view kind, const Json& limits) {
  if (kind == "joint_velocity") {
    return or_null(field(limits, "velocity_max_rad_s"));
  }
  if (kind == "joint_torque") {
    return or_null(field(limits, "effort_max_nm"));
  }
  if (kind == "joint_position") {
    const Json* high = field(limits, "position_max_rad");
    const Json* low = field(limits, "position_min_rad");
    if (high == nullptr || low == nullptr) {
      return Json::null();
    }
    // Position is bounded by travel rather than by a single magnitude; the ceiling handed to the
    // kernel is the larger absolute bound, and the kernel holds both ends.
    return Json::number(std::max(std::fabs(high->number_or(0)), std::fabs(low->number_or(0))));
  }
  return Json::null();
}

std::string control_mode_for(std::string_view kind) {
  if (kind == "joint_position") return "position";
  if (kind == "joint_velocity") return "velocity";
  if (kind == "joint_torque") return "torque";
  if (kind == "cartesian") return "cartesian";
  return {};
}

/// How far a channel must be scaled to stay inside a joint's rating.
///
/// 1 when either number is missing, and a warning is raised separately. Guessing a ratio from one
/// known side would be inventing the other.
double scale_for(const Json* peak, const Json& rating) {
  if (peak == nullptr || rating.is_null()) {
    return 1;
  }
  const double peak_value = peak->number_or(0);
  if (peak_value <= 0) {
    return 1;
  }
  return std::min(1.0, rating.number_or(0) / peak_value);
}

struct TargetIndex {
  std::vector<std::pair<std::string, const Json*>> by_name;
  std::vector<std::pair<std::string, const Json*>> by_role;
  std::vector<std::string> chains;
  std::vector<std::string> role_keys;

  [[nodiscard]] const Json* named(const std::string& name) const {
    for (const auto& entry : by_name) {
      if (entry.first == name) {
        return entry.second;
      }
    }
    return nullptr;
  }
  [[nodiscard]] const Json* roled(const std::string& key) const {
    for (const auto& entry : by_role) {
      if (entry.first == key) {
        return entry.second;
      }
    }
    return nullptr;
  }
};

TargetIndex index_target(const Json& target) {
  TargetIndex index;
  for (const Json& joint : target["joints"].items()) {
    index.by_name.emplace_back(std::string(joint["name"].string_or("")), &joint);
    const std::optional<roles::ParsedRole> parsed = roles::parse(joint["role"]);
    if (!parsed.has_value()) {
      continue;
    }
    const std::string key = roles::key(*parsed);
    // First declaration wins. A body declaring the same role twice is a descriptor bug, and
    // silently preferring the later one would make the mapping depend on array order.
    if (index.roled(key) == nullptr) {
      index.by_role.emplace_back(key, &joint);
    }
    ordered_insert(index.chains, parsed->chain);
    ordered_insert(index.role_keys, key);
  }
  return index;
}

struct Binding {
  Json source_index = Json::null();
  Json source_role = Json::null();
  std::string method;
  Json target = Json::null();
  double scale = 1;
  Json limit = Json::null();
  std::string reason;
};

Binding simple(const Json& channel, std::string method, std::string reason, double scale) {
  Binding out;
  out.source_index = channel["index"];
  out.source_role = or_null(field(channel, "role"));
  out.method = std::move(method);
  out.scale = scale;
  out.reason = std::move(reason);
  return out;
}

/// The plan's verdict is the worst binding in it.
///
/// One blocked required channel refuses the whole plan, because a policy missing an actuator it
/// depends on does not do most of its job — it does something nobody has characterised.
std::string derive_verdict(const std::vector<Binding>& bindings) {
  for (const Binding& one : bindings) {
    if (one.method == "blocked") {
      return "refused";
    }
  }
  for (const Binding& one : bindings) {
    if (one.method == "drop") {
      return "degraded";
    }
  }
  for (const Binding& one : bindings) {
    if (one.method == "retarget" || one.method == "synthesize") {
      return "retargeted";
    }
  }
  return "direct";
}

Binding finish_joint_binding(const Json& channel, const Json& joint, const std::string& method,
                             const std::string& reason, std::vector<Json>& warnings) {
  const Json rating = rating_for(channel["kind"].string_or(""), joint["limits"]);
  const Json* peak = field(channel, "peak");
  const double scale = scale_for(peak, rating);
  const std::string mode = control_mode_for(channel["kind"].string_or(""));
  const std::string joint_name(joint["name"].string_or(""));

  if (!mode.empty()) {
    bool accepted = false;
    for (const Json& available : joint["control_modes"].items()) {
      if (available.string_or("") == mode) {
        accepted = true;
        break;
      }
    }
    if (!accepted) {
      Binding out = simple(channel, "blocked",
                           "\"" + joint_name + "\" does not accept " + mode + " commands.", 1);
      out.target = Json::string(joint_name);
      out.limit = rating;
      return out;
    }
  }

  if (scale < 1) {
    warnings.push_back(warning_with(
        "scaled_down",
        "Commands to \"" + joint_name + "\" are scaled to " + js::to_fixed(scale * 100, 0) +
            "% because the policy was trained on a stronger joint.",
        {joint_name}));
  } else if (peak != nullptr && rating.is_null()) {
    warnings.push_back(warning_with(
        "unscalable_channel",
        "\"" + joint_name +
            "\" declares no rating for this control mode, so the policy's commands cannot be "
            "scaled to it.",
        {joint_name}));
  }

  Binding out = simple(channel, method, reason, scale);
  out.target = Json::string(joint_name);
  out.limit = rating;
  return out;
}

Binding bind_non_joint_channel(const Json& channel, const Json& target) {
  const std::string kind(channel["kind"].string_or(""));
  const bool optional = field(channel, "optional") != nullptr && channel["optional"].is_true();

  if (kind == "gripper") {
    const Json* effector = nullptr;
    for (const Json& one : target["end_effectors"].items()) {
      const std::string_view effector_kind = one["kind"].string_or("");
      if (effector_kind == "gripper" || effector_kind == "dexterous_hand") {
        effector = &one;
        break;
      }
    }
    if (effector == nullptr) {
      if (optional) {
        return simple(channel, "drop", "This body has no gripper; the channel is optional.", 0);
      }
      return simple(channel, "blocked", "This body has no gripper and the policy requires one.", 1);
    }
    const std::string name(( *effector)["name"].string_or(""));
    Binding out = simple(channel, "remap",
                         "Driven through the " + std::string((*effector)["kind"].string_or("")) +
                             " \"" + name + "\".",
                         1);
    out.target = Json::string(name);
    out.limit = or_null(field(*effector, "max_grip_force_n"));
    return out;
  }

  if (kind == "base_velocity") {
    const std::string base(target["base"].string_or(""));
    if (base == "fixed") {
      if (optional) {
        return simple(channel, "drop", "This body has a fixed base; the channel is optional.", 0);
      }
      return simple(channel, "blocked",
                    "The policy commands base motion and this body has a fixed base.", 1);
    }
    const Json* envelope = field(target, "envelope");
    const Json limit = envelope == nullptr || envelope->is_null()
                           ? Json::null()
                           : or_null(field(*envelope, "base_velocity_max_m_s"));
    // A wheeled base driven by a policy trained on a legged one is a synthesis, not a remap: the
    // command means the same thing, and nothing below the base interface is comparable.
    Binding out = simple(channel, base == "legged" ? "remap" : "synthesize",
                         "Base velocity applied to this " + base + " base.",
                         scale_for(field(channel, "peak"), limit));
    out.target = Json::string("base");
    out.limit = limit;
    return out;
  }

  if (kind == "cartesian") {
    const Json* effector = nullptr;
    for (const Json& one : target["end_effectors"].items()) {
      if (one["kind"].string_or("") != "none") {
        effector = &one;
        break;
      }
    }
    if (effector == nullptr || target["solvers"].items().empty()) {
      if (optional) {
        return simple(
            channel, "drop",
            "End-effector pose control is unavailable on this body; the channel is optional.", 0);
      }
      return simple(channel, "blocked",
                    "End-effector pose control needs an end effector and a solver; this body has "
                    "neither.",
                    1);
    }
    const std::string name((*effector)["name"].string_or(""));
    Binding out =
        simple(channel, "retarget",
               "Solved onto \"" + name + "\" using " +
                   std::string(target["solvers"].items().front().string_or("")) + ".",
               1);
    out.target = Json::string(name);
    return out;
  }

  // `discrete` — a mode switch or a named action. It carries no physical units, so nothing here can
  // bound it; the package's capability grants are what gate it.
  return simple(channel, "direct",
                "Discrete command passed through; it commands no actuator directly.", 1);
}

/// Which chains must go through task space, and whether this body can actually do that.
///
/// A chain qualifies when at least one of its joint channels has no direct or role match. It is
/// only *granted* when the caller asked for retargeting and the body has a solver — otherwise the
/// chain's channels fall through to `blocked`, which is the honest outcome. A plan that promised
/// retargeting on a robot with no IK solver would fail at the first command.
std::vector<std::string> decide_retarget_chains(const Json& source, const TargetIndex& index,
                                                const Json& target, const Json& options,
                                                std::vector<Json>& warnings) {
  std::vector<std::string> needs;

  for (const Json& channel : source["action_channels"].items()) {
    if (!is_joint_kind(channel["kind"].string_or(""))) {
      continue;
    }
    const Json* joint_name = field(channel, "joint");
    if (joint_name != nullptr && joint_name->is_string() && !joint_name->string_or("").empty() &&
        index.named(std::string(joint_name->string_or(""))) != nullptr) {
      continue;
    }
    const std::optional<roles::ParsedRole> parsed = roles::parse(channel["role"]);
    if (!parsed.has_value()) {
      continue;
    }
    if (index.roled(roles::key(*parsed)) != nullptr) {
      continue;
    }
    // The chain exists on this body but this particular joint does not — a six-joint arm asked to
    // run a seven-joint policy. That is exactly the case task space can rescue.
    if (std::find(index.chains.begin(), index.chains.end(), parsed->chain) != index.chains.end()) {
      ordered_insert(needs, parsed->chain);
    }
  }

  if (needs.empty()) {
    return needs;
  }

  // Strictly `true`. A truthy-but-not-true value does not grant retargeting, and the warning it
  // produces names the wrong cause if this is read loosely.
  const Json* allow = field(options, "allowRetarget");
  const bool asked = allow != nullptr && allow->is_true();
  const bool truthy = allow != nullptr && !allow->is_null() && !(allow->is_bool() && !allow->is_true()) &&
                      !(allow->is_string() && allow->string_or("").empty()) &&
                      !(allow->is_number() && allow->number_or(0) == 0);
  if (!asked || target["solvers"].items().empty()) {
    warnings.push_back(warning_with(
        "retarget_without_solver",
        truthy ? "Task-space retargeting is required but this body has no inverse-kinematics "
                 "solver installed."
               : "Task-space retargeting would be required, and was not requested.",
        needs));
    return {};
  }

  return needs;
}

Binding bind_channel(const Json& channel, const Json& target, const TargetIndex& index,
                     const std::vector<std::string>& retarget_chains,
                     std::vector<Json>& warnings) {
  const std::optional<roles::ParsedRole> parsed = roles::parse(channel["role"]);

  // Non-joint channels are matched against the body's own facilities rather than its joints.
  if (!is_joint_kind(channel["kind"].string_or(""))) {
    return bind_non_joint_channel(channel, target);
  }

  // 1. Exact wire name. Two vendors agreeing on a joint name is rare and unambiguous when it
  //    happens, so it outranks role matching.
  const Json* joint_name = field(channel, "joint");
  if (joint_name != nullptr && joint_name->is_string() && !joint_name->string_or("").empty()) {
    if (const Json* exact = index.named(std::string(joint_name->string_or(""))); exact != nullptr) {
      return finish_joint_binding(channel, *exact, "direct", "Joint name matches exactly.",
                                  warnings);
    }
  }

  // 2. Canonical role. This is the mechanism that makes bodies interoperable at all.
  if (parsed.has_value()) {
    const std::string key = roles::key(*parsed);
    if (const Json* by_role = index.roled(key); by_role != nullptr) {
      return finish_joint_binding(
          channel, *by_role, "remap",
          "Mapped by role " + key + " to \"" + std::string((*by_role)["name"].string_or("")) + "\".",
          warnings);
    }

    // 3. Task space, when the chain was granted it above.
    if (std::find(retarget_chains.begin(), retarget_chains.end(), parsed->chain) !=
        retarget_chains.end()) {
      return simple(channel, "retarget",
                    "Driven through task space on the " + parsed->chain +
                        " chain; this body has no " + key + ".",
                    1);
    }
  }

  // 4. Optional channels may be dropped; required ones are a refusal.
  if (field(channel, "optional") != nullptr && channel["optional"].is_true()) {
    return simple(channel, "drop",
                  "No equivalent on this body, and the policy declares the channel optional.", 0);
  }

  return simple(channel, "blocked",
                parsed.has_value()
                    ? "This body has no " + roles::key(*parsed) + " and the policy requires it."
                    : "The channel declares no canonical role, so it can only match a joint of "
                      "the same name, and none exists.",
                1);
}

/// `value ?? 0` — a sensor that declares no width is treated as zero-wide, not as a match.
double zero_default(const Json* value) {
  return value == nullptr || value->is_null() ? 0 : value->number_or(0);
}

const Json* match_sensor_by_role(const Json& requirement, const Json& target) {
  const std::optional<roles::ParsedRole> parsed = roles::parse(requirement["role"]);
  if (!parsed.has_value()) {
    return nullptr;
  }
  const std::string key = roles::key(*parsed);
  for (const Json& sensor : target["sensors"].items()) {
    if (sensor["modality"].string_or("") != requirement["modality"].string_or("")) {
      continue;
    }
    const std::optional<roles::ParsedRole> sensor_role = roles::parse(sensor["role"]);
    if (sensor_role.has_value() && roles::key(*sensor_role) == key) {
      return &sensor;
    }
  }
  return nullptr;
}

Json bind_observations(const Json& required, const Json& target, std::vector<Json>& warnings) {
  Json out = Json::array();
  for (const Json& requirement : required.items()) {
    const std::string name(requirement["name"].string_or(""));
    const std::string modality(requirement["modality"].string_or(""));
    const bool optional =
        field(requirement, "optional") != nullptr && requirement["optional"].is_true();

    const Json* exact = nullptr;
    for (const Json& sensor : target["sensors"].items()) {
      if (sensor["name"].string_or("") == name && sensor["modality"].string_or("") == modality) {
        exact = &sensor;
        break;
      }
    }
    const Json* candidate = exact != nullptr ? exact : match_sensor_by_role(requirement, target);

    Json entry = Json::object();
    entry.set("required", Json::string(name));

    if (candidate == nullptr) {
      if (!optional) {
        warnings.push_back(warning_with("observation_missing",
                                        "The policy reads a " + modality + " stream called \"" +
                                            name + "\" and this body has none.",
                                        {name}));
      }
      entry.set("method", Json::string("missing"));
      entry.set("target", Json::null());
      entry.set("reason",
                Json::string(optional ? "Not present on this body; the policy declares it optional."
                                      : "Not present on this body."));
      out.push(entry);
      continue;
    }

    const std::string candidate_name((*candidate)["name"].string_or(""));
    const auto below = [&](const char* key) {
      const Json* wanted = field(requirement, key);
      return wanted != nullptr && zero_default(field(*candidate, key)) < wanted->number_or(0);
    };
    if (below("width") || below("height") || below("fps")) {
      warnings.push_back(warning_with("observation_degraded",
                                      "\"" + candidate_name +
                                          "\" is lower resolution or slower than the stream the "
                                          "policy was trained on.",
                                      {candidate_name}));
      entry.set("method", Json::string("downscale"));
      entry.set("target", Json::string(candidate_name));
      entry.set("reason",
                Json::string("Present but below the trained resolution or frame rate; frames are "
                             "upscaled."));
      out.push(entry);
      continue;
    }

    entry.set("method", Json::string(exact != nullptr ? "direct" : "remap"));
    entry.set("target", Json::string(candidate_name));
    entry.set("reason", Json::string(exact != nullptr
                                         ? "Sensor name and modality match."
                                         : "Matched by role to \"" + candidate_name + "\"."));
    out.push(entry);
  }
  return out;
}

/// The rate the loop will actually run at.
///
/// Clamped down to what the body accepts, never up: running a policy faster than it was trained
/// changes its dynamics, and running it slower is a documented degradation rather than a silent
/// one. The watchdog check is the one that matters most — a loop slower than the controller's
/// timeout faults *while the robot is moving*, which is worse than refusing to start.
Json resolve_control_hz(const Json& source, const Json& target, const Json& options,
                        std::vector<Json>& warnings) {
  const Json* requested = field(options, "requestedHz");
  if (requested == nullptr || requested->is_null()) {
    requested = field(source, "control_hz");
  }
  if (requested == nullptr) {
    return Json::null();
  }

  double resolved = requested->number_or(0);
  const Json* maximum = field(target["control"], "rate_hz_max");
  if (maximum != nullptr && requested->number_or(0) > maximum->number_or(0)) {
    resolved = maximum->number_or(0);
    warnings.push_back(warning(
        "rate_unsupported",
        "The policy runs at " + js::number_to_string(requested->number_or(0)) +
            " Hz; this body accepts at most " + js::number_to_string(maximum->number_or(0)) +
            " Hz, so the loop is clamped."));
  }

  const Json* envelope = field(target, "envelope");
  const Json* watchdog_ms =
      envelope == nullptr || envelope->is_null() ? nullptr : field(*envelope, "watchdog_ms");
  if (watchdog_ms != nullptr && watchdog_ms->number_or(0) > 0) {
    const double min_hz = 1000 / watchdog_ms->number_or(0);
    if (resolved < min_hz) {
      warnings.push_back(
          warning("rate_below_watchdog",
                  "At " + js::number_to_string(resolved) + " Hz the controller's " +
                      js::number_to_string(watchdog_ms->number_or(0)) +
                      " ms watchdog fires between commands."));
    }
  }

  return Json::number(resolved);
}

Json build_clamp(const Json& target, const std::vector<Binding>& bindings) {
  const Json* envelope = field(target, "envelope");
  const bool has_envelope = envelope != nullptr && !envelope->is_null();

  Json per_channel = Json::object();
  std::size_t channels = 0;
  for (const Binding& binding : bindings) {
    if (!binding.target.is_null() && !binding.limit.is_null()) {
      per_channel.set(std::string(binding.target.string_or("")), binding.limit);
      ++channels;
    }
  }

  if (!has_envelope && channels == 0) {
    return Json::null();
  }

  Json clamp = Json::object();
  for (const char* key : {"joint_velocity_max_rad_s", "joint_torque_max_nm",
                          "base_velocity_max_m_s"}) {
    const Json* value = has_envelope ? field(*envelope, key) : nullptr;
    if (value != nullptr) {
      clamp.set(key, *value);
    }
  }
  const Json* watchdog_ms = has_envelope ? field(*envelope, "watchdog_ms") : nullptr;
  if (watchdog_ms != nullptr && watchdog_ms->number_or(0) > 0) {
    clamp.set("min_control_hz", Json::number(1000 / watchdog_ms->number_or(0)));
  }
  clamp.set("per_channel", per_channel);
  const Json* source = has_envelope ? field(*envelope, "source") : nullptr;
  if (source != nullptr) {
    clamp.set("source", *source);
  }
  return clamp;
}

// ── Transport over motions, §84.3 ────────────────────────────────────────────

/// Which negotiation a plan is. See `actionSpaceOf` in the reference for the table.
enum class Space { Joints, Motions, Ambiguous, MismatchMotions, MismatchJoints };

Space action_space_of(const Json& source, const Json& target) {
  const bool emits_motions = !source["motions"].items().empty();
  const bool emits_channels = !source["action_channels"].items().empty();
  const bool takes_motions = !target["motions"].items().empty();
  const bool takes_joints = !target["control"]["modes"].items().empty();

  if (emits_motions && emits_channels) return Space::Ambiguous;
  if (emits_motions) return takes_motions ? Space::Motions : Space::MismatchMotions;
  if (emits_channels && takes_motions && !takes_joints) return Space::MismatchJoints;
  return Space::Joints;
}

Json action_space_warning(Space space, const Json& target) {
  if (space == Space::Ambiguous) {
    return warning("action_space_ambiguous",
                   "The policy declares both joint channels and named motions. They are two "
                   "architectures, and a package that claims both has not decided which machine it "
                   "is for.");
  }
  return warning_with(
      "action_space_mismatch",
      space == Space::MismatchMotions
          ? "The policy commands named motions and this body is not commanded that way."
          : "The policy commands joint channels and this body accepts only named motions.",
      {std::string(target["robot_slug"].string_or(""))});
}

/// The body's motions with a repeated name removed, first declaration winning (§5.1's rule).
std::vector<const Json*> distinct_motions(const Json& target) {
  std::vector<const Json*> out;
  std::vector<std::string> seen;
  for (const Json& motion : target["motions"].items()) {
    const std::string name(motion["name"].string_or(""));
    if (std::find(seen.begin(), seen.end(), name) != seen.end()) {
      continue;
    }
    seen.push_back(name);
    out.push_back(&motion);
  }
  return out;
}

Json string_list(const std::vector<std::string>& values) {
  Json list = Json::array();
  for (const std::string& value : values) {
    list.push(Json::string(value));
  }
  return list;
}

std::string joined(const std::vector<std::string>& values) {
  std::string out;
  for (std::size_t at = 0; at < values.size(); ++at) {
    out += (at == 0 ? "" : ", ") + values[at];
  }
  return out;
}

Json motion_row(const std::string& source_motion, const std::string& method, Json target_name,
                const std::vector<std::string>& missing, const std::string& reason) {
  Json row = Json::object();
  row.set("source_motion", Json::string(source_motion));
  row.set("method", Json::string(method));
  row.set("target", std::move(target_name));
  row.set("missing", string_list(missing));
  row.set("reason", Json::string(reason));
  return row;
}

Json finish_motion_binding(const Json& motion, const Json& body, const std::string& method,
                           const std::string& reason, std::vector<Json>& warnings) {
  const std::string motion_name(motion["name"].string_or(""));
  const std::string body_name(body["name"].string_or(""));
  std::vector<std::string> supplied;
  for (const Json& name : motion["parameters"].items()) {
    supplied.emplace_back(name.string_or(""));
  }
  const auto supplies = [&supplied](const std::string& name) {
    return std::find(supplied.begin(), supplied.end(), name) != supplied.end();
  };

  // `required !== false`: only a boolean false releases a parameter.
  std::vector<std::string> missing;
  for (const Json& parameter : body["parameters"].items()) {
    const Json& required = parameter["required"];
    const bool released = required.is_bool() && !required.is_true();
    const std::string name(parameter["name"].string_or(""));
    if (!released && !supplies(name)) {
      missing.push_back(name);
    }
  }
  if (!missing.empty()) {
    return motion_row(motion_name, "blocked", Json::string(body_name), missing,
                      "\"" + body_name + "\" requires a parameter the policy does not supply: " +
                          joined(missing) + ".");
  }

  std::vector<std::string> unused;
  std::vector<std::string> seen;
  for (const std::string& name : supplied) {
    if (std::find(seen.begin(), seen.end(), name) != seen.end()) {
      continue;
    }
    seen.push_back(name);
    bool taken = false;
    for (const Json& parameter : body["parameters"].items()) {
      if (parameter["name"].string_or("") == name) {
        taken = true;
        break;
      }
    }
    if (!taken) {
      unused.push_back(name);
    }
  }
  if (!unused.empty()) {
    std::vector<std::string> subjects;
    for (const std::string& name : unused) {
      subjects.push_back(motion_name + "." + name);
    }
    warnings.push_back(warning_with(
        "motion_parameter_unused",
        "\"" + body_name + "\" takes no parameter named " + joined(unused) +
            ", so the policy's value reaches nothing.",
        subjects));
  }

  return motion_row(motion_name, method, Json::string(body_name), {}, reason);
}

Json bind_motion(const Json& motion, const Json& target, std::vector<Json>& warnings) {
  const std::vector<const Json*> body = distinct_motions(target);
  const std::string motion_name(motion["name"].string_or(""));

  // 1. The controller's own name.
  for (const Json* candidate : body) {
    if ((*candidate)["name"].string_or("") == motion_name) {
      return finish_motion_binding(motion, *candidate, "direct", "Motion name matches exactly.",
                                   warnings);
    }
  }

  // 2. The canonical motion, when exactly one body motion is it.
  const std::optional<std::string> canonical = embodiment::canonical_motion(motion["motion"]);
  std::vector<const Json*> same;
  if (canonical.has_value()) {
    for (const Json* candidate : body) {
      if (embodiment::canonical_motion((*candidate)["motion"]) == canonical) {
        same.push_back(candidate);
      }
    }
  }
  if (same.size() == 1) {
    return finish_motion_binding(
        motion, *same[0], "remap",
        "Mapped by motion " + *canonical + " to \"" +
            std::string((*same[0])["name"].string_or("")) + "\".",
        warnings);
  }

  // 3. Optional motions may be dropped; required ones are a refusal.
  std::string why;
  if (!canonical.has_value()) {
    why = "The motion declares no canonical motion, so it can only match a motion of the same "
          "name, and none exists";
  } else if (same.size() > 1) {
    why = "This body has " + std::to_string(same.size()) + " " + *canonical +
          " motions and none named \"" + motion_name + "\", so the vocabulary cannot choose one";
  } else {
    why = "This body has no " + *canonical + " motion";
  }

  if (motion["optional"].is_true()) {
    return motion_row(motion_name, "drop", Json::null(), {},
                      why + "; the policy declares the motion optional.");
  }
  return motion_row(motion_name, "blocked", Json::null(), {}, why + "; the policy requires it.");
}

std::string derive_motion_verdict(const Json& motions) {
  bool dropped = false;
  for (const Json& row : motions.items()) {
    if (row["method"].string_or("") == "blocked") return "refused";
    if (row["method"].string_or("") == "drop") dropped = true;
  }
  return dropped ? "degraded" : "direct";
}

/// The wire shape every path ends in, so a refused plan and a bound one cannot drift apart.
Json shape_plan(const std::string& verdict, const Json& source, const Json& target,
                const std::string& control_mode, const Json& control_hz,
                const std::vector<Binding>& bindings, const Json* motions, const Json& observations,
                const Json& clamp, const std::vector<Json>& warnings) {
  Json out = Json::object();
  out.set("verdict", Json::string(verdict));
  out.set("robot_slug", target["robot_slug"]);
  out.set("source_digest", or_null(field(source, "digest")));
  out.set("target_digest", target["digest"]);
  out.set("control_mode", Json::string(control_mode));
  out.set("control_hz", control_hz);

  Json binding_rows = Json::array();
  for (const Binding& binding : bindings) {
    Json row = Json::object();
    row.set("source_index", binding.source_index);
    row.set("source_role", binding.source_role);
    row.set("method", Json::string(binding.method));
    row.set("target", binding.target);
    row.set("scale", Json::number(binding.scale));
    row.set("limit", binding.limit);
    row.set("reason", Json::string(binding.reason));
    binding_rows.push(row);
  }
  out.set("bindings", binding_rows);
  // §84.3 — absent from a joint plan, so every plan written before motions is unchanged.
  if (motions != nullptr) {
    out.set("motions", *motions);
  }
  out.set("observations", observations);

  if (clamp.is_null()) {
    out.set("clamp", Json::null());
  } else {
    Json shaped = Json::object();
    // Each field is written only when the envelope declared it: an absent ceiling and a ceiling of
    // null are different facts, and the clamp is read by the kernel.
    for (const char* key : {"joint_velocity_max_rad_s", "joint_torque_max_nm",
                            "base_velocity_max_m_s", "min_control_hz"}) {
      if (clamp.has(key)) {
        shaped.set(key, clamp[key]);
      }
    }
    shaped.set("per_channel", clamp["per_channel"]);
    if (clamp.has("source")) {
      shaped.set("source", clamp["source"]);
    }
    out.set("clamp", shaped);
  }

  Json warning_rows = Json::array();
  for (const Json& one : warnings) {
    Json row = Json::object();
    row.set("code", one["code"]);
    row.set("message", one["message"]);
    if (one.has("subjects") && !one["subjects"].is_null()) {
      row.set("subjects", one["subjects"]);
    }
    warning_rows.push(row);
  }
  out.set("warnings", warning_rows);
  return out;
}

}  // namespace

Json resolve(const Json& source, const Json& target, const Json& options) {
  std::vector<Json> warnings;
  const TargetIndex index = index_target(target);
  std::vector<Binding> bindings;
  const Space space = action_space_of(source, target);

  if (!target["verified"].is_true()) {
    warnings.push_back(warning_with(
        "unverified_embodiment",
        "This body description is not signed by its vendor. Its joint ratings are unattested.",
        {std::string(target["robot_slug"].string_or(""))}));
  }

  if (source["action_channels"].items().empty() && source["motions"].items().empty()) {
    warnings.push_back(warning(
        "no_source_embodiment",
        "The policy declares no action space, so nothing about its fit to this body can be "
        "checked."));
  }

  const Json* envelope = field(target, "envelope");
  if (envelope == nullptr || envelope->is_null()) {
    warnings.push_back(warning_with(
        "no_target_envelope",
        "No safety envelope has been declared for this robot. Per-joint ratings are the only "
        "ceilings available.",
        {std::string(target["robot_slug"].string_or(""))}));
  }

  const Json* source_dof = field(source, "dof");
  if (source_dof != nullptr && !(source_dof->is_number() && target["dof"].is_number() &&
                                 source_dof->number_or(0) == target["dof"].number_or(0))) {
    warnings.push_back(warning("dof_mismatch",
                               "The policy was trained on a " + js::interpolate(source_dof) +
                                   " degree-of-freedom body; this one has " +
                                   js::interpolate(&target["dof"]) + "."));
  }

  const std::string control_mode(source["control_mode"].string_or(""));
  bool mode_supported = false;
  for (const Json& mode : target["control"]["modes"].items()) {
    if (mode.string_or("") == control_mode) {
      mode_supported = true;
      break;
    }
  }
  // A control mode is how a *joint* is commanded, so only a joint plan is asked about one.
  if (space == Space::Joints && !mode_supported) {
    warnings.push_back(warning_with("control_mode_unsupported",
                                    "This body does not accept " + control_mode + " commands.",
                                    {control_mode}));
  }

  if (field(target["control"], "command_latency_ms") == nullptr) {
    warnings.push_back(warning(
        "latency_undeclared",
        "This body declares no command latency, so loop stability at the policy rate cannot be "
        "predicted."));
  }

  // The two refusals that happen before anything is bound (§84.2, §84.3). Observations first and
  // the rate second, as on every other path, so their warnings arrive in one order.
  if (space == Space::Ambiguous || space == Space::MismatchMotions ||
      space == Space::MismatchJoints) {
    warnings.push_back(action_space_warning(space, target));
    const Json observations = bind_observations(source["observations"], target, warnings);
    const Json control_hz = resolve_control_hz(source, target, options, warnings);
    const Json no_motions = Json::array();
    const bool declared = !source["motions"].items().empty();
    return shape_plan("refused", source, target, control_mode, control_hz, {},
                      declared ? &no_motions : nullptr, observations, build_clamp(target, {}),
                      warnings);
  }

  if (space == Space::Motions) {
    Json motions = Json::array();
    for (const Json& motion : source["motions"].items()) {
      motions.push(bind_motion(motion, target, warnings));
    }
    const Json observations = bind_observations(source["observations"], target, warnings);
    const Json control_hz = resolve_control_hz(source, target, options, warnings);
    // No `unmapped_roles`: its sentence is that the controller *holds* the joints the policy does
    // not drive, and on a program-level body the controller is running the motion.
    return shape_plan(derive_motion_verdict(motions), source, target, control_mode, control_hz, {},
                      &motions, observations, build_clamp(target, {}), warnings);
  }

  // Chains that cannot be satisfied joint-by-joint. Retargeting is a property of a whole kinematic
  // chain, never of one joint: you cannot send an elbow through task space while its shoulder goes
  // direct. So the chains that need it are decided first, and every channel on such a chain is then
  // marked together.
  const std::vector<std::string> retarget_chains =
      decide_retarget_chains(source, index, target, options, warnings);

  for (const Json& channel : source["action_channels"].items()) {
    bindings.push_back(bind_channel(channel, target, index, retarget_chains, warnings));
  }

  const Json observations = bind_observations(source["observations"], target, warnings);
  const Json control_hz = resolve_control_hz(source, target, options, warnings);
  const Json clamp = build_clamp(target, bindings);
  const std::string verdict = derive_verdict(bindings);

  std::vector<std::string> unmapped;
  for (const std::string& role : index.role_keys) {
    bool driven = false;
    for (const Binding& binding : bindings) {
      if (binding.source_role.is_null()) {
        continue;
      }
      const std::optional<roles::ParsedRole> parsed = roles::parse(binding.source_role);
      if (parsed.has_value() && roles::key(*parsed) == role) {
        driven = true;
        break;
      }
    }
    if (!driven) {
      unmapped.push_back(role);
    }
  }
  if (!unmapped.empty()) {
    const std::string subject =
        unmapped.size() == 1 ? "joint on this body is" : "joints on this body are";
    warnings.push_back(warning_with(
        "unmapped_roles",
        std::to_string(unmapped.size()) + " " + subject +
            " not driven by the policy and will be held by the controller.",
        unmapped));
  }

  return shape_plan(verdict, source, target, control_mode, control_hz, bindings, nullptr,
                    observations, clamp, warnings);
}

}  // namespace skynet::transport

namespace skynet::contracts {

Json transport(const std::string& /*label*/, const Json& input) {
  return skynet::transport::resolve(input["source"], input["target"], input["options"]);
}

}  // namespace skynet::contracts
