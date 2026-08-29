/**
 * The Safety Kernel contract.
 *
 * One invariant holds the entire operating system together:
 *
 *   **The Safety Kernel is the only component with write access to an actuator.**
 *
 * A policy does not command a joint. It writes a *desired* command into a buffer the Safety
 * Kernel reads. The kernel decides what actually reaches the motors. That is not a stylistic
 * preference — it is the only structure under which the rest of the system can be permissive.
 * The artifact scanner already tells us that most published checkpoints are pickles, which
 * execute arbitrary code when they are read. A system where loading a model can move an arm is
 * a system where every published model is a remote code execution vulnerability attached to a
 * two-metre humanoid. Making the kernel the sole writer is what lets Skynet run untrusted
 * policies at all.
 *
 * ── Why this module is small, pure and boring ───────────────────────────────
 *
 * Everything here is arithmetic on declared numbers. No allocation in the decision path, no
 * model, no network, no clock beyond a monotonic tick supplied by the caller. The kernel that
 * ships on the robot is a C implementation of exactly these rules; this TypeScript is the
 * reference the conformance suite tests against, and the thing the Hub uses to show an operator
 * what will be enforced *before* they deploy. Two implementations of one specification, with the
 * suite holding them to each other.
 *
 * ── The honesty rule ────────────────────────────────────────────────────────
 *
 * A clamp is only worth what the scheduler underneath it can promise. Running as an ordinary
 * Linux process alongside a vendor stack, the kernel cannot guarantee it will be scheduled
 * before the next control cycle, so it **cannot** promise enforcement, and this module will not
 * let it claim otherwise. `resolveEnforcement()` derives the mode from the tier rather than
 * accepting one, and the advisory mode is reported everywhere it applies. An OS that displayed
 * "safety enforced" over a best-effort userspace process would be lying about the one thing it
 * must never lie about.
 */

import type { SafetyEnvelope } from './types.ts'
import type { TransportClamp, TransportPlan } from './embodiment.ts'

// ── Deployment tiers ─────────────────────────────────────────────────────────

/**
 * How deeply Skynet OS is installed on a given robot.
 *
 * The tiers exist because "universal" has to mean something defensible. There is no single
 * binary that boots on every robot ever built, and claiming one would be the sort of marketing
 * fiction this platform is built to displace. What *is* universal is the contract: the same
 * packages, the same transport planner, the same Hub, the same conformance suite — reached at
 * three depths of integration, with each tier stating plainly what it can and cannot promise.
 */
export const OS_TIERS = ['guest', 'host', 'certified'] as const
export type OsTier = (typeof OS_TIERS)[number]

export interface TierProfile {
  tier: OsTier
  /** One line, for an operator deciding what they are looking at. */
  summary: string
  /** What the OS runs on top of. */
  substrate: string
  /** Whether the OS owns the real-time control loop. */
  owns_control_loop: boolean
  /** Whether the kernel's decisions are enforced or only recorded. */
  enforcement: EnforcementMode
  /** Whether a stop path exists that does not depend on the application processor. */
  independent_stop_path: boolean
  /** Whether a functional-safety claim can rest on this tier at all. */
  supports_safety_case: boolean
}

/**
 * What the kernel does with a violation.
 *
 * `advisory` is not a weaker enforcement setting; it is the absence of enforcement, recorded
 * honestly. In advisory mode the kernel computes every verdict, logs every violation and raises
 * every alarm — and the command still reaches the vendor stack unchanged, because there is
 * nothing underneath it that the OS controls. The value is real (an operator learns their policy
 * commands 140% of rated torque before it is trusted with anything) and it is not a safety
 * function, and the UI says so in those words.
 */
export const ENFORCEMENT_MODES = ['advisory', 'enforcing', 'enforcing_redundant'] as const
export type EnforcementMode = (typeof ENFORCEMENT_MODES)[number]

