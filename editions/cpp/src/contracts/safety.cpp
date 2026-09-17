// The Safety Kernel contract, C++ edition.
//
// Transcribed from `reference/typescript/safety.ts`. Every number, every message string and every
// ordering decision is read from that file; nothing here is read from `corpus/`, because an edition
// generated from the corpus agrees with it by construction and proves nothing.
//
// One invariant holds the whole operating system together:
//
//     **The Safety Kernel is the only component with write access to an actuator.**
//
// A policy does not command a joint. It writes a *desired* command into a buffer the kernel reads,
// and the kernel decides what reaches the motors. That is what lets Skynet run untrusted policies
// at all: most published checkpoints are pickles, which execute arbitrary code when they are read,
// and a system where loading a model can move an arm is a system where every published model is a
// remote code execution vulnerability attached to a two-metre humanoid.
//
// ── Four places where C++ is quietly not JavaScript ─────────────────────────
//
// Each produces a *different verdict*, not a different message.
//
// `undefined` versus `null`
//     Three rules here test for `undefined` **strictly** — `requested !== undefined` in the
//     widening check, `self_collision_margin_m === undefined` for the margin notice, and
//     `capability !== undefined` at the mode gate. A key present and null takes the *other* branch
//     in the reference. A null `Json` cannot carry that distinction, so `field()` returns a
//     pointer and absence is nullptr.
//
// `typeof v === 'number'`
//     False for a boolean. An envelope carrying `joint_torque_max_nm: true` must not clamp every
//     joint to 1 Nm, so `narrowest` admits only `Json::Kind::Number`.
//
// `Math.sign`
//     Returns `-0` for `-0` and `NaN` for `NaN`, neither of which `std::copysign` reproduces:
//     `copysign(1, NaN)` is a confident ±1 for a command nobody can represent.
//
// `Object.entries`
//     JavaScript emits **array-index-like keys first, in ascending numeric order**, and only then
//     the remaining keys in insertion order. No channel in today's corpus is named `"0"`, so this
//     costs nothing today and reorders `clamped` the day a fuzz vector names one.
//
// `null > number`
//     The widening check reaches `null > ceiling` when a profile field is present and null, and
//     JavaScript coerces that to `0 > ceiling`. Transcribed rather than tidied: the reference is
//     the specification, and a "sensible" reading of it here is a divergence.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/kinematics.hpp"
#include "skynet_os/safety.hpp"
#include "skynet_os/text.hpp"
#include "skynet_os/vocabulary.hpp"

#include <algorithm>
#include <cmath>
#include <limits>

