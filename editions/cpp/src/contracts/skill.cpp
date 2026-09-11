// Skills — preconditions, success, timeout, composition and the lifecycle (§54). C++ edition.
//
// Transcribed from `reference/typescript/skill.ts` and its shapes in `reference/typescript/shape.ts`.
// Every rule and message string is read from those files, never from `corpus/`.
//
// The asymmetry is the substantive decision, and it is the reference's: **a precondition nobody can
// evaluate refuses the skill** (§54.2 — not "assume true", which is what free-text preconditions do
// by construction), and **a criterion nobody can evaluate makes the outcome `unlabeled`, not
// `failure`** (§54.3 — honest ignorance beats an invented pass). Before a skill runs, ignorance is a
// reason not to act; after it runs, a reason not to claim.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `Math.round`
//     `js::math_round`, ties toward +∞ and exact on the value itself — not a half-away rounding,
//     and not `floor(x + 0.5)`.
//
// `typeof x === 'object'`
//     True for an array, false for null. `readClauses` takes a predicate that is any truthy object,
//     arrays included, and hands it to the evaluator, which refuses what it cannot read.
//
// The composition walk
//     Depth-first in declaration order, which is what makes the chain a fault names reproducible.
//     Written as an explicit stack rather than recursion: the depth is bounded by the input, and a
//     walk whose stack an input can size is a crash an input can cause.
//
// `!progress.lease_token`, `!deadline.enforceable`
//     Truthiness, `js::truthy`. Everything else in the lifecycle is `=== true` — `Json::is_true` —
//     because an absent piece of evidence must never read as a gathered one.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/predicate.hpp"

#include <algorithm>
#include <cmath>
#include <map>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

/// §54.4: how far past its own estimate a skill may run. An operator setting with a default, not a
/// manifest field — a publisher choosing their own overrun is a publisher with no timeout.
constexpr double kDefaultOverrunFactor = 2;

/// §54.5's default depth. The root counts as depth 1.
constexpr double kMaxSkillDepth = 4;

Json optional_string(const std::optional<std::string>& value) {
  return value.has_value() ? Json::string(*value) : Json::null();
}

// ── §54.2 and §54.3: clauses ─────────────────────────────────────────────────

struct Clause {
  std::string statement;
  /// Absent means the OS cannot enforce this clause at all.
  std::optional<Json> predicate;
  bool hold = false;
  std::string origin;
};

/// `readClauses`: a bare string is prose with no predicate; an object is a statement and maybe a
/// predicate; anything else is skipped.
std::vector<Clause> read_clauses(const Json& input, const std::string& origin) {
  std::vector<Clause> clauses;
  for (const Json& entry : input.items()) {
    if (entry.is_string()) {
      clauses.push_back({std::string(entry.string_or("")), std::nullopt, false, origin});
      continue;
    }
    // `typeof entry !== 'object'` — and an array is an object, so an array entry is a clause with no
    // statement and no predicate rather than one that is skipped.
    if (!entry.is_object() && !entry.is_array()) {
      continue;
    }
    const Json& predicate = entry["predicate"];
    const bool usable = js::truthy(predicate) && (predicate.is_object() || predicate.is_array());
    clauses.push_back({entry["statement"].is_string() ? std::string(entry["statement"].string_or(""))
                                                      : std::string(),
                       usable ? std::optional<Json>(predicate) : std::nullopt,
                       entry["hold"].is_true(), origin});
  }
  return clauses;
}

struct ClauseResult {
  std::size_t index = 0;
  std::string statement;
  std::string origin;
  bool hold = false;
  std::string outcome;
  std::optional<std::string> reason;
  std::optional<std::string> path;
  std::optional<std::string> op;
};

ClauseResult judge_clause(const Clause& clause, std::size_t index, const Json& context) {
  ClauseResult result{index, clause.statement, clause.origin, clause.hold, "unenforceable", {}, {}, {}};
  if (!clause.predicate.has_value()) {
    return result;
  }
  const predicate::Outcome outcome = predicate::evaluate_outcome(*clause.predicate, context);
  result.outcome = outcome.outcome;
  result.reason = outcome.reason;
  result.path = outcome.path;
  result.op = outcome.op;
  return result;
}

