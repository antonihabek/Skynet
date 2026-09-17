//! The Hub-brokered console relay (§87.4), and what a session reached that way may do.
//!
//! The Rust edition of `reference/typescript/relay.ts`. Read that file for the design; this one
//! records the port.
//!
//! §39.2 is unambiguous about the LAN listener — *"reaching a robot's console from outside its
//! network is not a feature and is not a configuration option"* — and §43 calls that *"right for
//! security and wrong"* for one robot in particular: one in a customer's building with nobody on
//! site. **Nothing here makes anything routable.** The robot dials out, which is §34's bearer
//! machinery rather than a second listener, and this module is about what the session on the far
//! end is permitted to do once it exists.
//!
//! Three decisions, and the third is the substance:
//!
//! * **A relayed operator is an operator.** §5.1's roles are about *authority*; a route is about
//!   *latency*. What the session carries instead is its route, and every surface renders it.
//! * **An estop over a relay is not an estop.** §39.1 makes stop unauthenticated because a delay in
//!   front of a stop button is a hazard, and a relay is an *unbounded* delay in front of the same
//!   control. So the relayed control is `stop_request`, always available, rendered as a request.
//! * **A relayed session may hold a lease, and the deadline is the robot's.** Every other console
//!   session ends when somebody unplugs something. A relay session declares a lease, the robot
//!   enforces it locally, and a renewal that does not arrive ends it — without asking the Hub.
//!
//! **No maximum is invented here.** The figure is the operator profile's, for §5.2's reason. What
//! this module refuses is a relay session with *no* declared deadline.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// How a console session reached the robot.
pub const RELAY_ROUTES: [&str; 2] = ["lan", "relay"];

/// §39.1's roles, and the unauthenticated caller it puts first.
///
/// Ordered, least to most, because the permission table is cumulative and *"at least operator"* is
/// how every row in §39.1 reads.
pub const CONSOLE_ROLES: [&str; 4] = ["anonymous", "observer", "operator", "admin"];

/// What somebody may ask a console to do (§39.1, §87.4), in the order a console draws them.
pub const CONSOLE_ACTIONS: [&str; 13] = [
    "stop_request",
    "read_status",
    "read_health",
    "start_package",
    "stop_package",
    "grant_capability",
    "revoke_capability",
    "seal_blackbox",
    "apply_update",
    "pair",
    "install_driver",
    "change_profile",
    "factory_reset",
];

/// The least role §39.1 requires for each action, in [`CONSOLE_ACTIONS`] order.
const REQUIRES: [(&str, &str); 13] = [
    ("stop_request", "anonymous"),
    ("read_status", "anonymous"),
    ("read_health", "observer"),
    ("start_package", "operator"),
    ("stop_package", "operator"),
    ("grant_capability", "operator"),
    ("revoke_capability", "operator"),
    ("seal_blackbox", "operator"),
    ("apply_update", "operator"),
    ("pair", "admin"),
    ("install_driver", "admin"),
    ("change_profile", "admin"),
    ("factory_reset", "admin"),
];

/// The actions §87.4 refuses over a relay whatever the role.
///
/// §39.2 bootstraps with a chassis button or a printed code *"because possession of the machine is
/// the only credential that means anything before any other credential exists"*. These are the
/// actions whose **recovery** needs somebody present, and a relay is exactly the case where nobody
/// is. `change_profile` is deliberately not here: a profile is §38.2's narrowing-only stack.
const PRESENCE_REQUIRED: [&str; 3] = ["pair", "install_driver", "factory_reset"];

/// The actions that **widen** what the robot may do.
const WIDENING: [&str; 1] = ["grant_capability"];

/// One console session, from either route.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConsoleSession {
    /// `lan` or `relay`.
    pub route: String,
    /// One of §39.1's four.
    pub role: String,
    /// The lease, milliseconds. Required on a relay session and meaningless on a LAN one.
    pub lease_ms: Option<f64>,
    /// When the robot last saw liveness for this session, on the robot's own clock.
    pub renewed_at_ms: Option<f64>,
}

/// Whether a session may be used at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Renewed inside its lease, or on a cable.
    Live,
    /// The lease ran out and no renewal arrived.
    Expired,
    /// It was never usable.
    Refused,
}

impl SessionState {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionState::Live => "live",
            SessionState::Expired => "expired",
            SessionState::Refused => "refused",
        }
    }
}

/// Every session state, in the reference's declaration order.
pub const SESSION_STATES: [SessionState; 3] = [
    SessionState::Live,
    SessionState::Expired,
    SessionState::Refused,
];

