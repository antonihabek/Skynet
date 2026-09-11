// §11 and §17's check-in — the one thing a robot says about itself. C++ edition.
//
// Transcribed from `reference/typescript/checkin.ts` and the shapes `reference/typescript/shape.ts`
// puts on it. Every rule and message string is read from those files, never from `corpus/`.
//
// §17's rule is the whole of it: *the platform never claims a robot is running something it has not
// heard from.* So `applied` is derived from what is left to do rather than set by the daemon, the
// generation on the wire is the one the robot applied and never the one it was asked for, and a
// health reading with no age is not reported as a value.
//
// Three corpus files, because there are three separable ways to be wrong: `claim` is the field §17
// lives on, `health` is §33.6's age rule, and `document` is the bytes that cross to the Hub — an
// edition could reproduce the first two and still assemble them differently.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `[...new Set(...)].sort()`, and `a.stage < b.stage`
//     UTF-16 code-unit order, `js::utf16_less`. The pending list is sorted with a comparator and
//     JavaScript's sort is stable, so equal stages keep the order they arrived in — `stable_sort`.
//
// `entry.id` with no default
//     A package with no id reaches the shape as `undefined`, and a canonical writer omits an
//     undefined key rather than writing null. So an absent id is absent here too.
//
// `x ?? default`
//     Takes the default for null **and** for absent. For a field the reference types as a bare
//     `number`, a null in a case's input is NaN instead — see `js::bare_number`.
//
// `Number.isFinite`
//     False for anything that is not a number, a string of digits included.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/identity.hpp"
#include "skynet_os/js.hpp"

#include <algorithm>
#include <array>
#include <cmath>
#include <string>
#include <string_view>
#include <vector>

