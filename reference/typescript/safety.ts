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
import type { ControlMode, TransportClamp, TransportPlan } from './embodiment.ts'
import { compareUtf8 } from './canonical.ts'

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
import {
  adjacent, capsuleInWorld, collisionBodies, distance, ignores, margin, segmentDistanceSquared,
  solve, staticallyResolvable,
  type Frame, type JointBinding, type WorkspaceLayer,
} from './kinematics.ts'

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
    requires: ['profile.workspace', 'embodiment.frames'],
  },
  self_collision: {
    guard: 'self_collision',
    checks: 'Commanded configuration against the body\'s own collision model, one step lookahead.',
    response: 'hold',
    requires: ['embodiment.frames[].collision'],
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
  /**
   * Narrower travel for named joints, `[min, max]` in radians (§7.3, DRY-COVERAGE-PLAN.md F159).
   *
   * A travel composes by **intersection** rather than by minimum, because it has two ends: the
   * plan's travel and this one each narrow it and neither can widen it. A range that reaches past
   * the plan's is recorded (`travel_widened`) and its wider ends discarded; a range that does not
   * overlap the plan's at all leaves nothing to clamp to (`travel_empty`), and a policy is refused
   * rather than held at a point.
   */
  joint_travel?: Record<string, [number, number]>
  /**
   * Where the operator will allow named frames to be (§7.1's fourth guard).
   *
   * Layers compose by conjunction, so an added layer can only narrow — see `WorkspaceLayer`, where
   * the reasoning lives with the geometry.
   */
  workspace?: WorkspaceLayer[]
  /**
   * How close two collision bodies may come before the fifth guard holds, in metres.
   *
   * Absent means zero, which is a legal setting and a late one: the surfaces have to actually meet
   * before anything happens, and by then the command that put them there has been sent. The
   * `no_collision_margin` notice says so, as the missing jerk ceiling does.
   */
  self_collision_margin_m?: number
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
  /**
   * The mode each bound joint is commanded in (§86.3), from the transport plan.
   *
   * Without it the two whole-body figures above could not be applied to a joint at all — nothing at
   * this layer knew whether a channel carried radians per second or newton-metres — so they were
   * resolved, reported, and never enforced. See `ceilingFor`.
   */
  joint_modes: Record<string, ControlMode>
  /**
   * The travel in force for each joint, `[min, max]` (§7.1, F159): the plan's intersected with the
   * profile's. Held only on a channel commanded in position — a travel is radians, and a velocity
   * or torque command is not. A joint whose ranges do not overlap has no entry here, and a
   * `travel_empty` notice instead.
   */
  travel: Record<string, [number, number]>
  /** Guards that will actually run, given what this robot can supply. */
  active_guards: Guard[]
  /** Guards specified but inoperable here, with the input that is missing. */
  unavailable_guards: Array<{ guard: Guard; missing: string[] }>
  /**
   * Guards whose declared inputs all resolve and whose per-cycle reading nothing produces (§80).
   *
   * The third state, and it exists because the first two could not describe a real robot. §7.2
   * partitioned guards into *active* and *unavailable*, and `available.people_detection` opened a
   * gap between them: a body that declares a person detector resolves `proximity` **active**, every
   * input the resolver can see is present, and no cycle ever carries `nearest_person_m` — so the
   * guard is reported as running and will never trip.
   *
   * That matters because *active* is what every surface renders as **protection**. A console, a
   * check-in, a device page and `skynet limits` all draw this partition, and all four would show a
   * robot as guarded against approach by a guard that cannot fire.
   *
   * It is not `unavailable`, and folding it there was the tempting simplification. `unavailable`
   * means *an input this robot does not have*, and its remedy is to fit the sensor or write the
   * profile. This one's inputs are all present and its remedy is a producer in the operating
   * system, which is a different sentence to a different person.
   *
   * `reading` names the per-cycle field nothing fills, for §64's reason: a refusal that does not
   * say what to fix is a defect rather than a message.
   */
  inert_guards: Array<{ guard: Guard; reading: string }>
  /** Provenance of the numbers. Null when the envelope declared none. */
  source: string | null
  /**
   * Kind of the numbers (§5.3): `declared` or `measured`, following the provenance — the
   * envelope's basis when the source is the envelope's, null when it fell through to the
   * transport plan's. Null claims no kind for a digest.
   */
  basis: string | null
  /** Things an operator must read before this runs. */
  notices: SafetyNotice[]
  /**
   * Speed-and-separation thresholds, when the operator configured them.
   *
   * Carried on the resolved set rather than read from the profile at cycle time, so `resolveLimits`
   * stays the only place a limit is chosen. Its absence here is what silently disabled the
   * proximity guard in this edition: `partitionGuards` reported the guard active from the profile
   * while `evaluateCycle` had no thresholds to compare against, and no corpus case set a separation
   * distance, so nothing failed.
   */
  proximity: { stop_distance_m: number; slow_distance_m: number; slow_factor: number } | null
  /** The body's frames, flattened and ordered (§44.3), when it declares a tree. */
  frames: Frame[] | null
  /** The operator's allowed volumes, layered. */
  workspace: WorkspaceLayer[]
  /** The configured self-collision margin, in metres. Null is zero, with a notice. */
  self_collision_margin_m: number | null
  /** Capability tokens the operator granted the running package, for the mode gate. */
  grants: string[]
}

