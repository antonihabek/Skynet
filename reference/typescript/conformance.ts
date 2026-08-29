/**
 * SCTS — the Skynet Compatibility Test Suite.
 *
 * This is the module that decides whether "universal" is a claim or a fact.
 *
 * Android did not become universal by running everywhere. It became universal because Google
 * wrote down what an Android device must do, built a suite that measured it, and refused the
 * trademark and the app store to anything that failed. "Is this Android?" stopped being a
 * marketing question and became a test run with a pass or a fail at the end of it. Every part of
 * the ecosystem — developers targeting one API, users expecting apps to work — rests on that one
 * mechanism.
 *
 * Robotics has no equivalent. "ROS 2 compatible" is a sentence anybody may write about anything.
 * There is no artifact a buyer can demand, no number a vendor can be held to, and consequently a
 * policy published for one robot is a gamble on every other. SCTS is the missing mechanism, and
 * the reason certification sits in the operating system rather than in the marketplace: only the
 * thing running on the robot can measure estop latency.
 *
 * ── Three suites, because they measure different kinds of truth ─────────────
 *
 *   sim    Deterministic, free, runs in CI on every commit. Proves the *contract* — that the
 *          driver answers the RHAL correctly, that the descriptor parses, that the kernel
 *          reaches the specified verdict on a given cycle vector.
 *   bench  On the physical robot, once, with a human present. Proves the *machine* — that the
 *          estop actually cuts drive power within the latency the datasheet claims, that the
 *          watchdog faults when commands stop, that the joint rated for 80 Nm refuses 90.
 *   field  Continuous, from telemetry, forever. Proves it *stayed* true — because a robot that
 *          passed a bench test in March and has been drifting since is a certificate describing
 *          a machine that no longer exists.
 *
 * The third is the one nobody else does, and it is where certification stops being a document
 * and becomes a live property. A certificate that cannot expire is a certificate that lies with
 * increasing confidence over time.
 */

export const SCTS_SUITES = ['sim', 'bench', 'field'] as const
export type SctsSuite = (typeof SCTS_SUITES)[number]

export const SCTS_AREAS = [
  'rhal', 'embodiment', 'safety', 'package', 'ota', 'telemetry', 'offline', 'security',
] as const
export type SctsArea = (typeof SCTS_AREAS)[number]

export interface SctsTest {
  id: string
  suite: SctsSuite
  area: SctsArea
  /** What is measured, in the terms a vendor engineer would use to reproduce it. */
  measures: string
  /**
   * Lowest tier that must pass this. `guest` tests are the portable core every integration
   * passes; `certified` tests require hardware most robots do not have.
   */
  required_from: 'guest' | 'host' | 'certified'
  /** Whether a pass is a measured number rather than a boolean. */
  quantitative?: boolean
  /** Unit of the measurement, when quantitative. */
  unit?: string
}

/**
 * The suite.
 *
 * Deliberately short and load-bearing. A conformance suite that tests three hundred things is a
 * suite nobody runs and every vendor games; one that tests the two dozen properties the rest of
 * the system actually depends on is one that can be required. Every test here exists because
 * some other part of Skynet OS would be unsound without it.
 */
