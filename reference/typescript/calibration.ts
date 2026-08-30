/**
 * Calibration validity (§45).
 *
 * Calibration is the only state in this system that is **safety-relevant, individual to one
 * machine, and drifts over time**. The descriptor is signed and static; profiles are signed and
 * operator-set. Calibration is measured, it decays, and several guards are built on it.
 *
 * The failure this module exists to prevent is stated in §45.1 and it is not a dramatic one: *"a
 * camera extrinsic off by four centimetres means the `proximity` guard measures separation to the
 * wrong place while reporting perfectly healthy."* Nothing trips. Nothing looks wrong. The number
 * is simply about the world as it was last spring.
 *
 * ── What this module is, and what it is not ─────────────────────────────────
 *
 * It answers **is this record still valid, and if not which guards lose their input**. It does not
 * compute a calibration, does not hold the values, and has no opinion about hand-eye routines —
 * §45.6 puts those in packages, so a vendor can ship a better one without an OS release.
 *
 * Pure: the clock and the conditions are supplied, which is what lets the Hub answer "is this
 * fleet's calibration current" from a check-in and the robot answer the same question offline, by
 * the same function.
 *
 * ── Expiry degrades; it never silently continues ────────────────────────────
 *
 * §45.5, which is §7.2's rule applied to a measurement rather than to a sensor: an expired record
 * makes the guards that depend on it report **unavailable, naming the expired parameter set**. Not
 * "assume it is probably still fine", which is what every system without an expiry policy does by
 * omission — not by decision, which is the point.
 *
 * ── A residual is a threshold, not a decoration ─────────────────────────────
 *
 * §45.3: *"a hand-eye calibration with twelve millimetres of reprojection error is a failed
 * calibration, and accepting it silently produces a workspace guard that is precisely twelve
 * millimetres wrong forever."* A record whose residual exceeds its own declared threshold is
 * `failed` the moment it is read, at any age. It never becomes valid and it never expires — it was
 * never a calibration.
 *
 * ── Absence is not validity ─────────────────────────────────────────────────
 *
 * A parameter set with no record at all is `absent`, and its guards are unavailable for that
 * reason. The alternative — treating "never calibrated" as "no evidence of a problem" — is what
 * puts a `proximity` guard on a robot whose camera transform is whatever the CAD model said.
 */

/**
 * The parameter sets §45.2 names, in the order that table gives them.
 *
 * A closed vocabulary rather than a free string, for the reason §26's event codes are closed: an
 * open one is how `camera_extrinsics` and `camera-extrinsic` end up naming the same measurement on
 * two robots in one fleet, and the guard mapping below then answers differently for each.
 */
export const PARAMETER_SETS = [
  'joint_zeros', 'camera_intrinsics', 'camera_extrinsics', 'force_torque_bias',
  'imu_bias', 'tool_offset', 'gravity_vector',
] as const

export type ParameterSet = (typeof PARAMETER_SETS)[number]

/**
 * §45.2's table, as data: which guards lose their input when a set expires.
 *
 * **This table is the deliverable of §45.2.** It converts "we should calibrate the cameras
 * sometime" into "an expired extrinsic disables the proximity guard", which is a sentence an
 * operator can act on and a fleet console can count.
 *
 * `camera_intrinsics` feeds `proximity` *indirectly*, through projection, and is listed rather than
 * omitted: a guard whose input is two transforms away is still measuring the wrong thing when
 * either is wrong.
 */
const GUARDS_FED_BY: Record<string, readonly string[]> = {
  joint_zeros: ['workspace', 'self_collision'],
  camera_intrinsics: ['proximity'],
  camera_extrinsics: ['proximity', 'workspace'],
  force_torque_bias: ['envelope'],
  imu_bias: ['balance'],
  tool_offset: ['workspace'],
  gravity_vector: ['balance', 'envelope'],
}

export function guardsFedBy(set: string): readonly string[] {
  return GUARDS_FED_BY[set] ?? []
}

/**
 * The events §45.4 says invalidate a calibration outright.
 *
 * > "A robot that has just hit something has, by definition, possibly moved its cameras, and
 * > continuing to trust yesterday's extrinsics is exactly the wrong response to an impact."
 */
export const INVALIDATING_EVENTS = [
  'collision', 'estop_under_load', 'tool_change', 'driver_replaced', 'mainboard_swap',
] as const

/** The expiry reasons, as a closed vocabulary a console can translate. */
export const EXPIRY_REASONS = ['age', 'joint_travel', 'temperature', 'event'] as const
export type ExpiryReason = (typeof EXPIRY_REASONS)[number]

/**
 * When a record stops being believed (§45.4).
 *
 * Every field is optional, and an absent field is **not a policy of "never"** — it is a condition
 * this record is not checked against.
 */