/**
 * §38.2's fifth layer: a time-boxed narrowing somebody applied at the machine.
 *
 * The layering is `vendor defaults → fleet profile → site profile → cell profile → session
 * override`, and this is the last of them. A profile says "in this cell, half speed" and is signed
 * (§38.1); this says "half speed for the next twenty minutes while I have my hands near it", and
 * routing that through a Hub and a signing key would mean nobody used it — they would move the
 * robot at full speed instead, which is the outcome the layer exists to prevent.
 *
 * What makes it safe unsigned is that it can **only narrow**: `resolveLimits()` composes it by
 * minimum with everything above it, so the worst a forged one can do is slow the robot down.
 *
 * It carries no deadline, and that is deliberate. §38.2 requires it to expire, and the supervisor
 * holds the clock (§35.2 keeps time out of this layer entirely) — so an expired override is simply
 * never passed in. This type is only ever handed overrides that are still in force.
 */
export interface SessionOverride {
  name: string
  joint_velocity_max_rad_s?: number
  joint_torque_max_nm?: number
  base_velocity_max_m_s?: number
  /** Narrowed jerk ceiling. The one field here that can turn a guard on rather than off. */
  max_delta_per_cycle?: number
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
  | 'no_collision_margin'
  | 'session_override'
  | 'session_widened'
  // §7.3, F159.
  | 'travel_widened'
  | 'travel_empty'

export interface SafetyNotice {
  code: SafetyNoticeCode
  message: string
  subjects?: string[]
  /** Which guard the notice is about, where it is about one. */
  guard?: Guard
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
  /**
   * §38.2's session override, or null when none is in force **or it has expired**.
   *
   * The caller decides which of those two it is, because the caller has the clock.
   */
  session?: SessionOverride | null
  /**
   * The body's frames, flattened (§44.3), or null where it declares no tree.
   *
   * Absent, null and empty are three different bodies in the descriptor and all three arrive here
   * as null, because all three mean the same thing to a guard: there is nothing to compute a pose
   * from. Which of the three it was is the descriptor validator's story, told once.
   */
  frames?: Frame[] | null
  /**
   * Capabilities the robot can actually supply, for deciding which guards can run.
   *
   * Three flags, and it used to be five. `solvers` and `collision_model` went when §44's frame
   * graph arrived: both were claims about the body that nothing checked, and a descriptor saying
   * `collision_model: true` with no geometry in it produced a guard reported as running that had
   * nothing to evaluate. `solvers` was worse than unchecked — it named an *inverse*-kinematics
   * solver, and §44.2 gives the `workspace` guard forward kinematics the kernel computes itself.
   */
  available: {
    imu?: boolean
    people_detection?: boolean
    legged?: boolean
    /**
     * Whether anything on this robot fills the per-cycle proximity reading (§80).
     *
     * A different fact from `people_detection`, and the distinction is the whole of the third
     * state. `people_detection` is the **body's** claim, out of §5's descriptor: this machine has
     * a sensor that detects people. This is the **operating system's**: something running here
     * fills `cycle.nearest_person_m` every cycle.
     *
     * A robot can have the first without the second, and every robot that exists does — which is
     * exactly the case §7.2's two states could not express. Absent is false, because a caller that
     * has not said has not established it and a guard is never given the benefit of the doubt.
     */
    proximity_reading?: boolean
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
  const { tier, envelope, transport, profile, session } = params
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

  const { travel, travelWidened, travelEmpty } = resolveTravel(transport?.travel ?? {}, profile?.joint_travel ?? {})
  if (travelWidened.length > 0) {
    notices.push({
      code: 'travel_widened',
      message: `The operator profile "${profile?.name}" asks to widen the travel of a joint past what the body declares. The wider ends are ignored; the travel in force is where the two ranges overlap.`,
      subjects: travelWidened,
    })
  }
  if (travelEmpty.length > 0) {
    notices.push({
      code: 'travel_empty',
      message:
        'No position is inside every range that bounds the travel of these joints, so there is ' +
        'nothing to clamp a command to. A policy is refused rather than held at a point.',
      subjects: travelEmpty,
    })
  }

  // §38.2's session layer, checked against **what the layer above it allows** rather than against
  // the envelope alone — that is the section's own wording, and the layer above a session is
  // everything already composed. A session asking for 3 rad/s under a profile allowing 1 has not
  // widened the envelope; it has widened the thing actually in force, which is the number the
  // operator can see and the one they will believe they changed.
  //
  // Reported separately from `profile_widened`: they are different sentences about different
  // documents, and telling somebody their signed profile over-reached when it was their bench
  // override would send them to edit a file that is not the problem.
  const sessionWidened: string[] = []
  if (session) {
    const above = (
      e?: number,
      t?: number,
      p?: number,
    ) => narrowest(e, t, p)
    const checkSession = (label: string, requested?: number, ceiling?: number | null) => {
      if (requested !== undefined && ceiling !== null && ceiling !== undefined && requested > ceiling) {
        sessionWidened.push(label)
      }
    }
    checkSession(
      'joint_velocity_max_rad_s',
      session.joint_velocity_max_rad_s,
      above(
        envelope?.joint_velocity_max_rad_s,
        transport?.joint_velocity_max_rad_s,
        profile?.joint_velocity_max_rad_s,
      ),
    )
    checkSession(
      'joint_torque_max_nm',
      session.joint_torque_max_nm,
      above(
        envelope?.joint_torque_max_nm,
        transport?.joint_torque_max_nm,
        profile?.joint_torque_max_nm,
      ),
    )
    checkSession(
      'base_velocity_max_m_s',
      session.base_velocity_max_m_s,
      above(
        envelope?.base_velocity_max_m_s,
        transport?.base_velocity_max_m_s,
        profile?.base_velocity_max_m_s,
      ),
    )
    checkSession(
      'max_delta_per_cycle',
      session.max_delta_per_cycle,
      profile?.max_delta_per_cycle,
    )
  }
  if (sessionWidened.length > 0) {
    notices.push({
      code: 'session_widened',
      message:
        'The session override asks to raise limits above what is already in force. Those values ' +
        'are ignored; the narrower ceiling still applies.',
      subjects: sessionWidened,
    })
  }

  // The session joins the same minimum as every other layer, which is what makes "it only ever
  // narrows" a property of the arithmetic rather than a rule somebody has to remember.
  const jointVelocity = narrowest(
    envelope?.joint_velocity_max_rad_s,
    transport?.joint_velocity_max_rad_s,
    profile?.joint_velocity_max_rad_s,
    session?.joint_velocity_max_rad_s,
  )
  const jointTorque = narrowest(
    envelope?.joint_torque_max_nm,
    transport?.joint_torque_max_nm,
    profile?.joint_torque_max_nm,
    session?.joint_torque_max_nm,
  )
  const baseVelocity = narrowest(
    envelope?.base_velocity_max_m_s,
    transport?.base_velocity_max_m_s,
    profile?.base_velocity_max_m_s,
    session?.base_velocity_max_m_s,
  )

  const maxDelta = narrowest(profile?.max_delta_per_cycle, session?.max_delta_per_cycle)
  if (maxDelta === null) {
    notices.push({
      code: 'no_rate_limit',
      message:
        'No jerk ceiling is configured, so a single-cycle jump from zero to the clamped maximum is ' +
        'permitted. Set max_delta_per_cycle before running a policy near people.',
    })
  }

  // §38.2's hazard, stated rather than left to somebody noticing a small number: "a 'maintenance
  // mode, half speed' that outlives the maintenance is a hazard that looks like a setting." A robot
  // under an override announces it, and names it so the sentence says what to clear.
  if (session) {
    notices.push({
      code: 'session_override',
      message:
        `A session override "${session.name}" is narrowing the limits in force. It is a temporary ` +
        'setting rather than this robot\'s configuration — clear it when the work is done.',
      subjects: [session.name],
    })
  }

  const { active, unavailable, inert } = partitionGuards(params, maxDelta)

  // Emitted only when the guard it is about will actually run. A robot with no collision geometry
  // is already being told the guard is unavailable, and adding "and its margin is unset" to that is
  // a second sentence about a thing that is not happening.
  if (active.includes('self_collision') && profile?.self_collision_margin_m === undefined) {
    notices.push({
      code: 'no_collision_margin',
      message:
        "No self-collision margin is configured, so two of this robot's own bodies must actually " +
        'meet before the guard holds. Set self_collision_margin_m to stop the command that would ' +
        'bring them together.',
      guard: 'self_collision',
    })
  }

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
    joint_modes: transport?.joint_modes ?? {},
    travel,
    proximity: profile?.proximity ?? null,
    frames: params.frames ?? null,
    workspace: profile?.workspace ?? [],
    self_collision_margin_m: profile?.self_collision_margin_m ?? null,
    grants: [],
    active_guards: active,
    unavailable_guards: unavailable,
    inert_guards: inert,
    source: envelope?.source ?? transport?.source ?? null,
    // The basis follows the provenance, not the envelope: when the rendered source is the
    // envelope's, so is the basis (§5.3); when it fell through to the transport plan's, no
    // basis is claimed for a digest.
    basis: envelope?.source ? (envelope.basis ?? 'declared') : null,
    notices,
  }
}

/**
 * §7.3's rule for travel: intersection, not minimum (F159).
 *
 * Every joint either side names, in UTF-8 order so the subjects are the same list on every edition.
 * A range is used only when both of its ends are finite — the same rule `narrowest` applies to a
 * single figure — so a malformed one narrows nothing rather than everything. The plan's range comes
 * from the body's own declaration; the profile's can narrow it, and where it reaches past either end
 * that is a widening, recorded and discarded. An intersection with no inside is not entered: the
 * notice names the joint and `canStart` refuses.
 */
function resolveTravel(
  plan: Record<string, [number, number]>,
  profile: Record<string, [number, number]>,
): { travel: Record<string, [number, number]>; travelWidened: string[]; travelEmpty: string[] } {
  const usable = (range: [number, number] | undefined): range is [number, number] =>
    range !== undefined && Number.isFinite(range[0]) && Number.isFinite(range[1])
  const travel: Record<string, [number, number]> = {}
  const travelWidened: string[] = []
  const travelEmpty: string[] = []
  const names = [...new Set([...Object.keys(plan), ...Object.keys(profile)])].sort(compareUtf8)
  for (const name of names) {
    const declared = usable(plan[name]) ? plan[name] : undefined
    const narrowed = usable(profile[name]) ? profile[name] : undefined
    if (declared && narrowed && (narrowed[0] < declared[0] || narrowed[1] > declared[1])) {
      travelWidened.push(name)
    }
    const ranges = [declared, narrowed].filter((range): range is [number, number] => range !== undefined)
    if (ranges.length === 0) continue
    const low = Math.max(...ranges.map(range => range[0]))
    const high = Math.min(...ranges.map(range => range[1]))
    if (low > high) {
      travelEmpty.push(name)
      continue
    }
    travel[name] = [low, high]
  }
  return { travel, travelWidened, travelEmpty }
}

function partitionGuards(
  params: ResolveLimitsParams,
  maxDelta: number | null,
): {
  active: Guard[]
  unavailable: Array<{ guard: Guard; missing: string[] }>
  inert: Array<{ guard: Guard; reading: string }>
} {
  const active: Guard[] = []
  const unavailable: Array<{ guard: Guard; missing: string[] }> = []
  const inert: Array<{ guard: Guard; reading: string }> = []
  const { envelope, profile, available } = params
  const frames = params.frames ?? null

  const missingFor = (guard: Guard): string[] => {
    switch (guard) {
      case 'watchdog':
        return envelope?.watchdog_ms ? [] : ['envelope.watchdog_ms']
      case 'envelope':
        return []
      case 'rate_limit':
        return maxDelta === null ? ['profile.max_delta_per_cycle'] : []
      // §44.5: a body whose descriptor declares no frame graph, or whose graph does not reach the
      // frame the operator constrained, reports these two unavailable **with the missing frame
      // named** — rather than evaluating them against a frame it guessed.
      case 'workspace': {
        const missing: string[] = []
        const layers = profile?.workspace ?? []
        if (layers.reduce((count, layer) => count + layer.volumes.length, 0) === 0) {
          missing.push('profile.workspace')
        }
        if (!frames) {
          missing.push('embodiment.frames')
        } else {
          for (const layer of layers) {
            for (const volume of layer.volumes) {
              const index = frames.findIndex(frame => frame.name === volume.frame)
              if ((index < 0 || !staticallyResolvable(frames, index)) && !missing.includes(volume.frame)) {
                missing.push(volume.frame)
              }
            }
          }
        }
        return missing
      }
      case 'self_collision':
        if (!frames) return ['embodiment.frames']
        // Two, not one. A single collision body has nothing to collide with, and a guard reported
        // active with no pair to test is a guard that passes by not looking.
        return collisionBodies(frames) < 2
          ? ['embodiment.frames[].collision, on two or more frames']
          : []
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

  // Which per-cycle reading each guard acts on, for the guards whose reading can be absent while
  // every declared input is present. Only `proximity` today: the other seven act on the command,
  // the previous command, the clock or the frame graph, all of which the cycle always carries.
  //
  // A table rather than an `if` in the loop, so that adding a guard with the same shape is adding
  // a row — and so the field name lives beside the guard it belongs to rather than in a branch.
  const readingFor = (guard: Guard): { reading: string, produced: boolean } | null => {
    if (guard !== 'proximity') return null
    return { reading: 'cycle.nearest_person_m', produced: available.proximity_reading === true }
  }

  for (const guard of GUARDS) {
    const missing = missingFor(guard)
    if (missing.length > 0) {
      unavailable.push({ guard, missing })
      continue
    }
    // Checked **after** `missing`, and the order is the point: a guard with no sensor is
    // unavailable, not inert. Reporting a robot that has no person detector as "the reading is not
    // coming" would describe a missing producer on a machine whose problem is a missing sensor.
    const reading = readingFor(guard)
    if (reading && !reading.produced) {
      inert.push({ guard, reading: reading.reading })
      continue
    }
    active.push(guard)
  }

  return { active, unavailable, inert }
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
  /**
   * The capability token this command exercises, when the runtime supplied one.
   *
   * Absent means the caller did not say, and the mode gate does not evaluate — it does not mean
   * "no capability". A guard with no input reports unavailable; it never passes by default (§7.2).
   */
  capability?: string
  /** Distance to the nearest detected person, in metres, when something is measuring it. */
  nearest_person_m?: number
}

/** One pose-guard finding, with the frames it is about. */
export interface PoseTrip {
  guard: 'workspace' | 'self_collision'
  /** The frame that left its volume, or the first of the colliding pair. */
  frame: string
  /** The second of the pair, on a self-collision. Null on a workspace excursion. */
  other: string | null
  /**
   * **Negative is a violation**, in metres — one sign convention across both guards.
   *
   * Null when the frame had no pose this cycle, which is a trip in its own right: a guard that
   * cannot establish where a constrained frame is has not shown the command to be safe.
   */
  margin_m: number | null
}

/**
 * What the two pose guards found, and what they were able to look at.
 *
 * Present on every verdict, empty or not. If it were omitted when nothing tripped, an edition that
 * has not implemented the pose guards at all would emit bytes identical to an edition that ran them
 * and found nothing — the "passes by not looking" failure §31.4's corpus exists to prevent.
 */
export interface PoseOutcome {
  /** Whether forward kinematics ran this cycle. */
  evaluated: boolean
  /** Whether every joint value came from this cycle's command. */
  lookahead: boolean
  /** Whether more trips occurred than the report can carry. */
  truncated: boolean
  /** Frames whose pose could not be computed this cycle, named (§44.5). */
  unresolved: string[]
  /** What tripped, in frame order. */
  trips: PoseTrip[]
}

export interface CycleVerdict {
  /** What the kernel will send. Empty under a hold or a stop. */
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
  /** What the two pose guards found. */
  pose: PoseOutcome
}

/** Attach the capability grants the mode gate checks against. */
export function withGrants(limits: ResolvedLimits, grants: string[]): ResolvedLimits {
  return { ...limits, grants }
}

const MAX_POSE_TRIPS = 8

/** Graduated responses, weakest to strongest, following IEC 60204-1. */
const RESPONSE_ORDER: FaultResponse[] = ['clamp', 'hold', 'stop_cat2', 'stop_cat1', 'stop_cat0']

function stronger(current: FaultResponse | null, response: FaultResponse): FaultResponse {
  if (current === null) return response
  return RESPONSE_ORDER.indexOf(current) >= RESPONSE_ORDER.indexOf(response) ? current : response
}

function emptyPose(): PoseOutcome {
  return { evaluated: false, lookahead: false, truncated: false, unresolved: [], trips: [] }
}

/**
 * Evaluate one control cycle.
 *
 * The reference implementation of the hot path. It allocates a few small objects and runs in time
 * linear in the number of channels and quadratic in the number of collision bodies — the Rust on
 * the robot allocates nothing and runs the same arithmetic against a preallocated frame. The
 * conformance corpus feeds identical cycle vectors to both and requires identical verdicts.
 */
export function evaluateCycle(
  cycle: CommandCycle,
  limits: ResolvedLimits,
  envelope: SafetyEnvelope | null,
  lastSequence?: number,
  bindings: JointBinding[] = [],
): CycleVerdict {
  const tripped: Guard[] = []
  const clamped: CycleVerdict['clamped'] = []
  const advisory = limits.enforcement === 'advisory'

  // The strongest response any guard *demanded this cycle* — not the strongest response any tripped
  // guard is capable of demanding, which is a different and weaker thing. The difference is
  // `proximity`, whose §7.1 row reads `stop_cat1` because that is what it does inside the stop
  // distance; between the two distances it scales the command and lets it through, and a verdict
  // deriving its response from the guard's row would say "stopped" about a robot that was not.
  let demanded: FaultResponse | null = null

  // 1. Watchdog. A stale command is discarded, never clamped and forwarded — clamping it would keep
  //    a dead producer's last intention alive at a legal magnitude, which is the failure mode the
  //    watchdog exists to prevent.
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
      pose: emptyPose(),
    }
  }

  const command: Record<string, number> = {}

  // 2. The envelope: a position channel into its travel (F159), then every channel against its
  //    magnitude. Both in one clamp, recorded once with where the command started and where it
  //    ended, because they are one guard and a reader of the record wants the one number that went
  //    out. The magnitude is still applied after the travel: on a plan written here the travel is
  //    always inside it, and on one written anywhere else the narrower of the two must still win.
  for (const [channel, value] of Object.entries(cycle.desired)) {
    const ceiling = ceilingFor(channel, limits)
    const travel = travelFor(channel, limits)
    let next = value
    let held = false
    if (travel !== null && value < travel[0]) {
      next = travel[0]
      held = true
    } else if (travel !== null && value > travel[1]) {
      next = travel[1]
      held = true
    }
    if (ceiling !== null && Math.abs(next) > ceiling) {
      next = Math.sign(next) * ceiling
      held = true
    }
    if (held) {
      if (!tripped.includes('envelope')) tripped.push('envelope')
      demanded = stronger(demanded, 'clamp')
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
        demanded = stronger(demanded, 'clamp')
        clamped.push({ channel, from: value, to: next })
        command[channel] = next
      }
    }
  }

