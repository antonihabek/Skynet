/**
 * Power and thermal (§36).
 *
 * §36 opens with the sentence the whole module is built around:
 *
 * > A laptop at 3% shows a warning. A humanoid at 3% falls over.
 *
 * So power is handled in the register of the safety envelope rather than as a user-experience
 * concern: four states, a reserve computed rather than guessed at, a fixed order in which work is
 * given up under pressure, and one override that does not consult the fuel gauge at all.
 *
 * ── What this module had to decide, because §36 did not ────────────────────
 *
 * §36 is four short subsections of prose and this is the first code to consume any of it. Five
 * things had to be settled to make it run, and each is called out where it is defined rather than
 * left as a constant somebody has to reverse-engineer.
 *
 * **The band widths.** §36.1 says `low` is "approaching reserve" and `reserve` is "at the reserve
 * budget", which is a shape rather than a number. `RESERVE_BAND` and `LOW_BAND` give it one, and
 * both are multiples of the reserve because the reserve is the only energy scale the body declares
 * — a percentage of pack capacity would mean something different on every robot.
 *
 * **A fifth state.** §36.1's table has four and assumes a battery. An arm bolted to a bench and run
 * off mains has no state of charge, and forcing it into one of the four would either immobilise it
 * (`critical`) or claim a headroom nobody measured (`normal`). `unmetered` is the honest answer.
 *
 * **What a silent gauge is.** Not `critical`. §36.4 overrides the gauge on a **positive**
 * observation from the bus, and missing evidence is never a positive finding anywhere else in this
 * system. It is reported, it withholds new leases, and it does not stop what is running.
 *
 * **How the OS recognises a brownout.** §36.4 says voltage sag arrives as `RHAL_DEGRADED` with a
 * vendor fault code — and §24 says a fault code is "vendor-specific, surfaced verbatim to the black
 * box", which is to say the OS is told never to interpret one. Those two sentences cannot both be
 * honoured by an OS that guesses. `brownout_fault_codes` is the resolution: the vendor who defined
 * the codes declares which of them mean sag, and the OS **matches** rather than interprets.
 *
 * **How deep the ladder is cut.** §36.3 gives an order and no schedule. `shed` gives one, and the
 * dividing line is meaningful: throttling gives up everything that costs nothing to give up, and
 * only thermal `critical` touches the two rungs that change what the robot can do.
 *
 * ── The reserve is a floor, and the report says so ─────────────────────────
 *
 * §36.2 computes the reserve from three things: the declared figures, the current payload, and the
 * distance to the nearest safe state. **One of the three exists on this platform.** So a reserve
 * computed here is the declared energy and nothing else — an under-estimate, which is the direction
 * that runs a robot past the point where it can still sit down. `unaccounted` names the missing
 * terms rather than letting the number pass as complete.
 */

// ── The safe state's provenance (§87.2) ──────────────────────────────────────

/**
 * The two bases a `time_to_safe_state_s` may be declared on (§87.2). There is no third.
 *
 * **`live` is deliberately absent.** A figure the navigation stack supplies per cycle is the
 * attractive design and §87.2 rejects it for two reasons that have nothing to do with accuracy: it
 * puts a planner inside §36's reserve arithmetic, so a skill admitted at one instant is refused at
 * the next for a reason nobody can audit afterwards; and it makes navigation a safety-relevant
 * component with no tier, when §58.3 keeps perception and navigation as packages that §15 does not
 * test and §12's gate does not hold.
 *
 * A vendor who writes `live` has therefore not made a typo — they have proposed the design this
 * section rejected, so [`validateSafeState`] gives it its own fault rather than reporting an
 * unrecognised word and sending them to check their spelling.
 */
export const SAFE_STATE_BASES = ['fixed', 'mapped_worst_case'] as const
export type SafeStateBasis = (typeof SAFE_STATE_BASES)[number]

/** Base kinds that have somewhere to travel to. §5's `BASE_KINDS` minus `fixed`. */
export const MOBILE_BASES = [
  'wheeled', 'tracked', 'legged', 'aerial', 'rail', 'surface', 'submerged',
] as const

