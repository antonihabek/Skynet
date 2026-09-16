// §36's power and thermal assessment, C++ edition.
//
// Transcribed from `reference/typescript/power.ts`. Every constant, rule and message string is read
// from that file, never from `corpus/`.
//
// Two ideas hold the contract together.
//
// **A state and a fault answer different questions**, and an operator needs both: the state says
// how much energy is left, and a fault says whether that number can be believed. So they are
// computed separately and reported side by side rather than collapsed into one worse number.
//
// **Every absence produces a named unavailability rather than an assumption.** An empty
// `brownout_fault_codes` means *this vendor has not said which of its codes mean sag* — not *this
// body cannot brown out*.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `Number.isFinite`
//     False for a boolean, and it guards every reading. A `state_of_charge` of `true` is not 1.0.
//
// `codes.includes(fault_code)`
//     `Array.prototype.includes` compares by SameValueZero, so `[1, 2].includes(true)` is **false**
//     — a boolean is not a number however equal they look. A vendor fault code of `true` would
//     otherwise trigger §36.4's brownout override, the one rule that ignores the gauge and stops
//     the robot immediately.

#include "skynet_os/contracts.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/vocabulary.hpp"

#include <cmath>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

/// Where the `reserve` band ends, as a multiple of the reserve. A quarter of a reserve wide, because
/// §36.1's two rows are otherwise a single point and everything under it — and a gauge reporting
/// within a percent of the boundary would chatter between *begin a controlled stop* and *stop
/// immediately*, on a robot that is mid-ramp.
constexpr double kReserveBand = 1.25;

/// Where the `low` band ends. **One whole reserve above the reserve band**, and that width is the
/// argument: `low`'s only behaviour is that a running task may finish and no new one may start, so
/// the warning has to arrive with at least one further safe stop's worth of energy in hand.
constexpr double kLowBand = 2.25;

/// Thirty seconds. §36.3 raises DEGRADED on *sustained* throttling and does not say how long, and
/// both mistakes are real: one cycle makes every heavy motion a degraded robot, and ten minutes
/// makes DEGRADED a state a thermally limited robot never reaches.
constexpr double kSustainedThrottleMs = 30000;

/// The first three rungs. The line between three and four is the meaningful one: telemetry, OTA and
/// black-box compression are housekeeping, and giving them up costs the robot nothing it was hired
/// to do. Rungs four and five change what it can *do*.
constexpr std::size_t kThrottleDepth = 3;

/// `obj?.key` — nullptr when the object is missing or the key is not present.
const Json* field(const Json& obj, std::string_view key) {
  return obj.is_object() && obj.has(key) ? &obj[key] : nullptr;
}

/// `Number.isFinite` — false for a boolean.
bool is_finite(const Json* value) {
  return value != nullptr && value->is_number() && std::isfinite(value->number_or(0));
}

/// `Array.prototype.includes` — SameValueZero, so `true` is not `1`.
bool strict_includes(const Json* haystack, const Json* needle) {
  if (haystack == nullptr || !haystack->is_array() || needle == nullptr) {
    return false;
  }
  for (const Json& item : haystack->items()) {
    if (item.kind() != needle->kind()) {
      continue;
    }
    if (item.canonical() == needle->canonical()) {
      return true;
    }
  }
  return false;
}

struct Fault {
  std::string code;
  std::string detail;
};

/// Whether a fault raises §27's `DEGRADED`.
///
/// The two thermal ones do, because §36.3 says so in as many words. The declaration gaps do not: a
/// descriptor missing a field is a descriptor to fix, and a robot that reported itself degraded for
/// it would be reporting a paperwork problem in the same column as a hot motor.
bool fault_degrades(const std::string& code) {
  return code == "thermal_sustained" || code == "thermal_critical";
}

/// Whether a fault withholds new task leases.
///
/// A robot that cannot tell whether it is at reserve should not be handed *new* work, and should not
/// have taken from it the work it is already doing — which is §36.1's `low` behaviour arrived at
/// from the other direction.
bool fault_withholds_leases(const std::string& code) {
  return code == "gauge_silent" || code == "capacity_undeclared" || code == "reserve_undeclared" ||
         code == "charge_out_of_range" || code == "thermal_critical";
}