  // 4 and 5. The pose guards, on the configuration the command above would produce.
  //
  //    After clamping and rate limiting, and that ordering is the point: the pose that matters is
  //    the one the robot will actually be commanded into, not the one the policy asked for and the
  //    envelope refused.
  const pose = emptyPose()
  const wantsPose =
    limits.active_guards.includes('workspace') || limits.active_guards.includes('self_collision')
  if (wantsPose && limits.frames) {
    const frames = limits.frames
    const solved = solve(frames, bindings, command)
    pose.evaluated = true
    pose.lookahead = solved.lookahead
    pose.unresolved = solved.unresolved.map(index => frames[index].name)
    if (limits.active_guards.includes('workspace')) {
      checkWorkspace(frames, solved.poses, limits.workspace, pose)
    }
    if (limits.active_guards.includes('self_collision')) {
      checkSelfCollision(frames, solved.poses, limits.self_collision_margin_m ?? 0, pose)
    }
    if (pose.trips.length > MAX_POSE_TRIPS) {
      pose.trips = pose.trips.slice(0, MAX_POSE_TRIPS)
      pose.truncated = true
    }
    for (const trip of pose.trips) {
      if (!tripped.includes(trip.guard)) tripped.push(trip.guard)
      demanded = stronger(demanded, 'hold')
    }
  }