export interface SafeStateDeclaration {
  /** `fixed` — it reaches a safe state where it stands — or a worst case over a mapped area. */
  basis: string
  /**
   * The area the worst case was taken over. Required by `mapped_worst_case` and by nothing else: a
   * worst case over nothing is not a worst case.
   */
  area?: string
  /** When somebody measured it. Not checked, and recorded because a figure with no date is one
   * nobody will re-take. */
  measured_at?: string
}

export const SAFE_STATE_FAULTS = [
  'basis_undeclared',
  'basis_refused',
  'basis_unknown',
  'area_undeclared',
  'mapped_on_a_fixed_base',
  'figure_undeclared',
] as const
export type SafeStateFaultCode = (typeof SAFE_STATE_FAULTS)[number]

export interface SafeStateFault {
  code: SafeStateFaultCode
  detail: string
}

/** Whether this base kind has a navigation problem between it and a safe state. */
export function baseIsMobile(base: string | null | undefined): boolean {
  return typeof base === 'string' && (MOBILE_BASES as readonly string[]).includes(base)
}

/**
 * §87.2's checks over the declaration, or null.
 *
 * The first fault in declaration order, like [`validateMotions`] next door and for the same reason:
 * the basis is read before the area, and the area before anything about the figure, because a
 * declaration whose basis is wrong is one whose other fields are being read against the wrong rule.
 *
 * The first rule is the one that keeps every descriptor written before §87 valid: **an absent basis
 * on a `fixed` base is `fixed`.** A robot bolted to a bench should not have to declare what it is
 * not, which is §35.3's argument about `simulator` applied to the same shape. On a mobile base the
 * same silence is a rumour in §5.2's sense, and it is the term the reserve is built from.
 */
export function validateSafeState(
  profile: PowerProfile,
  base: string | null | undefined,
): SafeStateFault | null {
  const declaration = profile.safe_state
  const figure = profile.time_to_safe_state_s
  const hasFigure = figure !== undefined && Number.isFinite(figure) && figure > 0

  if (declaration === undefined || declaration === null) {
    if (!baseIsMobile(base)) return null
    return {
      code: 'basis_undeclared',
      detail: 'A figure with no basis is a rumour (§5.2), and on a body that travels the time to '
        + 'a safe state depends on where it is standing. Declare `fixed` if it stops where it is, '
        + 'or `mapped_worst_case` with the area the worst case was taken over.',
    }
  }

  if (declaration.basis === 'live') {
    return {
      code: 'basis_refused',
      detail: 'A figure the navigation stack supplies per cycle is the design §87.2 rejected: it '
        + "puts a planner inside §36's reserve arithmetic, so a skill admitted at one instant is "
        + 'refused at the next for a reason nobody can audit, and it makes navigation a '
        + 'safety-relevant component with no tier. Declare a worst case over a mapped area '
        + 'instead.',
    }
  }

  if (!(SAFE_STATE_BASES as readonly string[]).includes(declaration.basis)) {
    return {
      code: 'basis_unknown',
      detail: `\`${String(declaration.basis)}\` is not one of fixed or mapped_worst_case, so `
        + 'nothing can read how this figure was arrived at.',
    }
  }

  if (declaration.basis === 'mapped_worst_case') {
    if (!baseIsMobile(base)) {
      return {
        code: 'mapped_on_a_fixed_base',
        detail: 'This base does not travel, so there is no mapped area to take a worst case over. '
          + 'It is a claim about a navigation problem this body does not have, and it arrives by '
          + 'copying a descriptor between machines.',
      }
    }
    if (typeof declaration.area !== 'string' || declaration.area.trim() === '') {
      return {
        code: 'area_undeclared',
        detail: 'A worst case over nothing is not a worst case. Name the area it was taken over, '
          + 'so somebody reading the reserve later knows where the figure stops being true.',
      }
    }
  }

  if (!hasFigure) {
    return {
      code: 'figure_undeclared',
      detail: 'A basis with no `time_to_safe_state_s` beside it describes how a number was '
        + 'arrived at without carrying the number.',
    }
  }

  return null
}