namespace skynet::contracts {
namespace {

/// The named aggregate fields, in the order §33.6 lists them.
constexpr std::array<std::string_view, 5> kHealthFields = {
    "generation_lag", "guard_trip_rate", "storage_headroom", "battery", "temperature_c",
};

bool utf16_before(const std::string& left, const std::string& right) {
  return js::utf16_less(left, right);
}

/// `Array.prototype.join(', ')`.
std::string joined(const std::vector<std::string>& items) {
  std::string out;
  for (std::size_t at = 0; at < items.size(); ++at) {
    if (at > 0) {
      out += ", ";
    }
    out += items[at];
  }
  return out;
}

Json string_array(const std::vector<std::string>& items) {
  Json out = Json::array();
  for (const std::string& item : items) {
    out.push(Json::string(item));
  }
  return out;
}

/// `value ?? fallback` — the value as it arrived, or the fallback for null and for absent.
Json or_default(const Json& value, Json fallback) { return value.is_null() ? fallback : value; }

/// `(value ?? '').trim()`.
std::string trimmed_text(const Json& value) {
  return value.is_null() ? "" : js::trim(js::interpolate(&value));
}

// ── The claim ────────────────────────────────────────────────────────────────

/// What this robot is entitled to claim (§17, §33.3, §33.4). The order of the tests is the order of
/// severity; an empty plan with disagreeing generations is a supervisor that cannot see the
/// difference it is meant to close, and it gets a name rather than a green tick.
Json claim_for(const Json& progress) {
  const double observed = js::bare_number(progress, "observed_generation", 0);
  const double desired = js::bare_number(progress, "desired_generation", 0);
  const JsonArray& pending = progress["pending"].items();

  std::vector<std::string> blocking;
  for (const Json& entry : pending) {
    blocking.push_back(js::interpolate(&entry["stage"]));
  }
  std::sort(blocking.begin(), blocking.end(), utf16_before);
  blocking.erase(std::unique(blocking.begin(), blocking.end()), blocking.end());

  std::string claim;
  std::string detail;
  if (!pending.empty()) {
    const bool deferred = std::all_of(pending.begin(), pending.end(),
                                      [](const Json& entry) { return entry["deferred"].is_true(); });
    if (deferred) {
      claim = "deferred";
      detail = "Everything left to do is waiting for a safe state (§33.4): " + joined(blocking) +
               ". Nothing here converges until the robot is put in one, which is an operator action "
               "rather than a matter of time.";
    } else {
      claim = "converging";
      detail = "Still reconciling generation " + js::number_to_string(desired) +
               ", with work left at " + joined(blocking) + ". This robot is running generation " +
               js::number_to_string(observed) + ".";
    }
  } else if (observed != desired) {
    claim = "inconsistent";
    detail = "The plan is empty and this robot has applied generation " +
             js::number_to_string(observed) + " against a desired " +
             js::number_to_string(desired) +
             ". An empty plan that closes nothing is a supervisor that cannot see the difference it "
             "is meant to close, and reporting it as applied would put a green tick over it.";
  } else {
    claim = "applied";
    detail = "Generation " + js::number_to_string(observed) + " is applied and the plan is empty.";
  }

  // `shapeClaimVerdict`. The generation is the one the robot applied — never the desired one.
  Json out = Json::object();
  out.set("applied", Json::boolean(claim == "applied"));
  out.set("blocking", string_array(blocking));
  out.set("claim", Json::string(claim));
  out.set("detail", Json::string(detail));
  out.set("generation", Json::number(observed));
  return out;
}

// ── §33.6's health, and the age rule ─────────────────────────────────────────

bool is_finite_number(const Json& value) {
  return value.is_number() && std::isfinite(value.number_or(0));
}

/// Roll health up for a check-in (§33.6), in `shapeHealthReport`'s shape.
///
/// A stalled package is degraded and an unmeasured field is not; a value with no age is dropped and
/// named in `undated` rather than passed through as current; `unknown` readiness is not a stall.
Json summarize(const Json& health) {
  Json packages = Json::array();
  std::vector<std::string> stalled;
  for (const Json& entry : health["packages"].items()) {
    const Json readiness = or_default(entry["readiness"], Json::string("unknown"));
    Json row = Json::object();
    row.set("age_ms", Json::number(js::bare_number(entry, "age_ms", 0)));
    if (entry.has("id")) {
      row.set("id", entry["id"]);
    }
    row.set("live", or_default(entry["live"], Json::boolean(false)));
    row.set("readiness", readiness);
    packages.push(row);
    if (readiness.is_string() && readiness.string_or("") == "stalled") {
      stalled.push_back(js::interpolate(&entry["id"]));
    }
  }
  std::sort(stalled.begin(), stalled.end(), utf16_before);

  std::vector<std::string> unmeasured;
  std::vector<std::string> undated;
  Json fields = Json::object();
  for (const std::string_view name : kHealthFields) {
    const Json& measured = health[name];
    const Json& value = measured["value"];
    const Json& age = measured["age_ms"];
    if (!is_finite_number(value)) {
      unmeasured.emplace_back(name);
      fields.set(std::string(name), Json::null());
      continue;
    }
    if (!is_finite_number(age) || age.number_or(0) < 0) {
      undated.emplace_back(name);
      fields.set(std::string(name), Json::null());
      continue;
    }
    Json reading = Json::object();
    reading.set("age_ms", age);
    reading.set("value", value);
    fields.set(std::string(name), reading);
  }
  std::sort(unmeasured.begin(), unmeasured.end());
  std::sort(undated.begin(), undated.end());

  Json out = Json::object();
  out.set("degraded", Json::boolean(!stalled.empty()));
  out.set("fields", fields);
  out.set("packages", packages);
  out.set("stalled", string_array(stalled));
  out.set("undated", string_array(undated));
  out.set("unmeasured", string_array(unmeasured));
  return out;
}

// ── The document ─────────────────────────────────────────────────────────────

/// `shapeGuardReport`: §7.6's limits, carried through rather than derived — what the guards are and
/// what they lack is the kernel's answer for this body, and recomputing any of it here would be a
/// second implementation of §7.3 living in the reporting path. Re-keyed field by field, so a field
/// the report does not carry is absent rather than invented.
Json guard_report(const Json& report) {
  if (report.is_null()) {
    return Json::null();
  }
  Json out = Json::object();
  const auto copy = [&](const char* key) {
    if (report.has(key)) {
      out.set(key, report[key]);
    }
  };
  const auto list = [&](const char* key, const auto& row) {
    if (!report.has(key)) {
      return;
    }
    Json items = Json::array();
    for (const Json& item : report[key].items()) {
      items.push(row(item));
    }
    out.set(key, items);
  };
  const auto pick = [](const Json& item, std::initializer_list<const char*> keys) {
    Json row = Json::object();
    for (const char* key : keys) {
      if (item.has(key)) {
        row.set(key, item[key]);
      }
    }
    return row;
  };

  list("active", [](const Json& item) { return item; });
  copy("base_velocity_max_m_s");
  copy("enforced");
  copy("enforcement");
  copy("joint_torque_max_nm");
  copy("joint_velocity_max_rad_s");
  copy("max_delta_per_cycle");
  copy("self_collision_margin_m");
  copy("min_control_hz");
  list("notices", [&](const Json& item) { return pick(item, {"code", "subjects"}); });
  copy("resolved_at");
  copy("source");
  list("unavailable", [&](const Json& item) { return pick(item, {"guard", "missing"}); });
  list("inert", [&](const Json& item) { return pick(item, {"guard", "reading"}); });
  return out;
}

/// Build the document a robot sends (§11, §17), in `shapeCheckIn`'s shape. It cannot fail, for
/// `attest`'s reason: the robot most in need of a check-in is the one that cannot measure everything.
Json check_in(const Json& input) {
  const Json& progress = input["progress"];
  const Json claim = claim_for(progress);

  struct Pending {
    Json stage;
    bool has_stage;
    std::string order;
    Json deferred;
    Json reason;
  };
  std::vector<Pending> pending;
  for (const Json& entry : progress["pending"].items()) {
    pending.push_back({entry["stage"], entry.has("stage"), js::interpolate(&entry["stage"]),
                       or_default(entry["deferred"], Json::boolean(false)),
                       or_default(entry["reason"], Json::string(""))});
  }
  std::stable_sort(pending.begin(), pending.end(), [](const Pending& left, const Pending& right) {
    return js::utf16_less(left.order, right.order);
  });
  Json pending_json = Json::array();
  for (const Pending& entry : pending) {
    Json row = Json::object();
    row.set("deferred", entry.deferred);
    row.set("reason", entry.reason);
    if (entry.has_stage) {
      row.set("stage", entry.stage);
    }
    pending_json.push(row);
  }

  Json out = Json::object();
  out.set("agent", Json::string(trimmed_text(input["agent"])));
  out.set("applied", claim["applied"]);
  out.set("attestation", identity::attestation_block(input["attestation"]));
  out.set("blocking", claim["blocking"]);
  out.set("claim", claim["claim"]);
  out.set("contracts", Json::string(trimmed_text(input["contracts"])));
  out.set("desired_digest", Json::string(trimmed_text(input["desired_digest"])));
  out.set("detail", claim["detail"]);
  out.set("device_id", Json::string(trimmed_text(input["device_id"])));
  out.set("generation", claim["generation"]);
  out.set("health", summarize(input["health"]));
  out.set("limits", guard_report(input["limits"]));
  out.set("pending", pending_json);
  out.set("state", Json::string(trimmed_text(input["state"])));
  return out;
}

}  // namespace

Json checkin(const std::string& label, const Json& input) {
  if (label == "claim") {
    return claim_for(input);
  }
  if (label == "health") {
    return summarize(input);
  }
  if (label == "document") {
    return check_in(input);
  }
  throw Unimplemented("checkin", label);
}

}  // namespace skynet::contracts