  // 7. Proximity — speed and separation monitoring, when something is measuring separation.
  //
  //    After the magnitude and rate guards because it scales the command those two produced:
  //    applying it first would let a clamp raise a speed the separation had already reduced, and
  //    the composition rule everywhere in this system is that a later stage may only narrow.
  if (limits.active_guards.includes('proximity')) {
    const distanceM = cycle.nearest_person_m
    const prox = limits.proximity
    if (distanceM !== undefined && prox) {
      if (distanceM <= prox.stop_distance_m) {
        if (!tripped.includes('proximity')) tripped.push('proximity')
        return {
          command: advisory ? { ...cycle.desired } : {},
          response: 'stop_cat1',
          tripped,
          clamped,
          advisory,
          pose,
        }
      }
      if (distanceM <= prox.slow_distance_m && prox.slow_factor < 1) {
        if (!tripped.includes('proximity')) tripped.push('proximity')
        // A reduction, not a stop.
        demanded = stronger(demanded, 'clamp')
        for (const [channel, value] of Object.entries(command)) {
          const next = value * prox.slow_factor
          if (next !== value) {
            clamped.push({ channel, from: value, to: next })
            command[channel] = next
          }
        }
      }
    }
  }

  // 8. Mode gate — the capability this command exercises must be one the operator granted.
  //
  //    Enforced at every cycle rather than at start, so a revocation takes effect on the next
  //    command rather than on the next restart (§8.2).
  if (limits.active_guards.includes('mode_gate') && cycle.capability !== undefined) {
    if (!limits.grants.includes(cycle.capability)) {
      if (!tripped.includes('mode_gate')) tripped.push('mode_gate')
      demanded = stronger(demanded, 'hold')
    }
  }