/**
 * Whether this body may attempt the host tier, given what it says about its safe state (§87.2).
 *
 * The AMR-shaped sibling of §46.2's `hostBlockedByCommandLoss`, and it has the same shape for the
 * same reason: the tier is refused where the recovery path has not been established, and the
 * refusal names the fix rather than the rule. Returns the reason, or null.
 *
 * **A `fixed` base is never refused on this ground**, because it never was the problem — §36.2's
 * assumption that the robot knows where a safe state is holds trivially for an arm bolted down, and
 * §43 says so in the row this answers.
 */
export function hostBlockedBySafeState(
  profile: PowerProfile,
  base: string | null | undefined,
): string | null {
  if (!baseIsMobile(base)) return null
  const fault = validateSafeState(profile, base)
  if (fault === null) return null
  // Two sentences rather than one, and neither is assembled out of the other. §64 makes a message
  // that misdescribes what happened a defect, and a refusal built by splicing a clause in front of
  // a detail that already opens with its own subject is exactly how one gets written: the first
  // draft of this read *"this body travels and This body travels, so…"*. So the tier consequence
  // is stated here, the fault explains itself, and the seam between them is a full stop.
  return "§36.2 computes this body's reserve budget from the time it takes to reach a safe state, "
    + `and this body travels, so that figure is one nothing can place (${fault.code}). `
    + `${fault.detail} (§87.2)`
}

// ── What the body declares (§36.2, §52) ──────────────────────────────────────

/**
 * The power and thermal figures a vendor declares.
 *
 * Every field is optional and every absence produces a named unavailability rather than an
 * assumption. `brownout_fault_codes` empty means *this vendor has not said which of its codes mean
 * sag* — not *this body cannot brown out*.
 */
export interface PowerProfile {
  /** How long this body takes to reach its nearest safe state from full rated load. */
  time_to_safe_state_s?: number
  /**
   * How that figure was arrived at (§87.2).
   *
   * A block **about** the figure rather than a second copy of it, because §78 is this repository's
   * record of what two spellings of one fact cost. §36.2's third reserve term is *distance to the
   * nearest safe state*, and on a mobile robot the figure above is a number that depends on where
   * the robot is standing — so §87.2's decision is that it stays a **declaration** with a stated
   * basis, and the live figure a navigation stack could supply is refused.
   */
  safe_state?: SafeStateDeclaration
  /** The energy that takes, watt-hours. The reserve is built from this figure. */
  safe_stop_energy_wh?: number
  /** The pack's usable capacity, watt-hours. */
  pack_capacity_wh?: number
  /** The temperature at which this body begins giving up work (§36.3). */
  thermal_throttle_c?: number
  /** The temperature at which it gives up everything it is allowed to give up. */
  thermal_critical_c?: number
  /** The vendor's own fault codes that mean voltage sag (§36.4). */
  brownout_fault_codes?: readonly number[]
}

/**
 * Whether this body claims a battery at all.
 *
 * A pack is claimed by declaring either its capacity or the energy a safe stop costs — the second
 * because an energy budget in watt-hours is a statement about a pack even when nobody wrote the
 * capacity down. A body that declares neither is a mains machine as far as this module is
 * concerned, and `unmetered` is the correct reading of its silence.
 */
export function claimsAPack(profile: PowerProfile): boolean {
  return profile.pack_capacity_wh !== undefined || profile.safe_stop_energy_wh !== undefined
}

// ── What the robot observed ──────────────────────────────────────────────────

/** One cycle's power and thermal observation. Nothing here is derived and nothing is remembered. */
export interface PowerObservation {
  /** State of charge as the fuel gauge reports it, 0–1. */
  state_of_charge?: number
  /** Energy remaining, watt-hours, where the gauge reports energy rather than a fraction. */
  energy_remaining_wh?: number
  /** The hottest thermal zone, degrees Celsius. */
  temperature_c?: number
  /** How long this body has been above the throttle threshold without dropping back under it. */
  throttled_for_ms?: number
  /** Whether the RHAL raised `RHAL_DEGRADED` this cycle (§24). */
  rhal_degraded?: boolean
  /** The vendor fault code the RHAL surfaced, verbatim (§24). */
  fault_code?: number
}

// ── The reserve (§36.2) ──────────────────────────────────────────────────────

/** §36.2's terms, in the order that section lists them. */
export const RESERVE_TERMS = ['declared_energy', 'payload', 'distance_to_safe_state'] as const

