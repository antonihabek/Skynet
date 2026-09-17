// §88 — what admits a motion request on a program-level body. C++ edition of
// `reference/typescript/execute.ts`.
//
// §54.1 ended with two sentences that stood from the moment Part VI was written: *"`execute({ skill,
// arguments, timeout_s })` already existed on the Hub side as the invocation point. Nothing routed
// it."* For a joint-commanded body that stopped being true once §54.4's lifecycle was driven; for
// §59.2's machines it was still true, and §84 removed the first of Phase 4's two blockers.
//
// ── The second blocker dissolved rather than being bridged ─────────────────
//
// §33.1 keeps `skynetd` off the path that holds a driver, so a routed skill's destination is
// `skynetk`'s and there was no channel from the supervisor to it. Both obvious remedies spend
// §33.1's licence. §88.1's answer is that the route already exists: a skill over a program-level
// body is a policy process emitting motion requests into the ring it would post joint commands into,
// and what was missing was not a channel but the **admission rule** for a request arriving on one.
//
// ── It is a gate, and calling it a guard would be a claim ──────────────────
//
// §7's guards clamp a *value* against a resolved ceiling. There is no ceiling for `move_l`. What is
// checkable is **authority, state, and the body's own declared table** — the last being the half
// §84.1 created and nothing consumed.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/vocabulary.hpp"

#include <algorithm>
#include <cmath>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

struct Argument {
  std::string name;
  double value = 0;
};

struct Verdict {
  bool accepted = false;
  std::string refusal;
  std::string detail;
  std::vector<Argument> arguments;
  std::vector<std::string> unbounded;
};

Verdict refuse(std::string refusal, std::string detail) {
  Verdict out;
  out.refusal = std::move(refusal);
  out.detail = std::move(detail);
  return out;
}

bool finite_number(const Json& value, double& out) {
  if (!value.is_number()) {
    return false;
  }
  out = value.number_or(0);
  return std::isfinite(out);
}

bool ready_state(const std::string& state) {
  const vocabulary::Set& states = vocabulary::members("motion_ready_states");
  return std::find(states.begin(), states.end(), state) != states.end();
}