  // A hold means the command does not go out. The rule lives here, once, rather than at each guard
  // that demands one: `hold` is the last safe command being held while the policy is given a cycle
  // to recover, and forwarding a clamped command alongside a `hold` verdict would be the opposite
  // of what it reported.
  const withheld = demanded !== null && RESPONSE_ORDER.indexOf(demanded) >= RESPONSE_ORDER.indexOf('hold')

  return {
    // In advisory mode the desired command is what reaches the robot. Returning the clamped values
    // here would make a guest-tier deployment look enforced in its own logs.
    command: advisory ? { ...cycle.desired } : withheld ? {} : command,
    response: demanded,
    tripped,
    clamped,
    advisory,
    pose,
  }
}

/** §7.1's fourth guard: every constrained frame inside its operator-defined volumes. */
function checkWorkspace(
  frames: Frame[],
  poses: Array<{ translation: [number, number, number] } | null>,
  layers: WorkspaceLayer[],
  pose: PoseOutcome,
): void {
  frames.forEach((frame, index) => {
    // Layers compose by conjunction: the frame must satisfy every layer that names it, and the
    // reported margin is the worst of them. A layer that does not name this frame says nothing
    // about it, so a site rule about one arm does not confine the other.
    let constrained = false
    let worst: number | null = null
    for (const layer of layers) {
      let best: number | null = null
      for (const volume of layer.volumes) {
        if (volume.frame !== frame.name) continue
        constrained = true
        const solved = poses[index]
        if (solved) {
          const value = margin(volume.shape, solved.translation)
          best = best === null || value > best ? value : best
        }
      }
      if (best !== null) worst = worst === null || best < worst ? best : worst
    }

    if (!constrained) return
    const solved = poses[index]
    if (!solved) {
      // Constrained and unlocatable. Not a pass: the guard has not shown the command safe.
      pose.trips.push({ guard: 'workspace', frame: frame.name, other: null, margin_m: null })
      return
    }
    if (worst !== null && worst < 0) {
      pose.trips.push({ guard: 'workspace', frame: frame.name, other: null, margin_m: worst })
    }
  })
}