export const SCTS_TESTS: SctsTest[] = [
  // ── RHAL: the vendor interface ──────────────────────────────────────────
  { id: 'rhal.abi.version', suite: 'sim', area: 'rhal', required_from: 'guest',
    measures: 'The driver reports an RHAL ABI version the running OS supports, and refuses to load against one it does not.' },
  { id: 'rhal.state.completeness', suite: 'sim', area: 'rhal', required_from: 'guest',
    measures: 'Every joint in the embodiment descriptor appears in the state the driver returns, with matching names.' },
  { id: 'rhal.state.rate', suite: 'bench', area: 'rhal', required_from: 'host', quantitative: true, unit: 'Hz',
    measures: 'Sustained state-read rate over ten minutes, and the worst-case gap between two successive reads.' },
  { id: 'rhal.command.latency', suite: 'bench', area: 'rhal', required_from: 'host', quantitative: true, unit: 'ms',
    measures: 'Time from a command accepted at the RHAL boundary to measurable joint motion. The number the descriptor must declare.' },
  { id: 'rhal.command.rejection', suite: 'sim', area: 'rhal', required_from: 'guest',
    measures: 'The driver rejects a command naming a joint that does not exist, rather than ignoring it.' },

  // ── Embodiment: the body as an interface ────────────────────────────────
  { id: 'embodiment.descriptor.valid', suite: 'sim', area: 'embodiment', required_from: 'guest',
    measures: 'The descriptor parses, its degrees of freedom match its joint list, and every declared role is well formed.' },
  { id: 'embodiment.descriptor.signed', suite: 'sim', area: 'embodiment', required_from: 'host',
    measures: 'The descriptor carries a signature that verifies against a key registered to the vendor namespace.' },
  { id: 'embodiment.limits.sourced', suite: 'sim', area: 'embodiment', required_from: 'host',
    measures: 'Every declared limit cites a source. A rating with no provenance fails, because the safety kernel would be enforcing a rumour.' },
  { id: 'embodiment.limits.honest', suite: 'bench', area: 'embodiment', required_from: 'certified', quantitative: true, unit: '%',
    measures: 'Measured joint velocity and torque against the declared ratings under load. A body that cannot reach what it claims fails as surely as one that exceeds it.' },
  { id: 'embodiment.roles.coverage', suite: 'sim', area: 'embodiment', required_from: 'host', quantitative: true, unit: '%',
    measures: 'Share of actuated joints carrying a canonical role. Below full coverage, cross-body transport degrades to exact-name matching.' },

  // ── Safety: the claims that must not be wrong ───────────────────────────
  { id: 'safety.kernel.parity', suite: 'sim', area: 'safety', required_from: 'guest',
    measures: 'The on-robot kernel and the reference implementation return identical verdicts across the full cycle-vector corpus.' },
  { id: 'safety.watchdog.fires', suite: 'bench', area: 'safety', required_from: 'host', quantitative: true, unit: 'ms',
    measures: 'Commands are stopped mid-motion; time until the robot reaches a controlled stop, against the declared watchdog window.' },
  { id: 'safety.envelope.clamps', suite: 'bench', area: 'safety', required_from: 'host',
    measures: 'A command at 150% of a joint rating is issued; the joint never exceeds its rating, measured at the joint rather than reported by the driver.' },
  { id: 'safety.estop.latency', suite: 'bench', area: 'safety', required_from: 'host', quantitative: true, unit: 'ms',
    measures: 'Time from estop assertion to zero commanded torque. Measured at the actuator; a driver reporting success is not evidence.' },
  { id: 'safety.estop.independent', suite: 'bench', area: 'safety', required_from: 'certified', quantitative: true, unit: 'ms',
    measures: 'The application processor is halted mid-motion. The robot must still stop, proving the stop path does not run through it.' },
  { id: 'safety.ratelimit.enforced', suite: 'bench', area: 'safety', required_from: 'host',
    measures: 'A single-cycle step from zero to full scale produces a ramp at the configured jerk ceiling, not a step.' },
  { id: 'safety.policy.isolation', suite: 'sim', area: 'safety', required_from: 'guest',
    measures: 'A sandboxed package holding an actuator file descriptor is refused; the sandbox has no path to one.' },

  // ── Packages ────────────────────────────────────────────────────────────
  { id: 'package.signature.required', suite: 'sim', area: 'package', required_from: 'guest',
    measures: 'An unsigned package, and one whose bytes do not match its digest, are both refused.' },
  { id: 'package.budget.enforced', suite: 'bench', area: 'package', required_from: 'host',
    measures: 'A package exceeding its declared memory budget is stopped rather than allowed to exhaust the robot.' },
  { id: 'package.grants.gate', suite: 'sim', area: 'package', required_from: 'guest',
    measures: 'A package commands a capability the operator did not grant; the mode gate refuses it and the refusal is logged.' },

  // ── OTA ─────────────────────────────────────────────────────────────────
  { id: 'ota.rollback.works', suite: 'bench', area: 'ota', required_from: 'host',
    measures: 'A deliberately broken image is staged; the robot boots it, fails its health check, and returns to the previous slot unattended.' },
  { id: 'ota.safe_state.respected', suite: 'bench', area: 'ota', required_from: 'host',
    measures: 'An update issued mid-task is staged and not applied until the robot reaches a declared safe state.' },
  { id: 'ota.safety_gate', suite: 'sim', area: 'ota', required_from: 'host',
    measures: 'An update changing the safety kernel or the envelope is refused without a fresh bench pass. OTA must not be a route around certification.' },

  // ── Telemetry and the black box ─────────────────────────────────────────
  { id: 'telemetry.blackbox.complete', suite: 'bench', area: 'telemetry', required_from: 'host',
    measures: 'Every cycle in a ten-minute run appears in the black box with observation hash, policy output, kernel verdict and final command.' },
  { id: 'telemetry.replay.deterministic', suite: 'sim', area: 'telemetry', required_from: 'host',
    measures: 'Replaying a sealed recording through the same policy digest and embodiment digest reproduces the same actions.' },
  { id: 'telemetry.blackbox.survives', suite: 'bench', area: 'telemetry', required_from: 'certified',
    measures: 'Power is cut mid-motion; the black box up to the last completed cycle is intact after reboot.' },

  // ── Offline: the property the whole design rests on ─────────────────────
  { id: 'offline.control.independent', suite: 'bench', area: 'offline', required_from: 'guest',
    measures: 'The network is removed mid-task. The robot completes the task. No control decision may require a round trip.' },
  { id: 'offline.reconcile.resumes', suite: 'bench', area: 'offline', required_from: 'host',
    measures: 'After a week offline the robot reconciles to desired state and uploads its backlog without operator action.' },

  // ── Security ────────────────────────────────────────────────────────────
  { id: 'security.identity.attested', suite: 'sim', area: 'security', required_from: 'host',
    measures: 'The device key is held in a secure element or TPM and check-ins carry an attestation of the running image digest.' },
  { id: 'security.credential.scope', suite: 'sim', area: 'security', required_from: 'guest',
    measures: 'A device credential lifted from the robot cannot read repositories, claim jobs, or write to another namespace.' },
]

