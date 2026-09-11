// The watchdog chain (§46) and §11's pull (intent) — the C++ edition of
// `reference/typescript/watchdog.ts` and `reference/typescript/intent.ts`.
//
// Two contracts in one file because both are short and both are about the same thing from opposite
// ends: what a robot does when the thing that was supposed to keep talking to it stops.
//
// ── The chain ───────────────────────────────────────────────────────────────
//
//   1  Kernel `watchdog` guard   the policy stopped producing commands      stop_cat2
//   2  systemd `WatchdogSec`     the supervisor's reconcile loop hung       restart skynetd
//   3  SoC hardware WDT          the application processor wedged entirely  reset the SoC
//   4  Safety MCU timer          the AP wedged *and* the reset failed       cut drive enable
//
// Layer 3 is petted by the **Safety Kernel**, not by `skynetd`, and that is what makes it mean
// anything: the WDT is satisfied only while the thing that actually enforces the envelope is
// running its cycle. A watchdog petted by a supervisor tells you the supervisor is alive, which is
// not the question anybody is asking.
//
// ── A reset is not a stop ───────────────────────────────────────────────────
//
// When the SoC resets the drives lose their command source, and what they then do is a property of
// the **hardware**. Some hold, some brake, some fault — and **some coast**, and on such a body
// layer 3's own recovery path is a fall.
//
// So `host_blocked_by_command_loss` refuses the host tier for a coasting body, and the shape of
// that refusal is unusual and correct: **the middle rung is the unsafe one.** Guest is fine,
// because enforcement is advisory and nothing relies on a hardware watchdog. Certified is fine,
// because layer 4 cuts drive enable independently of the processor being reset. Only host, where
// the OS owns the control loop and layer 3 is the last line, is the configuration where a reset
// means a fall.
//
// ── Where C++ is not JavaScript ─────────────────────────────────────────────
//
// `present` refuses a boolean before it refuses anything else. A window declared as `true` would
// be compared as 1 ms in a host that treats a boolean as a number, which nests inside every real
// window and would report a chain as sound on the strength of a value that is not a duration at
// all. The reference gets this free from `typeof`.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/text.hpp"

#include <cmath>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

/// The chain, as data (§46.1).
struct Layer {
  double layer;
  const char* name;
  const char* catches;
  const char* response;
  const char* required_from;
  const char* petted_by;
};

const std::vector<Layer>& chain() {
  static const std::vector<Layer> layers = {
      {1, "kernel_guard", "the policy stopped producing commands", "stop_cat2", "guest",
       "the policy runtime, every cycle"},
      {2, "supervisor", "the supervisor's reconcile loop hung", "restart skynetd", "guest",
       "skynetd, from inside the reconcile loop"},
      // §46.1's sentence, and the reason layer 3 means anything.
      {3, "soc_wdt", "the application processor wedged entirely", "reset the SoC", "host",
       "the Safety Kernel, not skynetd"},
      {4, "safety_mcu", "the AP wedged and the reset did not recover it", "cut drive enable",
       "certified", "an independent timer on the safety MCU"},
  };
  return layers;
}

/// §46.2's vocabulary, in the order the specification lists it. Not one of the corpus's closed
/// sets — it lives in `watchdog.ts` and is read from there, so it is declared here rather than
/// added to a table the `vocabulary` corpus would then expect to find.
bool is_command_loss_behaviour(std::string_view value) {
  return value == "hold" || value == "brake" || value == "fault" || value == "coast";
}

/// The four windows §46.3 nests, in order, innermost first.
const std::vector<std::string>& windows() {
  static const std::vector<std::string> names = {
      "kernel_cycle_worst_case_ms", "envelope_watchdog_ms", "soc_wdt_ms",
      "drive_command_timeout_ms"};
  return names;
}

/// A declared, finite duration. A boolean is neither.
bool present(const Json& value) {
  return value.is_number() && std::isfinite(value.number_or(0));
}

std::string nesting_code(const std::string& inner, const std::string& outer) {
  if (inner == "kernel_cycle_worst_case_ms" && outer == "envelope_watchdog_ms") {
    return "cycle_exceeds_watchdog";
  }
  if (inner == "envelope_watchdog_ms" && outer == "soc_wdt_ms") {
    return "wdt_pre_empts_kernel";
  }
  if (inner == "soc_wdt_ms" && outer == "drive_command_timeout_ms") {
    return "drive_faults_first";
  }
  return "windows_not_nested";
}

std::string nesting_consequence(const std::string& inner, const std::string& outer) {
  if (inner == "kernel_cycle_worst_case_ms" && outer == "envelope_watchdog_ms") {
    return "The kernel cannot complete a cycle inside its own command deadline, so the watchdog "
           "guard trips on a robot that is working correctly.";
  }
  if (inner == "envelope_watchdog_ms" && outer == "soc_wdt_ms") {
    return "The processor resets before the kernel executes its controlled stop, so the harsher "
           "recovery pre-empts the gentler one and the robot never performs its best available "
           "behaviour (§46.3).";
  }
  if (inner == "soc_wdt_ms" && outer == "drive_command_timeout_ms") {
    return "The drive faults on its own before the hardware watchdog fires, so layer 3 is present, "
           "configured, and never reached (§46.3).";
  }
  return "The windows must nest innermost-first for every layer above to be reachable.";
}