/// The pressure a temperature implies, given what the body declares.
///
/// A temperature with no declared threshold is `nominal` **and a fault**, not a silent pass: nobody
/// has said what hot is on this body. Guessing 85 °C would be right for a CPU package and wrong for
/// a harmonic drive by fifty degrees.
std::string pressure(const Json& profile, const Json* temperature_c) {
  if (!is_finite(temperature_c)) {
    return "nominal";
  }
  const double reading = temperature_c->number_or(0);
  const Json* critical = field(profile, "thermal_critical_c");
  if (critical != nullptr && reading >= critical->number_or(0)) {
    return "critical";
  }
  const Json* throttle = field(profile, "thermal_throttle_c");
  if (throttle != nullptr && reading >= throttle->number_or(0)) {
    return "throttling";
  }
  return "nominal";
}

/// §36.1's bands.
std::string band_of(double remaining, double budget) {
  if (remaining < budget) {
    return "critical";
  }
  if (remaining < budget * kReserveBand) {
    return "reserve";
  }
  if (remaining < budget * kLowBand) {
    return "low";
  }
  return "normal";
}

/// Kept beside `band_of` rather than derived from the state, so the distinction between *at
/// reserve* and *below it* — the one §36.1 spends two rows on — survives.
std::string band_reason(const std::string& band) {
  if (band == "critical") return "below_reserve";
  if (band == "reserve") return "at_reserve";
  if (band == "low") return "approaching_reserve";
  return "above_reserve";
}

}  // namespace

