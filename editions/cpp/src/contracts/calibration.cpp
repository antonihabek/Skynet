// Calibration validity — §45, C++ edition.
//
// Transcribed from `reference/typescript/calibration.ts`. Every rule and message string is read
// from that file, never from `corpus/`.
//
// Calibration is the only state in this system that is **safety-relevant, individual to one
// machine, and drifts over time**. The descriptor is signed and static; profiles are signed and
// operator-set. Calibration is measured, it decays, and several guards are built on it.
//
// The failure this contract exists to prevent is stated in §45.1 and it is not a dramatic one:
//
//     a camera extrinsic off by four centimetres means the `proximity` guard measures separation to
//     the wrong place while reporting perfectly healthy.
//
// Nothing trips. Nothing looks wrong. The number is simply about the world as it was last spring.
//
// ── Expiry degrades; it never silently continues ───────────────────────────
//
// §45.5, which is §7.2's rule applied to a measurement rather than to a sensor: an expired record
// makes the guards that depend on it report **unavailable, naming the expired parameter set**. Not
// "assume it is probably still fine", which is what every system without an expiry policy does by
// omission — not by decision, which is the point.
//
// ── A residual is a threshold, not a decoration ────────────────────────────
//
// §45.3: *"a hand-eye calibration with twelve millimetres of reprojection error is a failed
// calibration, and accepting it silently produces a workspace guard that is precisely twelve
// millimetres wrong forever."* A record whose residual exceeds its own declared threshold is
// `failed` the moment it is read, at any age. It never becomes valid and it never expires — it was
// never a calibration.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `toFixed`
//     Every number in every detail sentence goes through it, and the sentences are compared byte
//     for byte. Ties go **away from zero** in ECMAScript, so `18.5 °C` renders as `19` rather than
//     as the `18` a round-half-to-even printf would produce.
//
// `if (!record.method)`
//     Falsy, so an empty string is as missing as a null. That is the reading §45.3 wants: a method
//     of `""` is not a method somebody recorded.
//
// `Number.isFinite`
//     Rejects NaN, the infinities and booleans. A `max_age_s` of `true` is not a validity of one
//     second.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/text.hpp"
#include "skynet_os/vocabulary.hpp"

#include <algorithm>
#include <cmath>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

/// §45.2's table, as data: which guards lose their input when a set expires.
///
/// **This table is the deliverable of §45.2.** It converts "we should calibrate the cameras
/// sometime" into "an expired extrinsic disables the proximity guard", which is a sentence an
/// operator can act on and a fleet console can count.
///
/// `camera_intrinsics` feeds `proximity` *indirectly*, through projection, and is listed rather
/// than omitted: a guard whose input is two transforms away is still measuring the wrong thing when
/// either is wrong.
const std::vector<std::string>& guards_fed_by(std::string_view parameter_set) {
  static const std::vector<std::string> none;
  static const std::vector<std::string> joint_zeros = {"workspace", "self_collision"};
  static const std::vector<std::string> camera_intrinsics = {"proximity"};
  static const std::vector<std::string> camera_extrinsics = {"proximity", "workspace"};
  static const std::vector<std::string> force_torque_bias = {"envelope"};
  static const std::vector<std::string> imu_bias = {"balance"};
  static const std::vector<std::string> tool_offset = {"workspace"};
  static const std::vector<std::string> gravity_vector = {"balance", "envelope"};
  if (parameter_set == "joint_zeros") return joint_zeros;
  if (parameter_set == "camera_intrinsics") return camera_intrinsics;
  if (parameter_set == "camera_extrinsics") return camera_extrinsics;
  if (parameter_set == "force_torque_bias") return force_torque_bias;
  if (parameter_set == "imu_bias") return imu_bias;
  if (parameter_set == "tool_offset") return tool_offset;
  if (parameter_set == "gravity_vector") return gravity_vector;
  return none;
}