Json clause_result_json(const ClauseResult& result) {
  Json out = Json::object();
  out.set("index", Json::number(static_cast<double>(result.index)));
  out.set("statement", Json::string(result.statement));
  out.set("origin", Json::string(result.origin));
  out.set("hold", Json::boolean(result.hold));
  out.set("outcome", Json::string(result.outcome));
  out.set("reason", optional_string(result.reason));
  out.set("path", optional_string(result.path));
  out.set("op", optional_string(result.op));
  return out;
}

Json clause_results_json(const std::vector<ClauseResult>& results) {
  Json out = Json::array();
  for (const ClauseResult& result : results) {
    out.push(clause_result_json(result));
  }
  return out;
}

/// §54.2 — may this skill start, or continue? The skill's clauses, then the site's: the operator's
/// list is only ever appended, so there is nothing by which it could shorten the result. Every
/// clause is evaluated after one fails, and `refused` names the first.
Json preconditions(const Json& input) {
  std::vector<Clause> clauses = read_clauses(input["published"], "skill");
  for (Clause& clause : read_clauses(input["operator"], "operator")) {
    clauses.push_back(std::move(clause));
  }
  const std::string phase(input["phase"].string_or("entry"));

  std::vector<ClauseResult> results;
  for (std::size_t index = 0; index < clauses.size(); ++index) {
    if (phase == "hold" && !clauses[index].hold) {
      continue;
    }
    results.push_back(judge_clause(clauses[index], index, input["context"]));
  }

  const auto refused = std::find_if(results.begin(), results.end(), [](const ClauseResult& result) {
    return result.outcome != "satisfied";
  });
  const auto unenforceable = std::count_if(results.begin(), results.end(), [](const ClauseResult& result) {
    return result.outcome == "unenforceable";
  });

  Json out = Json::object();
  out.set("phase", Json::string(phase));
  out.set("permitted", Json::boolean(refused == results.end()));
  out.set("refused", refused == results.end() ? Json::null() : clause_result_json(*refused));
  out.set("results", clause_results_json(results));
  out.set("unenforceable", Json::number(static_cast<double>(unenforceable)));
  return out;
}

/// §54.3 — judged by the OS over post-state, never by the skill. Definitely false beats unread;
/// unread is `unlabeled`; only every criterion holding is a success; and no criteria at all is
/// `unlabeled`, because a skill that scores itself by declaring nothing is the gaming §54.3 stops.
Json success(const Json& input) {
  const std::vector<Clause> clauses = read_clauses(input["criteria"], "skill");
  std::vector<ClauseResult> results;
  for (std::size_t index = 0; index < clauses.size(); ++index) {
    results.push_back(judge_clause(clauses[index], index, input["post_state"]));
  }

  std::string outcome = "success";
  const ClauseResult* deciding = nullptr;
  const auto failed = std::find_if(results.begin(), results.end(), [](const ClauseResult& result) {
    return result.outcome == "unsatisfied";
  });
  if (failed != results.end()) {
    outcome = "failure";
    deciding = &*failed;
  } else {
    const auto unread = std::find_if(results.begin(), results.end(), [](const ClauseResult& result) {
      return result.outcome != "satisfied";
    });
    if (unread != results.end()) {
      outcome = "unlabeled";
      deciding = &*unread;
    } else if (results.empty()) {
      outcome = "unlabeled";
    }
  }

  Json out = Json::object();
  out.set("outcome", Json::string(outcome));
  out.set("deciding", deciding == nullptr ? Json::null() : clause_result_json(*deciding));
  out.set("results", clause_results_json(results));
  return out;
}

// ── §54.4 Timeout, and §54.5's bound on it ───────────────────────────────────

struct Deadline {
  std::optional<double> deadline_ms;
  bool enforceable = false;
  double factor = kDefaultOverrunFactor;
};

bool is_finite_number(const Json& value) {
  return value.is_number() && std::isfinite(value.number_or(0));
}