/**
 * §7.1's fifth guard: no two collision bodies closer than the operator's margin.
 *
 * Quadratic, and the pairs are visited in index order so the reported list is the same on every
 * edition. Adjacent frames and declared exceptions are skipped before any arithmetic — a humanoid's
 * torso and pelvis overlap by design, and a guard that tripped on the robot standing still is a
 * guard an operator switches off.
 */
function checkSelfCollision(
  frames: Frame[],
  poses: Array<{ translation: [number, number, number]; rotation: [number, number, number, number] } | null>,
  marginM: number,
  pose: PoseOutcome,
): void {
  const clearance = marginM > 0 ? marginM : 0
  for (let i = 0; i < frames.length; i += 1) {
    const first = frames[i].collision
    if (!first) continue
    for (let j = i + 1; j < frames.length; j += 1) {
      const second = frames[j].collision
      if (!second) continue
      if (adjacent(frames, i, j) || ignores(frames, i, j)) continue

      const poseI = poses[i]
      const poseJ = poses[j]
      if (!poseI || !poseJ) {
        // One of the pair has no pose. Reported against the pair rather than silently skipped — a
        // collision test that quietly excludes a limb is a test that passes because it did not look.
        pose.trips.push({
          guard: 'self_collision',
          frame: frames[i].name,
          other: frames[j].name,
          margin_m: null,
        })
        continue
      }

      const [a1, b1] = capsuleInWorld(poseI, first)
      const [a2, b2] = capsuleInWorld(poseJ, second)
      const squared = segmentDistanceSquared(a1, b1, a2, b2)
      const threshold = first.radius_m + second.radius_m + clearance
      // The decision is taken in squared distance, so no square root can move a verdict. The root is
      // taken only for the number the report carries.
      if (squared < threshold * threshold) {
        pose.trips.push({
          guard: 'self_collision',
          frame: frames[i].name,
          other: frames[j].name,
          margin_m: distance(squared) - first.radius_m - second.radius_m - clearance,
        })
      }
    }
  }
}