export interface Reserve {
  /** The reserve, watt-hours, or `null` when the body declares nothing to compute one from. */
  wh: number | null
  /** §36.2's terms nothing on this robot supplies. Non-empty means `wh` is a floor. */
  unaccounted: string[]
  /**
   * The basis `distance_to_safe_state` was accounted on, or null when it was not (§87.2).
   *
   * The difference between a term **nobody supplied** and a term **bounded** over an area somebody
   * walked, and a reader who cannot tell the two apart is reading a floor as a measurement. It is
   * the same shape §7.2 uses for a guard whose input nobody declared: the value and the reason it
   * is what it is, together, so neither can be read without the other.
   */
  safe_state_basis: string | null
}

/** Whether every term §36.2 names was accounted for. False on every robot today. */
export function reserveComplete(reserve: Reserve): boolean {
  return reserve.wh !== null && reserve.unaccounted.length === 0
}

/**
 * Compute §36.2's reserve budget from what the body declares.
 *
 * The arithmetic is deliberately the identity: the reserve *is* `safe_stop_energy_wh`, because the
 * other two terms §36.2 names have no source. Scaling it by a plausible payload factor would have
 * been one line and a number nobody could defend — and this is the number that decides when a
 * two-metre humanoid stops walking.
 */
export function reserve(profile: PowerProfile, base?: string | null): Reserve {
  const unaccounted: string[] = []
  const declared = profile.safe_stop_energy_wh
  const usable = declared !== undefined && Number.isFinite(declared) && declared > 0
  if (!usable) unaccounted.push(RESERVE_TERMS[0])
  // `payload` still has no source on this platform. Pushed unconditionally rather than looked up,
  // so the day one arrives this is a visible place to change rather than a silent over-report.
  unaccounted.push(RESERVE_TERMS[1])

  // §87.2. The third term leaves `unaccounted` when the declaration is sound — which is not the
  // same as the figure being measured, and `safe_state_basis` is what keeps the two distinguishable.
  // A caller that passes no base gets the reading this function had before §87: nobody supplied it.
  const basis = base === undefined ? null : safeStateBasis(profile, base)
  if (basis === null) unaccounted.push(RESERVE_TERMS[2])

  return { wh: usable ? declared! : null, unaccounted, safe_state_basis: basis }
}

/**
 * The basis the third reserve term may be accounted on, or null.
 *
 * `fixed` on a fixed base is the reading a bolted-down arm's silence always had, spelled out; every
 * other combination has to be declared and has to pass [`validateSafeState`], because accounting a
 * term against a declaration with a fault in it would be the invented measurement §5.2 forbids.
 */
export function safeStateBasis(
  profile: PowerProfile,
  base: string | null | undefined,
): SafeStateBasis | null {
  if (validateSafeState(profile, base) !== null) return null
  const figure = profile.time_to_safe_state_s
  if (figure === undefined || !Number.isFinite(figure) || figure <= 0) return null
  const declared = profile.safe_state?.basis
  if (declared === 'mapped_worst_case') return 'mapped_worst_case'
  if (declared === 'fixed') return 'fixed'
  return baseIsMobile(base) ? null : 'fixed'
}

// ── The four states, and the fifth (§36.1) ───────────────────────────────────

/**
 * Where the `reserve` band ends, as a multiple of the reserve.
 *
 * A quarter of a reserve wide. §36.1 puts `reserve` "at the reserve budget" and `critical` "below
 * reserve", which as written are a single point and everything under it — and a gauge reporting
 * within a percent of the boundary would then chatter between *begin a controlled stop* and *stop
 * immediately*, on a robot that is mid-ramp.
 */
export const RESERVE_BAND = 1.25

/**
 * Where the `low` band ends, as a multiple of the reserve.
 *
 * **One whole reserve above the reserve band**, and that width is the argument: `low`'s only
 * behaviour is that a running task may finish and no new one may start, so the warning has to
 * arrive with at least one further safe stop's worth of energy in hand or the promise in §36.1's
 * row is one the robot cannot keep.
 */
export const LOW_BAND = 2.25