/// Whether a body may attempt the host tier, given what it says its drives do (§46.2).
///
/// Two refusals, and they are the same refusal: **the recovery path has not been established to be
/// survivable.** A body that coasts has one that demonstrably is not; a body that has declared
/// nothing has one nobody has characterised, and treating silence as `hold` would be inventing the
/// most convenient answer to the only question layer 3 turns on.
Json host_blocked(const Json& on_command_loss) {
  if (on_command_loss.is_string() && on_command_loss.string_or("") == "coast") {
    return Json::string(
        "this body's drives coast when commands stop, and layer 3 of the watchdog chain recovers "
        "by resetting the processor — which on this machine is a fall. The sound configuration is "
        "the certified tier, where layer 4 cuts drive enable independently of the processor "
        "(§46.2)");
  }
  if (on_command_loss.is_null()) {
    return Json::string(
        "this body does not declare what its drives do when commands stop, and layer 3 of the "
        "watchdog chain recovers by resetting the processor. Silence is not `hold`: it is a "
        "recovery path nobody has characterised (§46.2)");
  }
  if (!on_command_loss.is_string() || !is_command_loss_behaviour(on_command_loss.string_or(""))) {
    return Json::string("this body declares `on_command_loss: " +
                        js::interpolate(&on_command_loss) +
                        "`, which is not one of hold, brake, fault or coast — so what it does "
                        "while its processor resets is not something anybody can read off the "
                        "descriptor (§46.2)");
  }
  return Json::null();
}

}  // namespace

Json watchdog(const std::string& /*label*/, const Json& input) {
  const Json& declared = input["windows"];

  // Adjacent pairs only: comparing every pair would report the same inversion up to three times and
  // bury the one an operator has to fix.
  //
  // **Strictly less than.** Two watchdogs with the same window is a race whose winner is decided by
  // interrupt latency, and a chain whose order depends on which timer fires first is not a chain.
  //
  // A gap in the middle does not stop the walk: with no `envelope_watchdog_ms` declared, the kernel
  // cycle and the SoC window are still worth comparing, because that inversion is real whatever
  // sits between them.
  std::vector<std::string> missing;
  std::vector<std::pair<std::string, double>> present_windows;
  for (const std::string& name : windows()) {
    if (present(declared[name])) {
      present_windows.emplace_back(name, declared[name].number_or(0));
    } else {
      missing.push_back(name);
    }
  }

  Json faults = Json::array();
  for (std::size_t index = 0; index + 1 < present_windows.size(); ++index) {
    const auto& [inner_name, inner] = present_windows[index];
    const auto& [outer_name, outer] = present_windows[index + 1];
    if (inner < outer) {
      continue;
    }
    Json fault = Json::object();
    fault.set("code", Json::string(nesting_code(inner_name, outer_name)));
    fault.set("inner", Json::string(inner_name));
    fault.set("outer", Json::string(outer_name));
    fault.set("detail",
              Json::string(inner_name + " is " + js::number_to_string(inner) + " ms and " +
                           outer_name + " is " + js::number_to_string(outer) +
                           " ms, so the outer window is not outside the inner one. " +
                           nesting_consequence(inner_name, outer_name)));
    faults.push(fault);
  }

  Json layers = Json::array();
  for (const Layer& layer : chain()) {
    Json row = Json::object();
    row.set("layer", Json::number(layer.layer));
    row.set("name", Json::string(layer.name));
    row.set("catches", Json::string(layer.catches));
    row.set("response", Json::string(layer.response));
    row.set("required_from", Json::string(layer.required_from));
    row.set("petted_by", Json::string(layer.petted_by));
    layers.push(row);
  }

  // An array, not an object: canonical JSON sorts an object's keys, and these four windows mean
  // nothing except in order. The object form renders them alphabetically, which on a panel about
  // nesting is worse than rendering nothing.
  Json declared_rows = Json::array();
  for (const std::string& name : windows()) {
    Json row = Json::object();
    row.set("window", Json::string(name));
    row.set("ms", declared.has(name) ? declared[name] : Json::null());
    declared_rows.push(row);
  }

  Json missing_rows = Json::array();
  for (const std::string& name : missing) {
    missing_rows.push(Json::string(name));
  }

  Json nesting = Json::object();
  nesting.set("declared", declared_rows);
  nesting.set("faults", faults);
  nesting.set("missing", missing_rows);
  // Every pair that could be compared nests. **Not** the same as "the chain is sound".
  nesting.set("ordered", Json::boolean(faults.items().empty()));
  // Every window is declared *and* every pair nests.
  nesting.set("complete", Json::boolean(faults.items().empty() && missing.empty()));

  Json out = Json::object();
  out.set("chain", layers);
  out.set("on_command_loss", input["on_command_loss"]);
  out.set("host_blocked", host_blocked(input["on_command_loss"]));
  out.set("nesting", nesting);
  return out;
}