/// `skillDeadline`. A skill that declares no duration is `enforceable: false`, not a guessed
/// ceiling — and what the runtime does about that is not decided in a pure function.
Deadline skill_deadline(const Json& expected_seconds, const Json& overrun_factor) {
  Deadline out;
  if (is_finite_number(overrun_factor) && overrun_factor.number_or(0) >= 1) {
    out.factor = overrun_factor.number_or(0);
  }
  if (!is_finite_number(expected_seconds) || expected_seconds.number_or(0) <= 0) {
    return out;
  }
  out.deadline_ms = js::math_round(expected_seconds.number_or(0) * 1000 * out.factor);
  out.enforceable = true;
  return out;
}

Json deadline_json(const Deadline& deadline) {
  Json out = Json::object();
  out.set("deadline_ms", deadline.deadline_ms.has_value() ? Json::number(*deadline.deadline_ms) : Json::null());
  out.set("enforceable", Json::boolean(deadline.enforceable));
  out.set("factor", Json::number(deadline.factor));
  return out;
}

/// §54.5: the root's timeout bounds the whole tree. A child with no duration is still bounded by its
/// parent, and a root with nothing left bounds its child at zero rather than at a negative number.
Deadline bounded_deadline(const Deadline& child, const Json& root_remaining) {
  if (!is_finite_number(root_remaining)) {
    return child;
  }
  const double remaining = std::max(0.0, root_remaining.number_or(0));
  if (!child.enforceable || !child.deadline_ms.has_value()) {
    return {remaining, true, child.factor};
  }
  return {std::min(*child.deadline_ms, remaining), true, child.factor};
}

// ── §54.5 Composition ────────────────────────────────────────────────────────

/// §54.5 — is this call graph installable? Depth-limited and acyclic, refused at install by walking
/// the declared graph rather than discovered by a robot that has already taken a lease.
Json composition(const Json& input) {
  const std::string root(input["root"].string_or(""));
  const Json& max_depth = input["max_depth"];
  const double limit = is_finite_number(max_depth) && max_depth.number_or(0) >= 1
                           ? std::floor(max_depth.number_or(0))
                           : kMaxSkillDepth;

  // `readCalls`, then the edges in declaration order.
  std::map<std::string, std::vector<std::string>> edges;
  for (const Json& call : input["calls"].items()) {
    if (!call.is_object() || !call["from"].is_string() || !call["to"].is_string()) {
      continue;
    }
    edges[std::string(call["from"].string_or(""))].emplace_back(call["to"].string_or(""));
  }
  const std::vector<std::string> none;
  const auto children = [&](const std::string& node) -> const std::vector<std::string>& {
    const auto found = edges.find(node);
    return found == edges.end() ? none : found->second;
  };

  Json fault = Json::null();
  const auto refuse = [&](const char* code, const std::vector<std::string>& path, std::string detail) {
    Json chain = Json::array();
    for (const std::string& node : path) {
      chain.push(Json::string(node));
    }
    fault = Json::object();
    fault.set("code", Json::string(code));
    fault.set("path", chain);
    fault.set("detail", Json::string(std::move(detail)));
  };

  // The reference's recursion, as a stack of frames: each frame is a node on the current chain and
  // the index of the next of its edges to follow. Entering a node is where the depth is checked, so
  // the root is checked before anything is followed — and a limit is at least 1, so it never fails.
  struct Frame {
    std::string node;
    std::size_t next;
  };
  std::vector<std::string> path{root};
  std::vector<Frame> stack{{root, 0}};
  while (!stack.empty()) {
    Frame& frame = stack.back();
    const std::vector<std::string>& out = children(frame.node);
    if (frame.next >= out.size()) {
      stack.pop_back();
      path.pop_back();
      continue;
    }
    const std::string next = out[frame.next++];
    if (std::find(path.begin(), path.end(), next) != path.end()) {
      std::vector<std::string> chain = path;
      chain.push_back(next);
      refuse("cycle", chain, next + " is already in this call chain, so it would call itself.");
      break;
    }
    path.push_back(next);
    if (static_cast<double>(path.size()) > limit) {
      refuse("too_deep", path,
             "This call chain is " + std::to_string(path.size()) + " skills deep; the limit is " +
                 js::number_to_string(limit) + ".");
      break;
    }
    stack.push_back({next, 0});
  }

  Json out = Json::object();
  out.set("fault", fault);
  return out;
}