export const TIER_PROFILES: Record<OsTier, TierProfile> = {
  guest: {
    tier: 'guest',
    summary: 'Runs beside the robot\'s existing stack. Observes and advises; enforces nothing.',
    substrate: 'A container or process on any compute that can reach the robot over IP, serial or CAN.',
    owns_control_loop: false,
    enforcement: 'advisory',
    independent_stop_path: false,
    supports_safety_case: false,
  },
  host: {
    tier: 'host',
    summary: 'Skynet OS is the robot\'s onboard operating system and owns the control loop.',
    substrate: 'PREEMPT_RT Linux on the robot\'s application processor, driving hardware through an RHAL driver.',
    owns_control_loop: true,
    enforcement: 'enforcing',
    independent_stop_path: false,
    supports_safety_case: false,
  },
  certified: {
    tier: 'certified',
    summary: 'Host tier plus an independent safety processor with its own stop path.',
    substrate: 'PREEMPT_RT Linux for the policy runtime; the kernel mirrored on a separate safety MCU wired into the drive-enable line.',
    owns_control_loop: true,
    enforcement: 'enforcing_redundant',
    independent_stop_path: true,
    // The tier makes a safety case *possible*. It does not make one. See `SAFETY_CLAIM_RULE`.
    supports_safety_case: true,
  },
}

/**
 * The sentence the product is not allowed to shorten.
 *
 * A tier is an architecture, not a certificate. Skynet OS can be a component of somebody's
 * ISO 10218 or ISO 13482 safety case; it cannot confer one, because conformity assessment is
 * done by a notified body against a specific machine in a specific application, and no amount of
 * software architecture substitutes for that. Every surface that mentions the certified tier
 * carries this, and `supports_safety_case` above means "can be evidence in one", nothing more.
 */
export const SAFETY_CLAIM_RULE =
  'Skynet OS supplies evidence for a safety case. It does not certify a machine. Conformity ' +
  'assessment against ISO 10218, ISO 13482 or any other standard is performed by a notified ' +
  'body against a specific robot in a specific application.'

/** Derived, never supplied. A caller cannot pass in the enforcement mode it would prefer. */
export function resolveEnforcement(tier: OsTier): EnforcementMode {
  return TIER_PROFILES[tier].enforcement
}

// ── Guards ───────────────────────────────────────────────────────────────────

/**
 * The guard set, in the order the kernel evaluates it each cycle.
 *
 * Order is part of the specification. The watchdog runs first because a stale command must never
 * be clamped and forwarded — it must be discarded. Rate limiting runs after magnitude clamping
 * because clamping a magnitude can itself create a step change, and the rate limiter is what
 * turns that step into a ramp. Getting this order wrong produces a kernel that passes every unit
 * test and still whipcracks an arm on the first clamp.
 */
export const GUARDS = [
  'watchdog',
  'envelope',
  'rate_limit',
  'workspace',
  'self_collision',
  'balance',
  'proximity',
  'mode_gate',
] as const
export type Guard = (typeof GUARDS)[number]

export interface GuardSpec {
  guard: Guard
  /** What it checks. */
  checks: string
  /** What it does when the check fails. */
  response: FaultResponse
  /**
   * Whether the guard can run at all without data the robot may not have. A guard whose inputs
   * are missing is reported as unavailable rather than reported as passing.
   */
  requires: string[]
}

/**
 * Graduated responses, weakest to strongest.
 *
 * A kernel whose only response is a hard stop is a kernel operators disable, because a humanoid
 * that drops a crate every time a velocity estimate blips is worse than no policy at all. Most
 * violations are clamped and logged; stopping is reserved for the cases where continuing is the
 * dangerous option. The categories follow IEC 60204-1.
 */
export const FAULT_RESPONSES = [
  'clamp',        // Command reduced to the ceiling and forwarded. Logged.
  'hold',         // Last safe command held; the policy is given one cycle to recover.
  'stop_cat2',    // Controlled ramp to zero velocity, power retained, brakes engaged.
  'stop_cat1',    // Controlled ramp to zero, then drive power removed.
  'stop_cat0',    // Drive power removed immediately, uncontrolled.
] as const
export type FaultResponse = (typeof FAULT_RESPONSES)[number]