export interface ValidityPolicy {
  max_age_s?: number | null
  max_joint_travel_rad?: number | null
  max_temperature_delta_c?: number | null
  invalidating_events?: string[]
}

/**
 * The conditions a record was taken under (§45.3), and the ones it is later judged against.
 *
 * The same type on both sides on purpose: an expiry rule comparing a *taken* temperature against a
 * *current* one needs both to mean the same thing, and two interfaces would be two places for the
 * units to diverge.
 */
export interface Conditions {
  at_s?: number | null
  temperature_c?: number | null
  joint_travel_rad?: number | null
  tool?: string | null
  payload_kg?: number | null
}

/**
 * One stored calibration (§45.3).
 *
 * The **values are not here**. This module decides validity; a record's numbers are read by
 * whatever consumes them. What is here is everything §45.3 says must accompany them, because "a
 * calibration whose method nobody recorded is a rumour with six decimal places".
 */
export interface CalibrationRecord {
  set: string
  method?: string | null
  residual?: number | null
  residual_threshold?: number | null
  conditions?: Conditions
  operator?: string | null
  policy?: ValidityPolicy
  events_since?: string[]
}

export type Validity =
  | { state: 'valid', checked: string[] }
  | { state: 'failed', detail: string }
  | { state: 'expired', reason: ExpiryReason, detail: string }
  | { state: 'absent' }
  | { state: 'unusable', detail: string }

export function validityDetail(validity: Validity): string {
  switch (validity.state) {
    case 'valid': return ''
    // Phrased to compose: this is read on its own in a table row that already names the set, and
    // inside "the `set` calibration is absent — …" in the guard list.
    case 'absent':
      return 'nobody has ever measured it on this robot'
    default: return validity.detail
  }
}

function present(value: number | null | undefined): value is number {
  return value !== null && value !== undefined && Number.isFinite(value)
}

/**
 * Judge one record against the present (§45.4, §45.5).
 *
 * ── Order, and why it is this order ─────────────────────────────────────────
 *
 * 1. **Provenance**, because §45.3 makes it mandatory and a record without a method cannot be
 *    judged on anything else — "valid for another 30 days" would be a claim about a measurement
 *    nobody described.
 * 2. **Residual**, because a failed calibration was never valid and its age is irrelevant.
 * 3. **Events**, because §45.4 calls them the triggers that matter and they are a step rather than
 *    a slope: a robot that hit something has a bad extrinsic *now*, whatever its age says.
 * 4. Then the three gradual conditions, in the order §45.4's table gives them.
 *
 * A missing *current* reading does not extend validity. If the policy names a temperature delta and
 * the robot cannot say what temperature it is, the record expires — the alternative is a rule that
 * silently stops applying on exactly the robot whose sensor has failed.
 */
export function evaluate(record: CalibrationRecord, now: Conditions): Validity {
  const policy = record.policy ?? {}
  const taken = record.conditions ?? {}

  const missing: string[] = []
  if (!record.method) missing.push('method')
  if (!present(record.residual)) missing.push('residual')
  if (missing.length > 0) {
    return {
      state: 'unusable',
      detail: '§45.3 makes provenance mandatory and this record has no '
        + `${missing.join(' and no ')}. A calibration whose method nobody recorded is a rumour `
        + 'with six decimal places',
    }
  }

  if (present(record.residual) && present(record.residual_threshold)
    && record.residual > record.residual_threshold) {
    return {
      state: 'failed',
      detail: `the method reported a residual of ${record.residual} against its own threshold of `
        + `${record.residual_threshold}, so this was a failed calibration rather than one that has `
        + 'since expired',
    }
  }

  for (const event of record.events_since ?? []) {
    if ((policy.invalidating_events ?? []).includes(event)) {
      return {
        state: 'expired',
        reason: 'event',
        detail: `a \`${event}\` was recorded against this set since it was measured, and §45.4 `
          + 'invalidates it outright: continuing to trust the previous values is exactly the wrong '
          + 'response to one',
      }
    }
  }

  const checked: string[] = []

  if (present(policy.max_age_s)) {
    if (present(taken.at_s) && present(now.at_s)) {
      const age = now.at_s - taken.at_s
      if (age > policy.max_age_s) {
        return {
          state: 'expired',
          reason: 'age',
          detail: `measured ${age.toFixed(0)} s ago against a validity of `
            + `${policy.max_age_s.toFixed(0)} s`,
        }
      }
      checked.push(`age ${age.toFixed(0)}s of ${policy.max_age_s.toFixed(0)}s`)
    } else {
      return {
        state: 'expired',
        reason: 'age',
        detail: 'this record expires with time and either it or this robot cannot say when it was '
          + 'taken, so its age cannot be established',
      }
    }
  }

  if (present(policy.max_joint_travel_rad)) {
    if (present(taken.joint_travel_rad) && present(now.joint_travel_rad)) {
      const travelled = now.joint_travel_rad - taken.joint_travel_rad
      if (travelled > policy.max_joint_travel_rad) {
        return {
          state: 'expired',
          reason: 'joint_travel',
          detail: `${travelled.toFixed(1)} rad of joint travel since it was measured, against a `
            + `validity of ${policy.max_joint_travel_rad.toFixed(1)} rad`,
        }
      }
      checked.push(
        `travel ${travelled.toFixed(1)} of ${policy.max_joint_travel_rad.toFixed(1)} rad`)
    } else {
      return {
        state: 'expired',
        reason: 'joint_travel',
        detail: 'this record expires with joint travel and no travel counter is available, so how '
          + 'far this robot has moved since cannot be established',
      }
    }
  }

  if (present(policy.max_temperature_delta_c)) {
    if (present(taken.temperature_c) && present(now.temperature_c)) {
      const delta = Math.abs(now.temperature_c - taken.temperature_c)
      if (delta > policy.max_temperature_delta_c) {
        return {
          state: 'expired',
          reason: 'temperature',
          detail: `${delta.toFixed(1)} °C from the ${taken.temperature_c.toFixed(1)} °C it was `
            + `measured at, against a validity of ${policy.max_temperature_delta_c.toFixed(1)} °C`,
        }
      }
      checked.push(
        `temperature Δ${delta.toFixed(1)} of ${policy.max_temperature_delta_c.toFixed(1)} °C`)
    } else {
      return {
        state: 'expired',
        reason: 'temperature',
        detail: 'this record expires with temperature and no reading is available, so how far this '
          + 'robot has drifted from its calibration conditions cannot be established',
      }
    }
  }

  return { state: 'valid', checked }
}