// ── §54.4 The lifecycle ──────────────────────────────────────────────────────

/// Whether a robot in this state has stopped a skill that was running. `hold` is deliberately not
/// one: it is where §27's machine sits between a lease and a policy commanding.
bool robot_stopped_commanding(const std::string& state) {
  return state == "stopping" || state == "safe" || state == "post" || state == "unhomed";
}

/// Whether the deadline has passed. An unenforceable deadline never has.
bool overran(const Json& progress) {
  const Json& deadline = progress["deadline"];
  if (!js::truthy(deadline) || !js::truthy(deadline["enforceable"]) || deadline["deadline_ms"].is_null()) {
    return false;
  }
  const Json& elapsed = progress["elapsed_ms"];
  if (!is_finite_number(elapsed)) {
    return false;
  }
  const Json& limit = deadline["deadline_ms"];
  return limit.is_number() && elapsed.number_or(0) >= limit.number_or(0);
}

struct Abort {
  std::string reason;
  std::string detail;
};

/// Why a running skill is over, or nothing. Causes before effects: a timeout and a guard trip each
/// produce a robot that has stopped commanding, so they are tried first and `robot_stopped` catches
/// whatever this machine could not name.
std::optional<Abort> abort_reason(const Json& progress) {
  if (overran(progress)) {
    const Json& deadline = progress["deadline"];
    return Abort{"timeout",
                 "past " + js::interpolate(deadline.has("deadline_ms") ? &deadline["deadline_ms"] : nullptr) +
                     " ms, which is the declared estimate times " +
                     js::interpolate(deadline.has("factor") ? &deadline["factor"] : nullptr)};
  }
  if (progress["hold_lost"].is_true()) {
    return Abort{"hold_lost",
                 "a precondition marked `hold` stopped being true, and §54.2 re-checks those every "
                 "cycle rather than only at entry"};
  }
  if (progress["guard_tripped"].is_true()) {
    return Abort{"guard_tripped", "a guard tripped, and a skill is not an exemption"};
  }
  const Json& robot_field = progress["robot"];
  const std::string robot = robot_field.is_null() ? "post" : js::interpolate(&robot_field);
  if (robot_stopped_commanding(robot)) {
    return Abort{"robot_stopped",
                 "§27 says the robot is `" + robot + "`, which is not a state a policy commands in"};
  }
  return std::nullopt;
}

/// What a denied acquisition tells the operator: a named holder is somebody else using it, and no
/// holder is a capability this package was never granted. Two errands, two sentences.
std::string lease_denied_detail(const Json& progress) {
  if (!js::truthy(progress["lease_token"])) {
    return "a required capability could not be leased, and nothing named which";
  }
  const std::string token = js::interpolate(&progress["lease_token"]);
  if (!js::truthy(progress["lease_holder"])) {
    return "`" + token +
           "` was never granted to this package, so §25.2 will not let it take the lease — nobody is "
           "holding it. Grant it, or take it out of `required_capabilities`.";
  }
  return "`" + token + "` is held by " + js::interpolate(&progress["lease_holder"]);
}

