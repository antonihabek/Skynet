/**
 * The Hub-brokered console relay (§87.4), and what a session reached that way may do.
 *
 * §39.2 is unambiguous about the LAN listener:
 *
 * > The LAN listener binds to link-local interfaces only and is never routed. Reaching a robot's
 * > console from outside its network is not a feature and is not a configuration option.
 *
 * §43 calls that *"right for security and wrong"* for one robot in particular — one in a customer's
 * building with nobody on site — and the shape it suggests is a relay the robot **dials out** to.
 * **Nothing here makes anything routable.** The listener rule is untouched: the robot opens every
 * connection it has, which is §34's bearer machinery rather than a second listener, and this file is
 * about what the session on the far end is permitted to do once it exists.
 *
 * ── The three decisions, and the third is the substance ────────────────────
 *
 * **A relayed operator is an operator.** §5.1's four roles are about *authority*; a route is about
 * *latency*. A fifth role would have to be granted separately, and the first consequence of that is
 * absurd: an engineer who drives to the site and reaches the same relay from the car park would hold
 * less authority than the same person on the LAN, for a reason about cabling. What a session carries
 * instead is its [`RelayRoute`], and every surface that renders a session renders it.
 *
 * **An estop over a relay is not an estop.** §39.1's argument is that an authentication prompt
 * between a person and a stop button is a hazard, because it is a delay in front of a control
 * somebody is reaching for under pressure. A relay is an *unbounded* delay in front of the same
 * control, and the hazard is worse rather than milder because it looks like the control that works.
 * `safety.estop.latency` is a bench test with a figure in milliseconds and a relay has no bound to
 * measure. So the relayed control is `stop_request` — always available, whatever the role and
 * whatever the session's state — and it is rendered as a request with the robot's own
 * acknowledgement beside it.
 *
 * **A relayed session may hold a lease, and the deadline is the robot's.** Every other console
 * session ends when somebody unplugs something, and that is precisely the property a relay removes.
 * So the session declares a lease, the robot enforces it locally, liveness over the relay renews it,
 * and a renewal that does not arrive ends it — without asking the Hub whether it should. That is
 * §46's watchdog argument one layer up, and the two rejected alternatives are the two ways it is
 * usually built: an unbounded session, which is a robot holding authority for somebody who left; and
 * a Hub-held deadline, which makes a robot in a customer's building depend on a round trip to find
 * out whether anybody is still driving it.
 *
 * **No maximum is invented here.** The lease figure is the operator profile's, for §5.2's reason: a
 * platform number would be a limit nobody measured. What this file refuses is a relay session with
 * *no* declared deadline, which is what gives the decision teeth rather than a paragraph.
 */

/** How a console session reached the robot. */
export const RELAY_ROUTES = ['lan', 'relay'] as const
export type RelayRoute = (typeof RELAY_ROUTES)[number]

/**
 * §39.1's roles, and the unauthenticated caller it puts first.
 *
 * Ordered, least to most, because the permission table below is cumulative and *"at least
 * operator"* is how every row in §39.1 reads.
 */
export const CONSOLE_ROLES = ['anonymous', 'observer', 'operator', 'admin'] as const
export type ConsoleRole = (typeof CONSOLE_ROLES)[number]

/**
 * What somebody may ask a console to do.
 *
 * §39.1's table as a vocabulary, with one addition and one substitution. The addition is
 * `stop_request`, which §87.4 separates from *stop*; the substitution is that the table's *"stop"*
 * row becomes it, because on this platform there is no other kind of stop a console offers.
 */
export const CONSOLE_ACTIONS = [
  'stop_request',
  'read_status',
  'read_health',
  'start_package',
  'stop_package',
  'grant_capability',
  'revoke_capability',
  'seal_blackbox',
  'apply_update',
  'pair',
  'install_driver',
  'change_profile',
  'factory_reset',
] as const
export type ConsoleAction = (typeof CONSOLE_ACTIONS)[number]

/**
 * The least role §39.1 requires for each action.
 *
 * `stop_request` and `read_status` are `anonymous`, which is §39.1's inversion and the one rule in
 * that section with an injury behind it: *"an authentication prompt between a person and a stop
 * button is a hazard."*
 */
const REQUIRES: Record<ConsoleAction, ConsoleRole> = {
  stop_request: 'anonymous',
  read_status: 'anonymous',
  read_health: 'observer',
  start_package: 'operator',
  stop_package: 'operator',
  grant_capability: 'operator',
  revoke_capability: 'operator',
  seal_blackbox: 'operator',
  apply_update: 'operator',
  pair: 'admin',
  install_driver: 'admin',
  change_profile: 'admin',
  factory_reset: 'admin',
}

