/**
 * §11 and §17's check-in — the one thing a robot says about itself.
 *
 * > `POST /devices/{id}/checkin { generation, applied, health, attestation }`
 *
 * Four fields in the specification and one rule behind all of them, from §17:
 *
 * > **The platform never claims a robot is running something it has not heard from.** A deployment
 * > is `pending` until a check-in.
 *
 * Everything here exists to make that rule survive contact with a robot that is halfway through
 * converging, or waiting for a safe state, or running software nobody asked for. The check-in is
 * the *only* evidence the Hub has, so a check-in that overstates is a fleet console that lies, and
 * there is no second source to catch it.
 *
 * # A check-in carries no history, and that is what makes it collapsible
 *
 * §34.4 puts check-ins in a class of their own: *"superseded — only the newest matters, so older
 * ones are collapsed rather than queued"*, because *"a week offline should produce one current
 * check-in on reconnect, not ten thousand historical ones."*
 *
 * That collapsing is only safe because **every field here is a statement about now**. No deltas, no
 * counters since last time, no event list. A check-in that carried "twelve guard trips since the
 * last one" would make collapsing lossy, and the queue would be discarding data while believing it
 * was discarding duplicates. The level-triggered rule from §33.3 applies to the report as much as
 * to the loop.
 *
 * # `applied` is not a field the daemon may set
 *
 * §33.3: *"Convergence is one plan being empty, not one pass succeeding. A supervisor that reported
 * 'converged' because every action it applied succeeded would stamp the generation on a robot that
 * has not started the policy yet."* So `applied` is derived, here, from what is left to do — and
 * there turn out to be **four** answers rather than two, because *"still working"* and *"waiting for
 * a safe state that may not come for hours"* are different sentences to put in front of an operator
 * (§33.4).
 *
 * # The generation on the wire is the one the robot applied
 *
 * Never the one it was asked for. That is the single field §17's rule lives or dies on: a check-in
 * echoing the desired generation would mark every deployment applied the moment the robot heard
 * about it, which is precisely the claim the Hub is forbidden from making.
 *
 * # Pure
 *
 * No clock, no network. Ages arrive as durations rather than as timestamps, for §35.2's reason.
 */

import type { Attestation } from './identity.ts'
import { attest } from './identity.ts'

/** What a robot may claim about the generation it was given. */
export const CLAIMS = ['applied', 'converging', 'deferred', 'inconsistent'] as const

export type Claim = (typeof CLAIMS)[number]

/**
 * §33.4's six ordered stages, as a check-in names them.
 *
 * Here rather than left as free strings because a fleet console groups by them: *"forty robots are
 * waiting on stage 1"* is a fact somebody acts on, and *"forty robots reported forty spellings of
 * the OS image stage"* is not.
 */
export const STAGES = [
  'os_image',
  'rhal_driver',
  'package',
  'profile',
  'policy',
  'lease',
] as const

export type Stage = (typeof STAGES)[number]

/** One thing the reconciler has left to do. */
export interface Pending {
  stage: string
  /**
   * Whether it is waiting on §33.4's safe-state barrier rather than simply not done yet.
   *
   * The distinction is the whole reason `deferred` is a claim of its own: an action that has not
   * run is a robot mid-convergence, and an action waiting for a safe state is a robot that will not
   * converge until somebody puts it in one.
   */
  deferred?: boolean
  reason?: string
}

export interface Progress {
  /** What the robot has actually applied. */
  observed_generation?: number
  /** What the desired state asked for. */
  desired_generation?: number
  pending?: Pending[]
}

export interface ClaimVerdict {
  claim: Claim
  /** §17's field. True for `applied` and for nothing else. */
  applied: boolean
  /** The generation this check-in reports having applied. Never the desired one. */
  generation: number
  detail: string
  /** The stages with work left, sorted and deduplicated. Empty on `applied`. */
  blocking: string[]
}

