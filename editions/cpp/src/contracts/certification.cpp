// §15's certification award, C++ edition.
//
// Transcribed from `awardCertification` and its table in `reference/typescript/conformance.ts`.
// Every id, threshold and message string is read from that file, never from `corpus/`.
//
// Two rules make this worth having.
//
// **Absence is not a pass.** A required test with no result is a `missing`, never a silent success
// — the loophole every self-certification scheme eventually dies of.
//
// **The award is capped by the evidence, not by the claim.** A vendor claiming the certified tier
// whose independent stop path was never measured is awarded `host`, with the reason recorded,
// rather than refused outright. Downgrading tells the truth and keeps the vendor in the programme;
// refusing sends them to go write "ROS 2 compatible" on a web page instead, which helps nobody.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `toFixed(n)`
//     ECMAScript negates first and then picks the integer nearest `x × 10ⁿ`, **the larger on a
//     tie** — ties away from zero, decided on the exact binary value. `js::to_fixed` is that rule;
//     a bare printf rounds half to even. The figure lands in a suspension notice an operator reads.
//
// `indexOf` returning `-1`
//     An unrecognised tier makes `claimedIndex` `-1`, so the evidence walk runs **zero** times and
//     the verdict is `none` with no reasons at all — a refusal rather than a crash.
//
// `new Map(entries)`
//     Later entries overwrite earlier ones, so two results for one test id keep the **last**.
//
// strict `undefined`
//     `field_conformance`, `bench_age_days` and `simulator` are all tested strictly. A field
//     present and null takes the other branch.

#include "skynet_os/contracts.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/vocabulary.hpp"

#include <algorithm>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

/// A year, because bench testing a humanoid is expensive and a shorter window would make
/// certification something vendors avoid rather than seek. Field conformance is what carries the
/// claim in between — measured continuously, and able to revoke the award long before the year is
/// up.
constexpr double kBenchValidityDays = 365;

/// 0.999 rather than 1.0: a fleet of a hundred robots at 100 Hz produces tens of millions of cycles
/// a day, and demanding literal perfection would suspend every honest vendor on sensor noise while
/// telling operators nothing. What this catches is a *systematic drift* — a robot whose real
/// envelope has moved away from its declared one.
constexpr double kFieldConformanceFloor = 0.999;

/// Every SCTS test, in declaration order, as `(id, required_from)`.
///
/// Only the two fields the award reads. Declaration order is load-bearing: `missing` and `failures`
/// are reported in it, and the corpus compares those arrays element by element.
const std::vector<std::pair<std::string, std::string>>& scts_tests() {
  static const std::vector<std::pair<std::string, std::string>> tests = {
      {"rhal.abi.version", "guest"},
      {"rhal.state.completeness", "guest"},
      {"rhal.state.rate", "host"},
      {"rhal.command.latency", "host"},
      {"rhal.command.rejection", "guest"},
      {"rhal.sensor.latency", "host"},
      {"embodiment.descriptor.valid", "guest"},
      {"embodiment.descriptor.signed", "host"},
      {"embodiment.limits.sourced", "host"},
      {"embodiment.limits.honest", "certified"},
      {"embodiment.roles.coverage", "host"},
      {"embodiment.capabilities.declared", "host"},
      {"embodiment.frames.complete", "host"},
      {"safety.kernel.parity", "guest"},
      {"safety.watchdog.fires", "host"},
      {"safety.envelope.clamps", "host"},
      {"safety.estop.latency", "host"},
      {"safety.estop.independent", "certified"},
      {"safety.ratelimit.enforced", "host"},
      {"safety.command_loss.behaviour", "host"},
      {"safety.homing.envelope", "host"},
      {"safety.policy.isolation", "guest"},
      {"package.signature.required", "guest"},
      {"package.budget.enforced", "host"},
      {"package.grants.gate", "guest"},
      {"ota.rollback.works", "host"},
      {"ota.safe_state.respected", "host"},
      {"ota.safety_gate", "host"},
      {"telemetry.blackbox.complete", "host"},
      {"telemetry.replay.deterministic", "host"},
      {"telemetry.blackbox.survives", "certified"},
      {"offline.control.independent", "guest"},
      {"offline.reconcile.resumes", "host"},
      {"security.identity.attested", "host"},
      {"security.credential.scope", "guest"},
      {"power.safe_state.reachable", "host"},
  };
  return tests;
}

int tier_index(std::string_view tier) {
  const vocabulary::Set& order = vocabulary::members("os_tiers");
  const auto found = std::find(order.begin(), order.end(), tier);
  return found == order.end() ? -1 : static_cast<int>(found - order.begin());
}

/// Tests required at a tier: its own, plus every tier below it.
std::vector<std::string> tests_required_for(const std::string& tier) {
  const int ceiling = tier_index(tier);
  std::vector<std::string> out;
  for (const auto& [test_id, required_from] : scts_tests()) {
    if (tier_index(required_from) <= ceiling) {
      out.push_back(test_id);
    }
  }
  return out;
}

}  // namespace