export interface GuardUnavailable {
  guard: string
  set: string
  state: string
  detail: string
}

/**
 * Which guards lose their input, given what has been measured (§45.5).
 *
 * One entry per (guard, set) pair, deliberately: `workspace` disabled by expired joint zeros *and*
 * an expired tool offset is two measurements somebody has to take, and collapsing them to one line
 * would hide the second until the first was done.
 *
 * `applicable` names the sets that mean anything on this body — a fixed-base arm with no IMU should
 * not report `balance` unavailable for want of an IMU bias it can never have.
 */
export function guardsUnavailable(
  records: Array<[string, Validity]>,
  applicable: readonly string[],
): GuardUnavailable[] {
  const out: GuardUnavailable[] = []
  for (const set of applicable) {
    const held = records.find(([name]) => name === set)
    const validity: Validity = held ? held[1] : { state: 'absent' }
    if (validity.state === 'valid') continue
    for (const guard of guardsFedBy(set)) {
      out.push({
        guard,
        set,
        state: validity.state,
        detail: `the \`${set}\` calibration is ${validity.state} — ${validityDetail(validity)}`,
      })
    }
  }
  return out
}

/** Every parameter set, for a robot whose body does not say which apply. */
export function allSets(): string[] {
  return [...PARAMETER_SETS]
}

/**
 * The sets a body can actually hold, from what it has declared.
 *
 * Derived from the capability tokens and the base kind a descriptor already carries rather than
 * from a new field, so a robot does not declare the same fact twice.
 *
 * ── Two different absences, and they are not treated the same ───────────────
 *
 * **Undeclared capabilities** mean nobody said what this body senses, so every sensing-derived set
 * is included: over-reporting what is missing is the conservative direction, and quietly deciding a
 * guard was never relevant is not.
 *
 * **A declared `fixed` base** is not an absence at all — it is a positive statement that this body
 * cannot fall over. `balance` is meaningless on a bolted-down arm, so an IMU bias and a gravity
 * vector are not measurements it is missing; they are measurements it will never take, and
 * reporting `balance` unavailable on one is a permanent red mark nobody can clear. §45.2 makes the
 * same point about the gravity vector directly: it drifts with *base tilt*, and a fixed base does
 * not tilt.
 *
 * An **unknown** base is mobile, because that is the absence again and it goes the other way. A
 * body that does declare an IMU keeps both sets whatever its base, since it evidently has one.
 */
export function setsFor(capabilities: string[], base: string | null): string[] {
  const mobile = base === null || base === undefined ? true : base !== 'fixed'
  const declared = capabilities.length > 0
  const has = (prefix: string) => capabilities.some(c => c.startsWith(prefix))

  const sets = ['joint_zeros']
  if (!declared || has('sensing.rgb') || has('sensing.depth') || has('sensing.camera')) {
    sets.push('camera_intrinsics', 'camera_extrinsics')
  }
  if (!declared || has('sensing.force') || has('manipulation')) {
    sets.push('force_torque_bias', 'tool_offset')
  }
  if (mobile || has('sensing.imu')) {
    sets.push('imu_bias', 'gravity_vector')
  }
  return sets
}