/**
 * The ceiling that applies to one channel.
 *
 * Three sources, narrowest first, and the third is §86.3's. The per-channel figure is the joint's
 * own rating carried by the transport plan and is the tightest thing anybody knows about that
 * joint. `base` is the reserved whole-body channel. What is new is the fall-back: a joint with no
 * rating of its own is still commanded in a **mode**, and the mode says which of the two whole-body
 * figures is in its units.
 *
 * Until §86 that last step was impossible and the code said so — *"nothing at this layer knows
 * whether a given channel carries radians per second or newton-metres"* — so
 * `joint_velocity_max_rad_s` and `joint_torque_max_nm` were resolved through four layers, rendered
 * on the limits page, and applied to nothing. An operator who narrowed a robot's velocity ceiling
 * in a profile changed a number on a screen.
 *
 * **Position is deliberately not in the table.** There is no whole-body position ceiling to fall
 * back to: travel is a per-joint bound with two ends. The plan carries the wider end's magnitude as
 * this joint's `per_channel` entry, and both ends in `travel`, which `travelFor` hands the envelope
 * guard — the magnitude alone admitted every command between the narrow end and the negative of the
 * wide one (F159).
 *
 * **The two compose by narrowing, they do not take precedence over one another** — DRY-COVERAGE-PLAN.md
 * F36. `per_channel` first and the whole-body figure only as a fall-back would let a joint rated at
 * 2 rad/s be commanded at 2 on a robot whose operator narrowed the body to 1.5, which is the one
 * thing `narrowest` exists everywhere else in this file to prevent. It was unreachable before this
 * section because the whole-body figure reached no joint at all.
 */
