// §31.5's readiness — what a robot has declared, and what tier it could attempt.
//
// Transcribed from `reference/typescript/readiness.ts`. Every check, message and ordering decision
// is read from that file, never from `corpus/`.
//
// Pure. Every check is a question about a declared value, and **a check whose input is absent fails
// rather than passing** — the same rule `award_certification` applies to a missing test result, for
// the same reason: absence is not evidence of compliance.
//
// `attainable` is never a claim about the machine. It is the highest tier this robot could
// *attempt*, given what has been declared, and the walk **stops** at the first tier with an unmet
// requirement rather than skipping it: a robot that satisfies every certified-tier check but lacks
// a watchdog window is not "certified with a caveat", it is a robot that cannot attempt the host
// tier.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// truthy details beside strict `ok`
//     Several checks decide `ok` with `typeof x === 'number'` and choose their *sentence* with a
//     plain truthy test on the same field. A joint rated at **0** therefore passes the check and
//     gets the failure sentence. That is the reference's behaviour and it is transcribed rather
//     than reconciled — the two spellings are in the file, and quietly agreeing them here would be
//     this edition disagreeing with the specification in order to be tidy.
//
// `Object.keys(compute).length > 0`
//     A declared but empty `compute` object is not onboard compute.
//
// `limits?.estop === true`
//     Strict. A hardware stop is fitted or it is not; `1` is not a stop.
//
// `.sort()` with a comparator
//     Stable, so the blockers keep their check order within a tier.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"

#include <algorithm>
#include <cmath>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

const std::vector<std::string>& tier_order() {
  static const std::vector<std::string> order = {"guest", "host", "certified"};
  return order;
}

std::size_t tier_rank(const std::string& tier) {
  const auto found = std::find(tier_order().begin(), tier_order().end(), tier);
  return static_cast<std::size_t>(found - tier_order().begin());
}

/// `typeof x === 'number'` — false for a boolean, and **true for NaN**, which is why the checks
/// that use it pair it with a `> 0` rather than relying on it alone.
bool is_number(const Json& value) { return value.is_number(); }

/// JavaScript truthiness: `0`, `''`, `null`, `undefined`, `NaN` and `false` are not. Notably an
/// empty array and an empty object **are**.
bool truthy(const Json& value) {
  switch (value.kind()) {
    case Json::Kind::Null: return false;
    case Json::Kind::Bool: return value.is_true();
    case Json::Kind::Number: {
      const double held = value.number_or(0);
      return !(held == 0 || std::isnan(held));
    }
    case Json::Kind::String: return !value.string_or("").empty();
    default: return true;
  }
}

struct Check {
  std::string name;
  bool ok = false;
  std::string required_for;
  std::string detail;
};

std::vector<Check> evaluate(const Json& subject) {
  const Json& limits = subject["limits"];
  const bool has_limits = subject.has("limits") && !limits.is_null();
  const Json& compute = subject["compute"];
  const double adapters = subject["adapter_count"].number_or(0);
  const Json& dof = subject["dof"];
  const JsonArray& modes = subject["control_modes"].items();
  const Json& control_hz = subject["control_hz"];

  const Json& source = limits["source"];
  const Json& watchdog = limits["watchdog_ms"];
  const Json& velocity = limits["joint_velocity_max_rad_s"];
  const Json& torque = limits["joint_torque_max_nm"];

  const auto has_capability = [&](std::string_view token) {
    for (const Json& capability : subject["capabilities"].items()) {
      if (capability.string_or("") == token) {
        return true;
      }
    }
    return false;
  };
  const bool onboard =
      has_capability("platform.onboard_compute") || !compute.entries().empty();

  std::string mode_list;
  for (std::size_t at = 0; at < modes.size(); ++at) {
    if (at > 0) {
      mode_list += ", ";
    }
    mode_list += std::string(modes[at].string_or(""));
  }

  std::vector<Check> checks;
  checks.push_back({"adapter", adapters > 0, "guest",
                    adapters > 0 ? js::number_to_string(adapters) + " published adapter" +
                                       (adapters == 1 ? "" : "s") + "."
                                 : "No published adapter, so nothing can talk to this robot yet."});
  checks.push_back({"kinematics", is_number(dof) && dof.number_or(0) > 0, "guest",
                    truthy(dof) ? js::interpolate(&dof) + " degrees of freedom declared."
                                : "No degree-of-freedom count declared."});
  checks.push_back({"control_modes", !modes.empty(), "guest",
                    !modes.empty()
                        ? "Accepts " + mode_list + "."
                        : "No control modes declared, so no policy can be matched to it."});
  checks.push_back(
      {"control_rate", is_number(control_hz) && control_hz.number_or(0) > 0, "host",
       truthy(control_hz)
           ? "Control loop runs at " + js::interpolate(&control_hz) + " Hz."
           : "No control rate declared, so loop stability cannot be checked against a policy."});
  // Null and `{}` are deliberately different: nobody looked, versus somebody looked and found
  // nothing. Only the first blocks, and the message says which.
  checks.push_back({"envelope", has_limits, "host",
                    has_limits ? "A safety envelope has been declared."
                               : "No safety envelope declared. There is nothing for the kernel to "
                                 "enforce."});
  checks.push_back({"envelope_source", truthy(source), "host",
                    truthy(source) ? "Limits cite " + js::interpolate(&source) + "."
                                   : "Limits cite no datasheet. A safety limit without provenance "
                                     "is a rumour."});
  checks.push_back({"watchdog", is_number(watchdog) && watchdog.number_or(0) > 0, "host",
                    truthy(watchdog)
                        ? "Controller faults after " + js::interpolate(&watchdog) +
                              " ms without a command."
                        : "No watchdog window declared, so the watchdog guard cannot run."});
  checks.push_back({"joint_ratings", is_number(velocity) && is_number(torque), "host",
                    truthy(velocity) && truthy(torque)
                        ? "Rated to " + js::interpolate(&velocity) + " rad/s and " +
                              js::interpolate(&torque) + " Nm."
                        : "Velocity and torque ceilings are not both declared, so commands cannot "
                          "be clamped."});
  checks.push_back({"estop_software", has_capability("safety.estop"), "guest",
                    has_capability("safety.estop")
                        ? "Exposes a software emergency stop."
                        : "No software emergency stop capability declared."});
  checks.push_back({"estop_hardware", limits["estop"].is_true(), "certified",
                    limits["estop"].is_true()
                        ? "A hardware emergency stop is fitted."
                        : "No hardware emergency stop declared. The certified tier needs a stop "
                          "path off the application processor."});
  checks.push_back({"onboard_compute", onboard, "host",
                    onboard ? "Runs inference onboard."
                            : "No onboard compute declared, so the OS cannot own the control loop "
                              "here."});
  return checks;
}