/** §36.1's states, most capable first, plus the one §36.1 does not have. */
export const POWER_STATES = ['normal', 'low', 'reserve', 'critical', 'unmetered'] as const
export type PowerState = (typeof POWER_STATES)[number]

/** Why the power state is what it is. */
export const POWER_REASONS = [
  'brownout', 'below_reserve', 'at_reserve', 'approaching_reserve', 'above_reserve',
  'no_pack', 'no_reading', 'no_reserve', 'no_capacity', 'reading_unusable',
] as const
export type PowerReason = (typeof POWER_REASONS)[number]

/**
 * Whether §25.2 may grant a new task lease in this state (§36.1).
 *
 * `unmetered` grants, and that is the whole reason the state exists: a bench arm on mains must not
 * be refused work because it has no battery to report on. A *gauge fault* withholds leases, and
 * `grantsNewLeases` on the assessment is where the two are combined.
 */
export function stateGrantsNewLeases(state: PowerState): boolean {
  return state === 'normal' || state === 'unmetered'
}

/** Whether §36.1 requires the robot to be taken to a safe state. */
export function stateRequiresSafeState(state: PowerState): boolean {
  return state === 'reserve' || state === 'critical'
}

/** Whether the stop is immediate rather than after the running task (§36.1's last row). */
export function stateStopsImmediately(state: PowerState): boolean {
  return state === 'critical'
}

/** Whether an operator is notified (§36.1's `low` row, and everything below it). */
export function stateNotifiesOperator(state: PowerState): boolean {
  return state === 'low' || state === 'reserve' || state === 'critical'
}

// ── What is wrong, as distinct from where the state is ───────────────────────

/**
 * A fault in the power subsystem itself, rather than a level of charge.
 *
 * Separate from the state because they answer different questions and an operator needs both: the
 * state says how much energy is left, and a fault says whether that number can be believed.
 */
export const POWER_FAULT_CODES = [
  'gauge_silent', 'capacity_undeclared', 'reserve_undeclared', 'brownout_undeclarable',
  'charge_out_of_range', 'thermal_sustained', 'thermal_critical', 'thermal_undeclared',
] as const
export type PowerFaultCode = (typeof POWER_FAULT_CODES)[number]

export interface PowerFault {
  code: PowerFaultCode
  detail: string
}

/**
 * Whether a fault raises §27's `DEGRADED`.
 *
 * The two thermal ones do, because §36.3 says so in as many words. The declaration gaps do not: a
 * descriptor missing a field is a descriptor to fix, and a robot that reported itself degraded for
 * it would be reporting a paperwork problem in the same column as a hot motor.
 */
export function faultDegrades(code: PowerFaultCode): boolean {
  return code === 'thermal_sustained' || code === 'thermal_critical'
}

/**
 * Whether a fault withholds new task leases.
 *
 * A robot that cannot tell whether it is at reserve should not be handed *new* work, and should not
 * have taken from it the work it is already doing — which is exactly §36.1's `low` behaviour,
 * arrived at from the other direction.
 */
export function faultWithholdsLeases(code: PowerFaultCode): boolean {
  return code === 'gauge_silent' || code === 'capacity_undeclared'
    || code === 'reserve_undeclared' || code === 'charge_out_of_range'
    || code === 'thermal_critical'
}

// ── Thermal (§36.3) ──────────────────────────────────────────────────────────

/**
 * How long above the throttle threshold counts as "sustained" (§36.3).
 *
 * Thirty seconds. §36.3 raises `DEGRADED` on *sustained* throttling and does not say how long, and
 * both mistakes are real: one cycle makes every heavy motion a degraded robot, and ten minutes
 * makes `DEGRADED` a state a thermally limited robot never reaches while spending its afternoon at
 * the throttle point.
 */
export const SUSTAINED_THROTTLE_MS = 30_000

/** §36.3's ladder, weakest claim first. */
export const SHED_ORDER = [
  'telemetry', 'ota', 'blackbox_compression', 'inference_rate', 'non_essential_sensors',
] as const

/** What is never given up, at any pressure (§36.3). */
export const NEVER_SHED = ['rhal', 'kernel', 'control_loop'] as const