/// The parameter sets §45.2 names, in the order that table gives them. A closed vocabulary rather
/// than a free string, for the reason §26's event codes are closed: an open one is how
/// `camera_extrinsics` and `camera-extrinsic` end up naming the same measurement on two robots in
/// one fleet, and the guard mapping above then answers differently for each.
const std::vector<std::string>& parameter_sets() {
  static const std::vector<std::string> sets = {
      "joint_zeros",       "camera_intrinsics", "camera_extrinsics", "force_torque_bias",
      "imu_bias",          "tool_offset",       "gravity_vector"};
  return sets;
}

struct Validity {
  std::string state;
  std::string detail;
  std::string reason;
  std::vector<std::string> checked;
};

/// `value !== null && value !== undefined && Number.isFinite(value)`.
bool present(const Json& value) {
  return value.is_number() && std::isfinite(value.number_or(0));
}

std::string detail_of(const Validity& validity) {
  if (validity.state == "valid") {
    return "";
  }
  if (validity.state == "absent") {
    // Phrased to compose: this is read on its own in a table row that already names the set, and
    // inside "the `set` calibration is absent — …" in the guard list.
    return "nobody has ever measured it on this robot";
  }
  return validity.detail;
}

/// Judge one record against the present (§45.4, §45.5).
///
/// ── Order, and why it is this order ───────────────────────────────────────
///
///   1. **Provenance**, because §45.3 makes it mandatory and a record without a method cannot be
///      judged on anything else — "valid for another 30 days" would be a claim about a measurement
///      nobody described.
///   2. **Residual**, because a failed calibration was never valid and its age is irrelevant.
///   3. **Events**, because §45.4 calls them the triggers that matter and they are a step rather
///      than a slope: a robot that hit something has a bad extrinsic *now*, whatever its age says.
///   4. Then the three gradual conditions, in the order §45.4's table gives them.
///
/// A missing *current* reading does not extend validity. If the policy names a temperature delta
/// and the robot cannot say what temperature it is, the record expires — the alternative is a rule
/// that silently stops applying on exactly the robot whose sensor has failed.
Validity evaluate(const Json& record, const Json& now) {
  const Json& policy = record["policy"];
  const Json& taken = record["conditions"];

  std::vector<std::string> missing;
  // Falsy, not absent: a method of `""` is not a method somebody recorded.
  if (!record["method"].is_string() || record["method"].string_or("").empty()) {
    missing.emplace_back("method");
  }
  if (!present(record["residual"])) {
    missing.emplace_back("residual");
  }
  if (!missing.empty()) {
    std::string joined;
    for (std::size_t at = 0; at < missing.size(); ++at) {
      if (at > 0) {
        joined += " and no ";
      }
      joined += missing[at];
    }
    return {"unusable",
            "§45.3 makes provenance mandatory and this record has no " + joined +
                ". A calibration whose method nobody recorded is a rumour with six decimal places",
            "",
            {}};
  }

  const Json& residual = record["residual"];
  const Json& threshold = record["residual_threshold"];
  if (present(residual) && present(threshold) && residual.number_or(0) > threshold.number_or(0)) {
    return {"failed",
            "the method reported a residual of " + js::interpolate(&residual) +
                " against its own threshold of " + js::interpolate(&threshold) +
                ", so this was a failed calibration rather than one that has since expired",
            "",
            {}};
  }

  for (const Json& event : record["events_since"].items()) {
    for (const Json& invalidating : policy["invalidating_events"].items()) {
      if (event.kind() == invalidating.kind() && event.canonical() == invalidating.canonical()) {
        return {"expired",
                "a `" + js::interpolate(&event) +
                    "` was recorded against this set since it was measured, and §45.4 invalidates "
                    "it outright: continuing to trust the previous values is exactly the wrong "
                    "response to one",
                "event",
                {}};
      }
    }
  }

  std::vector<std::string> checked;

  const Json& max_age = policy["max_age_s"];
  if (present(max_age)) {
    if (present(taken["at_s"]) && present(now["at_s"])) {
      const double age = now["at_s"].number_or(0) - taken["at_s"].number_or(0);
      if (age > max_age.number_or(0)) {
        return {"expired",
                "measured " + js::to_fixed(age, 0) + " s ago against a validity of " +
                    js::to_fixed(max_age.number_or(0), 0) + " s",
                "age",
                {}};
      }
      checked.push_back("age " + js::to_fixed(age, 0) + "s of " +
                        js::to_fixed(max_age.number_or(0), 0) + "s");
    } else {
      return {"expired",
              "this record expires with time and either it or this robot cannot say when it was "
              "taken, so its age cannot be established",
              "age",
              {}};
    }
  }

  const Json& max_travel = policy["max_joint_travel_rad"];
  if (present(max_travel)) {
    if (present(taken["joint_travel_rad"]) && present(now["joint_travel_rad"])) {
      const double travelled =
          now["joint_travel_rad"].number_or(0) - taken["joint_travel_rad"].number_or(0);
      if (travelled > max_travel.number_or(0)) {
        return {"expired",
                js::to_fixed(travelled, 1) + " rad of joint travel since it was measured, against "
                                             "a validity of " +
                    js::to_fixed(max_travel.number_or(0), 1) + " rad",
                "joint_travel",
                {}};
      }
      checked.push_back("travel " + js::to_fixed(travelled, 1) + " of " +
                        js::to_fixed(max_travel.number_or(0), 1) + " rad");
    } else {
      return {"expired",
              "this record expires with joint travel and no travel counter is available, so how "
              "far this robot has moved since cannot be established",
              "joint_travel",
              {}};
    }
  }

  const Json& max_delta = policy["max_temperature_delta_c"];
  if (present(max_delta)) {
    if (present(taken["temperature_c"]) && present(now["temperature_c"])) {
      const double delta =
          std::fabs(now["temperature_c"].number_or(0) - taken["temperature_c"].number_or(0));
      if (delta > max_delta.number_or(0)) {
        return {"expired",
                js::to_fixed(delta, 1) + " °C from the " +
                    js::to_fixed(taken["temperature_c"].number_or(0), 1) +
                    " °C it was measured at, against a validity of " +
                    js::to_fixed(max_delta.number_or(0), 1) + " °C",
                "temperature",
                {}};
      }
      checked.push_back("temperature \xce\x94" + js::to_fixed(delta, 1) + " of " +
                        js::to_fixed(max_delta.number_or(0), 1) + " °C");
    } else {
      return {"expired",
              "this record expires with temperature and no reading is available, so how far this "
              "robot has drifted from its calibration conditions cannot be established",
              "temperature",
              {}};
    }
  }

  return {"valid", "", "", checked};
}