Json power(const std::string& /*label*/, const Json& input) {
  const Json& profile = input["profile"];
  const Json& observation = input["observation"];

  // §36.2's reserve budget. The arithmetic is deliberately the identity: the reserve *is*
  // `safe_stop_energy_wh`, because the other two terms §36.2 names have no source. Scaling it by a
  // plausible payload factor would have been one line and a number nobody could defend — and this
  // is the number that decides when a two-metre humanoid stops walking.
  const Json* declared_energy = field(profile, "safe_stop_energy_wh");
  const bool budget_usable = is_finite(declared_energy) && declared_energy->number_or(0) > 0;
  const vocabulary::Set& reserve_terms = vocabulary::members("reserve_terms");
  std::vector<std::string> unaccounted;
  if (!budget_usable) {
    unaccounted.emplace_back(reserve_terms[0]);
  }
  // Neither of the other two exists on this platform. Appended unconditionally rather than looked
  // up, so the day one of them arrives this is a visible place to change rather than a silent
  // over-report.
  unaccounted.emplace_back(reserve_terms[1]);
  unaccounted.emplace_back(reserve_terms[2]);

  const Json* temperature = field(observation, "temperature_c");
  const std::string thermal = pressure(profile, temperature);

  // A pack is claimed by declaring either its capacity or the energy a safe stop costs — the second
  // because an energy budget in watt-hours is a statement about a pack even when nobody wrote the
  // capacity down. A body that declares neither is a mains machine, and `unmetered` is the correct
  // reading of its silence.
  const Json* capacity = field(profile, "pack_capacity_wh");
  const Json* charge = field(observation, "state_of_charge");
  const Json* energy = field(observation, "energy_remaining_wh");
  const bool claims_pack = declared_energy != nullptr || capacity != nullptr;
  const bool no_reading = charge == nullptr && energy == nullptr;

  // `energy_remaining_wh` wins over a fraction times a capacity, because a gauge reporting energy
  // has been told the pack's capacity by something closer to the pack than a descriptor field. A
  // fraction outside 0–1 produces nothing rather than a clamp.
  bool have_remaining = false;
  double remaining = 0;
  if (is_finite(energy) && energy->number_or(0) >= 0) {
    have_remaining = true;
    remaining = energy->number_or(0);
  } else if (is_finite(charge) && charge->number_or(0) >= 0 && charge->number_or(0) <= 1 &&
             is_finite(capacity) && capacity->number_or(0) > 0) {
    have_remaining = true;
    remaining = charge->number_or(0) * capacity->number_or(0);
  }
  const bool charge_unusable =
      charge != nullptr &&
      (!is_finite(charge) || charge->number_or(0) < 0 || charge->number_or(0) > 1);

  const Json* codes = field(profile, "brownout_fault_codes");
  const Json* fault_code = field(observation, "fault_code");
  const bool rhal_degraded = observation["rhal_degraded"].is_true();

  // The order of the rules is the design, and the first one is what §36.4 is written for.
  //
  //   1. **Brownout.** The bus said sag; the gauge is not consulted, "because a gauge that
  //      disagrees with the bus is a gauge that is wrong". The only rule that ignores a reading it
  //      has — a brownout with a silent gauge is still a brownout.
  //   2. **No pack declared.** A mains machine, correctly reporting that its energy is not metered.
  //   3. **No reading.** A body that claims a pack and got nothing back. Not `critical`, and the
  //      fault is what makes it distinguishable from rule 2.
  //   4. **No reserve, or no capacity.** A reading arrived and cannot be compared against anything.
  //   5. **The bands.** The only rule that looks at a number, and last for that reason.
  const bool brownout = rhal_degraded && fault_code != nullptr && strict_includes(codes, fault_code);

  std::string state;
  std::string reason;
  if (brownout) {
    state = "critical";
    reason = "brownout";
  } else if (!claims_pack) {
    state = "unmetered";
    reason = "no_pack";
  } else if (no_reading) {
    state = "unmetered";
    reason = "no_reading";
  } else if (!have_remaining) {
    // Two ways to arrive with no energy figure, and they send an operator to two different places:
    // a gauge whose reading cannot be believed, and a descriptor with no capacity.
    state = "unmetered";
    reason = charge_unusable ? "reading_unusable" : "no_capacity";
  } else if (!budget_usable) {
    state = "unmetered";
    reason = "no_reserve";
  } else {
    state = band_of(remaining, declared_energy->number_or(0));
    reason = band_reason(state);
  }

  // The faults, in `power_fault_codes` order so a console renders them the same way every time.
  std::vector<Fault> faults;
  if (claims_pack && no_reading) {
    faults.push_back(
        {"gauge_silent",
         "this body declares a pack and nothing on this robot read one. The state is reported as "
         "unmetered rather than critical — §36.4 overrides the gauge on a positive reading from the "
         "bus, and a missing reading is not one — and no new task lease is granted until something "
         "answers for the pack (§36.1)."});
  }
  if (claims_pack && energy == nullptr && charge != nullptr && capacity == nullptr) {
    faults.push_back(
        {"capacity_undeclared",
         "the gauge reports a fraction and this body declares no `pack_capacity_wh` to turn it into "
         "watt-hours, so there is nothing to compare against the reserve. Declare the capacity, or "
         "report `energy_remaining_wh` from the gauge (§36.2)."});
  }
  if (claims_pack && !budget_usable) {
    faults.push_back(
        {"reserve_undeclared",
         "this body declares a pack and no `safe_stop_energy_wh`, so §36.2's reserve cannot be "
         "computed and the four states have no boundary. It is the figure "
         "`power.safe_state.reachable` measures, and the one that decides when this robot stops."});
  }
  if (rhal_degraded && (codes == nullptr || codes->items().empty())) {
    faults.push_back(
        {"brownout_undeclarable",
         "the RHAL is reporting `RHAL_DEGRADED` and this body declares no `brownout_fault_codes`, "
         "so nothing here can tell a voltage sag from a dead encoder. §24 makes a fault code "
         "vendor-specific and surfaced verbatim, which is precisely why the vendor rather than the "
         "OS has to name the codes that mean sag (§36.4)."});
  }
  if (charge_unusable) {
    faults.push_back(
        {"charge_out_of_range",
         "the gauge reports a state of charge of " + js::interpolate(charge) +
             ", and a state of charge is a fraction between 0 and 1. The reading is discarded "
             "rather than clamped: a clamped 1.4 is a full battery, and this one is a gauge nobody "
             "should be trusting yet (§36.1)."});
  }
  const Json* throttled_for = field(observation, "throttled_for_ms");
  const double throttled_ms =
      throttled_for == nullptr || throttled_for->is_null() ? 0 : throttled_for->number_or(0);
  if (thermal == "throttling" && throttled_ms >= kSustainedThrottleMs) {
    faults.push_back(
        {"thermal_sustained",
         "this body has been throttling for " + js::number_to_string(std::floor(throttled_ms / 1000)) +
             " s. §36.3 raises DEGRADED on sustained throttling, and " +
             js::number_to_string(static_cast<double>(kThrottleDepth)) +
             " of the five rungs are given up — never the RHAL, the kernel or the control loop."});
  }
  if (thermal == "critical") {
    faults.push_back(
        {"thermal_critical",
         "this body is at or above its declared critical temperature. Every rung of §36.3's ladder "
         "is given up, inference included; the RHAL, the kernel and the control loop are not, "
         "because there is no thermal situation in which giving up the safety envelope is the right "
         "trade."});
  }
  if (is_finite(temperature) && field(profile, "thermal_throttle_c") == nullptr &&
      field(profile, "thermal_critical_c") == nullptr) {
    faults.push_back(
        {"thermal_undeclared",
         "something is reading a temperature and this body declares neither `thermal_throttle_c` "
         "nor `thermal_critical_c`, so nothing here knows what hot is on this machine. The reading "
         "is reported and nothing is shed: 85 °C is right for a CPU package and fifty degrees wrong "
         "for a harmonic drive (§36.3)."});
  }

  // What is given up at this pressure, in shed order (§36.3). Never anything in `never_shed`: there
  // is no thermal situation in which giving up the safety envelope is the right trade, and
  // inference is throttled before control rather than beside it.
  const vocabulary::Set& order = vocabulary::members("shed_order");
  const std::size_t depth =
      thermal == "nominal" ? 0 : (thermal == "throttling" ? kThrottleDepth : order.size());
  Json shed = Json::array();
  for (std::size_t at = 0; at < depth; ++at) {
    shed.push(Json::string(std::string(order[at])));
  }

  bool degraded = false;
  bool withheld = false;
  Json fault_rows = Json::array();
  for (const Fault& fault : faults) {
    degraded = degraded || fault_degrades(fault.code);
    withheld = withheld || fault_withholds_leases(fault.code);
    Json row = Json::object();
    row.set("code", Json::string(fault.code));
    row.set("detail", Json::string(fault.detail));
    fault_rows.push(row);
  }

  // §36.3's last sentence, compared against the **observed** loop time rather than against the
  // thermal state. That is the honest coupling: thermal throttling is the usual cause of a missed
  // budget and not the only one, and a robot that blocked a package for being warm while its loop
  // was comfortably inside budget would be enforcing a proxy instead of the thing itself.
  //
  // The corpus writes an absent optional as null, because JSON has no `undefined`.
  const Json* declared_budget = field(input, "declared_budget_ms");
  const Json* observed_inference = field(input, "observed_inference_ms");
  Json budget_missed = Json::null();
  if (is_finite(declared_budget) && declared_budget->number_or(0) > 0 &&
      is_finite(observed_inference) &&
      observed_inference->number_or(0) > declared_budget->number_or(0)) {
    budget_missed = Json::string(
        "this package declares an inference budget of " + js::interpolate(declared_budget) +
        " ms and the loop is taking " + js::interpolate(observed_inference) +
        " ms. §36.3 throttles inference before control, so the loop is late rather than the robot "
        "being unsafe — but a policy quietly missing its own declared deadline is what the budget "
        "exists to prevent, so it is refused until the measurement comes back inside it.");
  }

  Json unaccounted_rows = Json::array();
  for (const std::string& term : unaccounted) {
    unaccounted_rows.push(Json::string(term));
  }
  Json budget = Json::object();
  budget.set("wh", budget_usable ? Json::number(declared_energy->number_or(0)) : Json::null());
  budget.set("unaccounted", unaccounted_rows);
  // False on every robot today: two of §36.2's three terms have no source on this platform.
  budget.set("complete", Json::boolean(budget_usable && unaccounted.empty()));

  // `unmetered` grants, and that is the whole reason the state exists: a bench arm on mains must
  // not be refused work because it has no battery to report on.
  const bool state_grants = state == "normal" || state == "unmetered";

  Json out = Json::object();
  out.set("state", Json::string(state));
  out.set("reason", Json::string(reason));
  out.set("remaining_wh", have_remaining ? Json::number(remaining) : Json::null());
  out.set("reserve", budget);
  out.set("thermal", Json::string(thermal));
  out.set("shed", shed);
  out.set("faults", fault_rows);
  // The predicates are rendered rather than left to the caller because they are what §25.2, §27 and
  // §36.1 each ask, and an edition could agree about the state while disagreeing about what it
  // licenses.
  out.set("degraded", Json::boolean(degraded));
  out.set("grants_new_leases", Json::boolean(state_grants && !withheld));
  out.set("requires_safe_state", Json::boolean(state == "reserve" || state == "critical"));
  out.set("stops_immediately", Json::boolean(state == "critical"));
  // §36.1's third row, missing from this document in every edition until 2026-09-16.
  out.set("notifies_operator",
          Json::boolean(state == "low" || state == "reserve" || state == "critical"));
  out.set("budget_missed", budget_missed);
  return out;
}

}  // namespace skynet::contracts