/**
 * The actions §87.4 refuses over a relay whatever the role.
 *
 * §39.2 bootstraps with a chassis button or a printed code *"because possession of the machine is
 * the only credential that means anything before any other credential exists"*. These are the
 * actions whose **recovery** needs somebody present, and a relay is exactly the case where nobody
 * is: an unpaired robot, a half-installed driver and a factory-reset robot are three machines that
 * cannot be fixed from a browser however much authority the browser holds.
 *
 * `change_profile` is deliberately **not** here. A profile is §38.2's narrowing-only stack, so the
 * worst a relayed admin can do with one is confine the robot further, and §39.1's rule is that the
 * safe direction never waits.
 */
const PRESENCE_REQUIRED: readonly ConsoleAction[] = ['pair', 'install_driver', 'factory_reset']

/**
 * The actions that **widen** what the robot may do.
 *
 * A widening made over a relay is bounded by the session that made it (§87.4): a grant that outlived
 * the session would leave a robot holding authority nobody present agreed to and nobody remote can
 * still revoke. Granting is §54.2's one gated widening, which is what makes this list one entry
 * long; a second entry would want the same treatment and the same sentence.
 */
const WIDENING: readonly ConsoleAction[] = ['grant_capability']

/** One console session, from either route. */
export interface ConsoleSession {
  route: string
  role: string
  /**
   * The lease, milliseconds. Required on a relay session and meaningless on a LAN one, where
   * §87.4's whole argument is that the session ends when somebody unplugs something.
   */
  lease_ms?: number | null
  /** When the robot last saw liveness for this session, on the robot's own clock. */
  renewed_at_ms?: number | null
}

export const SESSION_STATES = ['live', 'expired', 'refused'] as const
export type SessionState = (typeof SESSION_STATES)[number]

export const SESSION_REFUSALS = ['deadline_undeclared', 'route_unknown', 'role_unknown'] as const
export type SessionRefusalCode = (typeof SESSION_REFUSALS)[number]

export interface SessionVerdict {
  state: SessionState
  /** Why it was refused at open, or null. */
  refusal: SessionRefusalCode | null
  detail: string
  /** When the lease runs out on the robot's clock, or null where there is no lease to run out. */
  expires_at_ms: number | null
}

export const ACTION_DECISIONS = ['allowed', 'refused'] as const

export const ACTION_REFUSALS = [
  'role_insufficient',
  'presence_required',
  'session_expired',
  'session_refused',
] as const
export type ActionRefusalCode = (typeof ACTION_REFUSALS)[number]

export interface ActionVerdict {
  action: string
  allowed: boolean
  refusal: ActionRefusalCode | null
  detail: string
  /**
   * Whether this action's effect ends with the session (§87.4).
   *
   * True only for a widening over a relay. It is on the verdict rather than inferred by a caller
   * because the caller that has to honour it is the one recording the grant, and a grant recorded
   * without its bound is a grant that does not have one.
   */
  bounded_by_session: boolean
}

function roleRank(role: string): number {
  return (CONSOLE_ROLES as readonly string[]).indexOf(role)
}

/**
 * Whether the session may be used at all, and until when.
 *
 * Three states rather than two, and the third is §80's `inert` precedent: a session **refused at
 * open** and one that **expired after working** send a person to do two different things — write a
 * deadline into the profile, or reconnect — and folding them together would produce the single
 * unhelpful sentence this platform has a documented history of.
 *
 * A LAN session needs no deadline and is never expired by this function. That is not leniency: §39.2
 * makes the LAN listener link-local and unrouted, so the cable *is* the deadline, and a timer
 * invented here would log an operator out of a console they are standing in front of.
 */
export function relaySession(session: ConsoleSession, nowMs: number): SessionVerdict {
  if (!(RELAY_ROUTES as readonly string[]).includes(session.route)) {
    return {
      state: 'refused',
      refusal: 'route_unknown',
      detail: `\`${String(session.route)}\` is not one of lan or relay, so nothing can say whether `
        + 'this session can be ended by unplugging something (§87.4).',
      expires_at_ms: null,
    }
  }
  if (roleRank(session.role) < 0) {
    return {
      state: 'refused',
      refusal: 'role_unknown',
      detail: `\`${String(session.role)}\` is not one of §39.1's four roles, and a session whose `
        + 'authority nobody can read holds none.',
      expires_at_ms: null,
    }
  }

  if (session.route === 'lan') {
    return {
      state: 'live',
      refusal: null,
      detail: 'A LAN session ends when somebody unplugs something, which is the property §87.4 is '
        + 'about and the reason it needs no lease.',
      expires_at_ms: null,
    }
  }

  const lease = session.lease_ms
  if (lease === null || lease === undefined || !Number.isFinite(lease) || lease <= 0) {
    return {
      state: 'refused',
      refusal: 'deadline_undeclared',
      detail: 'A relay session cannot be ended by pulling a cable, so it is ended by a deadline the '
        + 'robot enforces. One with no deadline is a robot holding authority for somebody who may '
        + 'already have left. The figure is the operator profile\'s — §5.2 forbids a platform '
        + 'inventing one (§87.4).',
      expires_at_ms: null,
    }
  }

  const renewed = session.renewed_at_ms
  if (renewed === null || renewed === undefined || !Number.isFinite(renewed)) {
    return {
      state: 'refused',
      refusal: 'deadline_undeclared',
      detail: 'A relay session declares a lease and nothing has recorded when it was last renewed, '
        + 'so the deadline has no start. A lease whose clock never started never runs out (§87.4).',
      expires_at_ms: null,
    }
  }

  const expires = renewed + lease
  // `>` rather than `>=`: the instant the lease runs out is still inside it, which is the reading
  // every other deadline in this codebase takes and the one a renewal arriving exactly on time
  // deserves.
  if (nowMs > expires) {
    return {
      state: 'expired',
      refusal: null,
      detail: 'The lease ran out and no renewal arrived over the relay, so the robot ended the '
        + 'session itself rather than asking the Hub whether it should (§87.4).',
      expires_at_ms: expires,
    }
  }
  return {
    state: 'live',
    refusal: null,
    detail: 'Renewed inside its lease.',
    expires_at_ms: expires,
  }
}

