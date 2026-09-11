// The operating state machine (§27, extended by §48.1), C++ edition.
//
// Transcribed from `reference/typescript/opstate.ts`. Every rule, reason and ordering decision is
// read from that file, never from `corpus/`.
//
// ── It is a function of what is observed, not a table of edges ─────────────
//
// §27 gives a table of **entry conditions**, not a list of edges, and that is the shape it keeps.
// `derive` is a pure function of the conditions observed right now, in a fixed priority order, and
// every state is reachable from every other by the conditions changing. There is no edge to miss,
// no transition anybody has to remember to fire, and recovery is not a special case — a guard
// coming back is the same evaluation over a different input.
//
// ── The machine has exactly one bit of memory, and that bit is `safe` ──────
//
//     `SAFE` is deliberately sticky. A robot that recovers itself from a safety stop is a robot
//     that resumes moving while somebody is standing next to it working out what happened.
//
// ── The ordering is §33.4's asymmetry, stated out loud ─────────────────────
//
//     Anything that reduces what the robot may do applies immediately … because making a robot less
//     capable is never the dangerous direction.
//
// So the rules are tried least-capable-first. Where two conditions both hold, the one that stops
// the robot wins, and the reason reported is that one — which is also what an operator needs,
// because the condition holding the robot is the condition they have to clear.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `conditions.post ?? 'incomplete'`
//     Nullish, not truthy, and every default here is **the least capable reading**. A caller who
//     has not observed something has not established it, and a machine that read "nobody told me
//     about the kernel" as "the kernel is live" would be the single most dangerous line in this
//     repository.
//
// `conditions.stop_settled === true` and `guard_hold === true`
//     Strict. A truthy non-boolean does not settle a stop, and does not hold a robot.
//
// `typeof cleared_by === 'string' && length > 0`
//     A clearance is a **named** human action. A caller with nobody to name should not have one, so
//     an empty string is not a clearance — and neither is a number.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/opstate.hpp"
#include "skynet_os/vocabulary.hpp"

#include <string>
#include <utility>