/**
 * How many rungs are given up while throttling but not critical.
 *
 * The first three, and the line between three and four is the meaningful one: telemetry, OTA and
 * black-box compression are housekeeping, and giving them up costs the robot nothing it was hired
 * to do. Rungs four and five change what it can *do*.
 */
export const THROTTLE_DEPTH = 3

export const THERMAL_PRESSURES = ['nominal', 'throttling', 'critical'] as const
export type ThermalPressure = (typeof THERMAL_PRESSURES)[number]

/**
 * The pressure a temperature implies, given what the body declares.
 *
 * A temperature with no declared threshold is `nominal` and a fault, not a silent pass: nobody has
 * said what hot is on this body. Guessing 85 °C would be right for a CPU package and wrong for a
 * harmonic drive by fifty degrees.
 */
export function pressure(profile: PowerProfile, temperatureC?: number): ThermalPressure {
  if (temperatureC === undefined || !Number.isFinite(temperatureC)) return 'nominal'
  const critical = profile.thermal_critical_c
  if (critical !== undefined && temperatureC >= critical) return 'critical'
  const throttle = profile.thermal_throttle_c
  if (throttle !== undefined && temperatureC >= throttle) return 'throttling'
  return 'nominal'
}

/**
 * What is given up at this pressure, in `SHED_ORDER` order (§36.3).
 *
 * Never returns anything in `NEVER_SHED`: there is no thermal situation in which giving up the
 * safety envelope is the right trade, and inference is throttled before control rather than beside
 * it.
 */
export function shed(level: ThermalPressure): string[] {
  const depth = level === 'nominal' ? 0 : level === 'throttling' ? THROTTLE_DEPTH : SHED_ORDER.length
  return SHED_ORDER.slice(0, depth) as unknown as string[]
}

/**
 * Whether a package's declared inference budget can still be met (§36.3's last sentence).
 *
 * Returns the sentence when it cannot, and `null` when it can or when the question cannot be asked
 * — no declaration, no measurement, or a measurement that is not finite. A package is never blocked
 * on an absence.
 *
 * The comparison is against the **observed** loop time rather than against the thermal state, which
 * is the honest coupling: thermal throttling is the usual cause of a missed budget and not the only
 * one, and a robot that blocked a package for being warm while its loop was comfortably inside
 * budget would be enforcing a proxy instead of the thing itself.
 */
export function budgetMissed(declaredMs?: number, observedMs?: number): string | null {
  // Template interpolation is ECMAScript `ToString(Number)`, which is the spelling Rust's
  // `number_text` exists to reproduce. Nothing to import: this side is the definition.
  if (declaredMs === undefined || !Number.isFinite(declaredMs) || declaredMs <= 0) return null
  if (observedMs === undefined || !Number.isFinite(observedMs)) return null
  if (observedMs <= declaredMs) return null
  return `this package declares an inference budget of ${declaredMs} ms and the loop `
    + `is taking ${observedMs} ms. §36.3 throttles inference before control, so the `
    + 'loop is late rather than the robot being unsafe — but a policy quietly missing its own '
    + 'declared deadline is what the budget exists to prevent, so it is refused until the '
    + 'measurement comes back inside it.'
}

// ── The assessment ───────────────────────────────────────────────────────────

export interface Assessment {
  state: PowerState
  reason: PowerReason
  reserve: Reserve
  remaining_wh: number | null
  thermal: ThermalPressure
  shed: string[]
  faults: PowerFault[]
}

/** Whether §25.2 may grant a new task lease: the state's answer, narrowed by any fault. */
export function grantsNewLeases(assessment: Assessment): boolean {
  return stateGrantsNewLeases(assessment.state)
    && !assessment.faults.some(fault => faultWithholdsLeases(fault.code))
}

/** Whether §27's `DEGRADED` is raised by power or thermal (§36.3). */
export function degraded(assessment: Assessment): boolean {
  return assessment.faults.some(fault => faultDegrades(fault.code))
}

/**
 * The energy the gauge accounts for, watt-hours.
 *
 * `energy_remaining_wh` wins over a fraction times a capacity, because a gauge reporting energy has
 * been told the pack's capacity by something closer to the pack than a descriptor field. A fraction
 * outside 0–1 produces `null` rather than a clamp.
 */