/// Why a session holds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRefusalCode {
    /// A relay session with no deadline, or none that has started.
    DeadlineUndeclared,
    /// A route that is neither of the two.
    RouteUnknown,
    /// A role outside §39.1's four.
    RoleUnknown,
}

impl SessionRefusalCode {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            SessionRefusalCode::DeadlineUndeclared => "deadline_undeclared",
            SessionRefusalCode::RouteUnknown => "route_unknown",
            SessionRefusalCode::RoleUnknown => "role_unknown",
        }
    }
}

/// Every session refusal, in the reference's declaration order.
pub const SESSION_REFUSALS: [SessionRefusalCode; 3] = [
    SessionRefusalCode::DeadlineUndeclared,
    SessionRefusalCode::RouteUnknown,
    SessionRefusalCode::RoleUnknown,
];

/// What one session is.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionVerdict {
    /// Live, expired or refused.
    pub state: SessionState,
    /// Why it was refused at open, or `None`.
    pub refusal: Option<SessionRefusalCode>,
    /// The sentence.
    pub detail: String,
    /// When the lease runs out, or `None` where there is no lease to run out.
    pub expires_at_ms: Option<f64>,
}

/// Why one action is refused to a session that exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionRefusalCode {
    /// §39.1's table, unchanged by the route.
    RoleInsufficient,
    /// An action whose recovery needs somebody standing next to the robot.
    PresenceRequired,
    /// The lease ran out.
    SessionExpired,
    /// The session was never usable.
    SessionRefused,
}

impl ActionRefusalCode {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionRefusalCode::RoleInsufficient => "role_insufficient",
            ActionRefusalCode::PresenceRequired => "presence_required",
            ActionRefusalCode::SessionExpired => "session_expired",
            ActionRefusalCode::SessionRefused => "session_refused",
        }
    }
}

/// Every action refusal, in the reference's declaration order.
pub const ACTION_REFUSALS: [ActionRefusalCode; 4] = [
    ActionRefusalCode::RoleInsufficient,
    ActionRefusalCode::PresenceRequired,
    ActionRefusalCode::SessionExpired,
    ActionRefusalCode::SessionRefused,
];

/// What one session may do about one action.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionVerdict {
    /// The action asked about.
    pub action: String,
    /// Whether it is permitted.
    pub allowed: bool,
    /// Why not, or `None`.
    pub refusal: Option<ActionRefusalCode>,
    /// The sentence.
    pub detail: String,
    /// Whether this action's effect ends with the session (§87.4).
    pub bounded_by_session: bool,
}

fn role_rank(role: &str) -> Option<usize> {
    CONSOLE_ROLES.iter().position(|known| *known == role)
}

fn requires(action: &str) -> Option<&'static str> {
    REQUIRES
        .iter()
        .find(|(known, _)| *known == action)
        .map(|(_, role)| *role)
}

/// Whether the session may be used at all, and until when.
///
/// Three states rather than two, and the third is §80's `inert` precedent: a session **refused at
/// open** and one that **expired after working** send a person to do two different things — write a
/// deadline into the profile, or reconnect.
///
/// A LAN session needs no deadline and is never expired here. §39.2 makes the LAN listener
/// link-local and unrouted, so the cable *is* the deadline, and a timer invented here would log an
/// operator out of a console they are standing in front of.
pub fn relay_session(session: &ConsoleSession, now_ms: f64) -> SessionVerdict {
    if !RELAY_ROUTES.contains(&session.route.as_str()) {
        return SessionVerdict {
            state: SessionState::Refused,
            refusal: Some(SessionRefusalCode::RouteUnknown),
            detail: format!(
                "`{}` is not one of lan or relay, so nothing can say whether this session can be \
                 ended by unplugging something (§87.4).",
                session.route
            ),
            expires_at_ms: None,
        };
    }
    if role_rank(&session.role).is_none() {
        return SessionVerdict {
            state: SessionState::Refused,
            refusal: Some(SessionRefusalCode::RoleUnknown),
            detail: format!(
                "`{}` is not one of §39.1's four roles, and a session whose authority nobody can \
                 read holds none.",
                session.role
            ),
            expires_at_ms: None,
        };
    }

    if session.route == "lan" {
        return SessionVerdict {
            state: SessionState::Live,
            refusal: None,
            detail: "A LAN session ends when somebody unplugs something, which is the property \
                     §87.4 is about and the reason it needs no lease."
                .to_string(),
            expires_at_ms: None,
        };
    }

    let Some(lease) = session
        .lease_ms
        .filter(|value| value.is_finite() && *value > 0.0)
    else {
        return SessionVerdict {
            state: SessionState::Refused,
            refusal: Some(SessionRefusalCode::DeadlineUndeclared),
            detail: "A relay session cannot be ended by pulling a cable, so it is ended by a \
                     deadline the robot enforces. One with no deadline is a robot holding \
                     authority for somebody who may already have left. The figure is the operator \
                     profile's — §5.2 forbids a platform inventing one (§87.4)."
                .to_string(),
            expires_at_ms: None,
        };
    };

    let Some(renewed) = session.renewed_at_ms.filter(|value| value.is_finite()) else {
        return SessionVerdict {
            state: SessionState::Refused,
            refusal: Some(SessionRefusalCode::DeadlineUndeclared),
            detail: "A relay session declares a lease and nothing has recorded when it was last \
                     renewed, so the deadline has no start. A lease whose clock never started \
                     never runs out (§87.4)."
                .to_string(),
            expires_at_ms: None,
        };
    };

    let expires = renewed + lease;
    // `>` rather than `>=`: the instant the lease runs out is still inside it, which is the reading
    // every other deadline in this codebase takes.
    if now_ms > expires {
        return SessionVerdict {
            state: SessionState::Expired,
            refusal: None,
            detail: "The lease ran out and no renewal arrived over the relay, so the robot ended \
                     the session itself rather than asking the Hub whether it should (§87.4)."
                .to_string(),
            expires_at_ms: Some(expires),
        };
    }
    SessionVerdict {
        state: SessionState::Live,
        refusal: None,
        detail: "Renewed inside its lease.".to_string(),
        expires_at_ms: Some(expires),
    }
}

