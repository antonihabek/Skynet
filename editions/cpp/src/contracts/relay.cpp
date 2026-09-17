// The Hub-brokered console relay (§87.4), C++ edition of `reference/typescript/relay.ts`.
//
// §39.2 is unambiguous about the LAN listener — *"reaching a robot's console from outside its
// network is not a feature and is not a configuration option"* — and §43 calls that *"right for
// security and wrong"* for one robot in particular: one in a customer's building with nobody on
// site. **Nothing here makes anything routable.** The robot dials out, which is §34's bearer
// machinery rather than a second listener, and this file is about what the session on the far end
// may do once it exists.
//
// Three decisions, and the third is the substance:
//
//   * **A relayed operator is an operator.** §5.1's roles are about *authority*; a route is about
//     *latency*.
//   * **An estop over a relay is not an estop.** §39.1 makes stop unauthenticated because a delay in
//     front of a stop button is a hazard, and a relay is an *unbounded* delay in front of the same
//     control — worse rather than milder, because it looks like the control that works.
//   * **A relayed session may hold a lease, and the deadline is the robot's.** Every other console
//     session ends when somebody unplugs something.
//
// **No maximum is invented here.** The figure is the operator profile's, for §5.2's reason. What
// this file refuses is a relay session with *no* declared deadline.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/vocabulary.hpp"

#include <algorithm>
#include <cmath>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

/// The least role §39.1 requires for each action, in `console_actions` order.
const std::vector<std::pair<std::string, std::string>>& requires_role() {
  static const std::vector<std::pair<std::string, std::string>> table = {
      {"stop_request", "anonymous"},   {"read_status", "anonymous"},
      {"read_health", "observer"},     {"start_package", "operator"},
      {"stop_package", "operator"},    {"grant_capability", "operator"},
      {"revoke_capability", "operator"}, {"seal_blackbox", "operator"},
      {"apply_update", "operator"},    {"pair", "admin"},
      {"install_driver", "admin"},     {"change_profile", "admin"},
      {"factory_reset", "admin"},
  };
  return table;
}

/// The actions §87.4 refuses over a relay whatever the role.
///
/// §39.2 bootstraps with a chassis button or a printed code *"because possession of the machine is
/// the only credential that means anything before any other credential exists"*. These are the
/// actions whose **recovery** needs somebody present. `change_profile` is deliberately not here: a
/// profile is §38.2's narrowing-only stack.
bool presence_required(const std::string& action) {
  return action == "pair" || action == "install_driver" || action == "factory_reset";
}

/// The actions that widen what the robot may do.
bool widening(const std::string& action) { return action == "grant_capability"; }

int role_rank(const std::string& role) {
  const vocabulary::Set& roles = vocabulary::members("console_roles");
  for (std::size_t at = 0; at < roles.size(); ++at) {
    if (roles[at] == role) {
      return static_cast<int>(at);
    }
  }
  return -1;
}

bool known_route(const std::string& route) {
  const vocabulary::Set& routes = vocabulary::members("relay_routes");
  return std::find(routes.begin(), routes.end(), route) != routes.end();
}

struct SessionVerdict {
  std::string state;
  std::string refusal;
  std::string detail;
  bool has_expires = false;
  double expires = 0;
};

bool finite_number(const Json& value, double& out) {
  if (!value.is_number()) {
    return false;
  }
  out = value.number_or(0);
  return std::isfinite(out);
}

/// Whether the session may be used at all, and until when.
///
/// Three states rather than two, and the third is §80's `inert` precedent: a session **refused at
/// open** and one that **expired after working** send a person to do two different things.
///
/// A LAN session needs no deadline and is never expired here. §39.2 makes the LAN listener
/// link-local and unrouted, so the cable *is* the deadline, and a timer invented here would log an
/// operator out of a console they are standing in front of.
SessionVerdict relay_session(const Json& session, double now_ms) {
  const std::string route(session["route"].string_or(""));
  if (!known_route(route)) {
    return {"refused", "route_unknown",
            "`" + route +
                "` is not one of lan or relay, so nothing can say whether this session can be "
                "ended by unplugging something (§87.4).",
            false, 0};
  }
  const std::string role(session["role"].string_or(""));
  if (role_rank(role) < 0) {
    return {"refused", "role_unknown",
            "`" + role +
                "` is not one of §39.1's four roles, and a session whose authority nobody can "
                "read holds none.",
            false, 0};
  }

  if (route == "lan") {
    return {"live", "",
            "A LAN session ends when somebody unplugs something, which is the property §87.4 "
            "is about and the reason it needs no lease.",
            false, 0};
  }

  double lease = 0;
  if (!finite_number(session["lease_ms"], lease) || lease <= 0) {
    return {"refused", "deadline_undeclared",
            "A relay session cannot be ended by pulling a cable, so it is ended by a deadline the "
            "robot enforces. One with no deadline is a robot holding authority for somebody who "
            "may already have left. The figure is the operator profile's — §5.2 forbids a "
            "platform inventing one (§87.4).",
            false, 0};
  }

  double renewed = 0;
  if (!finite_number(session["renewed_at_ms"], renewed)) {
    return {"refused", "deadline_undeclared",
            "A relay session declares a lease and nothing has recorded when it was last renewed, "
            "so the deadline has no start. A lease whose clock never started never runs out "
            "(§87.4).",
            false, 0};
  }

  const double expires = renewed + lease;
  // `>` rather than `>=`: the instant the lease runs out is still inside it, which is the reading
  // every other deadline in this codebase takes.
  if (now_ms > expires) {
    return {"expired", "",
            "The lease ran out and no renewal arrived over the relay, so the robot ended the "
            "session itself rather than asking the Hub whether it should (§87.4).",
            true, expires};
  }
  return {"live", "", "Renewed inside its lease.", true, expires};
}