/// The sets a body can actually hold, from what it has declared (§45.2).
///
/// Derived from the capability tokens and the base kind a descriptor already carries rather than
/// from a new field, so a robot does not declare the same fact twice.
///
/// ── Two different absences, and they are not treated the same ─────────────
///
/// **Undeclared capabilities** mean nobody said what this body senses, so every sensing-derived set
/// is included: over-reporting what is missing is the conservative direction, and quietly deciding
/// a guard was never relevant is not.
///
/// **A declared `fixed` base** is not an absence at all — it is a positive statement that this body
/// cannot fall over. `balance` is meaningless on a bolted-down arm, so an IMU bias and a gravity
/// vector are not measurements it is missing; they are measurements it will never take, and
/// reporting `balance` unavailable on one is a permanent red mark nobody can clear.
///
/// An **unknown** base is mobile, because that is the absence again and it goes the other way. A
/// body that does declare an IMU keeps both sets whatever its base, since it evidently has one.
std::vector<std::string> sets_for(const Json& capabilities, const Json& base) {
  const bool mobile = base.is_null() || base.string_or("") != "fixed";
  const bool declared = !capabilities.items().empty();
  const auto has = [&](std::string_view prefix) {
    for (const Json& capability : capabilities.items()) {
      if (capability.is_string() && text::starts_with(capability.string_or(""), prefix)) {
        return true;
      }
    }
    return false;
  };

  std::vector<std::string> sets = {"joint_zeros"};
  if (!declared || has("sensing.rgb") || has("sensing.depth") || has("sensing.camera")) {
    sets.emplace_back("camera_intrinsics");
    sets.emplace_back("camera_extrinsics");
  }
  if (!declared || has("sensing.force") || has("manipulation")) {
    sets.emplace_back("force_torque_bias");
    sets.emplace_back("tool_offset");
  }
  if (mobile || has("sensing.imu")) {
    sets.emplace_back("imu_bias");
    sets.emplace_back("gravity_vector");
  }
  return sets;
}

}  // namespace