/// Whether this request may be handed to the driver, and with what.
///
/// Authority before state, state before content. A request from a holder that does not hold the body
/// is refused **without the platform reading what it asked for**, because reading it would mean
/// answering a question nobody was entitled to ask.
///
/// **`parameter_unknown` is a refusal here and a warning at plan time, and the asymmetry is the
/// point.** §84.3 raises `motion_parameter_unused` on a transport plan and does not refuse, because
/// the controller does take the motion. At command time the same fact is a value about to be handed
/// to a controller that will either interpret it or reject it on a moving robot.
Verdict admit_motion(const Json& gate, const Json& request) {
  const Json& held_field = gate["lease_held_by"];
  const Json& holder_field = request["holder"];

  if (!held_field.is_string()) {
    return refuse("no_lease",
                  "Nothing holds this body, so there is nobody whose motion this would be "
                  "(§25.2).");
  }
  const std::string held(held_field.string_or(""));
  if (!holder_field.is_string() || std::string(holder_field.string_or("")) != held) {
    const std::string named =
        holder_field.is_string() ? std::string(holder_field.string_or("")) : "nobody";
    return refuse("no_lease", "This body is held by `" + held + "` and the request came from `" +
                                  named + "` (§25.2).");
  }

  if (gate["estop"].is_bool() && gate["estop"].is_true()) {
    return refuse("estop",
                  "The emergency-stop chain is open (§24). What opened it is not something the "
                  "body can say, so the only correct response is to command nothing.");
  }

  const std::string state(gate["state"].string_or(""));
  if (!ready_state(state)) {
    return refuse("not_active",
                  "§27 has this body in `" + state +
                      "`, which is not a state in which it will run a program. Accepting the "
                      "request would be queueing work behind whatever the body is doing instead "
                      "— a controlled stop, a self-test, or waiting to be homed.");
  }

  if (gate["executing"].is_string() && !std::string(gate["executing"].string_or("")).empty()) {
    return refuse("busy", "The controller is executing `" +
                              std::string(gate["executing"].string_or("")) +
                              "`. A program-level controller owns the servo loop (§59.2), so "
                              "it finishes or it is aborted; there is no second motion to "
                              "interleave with the first.");
  }

  const std::string name(request["motion"].string_or(""));
  const Json* motion = nullptr;
  for (const Json& candidate : gate["motions"].items()) {
    if (candidate["name"].is_string() && std::string(candidate["name"].string_or("")) == name) {
      motion = &candidate;
      break;
    }
  }
  if (motion == nullptr) {
    return refuse("unknown_motion",
                  "This body declares no motion called `" + name +
                      "`. The name a request carries is the **body's** spelling, which "
                      "§84.3's plan resolved at install — a policy's own name reaching "
                      "here unresolved is the defect this refusal usually means.");
  }
  const std::string motion_name((*motion)["name"].string_or(""));

  double deadline = 0;
  if (!finite_number(request["deadline_ms"], deadline) || deadline <= 0) {
    return refuse("deadline_undeclared",
                  "A motion takes seconds, during which §7.1's watchdog guard has nothing to "
                  "watch: a correct policy produces no command at all. The deadline is what "
                  "replaces it (§88.3), so a request without one is a program nothing would "
                  "ever conclude had failed.");
  }

  // First wins, as §84.1's reader does with a repeated name: `duplicate_parameter` is the
  // descriptor's fault. A gate that preferred the later one would command against a bound the check
  // never looked at.
  std::vector<const Json*> declared;
  std::vector<std::string> declared_names;
  for (const Json& parameter : (*motion)["parameters"].items()) {
    if (!parameter["name"].is_string()) {
      continue;
    }
    const std::string parameter_name(parameter["name"].string_or(""));
    if (std::find(declared_names.begin(), declared_names.end(), parameter_name) !=
        declared_names.end()) {
      continue;
    }
    declared.push_back(&parameter);
    declared_names.push_back(parameter_name);
  }

  const Json& parameters = request["parameters"];

  // The request's own names first: a parameter the motion has no slot for means the policy and the
  // body disagree about the motion's *shape*, which subsumes arguing about its values.
  std::vector<std::string> supplied;
  for (const auto& entry : parameters.entries()) {
    supplied.push_back(entry.first);
  }
  std::sort(supplied.begin(), supplied.end(),
            [](const std::string& left, const std::string& right) {
              return js::utf16_less(left, right);
            });
  for (const std::string& key : supplied) {
    if (std::find(declared_names.begin(), declared_names.end(), key) != declared_names.end()) {
      continue;
    }
    return refuse("parameter_unknown",
                  "`" + motion_name + "` does not take `" + key +
                      "`. §84.3 warned about this at install — "
                      "`motion_parameter_unused` — and declined to refuse a plan over it, "
                      "because the controller does take the motion. Here it is a value about to be "
                      "sent to a controller that has no slot for it, and §25.2 is clear that "
                      "commanding with the parts of a command a machine happened to understand is "
                      "worse than refusing.");
  }

  Verdict out;
  for (const Json* parameter : declared) {
    const std::string parameter_name((*parameter)["name"].string_or(""));
    bool present = false;
    for (const auto& entry : parameters.entries()) {
      if (entry.first == parameter_name) {
        present = true;
      }
    }
    if (!present) {
      // §84.1's fail-closed reading: anything but an explicit `false` is required.
      const Json& required = (*parameter)["required"];
      if (!required.is_bool() || required.is_true()) {
        return refuse("parameter_missing",
                      "`" + motion_name + "` requires `" + parameter_name +
                          "` and the request does not carry it. §84.1 reads a requirement "
                          "fail-closed, so a parameter whose `required` the descriptor never "
                          "declared is required here too.");
      }
      // Absent rather than zero. §7.2's rule, and the controller owns its own defaults.
      continue;
    }

    double value = 0;
    if (!finite_number(parameters[parameter_name], value)) {
      return refuse("parameter_not_finite",
                    "`" + parameter_name +
                        "` is not a finite number. A controller handed one would either fault or "
                        "move somewhere nobody chose.");
    }

    double minimum = 0;
    double maximum = 0;
    const bool has_min = finite_number((*parameter)["min"], minimum);
    const bool has_max = finite_number((*parameter)["max"], maximum);
    if (has_min && value < minimum) {
      return refuse("parameter_out_of_bounds",
                    "`" + parameter_name + "` is " + js::number_to_string(value) + " and `" +
                        motion_name + "` declares a minimum of " + js::number_to_string(minimum) +
                        ".");
    }
    if (has_max && value > maximum) {
      return refuse("parameter_out_of_bounds",
                    "`" + parameter_name + "` is " + js::number_to_string(value) + " and `" +
                        motion_name + "` declares a maximum of " + js::number_to_string(maximum) +
                        ".");
    }
    if (!has_min && !has_max) {
      out.unbounded.push_back(parameter_name);
    }

    out.arguments.push_back({parameter_name, value});
  }

  out.accepted = true;
  if (out.unbounded.empty()) {
    out.detail =
        "`" + motion_name + "` accepted, with every supplied parameter inside a bound this body "
        "declared.";
  } else {
    std::string names;
    for (const std::string& entry : out.unbounded) {
      if (!names.empty()) {
        names += ", ";
      }
      names += entry;
    }
    out.detail = "`" + motion_name + "` accepted. " +
                 (out.unbounded.size() == 1 ? "One parameter was" : "Some parameters were") +
                 " not checked against anything, because this body declares no bound for " + names +
                 " — which §5.2 reads as *nobody declared one*, never as unbounded.";
  }
  return out;
}

}  // namespace