struct ActionVerdict {
  bool allowed = false;
  std::string refusal;
  std::string detail;
  bool bounded = false;
};

/// Whether this session may perform this action.
///
/// The order is the design, and the first rule is the one §39.1 inverts: `stop_request` is always
/// allowed, whatever the role, the route and the session's state, because it never needed the
/// session. A refused or expired session permits nothing else. Then §39.1's role table, unchanged by
/// the route; then presence, over a relay only — read in that order, because being an observer is
/// the fact that would still be true if they walked over to the robot.
ActionVerdict relay_action(const Json& session, const std::string& action, double now_ms) {
  const SessionVerdict verdict = relay_session(session, now_ms);

  if (action == "stop_request") {
    return {true, "",
            verdict.state == "live"
                ? "Stop is never privileged (§39.1). Over a relay it is a *request*, with the "
                  "robot's own acknowledgement beside it, because a relay has no latency bound and "
                  "a control that looks like an estop and is not one is the hazard §87.4 "
                  "refuses to build."
                : "Stop never needed the session. §39.1 makes it unauthenticated, so this "
                  "arrives as an anonymous stop request — which is what it is.",
            false};
  }

  if (verdict.state != "live") {
    return {false, verdict.state == "expired" ? "session_expired" : "session_refused",
            verdict.detail, false};
  }

  const std::string role(session["role"].string_or(""));
  std::string required = "admin";
  bool known = false;
  for (const auto& entry : requires_role()) {
    if (entry.first == action) {
      required = entry.second;
      known = true;
    }
  }
  if (!known || role_rank(role) < role_rank(required)) {
    return {false, "role_insufficient",
            known ? "§39.1 requires at least " + required +
                        " for this, and this session holds " + role + "."
                  : "`" + action +
                        "` is not one of §39.1's actions, so the most this console will assume "
                        "about it is that it needs admin — and this session does not hold it.",
            false};
  }

  const std::string route(session["route"].string_or(""));
  if (route == "relay" && presence_required(action)) {
    return {false, "presence_required",
            "§39.2 bootstraps with a chassis button or a printed code, because possession of "
            "the machine is the only credential that means anything before any other exists. This "
            "is an action whose recovery needs somebody present, and a relay is the case where "
            "nobody is (§87.4).",
            false};
  }

  const bool bounded = route == "relay" && widening(action);
  return {true, "",
          bounded ? "Allowed, and its effect ends with the session: a grant made over a relay that "
                    "outlived the session would leave the robot holding authority nobody present "
                    "agreed to and nobody remote can still revoke (§87.4)."
                  : "Allowed by §39.1, and the route does not narrow it.",
          bounded};
}

}  // namespace

Json relay(const std::string& /*label*/, const Json& input) {
  const Json& session = input["session"];
  double now_ms = 0;
  if (!finite_number(input["now_ms"], now_ms)) {
    now_ms = 0;
  }

  const SessionVerdict verdict = relay_session(session, now_ms);
  Json body = Json::object();
  body.set("route", session["route"]);
  body.set("role", session["role"]);
  body.set("state", Json::string(verdict.state));
  body.set("refusal", verdict.refusal.empty() ? Json::null() : Json::string(verdict.refusal));
  body.set("detail", Json::string(verdict.detail));
  body.set("expires_at_ms", verdict.has_expires ? Json::number(verdict.expires) : Json::null());

  // The whole table rather than one answer, for §78's reason: a surface that asked per control would
  // be a second place §39.1's rules are written down, in whatever order its layout happened to be.
  Json actions = Json::array();
  for (const std::string_view named : vocabulary::members("console_actions")) {
    const std::string action(named);
    const ActionVerdict entry = relay_action(session, action, now_ms);
    Json row = Json::object();
    row.set("action", Json::string(action));
    row.set("allowed", Json::boolean(entry.allowed));
    row.set("refusal", entry.refusal.empty() ? Json::null() : Json::string(entry.refusal));
    row.set("bounded_by_session", Json::boolean(entry.bounded));
    row.set("detail", Json::string(entry.detail));
    actions.push(row);
  }

  Json out = Json::object();
  out.set("session", body);
  out.set("actions", actions);
  return out;
}

}  // namespace skynet::contracts