function stagesOf(pending: readonly Pending[]): string[] {
  return [...new Set(pending.map(entry => entry.stage))].sort()
}

/**
 * What this robot is entitled to claim (§17, §33.3, §33.4).
 *
 * The order of the tests is the order of severity, and the last one is the interesting one. An
 * **empty plan with disagreeing generations** is a reconciler that computed a plan closing nothing:
 * the robot is not converged and nothing is going to converge it. Reporting that as `applied` would
 * hide a supervisor bug behind a green tick on a fleet console, which is the one place nobody would
 * go looking for it. It gets a name.
 */
export function claimFor(progress: Progress): ClaimVerdict {
  const observed = progress.observed_generation ?? 0
  const desired = progress.desired_generation ?? 0
  const pending = progress.pending ?? []
  const blocking = stagesOf(pending)

  if (pending.length > 0) {
    const deferred = pending.every(entry => entry.deferred === true)
    if (deferred) {
      return {
        claim: 'deferred',
        applied: false,
        generation: observed,
        detail: `Everything left to do is waiting for a safe state (§33.4): ${blocking.join(', ')}. `
          + 'Nothing here converges until the robot is put in one, which is an operator action '
          + 'rather than a matter of time.',
        blocking,
      }
    }
    return {
      claim: 'converging',
      applied: false,
      generation: observed,
      detail: `Still reconciling generation ${desired}, with work left at ${blocking.join(', ')}. `
        + `This robot is running generation ${observed}.`,
      blocking,
    }
  }

  if (observed !== desired) {
    return {
      claim: 'inconsistent',
      applied: false,
      generation: observed,
      detail: `The plan is empty and this robot has applied generation ${observed} against a `
        + `desired ${desired}. An empty plan that closes nothing is a supervisor that cannot see `
        + 'the difference it is meant to close, and reporting it as applied would put a green tick '
        + 'over it.',
      blocking,
    }
  }

  return {
    claim: 'applied',
    applied: true,
    generation: observed,
    detail: `Generation ${observed} is applied and the plan is empty.`,
    blocking,
  }
}

// ── §33.6's health, and the age rule ─────────────────────────────────────────

/**
 * One aggregate health reading.
 *
 * §33.6 ends with four words that decide this shape: *"Every field carries its age."* A number
 * without one is not a measurement — it is a value a fleet console will render as current, and the
 * battery reading from four hours ago is exactly the one somebody dispatches an engineer on.
 *
 * So a value with no age is **not reported as a value**. See `summarize`.
 */
export interface Measured {
  value?: number | null
  age_ms?: number | null
}

export const READINESS = ['ready', 'stalled', 'not_running', 'unknown'] as const

export type ReadinessState = (typeof READINESS)[number]

export interface PackageHealth {
  id: string
  live?: boolean
  readiness?: ReadinessState
  age_ms?: number
}

/** The named aggregate fields, in the order §33.6 lists them. */
export const HEALTH_FIELDS = [
  'generation_lag',
  'guard_trip_rate',
  'storage_headroom',
  'battery',
  'temperature_c',
] as const

export type HealthField = (typeof HEALTH_FIELDS)[number]

export type HealthInput = {
  packages?: PackageHealth[]
} & Partial<Record<HealthField, Measured>>

export interface HealthReport {
  /** Whether anything here should stop this robot being handed new work. */
  degraded: boolean
  /** Packages that are alive and producing nothing — the finding §33.6 exists for. */
  stalled: string[]
  /** Fields nobody measured, sorted. */
  unmeasured: string[]
  /** Fields that carried a value and no age, sorted. Reported as unmeasured, and named apart. */
  undated: string[]
  packages: Required<PackageHealth>[]
  fields: Record<string, { value: number, age_ms: number } | null>
}