Json calibration(const std::string& label, const Json& input) {
  if (label.find("sets") != std::string::npos) {
    Json list = Json::array();
    for (const std::string& one : sets_for(input["capabilities"], input["base"])) {
      list.push(Json::string(one));
    }
    Json out = Json::object();
    out.set("sets", list);
    return out;
  }

  const Json& now = input["now"];
  std::vector<std::pair<std::string, Validity>> records;
  for (const Json& record : input["records"].items()) {
    records.emplace_back(std::string(record["set"].string_or("")), evaluate(record, now));
  }

  std::vector<std::string> applicable;
  if (input.has("applicable") && !input["applicable"].is_null()) {
    for (const Json& one : input["applicable"].items()) {
      applicable.emplace_back(one.string_or(""));
    }
  } else {
    applicable = parameter_sets();
  }

  Json record_rows = Json::array();
  for (const auto& [parameter_set, validity] : records) {
    Json row = Json::object();
    row.set("set", Json::string(parameter_set));
    row.set("state", Json::string(validity.state));
    row.set("detail", Json::string(detail_of(validity)));
    row.set("reason", validity.state == "expired" ? Json::string(validity.reason) : Json::null());
    Json checked = Json::array();
    if (validity.state == "valid") {
      for (const std::string& one : validity.checked) {
        checked.push(Json::string(one));
      }
    }
    row.set("checked", checked);
    Json guards = Json::array();
    for (const std::string& guard : guards_fed_by(parameter_set)) {
      guards.push(Json::string(guard));
    }
    row.set("guards", guards);
    record_rows.push(row);
  }

  // One entry per (guard, set) pair, deliberately: `workspace` disabled by expired joint zeros *and*
  // an expired tool offset is two measurements somebody has to take, and collapsing them to one line
  // would hide the second until the first was done.
  //
  // `applicable` names the sets that mean anything on this body — a fixed-base arm with no IMU
  // should not report `balance` unavailable for want of an IMU bias it can never have.
  Json unavailable = Json::array();
  for (const std::string& parameter_set : applicable) {
    const auto held = std::find_if(records.begin(), records.end(), [&](const auto& entry) {
      return entry.first == parameter_set;
    });
    const Validity validity = held == records.end() ? Validity{"absent", "", "", {}} : held->second;
    if (validity.state == "valid") {
      continue;
    }
    for (const std::string& guard : guards_fed_by(parameter_set)) {
      Json row = Json::object();
      row.set("guard", Json::string(guard));
      row.set("set", Json::string(parameter_set));
      row.set("state", Json::string(validity.state));
      row.set("detail", Json::string("the `" + parameter_set + "` calibration is " +
                                     validity.state + " — " + detail_of(validity)));
      unavailable.push(row);
    }
  }

  Json out = Json::object();
  out.set("records", record_rows);
  out.set("unavailable", unavailable);
  return out;
}

}  // namespace skynet::contracts
