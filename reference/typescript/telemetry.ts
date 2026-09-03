/**
 * §74 — telemetry, as windows rather than samples.
 *
 * §34.4 gave telemetry a priority, a drop policy, a counter and a paragraph of its own, and neither
 * §17's eight endpoints nor §57's nine gave it a route. §74 adds the tenth, and this module is what
 * crosses it.
 *
 * # A window, not a sample and not a total
 *
 * An item covers a **half-open interval** and carries counts over it. The three obvious
 * alternatives each break a rule that already exists:
 *
 * * **A series of samples** — a week offline produces ten thousand items, which is the state §34.4
 *   collapses check-ins to avoid.
 * * **A running total since boot** — a drop becomes invisible, because the next total absorbs it
 *   and the series stays tidy. That is precisely what §47.4 forbids.
 * * **Counts since the last successful send** — makes delivery load-bearing for correctness, and a
 *   retry that succeeded twice would double-count with no way for the robot to know.
 *
 * Counts over a self-describing window have none of those properties, and they buy §47.4's rule
 * from the *shape of the data*: dropping one loses that window and nothing else, and the loss is
 * visible because the windows no longer tile. A Hub holding `[10:00, 10:05)` and `[10:15, 10:20)`
 * knows exactly what it does not have, without anybody having remembered to annotate the gap.
 *
 * # Two clocks, and both of them
 *
 * §35.1 assigns them: `CLOCK_REALTIME` is for *"correlation with the Hub"* and `CLOCK_MONOTONIC_RAW`
 * is for anything measured. A window does both jobs, so it carries both.
 *
 * Using wall time for the *duration* would let an NTP step change a rate — a clock that jumps
 * forward five minutes mid-window turns four hundred guard trips into a plausible number instead of
 * an alarming one, and nothing anywhere would look wrong. Using monotonic time for the *bounds*
 * would make two robots' windows incomparable.
 *
 * Their disagreement is **reported rather than reconciled**: the skew is a fact about the robot and
 * not a reason to discard the counts.
 *
 * # Pure
 *
 * No clock, no network. Both bounds and the elapsed monotonic time arrive as numbers, so the same
 * functions run on the robot that produced a window and on the Hub deciding what a set of them
 * covers.
 */

import { fieldConformance } from './conformance.ts'

/**
 * What a window may count.
 *
 * A closed set for `reconcile_stages`' reason: a fleet console divides by these, and six spellings
 * of one counter is six columns that each look empty.
 *
 * `cycles` and `interventions` are the two §15's field conformance is computed from, and
 * `guard_trips` is §33.6's aggregate field that a check-in structurally cannot carry — a rate is a
 * count over a window, and §34.4 collapses check-ins because every field in one is about *now*.
 */
export const TELEMETRY_COUNTERS = [
  'cycles',
  'guard_trips',
  'clamps',
  'interventions',
  'estops',
  'package_restarts',
  'queue_dropped',
  'reconcile_passes',
] as const

export type TelemetryCounter = (typeof TELEMETRY_COUNTERS)[number]

/**
 * One counter over a window.
 *
 * `null` is **not measured** and `0` is measured-and-zero, and §33.6's reason for keeping them apart
 * applies here with more force: "no measurement of guard trips" and "no guard trips" render
 * identically as a zero, and the second is the number an operator relaxes on.
 */
export type Counted = number | null

export interface Window {
  /** Wall-clock start, milliseconds since the epoch. §35.1's correlation clock. */
  from_ms?: number
  /** Wall-clock end, exclusive. */
  to_ms?: number
  /**
   * Monotonic milliseconds actually elapsed inside the window. §35.1's measurement clock, and the
   * denominator of every rate below.
   */
  elapsed_ms?: number
  counters?: Partial<Record<TelemetryCounter, Counted>>
}

export const WINDOW_FAULTS = [
  'unbounded',
  'inverted',
  'unmeasured_duration',
  'implausible_duration',
] as const

export type WindowFaultCode = (typeof WINDOW_FAULTS)[number]

export interface WindowFault {
  code: WindowFaultCode
  severity: 'error' | 'warning'
  message: string
}