export const GUARD_SPECS: Record<Guard, GuardSpec> = {
  watchdog: {
    guard: 'watchdog',
    checks: 'A command arrived within the envelope\'s watchdog window and its sequence number advanced.',
    response: 'stop_cat2',
    requires: ['envelope.watchdog_ms'],
  },
  envelope: {
    guard: 'envelope',
    checks: 'Per-joint position, velocity and torque against the declared rating and the transport clamp.',
    response: 'clamp',
    requires: ['embodiment.joints[].limits'],
  },
  rate_limit: {
    guard: 'rate_limit',
    checks: 'Change per cycle against the jerk ceiling — the guard that turns a policy glitch into a ramp.',
    response: 'clamp',
    requires: ['profile.max_delta_per_cycle'],
  },
  workspace: {
    guard: 'workspace',
    checks: 'Commanded end-effector and base pose against operator-defined allowed volumes.',
    response: 'hold',
    requires: ['profile.workspace', 'embodiment.solvers'],
  },
  self_collision: {
    guard: 'self_collision',
    checks: 'Commanded configuration against the body\'s own collision model, one step lookahead.',
    response: 'hold',
    requires: ['embodiment.collision_model'],
  },
  balance: {
    guard: 'balance',
    checks: 'Centre of pressure inside the support polygon; fall-arrest reflex takes the loop when it leaves.',
    response: 'stop_cat2',
    requires: ['sensing.imu', 'embodiment.base=legged'],
  },
  proximity: {
    guard: 'proximity',
    checks: 'Speed and separation monitoring: commanded speed against the distance to the nearest detected person.',
    response: 'stop_cat1',
    requires: ['profile.proximity', 'a sensor that detects people'],
  },
  mode_gate: {
    guard: 'mode_gate',
    checks: 'That the command\'s capability is one the operator granted this package, in this mode.',
    response: 'hold',
    requires: ['package.grants'],
  },
}

// ── The resolved limit set ───────────────────────────────────────────────────

/**
 * An operator's overrides.
 *
 * Only ever downward. A profile may narrow what the vendor rated; it may never widen it, and
 * `resolveLimits()` enforces that by taking the minimum rather than trusting the input. This is
 * the difference between an operator saying "in this cell, half speed" and an operator being
 * able to talk a robot past its own datasheet.
 */
export interface OperatorProfile {
  name: string
  joint_velocity_max_rad_s?: number
  joint_torque_max_nm?: number
  base_velocity_max_m_s?: number
  /** Maximum change per control cycle, as a fraction of the ceiling. Jerk limiting. */
  max_delta_per_cycle?: number
  /** Distance in metres below which the proximity guard caps speed, and its cap. */
  proximity?: { stop_distance_m: number; slow_distance_m: number; slow_factor: number }
  /** Capability tokens withheld regardless of what a package requests. */
  denied_capabilities?: string[]
}

export interface ResolvedLimits {
  tier: OsTier
  enforcement: EnforcementMode
  joint_velocity_max_rad_s: number | null
  joint_torque_max_nm: number | null
  base_velocity_max_m_s: number | null
  min_control_hz: number | null
  max_delta_per_cycle: number | null
  per_channel: Record<string, number>
  /** Guards that will actually run, given what this robot can supply. */
  active_guards: Guard[]
  /** Guards specified but inoperable here, with the input that is missing. */
  unavailable_guards: Array<{ guard: Guard; missing: string[] }>
  /** Provenance of the numbers. Null when the envelope declared none. */
  source: string | null
  /** Things an operator must read before this runs. */
  notices: SafetyNotice[]
}

export type SafetyNoticeCode =
  | 'advisory_only'
  | 'no_envelope'
  | 'no_independent_stop'
  | 'unsourced_limits'
  | 'guard_unavailable'
  | 'profile_widened'
  | 'transport_degraded'
  | 'no_rate_limit'