/**
 * Roll health up for a check-in (§33.6).
 *
 * Three rules, and the second is the one this function was written for:
 *
 * * **A stalled package is degraded and an unmeasured field is not.** *"We could not measure the
 *   battery"* is not *"the battery is fine"*, and it is also not *"the robot is unhealthy"* — it is
 *   a gap, and reporting a gap as a fault is how an operator learns to ignore the health screen.
 * * **A value with no age is dropped and named.** §33.6 requires an age on every field, so a
 *   reading that cannot be dated is reported as unmeasured — in `undated` rather than in
 *   `unmeasured`, because *"nobody measured this"* and *"somebody measured this and lost track of
 *   when"* send different people to different places. Passing the number through would be worse
 *   than dropping it: a console renders whatever it is given as current.
 * * **`unknown` readiness is not a stall.** A policy in its first cycle has legitimately produced
 *   nothing, and reporting that as a fault would make every start look like one.
 */
export function summarize(health: HealthInput): HealthReport {
  const packages = (health.packages ?? []).map(entry => ({
    id: entry.id,
    live: entry.live ?? false,
    readiness: entry.readiness ?? 'unknown',
    age_ms: entry.age_ms ?? 0,
  }))

  const stalled = packages
    .filter(entry => entry.readiness === 'stalled')
    .map(entry => entry.id)
    .sort()

  const unmeasured: string[] = []
  const undated: string[] = []
  const fields: Record<string, { value: number, age_ms: number } | null> = {}

  for (const name of HEALTH_FIELDS) {
    const measured = health[name] ?? null
    const value = measured?.value ?? null
    const age = measured?.age_ms ?? null
    if (value === null || !Number.isFinite(value)) {
      unmeasured.push(name)
      fields[name] = null
      continue
    }
    if (age === null || !Number.isFinite(age) || age < 0) {
      undated.push(name)
      fields[name] = null
      continue
    }
    fields[name] = { value, age_ms: age }
  }

  return {
    degraded: stalled.length > 0,
    stalled,
    unmeasured: unmeasured.sort(),
    undated: undated.sort(),
    packages,
    fields,
  }
}

// ── The document ─────────────────────────────────────────────────────────────

/**
 * What the Safety Kernel would enforce on this robot (§7.3, §7.6, §32.1).
 *
 * §32.1 asks a device page for *"active and unavailable guards with the missing input named,
 * resolved limits and their provenance"*, and until this field existed nothing carried them: the
 * robot resolved them, served them on its own LAN console, and the Hub's own route said so in a
 * `gaps` list. A producer with no consumer on one side of §17 and a consumer with no producer on
 * the other, which is the shape Part X was written about.
 *
 * # It is a statement about now, which is what makes it belong here
 *
 * §34.4 collapses check-ins because every field in one describes the present. These do: they are
 * the ceilings in force and the guard partition as of this pass, not a count, a delta or a history.
 * A robot whose profile changed twice while offline should reconnect reporting the limits it is
 * under, and exactly one of them.
 *
 * # `resolved_at` is not a tier claim, and the name is the reason
 *
 * It is *"the tier these limits were resolved at"* — `ResolvedLimits`' own field, under its own
 * name. §15 awards a tier from bench evidence a human witnessed and a robot never awards itself one
 * (§19), so a field called `tier` here would be read as one within a week. §7.6: nothing on a robot
 * decides its tier, the daemon resolves at the floor, and the numbers are identical at all three
 * tiers either way.
 *
 * # `enforcement` and `enforced` are different facts
 *
 * `enforcement` is what the tier *permits* — advisory, enforcing, enforcing-redundant — and is
 * derived from `resolved_at`. `enforced` is whether anything on this robot actually applies these
 * limits to a command, which on an edition with no policy runtime is `false` at every tier. A
 * consumer that had only the first would render a green guard list over a robot enforcing nothing.
 */