export interface WindowReport {
  /** Whether this window may be counted at all. */
  usable: boolean
  /** Monotonic milliseconds the rates below are over. */
  elapsed_ms: number
  /**
   * `(to − from) − elapsed`, in milliseconds.
   *
   * Positive is wall time running ahead of the robot's own measurement — a forward NTP step, or a
   * machine that was suspended. Negative is the reverse. Reported, never corrected.
   */
  skew_ms: number
  /** Per-second rates for every counter that was measured. */
  rates: Record<string, number>
  /** Counters nobody measured, sorted. Not the same set as the ones that were zero. */
  unmeasured: string[]
  faults: WindowFault[]
}

function fault(code: WindowFaultCode, severity: 'error' | 'warning', message: string): WindowFault {
  return { code, severity, message }
}

function finite(value: number | undefined | null): value is number {
  return typeof value === 'number' && Number.isFinite(value)
}

/**
 * What one window says, and whether it says anything (§74.2, §74.3).
 *
 * Every check runs and the result carries all of them, for `admitPackage`'s reason: a fleet operator
 * looking at a robot whose telemetry is being refused should get one list rather than one problem
 * per window.
 *
 * # An unusable window is refused rather than partly counted
 *
 * A window with no duration cannot produce a rate, and a window whose bounds are inverted is a
 * robot whose clock moved under it. Counting either would put a number on a fleet page that is not
 * a rate of anything — and the counts are still on the record, because the *item* is kept whether
 * or not the arithmetic is.
 */
export function assessWindow(window: Window): WindowReport {
  const faults: WindowFault[] = []
  const from = window.from_ms
  const to = window.to_ms
  const elapsed = window.elapsed_ms

  const bounded = finite(from) && finite(to)
  if (!bounded) {
    faults.push(fault('unbounded', 'error',
      'This window does not say when it was. §35.1 makes wall time the clock a Hub correlates on, '
      + 'so a window without both bounds cannot be placed beside another robot\'s — and the gap '
      + 'between windows is how a dropped one is noticed at all (§34.4, §47.4).'))
  } else if ((to as number) < (from as number)) {
    faults.push(fault('inverted', 'error',
      'This window ends before it starts. That is a clock that stepped backwards mid-window rather '
      + 'than a counting error, and the counts in it are over an interval nobody can name.'))
  }

  const measured = finite(elapsed) && (elapsed as number) > 0
  if (!measured) {
    faults.push(fault('unmeasured_duration', 'error',
      'This window carries no monotonic duration, so there is nothing to divide the counts by. '
      + '§35.1 keeps rates off wall time on purpose: an NTP step would otherwise turn four hundred '
      + 'guard trips into a plausible number and nothing would look wrong.'))
  }

  const span = bounded ? (to as number) - (from as number) : 0
  const skew = bounded && measured ? span - (elapsed as number) : 0

  // A minute of disagreement over a five-minute window is a machine that was suspended, an NTP
  // correction, or a clock nobody set. It is a fact about the robot and not a reason to throw the
  // counts away, so it is a warning with the number in it.
  // Compared and rendered without rounding: `Math.round` is half toward positive infinity and the
  // Rust edition's helper is half away from zero, and the two disagree on exactly the negative
  // halves a backwards clock produces. Truncation toward zero is the one reading both editions can
  // spell identically without a shared helper — and the corpus would have found the difference.
  if (bounded && measured && (skew > 60_000 || skew < -60_000)) {
    faults.push(fault('implausible_duration', 'warning',
      `Wall time and the monotonic clock disagree by ${Math.trunc(skew / 1000)} s over this window. `
      + 'The counts are kept and the rates are computed from the monotonic duration (§35.1); the '
      + 'disagreement is worth one line on a fleet page, because it is what a suspended machine and '
      + 'a corrected clock both look like.'))
  }

  const rates: Record<string, number> = {}
  const unmeasured: string[] = []
  for (const name of TELEMETRY_COUNTERS) {
    const count = window.counters?.[name] ?? null
    if (count === null || !Number.isFinite(count) || count < 0) {
      unmeasured.push(name)
      continue
    }
    if (measured) rates[name] = count / ((elapsed as number) / 1000)
  }

  return {
    usable: faults.every(entry => entry.severity !== 'error'),
    elapsed_ms: measured ? (elapsed as number) : 0,
    skew_ms: skew,
    rates,
    unmeasured: unmeasured.sort(),
    faults,
  }
}

// ── What a set of windows covers ─────────────────────────────────────────────