/// Whether this session may perform this action.
///
/// The order is the design, and the first rule is the one §39.1 inverts:
///
/// 1. **`stop_request` is always allowed** — whatever the role, the route and the session's state,
///    because it never needed the session.
/// 2. **A refused or expired session permits nothing else**, `read_status` included.
/// 3. **§39.1's role table**, unchanged by the route.
/// 4. **Presence, over a relay only.**
///
/// Reading 3 before 4 is deliberate: an observer asking for a factory reset is told they are an
/// observer, not that they are not standing next to the robot, because the first is the fact that
/// would still be true if they walked over to it.
pub fn relay_action(session: &ConsoleSession, action: &str, now_ms: f64) -> ActionVerdict {
    let verdict = relay_session(session, now_ms);

    if action == "stop_request" {
        let detail = if verdict.state == SessionState::Live {
            "Stop is never privileged (§39.1). Over a relay it is a *request*, with the robot's \
             own acknowledgement beside it, because a relay has no latency bound and a control \
             that looks like an estop and is not one is the hazard §87.4 refuses to build."
        } else {
            "Stop never needed the session. §39.1 makes it unauthenticated, so this arrives as an \
             anonymous stop request — which is what it is."
        };
        return ActionVerdict {
            action: action.to_string(),
            allowed: true,
            refusal: None,
            detail: detail.to_string(),
            bounded_by_session: false,
        };
    }

    if verdict.state != SessionState::Live {
        return ActionVerdict {
            action: action.to_string(),
            allowed: false,
            refusal: Some(if verdict.state == SessionState::Expired {
                ActionRefusalCode::SessionExpired
            } else {
                ActionRefusalCode::SessionRefused
            }),
            detail: verdict.detail,
            bounded_by_session: false,
        };
    }

    let known = requires(action);
    let held = role_rank(&session.role).unwrap_or(0);
    let needed = role_rank(known.unwrap_or("admin")).unwrap_or(0);
    if known.is_none() || held < needed {
        let detail = match known {
            Some(required) => format!(
                "§39.1 requires at least {required} for this, and this session holds {}.",
                session.role
            ),
            None => format!(
                "`{action}` is not one of §39.1's actions, so the most this console will assume \
                 about it is that it needs admin — and this session does not hold it."
            ),
        };
        return ActionVerdict {
            action: action.to_string(),
            allowed: false,
            refusal: Some(ActionRefusalCode::RoleInsufficient),
            detail,
            bounded_by_session: false,
        };
    }

    if session.route == "relay" && PRESENCE_REQUIRED.contains(&action) {
        return ActionVerdict {
            action: action.to_string(),
            allowed: false,
            refusal: Some(ActionRefusalCode::PresenceRequired),
            detail: "§39.2 bootstraps with a chassis button or a printed code, because possession \
                     of the machine is the only credential that means anything before any other \
                     exists. This is an action whose recovery needs somebody present, and a relay \
                     is the case where nobody is (§87.4)."
                .to_string(),
            bounded_by_session: false,
        };
    }

    let bounded = session.route == "relay" && WIDENING.contains(&action);
    let detail = if bounded {
        "Allowed, and its effect ends with the session: a grant made over a relay that outlived \
         the session would leave the robot holding authority nobody present agreed to and nobody \
         remote can still revoke (§87.4)."
    } else {
        "Allowed by §39.1, and the route does not narrow it."
    };
    ActionVerdict {
        action: action.to_string(),
        allowed: true,
        refusal: None,
        detail: detail.to_string(),
        bounded_by_session: bounded,
    }
}