namespace skynet::safety {
namespace {

using kinematics::Frame;
using kinematics::Transform;

/// `obj?.key` — nullptr when the object is missing or the key is not present. Optional chaining
/// short-circuits on `null` as well as `undefined` and yields `undefined` for both, which is why a
/// null `profile` and an absent one behave identically here.
const Json* field(const Json* obj, std::string_view key) {
  if (obj == nullptr || !obj->is_object() || !obj->has(key)) {
    return nullptr;
  }
  return &(*obj)[key];
}

/// `typeof v === 'number' && Number.isFinite(v)`.
bool finite_number(const Json* value) {
  return value != nullptr && value->is_number() && std::isfinite(value->number_or(0));
}

/// Narrowest of the values present. Absent inputs never widen the result.
std::optional<double> narrowest(std::initializer_list<const Json*> values) {
  std::optional<double> best;
  for (const Json* value : values) {
    if (!finite_number(value)) {
      continue;
    }
    const double held = value->number_or(0);
    if (!best.has_value() || held < *best) {
      best = held;
    }
  }
  return best;
}

/// `Math.sign` — which returns `-0` for `-0` and `NaN` for `NaN`. Returning the input for those
/// three cases is what `copysign` cannot do: it maps NaN onto ±1 and would turn an unrepresentable
/// command into a confident clamp.
double sign(double value) {
  if (value > 0) {
    return 1;
  }
  if (value < 0) {
    return -1;
  }
  return value;
}

/// `Object.entries` — array-index keys first in numeric order, then insertion order.
std::vector<const std::pair<std::string, Json>*> entries(const Json& object) {
  std::vector<const std::pair<std::string, Json>*> index_like;
  std::vector<const std::pair<std::string, Json>*> rest;
  for (const auto& entry : object.entries()) {
    const std::string& key = entry.first;
    const bool numeric = !key.empty() &&
                         std::all_of(key.begin(), key.end(), text::is_ascii_digit) &&
                         (key == "0" || key[0] != '0') && key.size() <= 10 &&
                         std::strtoull(key.c_str(), nullptr, 10) < 4294967295ULL;
    (numeric ? index_like : rest).push_back(&entry);
  }
  std::sort(index_like.begin(), index_like.end(), [](const auto* left, const auto* right) {
    return std::strtoull(left->first.c_str(), nullptr, 10) <
           std::strtoull(right->first.c_str(), nullptr, 10);
  });
  index_like.insert(index_like.end(), rest.begin(), rest.end());
  return index_like;
}

/// `value ?? null` — nullish coalescing, which does **not** fire on `0` or `""`.
Json or_null(const Json* value) {
  return value == nullptr || value->is_null() ? Json::null() : *value;
}

/// `a ?? b ?? null` — the first value that is neither absent nor null, and `""` is one.
Json coalesce(const Json* first, const Json* second) {
  if (first != nullptr && !first->is_null()) {
    return *first;
  }
  if (second != nullptr && !second->is_null()) {
    return *second;
  }
  return Json::null();
}

/// How a template literal renders a value. `undefined` is the six characters, not the empty string
/// — a profile with no name reads `The operator profile "undefined" …` in the reference, and an
/// edition that printed `""` there would produce a different message for the same input.
std::string js_string(const Json* value) {
  if (value == nullptr) {
    return "undefined";
  }
  switch (value->kind()) {
    case Json::Kind::Null: return "null";
    case Json::Kind::Bool: return value->is_true() ? "true" : "false";
    case Json::Kind::Number: return format_number(value->number_or(0));
    case Json::Kind::String: return std::string(value->string_or(""));
    default: return "[object Object]";
  }
}

/// JavaScript's `>` for the one shape the widening checks reach: a possibly-null requested value
/// against a finite ceiling. `null > x` is `0 > x`; everything that is not a number coerces to NaN
/// and compares false.
bool js_greater(const Json* requested, double ceiling) {
  if (requested == nullptr) {
    return false;
  }
  double left = 0;
  if (requested->is_null()) {
    left = 0;  // The coercion the reference performs, reproduced rather than tidied.
  } else if (requested->is_number()) {
    left = requested->number_or(0);
  } else if (requested->is_bool()) {
    left = requested->is_true() ? 1 : 0;
  } else {
    return false;
  }
  return left > ceiling;
}

Json notice(std::string code, std::string message) {
  Json out = Json::object();
  out.set("code", Json::string(std::move(code)));
  out.set("message", Json::string(std::move(message)));
  return out;
}

Json notice_with(std::string code, std::string message, Json subjects) {
  Json out = notice(std::move(code), std::move(message));
  out.set("subjects", std::move(subjects));
  return out;
}

Json string_array(const std::vector<std::string>& values) {
  Json out = Json::array();
  for (const std::string& value : values) {
    out.push(Json::string(value));
  }
  return out;
}

/// Graduated responses, weakest to strongest, following IEC 60204-1.
int response_rank(const std::string& response) {
  const vocabulary::Set& order = vocabulary::members("fault_responses");
  const auto found = std::find(order.begin(), order.end(), response);
  return found == order.end() ? -1 : static_cast<int>(found - order.begin());
}

std::string stronger(const std::string& current, const std::string& response) {
  if (current.empty()) {
    return response;
  }
  return response_rank(current) >= response_rank(response) ? current : response;
}

constexpr std::size_t kMaxPoseTrips = 8;

}  // namespace

Limits resolve_limits(const Params& params) {
  Limits limits;
  limits.tier = params.tier;
  limits.enforcement = resolve_enforcement(params.tier);

  const Json* envelope = params.envelope;
  const Json* transport = params.transport;
  const Json* profile = params.profile;
  const Json* session = params.session;

  if (limits.enforcement == "advisory") {
    limits.notices.push_back(notice(
        "advisory_only",
        "At the guest tier Skynet OS does not own the control loop. Violations are detected and "
        "recorded; commands still reach the robot unchanged. This is not a safety function."));
  }

  if (!independent_stop_path(params.tier)) {
    limits.notices.push_back(notice(
        "no_independent_stop",
        "There is no stop path independent of the application processor. If the processor hangs, "
        "nothing here stops the robot."));
  }

  if (envelope == nullptr || envelope->is_null()) {
    limits.notices.push_back(notice(
        "no_envelope",
        "No safety envelope has been declared for this robot. Only per-joint ratings from the body "
        "description are available, and whole-body ceilings are unenforced."));
  } else {
    // Falsy, not absent: an envelope citing the empty string has cited nothing. The `source` field
    // below still carries it, because `??` does not fire on `""` — the two disagree on purpose,
    // and the notice is the half an operator reads.
    const Json* source = field(envelope, "source");
    const bool truthy = source != nullptr && !source->is_null() &&
                        !(source->is_string() && source->string_or("").empty()) &&
                        !(source->is_bool() && !source->is_true()) &&
                        !(source->is_number() && source->number_or(0) == 0);
    if (!truthy) {
      limits.notices.push_back(notice("unsourced_limits",
                                      "The declared limits cite no datasheet. A safety limit "
                                      "without provenance is a rumour."));
    }
  }

  // Widening attempts are recorded, then discarded by the minimum. Silently ignoring one would
  // leave an operator believing a limit they set is in force.
  std::vector<std::string> widened;
  const auto check_widen = [&](const char* label, const Json* requested, const Json* ceiling) {
    if (requested == nullptr || !finite_number(ceiling)) {
      return;
    }
    if (js_greater(requested, ceiling->number_or(0))) {
      widened.emplace_back(label);
    }
  };
  check_widen("joint_velocity_max_rad_s", field(profile, "joint_velocity_max_rad_s"),
              field(envelope, "joint_velocity_max_rad_s"));
  check_widen("joint_torque_max_nm", field(profile, "joint_torque_max_nm"),
              field(envelope, "joint_torque_max_nm"));
  check_widen("base_velocity_max_m_s", field(profile, "base_velocity_max_m_s"),
              field(envelope, "base_velocity_max_m_s"));
  if (!widened.empty()) {
    limits.notices.push_back(notice_with(
        "profile_widened",
        "The operator profile \"" + js_string(field(profile, "name")) +
            "\" asks to raise limits above the declared envelope. Those values are ignored.",
        string_array(widened)));
  }

  // §38.2's session layer, checked against **what the layer above it allows** rather than against
  // the envelope alone — that is the section's own wording, and the layer above a session is
  // everything already composed. A session asking for 3 rad/s under a profile allowing 1 has not
  // widened the envelope; it has widened the number actually in force.
  //
  // Reported separately from `profile_widened`: two documents, two sentences, and telling somebody
  // their signed profile over-reached when it was their bench override sends them to the wrong file.
  std::vector<std::string> session_widened;
  const auto check_session = [&](const char* label, const Json* requested,
                                 const std::optional<double>& ceiling) {
    if (requested == nullptr || !ceiling.has_value()) {
      return;
    }
    if (js_greater(requested, *ceiling)) {
      session_widened.emplace_back(label);
    }
  };
  if (session != nullptr && !session->is_null()) {
    check_session("joint_velocity_max_rad_s", field(session, "joint_velocity_max_rad_s"),
                  narrowest({field(envelope, "joint_velocity_max_rad_s"),
                             field(transport, "joint_velocity_max_rad_s"),
                             field(profile, "joint_velocity_max_rad_s")}));
    check_session("joint_torque_max_nm", field(session, "joint_torque_max_nm"),
                  narrowest({field(envelope, "joint_torque_max_nm"),
                             field(transport, "joint_torque_max_nm"),
                             field(profile, "joint_torque_max_nm")}));
    check_session("base_velocity_max_m_s", field(session, "base_velocity_max_m_s"),
                  narrowest({field(envelope, "base_velocity_max_m_s"),
                             field(transport, "base_velocity_max_m_s"),
                             field(profile, "base_velocity_max_m_s")}));
    const Json* profile_delta = field(profile, "max_delta_per_cycle");
    check_session("max_delta_per_cycle", field(session, "max_delta_per_cycle"),
                  finite_number(profile_delta) ? std::optional<double>(profile_delta->number_or(0))
                                               : std::nullopt);
  }
  if (!session_widened.empty()) {
    limits.notices.push_back(notice_with(
        "session_widened",
        "The session override asks to raise limits above what is already in force. Those values "
        "are ignored; the narrower ceiling still applies.",
        string_array(session_widened)));
  }

  // The session joins the same minimum as every other layer, which is what makes "it only ever
  // narrows" a property of the arithmetic rather than a rule somebody has to remember.
  limits.joint_velocity_max_rad_s = narrowest({field(envelope, "joint_velocity_max_rad_s"),
                                               field(transport, "joint_velocity_max_rad_s"),
                                               field(profile, "joint_velocity_max_rad_s"),
                                               field(session, "joint_velocity_max_rad_s")});
  limits.joint_torque_max_nm =
      narrowest({field(envelope, "joint_torque_max_nm"), field(transport, "joint_torque_max_nm"),
                 field(profile, "joint_torque_max_nm"), field(session, "joint_torque_max_nm")});
  limits.base_velocity_max_m_s = narrowest(
      {field(envelope, "base_velocity_max_m_s"), field(transport, "base_velocity_max_m_s"),
       field(profile, "base_velocity_max_m_s"), field(session, "base_velocity_max_m_s")});
  limits.max_delta_per_cycle =
      narrowest({field(profile, "max_delta_per_cycle"), field(session, "max_delta_per_cycle")});

  if (!limits.max_delta_per_cycle.has_value()) {
    limits.notices.push_back(notice(
        "no_rate_limit",
        "No jerk ceiling is configured, so a single-cycle jump from zero to the clamped maximum is "
        "permitted. Set max_delta_per_cycle before running a policy near people."));
  }

  // §38.2's hazard, stated rather than left to somebody noticing a small number: "a 'maintenance
  // mode, half speed' that outlives the maintenance is a hazard that looks like a setting." A robot
  // under an override announces it, and names it so the sentence says what to clear.
  if (session != nullptr && !session->is_null()) {
    const Json* session_name = field(session, "name");
    Json subjects = Json::array();
    subjects.push(session_name == nullptr ? Json::null() : *session_name);
    limits.notices.push_back(notice_with(
        "session_override",
        "A session override \"" + js_string(session_name) +
            "\" is narrowing the limits in force. It is a temporary setting rather than this "
            "robot's configuration — clear it when the work is done.",
        subjects));
  }

  limits.min_control_hz = or_null(field(transport, "min_control_hz"));
  const Json* per_channel = field(transport, "per_channel");
  limits.per_channel = per_channel == nullptr || per_channel->is_null() ? Json::object()
                                                                       : *per_channel;
  const Json* joint_modes = field(transport, "joint_modes");
  limits.joint_modes = joint_modes == nullptr || joint_modes->is_null() ? Json::object()
                                                                       : *joint_modes;
  limits.proximity = or_null(field(profile, "proximity"));
  const Json* workspace = field(profile, "workspace");
  limits.workspace = workspace == nullptr || workspace->is_null() ? Json::array() : *workspace;
  limits.self_collision_margin_m = or_null(field(profile, "self_collision_margin_m"));
  limits.source = coalesce(field(envelope, "source"), field(transport, "source"));
  limits.frames = params.frames;

  partition_guards(params, limits);

  // Emitted only when the guard it is about will actually run. A robot with no collision geometry
  // is already being told the guard is unavailable, and adding "and its margin is unset" to that is
  // a second sentence about a thing that is not happening.
  const bool self_collision_active =
      std::find(limits.active_guards.begin(), limits.active_guards.end(), "self_collision") !=
      limits.active_guards.end();
  if (self_collision_active && field(profile, "self_collision_margin_m") == nullptr) {
    Json entry = notice("no_collision_margin",
                        "No self-collision margin is configured, so two of this robot's own bodies "
                        "must actually meet before the guard holds. Set self_collision_margin_m to "
                        "stop the command that would bring them together.");
    entry.set("guard", Json::string("self_collision"));
    limits.notices.push_back(entry);
  }

  for (const GuardUnavailable& entry : limits.unavailable_guards) {
    limits.notices.push_back(notice_with("guard_unavailable",
                                         "The " + entry.guard + " guard cannot run on this robot.",
                                         string_array(entry.missing)));
  }

  return limits;
}

void partition_guards(const Params& params, Limits& limits) {
  const Json* envelope = params.envelope;
  const Json* profile = params.profile;
  const Json* available = params.available;
  const std::vector<Frame>* frames = params.frames;
  const bool has_frames = frames != nullptr && !frames->empty();

  const auto truthy = [](const Json* value) {
    if (value == nullptr || value->is_null()) {
      return false;
    }
    if (value->is_bool()) {
      return value->is_true();
    }
    if (value->is_number()) {
      return value->number_or(0) != 0;
    }
    if (value->is_string()) {
      return !value->string_or("").empty();
    }
    return true;
  };
  const auto declared = [&](const char* key) {
    const Json* value = field(available, key);
    return value != nullptr && value->is_true();
  };

  const auto missing_for = [&](const std::string& guard) -> std::vector<std::string> {
    if (guard == "watchdog") {
      // Truthy, not present: a watchdog window of zero is no window at all.
      return truthy(field(envelope, "watchdog_ms")) ? std::vector<std::string>{}
                                                    : std::vector<std::string>{"envelope.watchdog_ms"};
    }
    if (guard == "envelope" || guard == "mode_gate") {
      return {};
    }
    if (guard == "rate_limit") {
      return limits.max_delta_per_cycle.has_value()
                 ? std::vector<std::string>{}
                 : std::vector<std::string>{"profile.max_delta_per_cycle"};
    }
    // §44.5: a body whose descriptor declares no frame graph, or whose graph does not reach the
    // frame the operator constrained, reports these two unavailable **with the missing frame
    // named** — rather than evaluating them against a frame it guessed.
    if (guard == "workspace") {
      std::vector<std::string> missing;
      std::size_t volumes = 0;
      for (const Json& layer : limits.workspace.items()) {
        volumes += layer["volumes"].items().size();
      }
      if (volumes == 0) {
        missing.emplace_back("profile.workspace");
      }
      if (!has_frames) {
        missing.emplace_back("embodiment.frames");
      } else {
        for (const Json& layer : limits.workspace.items()) {
          for (const Json& volume : layer["volumes"].items()) {
            const std::string wanted(volume["frame"].string_or(""));
            std::size_t index = frames->size();
            for (std::size_t at = 0; at < frames->size(); ++at) {
              if ((*frames)[at].name == wanted) {
                index = at;
                break;
              }
            }
            const bool resolvable =
                index < frames->size() && kinematics::statically_resolvable(*frames, index);
            if (!resolvable &&
                std::find(missing.begin(), missing.end(), wanted) == missing.end()) {
              missing.push_back(wanted);
            }
          }
        }
      }
      return missing;
    }
    if (guard == "self_collision") {
      if (!has_frames) {
        return {"embodiment.frames"};
      }
      // Two, not one. A single collision body has nothing to collide with, and a guard reported
      // active with no pair to test is a guard that passes by not looking.
      return kinematics::collision_bodies(*frames) < 2
                 ? std::vector<std::string>{"embodiment.frames[].collision, on two or more frames"}
                 : std::vector<std::string>{};
    }
    if (guard == "balance") {
      // Not applicable rather than missing on a wheeled base; reported as inactive either way, and
      // the missing list says which, so nobody reads "balance guard off" as a defect.
      if (!declared("legged")) {
        return {"a legged base — not applicable to this body"};
      }
      return declared("imu") ? std::vector<std::string>{} : std::vector<std::string>{"an IMU"};
    }
    if (guard == "proximity") {
      if (!truthy(field(profile, "proximity"))) {
        return {"profile.proximity"};
      }
      return declared("people_detection") ? std::vector<std::string>{}
                                          : std::vector<std::string>{"a sensor that detects people"};
    }
    throw JsonError("unknown guard " + guard);
  };

  for (const std::string_view guard_view : vocabulary::members("guards")) {
    const std::string guard(guard_view);
    std::vector<std::string> missing = missing_for(guard);
    if (!missing.empty()) {
      limits.unavailable_guards.push_back(GuardUnavailable{guard, std::move(missing)});
      continue;
    }
    // Checked **after** `missing`, and the order is the point: a guard with no sensor is
    // unavailable, not inert. Reporting a robot that has no person detector as "the reading is not
    // coming" would describe a missing producer on a machine whose problem is a sensor.
    //
    // Only `proximity` has a per-cycle reading that can be absent (§80): the other seven act on the
    // command, the previous command, the clock or the frame graph, all of which the cycle carries.
    if (guard == "proximity" && !declared("proximity_reading")) {
      limits.inert_guards.push_back(GuardInert{guard, "cycle.nearest_person_m"});
      continue;
    }
    limits.active_guards.push_back(guard);
  }
}

bool Limits::guard_active(std::string_view guard) const {
  return std::find(active_guards.begin(), active_guards.end(), guard) != active_guards.end();
}

std::string resolve_enforcement(const std::string& tier) {
  // Derived, never supplied. A caller cannot pass in the enforcement mode it would prefer.
  if (tier == "guest") {
    return "advisory";
  }
  if (tier == "host") {
    return "enforcing";
  }
  if (tier == "certified") {
    return "enforcing_redundant";
  }
  throw JsonError("unknown tier " + tier);
}

bool independent_stop_path(const std::string& tier) { return tier == "certified"; }

namespace {

/// The ceiling that applies to one channel.
///
/// Three sources, and they **compose by narrowing** (§86.3). The per-channel figure is the joint's
/// own rating carried by the transport plan. `base` is the reserved whole-body channel. The third is
/// the joint's mode, which says which of the two whole-body figures is in its units — a step that
/// was impossible before §86, so both were resolved through four layers and applied to nothing.
///
/// Position is deliberately not in the table: travel is a per-joint bound with two ends and one
/// magnitude cannot express it, so the plan's per-channel entry is all there is.
std::optional<double> ceiling_for(const std::string& channel, const Limits& limits) {
  std::optional<double> per_channel;
  if (limits.per_channel.has(channel)) {
    const Json& held = limits.per_channel[channel];
    if (held.is_number()) per_channel = held.number_or(0);
  }

  std::optional<double> whole_body;
  if (channel == "base") {
    whole_body = limits.base_velocity_max_m_s;
  } else if (limits.joint_modes.has(channel)) {
    const std::string_view mode = limits.joint_modes[channel].string_or("");
    if (mode == "velocity") {
      whole_body = limits.joint_velocity_max_rad_s;
    } else if (mode == "torque") {
      whole_body = limits.joint_torque_max_nm;
    }
  }

  if (!per_channel.has_value()) return whole_body;
  if (!whole_body.has_value()) return per_channel;
  return std::min(*per_channel, *whole_body);
}

/// §7.1's fourth guard: every constrained frame inside its operator-defined volumes.
void check_workspace(const std::vector<Frame>& frames,
                     const std::vector<std::optional<Transform>>& poses, const Json& layers,
                     Pose& pose) {
  for (std::size_t index = 0; index < frames.size(); ++index) {
    // Layers compose by conjunction: the frame must satisfy every layer that names it, and the
    // reported margin is the worst of them. A layer that does not name this frame says nothing
    // about it, so a site rule about one arm does not confine the other.
    bool constrained = false;
    std::optional<double> worst;
    for (const Json& layer : layers.items()) {
      std::optional<double> best;
      for (const Json& volume : layer["volumes"].items()) {
        if (volume["frame"].string_or("") != frames[index].name) {
          continue;
        }
        constrained = true;
        if (poses[index].has_value()) {
          const double value = kinematics::margin(volume["shape"], poses[index]->translation);
          if (!best.has_value() || value > *best) {
            best = value;
          }
        }
      }
      if (best.has_value() && (!worst.has_value() || *best < *worst)) {
        worst = best;
      }
    }

    if (!constrained) {
      continue;
    }
    if (!poses[index].has_value()) {
      // Constrained and unlocatable. Not a pass: the guard has not shown the command safe.
      pose.trips.push_back(PoseTrip{"workspace", frames[index].name, std::nullopt, std::nullopt});
      continue;
    }
    if (worst.has_value() && *worst < 0) {
      pose.trips.push_back(PoseTrip{"workspace", frames[index].name, std::nullopt, worst});
    }
  }
}

/// §7.1's fifth guard: no two collision bodies closer than the operator's margin.
///
/// Quadratic, and the pairs are visited in index order so the reported list is the same on every
/// edition. Adjacent frames and declared exceptions are skipped before any arithmetic — a
/// humanoid's torso and pelvis overlap by design, and a guard that tripped on the robot standing
/// still is a guard an operator switches off.
void check_self_collision(const std::vector<Frame>& frames,
                          const std::vector<std::optional<Transform>>& poses, double margin_m,
                          Pose& pose) {
  const double clearance = margin_m > 0 ? margin_m : 0;
  for (std::size_t i = 0; i < frames.size(); ++i) {
    if (!frames[i].collision.has_value()) {
      continue;
    }
    for (std::size_t j = i + 1; j < frames.size(); ++j) {
      if (!frames[j].collision.has_value()) {
        continue;
      }
      if (kinematics::adjacent(frames, i, j) || kinematics::ignores(frames, i, j)) {
        continue;
      }
      if (!poses[i].has_value() || !poses[j].has_value()) {
        // One of the pair has no pose. Reported against the pair rather than silently skipped — a
        // collision test that quietly excludes a limb is a test that passes because it did not look.
        pose.trips.push_back(
            PoseTrip{"self_collision", frames[i].name, frames[j].name, std::nullopt});
        continue;
      }
      const auto [a1, b1] = kinematics::capsule_in_world(*poses[i], *frames[i].collision);
      const auto [a2, b2] = kinematics::capsule_in_world(*poses[j], *frames[j].collision);
      const double squared = kinematics::segment_distance_squared(a1, b1, a2, b2);
      const double threshold =
          frames[i].collision->radius_m + frames[j].collision->radius_m + clearance;
      // The decision is taken in squared distance, so no square root can move a verdict. The root
      // is taken only for the number the report carries.
      if (squared < threshold * threshold) {
        pose.trips.push_back(PoseTrip{
            "self_collision", frames[i].name, frames[j].name,
            kinematics::distance(squared) - frames[i].collision->radius_m -
                frames[j].collision->radius_m - clearance});
      }
    }
  }
}

}  // namespace

Verdict evaluate_cycle(const Json& cycle, const Limits& limits, const Json* envelope,
                       const Json* last_sequence, const std::vector<kinematics::Binding>& bindings) {
  Verdict verdict;
  verdict.advisory = limits.enforcement == "advisory";

  // The strongest response any guard *demanded this cycle* — not the strongest response any tripped
  // guard is capable of demanding, which is a different and weaker thing. The difference is
  // `proximity`, whose §7.1 row reads `stop_cat1` because that is what it does inside the stop
  // distance; between the two distances it scales the command and lets it through, and a verdict
  // deriving its response from the guard's row would say "stopped" about a robot that was not.
  std::string demanded;

  const Json& desired = cycle["desired"];
  const auto desired_copy = [&]() {
    return desired.is_object() ? desired : Json::object();
  };
  const auto trip = [&](const char* guard) {
    if (std::find(verdict.tripped.begin(), verdict.tripped.end(), guard) == verdict.tripped.end()) {
      verdict.tripped.emplace_back(guard);
    }
  };

  // 1. Watchdog. A stale command is discarded, never clamped and forwarded — clamping it would keep
  //    a dead producer's last intention alive at a legal magnitude, which is the failure mode the
  //    watchdog exists to prevent.
  const Json* watchdog_ms = field(envelope, "watchdog_ms");
  const bool stale = watchdog_ms != nullptr && cycle["now_ms"].number_or(0) -
                                                       cycle["command_ms"].number_or(0) >
                                                   watchdog_ms->number_or(0);
  const bool stalled =
      last_sequence != nullptr && cycle["sequence"].number_or(0) <= last_sequence->number_or(0);
  if (limits.guard_active("watchdog") && (stale || stalled)) {
    trip("watchdog");
    verdict.command = verdict.advisory ? desired_copy() : Json::object();
    verdict.response = "stop_cat2";
    return verdict;
  }

  Json command = Json::object();

  // 2. Magnitude clamping against the resolved ceilings.
  for (const auto* entry : entries(desired)) {
    const std::string& channel = entry->first;
    const double value = entry->second.number_or(0);
    const std::optional<double> ceiling = ceiling_for(channel, limits);
    double next_value = value;
    if (ceiling.has_value() && std::fabs(value) > *ceiling) {
      next_value = sign(value) * *ceiling;
      trip("envelope");
      demanded = stronger(demanded, "clamp");
      verdict.clamped.push_back(Clamp{channel, value, next_value});
    }
    command.set(channel, Json::number(next_value));
  }

  // 3. Rate limiting, after clamping, because clamping creates step changes of its own.
  const Json& previous = cycle["previous"];
  const bool has_previous = previous.is_object() && !previous.entries().empty();
  if (limits.max_delta_per_cycle.has_value() && has_previous) {
    for (const auto* entry : entries(command)) {
      const std::string& channel = entry->first;
      const double value = entry->second.number_or(0);
      const Json* before = field(&previous, channel);
      if (before == nullptr) {
        continue;
      }
      const double last = before->number_or(0);
      const std::optional<double> ceiling = ceiling_for(channel, limits);
      const double max_step = ceiling.has_value() ? *ceiling * *limits.max_delta_per_cycle
                                                  : std::fabs(last) * *limits.max_delta_per_cycle;
      const double delta = value - last;
      if (std::fabs(delta) > max_step && max_step > 0) {
        const double next_value = last + sign(delta) * max_step;
        trip("rate_limit");
        demanded = stronger(demanded, "clamp");
        verdict.clamped.push_back(Clamp{channel, value, next_value});
        command.set(channel, Json::number(next_value));
      }
    }
  }

  // 4 and 5. The pose guards, on the configuration the command above would produce.
  //
  //    After clamping and rate limiting, and that ordering is the point: the pose that matters is
  //    the one the robot will actually be commanded into, not the one the policy asked for and the
  //    envelope refused.
  const bool wants_pose =
      limits.guard_active("workspace") || limits.guard_active("self_collision");
  if (wants_pose && limits.frames != nullptr && !limits.frames->empty()) {
    const std::vector<Frame>& frames = *limits.frames;
    const kinematics::Solved solved = kinematics::solve(frames, bindings, command);
    verdict.pose.evaluated = true;
    verdict.pose.lookahead = solved.lookahead;
    for (const std::size_t index : solved.unresolved) {
      verdict.pose.unresolved.push_back(frames[index].name);
    }
    if (limits.guard_active("workspace")) {
      check_workspace(frames, solved.poses, limits.workspace, verdict.pose);
    }
    if (limits.guard_active("self_collision")) {
      const double margin_m = limits.self_collision_margin_m.is_number()
                                  ? limits.self_collision_margin_m.number_or(0)
                                  : 0;
      check_self_collision(frames, solved.poses, margin_m, verdict.pose);
    }
    if (verdict.pose.trips.size() > kMaxPoseTrips) {
      verdict.pose.trips.resize(kMaxPoseTrips);
      verdict.pose.truncated = true;
    }
    for (const PoseTrip& one : verdict.pose.trips) {
      trip(one.guard.c_str());
      demanded = stronger(demanded, "hold");
    }
  }

  // 7. Proximity — speed and separation monitoring, when something is measuring separation.
  //
  //    After the magnitude and rate guards because it scales the command those two produced:
  //    applying it first would let a clamp raise a speed the separation had already reduced, and
  //    the composition rule everywhere in this system is that a later stage may only narrow.
  if (limits.guard_active("proximity")) {
    const Json* distance_m = field(&cycle, "nearest_person_m");
    const Json& prox = limits.proximity;
    const bool prox_declared = !prox.is_null() && !(prox.is_object() && prox.entries().empty());
    if (distance_m != nullptr && prox_declared) {
      const double separation = distance_m->number_or(0);
      if (separation <= prox["stop_distance_m"].number_or(0)) {
        trip("proximity");
        verdict.command = verdict.advisory ? desired_copy() : Json::object();
        verdict.response = "stop_cat1";
        return verdict;
      }
      if (separation <= prox["slow_distance_m"].number_or(0) &&
          prox["slow_factor"].number_or(0) < 1) {
        trip("proximity");
        demanded = stronger(demanded, "clamp");  // A reduction, not a stop.
        const double factor = prox["slow_factor"].number_or(0);
        for (const auto* entry : entries(command)) {
          const std::string& channel = entry->first;
          const double value = entry->second.number_or(0);
          const double next_value = value * factor;
          if (next_value != value) {
            verdict.clamped.push_back(Clamp{channel, value, next_value});
            command.set(channel, Json::number(next_value));
          }
        }
      }
    }
  }

  // 8. Mode gate — the capability this command exercises must be one the operator granted.
  //
  //    Enforced at every cycle rather than at start, so a revocation takes effect on the next
  //    command rather than on the next restart (§8.2).
  if (limits.guard_active("mode_gate")) {
    const Json* capability = field(&cycle, "capability");
    if (capability != nullptr) {
      bool granted = false;
      for (const Json& grant : limits.grants.items()) {
        if (grant.is_string() && capability->is_string() &&
            grant.string_or("") == capability->string_or("")) {
          granted = true;
          break;
        }
      }
      if (!granted) {
        trip("mode_gate");
        demanded = stronger(demanded, "hold");
      }
    }
  }

  // A hold means the command does not go out. The rule lives here, once, rather than at each guard
  // that demands one: `hold` is the last safe command being held while the policy is given a cycle
  // to recover, and forwarding a clamped command alongside a `hold` verdict would be the opposite
  // of what it reported.
  const bool withheld = !demanded.empty() && response_rank(demanded) >= response_rank("hold");

  // In advisory mode the desired command is what reaches the robot. Returning the clamped values
  // here would make a guest-tier deployment look enforced in its own logs.
  verdict.command = verdict.advisory ? desired_copy() : (withheld ? Json::object() : command);
  verdict.response = demanded;
  return verdict;
}

}  // namespace skynet::safety