function remainingWh(profile: PowerProfile, observation: PowerObservation): number | null {
  const energy = observation.energy_remaining_wh
  if (energy !== undefined && Number.isFinite(energy) && energy >= 0) return energy
  const charge = observation.state_of_charge
  if (charge === undefined || !Number.isFinite(charge) || charge < 0 || charge > 1) return null
  const capacity = profile.pack_capacity_wh
  if (capacity === undefined || !Number.isFinite(capacity) || capacity <= 0) return null
  return charge * capacity
}

/**
 * Whether the gauge produced a state of charge that is not a state of charge.
 *
 * Only asked when there is no energy figure, and only about the fraction: an `energy_remaining_wh`
 * that is negative or not finite is already ignored by `remainingWh`.
 */
function chargeUnusable(observation: PowerObservation): boolean {
  const charge = observation.state_of_charge
  if (charge === undefined) return false
  return !Number.isFinite(charge) || charge < 0 || charge > 1
}

/** §36.1's bands. */
function band(remaining: number, budget: number): PowerState {
  if (remaining < budget) return 'critical'
  if (remaining < budget * RESERVE_BAND) return 'reserve'
  if (remaining < budget * LOW_BAND) return 'low'
  return 'normal'
}

/**
 * The reason that goes with `band`. Kept beside it rather than derived from the state, so the
 * distinction between *at reserve* and *below it* — the one §36.1 spends two rows on — survives.
 */
function bandReason(remaining: number, budget: number): PowerReason {
  switch (band(remaining, budget)) {
    case 'critical': return 'below_reserve'
    case 'reserve': return 'at_reserve'
    case 'low': return 'approaching_reserve'
    default: return 'above_reserve'
  }
}

/**
 * Assess §36 for one cycle.
 *
 * The order of the rules is the design, and the first one is what §36.4 is written for.
 *
 * 1. **Brownout.** The bus said sag; the gauge is not consulted, "because a gauge that disagrees
 *    with the bus is a gauge that is wrong". The only rule that ignores a reading it has.
 * 2. **No pack declared.** A mains machine, correctly reporting that its energy is not metered.
 * 3. **No reading.** A body that claims a pack and got nothing back. Not `critical`, and the fault
 *    is what makes it distinguishable from rule 2.
 * 4. **No reserve, or no capacity.** A reading arrived and cannot be compared against anything.
 * 5. **The bands.** The only rule that looks at a number, and last for that reason.
 */