export interface SafetyNotice {
  code: SafetyNoticeCode
  message: string
  subjects?: string[]
}

/** Narrowest of the values present. Absent inputs never widen the result. */
function narrowest(...values: Array<number | undefined | null>): number | null {
  const present = values.filter((v): v is number => typeof v === 'number' && Number.isFinite(v))
  return present.length === 0 ? null : Math.min(...present)
}

export interface ResolveLimitsParams {
  tier: OsTier
  envelope: SafetyEnvelope | null
  /** The clamp the transport plan derived from the body and the policy's declared peaks. */
  transport: TransportClamp | null
  profile?: OperatorProfile | null
  /** Capabilities the robot can actually supply, for deciding which guards can run. */
  available: {
    imu?: boolean
    people_detection?: boolean
    collision_model?: boolean
    solvers?: boolean
    legged?: boolean
  }
}

/**
 * Compose every source of ceilings into the single set the kernel enforces.
 *
 * Three sources, and the rule between them is always the minimum: the robot's declared envelope,
 * the transport plan's per-channel limits, and the operator's profile. Taking the minimum rather
 * than a precedence order means no source can raise a limit another source lowered — including a
 * malicious or mistaken operator profile, which is why the profile is folded in here rather than
 * applied afterwards.
 */
export function resolveLimits(params: ResolveLimitsParams): ResolvedLimits {
  const { tier, envelope, transport, profile } = params
  const enforcement = resolveEnforcement(tier)
  const notices: SafetyNotice[] = []

  if (enforcement === 'advisory') {
    notices.push({
      code: 'advisory_only',
      message:
        'At the guest tier Skynet OS does not own the control loop. Violations are detected and ' +
        'recorded; commands still reach the robot unchanged. This is not a safety function.',
    })
  }

  if (!TIER_PROFILES[tier].independent_stop_path) {
    notices.push({
      code: 'no_independent_stop',
      message:
        'There is no stop path independent of the application processor. If the processor hangs, ' +
        'nothing here stops the robot.',
    })
  }

  if (envelope === null) {
    notices.push({
      code: 'no_envelope',
      message:
        'No safety envelope has been declared for this robot. Only per-joint ratings from the ' +
        'body description are available, and whole-body ceilings are unenforced.',
    })
  } else if (!envelope.source) {
    notices.push({
      code: 'unsourced_limits',
      message: 'The declared limits cite no datasheet. A safety limit without provenance is a rumour.',
    })
  }

  // Widening attempts are recorded, then discarded by the minimum. Silently ignoring one would
  // leave an operator believing a limit they set is in force.
  const widened: string[] = []
  const checkWiden = (label: string, requested?: number, ceiling?: number | null) => {
    if (requested !== undefined && ceiling !== null && ceiling !== undefined && requested > ceiling) {
      widened.push(label)
    }
  }
  checkWiden('joint_velocity_max_rad_s', profile?.joint_velocity_max_rad_s, envelope?.joint_velocity_max_rad_s)
  checkWiden('joint_torque_max_nm', profile?.joint_torque_max_nm, envelope?.joint_torque_max_nm)
  checkWiden('base_velocity_max_m_s', profile?.base_velocity_max_m_s, envelope?.base_velocity_max_m_s)
  if (widened.length > 0) {
    notices.push({
      code: 'profile_widened',
      message: `The operator profile "${profile?.name}" asks to raise limits above the declared envelope. Those values are ignored.`,
      subjects: widened,
    })
  }

  const jointVelocity = narrowest(
    envelope?.joint_velocity_max_rad_s,
    transport?.joint_velocity_max_rad_s,
    profile?.joint_velocity_max_rad_s,
  )
  const jointTorque = narrowest(
    envelope?.joint_torque_max_nm,
    transport?.joint_torque_max_nm,
    profile?.joint_torque_max_nm,
  )
  const baseVelocity = narrowest(
    envelope?.base_velocity_max_m_s,
    transport?.base_velocity_max_m_s,
    profile?.base_velocity_max_m_s,
  )

  const maxDelta = profile?.max_delta_per_cycle ?? null
  if (maxDelta === null) {
    notices.push({
      code: 'no_rate_limit',
      message:
        'No jerk ceiling is configured, so a single-cycle jump from zero to the clamped maximum is ' +
        'permitted. Set max_delta_per_cycle before running a policy near people.',
    })
  }

  const { active, unavailable } = partitionGuards(params, maxDelta)
  for (const entry of unavailable) {
    notices.push({
      code: 'guard_unavailable',
      message: `The ${entry.guard} guard cannot run on this robot.`,
      subjects: entry.missing,
    })
  }

  return {
    tier,
    enforcement,
    joint_velocity_max_rad_s: jointVelocity,
    joint_torque_max_nm: jointTorque,
    base_velocity_max_m_s: baseVelocity,
    min_control_hz: transport?.min_control_hz ?? null,
    max_delta_per_cycle: maxDelta,
    per_channel: transport?.per_channel ?? {},
    active_guards: active,
    unavailable_guards: unavailable,
    source: envelope?.source ?? transport?.source ?? null,
    notices,
  }
}