/// §54.4's lifecycle, advanced by one evaluation. A phase advances on evidence, and absence of
/// evidence is not advancement.
Json advance(const std::string& phase, const Json& progress) {
  const auto done = [&](const std::string& to, const char* reason, Json outcome, std::string detail) {
    Json out = Json::object();
    out.set("from", Json::string(phase));
    out.set("to", Json::string(to));
    out.set("reason", Json::string(reason));
    out.set("outcome", std::move(outcome));
    out.set("detail", Json::string(std::move(detail)));
    // Only on the way in: a skill already terminal releases nothing, because it has nothing.
    out.set("release_leases", Json::boolean(to == "terminal" && phase != "terminal"));
    out.set("changed", Json::boolean(to != phase));
    return out;
  };
  const Json aborted = Json::string("aborted");

  if (phase == "idle") {
    return progress["invoked"].is_true()
               ? done("preconditions", "invoked", Json::null(), "an invocation arrived")
               : done("idle", "not_invoked", Json::null(), "nothing has asked for this skill");
  }

  if (phase == "preconditions") {
    // Absent is not permitted: a runtime that had not evaluated §54.2 must not be able to start a
    // skill by failing to ask.
    const Json& permitted = progress["entry_permitted"];
    if (!permitted.is_bool()) {
      return done("preconditions", "preconditions_pending", Json::null(),
                  "the entry predicates have not been evaluated, and an unevaluated precondition is "
                  "not a satisfied one");
    }
    if (!permitted.is_true()) {
      return done("terminal", "preconditions_refused", aborted,
                  js::truthy(progress["refused_clause"])
                      ? "refused by: " + js::interpolate(&progress["refused_clause"])
                      : "refused by a precondition");
    }
    // §54.5: a nested invocation does not acquire; the root holds the leases for the whole tree.
    const Json& root = progress["root"];
    return root.is_bool() && !root.is_true()
               ? done("running", "leases_held_by_root", Json::null(),
                      "a nested call runs under the root invocation's leases (§54.5)")
               : done("acquiring", "preconditions_met", Json::null(), "every evaluated precondition held");
  }

  if (phase == "acquiring") {
    // §36.1's `low` row, before the table is consulted, and terminal rather than a wait. An empty
    // sentence is not a refusal: §64 makes a refusal name its fix.
    const Json& withheld_field = progress["power_withheld"];
    const std::string withheld = withheld_field.is_null() ? "" : js::trim(js::interpolate(&withheld_field));
    if (!withheld.empty()) {
      return done("terminal", "power_withheld", aborted, withheld);
    }
    const Json& leases_field = progress["leases"];
    const std::string leases = leases_field.is_null() ? "pending" : js::interpolate(&leases_field);
    if (leases == "denied") {
      return done("terminal", "lease_denied", aborted, lease_denied_detail(progress));
    }
    if (leases != "held") {
      return done("acquiring", "lease_pending", Json::null(),
                  "the leases for `required_capabilities` have not been taken");
    }
    return done("running", "leases_acquired", Json::null(), "every required capability is leased");
  }

  if (phase == "running") {
    if (const std::optional<Abort> stop = abort_reason(progress)) {
      return done("terminal", stop->reason.c_str(), aborted, stop->detail);
    }
    return progress["finished"].is_true()
               ? done("verifying", "verifying", Json::null(), "the skill returned; §54.3 has not judged it yet")
               : done("running", "running", Json::null(), "the skill is running");
  }

  if (phase == "verifying") {
    // The abort outranks the verdict: a skill whose robot fell over during verification did not
    // succeed at anything.
    if (const std::optional<Abort> stop = abort_reason(progress)) {
      return done("terminal", stop->reason.c_str(), aborted, stop->detail);
    }
    const Json& outcome = progress["outcome"];
    if (outcome.is_null()) {
      return done("verifying", "verdict_pending", Json::null(),
                  "the success criteria have not been judged, and §54.3 will not let the skill judge "
                  "itself");
    }
    return done("terminal", "judged", outcome, "§54.3 judged this " + js::interpolate(&outcome));
  }

  if (phase == "terminal") {
    return done("terminal", "terminal", progress["outcome"], "this invocation is over");
  }

  // The reference's `switch` has no default and returns nothing for a phase it does not have, which
  // no caller can shape. A failure, then — not `Unimplemented`, which would count a malformed case
  // as a gap in this edition.
  throw std::invalid_argument("a phase §54.4 does not have: " + phase);
}

}  // namespace

Json skill(const std::string& label, const Json& input) {
  if (label == "preconditions") {
    return preconditions(input);
  }
  if (label == "success") {
    return success(input);
  }
  if (label == "timeout") {
    return deadline_json(skill_deadline(input["expected_duration_seconds"], input["overrun_factor"]));
  }
  if (label == "bounded-deadline") {
    const Deadline child = skill_deadline(input["expected_duration_seconds"], Json::number(kDefaultOverrunFactor));
    return deadline_json(bounded_deadline(child, input["root_remaining_ms"]));
  }
  if (label == "composition") {
    return composition(input);
  }
  if (label == "lifecycle") {
    return advance(std::string(input["phase"].string_or("")), input["progress"]);
  }
  throw Unimplemented("skill", label);
}

}  // namespace skynet::contracts