/// Every action's verdict for one session, in [`CONSOLE_ACTIONS`] order.
///
/// The whole table rather than one answer per question, for §78's reason: a surface that asked per
/// control would be a second place the permission rules are written down, in whatever order its
/// layout happened to be.
pub fn relay_permissions(session: &ConsoleSession, now_ms: f64) -> Vec<ActionVerdict> {
    CONSOLE_ACTIONS
        .iter()
        .map(|action| relay_action(session, action, now_ms))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relayed() -> ConsoleSession {
        ConsoleSession {
            route: "relay".to_string(),
            role: "operator".to_string(),
            lease_ms: Some(60_000.0),
            renewed_at_ms: Some(1_000.0),
        }
    }

    /// The property the whole table has: the stop is reachable from every state a session can be
    /// in, including the two where it holds nothing at all.
    #[test]
    fn the_stop_survives_every_refusal() {
        for session in [
            relayed(),
            ConsoleSession {
                lease_ms: None,
                ..relayed()
            },
            ConsoleSession {
                route: "vpn".to_string(),
                ..relayed()
            },
            ConsoleSession {
                role: "superuser".to_string(),
                ..relayed()
            },
        ] {
            let verdict = relay_action(&session, "stop_request", 30_000.0);
            assert!(verdict.allowed, "{:?}", session);
        }
    }

    /// §87.4's third decision, and the one with teeth: a relay session with no deadline is refused
    /// at open rather than treated as opened now.
    #[test]
    fn a_relay_session_with_no_deadline_holds_nothing() {
        let session = ConsoleSession {
            lease_ms: None,
            ..relayed()
        };
        let verdict = relay_session(&session, 30_000.0);
        assert_eq!(verdict.state, SessionState::Refused);
        assert_eq!(
            verdict.refusal,
            Some(SessionRefusalCode::DeadlineUndeclared)
        );
        assert!(!relay_action(&session, "start_package", 30_000.0).allowed);
    }

    /// The instant the lease falls is still inside it.
    #[test]
    fn a_renewal_exactly_on_time_arrived_on_time() {
        let session = ConsoleSession {
            lease_ms: Some(10_000.0),
            renewed_at_ms: Some(1_000.0),
            ..relayed()
        };
        assert_eq!(relay_session(&session, 11_000.0).state, SessionState::Live);
        assert_eq!(
            relay_session(&session, 11_001.0).state,
            SessionState::Expired
        );
    }

    /// The presence narrowing, and the control beside it: the same admin on a cable may do all
    /// three, which is the whole of the argument — the actions are not dangerous, they are
    /// unrecoverable from a browser.
    #[test]
    fn presence_is_not_relayable_and_is_not_about_authority() {
        let admin = ConsoleSession {
            role: "admin".to_string(),
            ..relayed()
        };
        for action in PRESENCE_REQUIRED {
            assert_eq!(
                relay_action(&admin, action, 30_000.0).refusal,
                Some(ActionRefusalCode::PresenceRequired)
            );
        }
        let local = ConsoleSession {
            route: "lan".to_string(),
            role: "admin".to_string(),
            lease_ms: None,
            renewed_at_ms: None,
        };
        for action in PRESENCE_REQUIRED {
            assert!(relay_action(&local, action, 30_000.0).allowed);
        }
    }

    /// A widening over a relay is bounded by the session; the same widening on a cable is not, and
    /// narrowing is bounded by neither.
    #[test]
    fn only_a_widening_over_a_relay_is_bounded() {
        assert!(relay_action(&relayed(), "grant_capability", 30_000.0).bounded_by_session);
        assert!(!relay_action(&relayed(), "revoke_capability", 30_000.0).bounded_by_session);
        let local = ConsoleSession {
            route: "lan".to_string(),
            lease_ms: None,
            renewed_at_ms: None,
            ..relayed()
        };
        assert!(!relay_action(&local, "grant_capability", 30_000.0).bounded_by_session);
    }
}
