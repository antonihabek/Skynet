"""The Hub-brokered console relay (§87.4) — the Python edition of ``reference/typescript/relay.ts``.

§39.2 is unambiguous about the LAN listener — *"reaching a robot's console from outside its network
is not a feature and is not a configuration option"* — and §43 calls that *"right for security and
wrong"* for one robot in particular: one in a customer's building with nobody on site. **Nothing
here makes anything routable.** The robot dials out, which is §34's bearer machinery rather than a
second listener, and this module is about what the session on the far end may do once it exists.

Three decisions, and the third is the substance:

* **A relayed operator is an operator.** §5.1's roles are about *authority*; a route is about
  *latency*. A fifth role would mean an engineer who drove to the site and reached the same relay
  from the car park held less authority than the same person on the LAN, for a reason about cabling.
* **An estop over a relay is not an estop.** §39.1 makes stop unauthenticated because a delay in
  front of a stop button is a hazard, and a relay is an *unbounded* delay in front of the same
  control — worse rather than milder, because it looks like the control that works.
* **A relayed session may hold a lease, and the deadline is the robot's.** Every other console
  session ends when somebody unplugs something. A relay session declares a lease, the robot enforces
  it locally, and a renewal that does not arrive ends it — without asking the Hub.

**No maximum is invented here.** The figure is the operator profile's, for §5.2's reason. What this
module refuses is a relay session with *no* declared deadline.

── Where Python is quietly not JavaScript, here ─────────────────────────────

``Number.isFinite``
    Rejects ``NaN`` and the infinities, which ``isinstance(x, float)`` does not, and rejects
    booleans, which ``isinstance(True, int)`` lets through. A lease of ``true`` is not a lease of
    1 ms, and the arithmetic against ``NaN`` is the specific way this check fails if it is written
    as a comparison rather than as a declaration test.
"""

from __future__ import annotations

import math
from typing import Any, Final

__all__ = [
    "ACTION_REFUSALS",
    "CONSOLE_ACTIONS",
    "CONSOLE_ROLES",
    "RELAY_ROUTES",
    "SESSION_REFUSALS",
    "SESSION_STATES",
    "relay_action",
    "relay_permissions",
    "relay_session",
    "relay_to_json",
]

#: How a console session reached the robot.
RELAY_ROUTES: Final = ("lan", "relay")

#: §39.1's roles, ordered least to most, with the unauthenticated caller §39.1 puts first.
CONSOLE_ROLES: Final = ("anonymous", "observer", "operator", "admin")

#: What somebody may ask a console to do (§39.1, §87.4), in the order a console draws them.
CONSOLE_ACTIONS: Final = (
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
)

#: The least role §39.1 requires for each action.
_REQUIRES: Final = {
    "stop_request": "anonymous",
    "read_status": "anonymous",
    "read_health": "observer",
    "start_package": "operator",
    "stop_package": "operator",
    "grant_capability": "operator",
    "revoke_capability": "operator",
    "seal_blackbox": "operator",
    "apply_update": "operator",
    "pair": "admin",
    "install_driver": "admin",
    "change_profile": "admin",
    "factory_reset": "admin",
}

#: The actions §87.4 refuses over a relay whatever the role.
#:
#: §39.2 bootstraps with a chassis button or a printed code *"because possession of the machine is
#: the only credential that means anything before any other credential exists"*. These are the
#: actions whose **recovery** needs somebody present. ``change_profile`` is deliberately not here: a
#: profile is §38.2's narrowing-only stack.
_PRESENCE_REQUIRED: Final = ("pair", "install_driver", "factory_reset")

#: The actions that **widen** what the robot may do.
_WIDENING: Final = ("grant_capability",)

SESSION_STATES: Final = ("live", "expired", "refused")

SESSION_REFUSALS: Final = ("deadline_undeclared", "route_unknown", "role_unknown")

