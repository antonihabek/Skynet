/**
 * OS readiness — what the robot registry actually supports claiming today.
 *
 * The certification suite (`conformance.ts`) awards a tier from *measurements*: an estop latency
 * taken at the actuator, a joint that refused 150% of its rating, a rollback that recovered
 * unattended. None of those exist until somebody runs a bench suite against real hardware.
 *
 * This module answers the question that can be answered now, from the registry alone: **given
 * what has been declared about this robot, which tier could it even attempt?** A robot with no
 * declared safety envelope cannot attempt the host tier, not because it failed a test but because
 * there is nothing for the Safety Kernel to enforce.
 *
 * ── Why this is a separate concept from certification ───────────────────────
 *
 * Collapsing the two would produce the exact lie the platform is built to avoid: a page reading
 * "host tier" over a robot nobody has ever measured. So the vocabulary is deliberately different.
 * This module returns an **attainable** tier and a list of **blockers**; `awardCertification()`
 * returns an *awarded* tier. Attainable means "the paperwork is in order and you may now go and
 * measure it". It is never evidence of anything about the machine.
 *
 * The distinction has a pleasant side effect: the blockers are a to-do list. A vendor looking at
 * "declares no watchdog window" knows precisely what to publish next, which is more useful than a
 * red cross.
 */

import type { SafetyEnvelope } from './types.ts'
import type { OsTier } from './safety.ts'

/**
 * The subset of a registry row readiness depends on.
 *
 * Structural rather than the full `SkynetRobot`, so the checks can be exercised directly and so
 * a caller assembling a robot from somewhere other than the registry is not forced to fabricate
 * twenty irrelevant columns.
 */
export interface ReadinessSubject {
  slug: string
  name: string
  dof: number | null
  capabilities: string[]
  control_modes: string[]
  control_hz: number | null
  compute: Record<string, unknown> | null
  /** Null means nobody declared an envelope. Distinct from `{}`, which means one was declared. */
  limits: SafetyEnvelope | null
  /** How many published adapters exist. Zero means nothing can talk to this robot yet. */
  adapter_count: number
}

export const READINESS_CHECKS = [
  'adapter',
  'kinematics',
  'control_modes',
  'control_rate',
  'envelope',
  'envelope_source',
  'watchdog',
  'joint_ratings',
  'estop_software',
  'estop_hardware',
  'onboard_compute',
] as const
export type ReadinessCheck = (typeof READINESS_CHECKS)[number]

export interface CheckResult {
  check: ReadinessCheck
  ok: boolean
  /** Lowest tier that needs this. A failed `guest` check blocks everything above it too. */
  required_for: OsTier
  /** English, for logs and API clients. A UI translates by `check`. */
  detail: string
}

export interface ReadinessReport {
  slug: string
  name: string
  /**
   * Highest tier this robot could *attempt*, given what has been declared.
   *
   * Never a claim about the machine. `null` means it cannot yet attempt even the guest tier,
   * which in practice means no adapter exists.
   */
  attainable: OsTier | null
  checks: CheckResult[]
  /** Failed checks, in tier order — the vendor's to-do list. */
  blockers: CheckResult[]
  /** Share of checks passing, for sorting a fleet-wide view. Not a score anyone should quote. */
  completeness: number
}

const TIER_ORDER: OsTier[] = ['guest', 'host', 'certified']

/**
 * Evaluate one robot.
 *
 * Pure. Every check is a question about a declared value, and a check whose input is absent fails
 * rather than passing — the same rule `awardCertification()` applies to a missing test result, for
 * the same reason. Absence is not evidence of compliance.
 */