namespace skynet::contracts {
namespace {

/// Read `ResolveLimitsParams` off a corpus case, flattening the frame graph if it carries one.
///
/// The case carries the descriptor's **graph**; `resolveLimits` takes the **flattened** frame list
/// (§44.3), and the generator flattens it on the way in. An edition that passed the graph straight
/// through would report both pose guards unavailable on a body that declares frames.
struct CaseParams {
  safety::Params params;
  std::vector<kinematics::Frame> frames;
};

CaseParams params_from(const Json& case_limits) {
  CaseParams out;
  out.params.tier = std::string(case_limits["tier"].string_or(""));
  const auto optional_field = [&](const char* key) -> const Json* {
    return case_limits.has(key) && !case_limits[key].is_null() ? &case_limits[key] : nullptr;
  };
  out.params.envelope = optional_field("envelope");
  out.params.transport = optional_field("transport");
  out.params.profile = optional_field("profile");
  out.params.session = optional_field("session");
  out.params.available = optional_field("available");
  if (const Json* graph = optional_field("frames"); graph != nullptr) {
    out.frames = kinematics::flatten(*graph);
  }
  return out;
}

Json nullable(const std::optional<double>& value) {
  return value.has_value() ? Json::number(*value) : Json::null();
}

/// `shapeLimits`. A projection, not the whole resolved set: `frames`, `workspace`, `proximity` and
/// `grants` are inputs the kernel carries forward, not findings, and the corpus pins what an
/// operator is shown.
Json shape_limits(const safety::Limits& limits) {
  Json out = Json::object();
  out.set("tier", Json::string(limits.tier));
  out.set("enforcement", Json::string(limits.enforcement));
  out.set("joint_velocity_max_rad_s", nullable(limits.joint_velocity_max_rad_s));
  out.set("joint_torque_max_nm", nullable(limits.joint_torque_max_nm));
  out.set("base_velocity_max_m_s", nullable(limits.base_velocity_max_m_s));
  out.set("min_control_hz", limits.min_control_hz);
  out.set("max_delta_per_cycle", nullable(limits.max_delta_per_cycle));
  // §7.1's fifth guard holds at this distance. Reported because an operator who cannot see the
  // margin in force cannot tell a robot holding 5 cm from one holding at contact.
  out.set("self_collision_margin_m", limits.self_collision_margin_m);
  out.set("per_channel", limits.per_channel);
  // §86.3. Absent — not empty — on a body no joint of which is bound, so every limits document
  // written before this section is the same bytes.
  if (!limits.joint_modes.entries().empty()) {
    out.set("joint_modes", limits.joint_modes);
  }

  Json active = Json::array();
  for (const std::string& guard : limits.active_guards) {
    active.push(Json::string(guard));
  }
  out.set("active_guards", active);

  Json unavailable = Json::array();
  for (const safety::GuardUnavailable& entry : limits.unavailable_guards) {
    Json row = Json::object();
    row.set("guard", Json::string(entry.guard));
    Json missing = Json::array();
    for (const std::string& what : entry.missing) {
      missing.push(Json::string(what));
    }
    row.set("missing", missing);
    unavailable.push(row);
  }
  out.set("unavailable_guards", unavailable);

  Json inert = Json::array();
  for (const safety::GuardInert& entry : limits.inert_guards) {
    Json row = Json::object();
    row.set("guard", Json::string(entry.guard));
    row.set("reading", Json::string(entry.reading));
    inert.push(row);
  }
  out.set("inert_guards", inert);

  out.set("source", limits.source);

  Json notices = Json::array();
  for (const Json& one : limits.notices) {
    Json row = Json::object();
    row.set("code", one["code"]);
    row.set("message", one["message"]);
    if (one.has("subjects") && !one["subjects"].is_null()) {
      row.set("subjects", one["subjects"]);
    }
    notices.push(row);
  }
  out.set("notices", notices);
  return out;
}

Json shape_pose(const safety::Pose& pose) {
  Json out = Json::object();
  out.set("evaluated", Json::boolean(pose.evaluated));
  out.set("lookahead", Json::boolean(pose.lookahead));
  out.set("truncated", Json::boolean(pose.truncated));
  Json unresolved = Json::array();
  for (const std::string& name : pose.unresolved) {
    unresolved.push(Json::string(name));
  }
  out.set("unresolved", unresolved);
  Json trips = Json::array();
  for (const safety::PoseTrip& one : pose.trips) {
    Json row = Json::object();
    row.set("guard", Json::string(one.guard));
    row.set("frame", Json::string(one.frame));
    // Present and null on a workspace excursion rather than absent: "this finding is about one
    // frame" is a fact, and a reader of an absent key cannot tell it from a producer that forgot.
    row.set("other", one.other.has_value() ? Json::string(*one.other) : Json::null());
    row.set("margin_m", one.margin_m.has_value() ? Json::number(*one.margin_m) : Json::null());
    trips.push(row);
  }
  out.set("trips", trips);
  return out;
}

Json shape_verdict(const safety::Verdict& verdict) {
  Json out = Json::object();
  out.set("command", verdict.command);
  out.set("response", verdict.response.empty() ? Json::null() : Json::string(verdict.response));
  Json tripped = Json::array();
  for (const std::string& guard : verdict.tripped) {
    tripped.push(Json::string(guard));
  }
  out.set("tripped", tripped);
  Json clamped = Json::array();
  for (const safety::Clamp& one : verdict.clamped) {
    Json row = Json::object();
    row.set("channel", Json::string(one.channel));
    row.set("from", Json::number(one.from));
    row.set("to", Json::number(one.to));
    clamped.push(row);
  }
  out.set("clamped", clamped);
  out.set("advisory", Json::boolean(verdict.advisory));
  out.set("pose", shape_pose(verdict.pose));
  return out;
}

}  // namespace

Json limits(const std::string& /*label*/, const Json& input) {
  CaseParams prepared = params_from(input);
  prepared.params.frames = prepared.frames.empty() ? nullptr : &prepared.frames;
  return shape_limits(safety::resolve_limits(prepared.params));
}

Json cycle(const std::string& /*label*/, const Json& input) {
  const Json& case_limits = input["limits"];
  CaseParams prepared = params_from(case_limits);
  prepared.params.frames = prepared.frames.empty() ? nullptr : &prepared.frames;
  safety::Limits resolved = safety::resolve_limits(prepared.params);
  resolved.grants = case_limits.has("grants") && case_limits["grants"].is_array()
                        ? case_limits["grants"]
                        : Json::array();

  std::vector<kinematics::Binding> bindings;
  for (const Json& entry : input["joints"].items()) {
    kinematics::Binding binding;
    binding.joint = std::string(entry["joint"].string_or(""));
    if (entry.has("channel") && !entry["channel"].is_null()) {
      binding.channel = std::string(entry["channel"].string_or(""));
    }
    if (entry.has("measured") && !entry["measured"].is_null()) {
      binding.measured = &entry["measured"];
    }
    bindings.push_back(std::move(binding));
  }

  // The corpus writes an absent `lastSequence` as null, because JSON has no `undefined`.
  const Json* last_sequence =
      input.has("last_sequence") && !input["last_sequence"].is_null() ? &input["last_sequence"]
                                                                      : nullptr;
  return shape_verdict(safety::evaluate_cycle(input["cycle"], resolved, prepared.params.envelope,
                                              last_sequence, bindings));
}

}  // namespace skynet::contracts