function partitionGuards(
  params: ResolveLimitsParams,
  maxDelta: number | null,
): { active: Guard[]; unavailable: Array<{ guard: Guard; missing: string[] }> } {
  const active: Guard[] = []
  const unavailable: Array<{ guard: Guard; missing: string[] }> = []
  const { envelope, profile, available } = params

  const missingFor = (guard: Guard): string[] => {
    switch (guard) {
      case 'watchdog':
        return envelope?.watchdog_ms ? [] : ['envelope.watchdog_ms']
      case 'envelope':
        return []
      case 'rate_limit':
        return maxDelta === null ? ['profile.max_delta_per_cycle'] : []
      case 'workspace':
        return available.solvers ? [] : ['an inverse-kinematics solver']
      case 'self_collision':
        return available.collision_model ? [] : ['a collision model in the body description']
      case 'balance':
        // Not applicable rather than missing on a wheeled base; reported as inactive either way,
        // and the missing list says which, so nobody reads "balance guard off" as a defect.
        if (!available.legged) return ['a legged base — not applicable to this body']
        return available.imu ? [] : ['an IMU']
      case 'proximity':
        if (!profile?.proximity) return ['profile.proximity']
        return available.people_detection ? [] : ['a sensor that detects people']
      case 'mode_gate':
        return []
    }
  }

  for (const guard of GUARDS) {
    const missing = missingFor(guard)
    if (missing.length === 0) active.push(guard)
    else unavailable.push({ guard, missing })
  }

  return { active, unavailable }
}

// ── The per-cycle decision ───────────────────────────────────────────────────

export interface CommandCycle {
  /** Monotonic tick supplied by the caller. The kernel owns no clock. */
  now_ms: number
  /** When the command the policy produced was stamped. */
  command_ms: number
  /** Sequence number; must advance, or the command is a repeat of a stalled producer. */
  sequence: number
  /** Desired values keyed by target joint or actuator name. */
  desired: Record<string, number>
  /** What was actually sent last cycle, for rate limiting. */
  previous?: Record<string, number>
}

export interface CycleVerdict {
  /** What the kernel will send. Empty under a stop. */
  command: Record<string, number>
  response: FaultResponse | null
  /** Guards that tripped, in evaluation order. */
  tripped: Guard[]
  /** Channels whose values were reduced, with the ratio applied. */
  clamped: Array<{ channel: string; from: number; to: number }>
  /**
   * Whether the command was forwarded unchanged despite a trip, because the tier cannot enforce.
   * Always false at host and certified; the field exists so a guest-tier log cannot be mistaken
   * for an enforcement record.
   */
  advisory: boolean
}