export interface GuardReport {
  /** The tier these limits were resolved at. Never a tier claim — see above. */
  resolved_at: string
  /** What that tier permits. */
  enforcement: string
  /** Whether anything on this robot applies them to a command. */
  enforced: boolean
  joint_velocity_max_rad_s: number | null
  joint_torque_max_nm: number | null
  base_velocity_max_m_s: number | null
  min_control_hz: number | null
  max_delta_per_cycle: number | null
  /**
   * How close two collision bodies may come before §7.1's fifth guard holds, in metres.
   *
   * A **floor** among ceilings, and the only number in this block where a larger value is the
   * stricter setting. `null` is not zero-and-safe: the kernel treats an unset margin as zero, which
   * is a legal setting and a late one — the surfaces have to meet before anything happens — and
   * emits `no_collision_margin` to say so.
   */
  self_collision_margin_m: number | null
  /** Where the numbers came from. `null` is a limit set nobody cited a datasheet for. */
  source: string | null
  /** Guards that will run, in evaluation order. */
  active: string[]
  /** Guards specified and inoperable here, each with what it wanted. */
  unavailable: UnavailableGuard[]
  /**
   * §80's third state: every declared input resolves and the per-cycle reading is not coming.
   *
   * Carried across §17 rather than derived on the Hub, for the reason every other field here is: a
   * robot's own reading of its own guards is evidence, and a Hub that recomputed the partition
   * would be a second implementation of `resolveLimits`.
   */
  inert: InertGuard[]
  /**
   * Everything an operator must read, as codes and subjects.
   *
   * **Codes, never sentences.** The Hub renders these in thirty-five locales (§32.3) and a robot
   * that shipped English prose would put the translation on the side that cannot do it. Both
   * editions hold the same catalogue, so the code is enough.
   *
   * `guard_unavailable` is carried here *as well as* in `unavailable`, and the duplication is
   * deliberate: this field is what the kernel produced, and a wire that dropped part of it would be
   * a view rather than evidence. A renderer with the table beside it should filter the code out;
   * that is the renderer's decision to make.
   */
  notices: Notice[]
}

/** A guard that is specified and cannot run here. */
export interface UnavailableGuard {
  guard: string
  /** What it needs and does not have — a missing input, or "not applicable to this body". */
  missing: string[]
}

/**
 * A guard whose declared inputs all resolve and whose per-cycle reading nothing produces (§80).
 *
 * Separate from `UnavailableGuard` because the remedies differ and go to different people. An
 * unavailable guard wants a sensor fitted or a profile written; an inert one has both and wants a
 * producer in the operating system.
 */
export interface InertGuard {
  guard: string
  /** The per-cycle field nothing fills, named so the remedy is legible. */
  reading: string
}

/** One thing an operator must read, as the kernel emitted it. */
export interface Notice {
  /** The code. `NoticeCode`'s wire spelling. */
  code: string
  subjects: string[]
}

export interface CheckInInput {
  device_id?: string
  agent?: string
  contracts?: string
  /** §27's operating state, as the daemon last observed it. */
  state?: string
  progress?: Progress
  health?: HealthInput
  attestation: Attestation
  /**
   * `sha256:…` over the canonical desired-state document this robot is reconciling against.
   *
   * Empty when it holds none — a robot nobody has given one, which is §50.2's shipping state and
   * is not the same as a document that could not be digested.
   *
   * # This is not history, which is the test it had to pass
   *
   * §34.4 collapses check-ins because every field in one is a statement about **now**, and a field
   * about the past would make that collapse lossy while still looking like a duplicate. The digest
   * of the document currently on disk is exactly as present-tense as the `generation` beside it.
   *
   * # Why the generation was not enough
   *
   * §11 rests idempotence on the counter, and `assessOffer` enforces it by refusing a *different*
   * document under a generation this robot has already applied. That refusal is the robot's, and
   * until this field existed it was **only** the robot's: the Hub knew which generation a robot had
   * applied and had no way to ask which *document*. So a Hub restored from a backup — the exact
   * case §11 names — could serve generation 41 with different contents, watch a fleet refuse it,
   * and see nothing but robots that had quietly stopped converging.
   */
  desired_digest?: string
  /**
   * §7.6's resolved limits and guard partition, when this robot composed any.
   *
   * `undefined` is an edition that does not resolve them, and it is not the same as a robot with no
   * ceilings — that one reports a `GuardReport` whose numbers are all null, with the `no_envelope`
   * notice saying why.
   */
  limits?: GuardReport
  /**
   * §23.5's machine readings, as observed — the loop's refusal inputs, reported so a Hub
   * refusing an award has the machine's own measurements rather than the vendor's word.
   *
   * The lock is not among them: `mlockall` is per-process, and the daemon locking itself says
   * nothing about the loop. What travels here is what a supervisor can establish about the
   * machine both processes share: the kernel bit and the swap total.
   */
  machine?: MachineReadings
}