ACTION_REFUSALS: Final = (
    "role_insufficient",
    "presence_required",
    "session_expired",
    "session_refused",
)


def _is_finite(value: Any) -> bool:
    """``Number.isFinite``. A ``bool`` is not a number, whatever ``isinstance`` says."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return False
    return not (math.isnan(float(value)) or math.isinf(float(value)))


def _role_rank(role: Any) -> int:
    return CONSOLE_ROLES.index(role) if isinstance(role, str) and role in CONSOLE_ROLES else -1


def relay_session(session: dict[str, Any], now_ms: float) -> dict[str, Any]:
    """Whether the session may be used at all, and until when.

    Three states rather than two, and the third is §80's ``inert`` precedent: a session **refused at
    open** and one that **expired after working** send a person to do two different things — write a
    deadline into the profile, or reconnect.

    A LAN session needs no deadline and is never expired here. §39.2 makes the LAN listener
    link-local and unrouted, so the cable *is* the deadline, and a timer invented here would log an
    operator out of a console they are standing in front of.
    """
    route = session.get("route")
    if not isinstance(route, str) or route not in RELAY_ROUTES:
        return {
            "state": "refused",
            "refusal": "route_unknown",
            "detail": f"`{'' if route is None else route}` is not one of lan or relay, so nothing "
            "can say whether this session can be ended by unplugging something (§87.4).",
            "expires_at_ms": None,
        }
    role = session.get("role")
    if _role_rank(role) < 0:
        return {
            "state": "refused",
            "refusal": "role_unknown",
            "detail": f"`{'' if role is None else role}` is not one of §39.1's four roles, and a "
            "session whose authority nobody can read holds none.",
            "expires_at_ms": None,
        }

    if route == "lan":
        return {
            "state": "live",
            "refusal": None,
            "detail": "A LAN session ends when somebody unplugs something, which is the property "
            "§87.4 is about and the reason it needs no lease.",
            "expires_at_ms": None,
        }

    lease = session.get("lease_ms")
    if not _is_finite(lease) or lease <= 0:
        return {
            "state": "refused",
            "refusal": "deadline_undeclared",
            "detail": "A relay session cannot be ended by pulling a cable, so it is ended by a "
            "deadline the robot enforces. One with no deadline is a robot holding authority for "
            "somebody who may already have left. The figure is the operator profile's — §5.2 "
            "forbids a platform inventing one (§87.4).",
            "expires_at_ms": None,
        }

    renewed = session.get("renewed_at_ms")
    if not _is_finite(renewed):
        return {
            "state": "refused",
            "refusal": "deadline_undeclared",
            "detail": "A relay session declares a lease and nothing has recorded when it was last "
            "renewed, so the deadline has no start. A lease whose clock never started never runs "
            "out (§87.4).",
            "expires_at_ms": None,
        }

    expires = renewed + lease
    # `>` rather than `>=`: the instant the lease runs out is still inside it, which is the reading
    # every other deadline in this codebase takes.
    if now_ms > expires:
        return {
            "state": "expired",
            "refusal": None,
            "detail": "The lease ran out and no renewal arrived over the relay, so the robot ended "
            "the session itself rather than asking the Hub whether it should (§87.4).",
            "expires_at_ms": expires,
        }
    return {
        "state": "live",
        "refusal": None,
        "detail": "Renewed inside its lease.",
        "expires_at_ms": expires,
    }


def relay_action(session: dict[str, Any], action: str, now_ms: float) -> dict[str, Any]:
    """Whether this session may perform this action.

    The order is the design, and the first rule is the one §39.1 inverts:

    1. **``stop_request`` is always allowed** — whatever the role, the route and the session's
       state, because it never needed the session.
    2. **A refused or expired session permits nothing else**, ``read_status`` included.
    3. **§39.1's role table**, unchanged by the route.
    4. **Presence, over a relay only.**

    Reading 3 before 4 is deliberate: an observer asking for a factory reset is told they are an
    observer, not that they are not standing next to the robot, because the first is the fact that
    would still be true if they walked over to it.
    """
    verdict = relay_session(session, now_ms)

    if action == "stop_request":
        detail = (
            "Stop is never privileged (§39.1). Over a relay it is a *request*, with the robot's "
            "own acknowledgement beside it, because a relay has no latency bound and a control "
            "that looks like an estop and is not one is the hazard §87.4 refuses to build."
            if verdict["state"] == "live"
            else "Stop never needed the session. §39.1 makes it unauthenticated, so this arrives "
            "as an anonymous stop request — which is what it is."
        )
        return {
            "action": action,
            "allowed": True,
            "refusal": None,
            "detail": detail,
            "bounded_by_session": False,
        }

    if verdict["state"] != "live":
        return {
            "action": action,
            "allowed": False,
            "refusal": "session_expired" if verdict["state"] == "expired" else "session_refused",
            "detail": verdict["detail"],
            "bounded_by_session": False,
        }

    known = action in _REQUIRES
    required = _REQUIRES.get(action, "admin")
    if not known or _role_rank(session.get("role")) < _role_rank(required):
        detail = (
            f"§39.1 requires at least {required} for this, and this session holds "
            f"{session.get('role')}."
            if known
            else f"`{action}` is not one of §39.1's actions, so the most this console will assume "
            "about it is that it needs admin — and this session does not hold it."
        )
        return {
            "action": action,
            "allowed": False,
            "refusal": "role_insufficient",
            "detail": detail,
            "bounded_by_session": False,
        }

    if session.get("route") == "relay" and action in _PRESENCE_REQUIRED:
        return {
            "action": action,
            "allowed": False,
            "refusal": "presence_required",
            "detail": "§39.2 bootstraps with a chassis button or a printed code, because "
            "possession of the machine is the only credential that means anything before any other "
            "exists. This is an action whose recovery needs somebody present, and a relay is the "
            "case where nobody is (§87.4).",
            "bounded_by_session": False,
        }

    bounded = session.get("route") == "relay" and action in _WIDENING
    return {
        "action": action,
        "allowed": True,
        "refusal": None,
        "detail": "Allowed, and its effect ends with the session: a grant made over a relay that "
        "outlived the session would leave the robot holding authority nobody present agreed to and "
        "nobody remote can still revoke (§87.4)."
        if bounded
        else "Allowed by §39.1, and the route does not narrow it.",
        "bounded_by_session": bounded,
    }


def relay_permissions(session: dict[str, Any], now_ms: float) -> list[dict[str, Any]]:
    """Every action's verdict for one session, in :data:`CONSOLE_ACTIONS` order."""
    return [relay_action(session, action, now_ms) for action in CONSOLE_ACTIONS]


# ── The wire shape ───────────────────────────────────────────────────────────


def relay_to_json(case_input: dict[str, Any]) -> Any:
    """``relay/console`` — the session's own verdict, and the whole permission table beside it.

    The table rather than one answer, for §78's reason: a surface that asked per control would be a
    second place §39.1's rules are written down, in whatever order its layout happened to be.
    """
    session = case_input.get("session") or {}
    now_ms = case_input.get("now_ms")
    now_ms = now_ms if _is_finite(now_ms) else 0
    verdict = relay_session(session, now_ms)
    return {
        "session": {
            "route": session.get("route"),
            "role": session.get("role"),
            "state": verdict["state"],
            "refusal": verdict["refusal"],
            "detail": verdict["detail"],
            "expires_at_ms": verdict["expires_at_ms"],
        },
        "actions": [
            {
                "action": action["action"],
                "allowed": action["allowed"],
                "refusal": action["refusal"],
                "bounded_by_session": action["bounded_by_session"],
                "detail": action["detail"],
            }
            for action in relay_permissions(session, now_ms)
        ],
    }