export function evaluateReadiness(subject: ReadinessSubject): ReadinessReport {
  const caps = new Set(subject.capabilities ?? [])
  const limits = subject.limits
  const compute = subject.compute ?? {}

  const checks: CheckResult[] = [
    {
      check: 'adapter',
      ok: subject.adapter_count > 0,
      required_for: 'guest',
      detail: subject.adapter_count > 0
        ? `${subject.adapter_count} published adapter${subject.adapter_count === 1 ? '' : 's'}.`
        : 'No published adapter, so nothing can talk to this robot yet.',
    },
    {
      check: 'kinematics',
      ok: typeof subject.dof === 'number' && subject.dof > 0,
      required_for: 'guest',
      detail: subject.dof ? `${subject.dof} degrees of freedom declared.` : 'No degree-of-freedom count declared.',
    },
    {
      check: 'control_modes',
      ok: (subject.control_modes ?? []).length > 0,
      required_for: 'guest',
      detail: (subject.control_modes ?? []).length > 0
        ? `Accepts ${subject.control_modes.join(', ')}.`
        : 'No control modes declared, so no policy can be matched to it.',
    },
    {
      check: 'control_rate',
      ok: typeof subject.control_hz === 'number' && subject.control_hz > 0,
      required_for: 'host',
      detail: subject.control_hz
        ? `Control loop runs at ${subject.control_hz} Hz.`
        : 'No control rate declared, so loop stability cannot be checked against a policy.',
    },
    {
      check: 'envelope',
      // Null and `{}` are deliberately different: nobody looked, versus somebody looked and found
      // nothing. Only the first blocks, and the message says which.
      ok: limits !== null,
      required_for: 'host',
      detail: limits === null
        ? 'No safety envelope declared. There is nothing for the kernel to enforce.'
        : 'A safety envelope has been declared.',
    },
    {
      check: 'envelope_source',
      ok: Boolean(limits?.source),
      required_for: 'host',
      detail: limits?.source
        ? `Limits cite ${limits.source} (${limits.basis ?? 'declared'}).`
        : 'Limits cite no datasheet. A safety limit without provenance is a rumour.',
    },
    {
      check: 'watchdog',
      ok: typeof limits?.watchdog_ms === 'number' && limits.watchdog_ms > 0,
      required_for: 'host',
      detail: limits?.watchdog_ms
        ? `Controller faults after ${limits.watchdog_ms} ms without a command.`
        : 'No watchdog window declared, so the watchdog guard cannot run.',
    },
    {
      check: 'joint_ratings',
      ok: typeof limits?.joint_velocity_max_rad_s === 'number'
        && typeof limits?.joint_torque_max_nm === 'number',
      required_for: 'host',
      detail: limits?.joint_velocity_max_rad_s && limits?.joint_torque_max_nm
        ? `Rated to ${limits.joint_velocity_max_rad_s} rad/s and ${limits.joint_torque_max_nm} Nm.`
        : 'Velocity and torque ceilings are not both declared, so commands cannot be clamped.',
    },
    {
      check: 'estop_software',
      ok: caps.has('safety.estop'),
      required_for: 'guest',
      detail: caps.has('safety.estop')
        ? 'Exposes a software emergency stop.'
        : 'No software emergency stop capability declared.',
    },
    {
      check: 'estop_hardware',
      ok: limits?.estop === true,
      required_for: 'certified',
      detail: limits?.estop === true
        ? 'A hardware emergency stop is fitted.'
        : 'No hardware emergency stop declared. The certified tier needs a stop path off the application processor.',
    },
    {
      check: 'onboard_compute',
      ok: caps.has('platform.onboard_compute') || Object.keys(compute).length > 0,
      required_for: 'host',
      detail: caps.has('platform.onboard_compute') || Object.keys(compute).length > 0
        ? 'Runs inference onboard.'
        : 'No onboard compute declared, so the OS cannot own the control loop here.',
    },
  ]

  const attainable = highestAttainable(checks)
  const blockers = checks
    .filter(check => !check.ok)
    .sort((a, b) => TIER_ORDER.indexOf(a.required_for) - TIER_ORDER.indexOf(b.required_for))

  return {
    slug: subject.slug,
    name: subject.name,
    attainable,
    checks,
    blockers,
    completeness: checks.filter(c => c.ok).length / checks.length,
  }
}

/**
 * Walk upward and stop at the first tier with an unmet requirement.
 *
 * Stopping rather than skipping matters: a robot that satisfies every certified-tier check but
 * lacks a watchdog window is not "certified with a caveat", it is a robot that cannot attempt the
 * host tier. Tiers are cumulative and the walk enforces that.
 */
function highestAttainable(checks: CheckResult[]): OsTier | null {
  let best: OsTier | null = null
  for (const tier of TIER_ORDER) {
    const unmet = checks.some(check => check.required_for === tier && !check.ok)
    if (unmet) return best
    best = tier
  }
  return best
}

export interface FleetReadiness {
  total: number
  /** How many robots could attempt each tier. Robots are counted at their highest tier only. */
  by_tier: Record<'none' | OsTier, number>
  /** The checks failing most often across the registry — where the ecosystem is actually stuck. */
  common_blockers: Array<{ check: ReadinessCheck; robots: number }>
}

/**
 * Aggregate across the registry.
 *
 * `common_blockers` is the number worth putting on a screen: it says whether the ecosystem's
 * problem is missing adapters or missing datasheets, and those imply completely different work.
 */
export function summarizeReadiness(reports: ReadinessReport[]): FleetReadiness {
  const by_tier: Record<'none' | OsTier, number> = { none: 0, guest: 0, host: 0, certified: 0 }
  const counts = new Map<ReadinessCheck, number>()

  for (const report of reports) {
    by_tier[report.attainable ?? 'none'] += 1
    for (const blocker of report.blockers) {
      counts.set(blocker.check, (counts.get(blocker.check) ?? 0) + 1)
    }
  }

  return {
    total: reports.length,
    by_tier,
    common_blockers: [...counts.entries()]
      .map(([check, robots]) => ({ check, robots }))
      .sort((a, b) => b.robots - a.robots),
  }
}