namespace skynet::opstate {
namespace {

/// `value ?? default` — nullish, so `0` and `""` survive it.
std::string or_default(const Json& conditions, const char* key, const char* fallback) {
  if (!conditions.has(key) || conditions[key].is_null()) {
    return fallback;
  }
  return std::string(conditions[key].string_or(""));
}

/// The reason a stop is in progress, by cause.
///
/// A closed mapping rather than a template literal, so an added cause is a case the corpus rejects
/// instead of a reason string nothing declares.
std::string stopping_reason(const std::string& cause) {
  if (cause == "watchdog") return "stopping_watchdog";
  if (cause == "proximity") return "stopping_proximity";
  if (cause == "balance") return "stopping_balance";
  if (cause == "operator") return "stopping_operator";
  if (cause == "estop") return "stopping_estop";
  throw JsonError("unknown stop cause " + cause);
}

/// A clearance is a named human action. An empty name is not one, and neither is a number.
bool cleared(const Json& conditions) {
  const Json& value = conditions["cleared_by"];
  return value.is_string() && !value.string_or("").empty();
}

/// The state and reason the conditions imply, before stickiness.
std::pair<std::string, std::string> derive(const Json& conditions) {
  const std::string rhal = or_default(conditions, "rhal", "lost");

  // 1. The kernel lost the body. First because it is the condition under which nothing else can be
  //    known: a robot whose RHAL is gone has no observed joint state, no estop reading and no guard
  //    that can evaluate.
  if (rhal == "lost") {
    return {"safe", "rhal_lost"};
  }

  // 2. A failed self-test. §48.2: `safe`, not `degraded`, "because a robot that failed its own
  //    self-test should not be talked into running by a reconcile loop".
  if (conditions["post"].is_string() && conditions["post"].string_or("") == "failed") {
    return {"safe", "post_failed"};
  }

  // 3. A stop, complete or in progress. Above POST because an operator stop is honoured whatever
  //    the robot is doing, including booting.
  if (conditions.has("stop") && !conditions["stop"].is_null()) {
    if (conditions["stop_settled"].is_true()) {
      return {"safe", "stop_complete"};
    }
    return {"stopping", stopping_reason(std::string(conditions["stop"].string_or("")))};
  }

  // 4. The self-test has not established that this robot may run. Not a fault: §48.4's
  //    `post_incomplete`, which is a robot waiting on the rest of the operating system.
  if (or_default(conditions, "post", "incomplete") != "passed") {
    return {"post", "post_incomplete"};
  }

  // 5. §48.1. Joint positions are unknown, forward kinematics is meaningless, and every
  //    pose-dependent guard is unavailable — so a policy must not run, and homing is the only
  //    motion permitted (§48.3).
  if (or_default(conditions, "homing", "pending") == "pending") {
    return {"unhomed", "homing_pending"};
  }

  // 6. §27: "a robot with a policy and no kernel must not exist". Refusing to start is `hold` — the
  //    safe resting state — rather than `safe`: nothing has gone wrong with the robot, and a sticky
  //    state needing a human would be the wrong answer for a kernel that is merely still starting.
  if (or_default(conditions, "kernel", "down") != "live") {
    return {"hold", "kernel_down"};
  }

  // 7. §27's `HOLD` row, first half. A guard asking for a hold outranks a policy that would like to
  //    run, which is the whole point of a guard.
  if (conditions["guard_hold"].is_true()) {
    return {"hold", "guard_hold"};
  }

  // 8. §27's `HOLD` row, second half. Split in two, because "there is nothing to run" and "there is
  //    something and it has not got a lease" are different sentences and the second is the one with
  //    a remedy — §25.2's arbitration is where an operator finds out who else is holding the arm.
  const std::string policy = or_default(conditions, "policy", "none");
  if (policy == "none") {
    return {"hold", "no_policy"};
  }
  if (policy != "ready") {
    return {"hold", "policy_not_ready"};
  }

  // 9. §27's `DEGRADED` row, below the policy rules deliberately. Reaching here means a policy is
  //    ready to command, so `degraded` is the impaired running state rather than a resting one.
  if (conditions["guards_unavailable"].is_true()) {
    return {"degraded", "guards_unavailable"};
  }
  if (rhal == "degraded") {
    return {"degraded", "rhal_degraded"};
  }

  return {"running", "nominal"};
}

}  // namespace

bool is_safe_state(std::string_view state) {
  // ``stopping`` is not one, and it is the interesting entry: a robot mid-ramp is the state that
  // most looks like it is about to be safe and most certainly is not — swapping a driver during the
  // ramp would change the meaning of the deceleration in flight. `safe`, `post` and `unhomed` are,
  // which is what makes an OS update apply on a robot that failed its self-test: the remedy for a
  // robot in `safe` is often the update.
  return state == "post" || state == "unhomed" || state == "hold" || state == "safe";
}

bool commands_autonomously(std::string_view state) {
  // `running` and `degraded`, and the second is the one that had been getting this wrong. §27's
  // `DEGRADED` row says the robot "**runs**, reports, and refuses any package whose grants depend on
  // the failed guard" — it is the impaired *running* state, not a third resting one.
  return state == "running" || state == "degraded";
}

bool is_sticky(std::string_view state) { return state == "safe"; }

bool warning_required(std::string_view from_state, std::string_view to_state) {
  // Phrased over `commands_autonomously` so the two spellings cannot come apart. The version this
  // replaced compared against the literal `running` and was under-inclusive: `hold → degraded` is a
  // policy beginning to command a body, so a robot with a lapsed calibration — §45.5's ordinary
  // case — would have begun autonomous motion silently. "Resumption after a stop" falls out,
  // because `stopping` and `safe` command nothing.
  return commands_autonomously(to_state) && !commands_autonomously(from_state);
}

Transition evaluate(const std::string& previous, const Json& conditions) {
  const auto [state, reason] = derive(conditions);

  // §27's stickiness, applied after the derivation rather than instead of it. A robot in `safe`
  // whose conditions would put it back in `safe` reports the *condition's* reason rather than
  // `safe_sticky`, because "the estop is still pressed" is what an operator needs and "you have not
  // cleared it yet" is not.
  const bool stuck = previous == "safe" && !cleared(conditions) && state != "safe";

  Transition out;
  out.from = previous;
  out.to = stuck ? "safe" : state;
  out.reason = stuck ? "safe_sticky" : reason;
  out.changed = out.to != previous;
  out.warning = warning_required(previous, out.to);
  return out;
}

}  // namespace skynet::opstate

namespace skynet::contracts {

Json opstate(const std::string& label, const Json& input) {
  if (label.find("evaluate") != std::string::npos) {
    const skynet::opstate::Transition transition =
        skynet::opstate::evaluate(std::string(input["previous"].string_or("")),
                                  input["conditions"]);
    Json out = Json::object();
    out.set("from", Json::string(transition.from));
    out.set("to", Json::string(transition.to));
    out.set("reason", Json::string(transition.reason));
    out.set("changed", Json::boolean(transition.changed));
    out.set("warning", Json::boolean(transition.warning));
    return out;
  }

  const vocabulary::Set& states = vocabulary::members("operating_states");

  if (label.find("properties") != std::string::npos) {
    // Pinned as a table rather than case by case because the three predicates are read by different
    // sections, and an edition could get one right and another wrong for the same state without any
    // single caller noticing.
    Json out = Json::array();
    for (const std::string_view state : states) {
      Json row = Json::object();
      row.set("state", Json::string(std::string(state)));
      row.set("safe_state", Json::boolean(skynet::opstate::is_safe_state(state)));
      row.set("commands", Json::boolean(skynet::opstate::commands_autonomously(state)));
      row.set("sticky", Json::boolean(skynet::opstate::is_sticky(state)));
      out.push(row);
    }
    return out;
  }

  if (label.find("warning") != std::string::npos) {
    // All forty-nine ordered pairs, because the rule that was wrong before was wrong on one pair and
    // right on the one everybody tested.
    Json out = Json::array();
    for (const std::string_view from_state : states) {
      for (const std::string_view to_state : states) {
        Json row = Json::object();
        row.set("from", Json::string(std::string(from_state)));
        row.set("to", Json::string(std::string(to_state)));
        row.set("warning", Json::boolean(skynet::opstate::warning_required(from_state, to_state)));
        out.push(row);
      }
    }
    return out;
  }

  throw Unimplemented("opstate", label);
}

}  // namespace skynet::contracts