export function assess(
  profile: PowerProfile,
  observation: PowerObservation,
  /**
   * §5's base kind, when the caller knows it (§87.2).
   *
   * Optional, and the omission is not a default: a caller that does not say what this body stands
   * on gets the reading this function had before §87 — `distance_to_safe_state` unaccounted, basis
   * null — rather than the `fixed` reading, because *"nobody told me"* and *"it is bolted down"*
   * are the two things §7.2 exists to keep apart.
   */
  base?: string | null,
): Assessment {
  const budget = reserve(profile, base)
  const thermal = pressure(profile, observation.temperature_c)
  const faults: PowerFault[] = []
  const remaining = remainingWh(profile, observation)
  const claimsPack = claimsAPack(profile)
  const codes = profile.brownout_fault_codes ?? []
  const noReading = observation.state_of_charge === undefined
    && observation.energy_remaining_wh === undefined

  // Rule 1, before the faults are gathered because §36.4's override does not depend on any of them.
  // A brownout with a silent gauge is still a brownout.
  const brownout = observation.rhal_degraded === true
    && observation.fault_code !== undefined
    && codes.includes(observation.fault_code)

  let state: PowerState
  let reason: PowerReason
  if (brownout) {
    state = 'critical'
    reason = 'brownout'
  } else if (!claimsPack) {
    state = 'unmetered'
    reason = 'no_pack'
  } else if (noReading) {
    state = 'unmetered'
    reason = 'no_reading'
  } else if (remaining === null) {
    // Two ways to arrive with no energy figure, and they send an operator to two different places:
    // a gauge whose reading cannot be believed, and a descriptor with no capacity to scale a
    // believable one by.
    state = 'unmetered'
    reason = chargeUnusable(observation) ? 'reading_unusable' : 'no_capacity'
  } else if (budget.wh === null) {
    state = 'unmetered'
    reason = 'no_reserve'
  } else {
    state = band(remaining, budget.wh)
    reason = bandReason(remaining, budget.wh)
  }

  // The faults, in POWER_FAULT_CODES order so a console renders them the same way every time.
  if (claimsPack && noReading) {
    faults.push({
      code: 'gauge_silent',
      detail: 'this body declares a pack and nothing on this robot read one. The state is '
        + 'reported as unmetered rather than critical — §36.4 overrides the gauge on a positive '
        + 'reading from the bus, and a missing reading is not one — and no new task lease is '
        + 'granted until something answers for the pack (§36.1).',
    })
  }
  if (claimsPack && observation.energy_remaining_wh === undefined
    && observation.state_of_charge !== undefined && profile.pack_capacity_wh === undefined) {
    faults.push({
      code: 'capacity_undeclared',
      detail: 'the gauge reports a fraction and this body declares no `pack_capacity_wh` to turn '
        + 'it into watt-hours, so there is nothing to compare against the reserve. Declare the '
        + 'capacity, or report `energy_remaining_wh` from the gauge (§36.2).',
    })
  }
  if (claimsPack && budget.wh === null) {
    faults.push({
      code: 'reserve_undeclared',
      detail: 'this body declares a pack and no `safe_stop_energy_wh`, so §36.2\'s reserve cannot '
        + 'be computed and the four states have no boundary. It is the figure '
        + '`power.safe_state.reachable` measures, and the one that decides when this robot stops.',
    })
  }
  if (observation.rhal_degraded === true && codes.length === 0) {
    faults.push({
      code: 'brownout_undeclarable',
      detail: 'the RHAL is reporting `RHAL_DEGRADED` and this body declares no '
        + '`brownout_fault_codes`, so nothing here can tell a voltage sag from a dead encoder. '
        + '§24 makes a fault code vendor-specific and surfaced verbatim, which is precisely why '
        + 'the vendor rather than the OS has to name the codes that mean sag (§36.4).',
    })
  }
  const charge = observation.state_of_charge
  if (charge !== undefined && (!Number.isFinite(charge) || charge < 0 || charge > 1)) {
    faults.push({
      code: 'charge_out_of_range',
      detail: `the gauge reports a state of charge of ${charge}, and a state of `
        + 'charge is a fraction between 0 and 1. The reading is discarded rather than clamped: a '
        + 'clamped 1.4 is a full battery, and this one is a gauge nobody should be trusting yet '
        + '(§36.1).',
    })
  }
  const throttledFor = observation.throttled_for_ms ?? 0
  if (thermal === 'throttling' && throttledFor >= SUSTAINED_THROTTLE_MS) {
    faults.push({
      code: 'thermal_sustained',
      detail: `this body has been throttling for ${Math.floor(throttledFor / 1000)} s. `
        + `§36.3 raises DEGRADED on sustained throttling, and ${THROTTLE_DEPTH} of the five rungs `
        + 'are given up — never the RHAL, the kernel or the control loop.',
    })
  }
  if (thermal === 'critical') {
    faults.push({
      code: 'thermal_critical',
      detail: 'this body is at or above its declared critical temperature. Every rung of §36.3\'s '
        + 'ladder is given up, inference included; the RHAL, the kernel and the control loop are '
        + 'not, because there is no thermal situation in which giving up the safety envelope is '
        + 'the right trade.',
    })
  }
  if (observation.temperature_c !== undefined && Number.isFinite(observation.temperature_c)
    && profile.thermal_throttle_c === undefined && profile.thermal_critical_c === undefined) {
    faults.push({
      code: 'thermal_undeclared',
      detail: 'something is reading a temperature and this body declares neither '
        + '`thermal_throttle_c` nor `thermal_critical_c`, so nothing here knows what hot is on '
        + 'this machine. The reading is reported and nothing is shed: 85 °C is right for a CPU '
        + 'package and fifty degrees wrong for a harmonic drive (§36.3).',
    })
  }

  return { state, reason, reserve: budget, remaining_wh: remaining, thermal, shed: shed(thermal), faults }
}