// ── Results and certification ────────────────────────────────────────────────

export type SctsOutcome = 'pass' | 'fail' | 'skipped' | 'not_applicable'

export interface SctsResult {
  test_id: string
  outcome: SctsOutcome
  /** The measured figure, for quantitative tests. */
  value?: number
  /** When it ran. Certification decays from the oldest bench result, not the newest. */
  ran_at: string
  /** Free text from the runner — the failure, or the conditions of the measurement. */
  detail?: string
}

export interface CertificationInput {
  /** Tier the vendor is claiming. */
  claimed: 'guest' | 'host' | 'certified'
  results: SctsResult[]
  /** Field conformance over the reporting window, 0–1. Undefined when no fleet data exists yet. */
  field_conformance?: number
  /** Age of the oldest bench result in days. */
  bench_age_days?: number
  /**
   * Whether the body is simulated — `EmbodimentDescriptor.simulator` (§35.3, §70).
   *
   * Absent means false: a real robot should not have to declare what it is not, and the obligation
   * to declare sits on the simulator driver. Read from the descriptor rather than from the driver's
   * declared ceiling, because a simulator driver written in C++ has no language ceiling to fall
   * back on.
   */
  simulator?: boolean
}

export interface CertificationVerdict {
  /** Tier actually awarded. Never above what was claimed, often below. */
  awarded: 'none' | 'guest' | 'host' | 'certified'
  /** Tests required for the claimed tier that did not pass. */
  failures: string[]
  /** Required tests with no result at all. Absence is not a pass. */
  missing: string[]
  /** Why the award landed where it did, for the badge's tooltip and the audit log. */
  reasons: string[]
  /** Whether the award needs a re-run to stay valid, and by when. */
  expires_in_days: number | null
}

const TIER_ORDER = ['guest', 'host', 'certified'] as const

/** Tests required at a tier: its own, plus every tier below it. */
export function testsRequiredFor(tier: 'guest' | 'host' | 'certified'): SctsTest[] {
  const ceiling = TIER_ORDER.indexOf(tier)
  return SCTS_TESTS.filter(test => TIER_ORDER.indexOf(test.required_from) <= ceiling)
}

/**
 * How long a bench result stands before it must be re-measured.
 *
 * A year, because bench testing a humanoid is expensive and a shorter window would make
 * certification something vendors avoid rather than seek. Field conformance is what carries the
 * claim in between — it is measured continuously and can revoke the award long before the year
 * is up.
 */
export const BENCH_VALIDITY_DAYS = 365

/**
 * Field conformance below which an award is suspended.
 *
 * 0.999 rather than 1.0: a fleet of a hundred robots running at 100 Hz produces tens of millions
 * of cycles a day, and demanding literal perfection would suspend every honest vendor on sensor
 * noise while telling operators nothing. What this threshold actually catches is a systematic
 * drift — a robot whose real envelope has moved away from its declared one.
 */
export const FIELD_CONFORMANCE_FLOOR = 0.999