/// Walk upward and **stop** at the first tier with an unmet requirement.
///
/// Stopping rather than skipping matters: tiers are cumulative, and a robot that satisfies every
/// certified-tier check but lacks a watchdog window cannot attempt the host tier.
Json highest_attainable(const std::vector<Check>& checks) {
  Json best = Json::null();
  for (const std::string& tier : tier_order()) {
    for (const Check& check : checks) {
      if (check.required_for == tier && !check.ok) {
        return best;
      }
    }
    best = Json::string(tier);
  }
  return best;
}

Json check_json(const Check& check) {
  Json out = Json::object();
  out.set("check", Json::string(check.name));
  out.set("ok", Json::boolean(check.ok));
  out.set("required_for", Json::string(check.required_for));
  out.set("detail", Json::string(check.detail));
  return out;
}

std::vector<Check> blockers_of(const std::vector<Check>& checks) {
  std::vector<Check> blockers;
  for (const Check& check : checks) {
    if (!check.ok) {
      blockers.push_back(check);
    }
  }
  // Stable, so blockers keep their check order within a tier.
  std::stable_sort(blockers.begin(), blockers.end(), [](const Check& left, const Check& right) {
    return tier_rank(left.required_for) < tier_rank(right.required_for);
  });
  return blockers;
}

}  // namespace

Json readiness(const std::string& label, const Json& input) {
  if (label.find("robot") != std::string::npos) {
    const std::vector<Check> checks = evaluate(input);
    const std::vector<Check> blockers = blockers_of(checks);

    Json check_rows = Json::array();
    std::size_t passing = 0;
    for (const Check& check : checks) {
      check_rows.push(check_json(check));
      if (check.ok) {
        ++passing;
      }
    }
    Json blocker_rows = Json::array();
    for (const Check& check : blockers) {
      blocker_rows.push(check_json(check));
    }

    Json out = Json::object();
    out.set("slug", input["slug"]);
    out.set("name", input["name"]);
    out.set("attainable", highest_attainable(checks));
    out.set("checks", check_rows);
    out.set("blockers", blocker_rows);
    out.set("completeness",
            Json::number(static_cast<double>(passing) / static_cast<double>(checks.size())));
    return out;
  }

  if (label.find("fleet") != std::string::npos) {
    // `common_blockers` is the number worth putting on a screen: it says whether the ecosystem's
    // problem is missing adapters or missing datasheets, and those imply completely different work.
    Json by_tier = Json::object();
    double none = 0;
    double guest = 0;
    double host = 0;
    double certified = 0;
    // Insertion-ordered, because the sort below is stable and ties therefore keep first-seen order.
    std::vector<std::pair<std::string, double>> counts;
    double total = 0;

    for (const Json& subject : input["subjects"].items()) {
      ++total;
      const std::vector<Check> checks = evaluate(subject);
      const Json attainable = highest_attainable(checks);
      const std::string tier =
          attainable.is_null() ? "none" : std::string(attainable.string_or(""));
      if (tier == "none") ++none;
      else if (tier == "guest") ++guest;
      else if (tier == "host") ++host;
      else ++certified;

      for (const Check& blocker : blockers_of(checks)) {
        auto found = std::find_if(counts.begin(), counts.end(), [&](const auto& entry) {
          return entry.first == blocker.name;
        });
        if (found == counts.end()) {
          counts.emplace_back(blocker.name, 1);
        } else {
          found->second += 1;
        }
      }
    }

    by_tier.set("none", Json::number(none));
    by_tier.set("guest", Json::number(guest));
    by_tier.set("host", Json::number(host));
    by_tier.set("certified", Json::number(certified));

    std::stable_sort(counts.begin(), counts.end(),
                     [](const auto& left, const auto& right) { return left.second > right.second; });
    Json common = Json::array();
    for (const auto& [check, robots] : counts) {
      Json row = Json::object();
      row.set("check", Json::string(check));
      row.set("robots", Json::number(robots));
      common.push(row);
    }

    Json out = Json::object();
    out.set("total", Json::number(total));
    out.set("by_tier", by_tier);
    out.set("common_blockers", common);
    return out;
  }

  throw Unimplemented("readiness", label);
}

}  // namespace skynet::contracts