/**
 * What the machine is, for the tier claim (§23.5, as amended).
 *
 * Nulls are unestablished readings, not stock ones: a kernel too old for
 * `/sys/kernel/realtime` is not a kernel proven otherwise, and an unreadable meminfo is not
 * zero swap.
 */
export interface MachineReadings {
  /** Whether the running kernel is PREEMPT_RT. */
  preempt_rt: boolean | null
  /** Kilobytes of active swap (`SwapTotal`). */
  swap_kb: number | null
}

export interface CheckIn {
  claim: ClaimVerdict
  health: HealthReport
  attestation: Attestation
  device_id: string
  agent: string
  contracts: string
  state: string
  pending: Required<Pending>[]
  /** The document behind the generation. See `CheckInInput.desired_digest`. */
  desired_digest: string
  /** §7.6's limits, carried through unchanged. See `GuardReport`. */
  limits: GuardReport | null
  /** §23.5's machine readings, carried through unchanged. See `MachineReadings`. */
  machine: MachineReadings | null
}

/**
 * Build the document a robot sends (§11, §17).
 *
 * This function is the whole of what crosses to the Hub, which is why it is here rather than in the
 * daemon: two editions must produce the **same bytes** for the same robot, or a fleet console shows
 * two robots that differ only in who compiled their supervisor.
 *
 * It cannot fail, for `attest`'s reason, and it is the same reason: the robot most in need of a
 * check-in is the one that cannot measure everything. A report withheld for want of a field is a
 * robot the Hub believes is offline.
 */
export function checkIn(input: CheckInInput): CheckIn {
  const pending = (input.progress?.pending ?? []).map(entry => ({
    stage: entry.stage,
    deferred: entry.deferred ?? false,
    reason: entry.reason ?? '',
  }))
  pending.sort((a, b) => (a.stage < b.stage ? -1 : a.stage > b.stage ? 1 : 0))

  return {
    claim: claimFor(input.progress ?? {}),
    health: summarize(input.health ?? {}),
    attestation: input.attestation,
    device_id: (input.device_id ?? '').trim(),
    agent: (input.agent ?? '').trim(),
    contracts: (input.contracts ?? '').trim(),
    state: (input.state ?? '').trim(),
    pending,
    desired_digest: (input.desired_digest ?? '').trim(),
    // Carried through rather than derived. What the guards are and what they lack is the kernel's
    // answer for this body, and a builder that recomputed any of it would be a second
    // implementation of §7.3 living in the reporting path.
    limits: input.limits ?? null,
    machine: input.machine ?? null,
  }
}

/** The one-line summary a console and a terminal both render. */
export function summary(built: CheckIn): string {
  const report = attest(built.attestation)
  const who = built.device_id === '' ? 'This robot' : built.device_id
  const health = built.health.degraded
    ? `degraded (${built.health.stalled.join(', ')} alive and producing nothing)`
    : 'no package is alive and producing nothing'
  return `${who} reports generation ${built.claim.generation} as ${built.claim.claim}, `
    + `${health}. ${report.sentence}`
}