/**
 * Evaluate one control cycle.
 *
 * The reference implementation of the hot path. It allocates two small objects and runs in time
 * linear in the number of channels — the C version on the robot allocates nothing and runs the
 * same arithmetic against a preallocated frame. The conformance suite feeds identical cycle
 * vectors to both and requires identical verdicts.
 */
export function evaluateCycle(
  cycle: CommandCycle,
  limits: ResolvedLimits,
  envelope: SafetyEnvelope | null,
  lastSequence?: number,
): CycleVerdict {
  const tripped: Guard[] = []
  const clamped: CycleVerdict['clamped'] = []
  const advisory = limits.enforcement === 'advisory'

  // 1. Watchdog. A stale command is discarded, never clamped and forwarded — clamping it would
  //    keep a dead producer's last intention alive at a legal magnitude, which is the failure
  //    mode the watchdog exists to prevent.
  const watchdogMs = envelope?.watchdog_ms
  const stale = watchdogMs !== undefined && cycle.now_ms - cycle.command_ms > watchdogMs
  const stalled = lastSequence !== undefined && cycle.sequence <= lastSequence
  if (limits.active_guards.includes('watchdog') && (stale || stalled)) {
    tripped.push('watchdog')
    return {
      command: advisory ? { ...cycle.desired } : {},
      response: 'stop_cat2',
      tripped,
      clamped,
      advisory,
    }
  }

  const command: Record<string, number> = {}

  // 2. Magnitude clamping against the resolved ceilings.
  for (const [channel, value] of Object.entries(cycle.desired)) {
    const ceiling = ceilingFor(channel, limits)
    let next = value
    if (ceiling !== null && Math.abs(value) > ceiling) {
      next = Math.sign(value) * ceiling
      if (!tripped.includes('envelope')) tripped.push('envelope')
      clamped.push({ channel, from: value, to: next })
    }
    command[channel] = next
  }

  // 3. Rate limiting, after clamping, because clamping creates step changes of its own.
  if (limits.max_delta_per_cycle !== null && cycle.previous) {
    for (const [channel, value] of Object.entries(command)) {
      const previous = cycle.previous[channel]
      if (previous === undefined) continue
      const ceiling = ceilingFor(channel, limits)
      const maxStep = ceiling === null
        ? Math.abs(previous) * limits.max_delta_per_cycle
        : ceiling * limits.max_delta_per_cycle
      const delta = value - previous
      if (Math.abs(delta) > maxStep && maxStep > 0) {
        const next = previous + Math.sign(delta) * maxStep
        if (!tripped.includes('rate_limit')) tripped.push('rate_limit')
        clamped.push({ channel, from: value, to: next })
        command[channel] = next
      }
    }
  }

  return {
    // In advisory mode the desired command is what reaches the robot. Returning the clamped
    // values here would make a guest-tier deployment look enforced in its own logs.
    command: advisory ? { ...cycle.desired } : command,
    response: tripped.length > 0 ? 'clamp' : null,
    tripped,
    clamped,
    advisory,
  }
}

function ceilingFor(channel: string, limits: ResolvedLimits): number | null {
  const perChannel = limits.per_channel[channel]
  if (perChannel !== undefined) return perChannel
  if (channel === 'base') return limits.base_velocity_max_m_s
  return null
}

/**
 * Whether a transport plan and a limit set are safe enough to start.
 *
 * The kernel refuses two things and only two: a plan that could not be resolved, and enforcement
 * that was promised and cannot be delivered. Everything else — a missing guard, an unsourced
 * limit, a degraded observation — is a notice an operator reads and accepts, because a system
 * that refuses to start on every imperfection is a system that gets bypassed.
 */
export function canStart(plan: TransportPlan, limits: ResolvedLimits): { ok: boolean; reason?: string } {
  if (plan.verdict === 'refused') {
    return { ok: false, reason: 'The policy cannot be transported onto this body.' }
  }
  if (limits.tier !== 'guest' && limits.enforcement === 'advisory') {
    return { ok: false, reason: 'This tier promises enforcement and the kernel resolved to advisory.' }
  }
  return { ok: true }
}