/**
 * Award a tier from the evidence.
 *
 * Two rules make this worth having. **Absence is not a pass** — a required test with no result
 * is a `missing`, never a silent success, which is the loophole every self-certification scheme
 * eventually dies of. And **the award is capped by the evidence, not by the claim** — a vendor
 * claiming the certified tier whose independent stop path was never measured is awarded `host`,
 * with the reason recorded, rather than refused outright. Downgrading tells the truth and keeps
 * the vendor in the programme; refusing sends them to go write "ROS 2 compatible" on a web page
 * instead, which helps nobody.
 */
export function awardCertification(input: CertificationInput): CertificationVerdict {
  const reasons: string[] = []
  let best: CertificationVerdict['awarded'] = 'none'
  let failures: string[] = []
  let missing: string[] = []

  const byId = new Map(input.results.map(result => [result.test_id, result]))
  const claimedIndex = TIER_ORDER.indexOf(input.claimed)

  // Walk upward and keep the highest tier fully satisfied, so the evidence sets the ceiling.
  for (let i = 0; i <= claimedIndex; i += 1) {
    const tier = TIER_ORDER[i]
    const required = testsRequiredFor(tier)
    const tierFailures: string[] = []
    const tierMissing: string[] = []

    for (const test of required) {
      const result = byId.get(test.id)
      if (!result || result.outcome === 'skipped') {
        tierMissing.push(test.id)
      } else if (result.outcome === 'fail') {
        tierFailures.push(test.id)
      }
      // `not_applicable` passes: the balance guard on a wheeled base is genuinely inapplicable,
      // and forcing a fail there would push vendors to fake a result rather than declare a shape.
    }

    if (tierFailures.length === 0 && tierMissing.length === 0) {
      best = tier
      failures = []
      missing = []
    } else {
      failures = tierFailures
      missing = tierMissing
      if (i <= claimedIndex) {
        reasons.push(
          `${tier}: ${tierFailures.length} failing, ${tierMissing.length} unmeasured.`,
        )
      }
      break
    }
  }

  if (best !== 'none' && input.claimed !== best) {
    reasons.push(`Claimed ${input.claimed}; the evidence supports ${best}.`)
  }

  // §35.3 and §70: a simulator RHAL driver declares itself one, and the declaration caps the award
  // at guest. Not because a simulated run is worthless — the sim half of SCTS is load-bearing
  // infrastructure and cannot run without these drivers — but because no simulated run may ever be
  // presented as bench evidence, and a `host` badge on a body that does not exist is exactly that.
  //
  // Applied *after* the evidence walk rather than by refusing to walk, so the verdict still records
  // which tier the tests would have supported. "The evidence supports host; the body is simulated,
  // so guest" is a more useful sentence to a vendor than a silent guest, and it is the same
  // downgrade-and-say-why rule the award is built on.
  if (input.simulator === true && (best === 'host' || best === 'certified')) {
    reasons.push(
      `The body is simulated (§35.3), so the award is capped at guest; the evidence supports ${best}.`,
    )
    best = 'guest'
  }

  // Field conformance suspends an award rather than lowering it. A robot whose commands have
  // been leaving its envelope is not a lower-tier robot — it is one whose certificate no longer
  // describes it, and the honest state is suspended pending re-measurement.
  if (input.field_conformance !== undefined && input.field_conformance < FIELD_CONFORMANCE_FLOOR && best !== 'none') {
    reasons.push(
      `Field conformance ${(input.field_conformance * 100).toFixed(3)}% is below the ${(FIELD_CONFORMANCE_FLOOR * 100).toFixed(1)}% floor; the award is suspended pending re-measurement.`,
    )
    best = 'none'
  }

  let expires: number | null = null
  if (best !== 'none' && input.bench_age_days !== undefined) {
    expires = BENCH_VALIDITY_DAYS - input.bench_age_days
    if (expires <= 0) {
      reasons.push('Bench results are older than a year; the award has lapsed.')
      best = 'none'
      expires = 0
    }
  }

  return { awarded: best, failures, missing, reasons, expires_in_days: expires }
}

/**
 * Field conformance from a reporting window.
 *
 * The ratio of cycles the kernel did not have to intervene in. Not "violations detected", which
 * would reward a robot with a broken detector — a robot reporting zero cycles reports no
 * conformance at all, and the caller gets `undefined` rather than a flattering 1.0.
 */
export function fieldConformance(window: { cycles: number; interventions: number }): number | undefined {
  if (window.cycles <= 0) return undefined
  return Math.max(0, Math.min(1, (window.cycles - window.interventions) / window.cycles))
}