Json certification(const std::string& /*label*/, const Json& input) {
  std::vector<std::string> reasons;
  std::string best = "none";
  std::vector<std::string> failures;
  std::vector<std::string> missing;

  // Later entries overwrite earlier ones, as `new Map(entries)` does: two results for one test id
  // keep the last.
  std::vector<std::pair<std::string, const Json*>> by_id;
  for (const Json& result : input["results"].items()) {
    const std::string test_id(result["test_id"].string_or(""));
    auto found = std::find_if(by_id.begin(), by_id.end(),
                              [&](const auto& entry) { return entry.first == test_id; });
    if (found == by_id.end()) {
      by_id.emplace_back(test_id, &result);
    } else {
      found->second = &result;
    }
  }
  const auto result_for = [&](const std::string& test_id) -> const Json* {
    const auto found = std::find_if(by_id.begin(), by_id.end(),
                                    [&](const auto& entry) { return entry.first == test_id; });
    return found == by_id.end() ? nullptr : found->second;
  };

  // `indexOf` on an unrecognised tier is -1, so the walk runs zero times and the verdict is `none`
  // with no reasons — a refusal rather than an exception.
  const std::string claimed(input["claimed"].string_or(""));
  const int claimed_index = tier_index(claimed);
  const vocabulary::Set& tier_order = vocabulary::members("os_tiers");

  // Walk upward and keep the highest tier fully satisfied, so the evidence sets the ceiling.
  for (int index = 0; index <= claimed_index; ++index) {
    const std::string tier(tier_order[static_cast<std::size_t>(index)]);
    std::vector<std::string> tier_failures;
    std::vector<std::string> tier_missing;

    for (const std::string& test_id : tests_required_for(tier)) {
      const Json* result = result_for(test_id);
      const std::string outcome =
          result == nullptr ? "" : std::string((*result)["outcome"].string_or(""));
      if (result == nullptr || outcome == "skipped") {
        tier_missing.push_back(test_id);
      } else if (outcome == "fail") {
        tier_failures.push_back(test_id);
      }
      // `not_applicable` passes: the balance guard on a wheeled base is genuinely inapplicable, and
      // forcing a fail there would push vendors to fake a result rather than declare a shape.
    }

    if (tier_failures.empty() && tier_missing.empty()) {
      best = tier;
      failures.clear();
      missing.clear();
    } else {
      failures = tier_failures;
      missing = tier_missing;
      reasons.push_back(tier + ": " + std::to_string(tier_failures.size()) + " failing, " +
                        std::to_string(tier_missing.size()) + " unmeasured.");
      break;
    }
  }

  if (best != "none" && claimed != best) {
    reasons.push_back("Claimed " + claimed + "; the evidence supports " + best + ".");
  }

  // §35.3 and §70: a simulator RHAL driver declares itself one, and the declaration caps the award
  // at guest — not because a simulated run is worthless, but because no simulated run may ever be
  // presented as bench evidence, and a `host` badge on a body that does not exist is exactly that.
  //
  // Applied *after* the evidence walk rather than by refusing to walk, so the verdict still records
  // which tier the tests would have supported. "The evidence supports host; the body is simulated,
  // so guest" is a more useful sentence to a vendor than a silent guest.
  if (input["simulator"].is_true() && (best == "host" || best == "certified")) {
    reasons.push_back("The body is simulated (§35.3), so the award is capped at guest; the "
                      "evidence supports " +
                      best + ".");
    best = "guest";
  }

  // Field conformance suspends an award rather than lowering it. A robot whose commands have been
  // leaving its envelope is not a lower-tier robot — it is one whose certificate no longer
  // describes it, and the honest state is suspended pending re-measurement.
  const bool has_conformance = input.has("field_conformance");
  if (has_conformance && input["field_conformance"].number_or(0) < kFieldConformanceFloor &&
      best != "none") {
    reasons.push_back("Field conformance " +
                      js::to_fixed(input["field_conformance"].number_or(0) * 100, 3) +
                      "% is below the " + js::to_fixed(kFieldConformanceFloor * 100, 1) +
                      "% floor; the award is suspended pending re-measurement.");
    best = "none";
  }

  Json expires = Json::null();
  if (best != "none" && input.has("bench_age_days")) {
    double remaining = kBenchValidityDays - input["bench_age_days"].number_or(0);
    if (remaining <= 0) {
      reasons.push_back("Bench results are older than a year; the award has lapsed.");
      best = "none";
      remaining = 0;
    }
    expires = Json::number(remaining);
  }

  const auto string_array = [](const std::vector<std::string>& values) {
    Json out = Json::array();
    for (const std::string& value : values) {
      out.push(Json::string(value));
    }
    return out;
  };

  Json out = Json::object();
  out.set("awarded", Json::string(best));
  out.set("failures", string_array(failures));
  out.set("missing", string_array(missing));
  out.set("reasons", string_array(reasons));
  out.set("expires_in_days", expires);
  return out;
}

}  // namespace skynet::contracts