function ceilingFor(channel: string, limits: ResolvedLimits): number | null {
  const wholeBody = channel === 'base'
    ? limits.base_velocity_max_m_s
    : limits.joint_modes[channel] === 'velocity'
      ? limits.joint_velocity_max_rad_s
      : limits.joint_modes[channel] === 'torque'
        ? limits.joint_torque_max_nm
        : null
  return narrowest(limits.per_channel[channel], wholeBody)
}

/**
 * The travel a position channel is held to, or null where none applies (§7.1, F159).
 *
 * Only a channel commanded in position: a travel is radians, and a velocity or torque command held
 * to one would be compared in the wrong units — the defect F160 found in `--velocity-max`.
 */
function travelFor(channel: string, limits: ResolvedLimits): [number, number] | null {
  if (limits.joint_modes[channel] !== 'position') return null
  return limits.travel[channel] ?? null
}

/**
 * Whether a transport plan and a limit set are safe enough to start.
 *
 * The kernel refuses three things and only three: a plan that could not be resolved, enforcement
 * that was promised and cannot be delivered, and a joint whose travel narrows to nothing (F159) —
 * a command to it has no position to be clamped to, and clamping every one to a point would be a
 * robot that looks like it is running. Everything else — a missing guard, an unsourced limit, a
 * degraded observation — is a notice an operator reads and accepts, because a system that refuses
 * to start on every imperfection is a system that gets bypassed.
 */
export function canStart(plan: TransportPlan, limits: ResolvedLimits): { ok: boolean; reason?: string } {
  if (plan.verdict === 'refused') {
    return { ok: false, reason: 'The policy cannot be transported onto this body.' }
  }
  if (limits.tier !== 'guest' && limits.enforcement === 'advisory') {
    return { ok: false, reason: 'This tier promises enforcement and the kernel resolved to advisory.' }
  }
  if (limits.notices.some(notice => notice.code === 'travel_empty')) {
    return { ok: false, reason: 'A joint\'s travel narrows to nothing, so there is no position a command to it could be clamped to.' }
  }
  return { ok: true }
}
