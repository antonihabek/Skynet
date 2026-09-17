package contracts

import (
	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// The Hub-brokered console relay (§87.4), and what a session reached that way may do.
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
//   - **A relayed operator is an operator.** §5.1's roles are about *authority*; a route is about
//     *latency*. A fifth role would mean an engineer who drove to the site and reached the same
//     relay from the car park held less authority than the same person on the LAN.
//   - **An estop over a relay is not an estop.** §39.1 makes stop unauthenticated because a delay
//     in front of a stop button is a hazard, and a relay is an *unbounded* delay in front of the
//     same control — worse rather than milder, because it looks like the control that works.
//   - **A relayed session may hold a lease, and the deadline is the robot's.** Every other console
//     session ends when somebody unplugs something.
//
// **No maximum is invented here.** The figure is the operator profile's, for §5.2's reason. What
// this file refuses is a relay session with *no* declared deadline.

// requiresRole is the least role §39.1 requires for each action, in ConsoleActions order.
var requiresRole = map[string]string{
	"stop_request":      "anonymous",
	"read_status":       "anonymous",
	"read_health":       "observer",
	"start_package":     "operator",
	"stop_package":      "operator",
	"grant_capability":  "operator",
	"revoke_capability": "operator",
	"seal_blackbox":     "operator",
	"apply_update":      "operator",
	"pair":              "admin",
	"install_driver":    "admin",
	"change_profile":    "admin",
	"factory_reset":     "admin",
}

// presenceRequired is the actions §87.4 refuses over a relay whatever the role.
//
// §39.2 bootstraps with a chassis button or a printed code *"because possession of the machine is
// the only credential that means anything before any other credential exists"*. These are the
// actions whose **recovery** needs somebody present. `change_profile` is deliberately not here: a
// profile is §38.2's narrowing-only stack.
var presenceRequired = []string{"pair", "install_driver", "factory_reset"}

// widening is the actions that widen what the robot may do.
var widening = []string{"grant_capability"}

// SessionVerdict is whether a session may be used at all, and until when.
type SessionVerdict struct {
	State   string
	Refusal string
	Detail  string
	// ExpiresAt is absent where there is no lease to run out.
	ExpiresAt    float64
	HasExpiresAt bool
}

// ActionVerdict is what one session may do about one action.
type ActionVerdict struct {
	Action           string
	Allowed          bool
	Refusal          string
	Detail           string
	BoundedBySession bool
}

func roleRank(role string) int {
	for at, known := range vocabulary.ConsoleRoles {
		if known == role {
			return at
		}
	}
	return -1
}

// RelaySession is whether the session may be used at all, and until when.
//
// Three states rather than two, and the third is §80's `inert` precedent: a session **refused at
// open** and one that **expired after working** send a person to do two different things — write a
// deadline into the profile, or reconnect.
//
// A LAN session needs no deadline and is never expired here. §39.2 makes the LAN listener
// link-local and unrouted, so the cable *is* the deadline, and a timer invented here would log an
// operator out of a console they are standing in front of.
func RelaySession(session canonical.Value, nowMs float64) SessionVerdict {
	route, _ := session.Field("route").Str()
	if !containsString(vocabulary.RelayRoutes, route) {
		return SessionVerdict{
			State:   "refused",
			Refusal: "route_unknown",
			Detail: "`" + route + "` is not one of lan or relay, so nothing can say whether this " +
				"session can be ended by unplugging something (§87.4).",
		}
	}
	role, _ := session.Field("role").Str()
	if roleRank(role) < 0 {
		return SessionVerdict{
			State:   "refused",
			Refusal: "role_unknown",
			Detail: "`" + role + "` is not one of §39.1's four roles, and a session whose " +
				"authority nobody can read holds none.",
		}
	}

	if route == "lan" {
		return SessionVerdict{
			State: "live",
			Detail: "A LAN session ends when somebody unplugs something, which is the property " +
				"§87.4 is about and the reason it needs no lease.",
		}
	}

	lease, hasLease := finiteNumber(session.Field("lease_ms"))
	if !hasLease || lease <= 0 {
		return SessionVerdict{
			State:   "refused",
			Refusal: "deadline_undeclared",
			Detail: "A relay session cannot be ended by pulling a cable, so it is ended by a " +
				"deadline the robot enforces. One with no deadline is a robot holding authority " +
				"for somebody who may already have left. The figure is the operator profile's — " +
				"§5.2 forbids a platform inventing one (§87.4).",
		}
	}

	renewed, hasRenewed := finiteNumber(session.Field("renewed_at_ms"))
	if !hasRenewed {
		return SessionVerdict{
			State:   "refused",
			Refusal: "deadline_undeclared",
			Detail: "A relay session declares a lease and nothing has recorded when it was last " +
				"renewed, so the deadline has no start. A lease whose clock never started never " +
				"runs out (§87.4).",
		}
	}

	expires := renewed + lease
	// `>` rather than `>=`: the instant the lease runs out is still inside it, which is the reading
	// every other deadline in this codebase takes.
	if nowMs > expires {
		return SessionVerdict{
			State: "expired",
			Detail: "The lease ran out and no renewal arrived over the relay, so the robot ended " +
				"the session itself rather than asking the Hub whether it should (§87.4).",
			ExpiresAt:    expires,
			HasExpiresAt: true,
		}
	}
	return SessionVerdict{
		State:        "live",
		Detail:       "Renewed inside its lease.",
		ExpiresAt:    expires,
		HasExpiresAt: true,
	}
}

// RelayAction is whether this session may perform this action.
//
// The order is the design, and the first rule is the one §39.1 inverts:
//
//  1. **`stop_request` is always allowed** — whatever the role, the route and the session's state,
//     because it never needed the session.
//  2. **A refused or expired session permits nothing else**, `read_status` included.
//  3. **§39.1's role table**, unchanged by the route.
//  4. **Presence, over a relay only.**
//
// Reading 3 before 4 is deliberate: an observer asking for a factory reset is told they are an
// observer, not that they are not standing next to the robot, because the first is the fact that
// would still be true if they walked over to it.
func RelayAction(session canonical.Value, action string, nowMs float64) ActionVerdict {
	verdict := RelaySession(session, nowMs)

	if action == "stop_request" {
		detail := "Stop never needed the session. §39.1 makes it unauthenticated, so this arrives " +
			"as an anonymous stop request — which is what it is."
		if verdict.State == "live" {
			detail = "Stop is never privileged (§39.1). Over a relay it is a *request*, with the " +
				"robot's own acknowledgement beside it, because a relay has no latency bound and " +
				"a control that looks like an estop and is not one is the hazard §87.4 refuses to " +
				"build."
		}
		return ActionVerdict{Action: action, Allowed: true, Detail: detail}
	}

	if verdict.State != "live" {
		refusal := "session_refused"
		if verdict.State == "expired" {
			refusal = "session_expired"
		}
		return ActionVerdict{Action: action, Refusal: refusal, Detail: verdict.Detail}
	}

	role, _ := session.Field("role").Str()
	required, known := requiresRole[action]
	if !known {
		required = "admin"
	}
	if !known || roleRank(role) < roleRank(required) {
		detail := "`" + action + "` is not one of §39.1's actions, so the most this console will " +
			"assume about it is that it needs admin — and this session does not hold it."
		if known {
			detail = "§39.1 requires at least " + required + " for this, and this session holds " +
				role + "."
		}
		return ActionVerdict{Action: action, Refusal: "role_insufficient", Detail: detail}
	}

	route, _ := session.Field("route").Str()
	if route == "relay" && containsString(presenceRequired, action) {
		return ActionVerdict{
			Action:  action,
			Refusal: "presence_required",
			Detail: "§39.2 bootstraps with a chassis button or a printed code, because possession " +
				"of the machine is the only credential that means anything before any other " +
				"exists. This is an action whose recovery needs somebody present, and a relay is " +
				"the case where nobody is (§87.4).",
		}
	}

	bounded := route == "relay" && containsString(widening, action)
	detail := "Allowed by §39.1, and the route does not narrow it."
	if bounded {
		detail = "Allowed, and its effect ends with the session: a grant made over a relay that " +
			"outlived the session would leave the robot holding authority nobody present agreed " +
			"to and nobody remote can still revoke (§87.4)."
	}
	return ActionVerdict{Action: action, Allowed: true, Detail: detail, BoundedBySession: bounded}
}

// relayCase is §87.4's session verdict and the whole permission table beside it.
//
// The table rather than one answer, for §78's reason: a surface that asked per control would be a
// second place §39.1's rules are written down, in whatever order its layout happened to be.
func relayCase(input canonical.Value) (canonical.Value, error) {
	session := input.Field("session")
	nowMs, _ := finiteNumber(input.Field("now_ms"))
	verdict := RelaySession(session, nowMs)

	actions := canonical.Array()
	for _, action := range vocabulary.ConsoleActions {
		entry := RelayAction(session, action, nowMs)
		actions = actions.Push(canonical.Object().
			Set("action", canonical.String(entry.Action)).
			Set("allowed", canonical.Bool(entry.Allowed)).
			Set("refusal", optionalString(entry.Refusal, entry.Refusal != "")).
			Set("bounded_by_session", canonical.Bool(entry.BoundedBySession)).
			Set("detail", canonical.String(entry.Detail)))
	}

	return canonical.Object().
		Set("session", canonical.Object().
			Set("route", session.Field("route")).
			Set("role", session.Field("role")).
			Set("state", canonical.String(verdict.State)).
			Set("refusal", optionalString(verdict.Refusal, verdict.Refusal != "")).
			Set("detail", canonical.String(verdict.Detail)).
			Set("expires_at_ms", optionalNumber(verdict.ExpiresAt, verdict.HasExpiresAt))).
		Set("actions", actions), nil
}