/**
 * Whether this session may perform this action.
 *
 * The order is the design, and the first rule is the one §39.1 inverts:
 *
 * 1. **`stop_request` is always allowed.** Whatever the role, whatever the route, and whatever the
 *    session's state — because it never needed the session. §39.1 makes stop unauthenticated, so
 *    what reaches the robot after a session ends reaches it as an anonymous stop request, which is
 *    what it is.
 * 2. **A refused or expired session permits nothing else**, `observe` included. A session that
 *    cannot be renewed is a session nobody is holding.
 * 3. **§39.1's role table**, unchanged by the route.
 * 4. **Presence, over a relay only.** Pairing, installing a driver and a factory reset are refused
 *    whatever the role.
 *
 * Reading order 3 before 4 is deliberate: an observer asking for a factory reset is told they are an
 * observer, not that they are not standing next to the robot, because the first is the fact that
 * would still be true if they walked over to it.
 */
export function relayAction(
  session: ConsoleSession,
  action: string,
  nowMs: number,
): ActionVerdict {
  const verdict = relaySession(session, nowMs)
  const known = (CONSOLE_ACTIONS as readonly string[]).includes(action)

  if (action === 'stop_request') {
    return {
      action,
      allowed: true,
      refusal: null,
      detail: verdict.state === 'live'
        ? 'Stop is never privileged (§39.1). Over a relay it is a *request*, with the robot\'s own '
          + 'acknowledgement beside it, because a relay has no latency bound and a control that '
          + 'looks like an estop and is not one is the hazard §87.4 refuses to build.'
        : 'Stop never needed the session. §39.1 makes it unauthenticated, so this arrives as an '
          + 'anonymous stop request — which is what it is.',
      bounded_by_session: false,
    }
  }

  if (verdict.state !== 'live') {
    return {
      action,
      allowed: false,
      refusal: verdict.state === 'expired' ? 'session_expired' : 'session_refused',
      detail: verdict.detail,
      bounded_by_session: false,
    }
  }

  if (!known || roleRank(session.role) < roleRank(REQUIRES[action as ConsoleAction])) {
    const required = known ? REQUIRES[action as ConsoleAction] : 'admin'
    return {
      action,
      allowed: false,
      refusal: 'role_insufficient',
      detail: known
        ? `§39.1 requires at least ${required} for this, and this session holds ${session.role}.`
        : `\`${String(action)}\` is not one of §39.1's actions, so the most this console will `
          + 'assume about it is that it needs admin — and this session does not hold it.',
      bounded_by_session: false,
    }
  }

  if (session.route === 'relay' && PRESENCE_REQUIRED.includes(action as ConsoleAction)) {
    return {
      action,
      allowed: false,
      refusal: 'presence_required',
      detail: '§39.2 bootstraps with a chassis button or a printed code, because possession of the '
        + 'machine is the only credential that means anything before any other exists. This is an '
        + 'action whose recovery needs somebody present, and a relay is the case where nobody is '
        + '(§87.4).',
      bounded_by_session: false,
    }
  }

  const bounded = session.route === 'relay' && WIDENING.includes(action as ConsoleAction)
  return {
    action,
    allowed: true,
    refusal: null,
    detail: bounded
      ? 'Allowed, and its effect ends with the session: a grant made over a relay that outlived the '
        + 'session would leave the robot holding authority nobody present agreed to and nobody '
        + 'remote can still revoke (§87.4).'
      : 'Allowed by §39.1, and the route does not narrow it.',
    bounded_by_session: bounded,
  }
}

/**
 * Every action's verdict for one session, in [`CONSOLE_ACTIONS`] order.
 *
 * What a console renders when it decides which controls to draw, and what `skynet` prints. The
 * whole table rather than one answer per question, for §78's reason: a surface that asked per
 * control would be a second place the permission rules are written down, in whatever order its
 * layout happened to be.
 */
export function relayPermissions(session: ConsoleSession, nowMs: number): ActionVerdict[] {
  return CONSOLE_ACTIONS.map(action => relayAction(session, action, nowMs))
}