namespace {

Json fault(std::string code, std::string severity, std::string message) {
  Json out = Json::object();
  out.set("code", Json::string(std::move(code)));
  out.set("message", Json::string(std::move(message)));
  out.set("severity", Json::string(std::move(severity)));
  return out;
}

/// `String(value ?? '').trim()` — how both digests are read before they are compared.
std::string trimmed_string(const Json& value) {
  if (value.is_null()) {
    return "";
  }
  return js::trim(js::interpolate(&value));
}

double or_zero(const Json& value) { return value.is_null() ? 0 : value.number_or(0); }

}  // namespace

Json intent(const std::string& /*label*/, const Json& input) {
  // §11: **The Hub stores intent. The robot reconciles.** Never push.
  //
  // The document arrives over a network, which makes it the least trusted input in the system that
  // is also the most consequential: it decides what software runs on a machine that moves.
  //
  // **Refusing is not the same as ignoring.** Two of the four findings refuse the offer, and in
  // both cases the robot keeps reconciling against what it already has — the *ordinary* state
  // rather than a failure mode, because §34.1 assumes the robot is offline most of the time.
  const Json& offer = input["offer"];
  const Json& held = input["held"];

  const double held_generation = or_zero(held["generation"]);
  const std::string held_digest = trimmed_string(held["digest"]);
  const double offered = or_zero(offer["generation"]);
  const std::string digest = trimmed_string(offer["digest"]);

  Json faults = Json::array();
  const auto verdict = [&](bool adopt, bool changed, double generation) {
    Json out = Json::object();
    out.set("adopt", Json::boolean(adopt));
    out.set("changed", Json::boolean(changed));
    out.set("faults", faults);
    out.set("generation", Json::number(generation));
    return out;
  };

  // Strict, and it is the first gate. A truthy non-boolean does not make a response a desired state.
  if (!offer["decoded"].is_true()) {
    const std::string why = trimmed_string(offer["decode_error"]);
    const std::string because = why.empty() ? "" : ": " + why;
    faults.push(fault("undecodable", "error",
                      "This response is not a desired state" + because +
                          ". It has not been written to disk, and this robot carries on "
                          "reconciling against what it already has — which is what it does "
                          "whenever it cannot reach the Hub at all (§34.1), so nothing about the "
                          "robot is worse for it."));
    return verdict(false, false, held_generation);
  }

  // **A reused generation is refused.** A Hub serving two different documents under one generation
  // has broken the property §11 rests idempotence on, and the robot is the only party that can see
  // it. Adopting would have the robot running new software while reporting the old generation as
  // applied, and a fleet console would show it converged — an invisible wrong state.
  if (offered == held_generation && !digest.empty() && !held_digest.empty() &&
      digest != held_digest) {
    faults.push(fault("generation_reused", "error",
                      "The Hub is serving a different document under generation " +
                          js::number_to_string(offered) +
                          ", which this robot has already applied. §11 rests idempotence on that "
                          "counter, so adopting this would have the robot running new software "
                          "while reporting the old generation as applied — and a fleet console "
                          "would show it converged. Bump the generation on the Hub and this robot "
                          "will take it on the next poll."));
    return verdict(false, false, held_generation);
  }

  // **A *lower* generation is adopted.** The opposite call, and for a reason worth writing down: a
  // generation that goes backwards is a Hub restored from a backup, and a robot that refused to
  // follow one would be a fleet an operator could not recover, permanently, with no remedy short of
  // visiting every machine.
  if (offered < held_generation) {
    faults.push(fault("generation_went_backwards", "warning",
                      "This robot has applied generation " + js::number_to_string(held_generation) +
                          " and the Hub is asking for " + js::number_to_string(offered) +
                          ". Intent has gone backwards, which is what a Hub restored from a backup "
                          "looks like. It is adopted — a robot that refused to follow one would be "
                          "a fleet nobody could recover — and it is reported, because it is worth "
                          "somebody knowing once."));
  }

  const double running = or_zero(held["running"]);
  if (or_zero(offer["packages"]) == 0 && running > 0) {
    faults.push(fault("empties_the_robot", "warning",
                      "This desired state names no packages and this robot is running " +
                          js::number_to_string(running) +
                          ". That is what an operator emptying a robot looks like, and it is also "
                          "what a Hub serving a stub looks like. It is adopted, because stopping "
                          "is never the dangerous direction (§33.4) — and it is said out loud, "
                          "because the two are indistinguishable from here."));
  }

  return verdict(true, digest != held_digest, offered);
}

}  // namespace skynet::contracts