Json execute(const std::string& label, const Json& input) {
  if (label.find("progress") != std::string::npos) {
    // §88.3's per-cycle question. Separate from the admission gate because one is asked once per
    // request and this is asked every cycle until the controller answers.
    double deadline = 0;
    double started = 0;
    double now = 0;
    finite_number(input["deadline_ms"], deadline);
    finite_number(input["started_at_ms"], started);
    finite_number(input["now_ms"], now);
    const double elapsed = now - started;
    const double over = elapsed - deadline;

    Json out = Json::object();
    // `>` rather than `>=`, as §87.4's lease is: the instant the deadline falls is still inside it.
    if (over > 0) {
      out.set("progress", Json::string("overdue"));
      out.set("overdue_by_ms", Json::number(over));
      out.set("detail",
              Json::string("The controller has held this motion for " +
                           js::number_to_string(elapsed) + " ms against a " +
                           js::number_to_string(deadline) +
                           " ms deadline. §7.1's watchdog has nothing to watch on this path "
                           "(§88.3), so this is the bound that fires: abort the motion, enter "
                           "§27's fault path, and record it."));
      return out;
    }
    out.set("progress", Json::string("running"));
    out.set("overdue_by_ms", Json::number(over));
    out.set("detail", Json::string("Inside its deadline, with " + js::number_to_string(-over) +
                                   " ms left."));
    return out;
  }

  if (label.find("motion") == std::string::npos) {
    throw Unimplemented("execute", label);
  }

  const Verdict verdict = admit_motion(input["gate"], input["request"]);

  // `arguments` is an **array in the motion's declaration order**, which is the whole reason it is
  // on the verdict rather than left to the caller: a program-level controller's motion is a call
  // with positional arguments, and an object would be alphabetised by canonical JSON into an
  // argument list no controller agreed to.
  Json arguments = Json::array();
  for (const Argument& entry : verdict.arguments) {
    Json row = Json::object();
    row.set("name", Json::string(entry.name));
    row.set("value", Json::number(entry.value));
    arguments.push(row);
  }
  Json unbounded = Json::array();
  for (const std::string& entry : verdict.unbounded) {
    unbounded.push(Json::string(entry));
  }

  Json out = Json::object();
  out.set("accepted", Json::boolean(verdict.accepted));
  out.set("refusal", verdict.refusal.empty() ? Json::null() : Json::string(verdict.refusal));
  out.set("detail", Json::string(verdict.detail));
  out.set("arguments", arguments);
  out.set("unbounded", unbounded);
  return out;
}

}  // namespace skynet::contracts