export interface Gap {
  from_ms: number
  to_ms: number
}

export interface Coverage {
  /** Monotonic milliseconds actually observed, summed over usable windows. */
  observed_ms: number
  /** Wall-clock milliseconds between the earliest start and the latest end. */
  spanned_ms: number
  /** Intervals inside the span that no window covers, in order. */
  gaps: Gap[]
  /** How many windows were refused by `assessWindow`. */
  unusable: number
  /** Totals per counter, over usable windows that measured it. */
  totals: Record<string, number>
  /** Counters no usable window measured, sorted. */
  unmeasured: string[]
}

/**
 * What a set of windows actually covers (§74.1, §74.5).
 *
 * This is the function §15's field conformance has to be computed through, and the reason is one
 * line: **the denominator is the time that was observed, not the time that elapsed.**
 *
 * A robot that sent windows for one hour out of a day has not demonstrated anything about the other
 * twenty-three, and a Hub that divided by twenty-four hours would report a conformance figure that
 * is arithmetic over a period it did not observe. §15 puts a floor of 0.999 on that figure and
 * suspends an award below it, so the difference between the two denominators is the difference
 * between suspending a robot and not.
 *
 * Overlapping windows are merged rather than summed for the purpose of the span, and their counts
 * are summed anyway — an overlap is a robot that restarted and re-reported, and the honest reading
 * of two counts over overlapping intervals is that both events happened.
 */
export function coverage(windows: readonly Window[]): Coverage {
  const usable: Array<{ from: number; to: number; window: Window }> = []
  let unusable = 0
  let observed = 0

  for (const window of windows) {
    const report = assessWindow(window)
    if (!report.usable) {
      unusable += 1
      continue
    }
    observed += report.elapsed_ms
    usable.push({ from: window.from_ms as number, to: window.to_ms as number, window })
  }

  const totals: Record<string, number> = {}
  const measured = new Set<string>()
  for (const entry of usable) {
    for (const name of TELEMETRY_COUNTERS) {
      const count = entry.window.counters?.[name] ?? null
      if (count === null || !Number.isFinite(count) || count < 0) continue
      totals[name] = (totals[name] ?? 0) + count
      measured.add(name)
    }
  }

  const sorted = [...usable].sort((a, b) => a.from - b.from)
  const gaps: Gap[] = []
  let spanned = 0

  if (sorted.length > 0) {
    spanned = sorted[sorted.length - 1].to - sorted[0].from
    let reach = sorted[0].to
    for (const entry of sorted.slice(1)) {
      if (entry.from > reach) gaps.push({ from_ms: reach, to_ms: entry.from })
      reach = Math.max(reach, entry.to)
    }
    // The last window may end before the latest end seen, when windows overlap out of order.
    spanned = Math.max(0, reach - sorted[0].from)
  }

  return {
    observed_ms: observed,
    spanned_ms: spanned,
    gaps,
    unusable,
    totals,
    unmeasured: TELEMETRY_COUNTERS.filter(name => !measured.has(name)).sort(),
  }
}

/**
 * §15's field conformance, over what was actually observed.
 *
 * `undefined` when nothing counted cycles, which is not the same as 1 and is what
 * `awardCertification` takes for *"no fleet data exists yet"*. A Hub that substituted 1 here would
 * be awarding a certified tier on the strength of a robot that has never reported.
 */
export function conformanceOf(coverageOf: Coverage): number | undefined {
  const cycles = coverageOf.totals.cycles
  if (cycles === undefined || !Number.isFinite(cycles)) return undefined
  // Absent interventions read as zero *only once there are cycles*, and that is not the usual rule.
  // §74.3 keeps unmeasured and zero apart everywhere else — but a window that counted cycles and
  // did not count interventions is a robot whose kernel intervened in none of them, because the
  // counter that would have recorded one is the counter that recorded the cycles.
  const interventions = coverageOf.totals.interventions ?? 0
  // **Through `fieldConformance`, not beside it.** These two functions computed the same ratio,
  // with the same clamp, in two modules, and neither had a caller — so the day one of them gained a
  // rule about a robot whose counters disagree, the other would have kept the old answer and
  // nothing would have compared them. §15's number has one definition and it belongs to the module
  // that checks it against §15's floor.
  return fieldConformance({ cycles, interventions })
}
